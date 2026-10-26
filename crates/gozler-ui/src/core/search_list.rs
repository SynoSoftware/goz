//! The state of one list of gozlerd's results: Search's results list, or a
//! search panel's list (DESIGN.md: Search in a panel). Each list has its own
//! typed text, rows, selection and kept operations, so a search in one list
//! never changes another.

mod keeping;

pub(crate) use keeping::{EndEvidence, Ended, Keep, Kept};

use super::fetch::Generation;
use super::results::Results;
use super::selection::{Selection, Sums};
use super::work::{self, Retirement};
use gozler_core::proto::ResultItem;
use gozler_fileops::{OperationId, Taken};
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
    pub(super) selection_sums: Sums,
    /// The ended deletes and moves whose rows this list still shows blank
    /// until its next search, because the rows it shows are of the moment
    /// its search ran: each one that took a fetched row when it ended.
    pub(super) kept: Arc<HashSet<OperationId>>,
    keeping: Option<Generation>,
    pending: keeping::Pending,
    observing: Option<Generation>,
    /// A search panel's selected size, which its status line shows; `None`
    /// while nothing is read, and for Search's list.
    pub(super) chosen_bytes: Option<SelectedSize>,
    retired: Retirement,
}

/// The size of a search panel's selection, as its status line shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectedSize {
    /// The sum of the known sizes; `None` when no counted size is known.
    pub(crate) known: Option<u64>,
    /// How many counted entries have no known size.
    pub(crate) unknown: u64,
}

/// The size of `files`, a search panel's selection, as its status line
/// adds it: each entry once, so a file below a selected folder is left out,
/// and the unknown sizes counted apart, as a folder panel's sum (DESIGN.md:
/// Search in a panel). Only the depths at which a selected folder exists
/// are looked up.
pub(super) fn chosen_bytes(files: &[ResultItem]) -> SelectedSize {
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
    let mut size = SelectedSize {
        known: None,
        unknown: 0,
    };
    for file in files {
        if !folders.is_empty() && below_selected(&file.path) {
            continue;
        }
        match file.size {
            Some(bytes) => size.known = Some(size.known.unwrap_or(0) + bytes),
            None => size.unknown += 1,
        }
    }
    size
}

impl SearchList {
    pub(crate) fn new() -> SearchList {
        SearchList {
            text: String::new(),
            results: Results::new(),
            selection: Selection::None,
            selection_sums: Sums::default(),
            kept: Arc::default(),
            keeping: None,
            pending: keeping::Pending::default(),
            observing: None,
            chosen_bytes: None,
            retired: Retirement::default(),
        }
    }

    pub(crate) fn items(&self) -> usize {
        self.results.items() + self.selection.items() + self.kept.len() + self.pending.items()
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        let mut retired = std::mem::take(&mut self.retired);
        retired.append(self.results.take_retired());
        retired
    }

