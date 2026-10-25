//! The selection: a set of files, built at one moment (DESIGN.md:
//! window rule 10). The list owns which positions are selected; the model
//! builds the files from them once per user action (DESIGN.md: The
//! selection).

use super::fetch::{CachedRows, Generation};
use gozler_core::proto::{QueryTotals, ResultItem};
use gozler_fileops::{OperationId, Taken};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

mod capture;
pub(crate) use capture::{CaptureKey, Capturing, Telling, Told};
pub(crate) mod changes;

/// Open and Open file location stop here, because File Explorer asks before
/// it opens more items, and many windows opened by accident are worse
/// (DESIGN.md: Commands).
pub(crate) const OPEN_LIMIT: usize = 15;

/// A drag holds at most this many entries, because the window names each
/// one and builds the shell's data object on its thread before the drag
/// starts, and Edit > Copy has no limit (DESIGN.md: Dragging).
pub(crate) const DRAG_LIMIT: usize = 1_000;

/// The most rows a selection holds. A list reaches every match, and Select
/// all would hold about 20 MB of files for each 100,000 rows (DESIGN.md:
/// The selection).
pub(crate) const SELECTION_LIMIT: u64 = 100_000;

/// The positions and count observed in one list selection. Windows supplies the
/// count without walking a scattered selection; worker-made ranges count once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowSelection {
    pub(crate) positions: Arc<Vec<Range<u64>>>,
    pub(crate) count: u64,
}

impl RowSelection {
    pub(crate) fn new(positions: Vec<Range<u64>>, count: u64) -> Self {
        Self {
            positions: Arc::new(positions),
            count,
        }
    }
}

