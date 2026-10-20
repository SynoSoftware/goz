//! Copy with the Windows engine, and a move across volumes, which is a copy
//! that deletes each source when its copy has its final name.
//!
//! The walk streams, as the delete's does and as AngelCOPY's `WalkStream`
//! does: the worker that lists a source folder creates its destination
//! folder, reads that folder's names once, decides each file's final name
//! under that folder's lock, and hands its files on, up to 256 under the
//! large-file limit as one item and each large file to the one-at-a-time
//! lane. Copying starts while the walk still runs. Each file is written
//! straight to its final name (D10); an interrupted run leaves its file
//! there, and Continue resumes it in the ring.

mod ring;
mod streams;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gozler_core::layout::FolderEntry;
use gozler_winfs::{
    Access, CopyOptions, CopySource, Entry, EntryStat, Existing, FileIdentity, Reparse, copy_file,
    create_folder, open_entry, path_attributes, windows_build,
};

use crate::conflicts::{Decision, Rule, Run, Wanted, decide, interrupted_replace};
use crate::delete::{Disposition, remove};
use crate::io_threads::IoThreads;
use crate::listing::{FILES_PER_ITEM, LISTING_WORDS, is_folder, join, kind, list, read_names};
use crate::names::{Kind, Names};
use crate::operation::{Listed, Pause, Progress, Reason, RingCounts};
use crate::place::{Placed, Placing, Target, place, remove_replaced};
use crate::pool::{Lane, Pool};
use crate::receive::{Selected, listed};
use crate::retry::{retry, retry_also};
use crate::volumes::CopyPlan;
use crate::win32::{
    COPY_FILE_22H2_BUILD, ERROR_ALREADY_EXISTS, ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL,
    ERROR_OPERATION_ABORTED, ERROR_PATH_NOT_FOUND, ERROR_REQUEST_ABORTED, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE,
    IO_DESIRED_SIZE, IO_REPARSE_TAG_SYMLINK, is_gone, is_name_surrogate,
};
use ring::Ring;

/// How one copy runs.
pub(crate) struct Copying<'a> {
    pub(crate) plan: CopyPlan,
    pub(crate) rule: Rule,
    /// A move across volumes: each source is deleted when its copy has its
    /// final name, and each source folder when it is empty.
    pub(crate) moving: bool,
    pub(crate) target: TargetVolume,
    pub(crate) cancel: &'a AtomicBool,
    /// The queue closes, which also sets `cancel`: a file in progress then
    /// stays for Continue to resume, where the person's cancel deletes it
    /// (D13).
    pub(crate) closing: &'a AtomicBool,
    pub(crate) pause: &'a Pause,
    pub(crate) progress: &'a Progress,
    pub(crate) io: &'a IoThreads,
}

/// What the destination volume can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TargetVolume {
    /// `CopyFile2` keeps sparse files sparse: Windows build 22621 or later,
    /// and a destination that supports sparse files.
    pub(crate) sparse_copy: bool,
    /// The destination supports sparse files, which the ring keeps sparse
    /// on any Windows.
    pub(crate) sparse_files: bool,
    /// A FAT or exFAT file system.
    pub(crate) fat: bool,
    /// The volume's cluster size, within one volume that can clone blocks,
    /// where the ring clones (D20).
    pub(crate) clone_cluster: Option<u32>,
}

pub(crate) struct Copied {
    /// In the order they happened.
    pub(crate) listed: Vec<Listed>,
    /// The destination disk filled, which stopped the copy.
    pub(crate) disk_full: bool,
    pub(crate) ring: RingCounts,
}

/// A selected entry, and whether it is copied into its own folder.
pub(crate) struct Source {
    pub(crate) selected: Selected,
    pub(crate) own_folder: bool,
    /// Another selected entry has its name ([`Wanted::shared`]).
    pub(crate) shared: bool,
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
    // The buffer exists only while the copy runs, and only when the ring has
    // files to copy, so a first run that the settings give the Windows
    // engine allocates nothing.
    let ring = (copying.plan.ring)
        .filter(|plan| plan.from.is_some() || !matches!(copying.rule.run, Run::First { .. }))
        .and_then(Ring::new);
    let (ring, io) = match ring {
        Some((ring, io)) => (Some(ring), Some(io)),
        None => (None, None),
    };
    let walk = Walk {
        copying,
        full: &full,
        listed: Mutex::default(),
        pool: Pool::new(items, &stopped),
        destination: Disposition::default(),
        io_desired_size: (windows_build() >= COPY_FILE_22H2_BUILD).then_some(IO_DESIRED_SIZE),
        ring,
        to_windows: AtomicU64::new(0),
        clones_refused: AtomicU64::new(0),
    };
    std::thread::scope(|scope| {
        let walking: Vec<_> = (0..copying.plan.workers.max(1))
            .map(|_| scope.spawn(|| walk.work(Lane::Workers)))
            .chain((copying.plan.lane_beside).then(|| scope.spawn(|| walk.work(Lane::Large))))
            .collect();
        if let (Some(ring), Some(io)) = (&walk.ring, io) {
            scope.spawn(|| ring.run(&walk, io));
            for _ in 0..ring.plan.openers {
                scope.spawn(|| ring.open_and_close(&walk));
            }
            for walker in walking {
                walker.join().expect("no worker panics");
            }
            // The walk hands the ring nothing more.
            ring.close_input();
        }
    });
    Copied {
        listed: walk.listed.into_inner().expect("no worker panics"),
        disk_full: full.load(Ordering::Relaxed),
        ring: RingCounts {
            to_windows: walk.to_windows.into_inner(),
            clones_refused: walk.clones_refused.into_inner(),
        },
    }
}

