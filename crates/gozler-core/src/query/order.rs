//! The result order: which of two rows prints first.
//!
//! One rule decides the order for every caller, so a limited query is
//! always a prefix of the same query without a limit. `compare` decides it,
//! except for a path sort across volumes: within one volume, `compare` orders
//! folders by their folder ranks, and across volumes `compare_paths` orders
//! them by their folded paths, because folder ranks order only one volume's
//! folders. The engine sorts one volume's matches as `SortRecord`s, whose
//! keys order them as `compare` does, and orders each run of equal keys with
//! `compare` under the volume's index lock. The page builder merges several
//! volumes' rows with `compare`, or, for a path sort, with `compare_paths`.
//!
//! The sort key decides first. `Path` is es's `(parent folder, name)` order,
//! folded, not a byte compare of the whole path; an unknown size or date
//! sorts last in either direction; `Extension` compares the folded extension,
//! and an entry without one sorts first ascending. Ties then break, always
//! ascending, on the folded name, the volume, the file ID, the parent folder's
//! file ID and the raw name. Each key comes from the row's entry, so no path
//! is rebuilt to break a tie.

use super::engine::extension;
use super::paths::ResultNames;
use crate::types::{EntryIdx, Frn, SortDir, SortKey, SortSpec};
use core::cmp::Ordering;
use core::ops::Range;

/// One match as the engine sorts it: its primary key, its name key and its
/// entry, packed into one number, so the sort compares numbers. A record
/// with a smaller primary key, or an equal primary key and a smaller name
/// key, comes first in [`compare`]'s order. Records whose two keys are equal
/// form a tie run, which `compare` orders; the entry only makes the sort's
/// order total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SortRecord(u128);

const _: () = assert!(size_of::<SortRecord>() == 16);

impl SortRecord {
    /// A record of entry `idx` whose keys are not set yet.
    pub(crate) fn new(idx: EntryIdx) -> Self {
        Self(u128::from(idx))
    }

    pub(crate) fn keyed(primary: u64, name_key: u32, idx: EntryIdx) -> Self {
        Self((u128::from(primary) << 64) | (u128::from(name_key) << 32) | u128::from(idx))
    }

    pub(crate) fn idx(self) -> EntryIdx {
        self.0 as EntryIdx
    }

    pub(crate) fn primary(self) -> u64 {
        (self.0 >> 64) as u64
    }

    pub(crate) fn name_key(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// The primary key and the name key: equal for every record of a tie run.
    pub(crate) fn keys(self) -> u128 {
        self.0 >> 32
    }
}

/// What the primary key of each sort key is made from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum KeyValue {
    /// The index's [name key](crate::index::VolumeIndex::name_key).
    Name(u32),
    /// The [`folder_ranks`] rank of the match's parent folder, then the name
    /// key.
    Path {
        folder_rank: u32,
        name_key: u32,
    },
    Size(Option<u64>),
    DateModified(Option<i64>),
    DateCreated(Option<i64>),
    /// The place of the match's folded [`extension`] among the distinct
    /// extensions of the matches, `None` first.
    Extension(u32),
}

/// The primary key of a match from the value its sort key reads. A
/// descending sort inverts it, the whole of it for a path sort, so the
/// tie-breaks of [`compare`] stay ascending, and an unknown size or date
/// stays last.
pub(crate) fn primary_key(dir: SortDir, value: KeyValue) -> u64 {
    let directed = |key: u64| match dir {
        SortDir::Asc => key,
        SortDir::Desc => !key,
    };
    // A known value lies in 0..=u64::MAX - 1, so u64::MAX, last in either
    // direction, is left for an unknown one.
    let unknown_last = |known: Option<u64>| match (known, dir) {
        (None, _) => u64::MAX,
        (Some(value), SortDir::Asc) => value,
        (Some(value), SortDir::Desc) => u64::MAX - 1 - value,
    };
    match value {
        KeyValue::Name(name_key) => directed(u64::from(name_key)),
        KeyValue::Path {
            folder_rank,
            name_key,
        } => directed((u64::from(folder_rank) << 32) | u64::from(name_key)),
        // The index stores an unknown size as u64::MAX, so a known one is
        // below it.
        KeyValue::Size(size) => unknown_last(size),
        // Flipping the sign bit orders an `i64` as a `u64`. The index stores
        // an unknown date as i64::MIN, so a known one maps to 1 or more.
        KeyValue::DateModified(date) | KeyValue::DateCreated(date) => {
            unknown_last(date.map(|t| ((t as u64) ^ (1 << 63)) - 1))
        }
        KeyValue::Extension(rank) => directed(u64::from(rank)),
    }
}

