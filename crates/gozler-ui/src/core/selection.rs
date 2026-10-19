//! The selection: a set of files, built at one moment (design rule 10). The
//! list owns which positions are selected; the model builds the files from
//! them once per user action (docs/plans/ui-design.md: The selection).

use super::fetch::Generation;
use gozler_core::proto::ResultItem;
use std::ops::Range;

/// Open and Open file location stop here, because File Explorer asks before
/// it opens more items, and many windows opened by accident are worse
/// (spec: Commands).
pub(crate) const OPEN_LIMIT: usize = 15;

#[derive(Debug, PartialEq)]
pub(crate) enum Selection {
    None,
    /// Some selected rows are not cached; their one request is due, or in
    /// flight as `asked`.
    Reading {
        positions: Vec<Range<u64>>,
        asked: Option<Generation>,
    },
    /// In position order, each at most once.
    Files(Vec<ResultItem>),
}

impl Selection {
    /// The selected files, once they are read; `None` while nothing is
    /// selected or the selection is read.
    pub(crate) fn files(&self) -> Option<&[ResultItem]> {
        match self {
            Selection::Files(files) => Some(files),
            Selection::None | Selection::Reading { .. } => None,
        }
    }
}

/// How many entries are selected, and the size of the selected files whose
/// size is known; a folder's size counts only through the files below it, so
/// it is left out (spec: Status bar).
pub(crate) fn sums(files: &[ResultItem]) -> (u64, u64) {
    let bytes = files
        .iter()
        .filter(|file| !file.is_dir)
        .filter_map(|file| file.size)
        .sum();
    (files.len() as u64, bytes)
}

/// The full path of `file` as UTF-16, exactly as Windows names it, also
/// when the name holds an unpaired surrogate that its UTF-8 `path` lost.
pub(crate) fn wide_path(file: &ResultItem) -> Vec<u16> {
    match &file.path_u16 {
        Some(units) => units.clone(),
        None => file.path.encode_utf16().collect(),
    }
}
