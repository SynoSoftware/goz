//! What the window asks the queue for, and what it reads back.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// An operation the window asks for. Every path is a full path: the active
/// panel's selection, or Search's selection, which is the files of one
/// moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Copies the entries to `destination`.
    Copy {
        sources: Vec<PathBuf>,
        destination: Destination,
    },
    /// Moves the entries to `destination`: a rename on one volume, a copy
    /// and a delete across volumes.
    Move {
        sources: Vec<PathBuf>,
        destination: Destination,
    },
    /// Deletes the entries permanently.
    Delete { sources: Vec<PathBuf> },
    /// Creates the folder `path`; an existing name is refused.
    NewFolder { path: PathBuf },
}

/// Where a copy or a move writes. Prepare decides what the window cannot
/// without reading the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Into the folder `0`. Prepare creates it, with its missing parents,
    /// when it does not exist.
    Folder(PathBuf),
    /// One entry's destination as the person typed it: the folder `0` when
    /// it exists and is not the entry itself, otherwise the entry's new
    /// full path. Prepare replaces it with [`Destination::Folder`] or
    /// [`Destination::Named`], so it never runs or is saved.
    Typed(PathBuf),
    /// One entry's new full path: `name` in `folder`.
    Named { folder: PathBuf, name: OsString },
}

impl Destination {
    /// The path the person sees: the folder, or the entry's new full path.
    pub fn path(&self) -> PathBuf {
        match self {
            Destination::Folder(path) | Destination::Typed(path) => path.clone(),
            Destination::Named { folder, name } => folder.join(name),
        }
    }

    /// The folder it writes into; for [`Destination::Typed`], which prepare
    /// has not decided, the typed path.
    pub fn folder(&self) -> &Path {
        match self {
            Destination::Folder(path) | Destination::Typed(path) => path,
            Destination::Named { folder, .. } => folder,
        }
    }
}

/// Names an operation in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub(crate) u64);

impl OperationId {
    /// An id for the tests of the window's lists, which run without a
    /// queue; an id the queue never gave names no operation.
    #[doc(hidden)]
    pub fn for_tests(n: u64) -> OperationId {
        OperationId(n)
    }
}

/// One operation as the window reads it.
#[derive(Debug, Clone)]
pub struct Operation {
    pub id: OperationId,
    pub request: Arc<Request>,
    pub state: State,
}

/// An operation's state, as the window reads it.
#[derive(Debug, Clone)]
pub enum State {
    Preparing,
    /// Waiting for the person's answer.
    Prepared(Prepared),
    /// Submitted, and waiting for an operation that runs, or for one before
    /// it in the run order.
    Waiting {
        on: OperationId,
        reason: Wait,
        /// Prepare's total.
        total: Option<Total>,
    },
    /// Running, or paused.
    Running(Running),
    /// Stays until the window removes it.
    Ended(Arc<Outcome>),
    /// Saved by a run that did not end, as after a crash; Continue or
    /// Dismiss it.
    Interrupted,
}

/// A running operation's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// Items done so far: entries copied, moved or deleted, skipped and
    /// failed ones included.
    pub items: u64,
    /// Bytes copied so far.
    pub bytes: u64,
    /// The bytes of files counted in `items` but not copied: files that
    /// Continue finds done, and skipped and failed files. They count toward
    /// the total and not toward the speed, because a walk passes many at
    /// once.
    pub passed: u64,
    /// Prepare's total.
    pub total: Option<Total>,
    /// It moves no bytes: a move whose every entry is renamed on one
    /// volume, or a rename.
    pub renames: bool,
    /// Paused: it changes nothing until it is resumed, and still makes
    /// other operations wait.
    pub paused: bool,
    /// The file most recently started; `None` before the first.
    pub current: Option<Current>,
}

/// The file a running operation most recently started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    /// For a delete, the entry being deleted.
    pub source: PathBuf,
    /// Where it is written; `None` for a delete.
    pub destination: Option<PathBuf>,
    /// This file's bytes copied; 0 for a delete.
    pub done: u64,
    /// 0 for a delete.
    pub size: u64,
}

