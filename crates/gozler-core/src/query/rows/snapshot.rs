//! A result snapshot: the rows of one search, kept after its reply, so a
//! client reads any of them later without searching again (DESIGN.md:
//! Result snapshots).
//!
//! [`ResultRows::into_snapshot`] keeps each volume's rows, which never
//! change after the search, and for a search over several volumes the
//! merged order: one byte for each row naming the volume of the next row,
//! and every [`CHECKPOINT_ROWS`] rows each volume's position. The merge
//! compares the values the indexes hold, so it is made once, a page at a
//! time under the read locks of the volumes the page reads, as a reply's
//! pages are, and a row that could not be sent then is dropped, because a
//! dead slot has no value to compare.
//!
//! [`ResultSnapshot::read`] reads the rows at an offset: it starts at the
//! checkpoint below the offset and replays at most `CHECKPOINT_ROWS - 1`
//! bytes, without reading an index. Each row is checked under its volume's
//! read lock as a page checks it ([`row_folder`]); a row that a page would
//! leave out is [`SnapshotRow::Gone`] and keeps its place, because a client
//! holds rows by their place. An index replaced since the search makes the
//! whole result snapshot gone.

use super::{IndexedVolume, ResultRows, RowView, VolumeRows, row_folder};
use crate::index::VolumeIndex;
use crate::query::engine::MatchSums;
use crate::query::paths::{FolderCopy, ResultNames};
use crate::types::EntryIdx;
use rustc_hash::FxHashMap;

/// Rows of the merged order between two checkpoints.
const CHECKPOINT_ROWS: usize = 4096;

/// Rows the merge reads under one hold of the locks, as a reply's page.
const MERGE_PAGE_ROWS: usize = 4096;

/// The volumes of the merged order are named by one byte.
const MAX_VOLUMES: usize = u8::MAX as usize + 1;

/// The rows of one search, which a client reads at any offset until the
/// result snapshot is dropped. It never changes after it is made, so its
/// readers share it.
pub struct ResultSnapshot {
    /// Each volume's rows in the result order, the volumes in the order the
    /// search was given them.
    answers: Vec<VolumeRows>,
    /// For a search over several volumes, which volume each row is on.
    order: Option<MergedOrder>,
    total: u64,
    volumes_incomplete: bool,
    metadata_pending: bool,
    sums: Option<MatchSums>,
}

/// The merged order of a search over several volumes.
struct MergedOrder {
    /// The volume of each row, in the result order.
    tags: Vec<u8>,
    /// At every [`CHECKPOINT_ROWS`]th row, each volume's next row.
    checkpoints: Vec<Box<[u32]>>,
}

/// A search over more volumes than the merged order can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a result snapshot reaches at most {MAX_VOLUMES} volumes")]
pub struct TooManyVolumes;

/// The result snapshot no longer answers: a volume's index was replaced, or
/// the volume is gone, since the search. A new search answers instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the result snapshot is gone; search again")]
pub struct ResultSnapshotGone;

/// A row of a result snapshot as a read finds it.
#[derive(Clone, Copy, Debug)]
pub enum SnapshotRow<'a> {
    /// The row's file now.
    Row(RowView<'a>),
    /// The file was deleted, moved out of the search's folder, or its
    /// folder has no full path now.
    Gone,
}

impl<V: IndexedVolume> ResultRows<V> {
    /// Keeps every row of this answer as a result snapshot, with the
    /// volumes it reads, in its order. The search asked for every row: no
    /// offset and no limit.
    ///
    /// # Errors
    ///
    /// [`TooManyVolumes`] for a search over more volumes than the merged
    /// order names.
    pub fn into_snapshot(mut self) -> Result<(ResultSnapshot, Vec<V>), TooManyVolumes> {
        let volumes = self.reply.volumes.len();
        if volumes > MAX_VOLUMES {
            return Err(TooManyVolumes);
        }
        let order = (volumes > 1).then(|| self.merge());
        let snapshot = ResultSnapshot {
            total: match &order {
                Some(order) => order.tags.len() as u64,
                None => self.reply.total(),
            },
            order,
            volumes_incomplete: self.volumes_incomplete(),
            metadata_pending: self.metadata_pending(),
            sums: self.sums,
            answers: self.reply.volumes.into_iter().map(|v| v.answer).collect(),
        };
        Ok((snapshot, self.volumes))
    }

