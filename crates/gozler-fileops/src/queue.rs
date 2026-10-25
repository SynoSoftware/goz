//! The engine's queue owns operation lifetimes, volume-pair device admission,
//! recovery and every engine thread. Waiting transfers hold no device grant.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;

use gozler_winfs::{Access, Existing, Reparse, create_folder, open_entry, rename, system_time};

use crate::conflicts::{Policy, Rule, Run};
use crate::copy::{Copied, Copying, copy, is_folder_path};
use crate::delete::walk;
use crate::io_threads::IoThreads;
use crate::listing::{BACKSLASH, LISTING_WORDS, join, read_names};
use crate::moves::{Moving, move_by_rename};
use crate::names::Names;
use crate::operation::{
    Current, CurrentFile, End, Listed, Operation, OperationId, OrderRefused, Outcome, Pause,
    Progress, Refusal, Request, RingCounts, Running, SourcesFolder, State, Total, Wait,
};
use crate::place::Target;
use crate::prepare::{Preparation, Ready, Transfer, Work, check_space, prepare};
use crate::receive::{failure, plain};
use crate::saved::{self, Restored, Saved};
use crate::settings::{SpeedSettings, TransferLimit};
use crate::taken::Taken;
use crate::totals::{Counting, Gozlerd, Index};
use crate::waits::{Plan, TransferId, wait, waits};
use crate::win32::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND};

/// `ERROR_INVALID_NAME`: a new name that is empty or holds a separator,
/// which would move the entry instead of renaming it.
const ERROR_INVALID_NAME: u32 = 123;

/// The queue the window talks to. Dropping it cancels every operation and
/// joins every thread, so no engine thread outlives it.
pub struct Queue {
    shared: Arc<Shared>,
}

struct Shared {
    next_id: AtomicU64,
    operations: Mutex<Operations>,
    /// Called after each change of an operation's state, never under the
    /// queue's lock, so it may read the states at once.
    wake: Box<dyn Fn() + Send + Sync>,
    /// Where prepare reads the totals.
    index: Box<dyn Index>,
    saved_path: PathBuf,
    /// The operations of the saved file. A write holds its lock only to
    /// copy them, so a change on the window's call never waits for a write.
    saved: Mutex<SavedOperations>,
    /// Held while the saved file is written, which orders the writes, so
    /// an older copy is never written after a newer one.
    writing: Mutex<()>,
    /// Set when the queue closes or is dropped: a run that it cancels stays
    /// saved, so it is offered as interrupted at the next start, which is
    /// the truth, and keeps the files it was writing for Continue to resume.
    closing: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// The operations of the saved file.
struct SavedOperations {
    by_id: BTreeMap<OperationId, Arc<Recovery>>,
    /// Whether the file holds `by_id`. A change on the window's call is
    /// written on a queue thread, because the file lists every source of
    /// every saved operation.
    written: bool,
}

enum Recovery {
    Saved(Saved),
    Request {
        request: Arc<Request>,
        started: i64,
        settings: SpeedSettings,
        saved: OnceLock<Option<Saved>>,
    },
}

impl Recovery {
    fn saved(&self) -> Option<&Saved> {
        match self {
            Self::Saved(saved) => Some(saved),
            Self::Request {
                request,
                started,
                settings,
                saved,
            } => saved
                .get_or_init(|| Saved::new(request, *started, settings))
                .as_ref(),
        }
    }
}

#[derive(Default)]
struct Operations {
    limit: TransferLimit,
    next_order: u64,
    next_start: u64,
    by_id: BTreeMap<OperationId, Tracked>,
    taken: Arc<Taken>,
    /// Counts the reads of the operations that rebuild `taken`
    /// ([`Shared::retake`]).
    taken_reads: u64,
    /// The read that `taken` was built from, so an older rebuild that ends
    /// last does not replace a newer one.
    taken_from: u64,
    priorities: Vec<Priority>,
}

struct Priority {
    id: OperationId,
    displaced: Vec<TransferId>,
}

struct Tracked {
    request: Arc<Request>,
    folder: SourcesFolder,
    settings: SpeedSettings,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    paused: bool,
    progress: Arc<Progress>,
    io: Arc<IoThreads>,
    /// Continue's prepare: it submits with the interrupted run's choice and
    /// start as soon as it is prepared.
    continuing: Option<Rule>,
    /// Explicit Sync/Mirror consent from the destination dialog; recovery
    /// requires a fresh preview instead of retaining this consent.
    synchronize: Option<crate::Destination>,
    /// It was submitted, so its sources are taken while it lives, and not
    /// only while it prepares.
    submitted: bool,
    /// Prepare's total, from submit on.
    total: Option<Total>,
    /// [`Running::renames`], from submit on.
    renames: bool,
}

enum Phase {
    Receiving,
    Cancelling,
    Preparing,
    Prepared(Box<Ready>),
    Compared {
        review: Box<crate::sync::windows::Review>,
        plan: Plan,
    },
    Queued(Scheduled),
    /// An ended operation holds no plan, so it makes nothing wait.
    Ended(Arc<Outcome>),
    Interrupted(Restored),
}

struct Scheduled {
    phase: crate::RunningPhase,
    parts: Vec<Part>,
    order: u64,
    started: Option<u64>,
    rule: Rule,
    completion: Arc<Mutex<Completion>>,
}

struct Part {
    plan: Plan,
    pause: Arc<Pause>,
    state: PartState,
    on: Option<(OperationId, Wait)>,
}

enum PartState {
    Ready(Box<Start>),
    Admitted,
    Waiting,
    Settling,
    Ended,
}

struct Completion {
    remaining: usize,
    listed: Vec<Listed>,
    end: End,
    ring: RingCounts,
}

struct Admitted {
    id: TransferId,
    start: Box<Start>,
    pause: Arc<Pause>,
    completion: Arc<Mutex<Completion>>,
}

/// What a run takes to its thread.
struct Start {
    work: Work,
    failures: Vec<Listed>,
    choice: Option<Policy>,
    run: Run,
}

/// A submit for an operation that is not prepared: it does not exist, or
/// it is still preparing, already running or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operation is not prepared")]
pub struct NotPrepared;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the comparison is not ready to synchronize")]
pub struct NotCompared;

/// Continue or Dismiss for an operation that is not interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operation is not interrupted")]
pub struct NotInterrupted;

/// An allocated operation id, consumed once when its command reaches the queue.
pub struct ReservedId(OperationId);

impl ReservedId {
    pub fn id(&self) -> OperationId {
        self.0
    }
}

impl Queue {
    /// `saved` is the file where operations are saved; each operation saved
    /// there becomes an interrupted one. `wake` is called on an engine
    /// thread after each change of an operation's state. Prepare reads its
    /// totals from the installed gozlerd.
    pub fn new(saved: impl Into<PathBuf>, wake: impl Fn() + Send + Sync + 'static) -> Queue {
        Queue::with_index(saved, Box::new(Gozlerd), wake)
    }

    /// A queue whose prepare reads its totals from `index`.
    pub(crate) fn with_index(
        saved: impl Into<PathBuf>,
        index: Box<dyn Index>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Queue {
        let saved_path = saved.into();
        let mut operations = Operations::default();
        let mut saved_operations = BTreeMap::new();
        let mut requests = Vec::new();
        for (saved, restored) in saved::read(&saved_path) {
            let id = OperationId(operations.by_id.len() as u64);
            let request = Arc::new(restored.request.clone());
            requests.push((id, Arc::clone(&request)));
            let operation = Tracked::new(request, restored.settings, Phase::Interrupted(restored));
            operations.by_id.insert(id, operation);
            saved_operations.insert(id, Arc::new(Recovery::Saved(saved)));
        }
        let next_id = AtomicU64::new(operations.by_id.len() as u64);
        let queue = Queue {
            shared: Arc::new(Shared {
                next_id,
                operations: Mutex::new(operations),
                wake: Box::new(wake),
                index,
                saved_path,
                saved: Mutex::new(SavedOperations {
                    by_id: saved_operations,
                    written: true,
                }),
                writing: Mutex::new(()),
                closing: AtomicBool::new(false),
                threads: Mutex::default(),
            }),
        };
        if !requests.is_empty() {
            let thread = Arc::clone(&queue.shared);
            queue.shared.spawn(move || {
                for (id, request) in requests {
                    thread.set_folder(id, &request);
                }
            });
        }
        queue
    }

    /// Starts preparing `request` on a queue thread and returns its id at
    /// once.
    pub fn prepare(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let reserved = self.reserve_id();
        let id = reserved.id();
        self.prepare_reserved(reserved, request, settings);
        id
    }

    /// Accepts a command before asynchronous source intake. [`Queue::resolve`]
    /// completes that same operation; cancellation also covers this interval.
    pub fn accept(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let reserved = self.reserve_id();
        let id = reserved.id();
        self.accept_reserved(reserved, request, settings);
        id
    }

    /// Allocates an id without taking the queue lock. The returned token must
    /// reach this queue through [`Queue::accept_reserved`] or
    /// [`Queue::prepare_reserved`], in the command's original order.
    pub fn reserve_id(&self) -> ReservedId {
        #[allow(deprecated, reason = "try_update requires Rust 1.95; MSRV is 1.94")]
        let id = self
            .shared
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .expect("operation ids cannot wrap");
        ReservedId(OperationId(id))
    }

    pub fn prepare_reserved(
        &self,
        reserved: ReservedId,
        request: Request,
        settings: SpeedSettings,
    ) {
        let id = reserved.id();
        self.accept_reserved(reserved, request, settings);
        Shared::prepare(&self.shared, id);
    }

    /// Starts an explicitly chosen Sync or Mirror without a preview. The
    /// destination follows Copy's folder/new-name rules. Comparison and all
    /// Mirror safeguards still run; conflicts refuse the operation.
    pub fn synchronize_reserved(
        &self,
        reserved: ReservedId,
        source: PathBuf,
        destination: crate::Destination,
        mirror: bool,
        settings: SpeedSettings,
    ) {
        let id = reserved.id();
        let request = Request::Synchronize {
            source,
            destination: destination.path(),
            mirror,
        };
        self.accept_reserved(reserved, request, settings);
        if let Some(operation) = self.shared.lock().by_id.get_mut(&id) {
            operation.synchronize = Some(destination);
        }
        Shared::prepare(&self.shared, id);
    }

    pub fn accept_reserved(&self, reserved: ReservedId, request: Request, settings: SpeedSettings) {
        let id = reserved.id();
        {
            let mut operations = self.shared.lock();
            let operation = Tracked::new(Arc::new(request), settings, Phase::Receiving);
            let old = operations.by_id.insert(id, operation);
            assert!(
                old.is_none(),
                "a reserved id is inserted once into its queue"
            );
        }
        (self.shared.wake)();
    }

    /// Completes intake only while the accepted command still wants its reply.
    pub fn resolve(&self, id: OperationId, mut request: Request) -> bool {
        {
            let mut operations = self.shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return false;
            };
            if !matches!(operation.phase, Phase::Receiving) {
                return false;
            }
            if let Some(accepted) = operation.request.policy() {
                match &mut request {
                    Request::Copy { policy, .. } | Request::Move { policy, .. } => {
                        *policy = accepted
                    }
                    Request::Delete { .. }
                    | Request::NewFolder { .. }
                    | Request::Synchronize { .. } => return false,
                }
            }
            operation.request = Arc::new(request);
        }
        Shared::prepare(&self.shared, id);
        true
    }

