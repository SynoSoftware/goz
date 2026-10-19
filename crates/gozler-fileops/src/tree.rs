//! A folder in flight in a walk, which counts what is still open below it,
//! so the delete, and a move across volumes, delete it as soon as its last
//! entry is gone (D3).
//!
//! The count that deletes a folder when its last child goes is rmbrr's
//! (`broker.rs`). rmbrr's count was complete before its first delete,
//! because rmbrr walks first; with a streaming walk a count of the
//! subfolders alone would reach zero while other workers still handle the
//! folder's files, so each folder counts everything still open below it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use gozler_winfs::Entry;

use crate::delete::Disposition;
use crate::win32::ERROR_REPARSE_POINT_ENCOUNTERED;

/// Every open below it is relative to its handle, so a swap of a folder
/// above it for a link cannot redirect the walk.
pub(crate) struct Folder<T> {
    /// The folder itself, never a link's target.
    pub(crate) entry: Entry,
    /// The filter's view of a folder whose reparse tag is not a name
    /// surrogate, such as a OneDrive folder: listing it through `entry`
    /// would bypass the cloud filter and could see it empty.
    view: Option<Entry>,
    /// Its own attributes, which decide the access its delete asks for.
    pub(crate) attributes: u32,
    /// As the window gave it, for the result.
    pub(crate) path: Vec<u16>,
    parent: Option<Arc<Folder<T>>>,
    /// What is still open below it: its own listing until the listing ends,
    /// each item of its files until the item is done, and each subfolder
    /// until it is deleted or kept.
    open: AtomicUsize,
    /// An entry stays below it, so it is not tried.
    kept: AtomicBool,
    /// How its volume deletes.
    pub(crate) disposition: Arc<Disposition>,
    /// What the walk adds.
    pub(crate) walk: T,
}

impl<T> Folder<T> {
    /// A folder opened for listing, holding its listing's count. Fails when
    /// its filter's view is another entry than the folder ([`same_entry`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        entry: Entry,
        view: Option<Entry>,
        attributes: u32,
        path: Vec<u16>,
        parent: Option<Arc<Folder<T>>>,
        disposition: Arc<Disposition>,
        walk: T,
    ) -> Result<Arc<Self>, u32> {
        if let Some(view) = &view {
            same_entry(&entry, view)?;
        }
        if let Some(parent) = &parent {
            parent.hold();
        }
        Ok(Arc::new(Folder {
            entry,
            view,
            attributes,
            path,
            parent,
            open: AtomicUsize::new(1),
            kept: AtomicBool::new(false),
            disposition,
            walk,
        }))
    }

    /// The handle to list it and open its children through.
    pub(crate) fn children(&self) -> &Entry {
        self.view.as_ref().unwrap_or(&self.entry)
    }

    /// Counts one more item open below it.
    pub(crate) fn hold(&self) {
        self.open.fetch_add(1, Ordering::Relaxed);
    }

    /// An entry below it stays, so it stays too.
    pub(crate) fn keep(&self) {
        self.kept.store(true, Ordering::Relaxed);
    }

    /// Gives up one count of `folder`. The thread that gives up the last
    /// calls `empty` on it, unless an entry stays below it or `stopped` is
    /// set, and gives up its parent's count, and so on up the tree. `empty`
    /// returns whether the folder went; a folder that stays keeps its
    /// parents.
    pub(crate) fn release(
        folder: Arc<Self>,
        stopped: impl Fn() -> bool,
        mut empty: impl FnMut(&Self) -> bool,
    ) {
        let mut next = Some(folder);
        while let Some(folder) = next.take() {
            if folder.open.fetch_sub(1, Ordering::AcqRel) != 1 {
                return;
            }
            let parent = folder.parent.clone();
            let gone = !folder.kept.load(Ordering::Acquire) && !stopped() && empty(&folder);
            if !gone && let Some(parent) = &parent {
                parent.keep();
            }
            // Its own handles close before its parent is tried, because with
            // the classic delete its name stays until they do.
            drop(folder);
            next = parent;
        }
    }
}

/// Checks that a folder's filter view is the folder itself: both handles
/// have the same volume serial number and file ID, because a file ID is
/// unique only on its own volume. They differ only when the folder was
/// swapped for another entry between the two opens.
pub(crate) fn same_entry(entry: &Entry, view: &Entry) -> Result<(), u32> {
    match (entry.identity(), view.identity()) {
        (Ok(entry), Ok(view)) if entry == view => Ok(()),
        (Ok(_), Ok(_)) => Err(ERROR_REPARSE_POINT_ENCOUNTERED),
        (Err(error), _) | (_, Err(error)) => Err(error.code),
    }
}
