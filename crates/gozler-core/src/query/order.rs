//! The result order: which of two hits prints first.
//!
//! One comparison serves every caller, so a limited query is always a prefix
//! of the same query without a limit. The engine picks a page with it under
//! one volume's index lock; [`order_hits`] sorts a deferred page and merges
//! several volumes with it after the locks are released.
//!
//! The sort key decides first. `Path` is es's `(parent folder, name)` order,
//! folded, not a byte compare of the whole path; an unknown size or date
//! sorts last in either direction. Ties then break, always ascending, on the
//! folded name, the volume, the file ID, the parent folder's file ID and the
//! raw name. Each key comes from the hit itself, so no path is rebuilt to
//! break a tie.

use super::engine::QueryHit;
use crate::types::{Frn, SortDir, SortKey, SortSpec};
use core::cmp::Ordering;

/// What the result order reads from one hit. The engine reads it from the
/// index, [`order_hits`] from keys computed once per hit.
pub(crate) trait OrderFields {
    /// The folded full path of the hit's parent folder. Read only by
    /// [`SortKey::Path`].
    fn folded_parent(&self) -> &[u8];
    fn folded_name(&self) -> &[u8];
    fn size(&self) -> Option<u64>;
    fn mtime(&self) -> Option<i64>;
    /// Tells volumes apart; constant within one volume.
    fn volume(&self) -> &[u8];
    fn frn(&self) -> Frn;
    fn parent_frn(&self) -> Frn;
    /// The raw name: the last key, which separates two links of one file in
    /// one folder whose names differ only in case.
    fn name(&self) -> &[u8];
}

/// The result order of `a` and `b` under `sort`. Total: two different
/// entries never compare equal.
pub(crate) fn compare<T: OrderFields>(sort: SortSpec, a: &T, b: &T) -> Ordering {
    let primary = match sort.key {
        SortKey::Name => directed(sort.dir, a.folded_name().cmp(b.folded_name())),
        SortKey::Path => directed(
            sort.dir,
            a.folded_parent()
                .cmp(b.folded_parent())
                .then_with(|| a.folded_name().cmp(b.folded_name())),
        ),
        SortKey::Size => unknown_last(sort.dir, a.size(), b.size()),
        SortKey::DateModified => unknown_last(sort.dir, a.mtime(), b.mtime()),
    };
    primary
        .then_with(|| a.folded_name().cmp(b.folded_name()))
        .then_with(|| a.volume().cmp(b.volume()))
        .then_with(|| a.frn().cmp(&b.frn()))
        .then_with(|| a.parent_frn().cmp(&b.parent_frn()))
        .then_with(|| a.name().cmp(b.name()))
}

fn directed(dir: SortDir, ord: Ordering) -> Ordering {
    match dir {
        SortDir::Asc => ord,
        SortDir::Desc => ord.reverse(),
    }
}

fn unknown_last<T: Ord>(dir: SortDir, a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => directed(dir, a.cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// One volume's hits, as the engine returned them.
pub struct VolumeRun<'a> {
    /// The volume's mount prefix; it tells volumes apart for the tie-break.
    pub mount_prefix: &'a str,
    pub hits: Vec<QueryHit>,
    /// [`super::QueryOutcome::sorted`]: whether `hits` is already in result
    /// order.
    pub sorted: bool,
}

/// Puts the hits of one or more volumes into the result order.
///
/// Runs no index code, so the caller calls it after releasing every index
/// lock. Hits a run already holds in order cost a merge, not a full sort.
pub fn order_hits(sort: SortSpec, mut runs: Vec<VolumeRun<'_>>) -> Vec<QueryHit> {
    runs.retain(|run| !run.hits.is_empty());
    if let [run] = runs.as_mut_slice()
        && run.sorted
    {
        return std::mem::take(&mut run.hits);
    }
    let volumes: Vec<&[u8]> = runs.iter().map(|r| r.mount_prefix.as_bytes()).collect();
    let mut folded = Vec::new();
    let mut keys = Vec::new();
    let mut hits = Vec::new();
    for (volume, run) in runs.into_iter().enumerate() {
        for hit in run.hits {
            keys.push(HitKey::new(sort.key, volume, &hit, &mut folded));
            hits.push(Some(hit));
        }
    }
    // Sort positions, not the hits: an export moves millions of rows, and a
    // hit is far larger than its position. The stable sort detects the
    // ascending runs that sorted volumes contribute and merges them.
    let mut order: Vec<usize> = (0..hits.len()).collect();
    let fields = |i: usize| Keyed {
        key: &keys[i],
        hit: hits[i]
            .as_ref()
            .expect("every hit is present until the sort ends"),
        folded: &folded,
        volumes: &volumes,
    };
    order.sort_by(|&a, &b| compare(sort, &fields(a), &fields(b)));
    order
        .into_iter()
        .map(|i| hits[i].take().expect("the order holds each hit once"))
        .collect()
}

/// A hit's folded keys, computed once so the sort does not fold on every
/// comparison. The folded bytes live in one buffer shared by every hit.
struct HitKey {
    volume: usize,
    /// Empty unless the sort key is [`SortKey::Path`], the only reader.
    folded_parent: (usize, usize),
    folded_name: (usize, usize),
    name_start: usize,
}

impl HitKey {
    fn new(key: SortKey, volume: usize, hit: &QueryHit, folded: &mut Vec<u8>) -> Self {
        let separator = hit.path.iter().rposition(|&b| b == b'\\');
        let name_start = separator.map_or(0, |i| i + 1);
        let mut push = |bytes: &[u8]| {
            let start = folded.len();
            crate::fold::fold_into(bytes, folded);
            (start, folded.len())
        };
        let folded_parent = match (key, separator) {
            (SortKey::Path, Some(i)) => push(&hit.path[..i]),
            _ => (0, 0),
        };
        Self {
            volume,
            folded_parent,
            folded_name: push(&hit.path[name_start..]),
            name_start,
        }
    }
}

/// A hit as the result order reads it in [`order_hits`].
struct Keyed<'a> {
    key: &'a HitKey,
    hit: &'a QueryHit,
    folded: &'a [u8],
    volumes: &'a [&'a [u8]],
}

impl Keyed<'_> {
    fn bytes(&self, (start, end): (usize, usize)) -> &[u8] {
        &self.folded[start..end]
    }
}

impl OrderFields for Keyed<'_> {
    fn folded_parent(&self) -> &[u8] {
        self.bytes(self.key.folded_parent)
    }
    fn folded_name(&self) -> &[u8] {
        self.bytes(self.key.folded_name)
    }
    fn size(&self) -> Option<u64> {
        self.hit.size
    }
    fn mtime(&self) -> Option<i64> {
        self.hit.mtime
    }
    fn volume(&self) -> &[u8] {
        self.volumes[self.key.volume]
    }
    fn frn(&self) -> Frn {
        self.hit.frn
    }
    fn parent_frn(&self) -> Frn {
        self.hit.parent_frn
    }
    fn name(&self) -> &[u8] {
        &self.hit.path[self.key.name_start..]
    }
}
