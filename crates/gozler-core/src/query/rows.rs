//! A search's answer as rows, read from the index one page at a time.
//!
//! [`ResultRows::search`] runs a search over the volumes it is given and
//! [`search_totals`] sums one. A search holds each volume's index twice
//! through [`IndexedVolume`]: under the first hold it resolves the scope
//! folder and finds and keys the matches ([`super::engine::find_matches`]),
//! without a hold it sorts them, and under the second hold they become the
//! rows. Each page then holds the volumes it reads again.
//!
//! After the sort, each match becomes its row: its entry and its file ID,
//! and nothing else, so an answer of millions of rows holds no name and no
//! folder. A page is read under the index read lock of each volume it reads:
//! each row is the file as it is then, with its current name, size and date,
//! as Everything's list shows a renamed file. A row whose slot is dead or
//! holds another file, that left the search's scope, or whose folder has no
//! full path is left out, and the reply's total counts it out, because
//! `gozler` reads the last page's total and exits 10 when it shows fewer
//! rows. A volume whose index was replaced or got new entry indexes sends no
//! more rows, and the reply says so.
//!
//! A search reads for one [`Viewer`]. Under its first hold of each volume it
//! asks the viewer about every descriptor of the index once ([`SeenBy`]),
//! and the request keeps those answers to its end: the scan, the keys and
//! every page read the volume's entries through them, so a filtered user
//! never gets a row, a count or a folder size of an entry they cannot see.
//! The answers hold while the index keeps its instance, which every later
//! hold checks.
//!
//! [`Reply`] merges several volumes' rows one page at a time, by [`compare`]
//! or, for a path sort, by the folders' folded paths, because folder ranks
//! order only one volume's folders. A limited search keeps each volume's
//! first `offset + limit` rows. When a volume whose limit cut off later
//! matches runs out of rows because some were left out, its next match is
//! unknown, so the reply ends there, short, rather than send a later row of
//! another volume before it.

use super::CompiledQuery;
use super::engine::{
    IndexChanged, MatchSums, SizeOverflow, Totals, find_matches, in_scope, resolve_scope,
    run_totals,
};
use super::order::{OrderFields, SortRecord, compare, compare_paths};
use super::paths::{FolderCopy, FolderId, ResultNames, root_name};
use crate::attributes::Attributes;
use crate::fold::fold;
use crate::index::{SeenBy, VolumeIndex};
use crate::scope::{Scope, VolumeMounts};
use crate::types::{EntryIdx, Frn, SortDir, SortKey, SortSpec};
use core::cmp::Ordering;
use core::ops::Deref;
use rustc_hash::FxHashMap;
use std::sync::Arc;

mod snapshot;

pub use snapshot::{ResultSnapshot, ResultSnapshotGone, SnapshotRow, SnapshotRows, TooManyVolumes};

/// A row of a volume whose file IDs fit in 64 bits, as NTFS's do. It has the
/// size and alignment of a [`SortRecord`], so the records become rows in
/// place, without a second list.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(16))]
struct NarrowRow {
    frn: u64,
    idx: EntryIdx,
}

/// A row of a volume with 128-bit file IDs, as ReFS has, kept as two `u64`s
/// so that a row takes 24 bytes, not the 32 that a `u128`'s alignment gives.
/// It is larger than a [`SortRecord`], so these rows take a second list.
#[derive(Clone, Copy, Debug)]
struct WideRow {
    frn: [u64; 2],
    idx: EntryIdx,
}

const _: () = assert!(size_of::<NarrowRow>() == size_of::<SortRecord>());
const _: () = assert!(align_of::<NarrowRow>() == align_of::<SortRecord>());
const _: () = assert!(size_of::<WideRow>() == 24);

/// One volume's rows, at the width of its file IDs.
enum RowList {
    Narrow(Vec<NarrowRow>),
    Wide(Vec<WideRow>),
}

impl RowList {
    /// The rows of `records`, with each entry's file ID from `index`.
    fn new(records: Vec<SortRecord>, index: &VolumeIndex) -> Self {
        let frn = |record: SortRecord| index.entry(record.idx()).frn().0;
        if index.has_wide_ids() {
            RowList::Wide(
                records
                    .into_iter()
                    .map(|r| WideRow {
                        frn: [frn(r) as u64, (frn(r) >> 64) as u64],
                        idx: r.idx(),
                    })
                    .collect(),
            )
        } else {
            RowList::Narrow(
                records
                    .into_iter()
                    .map(|r| NarrowRow {
                        frn: u64::try_from(frn(r)).expect("an index without wide IDs has none"),
                        idx: r.idx(),
                    })
                    .collect(),
            )
        }
    }

    fn len(&self) -> usize {
        match self {
            RowList::Narrow(rows) => rows.len(),
            RowList::Wide(rows) => rows.len(),
        }
    }

    /// Keeps the rows whose place `kept` marks, in their order.
    fn retain_kept(&mut self, kept: &[bool]) {
        let mut kept = kept.iter();
        let mut keep = || *kept.next().expect("one mark for each row");
        match self {
            RowList::Narrow(rows) => rows.retain(|_| keep()),
            RowList::Wide(rows) => rows.retain(|_| keep()),
        }
    }

    /// The entry and the file ID of the row at `at`.
    fn get(&self, at: usize) -> (EntryIdx, Frn) {
        match self {
            RowList::Narrow(rows) => (rows[at].idx, Frn(u128::from(rows[at].frn))),
            RowList::Wide(rows) => {
                let [low, high] = rows[at].frn;
                (
                    rows[at].idx,
                    Frn(u128::from(low) | (u128::from(high) << 64)),
                )
            }
        }
    }
}

/// One volume's answer to a search: its rows in the result order, from
/// [`super::engine::OrderedMatches::read_rows`]. A reply only reads it; what
/// a reply changes as it reads is in its [`VolumeReader`].
pub(crate) struct VolumeRows {
    rows: RowList,
    /// [`VolumeIndex::instance`] of the index the rows name entries of.
    instance: u64,
    /// The scope folder and its file ID; `None` without a scope.
    scope: Option<(EntryIdx, Frn)>,
    /// The matches after the rows that the limit cut off, less those removed
    /// before the rows were read.
    past_cut: u64,
    /// The order of the rows.
    sort: SortSpec,
    sums: Option<MatchSums>,
    mount_prefix: String,
}

impl VolumeRows {
    pub(super) fn new(
        records: Vec<SortRecord>,
        index: &VolumeIndex,
        scope: Option<(EntryIdx, Frn)>,
        past_cut: u64,
        sort: SortSpec,
        sums: Option<MatchSums>,
        mount_prefix: String,
    ) -> Self {
        Self {
            rows: RowList::new(records, index),
            instance: index.instance(),
            scope,
            past_cut,
            sort,
            sums,
            mount_prefix,
        }
    }

    /// Every match's [`MatchSums`], when the search asked for them.
    pub(crate) fn sums(&self) -> Option<MatchSums> {
        self.sums
    }
}

/// A row as its page shows it, read from the index while the page is built.
#[derive(Clone, Copy, Debug)]
pub struct RowView<'a> {
    /// The folder that holds the row, in the reply's [`ResultNames`].
    pub folder: FolderId,
    /// The row's own name now (WTF-8).
    pub name: &'a [u8],
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub created: Option<i64>,
    pub attributes: Attributes,
}

/// The rows of every volume of one reply, merged in the result order and
/// read a page at a time with [`Self::fill_placed`].
pub(super) struct Reply {
    volumes: Vec<VolumeReader>,
    /// The folders of every row read so far, of every volume.
    names: ResultNames,
    sort: SortSpec,
    merge: Merge,
    /// Rows still to skip, for the request's offset.
    skip: u64,
    /// Rows still to send, for the request's limit; `None` without one.
    left: Option<u64>,
    /// The matches, less every row left out so far.
    total: u64,
    /// A volume's index changed during the reply, so its rows stopped.
    index_changed: bool,
}

/// How [`Reply`] chooses the volume whose row comes next.
enum Merge {
    /// The volumes in turn, each to its last row, in this order. For a path
    /// sort whose volumes' root paths do not start one another, such as `c:`
    /// and `d:`, every path of one volume sorts before every path of the
    /// other.
    InTurn(Vec<usize>),
    /// The volume whose next row comes first by [`compare`].
    Compare,
    /// For a path sort: the volume whose next row comes first by
    /// [`compare_paths`], because a root path starts another's, as `c:\data`
    /// starts `c:\data2`, so the volumes' paths interleave.
    ComparePaths,
}