/// The folder rank that [`primary_key`] packed into the primary key of a
/// path sort in direction `dir`.
pub(crate) fn packed_folder_rank(dir: SortDir, primary: u64) -> u32 {
    let key = match dir {
        SortDir::Asc => primary,
        SortDir::Desc => !primary,
    };
    (key >> 32) as u32
}

/// What the result order reads from one row, from the index that holds it.
pub(crate) trait OrderFields {
    /// Read only by [`SortKey::Extension`]: a folder has no extension.
    fn is_dir(&self) -> bool;
    /// The [`folder_ranks`] rank of the row's parent folder. Read only by
    /// [`compare`] for [`SortKey::Path`].
    fn folder_rank(&self) -> u32;
    fn folded_name(&self) -> &[u8];
    fn size(&self) -> Option<u64>;
    fn mtime(&self) -> Option<i64>;
    fn created(&self) -> Option<i64>;
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
            a.folder_rank()
                .cmp(&b.folder_rank())
                .then_with(|| a.folded_name().cmp(b.folded_name())),
        ),
        SortKey::Size => unknown_last(sort.dir, a.size(), b.size()),
        SortKey::DateModified => unknown_last(sort.dir, a.mtime(), b.mtime()),
        SortKey::DateCreated => unknown_last(sort.dir, a.created(), b.created()),
        // Folding keeps every `.` where it is, so the extension of the folded
        // name is the folded extension.
        SortKey::Extension => directed(
            sort.dir,
            extension(a.folded_name(), a.is_dir()).cmp(&extension(b.folded_name(), b.is_dir())),
        ),
    };
    primary.then_with(|| tie_breaks(a, b))
}

/// The path-sort order of `a` and `b` on different volumes, whose folder
/// ranks do not compare: `a_folder` and `b_folder` are the folded full paths
/// of their folders, which order folders as [`folder_ranks`] does.
pub(crate) fn compare_paths<T: OrderFields>(
    dir: SortDir,
    a: &T,
    a_folder: &[u8],
    b: &T,
    b_folder: &[u8],
) -> Ordering {
    directed(
        dir,
        a_folder
            .cmp(b_folder)
            .then_with(|| a.folded_name().cmp(b.folded_name())),
    )
    .then_with(|| tie_breaks(a, b))
}

/// What orders two rows whose sort keys are equal, always ascending.
fn tie_breaks<T: OrderFields>(a: &T, b: &T) -> Ordering {
    a.folded_name()
        .cmp(b.folded_name())
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

/// The rank of each folder of `names`, indexed by its
/// [`FolderId`](super::FolderId), in the order of the folded full paths that
/// [`SortKey::Path`] compares: two folders rank as their paths compare, and
/// folders with equal paths, such as `A` and `a` in a case-sensitive folder,
/// share a rank. Rank 0 is the empty path's: the root of a volume mounted
/// nowhere has it, and every other folder ranks above it.
///
/// No path is built, so the cost grows with the folders, not with their
/// depth. That order is not a walk of the tree in name order: `C:\a` sorts
/// before `C:\a b`, which sorts before `C:\a\x`, because a space sorts before
/// `\`. So among the folders in one folder, each sorts by two keys: its folded
/// name followed by an end that sorts before every byte, for the folder
/// itself, and its folded name followed by `\`, for the folders below it. A
/// name holds no `\`, so no key is the start of another, and the keys order
/// the folders exactly as their paths do. A volume's root is placed by each
/// part of its mount prefix, so a volume mounted in a folder sorts among that
/// folder's entries.
pub(crate) fn folder_ranks(names: &ResultNames) -> Vec<u32> {
    // The tree to rank: node 0 stands above the roots, then one node for each
    // part of a root's name and one for each other folder. Folders with
    // equal paths keep separate nodes; the walk merges them.
    let mut folded = Vec::new();
    let mut nodes = vec![RankNode {
        parent: 0,
        name: 0..0,
    }];
    let mut node_of: Vec<u32> = Vec::with_capacity(names.folder_count());
    let mut push = |parent: u32, name: &[u8]| {
        let start = folded.len();
        crate::fold::fold_into(name, &mut folded);
        nodes.push(RankNode {
            parent,
            name: start..folded.len(),
        });
        u32::try_from(nodes.len() - 1).expect("a rank node count fits in u32")
    };
    for position in 0..names.folder_count() {
        let folder = names
            .folder_at(position as u32)
            .expect("position counts the table's folders");
        let name = names.folder_name(folder);
        let node = match names.parent(folder) {
            Some(parent) => push(node_of[parent.index()], name),
            // The root of a volume mounted nowhere has an empty name, and no
            // separator comes before its entries' names, as under node 0.
            None if name.is_empty() => 0,
            None => name.split(|&b| b == b'\\').fold(0, &mut push),
        };
        node_of.push(node);
    }

    // Each node's children, listed together: those of node `n` are
    // `children[first[n]..first[n + 1]]`.
    let mut first = vec![0usize; nodes.len() + 1];
    for node in &nodes[1..] {
        first[node.parent as usize + 1] += 1;
    }
    for n in 1..first.len() {
        first[n] += first[n - 1];
    }
    let mut children = vec![0u32; nodes.len() - 1];
    let mut fill = first.clone();
    for (id, node) in nodes.iter().enumerate().skip(1) {
        let slot = &mut fill[node.parent as usize];
        children[*slot] = id as u32;
        *slot += 1;
    }

    // Walk the groups of nodes with equal paths in path order. A group lies
    // in `members`; `tasks` holds the keys still to walk, the next on top.
    let name = |node: u32| &folded[nodes[node as usize].name.clone()];
    let mut ranks = vec![0u32; nodes.len()];
    let mut next_rank = 1u32;
    let mut members: Vec<u32> = vec![0];
    let mut tasks = vec![(0..1, Part::Below)];
    let mut keys = Vec::new();
    while let Some((group, part)) = tasks.pop() {
        if part == Part::Folder {
            for &node in &members[group] {
                ranks[node as usize] = next_rank;
            }
            next_rank += 1;
            continue;
        }
        let start = members.len();
        for at in group {
            let node = members[at] as usize;
            members.extend_from_slice(&children[first[node]..first[node + 1]]);
        }
        members[start..].sort_unstable_by(|&a, &b| name(a).cmp(name(b)));
        let mut at = start;
        while at < members.len() {
            let run = name(members[at]);
            let end = at + members[at..].partition_point(|&m| name(m) == run);
            keys.push((at..end, Part::Folder));
            keys.push((at..end, Part::Below));
            at = end;
        }
        keys.sort_unstable_by(|(a, a_part), (b, b_part)| {
            key_units(name(members[a.start]), *a_part)
                .cmp(key_units(name(members[b.start]), *b_part))
        });
        tasks.extend(keys.drain(..).rev());
    }
    node_of.iter().map(|&node| ranks[node as usize]).collect()
}

struct RankNode {
    parent: u32,
    /// The folded name, in the walk's shared buffer.
    name: Range<usize>,
}

/// Which of a folder's two keys in [`folder_ranks`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The folder itself.
    Folder,
    /// The folders below it.
    Below,
}

