//! A search's answer as rows, read from the index one page at a time.
//!
//! After the sort, each match becomes its row: its entry and its file ID,
//! and nothing else, so an answer of millions of rows holds no name and no
//! folder. A page is read while the caller holds the index read lock of each
//! volume it reads: each row is the file as it is then, with its current
//! name, size and date, as Everything's list shows a renamed file. A row
//! whose slot is dead or holds another file, that left the search's scope,
//! or whose folder has no full path is left out, and the reply's total
//! counts it out, because `gozler` reads the last page's total and exits 10
//! when it shows fewer rows. A volume whose index was replaced or got new
//! entry indexes sends no more rows, and the reply says so.
//!
//! [`Reply`] merges several volumes' rows one page at a time, by [`compare`]
//! or, for a path sort, by the folders' folded paths, because folder ranks
//! order only one volume's folders. A limited search keeps each volume's
//! first `offset + limit` rows. When a volume whose limit cut off later
//! matches runs out of rows because some were left out, its next match is
//! unknown, so the reply ends there, short, rather than send a later row of
//! another volume before it.

use super::CompiledQuery;
use super::engine::{MatchSums, find_matches, in_scope};
use super::order::{OrderFields, SortRecord, compare, compare_paths};
use super::paths::{FolderCopy, FolderId, ResultNames, root_name};
use crate::attributes::Attributes;
use crate::fold::fold;
use crate::index::VolumeIndex;
use crate::types::{EntryIdx, Frn, SortDir, SortKey, SortSpec};
use core::cmp::Ordering;
use rustc_hash::FxHashMap;

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
/// [`super::OrderedMatches::read_rows`].
pub struct VolumeRows {
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
    pub fn sums(&self) -> Option<MatchSums> {
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
    pub attributes: Attributes,
}

/// The rows of every volume of one reply, merged in the result order and
/// read a page at a time with [`Self::fill`].
pub struct Reply {
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
    /// The next row to read.
    next: usize,
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
    /// A reply of `answers`, one for each volume of the same search, which
    /// skips the first `offset` rows and sends at most `limit`.
    pub fn new(answers: Vec<VolumeRows>, offset: u32, limit: Option<u32>) -> Self {
        let sort = answers.first().map_or_else(SortSpec::default, |a| a.sort);
        assert!(
            answers.iter().all(|a| a.sort == sort),
            "the answers of one search share its order"
        );
        let merge = match sort.key {
            SortKey::Path => merge_paths(sort.dir, &answers),
            _ => Merge::Compare,
        };
        Self {
            total: answers
                .iter()
                .map(|a| a.rows.len() as u64 + a.past_cut)
                .sum(),
            volumes: answers
                .into_iter()
                .map(|answer| VolumeReader {
                    answer,
                    next: 0,
                    head: None,
                    folders: None,
                    scope_memo: FxHashMap::default(),
                    folder_path: None,
                })
                .collect(),
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
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether a volume's index was replaced, or got new entry indexes,
    /// during the reply, so its rows stopped.
    pub fn index_changed(&self) -> bool {
        self.index_changed
    }

    /// The rows not read yet, at most the limit's: what sizes a page.
    pub fn rows_left(&self) -> usize {
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
    pub fn reads_volume(&self, v: usize) -> bool {
        match &self.merge {
            Merge::InTurn(order) => order.iter().find(|&&w| self.volumes[w].has_rows()) == Some(&v),
            Merge::Compare | Merge::ComparePaths => self.volumes[v].has_rows(),
        }
    }

    /// Reads the next rows from `indexes`, one for each answer given to
    /// [`Self::new`], in their order: the index held under its read lock
    /// where [`Self::reads_volume`] says the page reads it, and `None`
    /// elsewhere. Hands each row to `push` with the folders it names, until
    /// `push` refuses one, the page has no row left, or it read `page_rows`
    /// rows, sent or skipped for the offset, so that one hold of the locks
    /// does a page's work at most. Returns whether rows are left: a row
    /// `push` refused is read again for the next page.
    pub fn fill(
        &mut self,
        indexes: &[Option<&VolumeIndex>],
        page_rows: usize,
        mut push: impl FnMut(RowView<'_>, &ResultNames) -> bool,
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
                size: entry.size(),
                mtime: entry.mtime(),
                attributes: entry.attributes(),
            };
            if !push(row, &self.names) {
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
                if volume.answer.past_cut > 0 {
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
                if volumes[v].answer.past_cut > 0 {
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

/// How a path sort merges `answers`: in turn, in the order of their root
/// paths, when no root path starts another's, and otherwise by comparing
/// paths. A root path is the folded [`root_name`].
fn merge_paths(dir: SortDir, answers: &[VolumeRows]) -> Merge {
    let roots: Vec<Vec<u8>> = answers
        .iter()
        .map(|a| fold(root_name(&a.mount_prefix)))
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
    let mut order: Vec<usize> = (0..answers.len()).collect();
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
                *total -= self.answer.past_cut;
            }
            self.next = self.answer.rows.len();
            self.answer.past_cut = 0;
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
            let (idx, frn) = self.answer.rows.get(self.next);
            let sendable = index.holds(idx, frn)
                && match self.answer.scope {
                    Some((scope, _)) => in_scope(index, idx, scope, &mut self.scope_memo),
                    None => true,
                };
            if sendable {
                self.head = folders.entry_folder(index, idx, names);
            }
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

    /// Whether this volume's next row comes before `other`'s, by their folded
    /// folder paths when `by_paths`, which [`Self::note_folder_path`] noted.
    fn precedes(
        &self,
        other: &VolumeReader,
        index: &VolumeIndex,
        other_index: &VolumeIndex,
        sort: SortSpec,
        by_paths: bool,
    ) -> bool {
        let (a, b) = (self.live(index), other.live(other_index));
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

    /// The next row, as the merge reads it from `index`.
    fn live<'a>(&'a self, index: &'a VolumeIndex) -> Live<'a> {
        Live {
            index,
            idx: self.answer.rows.get(self.next).0,
            volume: self.answer.mount_prefix.as_bytes(),
        }
    }
}

/// A row as the merge of several volumes reads it: straight from its index.
struct Live<'a> {
    index: &'a VolumeIndex,
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
        self.index.entry(self.idx).size()
    }
    fn mtime(&self) -> Option<i64> {
        self.index.entry(self.idx).mtime()
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
/// as [`find_matches`] says, and reads its rows as one page, for a caller
/// that holds the index for the whole query and wants full paths: tests, the
/// benches and the daemon's demo query.
pub fn run_query(
    index: &VolumeIndex,
    mount_prefix: &str,
    query: &CompiledQuery,
    scope: Option<EntryIdx>,
    sort: SortSpec,
    limit: Option<u32>,
) -> QueryOutcome {
    let answer = find_matches(index, mount_prefix, query, scope, sort, limit, false)
        .order()
        .read_rows(index)
        .expect("the index is borrowed for the whole query, so it cannot change");
    let mut reply = Reply::new(vec![answer], 0, None);
    let mut rows = Vec::new();
    reply.fill(&[Some(index)], usize::MAX, |row, names| {
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

    /// The folders of every volume of the merge: `Data` and `Data2` beside
    /// the volumes mounted at `C:\Data\` and `C:\Data2\`, `a b`, whose space
    /// sorts before `\`, and `A`, which folds to `a`.
    const FOLDERS: [&str; 4] = ["Data", "Data2", "a b", "A"];

    /// A file of a volume: the folder that holds it (past the last of
    /// [`FOLDERS`], the root), its name, and its size and date.
    type File = (usize, String, Option<u64>);

    fn volume(files: &[File]) -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        let frn = |n: usize| Frn((n as u128 + 100) | (1 << 48));
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
        /// paths interleave, and `D:\` follows `C:\`.
        #[test]
        fn merged_volumes_follow_the_result_order_over_all_their_rows(
            mounts in prop::sample::subsequence(vec![r"C:\", r"C:\Data\", r"C:\Data2\", r"D:\"], 2..=4),
            files in prop::collection::vec(prop::collection::vec(file(), 0..8), 4),
        ) {
            let indexes: Vec<VolumeIndex> = mounts.iter().zip(&files).map(|(_, f)| volume(f)).collect();
            let held: Vec<&VolumeIndex> = indexes.iter().collect();
            let query = CompiledQuery::new(&parse_query("").expect("parses")).expect("compiles");
            for sort in all_sorts() {
                let answers: Vec<VolumeRows> = mounts
                    .iter()
                    .zip(&held)
                    .map(|(mount, index)| {
                        find_matches(index, mount, &query, None, sort, None, false)
                            .order()
                            .read_rows(index)
                            .expect("nothing changed")
                    })
                    .collect();

                // Every row of every volume, sorted by the result order.
                let all: Vec<(usize, EntryIdx)> = answers
                    .iter()
                    .enumerate()
                    .flat_map(|(v, a)| (0..a.rows.len()).map(move |at| (v, a.rows.get(at).0)))
                    .collect();
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
                let live = |at: usize| {
                    let (v, idx) = all[at];
                    Live { index: held[v], idx, volume: mounts[v].as_bytes() }
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

                let mut reply = Reply::new(answers, 0, None);
                let mut merged = Vec::new();
                loop {
                    let page: Vec<Option<&VolumeIndex>> = (0..held.len())
                        .map(|v| reply.reads_volume(v).then_some(held[v]))
                        .collect();
                    let more = reply.fill(&page, usize::MAX, |row, names| {
                        let mut path = Vec::new();
                        names.full_path_into(row.folder, row.name, &mut path);
                        merged.push(path);
                        true
                    });
                    if !more {
                        break;
                    }
                }
                prop_assert_eq!(&merged, &expected, "{:?} on {:?}", sort, mounts);
            }
        }
    }
}