/// Where [`Reply`] finds its next row.
enum Next {
    /// At the head of this volume.
    Volume(usize),
    /// On a volume whose index this page does not hold.
    NotHeld,
    /// Nowhere: the reply has no row left to send.
    End,
}

/// One volume's rows as a reply reads them.
struct VolumeReader {
    answer: VolumeRows,
    /// What the request sees of the volume, from the hold that found the
    /// answer.
    seen_by: SeenBy,
    /// The next row to read.
    next: usize,
    /// The matches after the rows that the limit cut off that `total` still
    /// counts; from [`VolumeRows`]'s, until the reply leaves them out.
    past_cut: u64,
    /// The next row's folder, once the page being built found that the row
    /// is still the file, under the scope, with a full path.
    head: Option<FolderId>,
    /// The folders of the rows read so far; made with the first page.
    folders: Option<FolderCopy>,
    /// The page's memo of folders inside or outside the scope.
    scope_memo: FxHashMap<EntryIdx, bool>,
    /// The folded full path of the folder of the next row, for
    /// [`Merge::ComparePaths`].
    folder_path: Option<(FolderId, Vec<u8>)>,
}

impl Reply {
    /// A reply of `answers`, one for each volume of the same search, each
    /// with what the request sees of that volume, from the hold that found
    /// the answer. It skips the first `offset` rows and sends at most
    /// `limit`.
    pub(super) fn new(answers: Vec<(VolumeRows, SeenBy)>, offset: u32, limit: Option<u32>) -> Self {
        let volumes: Vec<VolumeReader> = answers
            .into_iter()
            .map(|(answer, seen_by)| VolumeReader {
                past_cut: answer.past_cut,
                answer,
                seen_by,
                next: 0,
                head: None,
                folders: None,
                scope_memo: FxHashMap::default(),
                folder_path: None,
            })
            .collect();
        let sort = volumes
            .first()
            .map_or_else(SortSpec::default, |v| v.answer.sort);
        assert!(
            volumes.iter().all(|v| v.answer.sort == sort),
            "the answers of one search share its order"
        );
        let merge = match sort.key {
            SortKey::Path => merge_paths(sort.dir, &volumes),
            _ => Merge::Compare,
        };
        Self {
            total: volumes
                .iter()
                .map(|v| v.answer.rows.len() as u64 + v.past_cut)
                .sum(),
            volumes,
            names: ResultNames::default(),
            sort,
            merge,
            skip: u64::from(offset),
            left: limit.map(u64::from),
            index_changed: false,
        }
    }

    /// The matches, less every row left out so far: the total each page
    /// reports.
    pub(super) fn total(&self) -> u64 {
        self.total
    }

    /// Whether a volume's index was replaced, or got new entry indexes,
    /// during the reply, so its rows stopped.
    pub(super) fn index_changed(&self) -> bool {
        self.index_changed
    }

    /// The rows not read yet, at most the limit's: what sizes a page.
    pub(super) fn rows_left(&self) -> usize {
        let rows: usize = self
            .volumes
            .iter()
            .map(|v| v.answer.rows.len() - v.next)
            .sum();
        match self.left {
            Some(left) => rows.min(usize::try_from(left).unwrap_or(usize::MAX)),
            None => rows,
        }
    }

    /// Whether the next page reads the index of volume `v`, the `v`th answer
    /// given to [`Self::new`]: a volume with rows left, and of volumes that
    /// follow each other only the one whose turn it is, so that a page never
    /// waits for the lock of an index it does not read.
    pub(super) fn reads_volume(&self, v: usize) -> bool {
        match &self.merge {
            Merge::InTurn(order) => order.iter().find(|&&w| self.volumes[w].has_rows()) == Some(&v),
            Merge::Compare | Merge::ComparePaths => self.volumes[v].has_rows(),
        }
    }

