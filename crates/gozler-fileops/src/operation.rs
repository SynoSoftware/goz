//! What the window asks the queue for, and what it reads back.

use crate::conflicts::Policy;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, TryLockError, Weak};

/// An operation the window asks for. Every path is a full path: the active
/// panel's selection, or Search's selection, which is the files of one
/// moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Copies the entries to `destination`.
    Copy {
        sources: Vec<PathBuf>,
        destination: Destination,
        policy: Policy,
    },
    /// Moves the entries to `destination`: a rename on one volume, a copy
    /// and a delete across volumes.
    Move {
        sources: Vec<PathBuf>,
        destination: Destination,
        policy: Policy,
    },
    /// Deletes the entries permanently.
    Delete { sources: Vec<PathBuf> },
    /// Creates the folder `path`; an existing name is refused.
    NewFolder { path: PathBuf },
    Synchronize {
        source: PathBuf,
        destination: PathBuf,
        mirror: bool,
    },
}

impl Request {
    pub fn is_transfer(&self) -> bool {
        matches!(
            self,
            Self::Copy { .. } | Self::Move { .. } | Self::Synchronize { .. }
        )
    }

    pub fn policy(&self) -> Option<Policy> {
        match self {
            Self::Copy { policy, .. } | Self::Move { policy, .. } => Some(*policy),
            Self::Synchronize { .. } => Some(Policy::UpdateChanged),
            Self::Delete { .. } | Self::NewFolder { .. } => None,
        }
    }
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
    pub folder: SourcesFolder,
    pub state: State,
}

/// The folder that holds every source of an operation, which the window
/// names instead of each source. A queue thread works it out, because
/// reading every source of a large operation takes the window several
/// frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcesFolder {
    /// Not worked out yet: while the operation prepares, and for a moment
    /// after the queue opens for an interrupted one. A new operation's
    /// folder is known before it is prepared.
    Pending,
    /// The deepest folder that holds every source, also of one, as text.
    Shared(String),
    /// No folder holds every source, because they are on different drives
    /// or servers, or because there are none, as for a new folder.
    Several,
}

impl SourcesFolder {
    /// The folder of `request`'s sources.
    pub(crate) fn of(request: &Request) -> SourcesFolder {
        let sources = match request {
            Request::Copy { sources, .. }
            | Request::Move { sources, .. }
            | Request::Delete { sources } => sources,
            Request::NewFolder { .. } => return SourcesFolder::Several,
            Request::Synchronize { source, .. } => {
                return SourcesFolder::Shared(source.to_string_lossy().into_owned());
            }
        };
        common_folder(sources.iter().map(|path| path.to_string_lossy()))
            .map_or(SourcesFolder::Several, SourcesFolder::Shared)
    }
}

/// The deepest folder that holds every path of `paths`, compared as Windows
/// compares names, ignoring case; `None` when they share no drive or
/// server. It allocates nothing for each path, because an operation can
/// have 100,000 sources.
fn common_folder(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Option<String> {
    let mut paths = paths.into_iter();
    let first = paths.next()?;
    let first = parent_of(first.as_ref());
    // How many of `first`'s parts every path shares.
    let mut common = first.split('\\').count();
    for path in paths {
        common = (first.split('\\').zip(parent_of(path.as_ref()).split('\\')))
            .take(common)
            .take_while(|(a, b)| same_name(a, b))
            .count();
    }
    let parts = || first.split('\\').take(common);
    // Two drives share nothing, and two servers share only the `\\` that
    // starts each path.
    if parts().all(str::is_empty) {
        return None;
    }
    let end = parts().map(str::len).sum::<usize>() + common - 1;
    let folder = &first[..end];
    Some(if folder.ends_with(':') {
        format!("{folder}\\")
    } else {
        folder.to_owned()
    })
}

/// The parent folder of a Windows path, without its last `\`; a drive's
/// root keeps its `\`.
fn parent_of(path: &str) -> &str {
    match path.trim_end_matches('\\').rfind('\\') {
        Some(cut) if path[..cut].ends_with(':') => &path[..=cut],
        Some(cut) => &path[..cut],
        None => path,
    }
}

/// Whether `a` and `b` are one name to Windows, which ignores case.
fn same_name(a: &str, b: &str) -> bool {
    a == b || (a.chars().flat_map(char::to_lowercase)).eq(b.chars().flat_map(char::to_lowercase))
}

/// An operation's state, as the window reads it.
#[derive(Debug, Clone)]
pub enum State {
    Preparing,
    Cancelling,
    /// Prepared for submission with its accepted policy.
    Prepared(Prepared),
    /// Observes the preview without keeping its potentially large tree alive.
    Compared(Weak<crate::Comparison>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningPhase {
    Comparing,
    Synchronizing,
    Executing,
}

/// A running operation's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub phase: RunningPhase,
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
    /// The person's Pause; Resume clears only this flag.
    pub paused: bool,
    /// Priority admission holds the run until the queue readmits it.
    pub yielded: bool,
    /// The file most recently started; `None` before the first.
    pub current: Option<Current>,
}

impl Running {
    pub fn is_active(&self) -> bool {
        !self.paused && !self.yielded
    }
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
}

/// The accepted selection and its indexed progress total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// The selected entries the operation acts on.
    pub entries: usize,
    /// For a copy, a move or a delete, from gozlerd's index; `None` when
    /// gozlerd cannot answer exactly, and then the progress has no total.
    pub total: Option<Total>,
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

/// Why a submitted transfer waits for admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// A required physical device has reached its transfer limit.
    Disk,
}

/// How an operation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    Complete,
    Cancelled,
    /// The destination disk filled, which would fail every later file too.
    DiskFull(NoRoom),
    Refused(Refusal),
}

