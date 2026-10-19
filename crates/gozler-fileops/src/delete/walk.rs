//! The delete's walk: workers that list folders and delete their entries
//! while the walk goes on elsewhere, and delete each folder when its last
//! entry is gone.
//!
//! This combines two sources. The streaming walk in items of up to 256
//! files of one folder is AngelCOPY's (`Delete.cpp`): memory holds only the
//! folders in flight. The count that deletes a folder when its last child
//! goes is rmbrr's (`broker.rs`). rmbrr's count was complete before its
//! first delete, because rmbrr walks first; with a streaming walk a count of
//! the subfolders alone would reach zero while other workers still delete
//! the folder's files, so each folder counts everything still open below it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use gozler_core::layout::{FolderEntry, walk_full_listing_buffer};
use gozler_winfs::{Access, Entry, Reparse, WinError, open_entry};

use super::Disposition;
use crate::operation::{Failure, Progress};
use crate::receive::{Selected, failure};
use crate::retry::retry;
use crate::win32::{
    ERROR_INVALID_DATA, ERROR_REPARSE_POINT_ENCOUNTERED, FILE_ATTRIBUTE_DIRECTORY, is_gone,
    is_name_surrogate,
};

/// Files of one folder per work item, so one large folder uses several
/// workers (D2).
const FILES_PER_ITEM: usize = 256;

const BACKSLASH: u16 = b'\\' as u16;

/// One listing page. 64 KiB holds several hundred entries.
const LISTING_WORDS: usize = 64 * 1024 / 8;

/// Deletes `entries` and everything below them on `workers` threads, and
/// returns each entry it could not delete, in the order the failures
/// happened. Counts each item deleted in `progress`. Stops when `cancel` is
/// set, leaving what it has not deleted.
pub(crate) fn delete(
    entries: Vec<Selected>,
    workers: usize,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Vec<Failure> {
    let mut volumes: HashMap<Vec<u16>, Arc<Disposition>> = HashMap::new();
    let items = entries
        .into_iter()
        .map(|selected| {
            let disposition = volumes
                .entry(selected.identity.volume().to_vec())
                .or_default()
                .clone();
            Work::Selected(selected, disposition)
        })
        .collect();
    let walk = Walk {
        cancel,
        progress,
        failures: Mutex::default(),
        pending: Mutex::new(Pending { items, busy: 0 }),
        ready: Condvar::new(),
    };
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| walk.work());
        }
    });
    walk.failures.into_inner().expect("no worker panics")
}

enum Work {
    Selected(Selected, Arc<Disposition>),
    List(Arc<Folder>),
    Files(Arc<Folder>, Vec<FolderEntry>),
}

/// A folder being deleted. Every open below it is relative to its handle,
/// so a swap of a folder above it for a link cannot redirect the walk.
struct Folder {
    /// The folder itself, never a link's target.
    entry: Entry,
    /// The filter's view of a folder whose reparse tag is not a name
    /// surrogate, such as a OneDrive folder: listing it through `entry`
    /// would bypass the cloud filter and could see it empty.
    view: Option<Entry>,
    /// Its own attributes, which decide the access its delete asks for.
    attributes: u32,
    /// For the result.
    path: Vec<u16>,
    parent: Option<Arc<Folder>>,
    /// What is still open below it: its own listing until the listing ends,
    /// each item of its files until the item is done, and each subfolder
    /// until it is deleted or kept.
    open: AtomicUsize,
    /// A failed entry stays below it, so it is not tried.
    kept: AtomicBool,
    disposition: Arc<Disposition>,
}

impl Folder {
    fn children(&self) -> &Entry {
        self.view.as_ref().unwrap_or(&self.entry)
    }
}

struct Pending {
    items: Vec<Work>,
    /// Workers holding an item.
    busy: usize,
}

struct Walk<'a> {
    cancel: &'a AtomicBool,
    progress: &'a Progress,
    failures: Mutex<Vec<Failure>>,
    pending: Mutex<Pending>,
    ready: Condvar,
}

