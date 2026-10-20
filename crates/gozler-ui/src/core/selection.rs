//! The selection: a set of files, built at one moment (DESIGN.md:
//! window rule 10). The list owns which positions are selected; the model
//! builds the files from them once per user action (DESIGN.md: The
//! selection).

use super::fetch::Generation;
use gozler_core::proto::{QueryTotals, ResultItem};
use std::ops::Range;

/// Open and Open file location stop here, because File Explorer asks before
/// it opens more items, and many windows opened by accident are worse
/// (DESIGN.md: Commands).
pub(crate) const OPEN_LIMIT: usize = 15;

/// A drag holds at most this many entries, because the window names each
/// one and builds the shell's data object on its thread before the drag
/// starts, and Edit > Copy has no limit (DESIGN.md: Dragging).
pub(crate) const DRAG_LIMIT: usize = 1_000;

#[derive(Debug, PartialEq)]
pub(crate) enum Selection {
    None,
    /// Some selected rows are not cached; their one request is due, or in
    /// flight as `asked`.
    Reading {
        positions: Vec<Range<u64>>,
        asked: Option<Generation>,
    },
    /// In position order, each at most once; never one folder alone, which
    /// is [`Selection::Folder`].
    Files(Vec<ResultItem>),
    /// One folder alone, whose contents the inspector shows (DESIGN.md:
    /// Inspector pane).
    Folder {
        folder: ResultItem,
        contents: FolderContents,
    },
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
            Selection::None | Selection::Reading { .. } => None,
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
            | Selection::Folder {
                contents: FolderContents::Counted(_),
                ..
            } => None,
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
    match &file.path_u16 {
        Some(units) => units.clone(),
        None => file.path.encode_utf16().collect(),
    }
}
