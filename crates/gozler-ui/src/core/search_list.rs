//! The state of one list of gozlerd's results: Search's results list, or a
//! search panel's list (DESIGN.md: Search in a panel). Each list has its own
//! typed text, rows, selection and kept operations, so a search in one list
//! never changes another.

use super::results::Results;
use super::selection::{Selection, wide_path_into};
use gozler_core::proto::ResultItem;
use gozler_fileops::{OperationId, Request, Taken};
use std::collections::HashSet;
use std::sync::Arc;

/// Which rows a list's control unhighlights after its selection met taken
/// entries, which its rows then show blank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Deselect {
    /// The selection lost no file.
    Nothing,
    /// The selection lost every file, so every row, in one message,
    /// without reading which rows are selected.
    All,
    /// Each selected row whose entry is taken: the selection lost some of
    /// its files, or is read and may still name one.
    Taken,
}

/// One list of gozlerd's results: its typed text, its rows, its selection
/// and the ended operations it still shows blank
/// (DESIGN.md: The person never waits for the disk).
#[derive(Debug)]
pub(crate) struct SearchList {
    pub(super) text: String,
    pub(super) results: Results,
    pub(super) selection: Selection,
    /// The ended deletes and moves whose rows this list still shows blank
    /// until its next search, because the rows it shows are of the moment
    /// its search ran: each one that took a fetched row when it ended.
    pub(super) kept: HashSet<OperationId>,
    /// A search panel's selected size, which its status line shows; `None`
    /// while nothing is read, for Search's list, or when a counted folder's
    /// size is unknown.
    pub(super) chosen_bytes: Option<u64>,
}

/// The size of `files`, a search panel's selection, as its status line
/// adds it: each entry once, so a file below a selected folder is left out,
/// and `None` when a counted size is unknown, as a folder panel's sum
/// (DESIGN.md: Search in a panel). Only the depths at which a selected folder
/// exists are looked up.
fn chosen_bytes(files: &[ResultItem]) -> Option<u64> {
    let folders: HashSet<&str> = (files.iter())
        .filter(|file| file.is_dir)
        .map(|folder| folder.path.trim_end_matches('\\'))
        .collect();
    let mut depths: Vec<usize> = (folders.iter())
        .map(|folder| folder.split('\\').filter(|c| !c.is_empty()).count())
        .collect();
    depths.sort_unstable();
    depths.dedup();
    let below_selected = |path: &str| {
        let mut depth = 0;
        path.match_indices('\\').any(|(at, _)| {
            let ancestor = &path[..at];
            if !ancestor.is_empty() && !ancestor.ends_with('\\') {
                depth += 1;
            }
            depths.binary_search(&depth).is_ok() && folders.contains(ancestor)
        })
    };
    (files.iter())
        .filter(|file| folders.is_empty() || !below_selected(&file.path))
        .map(|file| file.size)
        .sum()
}