impl Walk<'_> {
    fn work(&self) {
        let mut page = vec![0u64; LISTING_WORDS];
        while let Some(work) = self.next() {
            match work {
                Work::Selected(selected, disposition) => self.selected(selected, disposition),
                Work::List(folder) => self.list(folder, &mut page),
                Work::Files(folder, names) => self.files(folder, names),
            }
            self.finish();
        }
    }

    /// The next item, the latest queued first, so the walk goes deep before
    /// it goes wide and fewer folders are in flight. `None` when the walk is
    /// done or cancelled.
    fn next(&self) -> Option<Work> {
        let mut pending = self.pending.lock().expect("no worker panics");
        loop {
            if self.cancelled() || (pending.items.is_empty() && pending.busy == 0) {
                self.ready.notify_all();
                return None;
            }
            if let Some(work) = pending.items.pop() {
                pending.busy += 1;
                return Some(work);
            }
            pending = self.ready.wait(pending).expect("no worker panics");
        }
    }

    fn push(&self, work: Work) {
        self.pending
            .lock()
            .expect("no worker panics")
            .items
            .push(work);
        self.ready.notify_one();
    }

    fn finish(&self) {
        let mut pending = self.pending.lock().expect("no worker panics");
        pending.busy -= 1;
        if pending.busy == 0 {
            self.ready.notify_all();
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn fail(&self, path: &[u16], error: u32) {
        if !self.cancelled() {
            let mut failures = self.failures.lock().expect("no worker panics");
            failures.push(failure(path, error));
        }
    }

    /// Reads what a selected entry is on its own handle, because a Search
    /// selection holds no reparse tag and a junction taken for a folder
    /// would be entered.
    fn selected(&self, selected: Selected, disposition: Arc<Disposition>) {
        let path = &selected.path;
        let opened = open_entry(&selected.verbatim, Access::Inspect, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
        let (tag, entry) = match opened {
            Ok(opened) => opened,
            Err(error) => return self.lost(path, &error, None),
        };
        if !is_folder(tag.attributes, tag.reparse_tag) {
            let access = Access::delete(tag.attributes);
            return self.delete_entry(path, &disposition, None, || entry.reopen(access));
        }
        let folder = entry.reopen(Access::List).and_then(|entry| {
            let view = if tag.reparse_tag == 0 {
                None
            } else {
                Some(open_entry(
                    &selected.verbatim,
                    Access::List,
                    Reparse::Follow,
                )?)
            };
            Ok((entry, view))
        });
        match folder {
            Ok((entry, view)) => {
                let path = path.clone();
                let folder = self.folder(entry, view, tag.attributes, path, None, disposition);
                if let Some(folder) = folder {
                    self.push(Work::List(folder));
                }
            }
            Err(error) => self.lost(path, &error, None),
        }
    }

    /// Lists `folder`, queues its files in items and its subfolders as their
    /// own items, then gives up the listing's count.
    fn list(&self, folder: Arc<Folder>, page: &mut [u64]) {
        let mut names = Vec::new();
        while !self.cancelled() {
            let entries = match folder.children().read_listing(page) {
                Ok(Some(bytes)) => walk_full_listing_buffer(bytes).map_err(|_| ERROR_INVALID_DATA),
                Ok(None) => break,
                Err(error) => Err(error.code),
            };
            let entries = match entries {
                Ok(entries) => entries,
                Err(error) => {
                    self.fail(&folder.path, error);
                    folder.kept.store(true, Ordering::Relaxed);
                    break;
                }
            };
            for listed in entries {
                // The relative open takes one component; an empty name
                // would open this folder itself.
                if listed.name.is_empty() || listed.name.contains(&BACKSLASH) {
                    self.fail(&folder.path, ERROR_INVALID_DATA);
                    folder.kept.store(true, Ordering::Relaxed);
                    continue;
                }
                if is_folder(listed.attributes, listed.reparse_tag.unwrap_or(0)) {
                    self.subfolder(&folder, listed);
                } else {
                    names.push(listed);
                    if names.len() == FILES_PER_ITEM {
                        self.queue_files(&folder, std::mem::take(&mut names));
                    }
                }
            }
        }
        if !names.is_empty() {
            self.queue_files(&folder, names);
        }
        self.release(folder);
    }

    fn queue_files(&self, folder: &Arc<Folder>, names: Vec<FolderEntry>) {
        folder.open.fetch_add(1, Ordering::Relaxed);
        self.push(Work::Files(Arc::clone(folder), names));
    }

    /// Opens a listed folder relative to its parent and reads what it is now,
    /// not at the listing: a folder swapped for a link since is deleted as a
    /// link.
    fn subfolder(&self, parent: &Arc<Folder>, listed: FolderEntry) {
        let path = join(&parent.path, &listed.name);
        let opened = parent
            .children()
            .open_child(&listed.name, Access::List, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
        let (tag, entry) = match opened {
            Ok(opened) => opened,
            Err(error) => return self.lost(&path, &error, Some(parent)),
        };
        if !is_folder(tag.attributes, tag.reparse_tag) {
            let access = Access::delete(tag.attributes);
            return self.delete_entry(&path, &parent.disposition, Some(parent), || {
                entry.reopen(access)
            });
        }
        let view = if tag.reparse_tag == 0 {
            Ok(None)
        } else {
            (parent.children())
                .open_child(&listed.name, Access::List, Reparse::Follow)
                .map(Some)
        };
        let view = match view {
            Ok(view) => view,
            Err(error) => return self.lost(&path, &error, Some(parent)),
        };
        parent.open.fetch_add(1, Ordering::Relaxed);
        let disposition = Arc::clone(&parent.disposition);
        let above = Some(Arc::clone(parent));
        match self.folder(entry, view, tag.attributes, path, above, disposition) {
            Some(folder) => self.push(Work::List(folder)),
            // The listing's own count keeps the parent above zero.
            None => {
                parent.open.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// The work item of a folder opened for listing. `None` when its
    /// filter's view is another entry than the folder, which happens only
    /// when the folder was swapped between the two opens; that is a failure.
    fn folder(
        &self,
        entry: Entry,
        view: Option<Entry>,
        attributes: u32,
        path: Vec<u16>,
        parent: Option<Arc<Folder>>,
        disposition: Arc<Disposition>,
    ) -> Option<Arc<Folder>> {
        if let Some(view) = &view {
            let same = match (entry.identity(), view.identity()) {
                (Ok(entry), Ok(view)) => Ok(entry == view),
                (Err(error), _) | (_, Err(error)) => Err(error.code),
            };
            if same != Ok(true) {
                self.fail(&path, same.err().unwrap_or(ERROR_REPARSE_POINT_ENCOUNTERED));
                if let Some(parent) = &parent {
                    parent.kept.store(true, Ordering::Relaxed);
                }
                return None;
            }
        }
        Some(Arc::new(Folder {
            entry,
            view,
            attributes,
            path,
            parent,
            open: AtomicUsize::new(1),
            kept: AtomicBool::new(false),
            disposition,
        }))
    }

    fn files(&self, folder: Arc<Folder>, names: Vec<FolderEntry>) {
        for listed in names {
            if self.cancelled() {
                break;
            }
            let path = || join(&folder.path, &listed.name);
            let access = Access::delete(listed.attributes);
            let open = || (folder.children()).open_child(&listed.name, access, Reparse::Open);
            match self.remove(&folder.disposition, open) {
                Ok(()) => self.progress.add_items(1),
                Err(error) => {
                    self.fail(&path(), error);
                    folder.kept.store(true, Ordering::Relaxed);
                }
            }
        }
        self.release(folder);
    }

    /// Deletes one file or link, or keeps it with its failure.
    fn delete_entry(
        &self,
        path: &[u16],
        disposition: &Disposition,
        parent: Option<&Arc<Folder>>,
        open: impl Fn() -> Result<Entry, WinError>,
    ) {
        match self.remove(disposition, open) {
            Ok(()) => self.progress.add_items(1),
            Err(error) => {
                self.fail(path, error);
                if let Some(parent) = parent {
                    parent.kept.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// An entry that failed to open: gone counts as deleted, anything else
    /// keeps it and its folders.
    fn lost(&self, path: &[u16], error: &WinError, parent: Option<&Arc<Folder>>) {
        if is_gone(error.code) {
            self.progress.add_items(1);
            return;
        }
        self.fail(path, error.code);
        if let Some(parent) = parent {
            parent.kept.store(true, Ordering::Relaxed);
        }
    }

    /// Opens and deletes one entry with the short retries. An entry already
    /// gone counts as deleted. The handle closes before this returns, so a
    /// name deleted with POSIX semantics has left its folder.
    fn remove(
        &self,
        disposition: &Disposition,
        open: impl Fn() -> Result<Entry, WinError>,
    ) -> Result<(), u32> {
        let removed = retry(self.cancel, || {
            let entry = open().map_err(|error| error.code)?;
            disposition.delete(
                || entry.delete_posix().map_err(|error| error.code),
                || entry.delete_classic().map_err(|error| error.code),
            )
        });
        match removed {
            Err(error) if is_gone(error) => Ok(()),
            removed => removed,
        }
    }

    /// Gives up one count of `folder`. The worker that gives up the last
    /// deletes it and gives up its parent's count, and so on up the tree. A
    /// folder that keeps a failed entry is not tried, and nor are its
    /// parents.
    fn release(&self, folder: Arc<Folder>) {
        let mut next = Some(folder);
        while let Some(folder) = next.take() {
            if folder.open.fetch_sub(1, Ordering::AcqRel) != 1 {
                return;
            }
            let parent = folder.parent.clone();
            let kept = folder.kept.load(Ordering::Acquire) || self.cancelled();
            if !kept {
                let reopen = || folder.entry.reopen(Access::delete(folder.attributes));
                self.delete_entry(&folder.path, &folder.disposition, parent.as_ref(), reopen);
            } else if let Some(parent) = &parent {
                parent.kept.store(true, Ordering::Relaxed);
            }
            // Its own handles close before its parent is tried, because with
            // the classic delete its name stays until they do.
            drop(folder);
            next = parent;
        }
    }
}

/// Whether the walk enters an entry: a folder, unless its reparse tag names
/// another entry. A link is deleted, never entered, because entering it
/// would delete files outside the selection.
fn is_folder(attributes: u32, reparse_tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_DIRECTORY != 0 && !is_name_surrogate(reparse_tag)
}

fn join(folder: &[u16], name: &[u16]) -> Vec<u16> {
    let mut path = folder.to_vec();
    if path.last() != Some(&BACKSLASH) {
        path.push(BACKSLASH);
    }
    path.extend_from_slice(name);
    path
}