    /// [`Self::fill_placed`] without each row's place.
    #[cfg(test)]
    pub(super) fn fill(
        &mut self,
        indexes: &[Option<&VolumeIndex>],
        page_rows: usize,
        mut push: impl FnMut(RowView<'_>, &ResultNames) -> bool,
    ) -> bool {
        self.fill_placed(indexes, page_rows, |_, _, row, names| push(row, names))
    }

    /// Reads the next rows from `indexes`, one for each answer given to
    /// [`Self::new`], in their order: the index held under its read lock
    /// where [`Self::reads_volume`] says the page reads it, and `None`
    /// elsewhere. Hands each row to `push` with the folders it names, until
    /// `push` refuses one, the page has no row left, or it read `page_rows`
    /// rows, sent or skipped for the offset, so that one hold of the locks
    /// does a page's work at most. Returns whether rows are left: a row
    /// `push` refused is read again for the next page.
    ///
    /// `push` also gets each row's volume, the `v`th answer, and its place
    /// in that volume's rows.
    fn fill_placed(
        &mut self,
        indexes: &[Option<&VolumeIndex>],
        page_rows: usize,
        mut push: impl FnMut(usize, usize, RowView<'_>, &ResultNames) -> bool,
    ) -> bool {
        assert_eq!(
            indexes.len(),
            self.volumes.len(),
            "one index for each volume's answer"
        );
        for (volume, index) in self.volumes.iter_mut().zip(indexes) {
            if let Some(index) = index
                && volume.begin_page(index, &mut self.names, &mut self.total)
            {
                self.index_changed = true;
            }
        }
        let mut read = 0;
        loop {
            if self.left == Some(0) {
                return false;
            }
            let v = match self.next_volume(indexes) {
                Next::Volume(v) => v,
                Next::NotHeld => return true,
                Next::End => return false,
            };
            if read == page_rows {
                return true;
            }
            read += 1;
            let index = indexes[v].expect("next_volume reads only the indexes held");
            let volume = &mut self.volumes[v];
            let folder = volume.head.take().expect("next_volume found the row");
            if self.skip > 0 {
                self.skip -= 1;
                volume.next += 1;
                continue;
            }
            let (idx, _) = volume.answer.rows.get(volume.next);
            let entry = index.entry(idx);
            let row = RowView {
                folder,
                name: entry.name(),
                is_dir: entry.is_dir(),
                size: entry.size(&volume.seen_by),
                mtime: entry.mtime(),
                created: entry.created(),
                attributes: entry.attributes(),
            };
            if !push(v, volume.next, row, &self.names) {
                volume.head = Some(folder);
                return true;
            }
            volume.next += 1;
            if let Some(left) = &mut self.left {
                *left -= 1;
            }
        }
    }

    /// The volume whose next row comes first, after each volume left out the
    /// rows before its next row that it cannot send. A volume that has no
    /// row left although its limit cut off matches ends the reply, because
    /// its next match is unknown.
    fn next_volume(&mut self, indexes: &[Option<&VolumeIndex>]) -> Next {
        let Self {
            volumes,
            names,
            total,
            merge,
            sort,
            ..
        } = self;
        if let Merge::InTurn(order) = merge {
            for &v in order.iter() {
                let volume = &mut volumes[v];
                if volume.has_rows() {
                    let Some(index) = indexes[v] else {
                        return Next::NotHeld;
                    };
                    if volume.find_head(index, names, total) {
                        return Next::Volume(v);
                    }
                }
                if volume.past_cut > 0 {
                    return Next::End;
                }
            }
            return Next::End;
        }
        let by_paths = matches!(merge, Merge::ComparePaths);
        let mut first: Option<usize> = None;
        for v in 0..volumes.len() {
            let found = volumes[v].has_rows()
                && volumes[v].find_head(
                    indexes[v].expect("a page holds every volume with rows left"),
                    names,
                    total,
                );
            if !found {
                if volumes[v].past_cut > 0 {
                    return Next::End;
                }
                continue;
            }
            if by_paths {
                volumes[v].note_folder_path(names);
            }
            first = match first {
                Some(f)
                    if !volumes[v].precedes(
                        &volumes[f],
                        indexes[v].expect("the volume has a row"),
                        indexes[f].expect("the volume has a row"),
                        *sort,
                        by_paths,
                    ) =>
                {
                    Some(f)
                }
                _ => Some(v),
            };
        }
        first.map_or(Next::End, Next::Volume)
    }
}

/// How a path sort merges `volumes`: in turn, in the order of their root
/// paths, when no root path starts another's, and otherwise by comparing
/// paths. A root path is the folded [`root_name`].
fn merge_paths(dir: SortDir, volumes: &[VolumeReader]) -> Merge {
    let roots: Vec<Vec<u8>> = volumes
        .iter()
        .map(|v| fold(root_name(&v.answer.mount_prefix)))
        .collect();
    let nested = roots.iter().enumerate().any(|(i, a)| {
        roots
            .iter()
            .enumerate()
            .any(|(j, b)| i != j && b.starts_with(a))
    });
    if nested {
        return Merge::ComparePaths;
    }
    let mut order: Vec<usize> = (0..volumes.len()).collect();
    order.sort_by(|&a, &b| match dir {
        SortDir::Asc => roots[a].cmp(&roots[b]),
        SortDir::Desc => roots[b].cmp(&roots[a]),
    });
    Merge::InTurn(order)
}

impl VolumeReader {
    /// Starts a page under `index`'s read lock. A volume whose index changed
    /// and one whose scope folder is gone leave out every row left, which
    /// `total` stops counting, and the reply does not wait for their matches
    /// past the limit. A gone scope holds none of them, so `total` stops
    /// counting them too; for a changed index the reply says its rows are
    /// incomplete. Returns whether the index changed.
    fn begin_page(
        &mut self,
        index: &VolumeIndex,
        names: &mut ResultNames,
        total: &mut u64,
    ) -> bool {
        self.head = None;
        self.scope_memo.clear();
        let left = (self.answer.rows.len() - self.next) as u64;
        if left == 0 {
            return false;
        }
        let changed = index.instance() != self.answer.instance;
        let scope_gone = self
            .answer
            .scope
            .is_some_and(|(scope, frn)| !changed && !index.holds(scope, frn));
        if changed || scope_gone {
            *total -= left;
            if scope_gone {
                *total -= self.past_cut;
            }
            self.next = self.answer.rows.len();
            self.past_cut = 0;
            return changed;
        }
        self.folders
            .get_or_insert_with(|| FolderCopy::new(index, &self.answer.mount_prefix, names))
            .forget_missing_paths();
        false
    }

    fn has_rows(&self) -> bool {
        self.next < self.answer.rows.len()
    }

    /// Finds the next row that the page can send, leaving out each row before
    /// it that it cannot, which `total` stops counting. Returns whether one
    /// is left.
    fn find_head(&mut self, index: &VolumeIndex, names: &mut ResultNames, total: &mut u64) -> bool {
        while self.head.is_none() && self.has_rows() {
            let folders = self
                .folders
                .as_mut()
                .expect("begin_page made the copy of a volume with rows left");
            self.head = row_folder(
                index,
                &self.seen_by,
                self.answer.rows.get(self.next),
                self.answer.scope,
                &mut self.scope_memo,
                folders,
                names,
            );
            if self.head.is_none() {
                self.next += 1;
                *total -= 1;
            }
        }
        self.head.is_some()
    }

    /// Keeps the folded full path of the next row's folder, which
    /// [`Self::precedes`] compares for [`Merge::ComparePaths`].
    fn note_folder_path(&mut self, names: &ResultNames) {
        let head = self.head.expect("the volume has a next row");
        if self.folder_path.as_ref().is_some_and(|(f, _)| *f == head) {
            return;
        }
        let mut path = Vec::new();
        names.folder_path_into(head, &mut path);
        self.folder_path = Some((head, fold(&path)));
    }

    /// Whether this volume's next row comes before `other`'s, each read from
    /// its index as its request sees it, by their folded folder paths when
    /// `by_paths`, which [`Self::note_folder_path`] noted.
    fn precedes(
        &self,
        other: &VolumeReader,
        index: &VolumeIndex,
        other_index: &VolumeIndex,
        sort: SortSpec,
        by_paths: bool,
    ) -> bool {
        let a = self.live(index);
        let b = other.live(other_index);
        let order = if by_paths {
            compare_paths(
                sort.dir,
                &a,
                self.noted_folder_path(),
                &b,
                other.noted_folder_path(),
            )
        } else {
            compare(sort, &a, &b)
        };
        order == Ordering::Less
    }

    fn noted_folder_path(&self) -> &[u8] {
        let (_, path) = self
            .folder_path
            .as_ref()
            .expect("note_folder_path noted the next row's folder");
        path
    }

    /// The next row, as the merge reads it from `index` for the request.
    fn live<'a>(&'a self, index: &'a VolumeIndex) -> Live<'a> {
        Live {
            index,
            seen_by: &self.seen_by,
            idx: self.answer.rows.get(self.next).0,
            volume: self.answer.mount_prefix.as_bytes(),
        }
    }
}

/// The folder of the row `(idx, frn)` when a page can send it: its slot still
/// holds the file, `seen_by` sees it now, the file is under `scope`, and its
/// folder has a full path. A file can move into a folder the caller cannot
/// list after the scan, and an export's pages can be read minutes later, so
/// every page asks again. `scope_memo` keeps which folders are under the
/// scope for the page.
fn row_folder(
    index: &VolumeIndex,
    seen_by: &SeenBy,
    (idx, frn): (EntryIdx, Frn),
    scope: Option<(EntryIdx, Frn)>,
    scope_memo: &mut FxHashMap<EntryIdx, bool>,
    folders: &mut FolderCopy,
    names: &mut ResultNames,
) -> Option<FolderId> {
    if !index.holds(idx, frn) || !index.is_seen(idx, seen_by) {
        return None;
    }
    if let Some((scope, _)) = scope
        && !in_scope(index, idx, scope, scope_memo)
    {
        return None;
    }
    folders.entry_folder(index, idx, names)
}

/// A row as the merge of several volumes reads it: straight from its index.
struct Live<'a> {
    index: &'a VolumeIndex,
    seen_by: &'a SeenBy,
    idx: EntryIdx,
    volume: &'a [u8],
}

impl OrderFields for Live<'_> {
    fn is_dir(&self) -> bool {
        self.index.entry(self.idx).is_dir()
    }
    fn folder_rank(&self) -> u32 {
        unreachable!("a merge of volumes orders a path sort by compare_paths")
    }
    fn folded_name(&self) -> &[u8] {
        self.index.folded_name(self.idx)
    }
    fn size(&self) -> Option<u64> {
        self.index.entry(self.idx).size(self.seen_by)
    }
    fn mtime(&self) -> Option<i64> {
        self.index.entry(self.idx).mtime()
    }
    fn created(&self) -> Option<i64> {
        self.index.entry(self.idx).created()
    }
    fn volume(&self) -> &[u8] {
        self.volume
    }
    fn frn(&self) -> Frn {
        self.index.entry(self.idx).frn()
    }
    fn parent_frn(&self) -> Frn {
        let parent = self.index.entry(self.idx).parent();
        self.index.entry(parent).frn()
    }
    fn name(&self) -> &[u8] {
        self.index.entry(self.idx).name()
    }
}

/// One volume as a search holds it: its index, under the volume's read lock
/// until this is dropped, and the volume's state, read in the same hold
/// because a state read on its own could describe another index.
pub struct HeldVolume<I> {
    pub index: I,
    /// The index is as complete as gozler can make it, as
    /// [`VolumePhase::is_complete`](crate::types::VolumePhase::is_complete)
    /// says.
    pub complete: bool,
    /// The size and date pass failed or stopped, so some sizes and dates are
    /// unknown.
    pub metadata_pending: bool,
}

/// A volume that [`ResultRows::search`] and [`search_totals`] read.
///
/// A page holds the read locks of several volumes at once, and takes them in
/// the order the volumes were given. A caller whose volumes other requests
/// can lock at the same time gives them in one fixed order, as gozlerd gives
/// its `VolumeSet`: a reader waits behind a waiting writer, so two requests
/// that took the locks in different orders could each wait for the other's
/// volume.
pub trait IndexedVolume {
    /// The volume's index, held under its read lock until it is dropped.
    type Index<'a>: Deref<Target = VolumeIndex>
    where
        Self: 'a;

    /// Takes the volume's read lock.
    fn read(&self) -> HeldVolume<Self::Index<'_>>;