/// A source folder in flight, with the destination folder it copies into.
struct Walking {
    verbatim: Vec<u16>,
    target: Arc<Target>,
    /// The selected folder it is in shares its name ([`Wanted::shared`]).
    shared: bool,
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
    write: Write,
    size: u64,
    attributes: u32,
    reparse_tag: u32,
    /// [`Wanted::shared`].
    shared: bool,
    /// How its source volume deletes, for a move.
    disposition: Arc<Disposition>,
}

/// What a copy finds at a file's final name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// Nothing: the copy creates the file.
    New,
    /// The person's file, which Replace writes over; `interrupted` when it
    /// is the interrupted run's Replace ([`interrupted_replace`]).
    Replace { interrupted: bool },
    /// The file an interrupted run was writing, which the copy resumes.
    Resume,
}

impl File {
    /// Its final name, written over the file there for Replace and Resume.
    fn placing(&self) -> Placing<'_> {
        Placing {
            base: &self.name,
            name: self.final_name.clone(),
            replace: self.write != Write::New,
            is_folder: self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            shared: self.shared,
        }
    }
}

/// A file the walk handed to the ring, with its source folder, which it
/// holds until the file is done.
struct Handed {
    file: File,
    folder: Option<Arc<Folder>>,
}

struct Walk<'a> {
    copying: &'a Copying<'a>,
    full: &'a AtomicBool,
    listed: Mutex<Vec<Listed>>,
    pool: Pool<'a, Work>,
    /// How the destination volume deletes, for the files the ring gives up.
    destination: Disposition,
    /// `ioDesiredSize` for an unbuffered `CopyFile2`; `None` before
    /// [`COPY_FILE_22H2_BUILD`].
    io_desired_size: Option<u32>,
    /// `None` without the ring, or when its buffer could not be allocated.
    ring: Option<Ring>,
    /// The result's [`RingCounts`].
    to_windows: AtomicU64,
    clones_refused: AtomicU64,
}