    /// Ends unavailable source intake without changing an already cancelled command.
    pub fn fail_intake(&self, id: OperationId) {
        {
            let mut operations = self.shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return;
            };
            if !matches!(operation.phase, Phase::Receiving) {
                return;
            }
            operation.phase = Phase::Ended(Arc::new(Outcome::ended(End::Refused(
                Refusal::SourceUnavailable,
            ))));
        }
        (self.shared.wake)();
    }

    /// Submits the prepared operation with its accepted policy.
    pub fn submit(&self, id: OperationId) -> Result<(), NotPrepared> {
        let starts = {
            let mut operations = self.shared.lock();
            let operation = operations.by_id.get_mut(&id).ok_or(NotPrepared)?;
            let Phase::Prepared(_) = operation.phase else {
                return Err(NotPrepared);
            };
            let Phase::Prepared(ready) = std::mem::replace(&mut operation.phase, Phase::Preparing)
            else {
                unreachable!("the phase was just matched");
            };
            self.shared.enqueue(&mut operations, id, *ready)
        };
        self.shared.start(starts);
        (self.shared.wake)();
        Ok(())
    }

    /// Commits the comparison retained on this id and reacquires its devices.
    pub fn synchronize(&self, id: OperationId) -> Result<(), NotCompared> {
        let starts = {
            let mut operations = self.shared.lock();
            self.shared.commit_comparison(&mut operations, id)?
        };
        self.shared.start(starts);
        (self.shared.wake)();
        Ok(())
    }

    /// Releases a potentially large preview on the queue's worker, including
    /// when cancellation already released the engine's reference.
    pub fn release_comparison(&self, comparison: Arc<crate::Comparison>) {
        self.shared.spawn(move || drop(comparison));
    }

    /// Cancels an operation. One that has not started ends at once; a
    /// running one stops within one `CopyFile2` chunk, or one delete and its
    /// retries, also while it is paused.
    pub fn cancel(&self, id: OperationId) {
        self.cancel_matching(|other, operation| {
            other == id && !matches!(operation.phase, Phase::Interrupted(_))
        });
    }

    /// Cancels all accepted transfers in one transition, before any
    /// completion can admit another one. Independent deletes keep running.
    pub fn cancel_all(&self) {
        self.cancel_matching(|_, operation| {
            operation.request.is_transfer() && !matches!(operation.phase, Phase::Interrupted(_))
        });
    }

    fn cancel_matching(&self, matches: impl Fn(OperationId, &Tracked) -> bool) {
        let (starts, left, cancelled, ids) = {
            let mut operations = self.shared.lock();
            let mut left = Vec::new();
            let mut cancelled = Vec::new();
            let mut ids = Vec::new();
            for (&id, operation) in &mut operations.by_id {
                if !matches(id, operation)
                    || matches!(operation.phase, Phase::Ended(_) | Phase::Cancelling)
                {
                    continue;
                }
                operation.cancel.store(true, Ordering::Relaxed);
                operation.io.cancel();
                if let Phase::Queued(run) = &mut operation.phase {
                    for (index, part) in run.parts.iter_mut().enumerate() {
                        part.pause.wake();
                        if matches!(part.state, PartState::Ready(_)) {
                            let PartState::Ready(start) =
                                std::mem::replace(&mut part.state, PartState::Settling)
                            else {
                                unreachable!("the part was ready")
                            };
                            cancelled.push(Admitted {
                                id: TransferId {
                                    operation: id,
                                    index,
                                },
                                start,
                                pause: Arc::clone(&part.pause),
                                completion: Arc::clone(&run.completion),
                            });
                        }
                    }
                } else {
                    left.push(std::mem::replace(&mut operation.phase, Phase::Cancelling));
                }
                ids.push(id);
            }
            (self.shared.schedule(&mut operations), left, cancelled, ids)
        };
        let removed = {
            let mut saved = self.shared.saved.lock().expect("no queue thread panics");
            let mut removed = Vec::new();
            for id in &ids {
                if let Some(recovery) = saved.by_id.remove(id) {
                    saved.written = false;
                    removed.push(recovery);
                }
            }
            removed
        };
        let shared = Arc::clone(&self.shared);
        self.shared.spawn(move || {
            drop(left);
            drop(removed);
            let written = shared.write_saved();
            {
                let mut operations = shared.lock();
                for id in ids {
                    if let Some(operation) = operations.by_id.get_mut(&id)
                        && matches!(operation.phase, Phase::Cancelling)
                    {
                        let mut outcome = Outcome::ended(End::Cancelled);
                        if let Err(error) = &written {
                            outcome.listed.push(Listed {
                                path: shared.saved_path.clone(),
                                reason: crate::operation::Reason::RecoveryFailed(
                                    error.raw_os_error().unwrap_or(29) as u32,
                                ),
                            });
                        }
                        operation.phase = Phase::Ended(Arc::new(outcome));
                    }
                }
            }
            for part in cancelled {
                drop(part.start);
                shared.finish_part(part.id, Outcome::ended(End::Cancelled), part.completion);
            }
            shared.retake();
            (shared.wake)();
        });
        self.shared.start(starts);
        (self.shared.wake)();
    }

    /// Removes an ended operation, once no list of the window keeps it.
    pub fn remove(&self, id: OperationId) {
        let removed = {
            let mut operations = self.shared.lock();
            match operations.by_id.get(&id) {
                Some(Tracked {
                    phase: Phase::Ended(_),
                    ..
                }) => operations.by_id.remove(&id),
                _ => None,
            }
        };
        if let Some(removed) = removed {
            Shared::retake_later(&self.shared, removed);
        }
    }

    /// Continue: prepares and submits an interrupted operation again, with
    /// the settings and the choice it ran with. A file already at its final
    /// name, or for Keep both at one of its numbered names, with the
    /// source's size and last write time is done; one there that is not
    /// done and was created since the operation first ran is resumed.
    pub fn resume(&self, id: OperationId) -> Result<(), NotInterrupted> {
        {
            let mut operations = self.shared.lock();
            let operation = operations.by_id.get_mut(&id).ok_or(NotInterrupted)?;
            let Phase::Interrupted(restored) = &operation.phase else {
                return Err(NotInterrupted);
            };
            operation.continuing = Some(Rule {
                choice: restored.request.policy(),
                run: if matches!(restored.request, Request::Synchronize { .. }) {
                    Run::First { started: None }
                } else {
                    Run::Continue {
                        started: restored.started,
                    }
                },
            });
            operation.phase = Phase::Preparing;
        }
        Shared::prepare(&self.shared, id);
        (self.shared.wake)();
        Ok(())
    }

    /// Dismiss: removes an interrupted operation and its saved entry.
    pub fn dismiss(&self, id: OperationId) -> Result<(), NotInterrupted> {
        {
            let operations = self.shared.lock();
            let Some(Tracked {
                phase: Phase::Interrupted(_),
                ..
            }) = operations.by_id.get(&id)
            else {
                return Err(NotInterrupted);
            };
        }
        self.cancel_matching(|other, _| other == id);
        Ok(())
    }

    /// Every operation: running priority work first, then other running
    /// work in start order, waiting work in run order, and prepared work.
    pub fn operations(&self) -> Vec<Operation> {
        let operations = self.shared.lock();
        let mut listed: Vec<((u8, u64), Operation)> = (operations.by_id.iter())
            .map(|(&id, operation)| snapshot(id, operation, &operations))
            .collect();
        listed.sort_unstable_by_key(|&(place, _)| place);
        listed.into_iter().map(|(_, operation)| operation).collect()
    }

    pub fn operation(&self, id: OperationId) -> Option<Operation> {
        let operations = self.shared.lock();
        operations
            .by_id
            .get(&id)
            .map(|operation| snapshot(id, operation, &operations).1)
    }