/// A [`folder_ranks`] key as units that compare as the key does: each byte
/// of the folded name above 0, then the end of the folder or a `\`.
fn key_units(name: &[u8], part: Part) -> impl Iterator<Item = u16> + '_ {
    let last = match part {
        Part::Folder => 0,
        Part::Below => u16::from(b'\\') + 1,
    };
    name.iter()
        .map(|&b| u16::from(b) + 1)
        .chain(core::iter::once(last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// The ranks must order folders exactly as their folded paths
        /// compare, or a path sort prints rows out of es's order. The names
        /// mix a space and `~`, which sort before and after `\`, and `A` with
        /// `a`, which fold together; the roots include mounted volumes.
        #[test]
        fn ranks_order_folders_as_their_folded_paths(
            roots in prop::collection::vec(
                prop::sample::select(vec!["C:", r"C:\m", r"C:\m\a", "D:"]),
                1..4,
            ),
            folders in prop::collection::vec(
                (any::<prop::sample::Index>(), "[aAb ~]{1,3}"),
                0..40,
            ),
        ) {
            let mut names = ResultNames::default();
            for root in &roots {
                names.push_folder(None, root.as_bytes());
            }
            for (parent, name) in &folders {
                let parent = names
                    .folder_at(parent.index(names.folder_count()) as u32)
                    .expect("the index is below the folder count");
                names.push_folder(Some(parent), name.as_bytes());
            }
            let ranks = folder_ranks(&names);
            let mut path = Vec::new();
            let paths: Vec<Vec<u8>> = (0..names.folder_count())
                .map(|position| {
                    let folder = names.folder_at(position as u32).expect("in the table");
                    names.folder_path_into(folder, &mut path);
                    let mut key = Vec::new();
                    crate::fold::fold_into(&path, &mut key);
                    key
                })
                .collect();
            for (a, a_path) in paths.iter().enumerate() {
                for (b, b_path) in paths.iter().enumerate() {
                    prop_assert_eq!(
                        ranks[a].cmp(&ranks[b]),
                        a_path.cmp(b_path),
                        "{:?} against {:?}",
                        String::from_utf8_lossy(a_path),
                        String::from_utf8_lossy(b_path),
                    );
                }
            }
        }
    }
}
