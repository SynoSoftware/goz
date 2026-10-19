//! Copy with the Windows engine, and a move across volumes, which is a copy
//! that deletes each source when its copy has its final name.
//!
//! The walk streams, as the delete's does and as AngelCOPY's `WalkStream`
//! does: the worker that lists a source folder creates its destination
//! folder, reads that folder's names once, decides each file's final name
//! under that folder's lock, and hands its files on, up to 256 under the
//! large-file limit as one item and each large file to the one-at-a-time
//! lane. Copying starts while the walk still runs. Each file is written
//! under its temporary name (D10) and renamed when complete, so a file at
//! its final name is always complete.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gozler_core::layout::FolderEntry;
use gozler_winfs::{
    Access, CopyOptions, CopySource, Entry, EntryStat, FileIdentity, Reparse, copy_file,
    create_folder, open_entry, path_attributes,
};

use crate::conflicts::{Decision, Rule, Wanted, decide};
use crate::delete::{Disposition, remove};
use crate::io_threads::IoThreads;
use crate::listing::{FILES_PER_ITEM, LISTING_WORDS, is_folder, join, kind, list, read_names};
use crate::names::{Holder, Kind, Names, partial_name};
use crate::operation::{Listed, Progress, Reason};
use crate::place::{Placed, Placing, Target, place};
use crate::pool::{Lane, Pool};
use crate::receive::{Selected, listed};
use crate::retry::{retry, retry_also};
use crate::volumes::CopyPlan;
use crate::win32::{
    ERROR_ALREADY_EXISTS, ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ERROR_OPERATION_ABORTED,
    ERROR_PATH_NOT_FOUND, ERROR_REQUEST_ABORTED, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_SPARSE_FILE, IO_REPARSE_TAG_SYMLINK, is_gone, is_name_surrogate,
};

/// How one copy runs.
pub(crate) struct Copying<'a> {
    pub(crate) plan: CopyPlan,
    pub(crate) rule: Rule,
    /// A move across volumes: each source is deleted when its copy has its
    /// final name, and each source folder when it is empty.
    pub(crate) moving: bool,
    /// Sparse files stay sparse: Windows build 22621 or later, and a
    /// destination that supports sparse files.
    pub(crate) sparse: bool,
    pub(crate) cancel: &'a AtomicBool,
    pub(crate) progress: &'a Progress,
    pub(crate) io: &'a IoThreads,
}

pub(crate) struct Copied {
    /// In the order they happened.
    pub(crate) listed: Vec<Listed>,
    /// The destination disk filled, which stopped the copy.
    pub(crate) disk_full: bool,
}

/// A selected entry, and whether it is copied into its own folder.
pub(crate) struct Source {
    pub(crate) selected: Selected,
    pub(crate) own_folder: bool,
}

/// Copies `sources` into `destination`, whose names it holds.
pub(crate) fn copy(
    sources: Vec<Source>,
    destination: Arc<Target>,
    copying: &Copying<'_>,
) -> Copied {
    let mut volumes: HashMap<Vec<u16>, Arc<Disposition>> = HashMap::new();
    let items = sources
        .into_iter()
        .map(|source| {
            let disposition = volumes
                .entry(source.selected.identity.volume().to_vec())
                .or_default()
                .clone();
            Work::Selected(source, Arc::clone(&destination), disposition)
        })
        .collect();
    let full = AtomicBool::new(false);
    let stopped = || copying.cancel.load(Ordering::Relaxed) || full.load(Ordering::Relaxed);
    let walk = Walk {
        copying,
        full: &full,
        listed: Mutex::default(),
        pool: Pool::new(items, &stopped),
        destination: Disposition::default(),
    };
    std::thread::scope(|scope| {
        for _ in 0..copying.plan.workers.max(1) {
            scope.spawn(|| walk.work(Lane::Workers));
        }
        if copying.plan.lane_beside {
            scope.spawn(|| walk.work(Lane::Large));
        }
    });
    Copied {
        listed: walk.listed.into_inner().expect("no worker panics"),
        disk_full: full.load(Ordering::Relaxed),
    }
}