impl Walk<'_> {
    fn work(&self, lane: Lane) {
        let _entered = self.copying.io.enter();
        let mut page = vec![0u64; LISTING_WORDS];
        while let Some(work) = self.pool.next(lane) {
            self.copying.pause.wait(self.copying.cancel);
            match work {
                Work::Selected(source, target, disposition) => {
                    self.selected(source, &target, disposition, &mut page);
                }
                Work::List(folder) => self.list(folder, &mut page),
                Work::Files(folder, files) => {
                    for file in files {
                        self.copying.pause.wait(self.copying.cancel);
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
            shared,
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
                copied: true,
                shared,
            };
            let entry = FolderEntry {
                name,
                attributes: tag.attributes,
                size: stat.size,
                created: stat.created,
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
            let Some(file) = decided.and_then(|file| self.hand_to_ring(file, None)) else {
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
            copied: true,
            shared,
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
            shared,
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
                copied: true,
                shared: folder.walk.shared,
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
            let Some(file) = decided.and_then(|file| self.hand_to_ring(file, Some(&folder))) else {
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
                copied: true,
                shared: parent.walk.shared,
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
            if let Some(file) = decided.and_then(|file| self.hand_to_ring(file, Some(parent))) {
                self.copy_file(&file, Some(parent));
            }
            return;
        }
        let wanted = Wanted {
            kind: Kind::Folder,
            size: 0,
            last_write: 0,
            own_folder: false,
            copied: true,
            shared: parent.walk.shared,
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
            shared: parent.walk.shared,
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
        let (decision, interrupted) = {
            let mut names = target.names.lock().expect("no worker panics");
            // Read before `decide` takes the name.
            let holder = names.get(&entry.name);
            let decision = decide(&mut names, &entry.name, wanted, self.copying.rule);
            (decision, interrupted_replace(holder, self.copying.rule))
        };
        let (final_name, write) = match decision {
            Decision::Write {
                name,
                replace: false,
            } => (name, Write::New),
            Decision::Write {
                name,
                replace: true,
            } => (name, Write::Replace { interrupted }),
            Decision::Resume { name } => (name, Write::Resume),
            Decision::Done if self.copying.moving => {
                // Equal size and time are a guess, and no hash checks it, so
                // a continued move never deletes a source on it.
                self.list_entry(path, Reason::SourceKept, folder);
                return None;
            }
            Decision::Done => {
                self.copying.progress.add_items(1);
                return None;
            }
            Decision::Skip => {
                self.list_entry(path, Reason::Skipped, folder);
                return None;
            }
            // Only a folder merges.
            Decision::Mismatch | Decision::Merge => {
                self.fail(path, ERROR_ALREADY_EXISTS, folder);
                return None;
            }
        };
        Some(File {
            path: path.to_vec(),
            from: from.to_vec(),
            target: Arc::clone(target),
            final_name,
            write,
            size: entry.size,
            attributes: entry.attributes,
            reparse_tag: entry.reparse_tag.unwrap_or(0),
            shared: wanted.shared,
            disposition: Arc::clone(disposition),
            name: entry.name,
        })
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
            Decision::Resume { .. } => unreachable!("only a file is unfinished"),
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

    /// Hands a decided file to the ring when the ring copies it, and
    /// otherwise gives it back: a file at the ring's size, and every file
    /// that Continue resumes, the interrupted run's Replace included,
    /// because only the ring keeps the start that already equals the
    /// source. Any other Replace follows the size, so many small files
    /// still go to the Windows engine's workers. A link
    /// and an EFS file go to the Windows engine, because the ring reads a
    /// file's data unfiltered and could not keep it encrypted.
    fn hand_to_ring(&self, file: File, folder: Option<&Arc<Folder>>) -> Option<File> {
        let Some(plan) = self.copying.plan.ring else {
            return Some(file);
        };
        let resumes = matches!(
            file.write,
            Write::Resume | Write::Replace { interrupted: true }
        );
        let takes = (resumes || plan.from.is_some_and(|from| file.size >= from))
            && file.reparse_tag == 0
            && file.attributes & FILE_ATTRIBUTE_ENCRYPTED == 0;
        if !takes {
            return Some(file);
        }
        let Some(ring) = &self.ring else {
            // The ring's buffer could not be allocated.
            self.to_windows.fetch_add(1, Ordering::Relaxed);
            return Some(file);
        };
        if let Some(folder) = folder {
            folder.hold();
        }
        ring.hand(Handed {
            file,
            folder: folder.cloned(),
        });
        None
    }

    /// Copies one file with the Windows engine to its final name, then
    /// [`Walk::finish`]es it. A file Continue resumes is removed and copied
    /// again from its start. `folder` is its source folder; `None` for a
    /// selected file.
    fn copy_file(&self, file: &File, folder: Option<&Arc<Folder>>) {
        let folder_ref = folder.map(|folder| &**folder);
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
        let unbuffered = self.is_large(file);
        let progress = self.copying.progress;
        let mut reported = 0u64;
        // For a move, the source as the copy found it, read by the path the
        // copy reads.
        let mut before = None;
        let copy_to = |to: &[u16], existing| {
            if existing == Existing::Replace {
                remove_replaced(self.copying.cancel, &self.destination, to)?;
            }
            let options = CopyOptions {
                source,
                unbuffered,
                sparse: self.copying.target.sparse_copy
                    && file.attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0,
                io_desired_size: self.io_desired_size.filter(|_| unbuffered),
            };
            let current = progress.start_file(&file.path, Some(to.to_vec()), file.size);
            retry(self.copying.cancel, || {
                if self.copying.moving {
                    let source = open_entry(&file.from, Access::Inspect, Reparse::Open);
                    before = Some(Snapshot::of(&source.map_err(|error| error.code)?)?);
                }
                let mut on_chunk = |copied: u64| {
                    // The total also counts the alternate data streams.
                    let copied = copied.min(file.size);
                    progress.add_bytes(copied.saturating_sub(reported));
                    reported = reported.max(copied);
                    current.set_done(copied);
                    // Pause holds the copy here, inside the callback.
                    self.copying.pause.wait(self.copying.cancel);
                    // A stopped `CopyFile2` deletes what it wrote, so at Exit
                    // the copy goes on until the process ends (`Queue::close`).
                    !self.cancelled() || self.copying.closing.load(Ordering::Relaxed)
                };
                copy_file(&file.from, to, options, &mut on_chunk).map_err(|error| error.code)
            })
        };
        let mut placing = file.placing();
        // A link put at a resumed file's name since the listing is not this
        // operation's file, so the choice applies to it.
        if file.write == Write::Resume
            && let Ok(Some(attributes)) =
                path_attributes(&join(&file.target.verbatim, &file.final_name))
            && attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            placing.replace = false;
        }
        let error = match place(&file.target, placing, self.copying.rule.choice, copy_to) {
            Placed::Named(()) => return self.finish(file, folder, before),
            Placed::Skipped => return self.list_entry(&file.path, Reason::Skipped, folder_ref),
            Placed::Failed(error) => error,
        };
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
    }

    /// Counts a file copied whole at its final name and, for a move, deletes
    /// its source when `before` still describes it.
    fn finish(&self, file: &File, folder: Option<&Arc<Folder>>, before: Option<Snapshot>) {
        let folder_ref = folder.map(|folder| &**folder);
        let progress = self.copying.progress;
        if let Some(before) = before {
            // No cancel between the end of the copy and the source's delete,
            // so a cancel leaves each file in one place: the cancel's
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