impl From<Vec<Range<u64>>> for RowSelection {
    fn from(mut positions: Vec<Range<u64>>) -> Self {
        positions.retain(|range| !range.is_empty());
        let count = positions.iter().map(|range| range.end - range.start).sum();
        Self::new(positions, count)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Selection {
    None,
    /// Some selected rows are not cached; their one request is due, or in
    /// flight as `asked`.
    Reading {
        positions: Arc<Vec<Range<u64>>>,
        asked: Option<Generation>,
    },
    Filtering {
        generation: Generation,
        source: SelectionSource,
        running: bool,
    },
    /// In position order, each at most once. In Search's list never one
    /// folder alone, which is [`Selection::Folder`].
    Files(Arc<[ResultItem]>),
    /// One folder alone, whose contents the inspector shows (DESIGN.md:
    /// Inspector pane).
    Folder {
        folder: ResultItem,
        contents: FolderContents,
    },
    /// More rows than [`SELECTION_LIMIT`] are selected, this many, so no
    /// command acts on them and none is read.
    TooMany(u64),
}

/// What the inspector knows of the contents of the one selected folder.
#[derive(Debug, PartialEq)]
pub(crate) enum FolderContents {
    /// Its one request is in flight as this generation.
    Calculating(Generation),
    Counted(QueryTotals),
}

impl Selection {
    /// The selected files, once they are read; `None` while nothing is
    /// selected or the selection is read.
    pub(crate) fn files(&self) -> Option<&[ResultItem]> {
        match self {
            Selection::Files(files) => Some(files),
            Selection::Folder { folder, .. } => Some(std::slice::from_ref(folder)),
            Selection::None
            | Selection::Reading { .. }
            | Selection::Filtering { .. }
            | Selection::TooMany(_) => None,
        }
    }

    pub(crate) fn shared_files(&self) -> Option<Arc<[ResultItem]>> {
        match self {
            Self::Files(files) => Some(Arc::clone(files)),
            Self::Folder { folder, .. } => Some(Arc::from([folder.clone()])),
            _ => None,
        }
    }

    /// The request in flight for the selection: its rows, or the contents of
    /// its folder.
    pub(crate) fn asked(&self) -> Option<Generation> {
        match self {
            Selection::Reading { asked, .. } => *asked,
            Selection::Folder {
                contents: FolderContents::Calculating(generation),
                ..
            } => Some(*generation),
            Selection::None
            | Selection::Files(_)
            | Selection::Filtering { .. }
            | Selection::TooMany(_)
            | Selection::Folder {
                contents: FolderContents::Counted(_),
                ..
            } => None,
        }
    }

    pub(crate) fn items(&self) -> usize {
        match self {
            Self::Files(files) => files.len(),
            Self::Filtering { source, .. } => source.retained_items(),
            Self::Reading { positions, .. } => positions.len(),
            Self::Folder { .. } => 1,
            Self::None | Self::TooMany(_) => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelectionSource {
    Files(Arc<[ResultItem]>),
    Cached {
        chunks: Vec<CachedRows>,
        positions: Arc<Vec<Range<u64>>>,
        count: usize,
    },
}

impl SelectionSource {
    pub(crate) fn items(&self) -> usize {
        match self {
            Self::Files(files) => files.len(),
            Self::Cached { count, .. } => *count,
        }
    }

    pub(crate) fn retained_items(&self) -> usize {
        match self {
            Self::Files(files) => files.len(),
            Self::Cached {
                chunks, positions, ..
            } => positions.len() + chunks.iter().map(|chunk| chunk.rows.len()).sum::<usize>(),
        }
    }
}

pub(crate) struct Filter {
    pub(crate) generation: Generation,
    pub(crate) source: SelectionSource,
    pub(crate) taken: Arc<Taken>,
    pub(crate) kept: Arc<HashSet<OperationId>>,
    pub(crate) count_bytes: bool,
}

pub(crate) struct Filtered {
    pub(crate) input: Filter,
    pub(crate) files: Option<Arc<[ResultItem]>>,
    pub(crate) chosen_bytes: Option<u64>,
    pub(crate) sums: Sums,
}

impl Filter {
    pub(crate) fn retained_items(&self) -> usize {
        self.source.retained_items() + self.taken.len() + self.kept.len()
    }

    pub(crate) fn run(self) -> Filtered {
        let (mut path, mut folded) = (Vec::new(), Vec::new());
        let mut valid = |file: &ResultItem| {
            if file.is_gone() {
                return false;
            }
            if self.taken.is_empty() {
                return true;
            }
            wide_path_into(file, &mut path);
            !self
                .taken
                .contains_folding(&path, &mut folded, |id| self.kept.contains(&id))
        };
        let files: Option<Arc<[ResultItem]>> = match &self.source {
            SelectionSource::Files(files) if files.iter().all(&mut valid) => {
                Some(Arc::clone(files))
            }
            SelectionSource::Files(files) => {
                Some(files.iter().filter(|file| valid(file)).cloned().collect())
            }
            SelectionSource::Cached {
                chunks, positions, ..
            } => {
                let row = |at: u64| {
                    chunks.iter().find_map(|chunk| {
                        at.checked_sub(chunk.range.start)
                            .and_then(|index| chunk.rows.get(index as usize))
                    })
                };
                let positions = positions.iter().flat_map(Range::clone);
                if positions.clone().all(|at| row(at).is_some()) {
                    Some(
                        positions
                            .filter_map(row)
                            .filter(|file| valid(file))
                            .cloned()
                            .collect(),
                    )
                } else {
                    None
                }
            }
        };
        let chosen_bytes = if self.count_bytes {
            files.as_deref().and_then(super::search_list::chosen_bytes)
        } else {
            None
        };
        let sums = Sums::of(files.as_deref().unwrap_or_default());
        Filtered {
            input: self,
            files,
            chosen_bytes,
            sums,
        }
    }
}

/// What the status bar and the inspector sum from the selected entries
/// (DESIGN.md: Status bar, Inspector pane).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sums {
    pub(crate) files: u64,
    pub(crate) folders: u64,
    /// The size of the files whose size is known. A folder's size counts only
    /// through the files below it, so a selected folder and a selected file
    /// below it would count twice; it is left out.
    pub(crate) bytes: u64,
    /// The files whose size is unknown.
    pub(crate) unknown: u64,
}

impl Sums {
    pub(crate) fn of(entries: &[ResultItem]) -> Sums {
        entries.iter().fold(Sums::default(), |mut sums, entry| {
            match (entry.is_dir, entry.size) {
                (true, _) => sums.folders += 1,
                (false, Some(size)) => {
                    sums.files += 1;
                    sums.bytes += size;
                }
                (false, None) => {
                    sums.files += 1;
                    sums.unknown += 1;
                }
            }
            sums
        })
    }

    pub(crate) fn entries(self) -> u64 {
        self.files + self.folders
    }
}

/// The full path of `file` as UTF-16, exactly as Windows names it, also
/// when the name holds an unpaired surrogate that its UTF-8 `path` lost.
pub(crate) fn wide_path(file: &ResultItem) -> Vec<u16> {
    let mut path = Vec::new();
    wide_path_into(file, &mut path);
    path
}

/// [`wide_path`] into `path`, which it empties first, so a pass over many
/// rows reuses one buffer.
pub(crate) fn wide_path_into(file: &ResultItem, path: &mut Vec<u16>) {
    path.clear();
    match &file.path_u16 {
        Some(units) => path.extend_from_slice(units),
        None => path.extend(file.path.encode_utf16()),
    }
}

/// A change of a list's selected rows, as the list's notification tells
/// it (DESIGN.md: The selection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowsChange {
    /// One row, as a Ctrl+click tells it.
    Row { row: u64, selected: bool },
    /// A range of rows, as Shift+End tells it.
    Rows { rows: Range<u64>, selected: bool },
    /// Every row, as Select all, or a click that clears the others, tells
    /// it.
    All { selected: bool },
}

/// The rows a list selects, as its notifications told them, so the window
/// knows a large selection after a change without a message for each
/// selected row. The list stays the owner: the window checks these rows
/// against it before it uses them (DESIGN.md: The selection).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToldRows {
    /// Each run of selected rows, by its first row, to the row after its
    /// last; runs neither overlap nor touch. A run can reach past the list's
    /// rows, which a shorter list dropped without a notification.
    runs: BTreeMap<u64, u64>,
}

impl ToldRows {
    /// The rows `ranges`, sorted ranges that do not touch, read from the
    /// list.
    pub(crate) fn read(ranges: &[Range<u64>]) -> ToldRows {
        ToldRows {
            runs: ranges
                .iter()
                .map(|range| (range.start, range.end))
                .collect(),
        }
    }

    pub(crate) fn change(&mut self, change: RowsChange) {
        match change {
            RowsChange::Row { row, selected } => self.set(row..row + 1, selected),
            RowsChange::Rows { rows, selected } => self.set(rows, selected),
            RowsChange::All { selected } => {
                self.runs.clear();
                if selected {
                    self.runs.insert(0, u64::MAX);
                }
            }
        }
    }

    fn set(&mut self, rows: Range<u64>, selected: bool) {
        if rows.is_empty() {
            return;
        }
        // The runs that `rows` overlaps, and for a selection also those it
        // touches, which merge with it.
        let last_start = if selected { rows.end } else { rows.end - 1 };
        let met: Vec<(u64, u64)> = (self.runs.range(..=last_start).rev())
            .take_while(|&(_, &end)| end > rows.start || selected && end == rows.start)
            .map(|(&start, &end)| (start, end))
            .collect();
        for (start, _) in &met {
            self.runs.remove(start);
        }
        if selected {
            let start = met.iter().map(|run| run.0).fold(rows.start, u64::min);
            let end = met.iter().map(|run| run.1).fold(rows.end, u64::max);
            self.runs.insert(start, end);
        } else {
            for (start, end) in met {
                if start < rows.start {
                    self.runs.insert(start, rows.start);
                }
                if end > rows.end {
                    self.runs.insert(rows.end, end);
                }
            }
        }
    }

    /// The selected rows of a list of `count` rows, as sorted ranges that
    /// do not touch.
    pub(crate) fn rows(&self, count: u64) -> Vec<Range<u64>> {
        (self.runs.range(..count))
            .map(|(&start, &end)| start..end.min(count))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn change() -> impl Strategy<Value = RowsChange> {
        prop_oneof![
            (0..40u64, any::<bool>()).prop_map(|(row, selected)| RowsChange::Row { row, selected }),
            (0..40u64, 0..12u64, any::<bool>()).prop_map(|(start, len, selected)| {
                RowsChange::Rows {
                    rows: start..start + len,
                    selected,
                }
            }),
            any::<bool>().prop_map(|selected| RowsChange::All { selected }),
        ]
    }

    proptest! {
        /// The window acts on these rows in place of the list's, so after
        /// any notifications they are the rows the list selects: one row
        /// too many or too few is a command on a file the person did not
        /// select, or none on one they did.
        #[test]
        fn told_rows_follow_every_change(changes in proptest::collection::vec(change(), 0..30)) {
            let count = 32;
            let mut told = ToldRows::default();
            let mut list = BTreeSet::new();
            for change in changes {
                match &change {
                    RowsChange::Row { row, selected } => {
                        if *selected { list.insert(*row); } else { list.remove(row); }
                    }
                    RowsChange::Rows { rows, selected } => {
                        for row in rows.clone() {
                            if *selected { list.insert(row); } else { list.remove(&row); }
                        }
                    }
                    RowsChange::All { selected } => {
                        list.clear();
                        if *selected { list.extend(0..count); }
                    }
                }
                told.change(change);
            }
            let rows: Vec<u64> = told.rows(count).into_iter().flatten().collect();
            let expected: Vec<u64> = list.into_iter().filter(|row| *row < count).collect();
            prop_assert_eq!(rows, expected);
            let runs = told.rows(u64::MAX);
            prop_assert!(runs.windows(2).all(|pair| pair[0].end < pair[1].start));
        }
    }
}