    /// Pause releases each transfer's grant only after its backend is quiet.
    /// Resume requests admission before any worker can change files again.
    pub fn set_paused(&self, id: OperationId, paused: bool) {
        let (starts, held) = {
            let mut operations = self.shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return;
            };
            let Phase::Queued(run) = &operation.phase else {
                return;
            };
            operation.paused = paused;
            let held = run
                .parts
                .iter()
                .enumerate()
                .filter(|(_, part)| paused && matches!(part.state, PartState::Admitted))
                .map(|(index, part)| {
                    (
                        TransferId {
                            operation: id,
                            index,
                        },
                        Arc::clone(&part.pause),
                    )
                })
                .collect();
            (self.shared.schedule(&mut operations), held)
        };
        self.shared.start(starts);
        self.shared.quiesce(held);
        (self.shared.wake)();
    }

    pub fn set_transfer_limit(&self, limit: TransferLimit) {
        let starts = {
            let mut operations = self.shared.lock();
            operations.limit = limit;
            self.shared.schedule(&mut operations)
        };
        self.shared.start(starts);
        (self.shared.wake)();
    }

    /// Moves waiting work in priority order. Passing executing work yields
    /// only transfers that share its devices; unrelated work continues.
    pub fn move_waiting(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<(), OrderRefused> {
        let (starts, held) = {
            let mut operations = self.shared.lock();
            let priority = operations.priority(id, before)?;
            let order = if priority.is_some() {
                let first = operations.waiting().first().copied();
                operations.reordered(id, first)?
            } else {
                operations.reordered(id, before)?
            };
            for (place, id) in order.into_iter().enumerate() {
                if let Phase::Queued(run) = &mut operations
                    .by_id
                    .get_mut(&id)
                    .expect("listed as queued")
                    .phase
                {
                    run.order = place as u64;
                }
            }
            if let Some(displaced) = priority {
                operations.priorities.push(Priority { id, displaced });
            }
            let starts = self.shared.schedule(&mut operations);
            let held = operations
                .priorities
                .iter()
                .flat_map(|p| &p.displaced)
                .filter_map(|&id| {
                    let operation = operations.by_id.get(&id.operation)?;
                    let Phase::Queued(run) = &operation.phase else {
                        return None;
                    };
                    let part = &run.parts[id.index];
                    matches!(part.state, PartState::Admitted).then(|| (id, Arc::clone(&part.pause)))
                })
                .collect();
            (starts, held)
        };
        self.shared.start(starts);
        self.shared.quiesce(held);
        (self.shared.wake)();
        Ok(())
    }

    /// Whether [`Queue::move_waiting`] would move `id`, without moving it.
    pub fn check_move(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<(), OrderRefused> {
        let operations = self.shared.lock();
        if operations.priority(id, before)?.is_some() {
            Ok(())
        } else {
            operations.reordered(id, before).map(drop)
        }
    }

    /// The taken entries, which the window leaves out of its lists. Take
    /// it once per wake. It follows a change of the operations a moment
    /// later, with a wake of its own, so an operation can show as ended or
    /// be gone while its sources are still taken as before. An ended
    /// operation stays in it until [`Queue::remove`].
    pub fn taken(&self) -> Arc<Taken> {
        Arc::clone(&self.shared.lock().taken)
    }

    /// Exit: stops every operation, a paused one too, so that Continue can
    /// finish it at the next start, and returns without waiting for them to
    /// stop; the window then ends the process without dropping the queue.
    /// `CopyFile2` deletes what it wrote when it is stopped, so a file the
    /// Windows engine is copying goes on until the process ends, which
    /// leaves it at its final name.
    /// The caller keeps the process alive until the returned receiver reports
    /// that recovery state was written, including explicit cancellations.
    pub fn close(&self) -> Receiver<std::io::Result<()>> {
        self.shared.closing.store(true, Ordering::Relaxed);
        for operation in self.shared.lock().by_id.values() {
            operation.cancel.store(true, Ordering::Relaxed);
            if let Phase::Queued(run) = &operation.phase {
                for part in &run.parts {
                    part.pause.yield_to_priority(false);
                    part.pause.wake();
                }
            }
        }
        let shared = Arc::clone(&self.shared);
        let (done, finished) = mpsc::channel();
        self.shared.spawn(move || {
            let _ = done.send(shared.write_saved());
        });
        finished
    }
}

/// Stops every operation as [`Queue::close`] does, cancels the Windows calls
/// its threads wait in, and waits for each to end. A file the Windows engine
/// is copying is then finished, or lost where that cancel stops it.
impl Drop for Queue {
    fn drop(&mut self) {
        let _ = self.close();
        for operation in self.shared.lock().by_id.values() {
            operation.io.cancel();
        }
        loop {
            let thread = self
                .shared
                .threads
                .lock()
                .expect("no queue thread panics")
                .pop();
            match thread {
                Some(thread) => {
                    let _ = thread.join();
                }
                None => break,
            }
        }
    }
}

impl Scheduled {
    fn unfinished(&self) -> bool {
        self.parts
            .iter()
            .any(|part| !matches!(part.state, PartState::Ended))
    }

    fn waiting(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part.state, PartState::Ready(_) | PartState::Waiting))
    }
}

impl Operations {
    fn priority(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<Option<Vec<TransferId>>, OrderRefused> {
        let Some(before) = before.and_then(|before| self.by_id.get(&before)) else {
            return Ok(None);
        };
        if !matches!(&before.phase, Phase::Queued(run) if run.started.is_some()) {
            return Ok(None);
        }
        let transfer = |operation: &Tracked| operation.request.is_transfer();
        let operation = self.by_id.get(&id).ok_or(OrderRefused::NotWaiting)?;
        let Phase::Queued(chosen) = &operation.phase else {
            return Err(OrderRefused::NotWaiting);
        };
        if !chosen.waiting() || !transfer(operation) || !transfer(before) {
            return Err(OrderRefused::NotWaiting);
        }
        let mut displaced = Vec::new();
        for (&other, operation) in &self.by_id {
            if other == id || !transfer(operation) {
                continue;
            }
            if let Phase::Queued(run) = &operation.phase {
                for (index, part) in run.parts.iter().enumerate() {
                    if matches!(part.state, PartState::Admitted | PartState::Waiting)
                        && chosen.parts.iter().any(|chosen| {
                            !matches!(chosen.state, PartState::Ended)
                                && wait(&chosen.plan, &part.plan).is_some()
                        })
                    {
                        displaced.push(TransferId {
                            operation: other,
                            index,
                        });
                    }
                }
            }
        }
        Ok(Some(displaced))
    }

    fn waiting(&self) -> Vec<OperationId> {
        let mut waiting: Vec<_> = self
            .by_id
            .iter()
            .filter_map(|(&id, operation)| match &operation.phase {
                Phase::Queued(run) if run.waiting() => Some((run.order, id)),
                _ => None,
            })
            .collect();
        waiting.sort_unstable();
        waiting.into_iter().map(|(_, id)| id).collect()
    }

    fn reordered(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<Vec<OperationId>, OrderRefused> {
        let mut order = self.waiting();
        let from = order
            .iter()
            .position(|&other| other == id)
            .ok_or(OrderRefused::NotWaiting)?;
        if before == Some(id) {
            return Ok(order);
        }
        order.remove(from);
        let to = match before {
            Some(before) => order
                .iter()
                .position(|&id| id == before)
                .ok_or(OrderRefused::NotWaiting)?,
            None => order.len(),
        };
        order.insert(to, id);
        Ok(order)
    }
}

impl Tracked {
    fn new(request: Arc<Request>, settings: SpeedSettings, phase: Phase) -> Tracked {
        Tracked {
            request,
            folder: SourcesFolder::Pending,
            settings,
            phase,
            cancel: Arc::default(),
            paused: false,
            progress: Arc::default(),
            io: Arc::default(),
            continuing: None,
            synchronize: None,
            submitted: false,
            total: None,
            renames: false,
        }
    }
}

impl Shared {
    fn quiesce(self: &Arc<Self>, yielded: Vec<(TransferId, Arc<Pause>)>) {
        for (id, pause) in yielded {
            let shared = Arc::downgrade(self);
            let observed = Arc::downgrade(&pause);
            pause.on_quiet(move || {
                let (Some(shared), Some(pause)) = (shared.upgrade(), observed.upgrade()) else {
                    return;
                };
                let starts = {
                    let mut operations = shared.lock();
                    if pause.is_quiet()
                        && let Some(operation) = operations.by_id.get_mut(&id.operation)
                        && let Phase::Queued(run) = &mut operation.phase
                        && matches!(run.parts[id.index].state, PartState::Admitted)
                    {
                        run.parts[id.index].state = PartState::Waiting;
                    }
                    shared.schedule(&mut operations)
                };
                shared.start(starts);
                (shared.wake)();
            });
        }
    }

    fn lock(&self) -> MutexGuard<'_, Operations> {
        self.operations.lock().expect("no queue thread panics")
    }

    fn spawn(&self, thread: impl FnOnce() + Send + 'static) {
        let mut threads = self.threads.lock().expect("no queue thread panics");
        threads.retain(|thread| !thread.is_finished());
        threads.push(std::thread::spawn(thread));
    }

    /// Works out the folder of the sources of `id`, whose request is
    /// `request`, sets it, and wakes the window.
    fn set_folder(&self, id: OperationId, request: &Request) {
        let folder = SourcesFolder::of(request);
        if let Some(operation) = self.lock().by_id.get_mut(&id) {
            operation.folder = folder;
        }
        (self.wake)();
    }

    /// Prepares the operation `id`, which is preparing, on its own thread.
    fn prepare(shared: &Arc<Shared>, id: OperationId) {
        let (request, settings, continuing, synchronize, cancel, io) = {
            let mut operations = shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return;
            };
            if !matches!(operation.phase, Phase::Receiving | Phase::Preparing) {
                return;
            }
            operation.phase = Phase::Preparing;
            (
                Arc::clone(&operation.request),
                operation.settings,
                operation.continuing,
                operation.synchronize.clone(),
                Arc::clone(&operation.cancel),
                Arc::clone(&operation.io),
            )
        };
        let thread = Arc::clone(shared);
        shared.spawn(move || {
            // Its sources are taken while prepare opens each of them, one
            // open per entry on a share, so a deleted row goes at once. The
            // rebuild runs here, not on the window's thread.
            if matches!(*request, Request::Delete { .. } | Request::Move { .. }) {
                thread.retake();
                (thread.wake)();
            }
            thread.set_folder(id, &request);
            let entered = io.enter();
            let first = Rule {
                choice: request.policy(),
                run: Run::First { started: None },
            };
            let rule = continuing.unwrap_or(first);
            let preparing = Preparation {
                settings: &settings,
                rule,
                index: &*thread.index,
                cancel: &cancel,
                pause: None,
            };
            let prepared = match synchronize {
                Some(destination) => {
                    crate::prepare::synchronize(&request, &destination, &preparing)
                }
                None => prepare(&request, &preparing),
            };
            drop(entered);
            let starts = {
                let mut operations = thread.lock();
                let Some(operation) = operations.by_id.get_mut(&id) else {
                    return;
                };
                if !matches!(operation.phase, Phase::Preparing) {
                    return;
                }
                match prepared {
                    Err(refusal) => {
                        let mut outcome = Outcome::ended(End::Refused(refusal));
                        drop(operations);
                        // A refused Continue ends, as any operation does.
                        if let Err(error) = thread.unsave(id) {
                            outcome.listed.push(Listed {
                                path: thread.saved_path.clone(),
                                reason: crate::operation::Reason::RecoveryFailed(
                                    error.raw_os_error().unwrap_or(29) as u32,
                                ),
                            });
                        }
                        {
                            let mut operations = thread.lock();
                            if let Some(operation) = operations.by_id.get_mut(&id)
                                && matches!(operation.phase, Phase::Preparing)
                            {
                                operation.phase = Phase::Ended(Arc::new(outcome));
                            }
                        }
                        thread.retake();
                        (thread.wake)();
                        return;
                    }
                    Ok(mut ready) => {
                        // The decided destination stands for the asked one
                        // from here on, in the window's texts and the saved
                        // file.
                        if let Some(resolved) = ready.resolved.take() {
                            operation.request = Arc::new(resolved);
                        }
                        match continuing {
                            Some(_) => thread.enqueue(&mut operations, id, ready),
                            None => {
                                operation.phase = Phase::Prepared(Box::new(ready));
                                Vec::new()
                            }
                        }
                    }
                }
            };
            thread.start(starts);
            (thread.wake)();
        });
    }