    /// Walks the merged order once and keeps it, then drops from each
    /// volume's rows those the walk could not send.
    fn merge(&mut self) -> MergedOrder {
        let mut kept: Vec<Vec<bool>> = (self.reply.volumes.iter())
            .map(|v| vec![false; v.answer.rows.len()])
            .collect();
        let mut placed = vec![0u32; kept.len()];
        let mut tags = Vec::with_capacity(self.reply.rows_left());
        let mut checkpoints = Vec::new();
        while self.page_placed(MERGE_PAGE_ROWS, |v, at, _, _| {
            if tags.len() % CHECKPOINT_ROWS == 0 {
                checkpoints.push(placed.clone().into_boxed_slice());
            }
            tags.push(u8::try_from(v).expect("into_snapshot checked the volume count"));
            kept[v][at] = true;
            placed[v] += 1;
            true
        }) {}
        for (volume, kept) in self.reply.volumes.iter_mut().zip(&kept) {
            volume.answer.rows.retain_kept(kept);
        }
        MergedOrder { tags, checkpoints }
    }
}

impl ResultSnapshot {
    /// The rows it holds, a gone row counted.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether a volume searched was not complete, as
    /// [`ResultRows::volumes_incomplete`] said when it was made.
    pub fn volumes_incomplete(&self) -> bool {
        self.volumes_incomplete
    }

    /// Whether some sizes and dates were unknown when it was made.
    pub fn metadata_pending(&self) -> bool {
        self.metadata_pending
    }

    /// Every match's sums, when the search asked for them.
    pub fn sums(&self) -> Option<MatchSums> {
        self.sums
    }

    /// Reads the rows from `offset`, at most `limit` of them, from
    /// `volumes`: the volumes [`ResultRows::into_snapshot`] returned, in
    /// that order.
    ///
    /// # Panics
    ///
    /// When `volumes` holds another number of volumes.
    pub fn read<V: IndexedVolume>(
        &self,
        volumes: Vec<V>,
        offset: u32,
        limit: Option<u32>,
    ) -> SnapshotRows<'_, V> {
        assert_eq!(
            volumes.len(),
            self.answers.len(),
            "a read gives the volumes of the search"
        );
        let next = u64::from(offset).min(self.total);
        let end = limit.map_or(self.total, |limit| {
            (next + u64::from(limit)).min(self.total)
        });
        let starts: Vec<usize> = match &self.order {
            None => vec![next as usize; self.answers.len()],
            Some(order) => {
                let checkpoint = next as usize / CHECKPOINT_ROWS;
                match order.checkpoints.get(checkpoint) {
                    Some(start) => {
                        let mut starts: Vec<usize> = start.iter().map(|&p| p as usize).collect();
                        for &v in &order.tags[checkpoint * CHECKPOINT_ROWS..next as usize] {
                            starts[usize::from(v)] += 1;
                        }
                        starts
                    }
                    // Past the last row: nothing is read.
                    None => vec![0; self.answers.len()],
                }
            }
        };
        SnapshotRows {
            snapshot: self,
            volumes,
            readers: starts
                .into_iter()
                .map(|next| PlaceReader {
                    next,
                    folders: None,
                    scope_memo: FxHashMap::default(),
                    scope_gone: false,
                })
                .collect(),
            names: ResultNames::default(),
            next,
            end,
        }
    }

    /// The volume of the row at `at` in the result order.
    fn volume_of(&self, at: u64) -> usize {
        self.order
            .as_ref()
            .map_or(0, |order| usize::from(order.tags[at as usize]))
    }
}

/// One read of a [`ResultSnapshot`], a page at a time with [`Self::page`].
pub struct SnapshotRows<'s, V> {
    snapshot: &'s ResultSnapshot,
    volumes: Vec<V>,
    readers: Vec<PlaceReader>,
    /// The folders of every row read so far, of every volume.
    names: ResultNames,
    /// The place of the next row in the result order.
    next: u64,
    /// The place after the last row the read sends.
    end: u64,
}