impl SearchList {
    pub(crate) fn new() -> SearchList {
        SearchList {
            text: String::new(),
            results: Results::new(),
            selection: Selection::None,
            kept: HashSet::new(),
            chosen_bytes: None,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn results(&self) -> &Results {
        &self.results
    }

    pub(crate) fn selection(&self) -> &Selection {
        &self.selection
    }

    pub(crate) fn chosen_bytes(&self) -> Option<u64> {
        self.chosen_bytes
    }

    /// Forgets the rows, the selection and the kept operations, but not the
    /// typed text, while a search panel does not show, because it costs
    /// memory only while it shows (DESIGN.md: Search in a panel).
    pub(crate) fn drop_rows(&mut self) {
        self.results = Results::new();
        self.selection = Selection::None;
        self.kept.clear();
        self.chosen_bytes = None;
    }

    /// Adds up the selection's size again, for a search panel's status
    /// line, after its files changed.
    pub(super) fn count_chosen_bytes(&mut self) {
        self.chosen_bytes = self.selection.files().and_then(chosen_bytes);
    }

    /// The row at `index` as the list shows it: `None` while its chunk has
    /// not arrived, or while a delete or a move has taken its entry.
    pub(crate) fn shown_row(&self, index: u64, taken: &Taken) -> Option<&ResultItem> {
        self.results
            .row(index)
            .filter(|row| !is_taken(taken, &self.kept, row))
    }

    /// Whether a delete or a move has taken the entry of the row at `index`.
    pub(crate) fn row_taken(&self, index: u64, taken: &Taken) -> bool {
        self.results
            .row(index)
            .is_some_and(|row| is_taken(taken, &self.kept, row))
    }

    /// The operations `ended`, the operations `in_queue`, and the taken
    /// entries `taken` that the list shows next: the list keeps an ended
    /// delete or move that takes one of its fetched rows, until its next
    /// search or until it leaves the queue. The fetched rows are walked
    /// once, each encoded and folded into one buffer and looked up once for
    /// every ended delete or move, and only while one is not yet kept.
    pub(crate) fn ended(
        &mut self,
        ended: &[(OperationId, Arc<Request>)],
        in_queue: &HashSet<OperationId>,
        taken: &Taken,
    ) {
        let mut wanted: Vec<OperationId> = (ended.iter())
            .filter(|(id, request)| {
                matches!(**request, Request::Delete { .. } | Request::Move { .. })
                    && !self.kept.contains(id)
            })
            .map(|(id, _)| *id)
            .collect();
        let (mut path, mut folded) = (Vec::new(), Vec::new());
        for row in self.results.fetched() {
            if wanted.is_empty() {
                break;
            }
            wide_path_into(row, &mut path);
            taken.takers(&path, &mut folded, |takers| {
                wanted.retain(|id| {
                    let takes = takers.contains(id);
                    if takes {
                        self.kept.insert(*id);
                    }
                    !takes
                });
            });
        }
        // From the queue, not from `taken`, which can be older and miss an
        // operation that ended earlier.
        self.kept.retain(|id| in_queue.contains(id));
    }

    /// Whether the list still shows the rows of the ended operation `id`
    /// blank, so it stays in the queue.
    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        self.kept.contains(&id)
    }

    /// Which rows the list's control unhighlights, after
    /// [`SearchList::drop_taken`] said whether the selection `lost` files.
    pub(crate) fn deselect(&self, lost: bool) -> Deselect {
        match &self.selection {
            Selection::Reading { .. } => Deselect::Taken,
            Selection::None if lost => Deselect::All,
            _ if lost => Deselect::Taken,
            _ => Deselect::Nothing,
        }
    }

    /// Takes the entries of `taken` out of the selection; the list shows
    /// their rows blank once it draws them again. Returns whether the
    /// selection lost files.
    pub(crate) fn drop_taken(&mut self, taken: &Taken) -> bool {
        if taken.is_empty() || self.selection.files().is_none() {
            return false;
        }
        let selected = self.selection.files().map_or(0, <[_]>::len);
        let mut files = match std::mem::replace(&mut self.selection, Selection::None) {
            Selection::Files(files) => files,
            Selection::Folder { folder, contents } => {
                if !is_taken(taken, &self.kept, &folder) {
                    self.selection = Selection::Folder { folder, contents };
                }
                Vec::new()
            }
            other => {
                self.selection = other;
                Vec::new()
            }
        };
        retain_untaken(taken, &self.kept, &mut files);
        if !files.is_empty() {
            self.selection = Selection::Files(files);
        }
        self.selection.files().map_or(0, <[_]>::len) != selected
    }
}

/// Whether a delete or a move has taken `row`'s entry, by its exact name
/// when its UTF-8 path lost an unpaired surrogate. The window asks for every
/// cell it paints, so an empty set answers before the path is encoded.
pub(super) fn is_taken(taken: &Taken, kept: &HashSet<OperationId>, row: &ResultItem) -> bool {
    !taken.is_empty() && taken.contains(&super::selection::wide_path(row), |id| kept.contains(&id))
}

/// Keeps the `files` that no delete or move has taken, as [`is_taken`]
/// says, encoding and folding every path into the same two buffers, because
/// a selection can hold [`super::selection::SELECTION_LIMIT`] files.
pub(crate) fn retain_untaken(
    taken: &Taken,
    kept: &HashSet<OperationId>,
    files: &mut Vec<ResultItem>,
) {
    if taken.is_empty() {
        return;
    }
    let (mut path, mut folded) = (Vec::new(), Vec::new());
    files.retain(|file| {
        wide_path_into(file, &mut path);
        !taken.contains_folding(&path, &mut folded, |id| kept.contains(&id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, size: Option<u64>, is_dir: bool) -> ResultItem {
        ResultItem {
            path: path.to_owned(),
            path_u16: None,
            size,
            mtime_ft: None,
            created_ft: None,
            is_dir,
            attributes: None,
        }
    }

    /// T8: a folder's size already holds the files below it, so a sum of
    /// every selected size counts them twice; an unknown folder size hides
    /// the sum, as a folder panel's does.
    #[test]
    fn a_file_below_a_selected_folder_counts_once() {
        let files = |folder_size| {
            vec![
                item(r"C:\a", folder_size, true),
                item(r"C:\a\b.txt", Some(4), false),
                item(r"C:\c.txt", Some(1), false),
            ]
        };
        assert_eq!(chosen_bytes(&files(Some(10))), Some(11));
        assert_eq!(chosen_bytes(&files(None)), None);
    }
}