    fn enqueue(&self, operations: &mut Operations, id: OperationId, ready: Ready) -> Vec<Admitted> {
        let order = operations.next_order;
        operations.next_order += 1;
        let operation = operations.by_id.get_mut(&id).expect("the caller found it");
        let choice = operation.request.policy();
        operation.submitted = true;
        operation.total = ready.prepared.total;
        operation.renames = ready.work.iter().all(|(_, work)| match work {
            Work::Transfer(transfer) => transfer.moving && transfer.copied.is_empty(),
            Work::Rename { .. } => true,
            Work::Delete { .. }
            | Work::NewFolder { .. }
            | Work::Compare(_)
            | Work::Synchronize(_) => false,
        });
        let rule = Rule {
            choice,
            run: operation
                .continuing
                .map_or(Run::First { started: None }, |rule| rule.run),
        };
        let completion = Arc::new(Mutex::new(Completion {
            remaining: ready.work.len(),
            listed: Vec::new(),
            end: End::Complete,
            ring: RingCounts::default(),
        }));
        let phase = if ready
            .work
            .iter()
            .any(|(_, work)| matches!(work, Work::Compare(_)))
        {
            crate::RunningPhase::Comparing
        } else if ready
            .work
            .iter()
            .any(|(_, work)| matches!(work, Work::Synchronize(_)))
        {
            crate::RunningPhase::Synchronizing
        } else {
            crate::RunningPhase::Executing
        };
        let mut failures = ready.failures;
        let parts = ready
            .work
            .into_iter()
            .map(|(plan, work)| Part {
                plan,
                pause: Arc::default(),
                state: PartState::Ready(Box::new(Start {
                    work,
                    failures: std::mem::take(&mut failures),
                    choice,
                    run: rule.run,
                })),
                on: None,
            })
            .collect();
        operation.phase = Phase::Queued(Scheduled {
            phase,
            parts,
            order,
            started: None,
            rule,
            completion,
        });
        self.schedule(operations)
    }

    fn schedule(&self, operations: &mut Operations) -> Vec<Admitted> {
        if self.closing.load(Ordering::Relaxed) {
            return Vec::new();
        }
        operations.priorities.retain(|priority| {
            operations.by_id.get(&priority.id).is_some_and(
                |operation| matches!(&operation.phase, Phase::Queued(run) if run.unfinished()),
            )
        });
        for (&id, operation) in &operations.by_id {
            if let Phase::Queued(run) = &operation.phase {
                for (index, part) in run.parts.iter().enumerate() {
                    let key = TransferId {
                        operation: id,
                        index,
                    };
                    let priority = operations
                        .priorities
                        .iter()
                        .any(|p| p.displaced.contains(&key));
                    let cancelled = operation.cancel.load(Ordering::Relaxed);
                    part.pause.set(operation.paused);
                    part.pause.yield_to_priority(
                        matches!(part.state, PartState::Waiting)
                            || ((priority || operation.paused) && !cancelled),
                    );
                }
            }
        }
        let decided = {
            let mut running = Vec::new();
            let mut waiting = Vec::new();
            for (&id, operation) in &operations.by_id {
                if let Phase::Queued(run) = &operation.phase {
                    for (index, part) in run.parts.iter().enumerate() {
                        let key = TransferId {
                            operation: id,
                            index,
                        };
                        match part.state {
                            PartState::Admitted => running.push((key, &part.plan)),
                            PartState::Ready(_) | PartState::Waiting => {
                                let priority = operations
                                    .priorities
                                    .iter()
                                    .any(|p| p.displaced.contains(&key));
                                if operation.cancel.load(Ordering::Relaxed)
                                    || (!operation.paused && !priority)
                                {
                                    let rank = operations
                                        .priorities
                                        .iter()
                                        .rev()
                                        .position(|p| p.id == id)
                                        .unwrap_or(operations.priorities.len());
                                    waiting.push(((rank, run.order, index), key, &part.plan));
                                }
                            }
                            PartState::Settling | PartState::Ended => {}
                        }
                    }
                }
            }
            waiting.sort_unstable_by_key(|&(rank, _, _)| rank);
            let waiting: Vec<_> = waiting
                .into_iter()
                .map(|(_, key, plan)| (key, plan))
                .collect();
            let on = waits(&running, &waiting, operations.limit);
            waiting
                .into_iter()
                .zip(on)
                .map(|((id, _), on)| (id, on))
                .collect::<Vec<_>>()
        };
        let mut starts = Vec::new();
        for (id, blocker) in decided {
            let operation = operations
                .by_id
                .get_mut(&id.operation)
                .expect("listed as waiting");
            let Phase::Queued(run) = &mut operation.phase else {
                unreachable!("listed as waiting");
            };
            let part = &mut run.parts[id.index];
            part.on = blocker.map(|(owner, reason)| (owner.operation, reason));
            if blocker.is_some() {
                continue;
            }
            match std::mem::replace(&mut part.state, PartState::Admitted) {
                PartState::Waiting => part.pause.yield_to_priority(false),
                PartState::Ready(mut start) => {
                    if run.started.is_none() {
                        run.started = Some(operations.next_start);
                        operations.next_start += 1;
                        if matches!(run.rule.run, Run::First { .. }) {
                            run.rule.run = Run::First {
                                started: Some(system_time()),
                            };
                        }
                        if !matches!(*operation.request, Request::NewFolder { .. }) {
                            let timestamp = match run.rule.run {
                                Run::First { started } | Run::Continue { started } => {
                                    started.unwrap_or_else(system_time)
                                }
                            };
                            let mut saved = self.saved.lock().expect("no queue thread panics");
                            saved.by_id.entry(id.operation).or_insert_with(|| {
                                Arc::new(Recovery::Request {
                                    request: Arc::clone(&operation.request),
                                    started: timestamp,
                                    settings: operation.settings,
                                    saved: OnceLock::new(),
                                })
                            });
                            saved.written = false;
                        }
                    }
                    start.run = run.rule.run;
                    starts.push(Admitted {
                        id,
                        start,
                        pause: Arc::clone(&part.pause),
                        completion: Arc::clone(&run.completion),
                    });
                }
                _ => unreachable!("only ready or waiting transfers request admission"),
            }
        }
        starts
    }

    fn start(self: &Arc<Self>, starts: Vec<Admitted>) {
        for admitted in starts {
            let Admitted {
                id,
                start,
                pause,
                completion,
            } = admitted;
            let (settings, cancel, progress, io) = {
                let operations = self.lock();
                let operation = &operations.by_id[&id.operation];
                (
                    operation.settings,
                    Arc::clone(&operation.cancel),
                    Arc::clone(&operation.progress),
                    Arc::clone(&operation.io),
                )
            };
            let shared = Arc::clone(self);
            self.spawn(move || {
                if let Work::Delete { entries, missing } = &start.work {
                    shared.count(
                        id.operation,
                        Counting::of(entries, *missing),
                        Arc::clone(&cancel),
                    );
                }
                let saved = shared.write_saved();
                let _entered = io.enter();
                let outcome = match *start {
                    Start {
                        work: Work::Compare(roots),
                        ..
                    } => {
                        let compared = match saved {
                            Ok(()) if cancel.load(Ordering::Relaxed) => Err(End::Cancelled),
                            Ok(()) => {
                                let _active = pause.enter(&cancel);
                                roots.compare(&cancel, &pause)
                            }
                            Err(error) => Err(End::Refused(Refusal::RecoveryUnavailable(
                                error.raw_os_error().unwrap_or(29) as u32,
                            ))),
                        };
                        match compared {
                            Ok(review) => match shared.finish_comparison(id, review) {
                                Ok(()) => return,
                                Err(end) => Outcome::ended(end),
                            },
                            Err(end) => Outcome::ended(end),
                        }
                    }
                    start => match saved {
                        Ok(()) => run(
                            start,
                            &Execution {
                                settings: &settings,
                                cancel: &cancel,
                                closing: &shared.closing,
                                pause: &pause,
                                progress: &progress,
                                io: &io,
                            },
                        ),
                        Err(error) => Outcome::ended(End::Refused(Refusal::RecoveryUnavailable(
                            error.raw_os_error().unwrap_or(29) as u32,
                        ))),
                    },
                };
                shared.finish_part(id, outcome, completion);
            });
        }
    }