    /// Where the volume is mounted: a scope reaches the volume by it, and its
    /// rows print with its [`VolumeMounts::display_prefix`].
    fn mounts(&self) -> VolumeMounts<'_>;
}

/// A shared volume reads as the volume it points to, so a search can own the
/// volumes it reads while their owner keeps them too.
impl<T: IndexedVolume + ?Sized> IndexedVolume for Arc<T> {
    type Index<'a>
        = T::Index<'a>
    where
        Self: 'a;

    fn read(&self) -> HeldVolume<Self::Index<'_>> {
        (**self).read()
    }

    fn mounts(&self) -> VolumeMounts<'_> {
        (**self).mounts()
    }
}

/// A volume searched through a plain borrow of its index, which nothing can
/// change while it is borrowed: the volume [`run_query`] searches for tests,
/// the benches and `gozlerd scan`. It reads as complete, with every size and
/// date known.
#[derive(Clone, Copy)]
pub(crate) struct PlainVolume<'a> {
    index: &'a VolumeIndex,
    mounts: VolumeMounts<'a>,
}

impl IndexedVolume for PlainVolume<'_> {
    type Index<'a>
        = &'a VolumeIndex
    where
        Self: 'a;

    fn read(&self) -> HeldVolume<&VolumeIndex> {
        HeldVolume {
            index: self.index,
            complete: true,
            metadata_pending: false,
        }
    }

    fn mounts(&self) -> VolumeMounts<'_> {
        self.mounts
    }
}

/// Who a search reads the volumes for.
#[derive(Clone, Copy)]
pub enum Viewer<'a> {
    /// A user who sees every file.
    Everything,
    /// A filtered user: whether they may list a folder with a descriptor,
    /// given its bytes. gozlerd makes it from the caller's token; an error
    /// there answers `false`, so an error hides rather than shows.
    Filtered(&'a dyn Fn(&[u8]) -> bool),
}

impl Viewer<'_> {
    /// What this viewer sees of `index`, asked under the hold that reads it.
    fn seen_by(self, index: &VolumeIndex) -> SeenBy {
        match self {
            Viewer::Everything => SeenBy::Everything,
            Viewer::Filtered(may_list) => SeenBy::Filtered(index.listable(may_list)),
        }
    }
}

/// The rows a search returns.
#[derive(Clone, Copy, Debug)]
pub struct RowsSpec {
    pub sort: SortSpec,
    /// Rows to skip from the start of the result order.
    pub offset: u32,
    /// The most rows to return; `None` returns every row.
    pub limit: Option<u32>,
    /// Count every match's files and folders and sum their sizes
    /// ([`ResultRows::sums`]).
    pub sums: bool,
}

/// A search's scope is on no indexed volume, such as a network drive or a
/// drive letter that does not exist. The search is refused, because an empty
/// answer would read as "no such file".
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{scope} is not on an indexed volume; gozlerd indexes fixed NTFS and ReFS volumes")]
pub struct NotIndexed {
    scope: String,
}

/// Why [`search_totals`] has no totals.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TotalsError {
    #[error(transparent)]
    NotIndexed(#[from] NotIndexed),
    #[error(transparent)]
    Overflow(#[from] SizeOverflow),
}

/// The totals of every match of a search, and whether they may be short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchTotals {
    pub totals: Totals,
    /// As [`ResultRows::volumes_incomplete`].
    pub volumes_incomplete: bool,
    /// As [`ResultRows::metadata_pending`]: a file whose size is unknown adds
    /// nothing.
    pub metadata_pending: bool,
}

/// The rows of a search over several volumes, merged in the result order and
/// read a page at a time with [`Self::page`]. It keeps the volumes it
/// searched, whose read locks each page takes.
pub struct ResultRows<V> {
    /// The volumes with rows, in the order given, which the answers of
    /// `reply` follow.
    volumes: Vec<V>,
    reply: Reply,
    sums: Option<MatchSums>,
    coverage: Coverage,
}

impl<V: IndexedVolume> ResultRows<V> {
    /// Searches `volumes` below `scope`, an absolute folder, or everywhere
    /// without one, for the rows `spec` asks for, as `viewer` sees them, and
    /// drops the volumes it does not search. A volume whose index changed
    /// between its two holds gives no rows, and the answer says it is
    /// incomplete.
    ///
    /// # Errors
    ///
    /// [`NotIndexed`] when `scope` is on none of `volumes`.
    pub fn search(
        volumes: Vec<V>,
        query: &CompiledQuery,
        scope: Option<&str>,
        spec: RowsSpec,
        viewer: Viewer<'_>,
    ) -> Result<Self, NotIndexed> {
        // The offset applies after the volumes merge, so each volume keeps
        // the rows up to the limit's end.
        let per_volume_limit = spec.limit.map(|limit| spec.offset.saturating_add(limit));
        let (found, mut coverage) = run_held(
            targets(volumes, scope)?,
            query.match_case(),
            viewer,
            |index, mount_prefix, scope, seen_by| {
                find_matches(
                    index,
                    mount_prefix,
                    query,
                    scope,
                    spec.sort,
                    per_volume_limit,
                    spec.sums,
                    seen_by,
                )
            },
        );
        let mut sums = spec.sums.then(MatchSums::default);
        let mut volumes = Vec::with_capacity(found.len());
        let mut answers = Vec::with_capacity(found.len());
        for (volume, seen_by, matches) in found {
            // Without a hold, so the journal does not wait for the sort.
            let ordered = matches.order();
            let read = ordered.read_rows(&volume.read().index, &seen_by);
            match read {
                Ok(answer) => {
                    if let (Some(sums), Some(found)) = (sums.as_mut(), answer.sums()) {
                        sums.add(found);
                    }
                    volumes.push(volume);
                    answers.push((answer, seen_by));
                }
                // `IndexChanged` says why this volume's rows are lost.
                Err(IndexChanged) => coverage.volumes_incomplete = true,
            }
        }
        Ok(Self {
            volumes,
            reply: Reply::new(answers, spec.offset, spec.limit),
            sums,
            coverage,
        })
    }