/// The first file that found the destination disk full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoRoom {
    /// As the window gave it.
    pub path: PathBuf,
    pub size: u64,
    /// The destination's free space after the copy stopped, or `None` when
    /// it cannot be read. It can be more than `size` when files copied at
    /// the same time filled the disk, because the copy deletes what it wrote
    /// of the files it stops.
    pub free: Option<u64>,
}

/// What an operation refuses at prepare, or a copy at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the synchronization roots are identical or nested")]
    SynchronizationRootsOverlap,
    #[error("the folders changed since comparison")]
    SynchronizationChanged,
    #[error("file and folder names conflict between the synchronization roots")]
    SynchronizationConflicts,
    #[error("the folders cannot be compared: Windows error {0}")]
    ComparisonFailed(u32),
    #[error("the clipboard or dropped files are no longer available")]
    SourceUnavailable,
    #[error("the operation's recovery state cannot be saved: Windows error {0}")]
    RecoveryUnavailable(u32),
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
    SynchronizationChanged,
    /// With Windows' error code.
    Failed(u32),
    RecoveryFailed(u32),
    CleanupFailed(u32),
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
    /// Successfully processed entries, computed before publishing the outcome
    /// so presentation never scans a large report to count them.
    pub completed: u64,
    /// All source entries were processed, even if recovery cleanup failed.
    pub succeeded: bool,
    pub bytes: u64,
    /// In the order they happened.
    pub listed: Vec<Listed>,
    pub ring: RingCounts,
}

impl Outcome {
    pub fn new(end: End, items: u64, bytes: u64, listed: Vec<Listed>, ring: RingCounts) -> Outcome {
        let failed = listed
            .iter()
            .filter(|entry| {
                !matches!(
                    entry.reason,
                    Reason::RecoveryFailed(_) | Reason::CleanupFailed(_)
                )
            })
            .count() as u64;
        Outcome {
            succeeded: end == End::Complete && failed == 0,
            end,
            items,
            completed: items.saturating_sub(failed),
            bytes,
            listed,
            ring,
        }
    }

    pub(crate) fn ended(end: End) -> Outcome {
        Outcome {
            end,
            items: 0,
            completed: 0,
            succeeded: false,
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
/// ring between blocks, `CopyFile2` after its native paused return, and the
/// workers between entries.
#[derive(Default)]
pub(crate) struct Pause {
    /// Read on every block, chunk and entry; `state` decides.
    paused: AtomicBool,
    state: Mutex<PauseState>,
    resumed: Condvar,
    /// The engine threads waiting now, which a test waits for.
    #[cfg(test)]
    pub(crate) waiting: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct PauseState {
    manual: bool,
    yielded: bool,
    active: HashMap<std::thread::ThreadId, bool>,
    quiet: Option<Box<dyn FnOnce() + Send>>,
}

impl PauseState {
    fn take_quiet(&mut self) -> Option<Box<dyn FnOnce() + Send>> {
        if self.yielded && !self.active.values().any(|&active| active) {
            self.quiet.take()
        } else {
            None
        }
    }
}

pub(crate) struct Activity<'a>(&'a Pause);

impl Drop for Activity<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("no engine thread panics");
        state.active.remove(&std::thread::current().id());
        let quiet = state.take_quiet();
        drop(state);
        self.0.resumed.notify_all();
        if let Some(quiet) = quiet {
            quiet();
        }
    }
}

impl Pause {
    pub(crate) fn set(&self, paused: bool) {
        let mut state = self.state.lock().expect("no engine thread panics");
        state.manual = paused;
        self.paused
            .store(state.manual || state.yielded, Ordering::Relaxed);
        self.resumed.notify_all();
    }