    fn finish_comparison(
        self: &Arc<Self>,
        id: TransferId,
        review: crate::sync::windows::Review,
    ) -> Result<(), End> {
        let starts = {
            let mut operations = self.lock();
            let operation = operations
                .by_id
                .get_mut(&id.operation)
                .expect("a live worker owns its operation");
            if operation.cancel.load(Ordering::Relaxed) {
                return Err(End::Cancelled);
            }
            let synchronize = operation.synchronize.is_some();
            if synchronize && review.comparison.summary().conflicts != 0 {
                return Err(End::Refused(Refusal::SynchronizationConflicts));
            }
            let Phase::Queued(run) = &operation.phase else {
                unreachable!("comparison retains its admitted part");
            };
            let plan = run.parts[id.index].plan.clone();
            operation.total = Some(review.total());
            operation.phase = Phase::Compared {
                review: Box::new(review),
                plan,
            };
            if synchronize {
                self.commit_comparison(&mut operations, id.operation)
                    .expect("an unconflicted comparison was just retained")
            } else {
                self.schedule(&mut operations)
            }
        };
        self.start(starts);
        (self.wake)();
        Ok(())
    }

    fn commit_comparison(
        &self,
        operations: &mut Operations,
        id: OperationId,
    ) -> Result<Vec<Admitted>, NotCompared> {
        let operation = operations.by_id.get_mut(&id).ok_or(NotCompared)?;
        match &operation.phase {
            Phase::Compared { review, .. } if review.comparison.summary().conflicts == 0 => {}
            _ => return Err(NotCompared),
        }
        let Phase::Compared { review, plan } =
            std::mem::replace(&mut operation.phase, Phase::Preparing)
        else {
            unreachable!("the phase was just matched");
        };
        operation.progress = Arc::default();
        operation.continuing = None;
        let ready = Ready {
            prepared: crate::Prepared {
                entries: 1,
                total: Some(review.total()),
            },
            work: vec![(plan, Work::Synchronize(review))],
            failures: Vec::new(),
            resolved: None,
        };
        Ok(self.enqueue(operations, id, ready))
    }

    fn finish_part(
        self: &Arc<Self>,
        id: TransferId,
        outcome: Outcome,
        completion: Arc<Mutex<Completion>>,
    ) {
        let (starts, progress) = {
            let mut operations = self.lock();
            let operation = operations
                .by_id
                .get_mut(&id.operation)
                .expect("a live worker owns its operation");
            let Phase::Queued(run) = &mut operation.phase else {
                unreachable!("an unsettled part keeps its operation queued");
            };
            assert!(
                !matches!(run.parts[id.index].state, PartState::Ended),
                "a part ends once"
            );
            run.parts[id.index].state = PartState::Ended;
            let progress = Arc::clone(&operation.progress);
            (self.schedule(&mut operations), progress)
        };
        self.start(starts);
        (self.wake)();
        let final_outcome = {
            let mut completion = completion.lock().expect("no engine worker panics");
            completion.listed.extend(outcome.listed);
            completion.ring.to_windows += outcome.ring.to_windows;
            completion.ring.clones_refused += outcome.ring.clones_refused;
            if outcome.end != End::Complete
                && (completion.end == End::Complete || completion.end == End::Cancelled)
            {
                completion.end = outcome.end;
            }
            completion.remaining -= 1;
            (completion.remaining == 0).then(|| {
                Outcome::new(
                    completion.end.clone(),
                    progress.items(),
                    progress.bytes(),
                    std::mem::take(&mut completion.listed),
                    completion.ring,
                )
            })
        };
        let Some(mut outcome) = final_outcome else {
            return;
        };
        if !self.closing.load(Ordering::Relaxed)
            && let Err(error) = self.unsave(id.operation)
        {
            outcome.listed.push(Listed {
                path: self.saved_path.clone(),
                reason: crate::operation::Reason::RecoveryFailed(
                    error.raw_os_error().unwrap_or(29) as u32,
                ),
            });
        }
        let starts = {
            let mut operations = self.lock();
            if let Some(operation) = operations.by_id.get_mut(&id.operation) {
                operation.phase = Phase::Ended(Arc::new(outcome));
            }
            self.schedule(&mut operations)
        };
        self.retake();
        self.start(starts);
        (self.wake)();
    }

    /// Builds the taken entries again from the operations as they are now,
    /// and installs them unless a rebuild from a later read already has.
    /// The build runs outside the queue's lock, because it folds and hashes
    /// every source, which takes a large delete or move long enough that
    /// the window, which takes that lock, would stop answering. Each change
    /// of which sources are taken calls it after it releases the lock.
    fn retake(&self) {
        let (read, takers) = {
            let mut operations = self.lock();
            operations.taken_reads += 1;
            (operations.taken_reads, takers(&operations))
        };
        let mut taken = Taken::default();
        for (id, request, ended) in takers {
            if let Request::Delete { sources } | Request::Move { sources, .. } = &*request {
                for source in sources {
                    taken.insert(id, source, ended);
                }
            }
        }
        let mut taken = Arc::new(taken);
        {
            let mut operations = self.lock();
            if read > operations.taken_from {
                operations.taken_from = read;
                std::mem::swap(&mut operations.taken, &mut taken);
            }
        }
        // The replaced entries, or these when a later read's are installed,
        // are freed here, outside the lock.
        drop(taken);
    }

    /// [`Shared::retake`] for a change on the window's call, on a queue
    /// thread of its own, which first frees `left`, what the change took
    /// out of the queue, and then wakes the window.
    fn retake_later(shared: &Arc<Shared>, left: impl Send + 'static) {
        let thread = Arc::clone(shared);
        shared.spawn(move || {
            drop(left);
            thread.retake();
            (thread.wake)();
        });
    }

    /// Asks gozlerd for a running delete's total on a thread of its own, and
    /// shows it when gozlerd answers before the delete ends.
    fn count(self: &Arc<Self>, id: OperationId, counting: Counting, cancel: Arc<AtomicBool>) {
        let shared = Arc::clone(self);
        self.spawn(move || {
            let Some(total) = counting.total(&*shared.index, &cancel) else {
                return;
            };
            {
                let mut operations = shared.lock();
                match operations.by_id.get_mut(&id) {
                    Some(operation) if matches!(&operation.phase, Phase::Queued(run) if run.started.is_some()) => {
                        operation.total = Some(total);
                    }
                    _ => return,
                }
            }
            (shared.wake)();
        });
    }

    /// Writes the saved file, unless it holds every saved operation. A
    /// change made while it writes marks the file unwritten again, and its
    /// own write follows this one.
    fn write_saved(&self) -> std::io::Result<()> {
        let _writing = self.writing.lock().expect("no queue thread panics");
        let operations: Vec<Arc<Recovery>> = {
            let mut operations = self.saved.lock().expect("no queue thread panics");
            if operations.written {
                return Ok(());
            }
            operations.written = true;
            operations.by_id.values().cloned().collect()
        };
        let written = saved::write(
            &self.saved_path,
            operations.iter().filter_map(|saved| saved.saved()),
        );
        if written.is_err() {
            self.saved.lock().expect("no queue thread panics").written = false;
        }
        written
    }

    /// Removes the operation `id` from the saved file, on a queue thread.
    fn unsave(&self, id: OperationId) -> std::io::Result<()> {
        {
            let mut operations = self.saved.lock().expect("no queue thread panics");
            let removed = operations.by_id.remove(&id).is_some();
            operations.written &= !removed;
        }
        self.write_saved()
    }
}

fn snapshot(
    id: OperationId,
    operation: &Tracked,
    operations: &Operations,
) -> ((u8, u64), Operation) {
    let (place, state) = match &operation.phase {
        Phase::Receiving | Phase::Preparing => ((2, id.0), State::Preparing),
        Phase::Cancelling => ((2, id.0), State::Cancelling),
        Phase::Prepared(ready) => ((2, id.0), State::Prepared(ready.prepared)),
        Phase::Compared { review, .. } => (
            (2, id.0),
            State::Compared(Arc::downgrade(&review.comparison)),
        ),
        Phase::Queued(run) => {
            let cancelled = operation.cancel.load(Ordering::Relaxed);
            let place = operations
                .priorities
                .iter()
                .rev()
                .position(|priority| priority.id == id)
                .map_or(
                    operations.priorities.len() as u64 + run.started.unwrap_or(run.order),
                    |rank| rank as u64,
                );
            if cancelled {
                ((0, place), State::Cancelling)
            } else if run.started.is_none() {
                let (on, reason) = run
                    .parts
                    .iter()
                    .find_map(|part| part.on)
                    .expect("unstarted work has an admission blocker");
                (
                    (1, run.order),
                    State::Waiting {
                        on,
                        reason,
                        total: operation.total,
                    },
                )
            } else {
                let progress = &operation.progress;
                let yielded = run
                    .parts
                    .iter()
                    .filter(|part| !matches!(part.state, PartState::Ended))
                    .all(|part| {
                        !matches!(part.state, PartState::Admitted) || part.pause.snapshot().1
                    });
                let running = Running {
                    phase: run.phase,
                    items: progress.items(),
                    bytes: progress.bytes(),
                    passed: progress.passed(),
                    total: operation.total,
                    renames: operation.renames,
                    paused: operation.paused,
                    yielded,
                    current: progress.current().map(|file| current(&file)),
                };
                ((0, place), State::Running(running))
            }
        }
        Phase::Ended(outcome) => ((2, id.0), State::Ended(Arc::clone(outcome))),
        Phase::Interrupted(_) => ((2, id.0), State::Interrupted),
    };
    let request = Arc::clone(&operation.request);
    let folder = operation.folder.clone();
    (
        place,
        Operation {
            id,
            request,
            folder,
            state,
        },
    )
}

fn current(file: &CurrentFile) -> Current {
    Current {
        source: PathBuf::from(OsString::from_wide(&file.source)),
        destination: file.destination.as_deref().map(plain),
        done: file.done(),
        size: file.size,
    }
}