    /// Reads the next page. It holds the read lock of each volume the page
    /// reads, in the order the volumes were given, and hands each row to
    /// `push` with the folders it names, until `push` refuses one, no row is
    /// left, or it read `page_rows` rows, sent or skipped for the offset. It
    /// releases the locks before it returns, so the caller sends the page
    /// without them. Returns whether rows are left: a row `push` refused is
    /// read again for the next page.
    pub fn page(
        &mut self,
        page_rows: usize,
        mut push: impl FnMut(RowView<'_>, &ResultNames) -> bool,
    ) -> bool {
        self.page_placed(page_rows, |_, _, row, names| push(row, names))
    }

    /// [`Self::page`], which also hands `push` each row's volume and its
    /// place in that volume's rows.
    fn page_placed(
        &mut self,
        page_rows: usize,
        push: impl FnMut(usize, usize, RowView<'_>, &ResultNames) -> bool,
    ) -> bool {
        let held: Vec<Option<V::Index<'_>>> = self
            .volumes
            .iter()
            .enumerate()
            .map(|(v, volume)| self.reply.reads_volume(v).then(|| volume.read().index))
            .collect();
        let indexes: Vec<Option<&VolumeIndex>> = held.iter().map(Option::as_deref).collect();
        self.reply.fill_placed(&indexes, page_rows, push)
    }

    /// The matches, less every row left out so far: the total each page
    /// reports.
    pub fn total(&self) -> u64 {
        self.reply.total()
    }

    /// The rows not read yet, at most the limit's: what sizes a page.
    pub fn rows_left(&self) -> usize {
        self.reply.rows_left()
    }

    /// Every match's files and folders and their sizes, when the search asked
    /// for them.
    pub fn sums(&self) -> Option<MatchSums> {
        self.sums
    }

    /// Whether the answer may be short: a volume searched was indexing,
    /// rescanning or offline when it was searched, or its index changed
    /// during the search, so its rows stopped.
    pub fn volumes_incomplete(&self) -> bool {
        self.coverage.volumes_incomplete || self.reply.index_changed()
    }

    /// Whether the size and date pass failed or stopped on a volume searched,
    /// so some sizes and dates are unknown.
    pub fn metadata_pending(&self) -> bool {
        self.coverage.metadata_pending
    }
}

/// The totals of every match of `query` over `volumes` below `scope`, an
/// absolute folder, or everywhere without one, that `viewer` sees. Each
/// volume sums its matches under one hold from the sizes its index holds, so
/// no row is made: `-get-folder-size C:\` covers the whole volume.
///
/// # Errors
///
/// [`TotalsError::NotIndexed`] when `scope` is on none of `volumes`, and
/// [`TotalsError::Overflow`] when the summed size passes `u64::MAX`.
pub fn search_totals<V: IndexedVolume>(
    volumes: Vec<V>,
    query: &CompiledQuery,
    scope: Option<&str>,
    viewer: Viewer<'_>,
) -> Result<SearchTotals, TotalsError> {
    let (per_volume, coverage) = run_held(
        targets(volumes, scope)?,
        query.match_case(),
        viewer,
        |index, mount_prefix, scope, seen_by| {
            run_totals(index, mount_prefix, query, scope, seen_by)
        },
    );
    let mut totals = Totals::default();
    for (_, _, volume) in per_volume {
        totals.add(volume?)?;
    }
    Ok(SearchTotals {
        totals,
        volumes_incomplete: coverage.volumes_incomplete,
        metadata_pending: coverage.metadata_pending,
    })
}

/// What the volumes a search reached said about its answer, read once, at
/// each volume's first hold: read again for a page, a rescan that began
/// later would mark rows from a complete index incomplete.
#[derive(Clone, Copy, Default)]
struct Coverage {
    /// A volume searched is indexing, rescanning or offline, or its index
    /// changed during the search.
    volumes_incomplete: bool,
    /// The size and date pass failed or stopped on a volume searched.
    metadata_pending: bool,
}

/// What a search reads of one volume: all of it, or the folder below its
/// mount that the scope names. [`run_held`] resolves the folder under the
/// same hold as the scan; otherwise a rebuild between the two could swap the
/// index and leave the resolved entry stale (wrong or empty results, or an
/// out-of-bounds panic).
enum VolScope<'s> {
    All,
    Folder(&'s str),
}

/// The volumes that `scope` reaches, by the rule the clients also wait by
/// ([`Scope::of`]), each with what of it the search reads; the other volumes
/// are dropped.
fn targets<'s, V: IndexedVolume>(
    volumes: Vec<V>,
    scope: Option<&'s str>,
) -> Result<Vec<(V, VolScope<'s>)>, NotIndexed> {
    match Scope::of(scope, volumes.iter().map(|v| v.mounts())) {
        Scope::Everywhere => Ok(volumes.into_iter().map(|v| (v, VolScope::All)).collect()),
        Scope::Folder { volume, folder } => {
            let volume = volumes
                .into_iter()
                .nth(volume)
                .expect("Scope::of routes to one of the volumes");
            Ok(vec![(volume, VolScope::Folder(folder))])
        }
        Scope::Nowhere => Err(NotIndexed {
            scope: scope
                .expect("only a scope can miss every volume")
                .to_string(),
        }),
    }
}

/// Runs `run` under one hold of each target's index, with its mount prefix,
/// its scope folder resolved and what `viewer` sees of it, and returns each
/// volume with that view and what `run` returned. A volume without the scope
/// folder holds no match and is dropped; its state still counts.
fn run_held<V: IndexedVolume, T>(
    targets: Vec<(V, VolScope<'_>)>,
    match_case: bool,
    viewer: Viewer<'_>,
    mut run: impl FnMut(&VolumeIndex, &str, Option<EntryIdx>, &SeenBy) -> T,
) -> (Vec<(V, SeenBy, T)>, Coverage) {
    let mut coverage = Coverage::default();
    let mut results = Vec::with_capacity(targets.len());
    for (volume, scope) in targets {
        let held = volume.read();
        coverage.volumes_incomplete |= !held.complete;
        coverage.metadata_pending |= held.metadata_pending;
        let index: &VolumeIndex = &held.index;
        let scope = match scope {
            VolScope::All => None,
            VolScope::Folder(folder) => match resolve_scope(index, folder.as_bytes(), match_case) {
                Some(entry) => Some(entry),
                None => continue,
            },
        };
        let seen_by = viewer.seen_by(index);
        let result = run(index, volume.mounts().display_prefix(), scope, &seen_by);
        drop(held);
        results.push((volume, seen_by, result));
    }
    (results, coverage)
}

/// What [`run_query`] returns: the full match count, and the rows with their
/// full paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryOutcome {
    /// Total matches before the limit (es `totitems` semantics).
    pub total: u64,
    pub rows: Vec<PathRow>,
}

/// A row with its full path, as [`run_query`] returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRow {
    /// WTF-8.
    pub path: Vec<u8>,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub attributes: Attributes,
}