    pub(crate) fn yield_to_priority(&self, yielded: bool) {
        let mut state = self.state.lock().expect("no engine thread panics");
        state.yielded = yielded;
        if !yielded {
            state.quiet = None;
        }
        self.paused
            .store(state.manual || state.yielded, Ordering::Relaxed);
        self.resumed.notify_all();
    }

    /// Tracks a thread only while it can change files, excluding idle pool
    /// and ring workers so priority admission can observe quiescence.
    pub(crate) fn enter(&self, cancel: &AtomicBool) -> Activity<'_> {
        let mut state = self.state.lock().expect("no engine thread panics");
        #[cfg(test)]
        let held = state.yielded || (state.manual && !cancel.load(Ordering::Relaxed));
        #[cfg(test)]
        if held {
            self.waiting.fetch_add(1, Ordering::Relaxed);
        }
        while state.yielded || (state.manual && !cancel.load(Ordering::Relaxed)) {
            state = self.resumed.wait(state).expect("no engine thread panics");
        }
        #[cfg(test)]
        if held {
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        }
        let previous = state.active.insert(std::thread::current().id(), true);
        assert!(previous.is_none(), "an engine thread enters its pause once");
        Activity(self)
    }

    /// Waiting for shared initialization uses no device. The caller
    /// reacquires its gate before returning.
    pub(crate) fn idle<T>(&self, cancel: &AtomicBool, work: impl FnOnce() -> T) -> T {
        let thread = std::thread::current().id();
        let quiet = {
            let mut state = self.state.lock().expect("no engine thread panics");
            let active = state
                .active
                .get_mut(&thread)
                .expect("an activity surrounds the idle wait");
            *active = false;
            state.take_quiet()
        };
        if let Some(quiet) = quiet {
            quiet();
        }
        let result = work();
        let mut state = self.state.lock().expect("no engine thread panics");
        while state.yielded || (state.manual && !cancel.load(Ordering::Relaxed)) {
            state = self.resumed.wait(state).expect("no engine thread panics");
        }
        *state
            .active
            .get_mut(&thread)
            .expect("the surrounding activity is still live") = true;
        result
    }

    /// Calls once outside the lock when the closed gate has no active worker.
    pub(crate) fn on_quiet(&self, quiet: impl FnOnce() + Send + 'static) {
        let quiet = {
            let mut state = self.state.lock().expect("no engine thread panics");
            if !state.yielded {
                return;
            }
            state.quiet = Some(Box::new(quiet));
            state.take_quiet()
        };
        if let Some(quiet) = quiet {
            quiet();
        }
    }