/// One volume's rows as a read reads them.
struct PlaceReader {
    /// The next row of the volume.
    next: usize,
    /// The folders of the rows read so far; made with the first page that
    /// reads the volume.
    folders: Option<FolderCopy>,
    /// The page's memo of folders inside or outside the scope.
    scope_memo: FxHashMap<EntryIdx, bool>,
    /// The search's folder is gone, so every row of the volume is.
    scope_gone: bool,
}

impl<V: IndexedVolume> SnapshotRows<'_, V> {
    /// The rows the result snapshot holds: the total each page reports.
    pub fn total(&self) -> u64 {
        self.snapshot.total
    }

    /// The rows this read has still to send: what sizes a page.
    pub fn rows_left(&self) -> usize {
        usize::try_from(self.end - self.next).unwrap_or(usize::MAX)
    }

    pub fn volumes_incomplete(&self) -> bool {
        self.snapshot.volumes_incomplete
    }

    pub fn metadata_pending(&self) -> bool {
        self.snapshot.metadata_pending
    }

    /// Reads the next page under the read lock of each volume it reads, in
    /// the order the volumes were given, and hands each row to `push` with
    /// the folders it names, until `push` refuses one, the read has no row
    /// left, or it read `page_rows` rows. Returns whether rows are left: a
    /// row `push` refused is read again for the next page.
    ///
    /// # Errors
    ///
    /// [`ResultSnapshotGone`] when a volume's index was replaced since the
    /// search.
    pub fn page(
        &mut self,
        page_rows: usize,
        mut push: impl FnMut(SnapshotRow<'_>, &ResultNames) -> bool,
    ) -> Result<bool, ResultSnapshotGone> {
        let snapshot = self.snapshot;
        let page_end = self.end.min(self.next.saturating_add(page_rows as u64));
        let mut reads = vec![false; self.volumes.len()];
        for at in self.next..page_end {
            reads[snapshot.volume_of(at)] = true;
        }
        let held: Vec<Option<V::Index<'_>>> = self
            .volumes
            .iter()
            .zip(&reads)
            .map(|(volume, &reads)| reads.then(|| volume.read().index))
            .collect();
        for ((reader, answer), index) in self.readers.iter_mut().zip(&snapshot.answers).zip(&held) {
            let Some(index) = index.as_deref() else {
                continue;
            };
            if index.instance() != answer.instance {
                return Err(ResultSnapshotGone);
            }
            reader.begin_page(index, answer, &mut self.names);
        }
        while self.next < page_end {
            let v = snapshot.volume_of(self.next);
            let answer = &snapshot.answers[v];
            let reader = &mut self.readers[v];
            let index: &VolumeIndex = held[v].as_deref().expect("the page holds the row's volume");
            let row = answer.rows.get(reader.next);
            let folder = if reader.scope_gone {
                None
            } else {
                let folders = (reader.folders.as_mut()).expect("begin_page made the copy");
                row_folder(
                    index,
                    row,
                    answer.scope,
                    &mut reader.scope_memo,
                    folders,
                    &mut self.names,
                )
            };
            let found = match folder {
                Some(folder) => {
                    let entry = index.entry(row.0);
                    SnapshotRow::Row(RowView {
                        folder,
                        name: entry.name(),
                        is_dir: entry.is_dir(),
                        size: entry.size(),
                        mtime: entry.mtime(),
                        attributes: entry.attributes(),
                    })
                }
                None => SnapshotRow::Gone,
            };
            if !push(found, &self.names) {
                return Ok(true);
            }
            reader.next += 1;
            self.next += 1;
        }
        Ok(self.next < self.end)
    }
}

impl PlaceReader {
    /// Starts a page under `index`'s read lock.
    fn begin_page(&mut self, index: &VolumeIndex, answer: &VolumeRows, names: &mut ResultNames) {
        self.scope_memo.clear();
        self.scope_gone = answer
            .scope
            .is_some_and(|(scope, frn)| !index.holds(scope, frn));
        self.folders
            .get_or_insert_with(|| FolderCopy::new(index, &answer.mount_prefix, names))
            .forget_missing_paths();
    }
}