/// Runs a query over `index`, the volume mounted at `mount_prefix` (`C:\`),
/// below `scope`, an absolute folder on it, and reads its rows as one page
/// with their full paths, for a caller that owns the index: tests, the
/// benches and `gozlerd scan`.
///
/// # Panics
///
/// When `scope` is not on the volume.
pub fn run_query(
    index: &VolumeIndex,
    mount_prefix: &str,
    query: &CompiledQuery,
    scope: Option<&str>,
    sort: SortSpec,
    limit: Option<u32>,
) -> QueryOutcome {
    let mounts = [mount_prefix.to_string()];
    let volume = PlainVolume {
        index,
        mounts: VolumeMounts {
            mounts: &mounts,
            guid: "",
        },
    };
    let spec = RowsSpec {
        sort,
        offset: 0,
        limit,
        sums: false,
    };
    let mut reply = ResultRows::search(vec![volume], query, scope, spec, Viewer::Everything)
        .expect("the scope is on the volume");
    let mut rows = Vec::new();
    reply.page(usize::MAX, |row, names| {
        let mut path = Vec::new();
        names.full_path_into(row.folder, row.name, &mut path);
        rows.push(PathRow {
            path,
            is_dir: row.is_dir,
            size: row.size,
            mtime: row.mtime,
            attributes: row.attributes,
        });
        true
    });
    QueryOutcome {
        total: reply.total(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::NTFS_ROOT_FRN;
    use crate::query::parse_query;
    use crate::query::paths::{DirPaths, NameForm};
    use crate::usn::record::{FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE};
    use proptest::prelude::*;
    use std::cell::{Cell, RefCell};

    /// The folders of every volume of the merge: `Data` and `Data2` beside
    /// the volumes mounted at `C:\Data\` and `C:\Data2\`, `a b`, whose space
    /// sorts before `\`, and `A`, which folds to `a`.
    const FOLDERS: [&str; 4] = ["Data", "Data2", "a b", "A"];

    /// A file of a volume: the folder that holds it (past the last of
    /// [`FOLDERS`], the root), its name, and its size and date.
    type File = (usize, String, Option<u64>);

    fn volume(files: &[File]) -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        let frn = |n: usize| Frn::ntfs(n as u64 + 100, 1);
        let rec = |f: Frn, parent: Frn, name: &str, is_dir: bool| ParsedUsnRecord {
            frn: f,
            parent_frn: parent,
            reason: USN_REASON_FILE_CREATE,
            attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.as_bytes().to_vec(),
        };
        for (n, name) in FOLDERS.iter().enumerate() {
            index.insert_enum(&rec(frn(n), NTFS_ROOT_FRN, name, true));
        }
        for (n, (folder, name, size)) in files.iter().enumerate() {
            let parent = FOLDERS.get(*folder).map_or(NTFS_ROOT_FRN, |_| frn(*folder));
            let f = frn(FOLDERS.len() + n);
            index.insert_enum(&rec(f, parent, name, false));
            if let Some(size) = size {
                index.set_stat(f, *size, (*size as i64) % 3);
            }
        }
        index.rank_names();
        index
    }

    fn file() -> impl Strategy<Value = File> {
        (
            0..=FOLDERS.len(),
            r"[aA~ ]{1,2}(\.[ab])?",
            prop::option::of(0u64..3),
        )
    }

    fn all_sorts() -> Vec<SortSpec> {
        let keys = [
            SortKey::Name,
            SortKey::Path,
            SortKey::Size,
            SortKey::DateModified,
            SortKey::Extension,
        ];
        keys.into_iter()
            .flat_map(|key| [SortDir::Asc, SortDir::Desc].map(|dir| SortSpec { key, dir }))
            .collect()
    }

    proptest! {
        /// Volumes merged, with mount prefixes as the daemon stores them,
        /// ending in `\`, give the order of the result order over the rows
        /// of all of them: `c:\data` starts `c:\data2`, so those volumes'
        /// paths interleave, and `D:\` follows `C:\`. A result snapshot
        /// sorted again into each order with no search gives that order too.
        #[test]
        fn merged_volumes_follow_the_result_order_over_all_their_rows(
            mounts in prop::sample::subsequence(vec![r"C:\", r"C:\Data\", r"C:\Data2\", r"D:\"], 2..=4),
            files in prop::collection::vec(prop::collection::vec(file(), 0..8), 4),
        ) {
            let indexes: Vec<VolumeIndex> = mounts.iter().zip(&files).map(|(_, f)| volume(f)).collect();
            let held: Vec<&VolumeIndex> = indexes.iter().collect();
            let mount_lists: Vec<[String; 1]> = mounts.iter().map(|m| [m.to_string()]).collect();
            let query = CompiledQuery::new(&parse_query("").expect("parses")).expect("compiles");
            // The empty search matches every entry of every volume.
            let all: Vec<(usize, EntryIdx)> = held
                .iter()
                .enumerate()
                .flat_map(|(v, index)| {
                    index
                        .live_entries()
                        .filter(|&idx| !index.is_synthetic(idx))
                        .map(move |idx| (v, idx))
                })
                .collect();
            let volumes: Vec<PlainVolume> = indexes
                .iter()
                .zip(&mount_lists)
                .map(|(index, mounts)| PlainVolume {
                    index,
                    mounts: VolumeMounts { mounts, guid: "" },
                })
                .collect();
            let first = RowsSpec {
                sort: SortSpec { key: SortKey::Size, dir: SortDir::Desc },
                offset: 0,
                limit: None,
                sums: false,
            };
            let (snapshot, kept) = ResultRows::search(volumes.clone(), &query, None, first, Viewer::Everything)
                .expect("no scope")
                .into_snapshot()
                .expect("at most four volumes");
            for sort in all_sorts() {
                // Every row of every volume, sorted by the result order.
                let mut folded: Vec<DirPaths> = held
                    .iter()
                    .zip(&mounts)
                    .map(|(index, mount)| DirPaths::new(index, NameForm::Folded, mount))
                    .collect();
                let mut folder_path = |v: usize, idx: EntryIdx| {
                    let mut path = Vec::new();
                    let parent = held[v].entry(idx).parent();
                    folded[v].folder_path_into(held[v], parent, &mut path);
                    path
                };
                let keyed: Vec<Vec<u8>> = all.iter().map(|&(v, idx)| folder_path(v, idx)).collect();
                let mut order: Vec<usize> = (0..all.len()).collect();
                let everything = SeenBy::Everything;
                let live = |at: usize| {
                    let (v, idx) = all[at];
                    Live { index: held[v], seen_by: &everything, idx, volume: mounts[v].as_bytes() }
                };
                order.sort_by(|&a, &b| match sort.key {
                    SortKey::Path => compare_paths(sort.dir, &live(a), &keyed[a], &live(b), &keyed[b]),
                    _ => compare(sort, &live(a), &live(b)),
                });
                let mut path = Vec::new();
                let expected: Vec<Vec<u8>> = order
                    .iter()
                    .map(|&at| {
                        let (v, idx) = all[at];
                        DirPaths::new(held[v], NameForm::Raw, mounts[v]).full_path_into(held[v], idx, &mut path);
                        path.clone()
                    })
                    .collect();

                let spec = RowsSpec { sort, offset: 0, limit: None, sums: false };
                let mut reply = ResultRows::search(volumes.clone(), &query, None, spec, Viewer::Everything).expect("no scope");
                let mut merged = Vec::new();
                while reply.page(usize::MAX, |row, names| {
                    let mut path = Vec::new();
                    names.full_path_into(row.folder, row.name, &mut path);
                    merged.push(path);
                    true
                }) {}
                prop_assert_eq!(&merged, &expected, "{:?} on {:?}", sort, mounts);

                let (sorted, kept_again) = snapshot
                    .sort_again(kept.clone(), sort, Viewer::Everything)
                    .expect("no index changed")
                    .into_snapshot()
                    .expect("at most four volumes");
                let read = snapshot_paths(&sorted, kept_again, 0, None).expect("no index changed");
                let expected: Vec<Option<Vec<u8>>> = expected.into_iter().map(Some).collect();
                prop_assert_eq!(read, expected, "sorted again {:?} on {:?}", sort, mounts);
            }
        }
    }

    /// A volume that notes, by its number, each time a search takes its read
    /// lock, and counts the holds alive.
    struct Noted<'a> {
        number: usize,
        index: &'a VolumeIndex,
        mounts: [String; 1],
        taken: &'a RefCell<Vec<usize>>,
        alive: &'a Cell<usize>,
    }

    struct NotedHold<'a> {
        index: &'a VolumeIndex,
        alive: &'a Cell<usize>,
    }

    impl Deref for NotedHold<'_> {
        type Target = VolumeIndex;

        fn deref(&self) -> &VolumeIndex {
            self.index
        }
    }

    impl Drop for NotedHold<'_> {
        fn drop(&mut self) {
            self.alive.set(self.alive.get() - 1);
        }
    }

    impl IndexedVolume for Noted<'_> {
        type Index<'h>
            = NotedHold<'h>
        where
            Self: 'h;

        fn read(&self) -> HeldVolume<NotedHold<'_>> {
            self.taken.borrow_mut().push(self.number);
            self.alive.set(self.alive.get() + 1);
            HeldVolume {
                index: NotedHold {
                    index: self.index,
                    alive: self.alive,
                },
                complete: true,
                metadata_pending: false,
            }
        }

        fn mounts(&self) -> VolumeMounts<'_> {
            VolumeMounts {
                mounts: &self.mounts,
                guid: "",
            }
        }
    }

    /// A page takes the read locks of the volumes it reads in the order the
    /// volumes were given, not in the order their rows merge, and holds none
    /// once it returns. A reader waits behind a waiting writer, so two
    /// requests that took the locks in different orders could each wait for
    /// the other's volume, and gozlerd would hang.
    #[test]
    fn a_page_locks_its_volumes_in_the_order_given_and_releases_them() {
        let root_file = |name: &str| volume(&[(FOLDERS.len(), name.to_string(), None)]);
        let indexes = [root_file("b.txt"), root_file("a.txt")];
        let taken = RefCell::new(Vec::new());
        let alive = Cell::new(0);
        let volumes = indexes
            .iter()
            .zip([r"D:\", r"C:\"])
            .enumerate()
            .map(|(number, (index, mount))| Noted {
                number,
                index,
                mounts: [mount.to_string()],
                taken: &taken,
                alive: &alive,
            })
            .collect();
        let query = CompiledQuery::new(&parse_query("txt").expect("parses")).expect("compiles");
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: false,
        };
        let mut rows =
            ResultRows::search(volumes, &query, None, spec, Viewer::Everything).expect("no scope");
        taken.borrow_mut().clear();
        let mut names = Vec::new();
        let more = rows.page(usize::MAX, |row, _| {
            names.push(row.name.to_vec());
            true
        });
        assert!(!more);
        assert_eq!(
            names,
            [b"a.txt", b"b.txt"],
            "the second volume's row comes first"
        );
        assert_eq!(*taken.borrow(), [0, 1], "the page locks in the order given");
        assert_eq!(alive.get(), 0, "the page holds no lock once it returns");
    }

    /// A file whose size is unknown, as after a size pass that stopped, is
    /// counted apart and leaves the known sizes in the sum, so the window
    /// shows the space found instead of "size unknown" for every match.
    #[test]
    fn a_sum_keeps_the_known_sizes_and_counts_the_unknown_ones() {
        let index = volume(&[
            (0, "read.txt".to_string(), Some(5)),
            (0, "unread.txt".to_string(), None),
        ]);
        let mounts = [r"C:\".to_string()];
        let plain = PlainVolume {
            index: &index,
            mounts: VolumeMounts {
                mounts: &mounts,
                guid: "",
            },
        };
        let query = CompiledQuery::new(&parse_query("txt").expect("parses")).expect("compiles");
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: true,
        };
        let rows = ResultRows::search(vec![plain], &query, None, spec, Viewer::Everything)
            .expect("no scope");
        let sums = rows.sums().expect("the search asked for sums");
        assert_eq!(
            (sums.files, sums.file_bytes, sums.files_without_size),
            (2, 5, 1)
        );
    }

    /// The rows a read of `snapshot` from `offset` finds, each by its full
    /// path, or `None` for a gone row.
    fn snapshot_paths<V: IndexedVolume>(
        snapshot: &ResultSnapshot,
        volumes: Vec<V>,
        offset: u32,
        limit: Option<u32>,
    ) -> Result<Vec<Option<Vec<u8>>>, ResultSnapshotGone> {
        let mut read = snapshot.read(volumes, offset, limit, Viewer::Everything);
        let mut paths = Vec::new();
        while read.page(1000, |row, names| {
            paths.push(match row {
                SnapshotRow::Row(row) => {
                    let mut path = Vec::new();
                    names.full_path_into(row.folder, row.name, &mut path);
                    Some(path)
                }
                SnapshotRow::Gone => None,
            });
            true
        })? {}
        Ok(paths)
    }

    /// A read of a result snapshot over several volumes starts at the
    /// checkpoint below its offset and replays the merged order from there;
    /// an off-by-one in either would show another file at a far offset,
    /// where the window cannot check it. Every offset, on both sides of a
    /// checkpoint, reads the rows that one read from the start finds there,
    /// which are the reply's rows.
    #[test]
    fn a_result_snapshot_reads_each_offset_as_the_merged_order() {
        // Even numbers on C:, odd ones on D:, so the volumes take turns.
        let files = |first: usize| -> Vec<File> {
            (0..2100)
                .map(|n| {
                    (
                        n % (FOLDERS.len() + 1),
                        format!("{:05}.txt", 2 * n + first),
                        None,
                    )
                })
                .collect()
        };
        let indexes = [volume(&files(0)), volume(&files(1))];
        let mounts = [[r"C:\".to_string()], [r"D:\".to_string()]];
        let volumes: Vec<PlainVolume> = indexes
            .iter()
            .zip(&mounts)
            .map(|(index, mounts)| PlainVolume {
                index,
                mounts: VolumeMounts { mounts, guid: "" },
            })
            .collect();
        let query = CompiledQuery::new(&parse_query("txt").expect("parses")).expect("compiles");
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: false,
        };
        let search = || {
            ResultRows::search(volumes.clone(), &query, None, spec, Viewer::Everything)
                .expect("no scope")
        };
        let mut reply = Vec::new();
        let mut rows = search();
        while rows.page(1000, |row, names| {
            let mut path = Vec::new();
            names.full_path_into(row.folder, row.name, &mut path);
            reply.push(Some(path));
            true
        }) {}
        let (snapshot, kept) = search().into_snapshot().expect("two volumes");
        assert_eq!(snapshot.total(), 4200);
        let all = snapshot_paths(&snapshot, kept.clone(), 0, None).expect("not gone");
        assert_eq!(all, reply, "the reply's order");
        for offset in [1, 4095, 4096, 4097, 4198, 4200] {
            let at = offset as usize;
            let read = snapshot_paths(&snapshot, kept.clone(), offset, Some(3)).expect("not gone");
            assert_eq!(read, all[at..(at + 3).min(all.len())], "from {offset}");
        }
    }

    /// A volume whose index a test changes between reads.
    struct ChangingVolume {
        index: std::sync::RwLock<VolumeIndex>,
        mounts: [String; 1],
    }

    impl IndexedVolume for ChangingVolume {
        type Index<'h>
            = std::sync::RwLockReadGuard<'h, VolumeIndex>
        where
            Self: 'h;

        fn read(&self) -> HeldVolume<std::sync::RwLockReadGuard<'_, VolumeIndex>> {
            HeldVolume {
                index: self.index.read().expect("no test panics holding it"),
                complete: true,
                metadata_pending: false,
            }
        }

        fn mounts(&self) -> VolumeMounts<'_> {
            VolumeMounts {
                mounts: &self.mounts,
                guid: "",
            }
        }
    }

    /// The window holds a result snapshot's rows by their place, so a file
    /// deleted after the search is a gone row in its place and the total
    /// stays; sorted again, it is left out, because its slot has nothing to
    /// sort by. Once the index is replaced, the rows name other entries, so
    /// the whole result snapshot is gone.
    #[test]
    fn a_deleted_file_is_gone_in_its_place_and_a_new_index_ends_the_snapshot() {
        let files: Vec<File> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|name| (0, name.to_string(), None))
            .collect();
        let volume = Arc::new(ChangingVolume {
            index: std::sync::RwLock::new(volume(&files)),
            mounts: [r"C:\".to_string()],
        });
        let query = CompiledQuery::new(&parse_query("txt").expect("parses")).expect("compiles");
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: false,
        };
        let rows = ResultRows::search(vec![volume.clone()], &query, None, spec, Viewer::Everything)
            .expect("no scope");
        let (snapshot, kept) = rows.into_snapshot().expect("one volume");
        // b.txt, the second file the helper makes after the folders.
        let b = Frn::ntfs(FOLDERS.len() as u64 + 1 + 100, 1);
        let mut changed = volume.index.write().expect("no test panics holding it");
        changed.apply_batch(&[ParsedUsnRecord {
            frn: b,
            parent_frn: NTFS_ROOT_FRN,
            reason: crate::usn::record::USN_REASON_FILE_DELETE,
            attributes: 0,
            name: Vec::new(),
        }]);
        drop(changed);
        let path = |name: &str| Some(format!(r"C:\Data\{name}").into_bytes());
        assert_eq!(
            snapshot_paths(&snapshot, kept.clone(), 0, None),
            Ok(vec![path("a.txt"), None, path("c.txt")])
        );
        assert_eq!(snapshot.total(), 3);
        let by_name_down = SortSpec {
            key: SortKey::Name,
            dir: SortDir::Desc,
        };
        let sorted = snapshot.sort_again(kept.clone(), by_name_down, Viewer::Everything);
        let (sorted, kept_again) = sorted
            .expect("the same index")
            .into_snapshot()
            .expect("one volume");
        assert_eq!(
            snapshot_paths(&sorted, kept_again, 0, None),
            Ok(vec![path("c.txt"), path("a.txt")])
        );

        *volume.index.write().expect("no test panics holding it") = super::tests::volume(&files);
        assert_eq!(
            snapshot_paths(&snapshot, kept.clone(), 0, None),
            Err(ResultSnapshotGone)
        );
        assert!(matches!(
            snapshot.sort_again(kept, by_name_down, Viewer::Everything),
            Err(ResultSnapshotGone)
        ));
    }

    /// The descriptor of the folders the filtered user of these tests may
    /// list.
    const OPEN: &[u8] = b"open";

    /// `open\a.txt` (5 bytes), `open\sub\b.txt` (7), `hidden\c.txt` (11) and
    /// `unread\e.txt` (17), with folder sizes. The filtered user may not list
    /// `hidden`, and `unread` has no descriptor, as after a read that failed.
    fn visibility_index() -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        let frn = |n: u64| Frn::ntfs(n, 1);
        let entries = [
            (100, NTFS_ROOT_FRN, "open", None),
            (101, frn(100), "sub", None),
            (102, NTFS_ROOT_FRN, "hidden", None),
            (103, NTFS_ROOT_FRN, "unread", None),
            (200, frn(100), "a.txt", Some(5)),
            (201, frn(101), "b.txt", Some(7)),
            (202, frn(102), "c.txt", Some(11)),
            (203, frn(103), "e.txt", Some(17)),
        ];
        for (n, parent, name, size) in entries {
            index.insert_enum(&ParsedUsnRecord {
                frn: frn(n),
                parent_frn: parent,
                reason: USN_REASON_FILE_CREATE,
                attributes: if size.is_none() {
                    FILE_ATTRIBUTE_DIRECTORY
                } else {
                    0
                },
                name: name.as_bytes().to_vec(),
            });
            if let Some(size) = size {
                index.set_stat(frn(n), size, 0);
            }
        }
        for (folder, descriptor) in [
            (NTFS_ROOT_FRN, OPEN),
            (frn(100), OPEN),
            (frn(101), OPEN),
            (frn(102), &b"hidden"[..]),
        ] {
            index.set_folder_descriptor(folder, descriptor);
        }
        index.compute_folder_sizes();
        index.rank_names();
        index
    }

    fn compiled(query: &str, match_path: bool) -> CompiledQuery {
        let options = crate::query::ParseOptions {
            match_path,
            ..crate::query::ParseOptions::default()
        };
        let parsed = crate::query::parse_query_with(query, options).expect("parses");
        CompiledQuery::new(&parsed).expect("compiles")
    }

    /// Each row of `query` below `scope` on `volumes` as `viewer` sees it,
    /// by its full path and with its size, the total the last page reports,
    /// and the sums.
    fn rows_as<V: IndexedVolume>(
        volumes: Vec<V>,
        query: &CompiledQuery,
        scope: Option<&str>,
        sort: SortSpec,
        viewer: Viewer<'_>,
    ) -> (Vec<(String, Option<u64>)>, u64, MatchSums) {
        let spec = RowsSpec {
            sort,
            offset: 0,
            limit: None,
            sums: true,
        };
        let mut rows =
            ResultRows::search(volumes, query, scope, spec, viewer).expect("on a volume");
        let sums = rows.sums().expect("the search asked for sums");
        let mut found = Vec::new();
        while rows.page(usize::MAX, |row, names| {
            let mut path = Vec::new();
            names.full_path_into(row.folder, row.name, &mut path);
            found.push((String::from_utf8(path).expect("ASCII"), row.size));
            true
        }) {}
        (found, rows.total(), sums)
    }

    fn plain<'a>(index: &'a VolumeIndex, mounts: &'a [String; 1]) -> PlainVolume<'a> {
        PlainVolume {
            index,
            mounts: VolumeMounts { mounts, guid: "" },
        }
    }

    /// A filtered user finds, counts and sums only the entries of the
    /// folders they may list, gets no folder size, and a `size:` term
    /// matches none of their folders. Without it, a reader that skips the
    /// filter shows the names, the count or the space of files the user
    /// cannot see.
    #[test]
    fn a_filtered_user_finds_only_the_entries_of_folders_they_may_list() {
        let index = visibility_index();
        let mounts = [r"C:\".to_string()];
        let may_list = |descriptor: &[u8]| descriptor == OPEN;
        let caller = Viewer::Filtered(&may_list);
        let by_name = SortSpec::default_for(SortKey::Name);
        let paths = |query: &CompiledQuery, scope, viewer| {
            let (rows, total, _) =
                rows_as(vec![plain(&index, &mounts)], query, scope, by_name, viewer);
            assert_eq!(total, rows.len() as u64, "the total counts the rows");
            rows.into_iter().map(|(path, _)| path).collect::<Vec<_>>()
        };
        let (a, b, c, e) = (
            r"C:\open\a.txt",
            r"C:\open\sub\b.txt",
            r"C:\hidden\c.txt",
            r"C:\unread\e.txt",
        );
        let (hidden, open, sub, unread) = (r"C:\hidden", r"C:\open", r"C:\open\sub", r"C:\unread");
        // Each way a scan finds its candidates: the name arena, every entry,
        // the component walk of a path term, and the walk of a scope folder.
        for (query, match_path, scope, filtered, everything) in [
            ("txt", false, None, vec![a, b], vec![a, b, c, e]),
            (
                "",
                false,
                None,
                vec![a, b, hidden, open, sub, unread],
                vec![a, b, c, e, hidden, open, sub, unread],
            ),
            ("hidden", true, None, vec![hidden], vec![c, hidden]),
            ("", false, Some(hidden), vec![], vec![c]),
        ] {
            let query_ = compiled(query, match_path);
            assert_eq!(
                paths(&query_, scope, caller),
                filtered,
                "{query:?} in {scope:?}"
            );
            assert_eq!(
                paths(&query_, scope, Viewer::Everything),
                everything,
                "{query:?} in {scope:?}"
            );
        }

        let every = compiled("", false);
        let (rows, _, sums) = rows_as(vec![plain(&index, &mounts)], &every, None, by_name, caller);
        assert!(
            rows.iter()
                .all(|(path, size)| [a, b].contains(&path.as_str()) || size.is_none()),
            "no folder has a size: {rows:?}"
        );
        let shown = |sums: MatchSums| {
            (
                sums.files,
                sums.file_bytes,
                sums.folders,
                sums.folder_bytes,
                sums.folder_bytes_at_least,
            )
        };
        assert_eq!(shown(sums), (2, 12, 4, 0, true));
        let (_, _, sums) = rows_as(
            vec![plain(&index, &mounts)],
            &every,
            None,
            by_name,
            Viewer::Everything,
        );
        assert_eq!(shown(sums), (4, 40, 4, 40, false));

        assert_eq!(paths(&compiled("size:>0", false), None, caller), [a, b]);

        let txt = compiled("txt", false);
        let totals = |viewer| {
            let found = search_totals(vec![plain(&index, &mounts)], &txt, None, viewer);
            let totals = found.expect("no scope, no overflow").totals;
            (totals.count, totals.size)
        };
        assert_eq!(totals(caller), (2, 12));
        assert_eq!(totals(Viewer::Everything), (4, 40));
    }

    /// A size sort places a filtered user's folders by name, on one
    /// volume and in the merge of two. Without it, the order of the tie runs
    /// or of the merge shows which folder is larger.
    #[test]
    fn a_size_sort_places_a_filtered_users_folders_by_name() {
        let index = visibility_index();
        let mounts = [[r"C:\".to_string()], [r"D:\".to_string()]];
        let may_list = |descriptor: &[u8]| descriptor == OPEN;
        let by_size = SortSpec {
            key: SortKey::Size,
            dir: SortDir::Desc,
        };
        let every = compiled("", false);
        let names = |volumes: Vec<PlainVolume>| {
            let (rows, _, _) = rows_as(volumes, &every, None, by_size, Viewer::Filtered(&may_list));
            rows.into_iter().map(|(path, _)| path).collect::<Vec<_>>()
        };
        assert_eq!(
            names(vec![plain(&index, &mounts[0])]),
            [
                r"C:\open\sub\b.txt",
                r"C:\open\a.txt",
                r"C:\hidden",
                r"C:\open",
                r"C:\open\sub",
                r"C:\unread"
            ]
        );
        assert_eq!(
            names(vec![plain(&index, &mounts[0]), plain(&index, &mounts[1])]),
            [
                r"C:\open\sub\b.txt",
                r"D:\open\sub\b.txt",
                r"C:\open\a.txt",
                r"D:\open\a.txt",
                r"C:\hidden",
                r"D:\hidden",
                r"C:\open",
                r"D:\open",
                r"C:\open\sub",
                r"D:\open\sub",
                r"C:\unread",
                r"D:\unread"
            ]
        );
    }

    /// Every page asks again whether the user sees each row. Here,
    /// between the search and its page, `sub` gets a descriptor the answers
    /// never saw, and `a.txt` moves into `hidden`. Without it, a long
    /// export shows a file that moved into a folder the user cannot list,
    /// and a descriptor added after the answers panics the daemon.
    #[test]
    fn a_page_leaves_out_a_row_the_user_no_longer_sees() {
        let volume = Arc::new(ChangingVolume {
            index: std::sync::RwLock::new(visibility_index()),
            mounts: [r"C:\".to_string()],
        });
        let may_list = |descriptor: &[u8]| descriptor == OPEN;
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: false,
        };
        let txt = compiled("txt", false);
        let viewer = Viewer::Filtered(&may_list);
        let mut rows =
            ResultRows::search(vec![volume.clone()], &txt, None, spec, viewer).expect("no scope");
        assert_eq!(rows.total(), 2, "a.txt and b.txt");
        {
            let mut index = volume.index.write().expect("no test panics holding it");
            index.set_folder_descriptor(Frn::ntfs(101, 1), b"changed");
            index.apply_batch(&[ParsedUsnRecord {
                frn: Frn::ntfs(200, 1),
                parent_frn: Frn::ntfs(102, 1),
                reason: crate::usn::record::USN_REASON_RENAME_NEW_NAME,
                attributes: 0,
                name: b"a.txt".to_vec(),
            }]);
        }
        let mut names = Vec::new();
        while rows.page(usize::MAX, |row, _| {
            names.push(row.name.to_vec());
            true
        }) {}
        assert!(names.is_empty(), "both rows are left out: {names:?}");
        assert_eq!(rows.total(), 0, "and the total counts neither");
        assert!(!rows.volumes_incomplete(), "the index kept its instance");
    }
}