/// Why a waiting operation cannot move in the run order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrderRefused {
    /// It, or the operation it would move before, is no longer waiting.
    #[error("the operation is no longer waiting")]
    NotWaiting,
    /// The move would pass `other`, whose paths it overlaps (D14), so it
    /// stays on `side` of it.
    #[error("the operation must stay {side:?} an operation it overlaps")]
    Overlaps { other: OperationId, side: Side },
}

/// Where an operation stays relative to another in the run order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    After,
    Before,
}

/// What prepare found, so the window can ask the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// The selected entries the operation acts on.
    pub entries: usize,
    /// For a copy, a move or a delete, from gozlerd's index; `None` when
    /// gozlerd cannot answer exactly, and then the progress has no total.
    pub total: Option<Total>,
    /// For a copy or a move.
    pub conflicts: Option<Conflicts>,
}

/// What an operation covers: the selected entries and everything below
/// them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Total {
    /// Files and folders.
    pub items: u64,
    /// The bytes it copies; none for a delete and a move by rename.
    pub bytes: u64,
}

impl std::ops::Add for Total {
    type Output = Total;

    fn add(self, other: Total) -> Total {
        Total {
            items: self.items + other.items,
            bytes: self.bytes + other.bytes,
        }
    }
}

impl std::iter::Sum for Total {
    fn sum<I: Iterator<Item = Total>>(totals: I) -> Total {
        totals.fold(Total::default(), |sum, total| sum + total)
    }
}

/// The names that a copy or a move finds taken in its destination folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Conflicts {
    /// Files at the destination that a source would replace.
    pub files: u64,
    /// Selected entries that share a name with another selected entry.
    pub same_name: u64,
    /// Selected folders that merge into a folder of their name. Prepare does
    /// not read the files inside them, so the window asks the person as for
    /// a conflict, and the run applies the answer to each file it meets.
    pub merges: u64,
}

/// Why a submitted operation waits (D14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// One of the two writes or deletes a path that the other reads or
    /// writes.
    Paths,
    /// Both move bytes on one disk that seeks.
    Disk,
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Complete,
    Cancelled,
    /// The destination disk filled, which would fail every later file too.
    DiskFull,
    Refused(Refusal),
}

/// What an operation refuses at prepare, or a copy at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the destination is inside a selected folder")]
    DestinationInsideSource,
    /// No folder above the destination exists, as when its drive is gone.
    #[error("no folder above the destination exists")]
    DestinationMissing,
    #[error("the destination cannot be read: Windows error {0}")]
    DestinationUnreadable(u32),
    #[error("the copy needs {needed} bytes and the destination has {free} free")]
    TooLittleSpace { needed: u64, free: u64 },
}

/// An entry the result lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// With Windows' error code.
    Failed(u32),
    /// A conflict that the choice skipped, or that nobody was asked about.
    Skipped,
    /// A continued move found the file already at the destination and kept
    /// its source, because equal size and time are a guess.
    SourceKept,
    /// A move kept a source that another program changed or replaced while
    /// it was copied, so its copy may not hold what it holds now.
    SourceChanged,
}

/// What an ended operation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// Items done, as [`State::Running`] counts them; for a delete, a
    /// selected entry that no longer existed included.
    pub items: u64,
    pub bytes: u64,
    /// In the order they happened.
    pub listed: Vec<Listed>,
    pub ring: RingCounts,
}

impl Outcome {
    pub(crate) fn ended(end: End) -> Outcome {
        Outcome {
            end,
            items: 0,
            bytes: 0,
            listed: Vec::new(),
            ring: RingCounts::default(),
        }
    }
}

/// What the ring did not do itself, so a ring or a clone that never runs
/// shows in the result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingCounts {
    /// Files that the ring could not copy, which the Windows copy function
    /// copied again.
    pub to_windows: u64,
    /// Clones that the volume refused, which the ring copied instead.
    pub clones_refused: u64,
}