/// The operations that take their sources if they delete or move them, each
/// with whether it has ended: every one that prepares or
/// was submitted and has not been removed. One that ended before it was
/// submitted, refused or cancelled, changed nothing and takes nothing.
fn takers(operations: &Operations) -> Vec<(OperationId, Arc<Request>, bool)> {
    let takes = |operation: &Tracked| {
        operation.submitted || matches!(operation.phase, Phase::Preparing | Phase::Prepared(_))
    };
    (operations.by_id.iter())
        .filter(|(_, operation)| takes(operation))
        .map(|(&id, operation)| {
            let ended = matches!(operation.phase, Phase::Ended(_));
            (id, Arc::clone(&operation.request), ended)
        })
        .collect()
}

struct Execution<'a> {
    settings: &'a SpeedSettings,
    cancel: &'a Arc<AtomicBool>,
    closing: &'a AtomicBool,
    pause: &'a Pause,
    progress: &'a Progress,
    io: &'a IoThreads,
}

/// Runs an operation on its own thread, with its workers.
fn run(start: Start, execution: &Execution<'_>) -> Outcome {
    let Execution {
        settings,
        cancel,
        closing,
        pause,
        progress,
        io,
    } = *execution;
    let Start {
        work,
        mut failures,
        choice,
        run,
    } = start;
    if cancel.load(Ordering::Relaxed) {
        return Outcome::ended(End::Cancelled);
    }
    progress.add_items(failures.len() as u64);
    let rule = Rule { choice, run };
    let mut disk_full = None;
    let mut ring = RingCounts::default();
    match work {
        Work::Compare(_) => unreachable!("the queue handles comparison before execution"),
        Work::Synchronize(review) => {
            return run_synchronization(*review, rule, execution);
        }
        Work::Delete { entries, missing } => {
            // A selected entry that no longer exists counts as deleted.
            progress.add_items(missing);
            let workers = settings.delete_workers.get();
            failures.extend(walk::delete(entries, workers, cancel, pause, progress, io));
        }
        Work::Transfer(transfer) => {
            match run_transfer(*transfer, rule, cancel, closing, pause, progress, io) {
                Ok(copied) => {
                    failures.extend(copied.listed);
                    disk_full = copied.disk_full;
                    ring = copied.ring;
                }
                Err(end) => {
                    return Outcome::new(end, progress.items(), progress.bytes(), failures, ring);
                }
            }
        }
        Work::Rename {
            path,
            from,
            to,
            name,
        } => {
            let _active = pause.enter(cancel);
            if cancel.load(Ordering::Relaxed) {
                return Outcome::ended(End::Cancelled);
            }
            let one_name =
                !name.is_empty() && !name.contains(&BACKSLASH) && !name.contains(&u16::from(b'/'));
            let renamed = if one_name && !to.is_empty() {
                rename(&from, &to, Existing::Refuse).map_err(|error| error.code)
            } else {
                Err(ERROR_INVALID_NAME)
            };
            match renamed {
                Ok(()) => progress.add_items(1),
                Err(error) => failures.push(failure(&path, error)),
            }
        }
        Work::NewFolder { path, verbatim } => match create_folder(&verbatim, None) {
            Ok(()) => progress.add_items(1),
            Err(error) => failures.push(failure(&path, error.code)),
        },
    }
    let end = if let Some(no_room) = disk_full {
        End::DiskFull(no_room)
    } else if cancel.load(Ordering::Relaxed) {
        End::Cancelled
    } else {
        End::Complete
    };
    Outcome::new(end, progress.items(), progress.bytes(), failures, ring)
}

fn run_synchronization(
    mut review: crate::sync::windows::Review,
    rule: Rule,
    execution: &Execution<'_>,
) -> Outcome {
    let Execution {
        settings,
        cancel,
        closing,
        pause,
        progress,
        io,
    } = *execution;
    let mut listed = Vec::new();
    let mut ring = RingCounts::default();
    let copied = || {
        let _active = pause.enter(cancel);
        if cancel.load(Ordering::Relaxed) {
            return Err(End::Cancelled);
        }
        review.current()?;
        let request = review.copy_request(cancel, pause)?;
        let index = review.index(pause);
        prepare(
            &request,
            &Preparation {
                settings,
                rule,
                index: &index,
                cancel,
                pause: Some(pause),
            },
        )
        .map_err(|refusal| {
            if cancel.load(Ordering::Relaxed) {
                End::Cancelled
            } else {
                End::Refused(refusal)
            }
        })
    };
    let ready = match copied() {
        Ok(ready) => ready,
        Err(end) => return Outcome::new(end, progress.items(), progress.bytes(), listed, ring),
    };
    progress.add_items(ready.failures.len() as u64);
    listed.extend(ready.failures);
    for (_, work) in ready.work {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Work::Transfer(transfer) = work else {
            unreachable!("a copy prepares only transfer work");
        };
        match run_transfer(*transfer, rule, cancel, closing, pause, progress, io) {
            Ok(copied) => {
                listed.extend(
                    copied
                        .listed
                        .into_iter()
                        .filter(|entry| entry.reason != crate::Reason::Skipped),
                );
                ring.to_windows += copied.ring.to_windows;
                ring.clones_refused += copied.ring.clones_refused;
                if let Some(no_room) = copied.disk_full {
                    return Outcome::new(
                        End::DiskFull(no_room),
                        progress.items(),
                        progress.bytes(),
                        listed,
                        ring,
                    );
                }
            }
            Err(end) => return Outcome::new(end, progress.items(), progress.bytes(), listed, ring),
        }
    }
    let failed = !listed.is_empty();
    let end = if cancel.load(Ordering::Relaxed) {
        End::Cancelled
    } else if review.mirrors() && !failed {
        let _active = pause.enter(cancel);
        match review.verify_output(cancel, pause) {
            Err(end) => end,
            Ok(()) => {
                let (removed, end) = review.remove_extras(cancel, pause, progress);
                listed.extend(removed);
                end
            }
        }
    } else {
        End::Complete
    };
    Outcome::new(end, progress.items(), progress.bytes(), listed, ring)
}