    pub(crate) fn is_quiet(&self) -> bool {
        let state = self.state.lock().expect("no engine thread panics");
        state.yielded && !state.active.values().any(|&active| active)
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Manual Pause and the priority gate, read under one lock.
    pub(crate) fn snapshot(&self) -> (bool, bool) {
        let state = self.state.lock().expect("no engine thread panics");
        (state.manual, state.yielded)
    }

    /// Wakes every waiting engine thread, after `cancel` was set, so it sees
    /// the cancel.
    pub(crate) fn wake(&self) {
        let _state = self.state.lock().expect("no engine thread panics");
        self.resumed.notify_all();
    }

    /// Cancellation bypasses manual Pause. A priority yield holds cleanup
    /// too, until the queue readmits the operation's disk plan.
    pub(crate) fn wait(&self, cancel: &AtomicBool) {
        if !self.is_paused() {
            return;
        }
        let mut state = self.state.lock().expect("no engine thread panics");
        let thread = std::thread::current().id();
        if let Some(active) = state.active.get_mut(&thread) {
            *active = false;
            self.resumed.notify_all();
        }
        if let Some(quiet) = state.take_quiet() {
            drop(state);
            quiet();
            state = self.state.lock().expect("no engine thread panics");
        }
        #[cfg(test)]
        self.waiting.fetch_add(1, Ordering::Relaxed);
        while state.yielded || (state.manual && !cancel.load(Ordering::Relaxed)) {
            state = self.resumed.wait(state).expect("no engine thread panics");
        }
        if let Some(active) = state.active.get_mut(&thread) {
            *active = true;
        }
        #[cfg(test)]
        self.waiting.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The initializer keeps its device grant until publication; consumers
/// waiting for it are idle and reacquire their gate before returning.
#[derive(Debug)]
pub(crate) struct Initialization<T> {
    ready: OnceLock<T>,
    initializing: Mutex<()>,
}

impl<T> Default for Initialization<T> {
    fn default() -> Self {
        Self {
            ready: OnceLock::new(),
            initializing: Mutex::new(()),
        }
    }
}

impl<T: Clone> Initialization<T> {
    pub(crate) fn get_or_init(
        &self,
        pause: &Pause,
        cancel: &AtomicBool,
        initialize: impl FnOnce() -> T,
    ) -> T {
        if let Some(ready) = self.ready.get() {
            return ready.clone();
        }
        match self.initializing.try_lock() {
            Ok(_initializing) => self.ready.get_or_init(initialize).clone(),
            Err(TryLockError::WouldBlock) => pause.idle(cancel, || {
                let _initializing = self.initializing.lock().expect("no initializer panics");
                self.ready
                    .get()
                    .expect("the initializer publishes before unlocking")
                    .clone()
            }),
            Err(TryLockError::Poisoned(_)) => panic!("no initializer panics"),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_cannot_enter_a_device_while_its_grant_is_yielded() {
        let pause = Pause::default();
        let cancel = AtomicBool::new(true);
        let readmitted = AtomicBool::new(false);
        pause.yield_to_priority(true);
        let observed = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let _active = pause.enter(&cancel);
                readmitted.load(Ordering::Relaxed)
            });
            while pause.waiting.load(Ordering::Relaxed) == 0 && !worker.is_finished() {
                std::thread::yield_now();
            }
            readmitted.store(true, Ordering::Relaxed);
            pause.yield_to_priority(false);
            worker.join().expect("cleanup is readmitted")
        });
        assert!(observed);
    }

    #[test]
    fn priority_yield_waits_for_quiescence_and_preserves_manual_pause() {
        let pause = Pause::default();
        let cancel = AtomicBool::new(false);
        let (entered, active) = std::sync::mpsc::channel();
        let (release, ready) = std::sync::mpsc::channel();
        let pause = &pause;
        let cancel = &cancel;
        let (was_active, was_quiet, stayed_paused) = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let _active = pause.enter(cancel);
                entered.send(()).expect("the test receives entry");
                ready.recv().expect("the test releases the worker");
                pause.wait(cancel);
            });
            active.recv().expect("the worker enters");
            pause.yield_to_priority(true);
            let was_active = !pause.is_quiet();
            let (quiet, noticed) = std::sync::mpsc::channel();
            pause.on_quiet(move || quiet.send(()).expect("the test receives quiescence"));
            assert!(noticed.try_recv().is_err());
            release.send(()).expect("the worker waits for release");
            noticed.recv().expect("the worker becomes quiet");
            let was_quiet = pause.is_quiet();
            pause.set(true);
            pause.yield_to_priority(false);
            let stayed_paused = pause.is_paused();
            pause.set(false);
            worker.join().expect("the worker resumes");
            (was_active, was_quiet, stayed_paused)
        });
        assert!(was_active);
        assert!(was_quiet);
        assert!(stayed_paused);
    }

    #[test]
    fn shared_initialization_keeps_its_grant_and_waiters_need_readmission() {
        let initialization = Initialization::default();
        let initializer = Pause::default();
        let consumer = Pause::default();
        let cancel = AtomicBool::new(false);
        let (entered, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (returned, result) = std::sync::mpsc::channel();
        let (entered_consumer, consuming) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let initialization = &initialization;
            let initializer = &initializer;
            let consumer = &consumer;
            let cancel = &cancel;
            let first = scope.spawn(move || {
                let _active = initializer.enter(cancel);
                initialization.get_or_init(initializer, cancel, || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    7
                })
            });
            started.recv().unwrap();
            let second = scope.spawn(move || {
                let _active = consumer.enter(cancel);
                entered_consumer.send(()).unwrap();
                let value = initialization.get_or_init(consumer, cancel, || {
                    panic!("the first worker initializes once")
                });
                returned.send(value).unwrap();
            });
            consuming.recv().unwrap();
            consumer.yield_to_priority(true);
            let (quiet, noticed) = std::sync::mpsc::channel();
            consumer.on_quiet(move || quiet.send(()).unwrap());
            noticed.recv().unwrap();
            initializer.yield_to_priority(true);
            let (quiet, published) = std::sync::mpsc::channel();
            initializer.on_quiet(move || quiet.send(()).unwrap());
            assert!(published.try_recv().is_err());
            release.send(()).unwrap();
            published.recv().unwrap();
            assert!(result.try_recv().is_err());
            consumer.yield_to_priority(false);
            assert_eq!(result.recv().unwrap(), 7);
            assert_eq!(first.join().unwrap(), 7);
            second.join().unwrap();
        });
    }

    #[test]
    fn the_sources_folder_holds_every_source() {
        assert_eq!(
            common_folder([r"C:\Photos\2026\a.jpg", r"C:\photos\2025\b.jpg"]).as_deref(),
            Some(r"C:\Photos")
        );
        assert_eq!(
            common_folder([r"C:\a.txt", r"C:\b\c.txt"]).as_deref(),
            Some(r"C:\")
        );
        assert_eq!(common_folder([r"C:\a.txt", r"D:\b.txt"]), None);
        assert_eq!(
            common_folder([r"\\one\share\a.txt", r"\\two\share\b.txt"]),
            None
        );
    }
}