    pub(super) fn set_selection(&mut self, selection: Selection) {
        self.selection_sums = Sums::default();
        let items = self.selection.items();
        let previous = std::mem::replace(&mut self.selection, selection);
        self.retired.push(previous, items);
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

    pub(crate) fn selection_sums(&self) -> Sums {
        self.selection_sums
    }

    pub(crate) fn chosen_bytes(&self) -> Option<SelectedSize> {
        self.chosen_bytes
    }

    /// Forgets the rows, the selection and the kept operations, but not the
    /// typed text, while a search panel does not show, because it costs
    /// memory only while it shows (DESIGN.md: Search in a panel).
    pub(crate) fn drop_rows(&mut self) {
        self.results.clear();
        self.set_selection(Selection::None);
        self.reset_kept();
        self.chosen_bytes = None;
    }

    /// The row at `index` as the list shows it: `None` while its chunk has
    /// not arrived, or while a delete or a move has taken its entry.
    pub(crate) fn shown_row(&self, index: u64, taken: &Taken) -> Option<&ResultItem> {
        self.results
            .row(index)
            .filter(|row| !self.takes(taken, row))
    }

    /// Whether a delete or a move has taken the entry of the row at `index`.
    pub(crate) fn row_taken(&self, index: u64, taken: &Taken) -> bool {
        self.results
            .row(index)
            .is_some_and(|row| self.takes(taken, row))
    }

    pub(super) fn takes(&self, taken: &Taken, row: &ResultItem) -> bool {
        !taken.is_empty()
            && taken.contains(&super::selection::wide_path(row), |id| {
                self.is_keeping() || self.kept.contains(&id)
            })
    }

    pub(crate) fn observe_end(&mut self) -> Option<EndEvidence> {
        let generation = self.results.generation()?;
        self.observing = Some(generation);
        Some(EndEvidence {
            generation,
            rows: self.results.cached(),
        })
    }

    pub(crate) fn accept_end(&mut self, evidence: EndEvidence, ended: Option<Arc<Ended>>) {
        if self.observing == Some(evidence.generation) {
            self.observing = None;
        }
        if self.results.generation() != Some(evidence.generation) || ended.is_none() {
            let items = evidence.items();
            self.retired.push(evidence, items);
            return;
        }
        self.pending
            .record(ended.expect("a current end has evidence"), evidence.rows);
        if self.keeping.is_none() {
            let items = self.pending.items().saturating_add(self.kept.len());
            let _ = work::pass(items, self, |shown| {
                let pass = shown.take_keep().expect("the new evidence is pending");
                shown.accept_kept(pass.run());
            });
        }
    }

    pub(crate) fn is_keeping(&self) -> bool {
        self.observing.is_some() || self.keeping.is_some() || !self.pending.is_empty()
    }

    pub(crate) fn take_keep(&mut self) -> Option<Keep> {
        if self.keeping.is_some() || self.pending.is_empty() {
            return None;
        }
        let generation = self.results.generation()?;
        self.keeping = Some(generation);
        Some(Keep {
            generation,
            kept: Arc::clone(&self.kept),
            pending: std::mem::take(&mut self.pending),
        })
    }

    pub(crate) fn accept_kept(&mut self, answer: Kept) -> bool {
        if self.keeping != Some(answer.generation)
            || self.results.generation() != Some(answer.generation)
        {
            let items = answer.kept.len();
            self.retired.push(answer, items);
            return false;
        }
        self.keeping = None;
        let old = std::mem::replace(&mut self.kept, answer.kept);
        let items = old.len();
        self.retired.push(old, items);
        true
    }

    pub(super) fn abandon_keeping(&mut self) {
        self.observing = None;
        self.keeping = None;
        let pending = std::mem::take(&mut self.pending);
        let items = pending.items();
        self.retired.push(pending, items);
    }

    pub(super) fn reset_kept(&mut self) {
        self.abandon_keeping();
        let old = std::mem::take(&mut self.kept);
        let items = old.len();
        self.retired.push(old, items);
    }

    /// Whether the list still shows the rows of the ended operation `id`
    /// blank, so it stays in the queue.
    #[cfg(test)]
    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        self.is_keeping() || self.kept.contains(&id)
    }

    /// Which rows the list's control unhighlights after its accepted filter.
    pub(crate) fn deselect(&self) -> Deselect {
        match &self.selection {
            Selection::Reading { .. } => Deselect::Taken,
            _ => Deselect::Nothing,
        }
    }
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
    /// every selected size counts them twice. An unknown folder size, which
    /// every folder has for a user who does not see every file, is counted
    /// apart and never hides the known sum, as in a folder panel.
    #[test]
    fn a_file_below_a_selected_folder_counts_once() {
        let files = |folder_size| {
            vec![
                item(r"C:\a", folder_size, true),
                item(r"C:\a\b.txt", Some(4), false),
                item(r"C:\c.txt", Some(1), false),
            ]
        };
        let size = |known, unknown| SelectedSize { known, unknown };
        assert_eq!(chosen_bytes(&files(Some(10))), size(Some(11), 0));
        assert_eq!(chosen_bytes(&files(None)), size(Some(1), 1));
        assert_eq!(chosen_bytes(&files(None)[..2]), size(None, 1));
    }
}