/// Runs a copy or a move. Returns what it did, or how it ended before it
/// changed anything.
fn run_transfer(
    transfer: Transfer,
    rule: Rule,
    cancel: &AtomicBool,
    closing: &AtomicBool,
    pause: &Pause,
    progress: &Progress,
    io: &IoThreads,
) -> Result<Copied, End> {
    let Transfer {
        destination,
        name: new_name,
        missing,
        renamed,
        copied,
        moving,
        plan,
        target: target_volume,
    } = transfer;
    let active = pause.enter(cancel);
    let target = destination.ready.get_or_init(pause, cancel, || {
        if cancel.load(Ordering::Relaxed) {
            return Err(End::Cancelled);
        }
        let received = &destination.received;
        if let Some(needed) = destination.needed {
            check_space(received, needed).map_err(End::Refused)?;
        }
        let mut page = vec![0u64; LISTING_WORDS];
        let unreadable = |error| End::Refused(Refusal::DestinationUnreadable(error));
        let mut made = false;
        let mut verbatim = received.verbatim.clone();
        for name in &received.missing {
            if cancel.load(Ordering::Relaxed) {
                return Err(End::Cancelled);
            }
            verbatim = join(&verbatim, name);
            made = match create_folder(&verbatim, None) {
                Ok(()) => true,
                Err(error) if error.code == ERROR_ALREADY_EXISTS && is_folder_path(&verbatim) => {
                    false
                }
                Err(error) => return Err(unreadable(error.code)),
            };
        }
        let names = if made {
            Names::default()
        } else if received.missing.is_empty() {
            received
                .names(&mut page, || cancel.load(Ordering::Relaxed))
                .map_err(unreadable)?
        } else {
            open_entry(&verbatim, Access::List, Reparse::Open)
                .map_err(|error| error.code)
                .and_then(|folder| {
                    read_names(&folder, &mut page, || cancel.load(Ordering::Relaxed))
                })
                .map_err(unreadable)?
        };
        Ok(Arc::new(Target::new(verbatim, names, made)))
    })?;
    if cancel.load(Ordering::Relaxed) {
        return Err(End::Cancelled);
    }
    let mut listed = Vec::new();
    for path in missing {
        progress.add_items(1);
        // After a crash Continue meets exactly the entries that already
        // moved; on a first run the engine knows it moved nothing, so a
        // source gone meanwhile is never reported as moved.
        let name = new_name.as_deref().unwrap_or_else(|| {
            path.rsplit(|&unit| unit == BACKSLASH)
                .next()
                .unwrap_or(&path)
        });
        let names = target.names.lock().expect("no worker panics");
        let keep_both = rule.choice == Some(Policy::KeepBoth);
        let numbered = |is_folder| names.numbered(name, is_folder).next().is_some();
        let arrived =
            names.get(name).is_some() || (keep_both && (numbered(false) || numbered(true)));
        drop(names);
        if !(moving && !matches!(rule.run, Run::First { .. }) && arrived) {
            listed.push(failure(&path, ERROR_FILE_NOT_FOUND));
        }
    }
    drop(active);
    if !renamed.is_empty() {
        let moves = Moving {
            rule,
            cancel,
            pause,
            progress,
        };
        listed.extend(move_by_rename(renamed, &target, &moves));
    }
    let mut done = Copied {
        listed,
        disk_full: None,
        ring: RingCounts::default(),
    };
    if !copied.is_empty() && !cancel.load(Ordering::Relaxed) {
        let copying = Copying {
            plan,
            rule,
            moving,
            target: target_volume,
            cancel,
            closing,
            pause,
            progress,
            io,
        };
        let copied = copy(copied, target, &copying);
        done.listed.extend(copied.listed);
        done.disk_full = copied.disk_full;
        done.ring = copied.ring;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            next_id: AtomicU64::new(0),
            operations: Mutex::default(),
            wake: Box::new(|| {}),
            index: Box::new(Gozlerd),
            saved_path: PathBuf::new(),
            saved: Mutex::new(SavedOperations {
                by_id: BTreeMap::new(),
                written: true,
            }),
            writing: Mutex::default(),
            closing: AtomicBool::new(false),
            threads: Mutex::default(),
        }
    }

    #[test]
    fn operation_ids_can_be_reserved_while_engine_state_is_locked() {
        let queue = Arc::new(Queue {
            shared: Arc::new(shared()),
        });
        let locked = queue.shared.lock();
        let other = Arc::clone(&queue);
        let (sender, reserved) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _ = sender.send((other.reserve_id(), other.reserve_id()));
        });
        let answer = reserved.recv_timeout(std::time::Duration::from_secs(5));
        drop(locked);
        thread.join().unwrap();
        let (first, second) = answer.expect("reservation never waits for engine state");
        assert_eq!(first.id(), OperationId(0));
        assert_eq!(second.id(), OperationId(1));
        for (reserved, path) in [(first, "first"), (second, "second")] {
            queue.accept_reserved(
                reserved,
                Request::NewFolder {
                    path: PathBuf::from(path),
                },
                SpeedSettings::automatic(),
            );
        }
        assert_eq!(
            queue.operation(OperationId(0)).unwrap().request.as_ref(),
            &Request::NewFolder {
                path: PathBuf::from("first")
            }
        );
        assert_eq!(
            queue.operation(OperationId(1)).unwrap().request.as_ref(),
            &Request::NewFolder {
                path: PathBuf::from("second")
            }
        );
    }

    fn key(operation: u64, index: usize) -> TransferId {
        TransferId {
            operation: OperationId(operation),
            index,
        }
    }

    fn part(disks: &[u32], running: bool) -> Part {
        Part {
            plan: Plan::new(
                disks
                    .iter()
                    .copied()
                    .map(crate::volumes::Disk::Number)
                    .collect(),
            ),
            pause: Arc::default(),
            state: if running {
                PartState::Admitted
            } else {
                PartState::Ready(Box::new(Start {
                    work: Work::NewFolder {
                        path: Vec::new(),
                        verbatim: Vec::new(),
                    },
                    failures: Vec::new(),
                    choice: None,
                    run: Run::First { started: None },
                }))
            },
            on: Some((OperationId(1), Wait::Disk)),
        }
    }

    fn tracked(id: u64, running: bool) -> Tracked {
        Tracked::new(
            Arc::new(Request::Copy {
                policy: crate::Policy::default(),
                sources: Vec::new(),
                destination: crate::Destination::Folder(PathBuf::new()),
            }),
            SpeedSettings::automatic(),
            Phase::Queued(Scheduled {
                phase: crate::RunningPhase::Executing,
                parts: vec![part(&[1], running)],
                order: id,
                started: running.then_some(id),
                rule: Rule {
                    choice: None,
                    run: Run::First { started: None },
                },
                completion: Arc::new(Mutex::new(Completion {
                    remaining: 1,
                    listed: Vec::new(),
                    end: End::Complete,
                    ring: RingCounts::default(),
                })),
            }),
        )
    }

    fn scheduled(operation: &mut Tracked) -> &mut Scheduled {
        let Phase::Queued(run) = &mut operation.phase else {
            panic!("queued")
        };
        run
    }

    fn first(operation: &Tracked) -> &Part {
        let Phase::Queued(run) = &operation.phase else {
            panic!("queued")
        };
        &run.parts[0]
    }

    #[test]
    fn snapshots_keep_manual_pause_separate_from_priority_yield() {
        let queue = Queue {
            shared: Arc::new(shared()),
        };
        let operation = tracked(1, true);
        first(&operation).pause.yield_to_priority(true);
        queue.shared.lock().by_id.insert(OperationId(1), operation);
        let flags = || {
            let snapshot = queue.operations();
            let State::Running(running) = &snapshot[0].state else {
                panic!("running")
            };
            (running.paused, running.yielded)
        };
        assert_eq!(flags(), (false, true));
        queue.set_paused(OperationId(1), true);
        assert_eq!(flags(), (true, true));
        queue.set_paused(OperationId(1), false);
        assert_eq!(flags(), (false, false));
    }

    #[test]
    fn priority_keeps_progress_and_returns_without_resuming_manual_pause() {
        let shared = shared();
        for manual in [false, true] {
            let mut operations = Operations::default();
            operations.by_id.insert(OperationId(1), tracked(1, true));
            operations.by_id.insert(OperationId(2), tracked(2, false));
            let mut unrelated = tracked(3, true);
            scheduled(&mut unrelated).parts[0].plan =
                Plan::new(vec![crate::volumes::Disk::Number(2)]);
            operations.by_id.insert(OperationId(3), unrelated);
            let original = operations.by_id.get_mut(&OperationId(1)).unwrap();
            original.progress.add_bytes(512);
            original.paused = manual;
            let displaced = operations
                .priority(OperationId(2), Some(OperationId(1)))
                .unwrap()
                .unwrap();
            assert_eq!(displaced, [key(1, 0)]);
            operations.priorities.push(Priority {
                id: OperationId(2),
                displaced,
            });
            assert!(shared.schedule(&mut operations).is_empty());
            assert!(!first(&operations.by_id[&OperationId(3)]).pause.is_paused());
            scheduled(operations.by_id.get_mut(&OperationId(1)).unwrap()).parts[0].state =
                PartState::Waiting;
            let starts = shared.schedule(&mut operations);
            assert_eq!(
                starts.iter().map(|start| start.id).collect::<Vec<_>>(),
                [key(2, 0)]
            );
            operations.by_id.get_mut(&OperationId(2)).unwrap().phase =
                Phase::Ended(Arc::new(Outcome::ended(End::Complete)));
            assert!(shared.schedule(&mut operations).is_empty());
            let original = &operations.by_id[&OperationId(1)];
            assert_eq!(original.progress.bytes(), 512);
            assert_eq!(first(original).pause.is_paused(), manual);
            assert_eq!(matches!(first(original).state, PartState::Waiting), manual);
        }
    }

    #[test]
    fn nested_priority_keeps_the_outer_yield_until_the_inner_job_ends() {
        let shared = shared();
        let mut operations = Operations::default();
        for id in 1..=3 {
            operations.by_id.insert(OperationId(id), tracked(id, true));
        }
        operations.priorities.push(Priority {
            id: OperationId(2),
            displaced: vec![key(1, 0)],
        });
        operations.priorities.push(Priority {
            id: OperationId(3),
            displaced: vec![key(1, 0), key(2, 0)],
        });
        shared.schedule(&mut operations);
        operations.by_id.get_mut(&OperationId(2)).unwrap().phase =
            Phase::Ended(Arc::new(Outcome::ended(End::Cancelled)));
        shared.schedule(&mut operations);
        assert!(first(&operations.by_id[&OperationId(1)]).pause.is_paused());
        operations.by_id.get_mut(&OperationId(3)).unwrap().phase =
            Phase::Ended(Arc::new(Outcome::ended(End::Complete)));
        shared.schedule(&mut operations);
        assert!(!first(&operations.by_id[&OperationId(1)]).pause.is_paused());
    }

    #[test]
    fn cancelled_yielded_work_reacquires_every_device_before_cleanup() {
        let shared = shared();
        let mut operations = Operations::default();
        let mut original = tracked(1, true);
        let run = scheduled(&mut original);
        run.parts[0].plan = Plan::new(vec![
            crate::volumes::Disk::Number(1),
            crate::volumes::Disk::Number(2),
        ]);
        run.parts[0].state = PartState::Waiting;
        original.cancel.store(true, Ordering::Relaxed);
        operations.by_id.insert(OperationId(1), original);
        operations.by_id.insert(OperationId(2), tracked(2, true));
        let mut other = tracked(3, true);
        scheduled(&mut other).parts[0].plan = Plan::new(vec![crate::volumes::Disk::Number(2)]);
        operations.by_id.insert(OperationId(3), other);
        operations.priorities.push(Priority {
            id: OperationId(2),
            displaced: vec![key(1, 0)],
        });
        shared.schedule(&mut operations);
        operations.by_id.get_mut(&OperationId(2)).unwrap().phase =
            Phase::Ended(Arc::new(Outcome::ended(End::Complete)));
        shared.schedule(&mut operations);
        assert!(matches!(
            first(&operations.by_id[&OperationId(1)]).state,
            PartState::Waiting
        ));
        assert!(first(&operations.by_id[&OperationId(1)]).pause.is_paused());
        operations.by_id.get_mut(&OperationId(3)).unwrap().phase =
            Phase::Ended(Arc::new(Outcome::ended(End::Complete)));
        shared.schedule(&mut operations);
        assert!(matches!(
            first(&operations.by_id[&OperationId(1)]).state,
            PartState::Admitted
        ));
        assert!(!first(&operations.by_id[&OperationId(1)]).pause.is_paused());
    }

    #[test]
    fn delete_bypasses_devices_without_resuming_a_yielded_copy() {
        let shared = shared();
        let mut operations = Operations::default();
        let mut original = tracked(1, true);
        scheduled(&mut original).parts[0].state = PartState::Waiting;
        let mut deleting = tracked(3, false);
        deleting.request = Arc::new(Request::Delete {
            sources: Vec::new(),
        });
        scheduled(&mut deleting).parts[0].plan = Plan::default();
        operations.by_id.insert(OperationId(1), original);
        operations.by_id.insert(OperationId(2), tracked(2, true));
        operations.by_id.insert(OperationId(3), deleting);
        operations.priorities.push(Priority {
            id: OperationId(2),
            displaced: vec![key(1, 0)],
        });
        let starts = shared.schedule(&mut operations);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].id, key(3, 0));
        assert!(first(&operations.by_id[&OperationId(1)]).pause.is_paused());
    }

    #[test]
    fn a_busy_pair_does_not_hold_an_independent_pair_in_the_same_operation() {
        let shared = shared();
        let mut operations = Operations::default();
        operations.by_id.insert(OperationId(1), tracked(1, true));
        let mut operation = tracked(2, false);
        scheduled(&mut operation).parts.push(part(&[2, 3], false));
        scheduled(&mut operation)
            .completion
            .lock()
            .unwrap()
            .remaining = 2;
        operations.by_id.insert(OperationId(2), operation);
        let starts = shared.schedule(&mut operations);
        assert_eq!(
            starts.iter().map(|start| start.id).collect::<Vec<_>>(),
            [key(2, 1)]
        );
        assert!(matches!(
            first(&operations.by_id[&OperationId(2)]).state,
            PartState::Ready(_)
        ));
    }

    #[test]
    fn priority_yields_only_conflicting_pairs_including_synchronization() {
        for sync in [None, Some(1), Some(2)] {
            let mut operations = Operations::default();
            let mut original = tracked(1, true);
            scheduled(&mut original).parts.push(part(&[2, 3], true));
            operations.by_id.insert(OperationId(1), original);
            operations.by_id.insert(OperationId(2), tracked(2, false));
            if let Some(id) = sync {
                operations.by_id.get_mut(&OperationId(id)).unwrap().request =
                    Arc::new(Request::Synchronize {
                        source: PathBuf::from(r"C:\source"),
                        destination: PathBuf::from(r"D:\destination"),
                        mirror: false,
                    });
            }
            assert_eq!(
                operations
                    .priority(OperationId(2), Some(OperationId(1)))
                    .unwrap(),
                Some(vec![key(1, 0)]),
            );
        }
    }

    #[test]
    fn lowering_the_limit_drains_without_preempting_and_raising_admits() {
        let shared = shared();
        let mut operations = Operations {
            limit: TransferLimit::try_from(2).unwrap(),
            ..Operations::default()
        };
        operations.by_id.insert(OperationId(1), tracked(1, true));
        operations.by_id.insert(OperationId(2), tracked(2, true));
        operations.by_id.insert(OperationId(3), tracked(3, false));
        operations.limit = TransferLimit::default();
        assert!(shared.schedule(&mut operations).is_empty());
        assert!(!first(&operations.by_id[&OperationId(1)]).pause.is_paused());
        assert!(!first(&operations.by_id[&OperationId(2)]).pause.is_paused());
        operations.limit = TransferLimit::try_from(3).unwrap();
        let starts = shared.schedule(&mut operations);
        assert_eq!(
            starts.iter().map(|start| start.id).collect::<Vec<_>>(),
            [key(3, 0)]
        );
    }

    #[test]
    fn first_admission_records_recovery_before_starting_any_worker() {
        let shared = shared();
        let mut operations = Operations::default();
        operations.by_id.insert(OperationId(1), tracked(1, false));
        let starts = shared.schedule(&mut operations);
        assert_eq!(starts.len(), 1);
        let saved = shared.saved.lock().unwrap();
        let Recovery::Request { request, saved, .. } = &*saved.by_id[&OperationId(1)] else {
            panic!("first admission records the accepted request")
        };
        assert!(Arc::ptr_eq(
            request,
            &operations.by_id[&OperationId(1)].request
        ));
        assert!(saved.get().is_none());
    }

    fn wait_for(
        queue: &Queue,
        wakes: &Receiver<()>,
        id: OperationId,
        until: impl Fn(&State) -> bool,
    ) -> State {
        loop {
            let state = queue
                .operations()
                .into_iter()
                .find(|operation| operation.id == id)
                .unwrap()
                .state;
            if until(&state) {
                return state;
            }
            wakes
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the operation moves on");
        }
    }

    fn split_prepared(queue: &Queue, id: OperationId) {
        let mut operations = queue.shared.lock();
        let Phase::Prepared(ready) = &mut operations.by_id.get_mut(&id).unwrap().phase else {
            panic!("prepared")
        };
        let (_, Work::Transfer(mut transfer)) = ready.work.pop().unwrap() else {
            panic!("copy")
        };
        let second = Transfer {
            destination: Arc::clone(&transfer.destination),
            name: None,
            missing: Vec::new(),
            renamed: Vec::new(),
            copied: vec![transfer.copied.pop().unwrap()],
            moving: false,
            plan: transfer.plan,
            target: transfer.target,
        };
        ready.work = vec![
            (
                Plan::new(vec![
                    crate::volumes::Disk::Number(1),
                    crate::volumes::Disk::Number(3),
                ]),
                Work::Transfer(transfer),
            ),
            (
                Plan::new(vec![
                    crate::volumes::Disk::Number(2),
                    crate::volumes::Disk::Number(3),
                ]),
                Work::Transfer(Box::new(second)),
            ),
        ];
    }

    #[test]
    fn concurrent_pairs_share_nested_destination_names() {
        let scratch = crate::tests::Scratch::new("pair-names");
        let first = scratch.write(r"one\a\inner\x.bin", b"first");
        let second = scratch.write(r"two\a\inner\x.bin", b"second");
        let existing = scratch.write(r"out\a\inner\x.bin", b"original");
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for path in [&first, &second, &existing] {
            crate::tests::set_created(path, older);
        }
        let (wake, wakes) = mpsc::channel();
        let queue = Queue::with_index(
            scratch.saved(),
            Box::new(crate::tests::NoIndex),
            move || {
                let _ = wake.send(());
            },
        );
        queue.set_transfer_limit(TransferLimit::try_from(2).unwrap());
        let id = queue.prepare(
            Request::Copy {
                policy: Policy::Replace,
                sources: vec![scratch.path(r"one\a"), scratch.path(r"two\a")],
                destination: crate::Destination::Folder(scratch.path("out")),
            },
            SpeedSettings::automatic(),
        );
        wait_for(&queue, &wakes, id, |state| {
            matches!(state, State::Prepared(_))
        });
        split_prepared(&queue, id);
        queue.submit(id).unwrap();
        let State::Ended(outcome) =
            wait_for(&queue, &wakes, id, |state| matches!(state, State::Ended(_)))
        else {
            panic!("ended")
        };
        assert_eq!(outcome.end, End::Complete);
        assert!(outcome.listed.is_empty(), "{:?}", outcome.listed);
        let mut contents = [r"out\a\inner\x.bin", r"out\a\inner\x (2).bin"]
            .map(|path| std::fs::read(scratch.path(path)).unwrap());
        contents.sort();
        assert_eq!(contents, [b"first".to_vec(), b"second".to_vec()]);
    }

    #[test]
    fn pause_handoff_requires_readmission_and_cancellation_skips_ready_work() {
        for cancelled in [false, true] {
            let scratch = crate::tests::Scratch::new("pair-handoff");
            let first = scratch.write(r"one\first.bin", &vec![1; 256 * 1024]);
            let second = scratch.write(r"one\second.bin", b"second");
            let other = scratch.write(r"two\other.bin", b"other");
            std::fs::create_dir_all(scratch.path("out")).unwrap();
            std::fs::create_dir_all(scratch.path("other")).unwrap();
            let (wake, wakes) = mpsc::channel();
            let queue = Arc::new(Queue::with_index(
                scratch.saved(),
                Box::new(crate::tests::NoIndex),
                move || {
                    let _ = wake.send(());
                },
            ));
            let id = queue.prepare(
                Request::Copy {
                    policy: crate::Policy::default(),
                    sources: vec![first, second],
                    destination: crate::Destination::Folder(scratch.path("out")),
                },
                SpeedSettings::automatic(),
            );
            wait_for(&queue, &wakes, id, |state| {
                matches!(state, State::Prepared(_))
            });
            split_prepared(&queue, id);
            let other = queue.prepare(
                Request::Copy {
                    policy: crate::Policy::default(),
                    sources: vec![other],
                    destination: crate::Destination::Folder(scratch.path("other")),
                },
                SpeedSettings::automatic(),
            );
            wait_for(&queue, &wakes, other, |state| {
                matches!(state, State::Prepared(_))
            });
            let (paused, notice) = mpsc::channel();
            let weak = Arc::downgrade(&queue);
            let (busy, holding) = mpsc::channel();
            let (release, released) = mpsc::channel();
            {
                let mut operations = queue.shared.lock();
                *operations.by_id[&id]
                    .progress
                    .at_first_report
                    .lock()
                    .unwrap() = Some(Box::new(move || {
                    weak.upgrade().unwrap().set_paused(id, true);
                    paused.send(()).unwrap();
                }));
                let operation = operations.by_id.get_mut(&other).unwrap();
                let Phase::Prepared(ready) = &mut operation.phase else {
                    panic!("prepared")
                };
                ready.work[0].0 = Plan::new(vec![crate::volumes::Disk::Number(3)]);
                *operation.progress.at_first_report.lock().unwrap() = Some(Box::new(move || {
                    busy.send(()).unwrap();
                    released
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap();
                }));
            }
            queue.submit(id).unwrap();
            notice
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            queue.submit(other).unwrap();
            holding
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            if cancelled {
                queue.cancel(id);
                wait_for(&queue, &wakes, id, |state| {
                    matches!(state, State::Cancelling)
                });
            } else {
                queue.set_paused(id, false);
                let State::Running(running) = wait_for(&queue, &wakes, id, |state| {
                    matches!(state, State::Running(_))
                }) else {
                    panic!("running")
                };
                assert!(!running.paused);
                assert!(running.yielded);
            }
            assert!(!scratch.path(r"out\second.bin").exists());
            release.send(()).unwrap();
            wait_for(&queue, &wakes, other, |state| {
                matches!(state, State::Ended(_))
            });
            let State::Ended(outcome) =
                wait_for(&queue, &wakes, id, |state| matches!(state, State::Ended(_)))
            else {
                panic!("ended")
            };
            if cancelled {
                assert_eq!(outcome.end, End::Cancelled);
                assert!(!scratch.path(r"out\second.bin").exists());
            } else {
                assert_eq!(outcome.end, End::Complete);
                assert_eq!(outcome.items, 2);
                assert_eq!(
                    std::fs::read(scratch.path(r"out\second.bin")).unwrap(),
                    b"second"
                );
            }
        }
    }
}