/// A running operation's counters, which the window reads about twice a
/// second.
#[derive(Default)]
pub(crate) struct Progress {
    items: AtomicU64,
    bytes: AtomicU64,
    passed: AtomicU64,
    current: Mutex<Option<Arc<CurrentFile>>>,
    /// Runs once, on the engine's thread, at the operation's first report
    /// that counts an item or a byte, while the engine waits for it, so a
    /// test acts at a point the engine cannot have passed.
    #[cfg(test)]
    pub(crate) at_first_report: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// The file an engine works on, which [`Progress`] shows as the current one
/// while no later file has started.
pub(crate) struct CurrentFile {
    /// As the window gave it.
    pub(crate) source: Vec<u16>,
    /// With the `\\?\` or `\\?\UNC\` prefix; `None` for a delete.
    pub(crate) destination: Option<Vec<u16>>,
    pub(crate) size: u64,
    done: AtomicU64,
}

impl CurrentFile {
    /// Its bytes copied so far.
    pub(crate) fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub(crate) fn set_done(&self, done: u64) {
        self.done.store(done, Ordering::Relaxed);
    }
}

/// Pause and Resume of a running operation. Each engine waits in
/// [`Pause::wait`] where it can stop without leaving work half done: the
/// ring between blocks, `CopyFile2` in its progress callback, and the
/// workers between entries.
#[derive(Default)]
pub(crate) struct Pause {
    /// Read on every block, chunk and entry; `state` decides.
    paused: AtomicBool,
    state: Mutex<bool>,
    resumed: Condvar,
    /// The engine threads waiting now, which a test waits for.
    #[cfg(test)]
    pub(crate) waiting: std::sync::atomic::AtomicUsize,
}

impl Pause {
    pub(crate) fn set(&self, paused: bool) {
        let mut state = self.state.lock().expect("no engine thread panics");
        *state = paused;
        self.paused.store(paused, Ordering::Relaxed);
        self.resumed.notify_all();
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Wakes every waiting engine thread, after `cancel` was set, so it sees
    /// the cancel.
    pub(crate) fn wake(&self) {
        let _state = self.state.lock().expect("no engine thread panics");
        self.resumed.notify_all();
    }

    /// Waits while the operation is paused, until it resumes or `cancel`
    /// is set; the caller then checks `cancel` as it always does.
    pub(crate) fn wait(&self, cancel: &AtomicBool) {
        if !self.is_paused() {
            return;
        }
        let mut state = self.state.lock().expect("no engine thread panics");
        #[cfg(test)]
        self.waiting.fetch_add(1, Ordering::Relaxed);
        while *state && !cancel.load(Ordering::Relaxed) {
            state = self.resumed.wait(state).expect("no engine thread panics");
        }
        #[cfg(test)]
        self.waiting.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Progress {
    pub(crate) fn add_items(&self, count: u64) {
        self.items.fetch_add(count, Ordering::Relaxed);
        self.reported();
    }

    /// Counts an item that is not copied, with `bytes`, the size of its
    /// file in the total, as [`Running::passed`].
    pub(crate) fn pass(&self, bytes: u64) {
        self.passed.fetch_add(bytes, Ordering::Relaxed);
        self.add_items(1);
    }

    pub(crate) fn add_bytes(&self, count: u64) {
        self.bytes.fetch_add(count, Ordering::Relaxed);
        if count > 0 {
            self.reported();
        }
    }

    #[cfg(test)]
    fn reported(&self) {
        if let Some(hook) = self.at_first_report.lock().expect("no hook panics").take() {
            hook();
        }
    }

    #[cfg(not(test))]
    fn reported(&self) {}

    /// Makes `source` the current file, which `destination` receives, and
    /// returns it so the engine can count its bytes.
    pub(crate) fn start_file(
        &self,
        source: &[u16],
        destination: Option<Vec<u16>>,
        size: u64,
    ) -> Arc<CurrentFile> {
        let file = Arc::new(CurrentFile {
            source: source.to_vec(),
            destination,
            size,
            done: AtomicU64::new(0),
        });
        let earlier = self
            .current
            .lock()
            .expect("no engine thread panics")
            .replace(Arc::clone(&file));
        // The earlier file's last owner may be this slot, so it drops after
        // the lock.
        drop(earlier);
        file
    }

    pub(crate) fn current(&self) -> Option<Arc<CurrentFile>> {
        self.current
            .lock()
            .expect("no engine thread panics")
            .clone()
    }

    /// Takes back the bytes of a file that was not copied after all.
    pub(crate) fn remove_bytes(&self, count: u64) {
        self.bytes.fetch_sub(count, Ordering::Relaxed);
    }

    pub(crate) fn items(&self) -> u64 {
        self.items.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn passed(&self) -> u64 {
        self.passed.load(Ordering::Relaxed)
    }
}
