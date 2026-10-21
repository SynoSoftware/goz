//! The system image list's index for each extension, asked of Windows once
//! per extension (DESIGN.md: Results list), and of the panels' `..`.

use crate::win::gdi::ImageList;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// The extension, lowercased, or `None` for a folder; a file without an
/// extension is the empty extension.
type Kind = Option<String>;

#[derive(Default)]
pub(super) struct Icons {
    kinds: RefCell<HashMap<Kind, i32>>,
    up: Cell<Option<i32>>,
}

impl Icons {
    /// The icon of an item named `name`. A new extension asks Windows with
    /// no borrow held, because the shell call can dispatch messages (DESIGN.md:
    /// window rule 1).
    pub(super) fn index(&self, name: &str, is_folder: bool) -> i32 {
        let kind: Kind = (!is_folder).then(|| {
            gozler_core::query::extension(name.as_bytes(), false)
                .map(|ext| String::from_utf8_lossy(ext).to_lowercase())
                .unwrap_or_default()
        });
        if let Some(&index) = self.kinds.borrow().get(&kind) {
            return index;
        }
        let sample = match &kind {
            None => "folder".to_owned(),
            Some(ext) if ext.is_empty() => "file".to_owned(),
            Some(ext) => format!("file.{ext}"),
        };
        let index = ImageList::system_index(&sample, is_folder);
        self.kinds.borrow_mut().insert(kind, index);
        index
    }

    /// The icon of a panel's `..` row: Windows' open folder, which tells
    /// it from the folders below it; a folder's when the shell does not
    /// say.
    pub(super) fn up(&self) -> i32 {
        if let Some(index) = self.up.get() {
            return index;
        }
        let index = ImageList::open_folder_index().unwrap_or_else(|| self.index("", true));
        self.up.set(Some(index));
        index
    }
}
