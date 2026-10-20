//! The delete's walk: workers that list folders and delete their entries
//! while the walk goes on elsewhere, and delete each folder when its last
//! entry is gone.
//!
//! The streaming walk in items of up to 256 files of one folder is
//! AngelCOPY's (`Delete.cpp`): memory holds only the folders in flight. The
//! count that deletes a folder when its last entry goes is in
//! [`crate::tree`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gozler_core::layout::FolderEntry;
use gozler_winfs::{Access, Entry, Reparse, WinError, open_entry};

use super::{Disposition, remove};
use crate::io_threads::IoThreads;
use crate::listing::{FILES_PER_ITEM, LISTING_WORDS, is_folder, join, list};
use crate::operation::{Listed, Pause, Progress};
use crate::pool::{Lane, Pool};
use crate::receive::{Selected, failure};
use crate::win32::is_gone;

type Folder = crate::tree::Folder<()>;

/// Deletes `entries` and everything below them on `workers` threads, and
/// returns each entry it could not delete, in the order the failures
/// happened. Counts each item deleted in `progress`. Stops when `cancel` is
/// set, leaving what it has not deleted, and waits between entries while
/// `pause` holds it.
pub(crate) fn delete(
    entries: Vec<Selected>,
    workers: usize,
    cancel: &AtomicBool,
    pause: &Pause,
    progress: &Progress,
    io: &IoThreads,
) -> Vec<Listed> {
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
    let stopped = || cancel.load(Ordering::Relaxed);
    let walk = Walk {
        cancel,
        pause,
        progress,
        failures: Mutex::default(),
        pool: Pool::new(items, &stopped),
    };
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| {
                let _entered = io.enter();
                walk.work();
            });
        }
    });
    walk.failures.into_inner().expect("no worker panics")
}

enum Work {
    Selected(Selected, Arc<Disposition>),
    List(Arc<Folder>),
    Files(Arc<Folder>, Vec<FolderEntry>),
}

struct Walk<'a> {
    cancel: &'a AtomicBool,
    pause: &'a Pause,
    progress: &'a Progress,
    failures: Mutex<Vec<Listed>>,
    pool: Pool<'a, Work>,
}

impl Walk<'_> {
    fn work(&self) {
        let mut page = vec![0u64; LISTING_WORDS];
        while let Some(work) = self.pool.next(Lane::Workers) {
            self.pause.wait(self.cancel);
            match work {
                Work::Selected(selected, disposition) => self.selected(selected, disposition),
                Work::List(folder) => self.list(folder, &mut page),
                Work::Files(folder, names) => self.files(folder, names),
            }
            self.pool.finish();
        }
    }

    fn push(&self, work: Work) {
        self.pool.push(work, Lane::Workers);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Records a failure; the entry stays, and so does `parent`.
    fn fail(&self, path: &[u16], error: u32, parent: Option<&Folder>) {
        if let Some(parent) = parent {
            parent.keep();
        }
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
            self.delete_entry(path, &disposition, None, || entry.reopen(access));
            return;
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
        let (entry, view) = match folder {
            Ok(folder) => folder,
            Err(error) => return self.lost(path, &error, None),
        };
        match Folder::new(
            entry,
            view,
            tag.attributes,
            path.clone(),
            None,
            disposition,
            (),
        ) {
            Ok(folder) => self.push(Work::List(folder)),
            Err(error) => self.fail(path, error, None),
        }
    }

    /// Lists `folder`, queues its files in items and its subfolders as their
    /// own items, then gives up the listing's count.
    fn list(&self, folder: Arc<Folder>, page: &mut [u64]) {
        let mut names = Vec::new();
        let listed = list(
            folder.children(),
            page,
            || self.cancelled(),
            |listed| {
                let listed = match listed {
                    Ok(listed) => listed,
                    Err(error) => return self.fail(&folder.path, error, Some(&folder)),
                };
                if is_folder(listed.attributes, listed.reparse_tag.unwrap_or(0)) {
                    self.subfolder(&folder, listed);
                } else {
                    names.push(listed);
                    if names.len() == FILES_PER_ITEM {
                        self.queue_files(&folder, std::mem::take(&mut names));
                    }
                }
            },
        );
        if let Err(error) = listed {
            self.fail(&folder.path, error, Some(&folder));
        }
        if !names.is_empty() {
            self.queue_files(&folder, names);
        }
        self.release(folder);
    }

    fn queue_files(&self, folder: &Arc<Folder>, names: Vec<FolderEntry>) {
        folder.hold();
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
            self.delete_entry(&path, &parent.disposition, Some(parent), || {
                entry.reopen(access)
            });
            return;
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
        let disposition = Arc::clone(&parent.disposition);
        let above = Some(Arc::clone(parent));
        match Folder::new(
            entry,
            view,
            tag.attributes,
            path.clone(),
            above,
            disposition,
            (),
        ) {
            Ok(folder) => self.push(Work::List(folder)),
            Err(error) => self.fail(&path, error, Some(parent)),
        }
    }

    fn files(&self, folder: Arc<Folder>, names: Vec<FolderEntry>) {
        for listed in names {
            self.pause.wait(self.cancel);
            if self.cancelled() {
                break;
            }
            let path = join(&folder.path, &listed.name);
            self.progress.start_file(&path, None, 0);
            let access = Access::delete(listed.attributes);
            let open = || (folder.children()).open_child(&listed.name, access, Reparse::Open);
            match remove(self.cancel, &folder.disposition, open) {
                Ok(()) => self.progress.add_items(1),
                Err(error) => self.fail(&path, error, Some(&folder)),
            }
        }
        self.release(folder);
    }

    /// Deletes one file or link, or keeps it with its failure. Returns
    /// whether it went.
    fn delete_entry(
        &self,
        path: &[u16],
        disposition: &Disposition,
        parent: Option<&Folder>,
        open: impl Fn() -> Result<Entry, WinError>,
    ) -> bool {
        self.progress.start_file(path, None, 0);
        match remove(self.cancel, disposition, open) {
            Ok(()) => {
                self.progress.add_items(1);
                true
            }
            Err(error) => {
                self.fail(path, error, parent);
                false
            }
        }
    }

    /// An entry that failed to open: gone counts as deleted, anything else
    /// keeps it and its folders.
    fn lost(&self, path: &[u16], error: &WinError, parent: Option<&Folder>) {
        if is_gone(error.code) {
            self.progress.add_items(1);
        } else {
            self.fail(path, error.code, parent);
        }
    }

    /// Gives up one count of `folder`, and deletes each folder that it
    /// empties.
    fn release(&self, folder: Arc<Folder>) {
        Folder::release(
            folder,
            || self.cancelled(),
            |folder| {
                let reopen = || folder.entry.reopen(Access::delete(folder.attributes));
                self.delete_entry(&folder.path, &folder.disposition, None, reopen)
            },
        );
    }
}