/// A source folder in flight, with the destination folder it copies into.
struct Walking {
    verbatim: Vec<u16>,
    target: Arc<Target>,
}

type Folder = crate::tree::Folder<Walking>;

enum Work {
    Selected(Source, Arc<Target>, Arc<Disposition>),
    List(Arc<Folder>),
    Files(Arc<Folder>, Vec<File>),
    /// A large file, with its source folder; `None` for a selected file.
    Large(Option<Arc<Folder>>, File),
}

/// A file or a link whose final name is decided.
struct File {
    /// Its name in its source folder.
    name: Vec<u16>,
    /// As the window gave it, for the result.
    path: Vec<u16>,
    /// With the `\\?\` or `\\?\UNC\` prefix.
    from: Vec<u16>,
    target: Arc<Target>,
    final_name: Vec<u16>,
    replace: bool,
    size: u64,
    attributes: u32,
    reparse_tag: u32,
    /// How its source volume deletes, for a move.
    disposition: Arc<Disposition>,
}

struct Walk<'a> {
    copying: &'a Copying<'a>,
    full: &'a AtomicBool,
    listed: Mutex<Vec<Listed>>,
    pool: Pool<'a, Work>,
    /// How the destination volume deletes, for the temporary files.
    destination: Disposition,
}

impl Walk<'_> {
    fn work(&self, lane: Lane) {
        let _entered = self.copying.io.enter();
        let mut page = vec![0u64; LISTING_WORDS];
        while let Some(work) = self.pool.next(lane) {
            match work {
                Work::Selected(source, target, disposition) => {
                    self.selected(source, &target, disposition, &mut page);
                }
                Work::List(folder) => self.list(folder, &mut page),
                Work::Files(folder, files) => {
                    for file in files {
                        if self.stopped() {
                            break;
                        }
                        self.copy_file(&file, Some(&folder));
                    }
                    self.release(folder);
                }
                Work::Large(folder, file) => {
                    self.copy_file(&file, folder.as_ref());
                    if let Some(folder) = folder {
                        self.release(folder);
                    }
                }
            }
            self.pool.finish();
        }
    }

    fn cancelled(&self) -> bool {
        self.copying.cancel.load(Ordering::Relaxed)
    }

    fn stopped(&self) -> bool {
        self.cancelled() || self.full.load(Ordering::Relaxed)
    }

    /// Counts an entry that is not copied and lists it with `reason`. It
    /// keeps its source folder, because a move must not delete it.
    fn list_entry(&self, path: &[u16], reason: Reason, folder: Option<&Folder>) {
        if let Some(folder) = folder {
            folder.keep();
        }
        if matches!(reason, Reason::Failed(_)) && self.stopped() {
            return;
        }
        self.copying.progress.add_items(1);
        let mut entries = self.listed.lock().expect("no worker panics");
        entries.push(listed(path, reason));
    }

    fn fail(&self, path: &[u16], error: u32, folder: Option<&Folder>) {
        self.list_entry(path, Reason::Failed(error), folder);
    }

    /// Reads what a selected entry is on its own handle, as the delete does,
    /// then copies it into the destination.
    fn selected(
        &self,
        source: Source,
        target: &Arc<Target>,
        disposition: Arc<Disposition>,
        page: &mut [u64],
    ) {
        let Source {
            selected,
            own_folder,
        } = source;
        let opened = open_entry(&selected.verbatim, Access::Inspect, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
        let (tag, entry) = match opened {
            Ok(opened) => opened,
            Err(error) => return self.fail(&selected.path, error.code, None),
        };
        let name = selected.identity.name().to_vec();
        if !is_folder(tag.attributes, tag.reparse_tag) {
            let stat = match entry.stat() {
                Ok(stat) => stat,
                Err(error) => return self.fail(&selected.path, error.code, None),
            };
            drop(entry);
            let wanted = Wanted {
                kind: kind(tag.attributes, tag.reparse_tag),
                size: stat.size,
                last_write: stat.last_write,
                own_folder,
            };
            let entry = FolderEntry {
                name,
                attributes: tag.attributes,
                size: stat.size,
                last_write: stat.last_write,
                reparse_tag: Some(tag.reparse_tag),
            };
            let from = &selected.verbatim;
            let decided = self.decide_file(
                target,
                entry,
                wanted,
                &selected.path,
                from,
                &disposition,
                None,
            );
            let Some(file) = decided else {
                return;
            };
            if self.is_large(&file) && self.copying.plan.lane_beside {
                self.pool.push(Work::Large(None, file), Lane::Large);
            } else {
                self.copy_file(&file, None);
            }
            return;
        }
        let wanted = Wanted {
            kind: Kind::Folder,
            size: 0,
            last_write: 0,
            own_folder,
        };
        let path = &selected.path;
        let into = self.folder_target(target, &name, &selected.verbatim, wanted, path, None, page);
        let Some(into) = into else {
            return;
        };
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
            Err(error) => return self.fail(path, error.code, None),
        };
        let walking = Walking {
            verbatim: selected.verbatim.clone(),
            target: into,
        };
        match Folder::new(
            entry,
            view,
            tag.attributes,
            path.clone(),
            None,
            disposition,
            walking,
        ) {
            Ok(folder) => self.pool.push(Work::List(folder), Lane::Workers),
            Err(error) => self.fail(path, error, None),
        }
    }

    /// Lists a source folder, creates or merges each subfolder at the
    /// destination and queues it, and hands its files on.
    fn list(&self, folder: Arc<Folder>, page: &mut [u64]) {
        let mut files = Vec::new();
        let mut subfolders = Vec::new();
        let listed_all = list(
            folder.children(),
            page,
            || self.stopped(),
            |entry| match entry {
                Ok(entry) if is_folder(entry.attributes, entry.reparse_tag.unwrap_or(0)) => {
                    subfolders.push(entry);
                }
                Ok(entry) => files.push(entry),
                Err(error) => self.fail(&folder.path, error, Some(&folder)),
            },
        );
        if let Err(error) = listed_all {
            self.fail(&folder.path, error, Some(&folder));
        }
        // A merged subfolder's names are read into the same page, so the
        // subfolders wait until this listing is read.
        for entry in subfolders {
            if self.stopped() {
                break;
            }
            self.subfolder(&folder, entry, page);
        }
        let mut small = Vec::new();
        for entry in files {
            if self.stopped() {
                break;
            }
            let path = join(&folder.path, &entry.name);
            let from = join(&folder.walk.verbatim, &entry.name);
            let wanted = Wanted {
                kind: kind(entry.attributes, entry.reparse_tag.unwrap_or(0)),
                size: entry.size,
                last_write: entry.last_write,
                own_folder: false,
            };
            let target = &folder.walk.target;
            let disposition = &folder.disposition;
            let decided = self.decide_file(
                target,
                entry,
                wanted,
                &path,
                &from,
                disposition,
                Some(&folder),
            );
            let Some(file) = decided else {
                continue;
            };
            if self.is_large(&file) {
                folder.hold();
                let lane = if self.copying.plan.lane_beside {
                    Lane::Large
                } else {
                    Lane::Workers
                };
                self.pool
                    .push(Work::Large(Some(Arc::clone(&folder)), file), lane);
            } else {
                small.push(file);
                if small.len() == FILES_PER_ITEM {
                    self.queue_files(&folder, std::mem::take(&mut small));
                }
            }
        }
        if !small.is_empty() {
            self.queue_files(&folder, small);
        }
        self.release(folder);
    }

    fn queue_files(&self, folder: &Arc<Folder>, files: Vec<File>) {
        folder.hold();
        self.pool
            .push(Work::Files(Arc::clone(folder), files), Lane::Workers);
    }

    /// Opens a listed subfolder relative to its parent, reads what it is now,
    /// and queues it with its destination folder. A folder swapped for a link
    /// since the listing is copied as a link.
    fn subfolder(&self, parent: &Arc<Folder>, listed: FolderEntry, page: &mut [u64]) {
        let path = join(&parent.path, &listed.name);
        let from = join(&parent.walk.verbatim, &listed.name);
        let opened = parent
            .children()
            .open_child(&listed.name, Access::List, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
        let (tag, entry) = match opened {
            Ok(opened) => opened,
            Err(error) if is_gone(error.code) => return self.copying.progress.add_items(1),
            Err(error) => return self.fail(&path, error.code, Some(parent)),
        };
        let target = &parent.walk.target;
        if !is_folder(tag.attributes, tag.reparse_tag) {
            let entry = FolderEntry {
                reparse_tag: Some(tag.reparse_tag),
                attributes: tag.attributes,
                ..listed
            };
            let wanted = Wanted {
                kind: kind(tag.attributes, tag.reparse_tag),
                size: entry.size,
                last_write: entry.last_write,
                own_folder: false,
            };
            let disposition = &parent.disposition;
            let decided = self.decide_file(
                target,
                entry,
                wanted,
                &path,
                &from,
                disposition,
                Some(parent),
            );
            if let Some(file) = decided {
                self.copy_file(&file, Some(parent));
            }
            return;
        }
        let wanted = Wanted {
            kind: Kind::Folder,
            size: 0,
            last_write: 0,
            own_folder: false,
        };
        let into = self.folder_target(
            target,
            &listed.name,
            &from,
            wanted,
            &path,
            Some(parent),
            page,
        );
        let Some(into) = into else {
            return;
        };
        let view = if tag.reparse_tag == 0 {
            Ok(None)
        } else {
            (parent.children())
                .open_child(&listed.name, Access::List, Reparse::Follow)
                .map(Some)
        };
        let view = match view {
            Ok(view) => view,
            Err(error) => return self.fail(&path, error.code, Some(parent)),
        };
        let walking = Walking {
            verbatim: from,
            target: into,
        };
        let above = Some(Arc::clone(parent));
        let disposition = Arc::clone(&parent.disposition);
        match Folder::new(
            entry,
            view,
            tag.attributes,
            path.clone(),
            above,
            disposition,
            walking,
        ) {
            Ok(folder) => self.pool.push(Work::List(folder), Lane::Workers),
            Err(error) => self.fail(&path, error, Some(parent)),
        }
    }

    /// Decides a file's final name in `target`. `None` when it is not
    /// copied: done, skipped or refused, each counted and listed here.
    #[allow(clippy::too_many_arguments)]
    fn decide_file(
        &self,
        target: &Arc<Target>,
        entry: FolderEntry,
        wanted: Wanted,
        path: &[u16],
        from: &[u16],
        disposition: &Arc<Disposition>,
        folder: Option<&Folder>,
    ) -> Option<File> {
        let decision = {
            let mut names = target.names.lock().expect("no worker panics");
            decide(&mut names, &entry.name, wanted, self.copying.rule)
        };
        match decision {
            Decision::Write { name, replace } => Some(File {
                path: path.to_vec(),
                from: from.to_vec(),
                target: Arc::clone(target),
                final_name: name,
                replace,
                size: entry.size,
                attributes: entry.attributes,
                reparse_tag: entry.reparse_tag.unwrap_or(0),
                disposition: Arc::clone(disposition),
                name: entry.name,
            }),
            Decision::Done if self.copying.moving => {
                // Equal size and time are a guess, and no hash checks it, so
                // a continued move never deletes a source on it.
                self.list_entry(path, Reason::SourceKept, folder);
                None
            }
            Decision::Done => {
                self.copying.progress.add_items(1);
                None
            }
            Decision::Skip => {
                self.list_entry(path, Reason::Skipped, folder);
                None
            }
            // Only a folder merges.
            Decision::Mismatch | Decision::Merge => {
                self.fail(path, ERROR_ALREADY_EXISTS, folder);
                None
            }
        }
    }

    /// The destination folder for the source folder `name`: created with
    /// the source folder as its template, which keeps its attributes, or the
    /// folder of that name it merges into. `None` when the folder is not
    /// copied, which is counted and listed here.
    #[allow(clippy::too_many_arguments)]
    fn folder_target(
        &self,
        target: &Target,
        name: &[u16],
        from: &[u16],
        wanted: Wanted,
        path: &[u16],
        folder: Option<&Folder>,
        page: &mut [u64],
    ) -> Option<Arc<Target>> {
        let decision = {
            let mut names = target.names.lock().expect("no worker panics");
            decide(&mut names, name, wanted, self.copying.rule)
        };
        let (to, merge) = match decision {
            Decision::Write { name, .. } => (join(&target.verbatim, &name), false),
            Decision::Merge => (join(&target.verbatim, name), true),
            Decision::Skip => {
                self.list_entry(path, Reason::Skipped, folder);
                return None;
            }
            Decision::Mismatch | Decision::Done => {
                self.fail(path, ERROR_ALREADY_EXISTS, folder);
                return None;
            }
        };
        let made = !merge && {
            // A folder this operation made can briefly answer that it does
            // not exist below it, as folders on a file server did for
            // AngelCOPY, whose subtrees were then lost.
            let retry_missing = target.made;
            let created = retry_also(
                self.copying.cancel,
                |code| retry_missing && code == ERROR_PATH_NOT_FOUND,
                || create_folder(&to, Some(from)).map_err(|error| error.code),
            );
            match created {
                Ok(()) => true,
                // Another program made it after the listing: merge, as the
                // listing would have.
                Err(ERROR_ALREADY_EXISTS) if is_folder_path(&to) => false,
                Err(error) => {
                    self.fail(path, error, folder);
                    return None;
                }
            }
        };
        let names = if made {
            Names::default()
        } else {
            let names = open_entry(&to, Access::List, Reparse::Open)
                .map_err(|error| error.code)
                .and_then(|entry| read_names(&entry, page));
            match names {
                Ok(names) => names,
                Err(error) => {
                    self.fail(path, error, folder);
                    return None;
                }
            }
        };
        self.copying.progress.add_items(1);
        Some(Arc::new(Target {
            verbatim: to,
            names: Mutex::new(names),
            made,
        }))
    }

    fn is_large(&self, file: &File) -> bool {
        (self.copying.plan.large_from).is_some_and(|limit| file.size >= limit)
    }

    /// Copies one file under its temporary name, renames it to its final
    /// name, and for a move deletes its source. `folder` is its source
    /// folder; `None` for a selected file.
    fn copy_file(&self, file: &File, folder: Option<&Arc<Folder>>) {
        let folder_ref = folder.map(|folder| &**folder);
        let partial = partial_name(&file.final_name);
        let partial_path = join(&file.target.verbatim, &partial);
        let delete_partial = || {
            // Our own temporary file, which nobody else has a reason to keep.
            let open = || open_entry(&partial_path, Access::DeleteReadOnly, Reparse::Open);
            remove(&AtomicBool::new(false), &self.destination, open)
        };
        // Only an interrupted run of Gozler makes that name, so only Continue
        // finds it, in the listing.
        let stale = {
            let names = file.target.names.lock().expect("no worker panics");
            matches!(names.get(&partial), Some(Holder::Found(_)))
        };
        if stale && let Err(error) = delete_partial() {
            return self.fail(&file.path, error, folder_ref);
        }
        let folder_link = file.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let source = if !is_name_surrogate(file.reparse_tag) {
            CopySource::File
        } else if file.reparse_tag == IO_REPARSE_TAG_SYMLINK {
            CopySource::SymbolicLink {
                folder: folder_link,
            }
        } else {
            CopySource::ReparsePoint {
                folder: folder_link,
            }
        };
        let options = CopyOptions {
            source,
            unbuffered: self.is_large(file),
            sparse: self.copying.sparse && file.attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0,
        };
        let progress = self.copying.progress;
        let mut reported = 0u64;
        // For a move, the source as the copy found it, read by the path the
        // copy reads.
        let mut before = None;
        let copied = retry(self.copying.cancel, || {
            if self.copying.moving {
                let source = open_entry(&file.from, Access::Inspect, Reparse::Open);
                before = Some(Snapshot::of(&source.map_err(|error| error.code)?)?);
            }
            let mut on_chunk = |copied: u64| {
                // The total also counts the alternate data streams.
                let copied = copied.min(file.size);
                progress.add_bytes(copied.saturating_sub(reported));
                reported = reported.max(copied);
                !self.cancelled()
            };
            copy_file(&file.from, &partial_path, options, &mut on_chunk).map_err(|error| error.code)
        });
        if let Err(error) = copied {
            // `CopyFile2` deleted what it wrote.
            progress.remove_bytes(reported);
            match error {
                ERROR_REQUEST_ABORTED if self.cancelled() => {}
                ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => {
                    self.full.store(true, Ordering::Relaxed);
                    if let Some(folder) = folder_ref {
                        folder.keep();
                    }
                }
                // A file the walk listed that is gone since.
                error if is_gone(error) && folder.is_some() => self.copying.progress.add_items(1),
                error => self.fail(&file.path, error, folder_ref),
            }
            return;
        }
        let placing = Placing {
            from: &partial_path,
            base: &file.name,
            name: file.final_name.clone(),
            replace: file.replace,
            is_folder: folder_link,
        };
        let reason = match place(&file.target, placing, self.copying.rule.choice) {
            Placed::Named => None,
            Placed::Skipped => Some(Reason::Skipped),
            Placed::Failed(error) => Some(Reason::Failed(error)),
        };
        if let Some(reason) = reason {
            progress.remove_bytes(reported);
            let _ = delete_partial();
            return self.list_entry(&file.path, reason, folder_ref);
        }
        if let Some(before) = before {
            // No cancel between the final rename and the source's delete, so
            // a cancel leaves each file in one place: the cancel's
            // `CancelSynchronousIo` may abort the delete once, so it is tried
            // again, and a failure is listed even after the cancel, because
            // the file is then at both places.
            let never = AtomicBool::new(false);
            let access = Access::delete(file.attributes);
            let open = || match folder {
                Some(folder) => (folder.children()).open_child(&file.name, access, Reparse::Open),
                None => open_entry(&file.from, access, Reparse::Open),
            };
            let aborted = |code| code == ERROR_OPERATION_ABORTED;
            // The handle that deletes is the one compared, so the entry
            // deleted is the one found unchanged.
            let deleted = retry_also(&never, aborted, || {
                let source = open().map_err(|error| error.code)?;
                if Snapshot::of(&source)? != before {
                    return Ok(false);
                }
                file.disposition.delete_entry(&source).map(|()| true)
            });
            let reason = match deleted {
                Ok(true) => None,
                Err(error) if is_gone(error) => None,
                Ok(false) => Some(Reason::SourceChanged),
                Err(error) => Some(Reason::Failed(error)),
            };
            if let Some(reason) = reason {
                if let Some(folder) = folder_ref {
                    folder.keep();
                }
                progress.add_items(1);
                let mut entries = self.listed.lock().expect("no worker panics");
                entries.push(listed(&file.path, reason));
                return;
            }
        }
        progress.add_items(1);
    }

    /// Gives up one count of `folder`; for a move, each source folder that
    /// it empties is deleted.
    fn release(&self, folder: Arc<Folder>) {
        if !self.copying.moving {
            return;
        }
        Folder::release(
            folder,
            || self.stopped(),
            |folder| {
                let never = AtomicBool::new(false);
                let reopen = || folder.entry.reopen(Access::delete(folder.attributes));
                match remove(&never, &folder.disposition, reopen) {
                    Ok(()) => true,
                    Err(error) => {
                        self.fail(&folder.path, error, None);
                        false
                    }
                }
            },
        );
    }
}

/// What a moved source is. Its source is deleted only while this is
/// unchanged from before its copy: the Windows engine copies a file that
/// another program writes to, and a program can rename a new file over it.
#[derive(PartialEq, Eq)]
struct Snapshot {
    identity: FileIdentity,
    stat: EntryStat,
}

impl Snapshot {
    fn of(entry: &Entry) -> Result<Snapshot, u32> {
        Ok(Snapshot {
            identity: entry.identity().map_err(|error| error.code)?,
            stat: entry.stat().map_err(|error| error.code)?,
        })
    }
}

fn is_folder_path(path: &[u16]) -> bool {
    path_attributes(path).is_ok_and(|attributes| {
        attributes.is_some_and(|attributes| attributes & FILE_ATTRIBUTE_DIRECTORY != 0)
    })
}
