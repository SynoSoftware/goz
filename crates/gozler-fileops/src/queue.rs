//! The engine's queue: the one owner of which operations exist, of which
//! wait for each other (D14), of the saved file (D19), and of every engine
//! thread.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use gozler_winfs::{Existing, create_folder, rename, system_time};

use crate::conflicts::{Choice, Rule, Run};
use crate::copy::{Copied, Copying, copy};
use crate::delete::walk;
use crate::io_threads::IoThreads;
use crate::listing::{BACKSLASH, LISTING_WORDS};
use crate::moves::{Moving, move_by_rename};
use crate::operation::{
    Current, CurrentFile, End, Listed, Operation, OperationId, OrderRefused, Outcome, Pause,
    Progress, Request, RingCounts, Running, State, Total, Wait,
};
use crate::place::Target;
use crate::prepare::{Ready, Transfer, Work, check_space, prepare};
use crate::receive::{failure, plain};
use crate::saved::{self, Restored, Saved};
use crate::settings::SpeedSettings;
use crate::taken::Taken;
use crate::totals::{Counting, Gozlerd, Index};
use crate::waits::{Plan, reorder, waits};
use crate::win32::ERROR_FILE_NOT_FOUND;

/// `ERROR_INVALID_NAME`: a new name that is empty or holds a separator,
/// which would move the entry instead of renaming it.
const ERROR_INVALID_NAME: u32 = 123;

/// The queue the window talks to. Dropping it cancels every operation and
/// joins every thread, so no engine thread outlives it.
pub struct Queue {
    shared: Arc<Shared>,
}

struct Shared {
    operations: Mutex<Operations>,
    /// Called after each change of an operation's state, never under the
    /// queue's lock, so it may read the states at once.
    wake: Box<dyn Fn() + Send + Sync>,
    /// Where prepare reads the totals.
    index: Box<dyn Index>,
    saved_path: PathBuf,
    /// The operations in the saved file. Its lock also orders the file's
    /// writes.
    saved: Mutex<BTreeMap<OperationId, Saved>>,
    /// Set when the queue closes or is dropped: a run that it cancels stays
    /// saved, so it is offered as interrupted at the next start, which is
    /// the truth, and keeps the files it was writing for Continue to resume.
    closing: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct Operations {
    next_id: u64,
    next_order: u64,
    next_start: u64,
    by_id: BTreeMap<OperationId, Tracked>,
    taken: Arc<Taken>,
}

struct Tracked {
    request: Arc<Request>,
    settings: SpeedSettings,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    pause: Arc<Pause>,
    progress: Arc<Progress>,
    io: Arc<IoThreads>,
    /// Continue's prepare: it submits with the interrupted run's choice and
    /// start as soon as it is prepared.
    continuing: Option<Rule>,
    /// It was submitted, so its sources are taken while it lives.
    submitted: bool,
    /// Prepare's total, from submit on.
    total: Option<Total>,
    /// [`Running::renames`], from submit on.
    renames: bool,
}

enum Phase {
    Preparing,
    Prepared(Box<Ready>),
    Waiting {
        plan: Plan,
        start: Box<Start>,
        /// Its place in the run order: submit order, which the person can
        /// change.
        order: u64,
        /// Set under the same lock as the phase, by `schedule`.
        on: Option<(OperationId, Wait)>,
    },
    Running {
        plan: Plan,
        /// Its place in start order.
        started: u64,
    },
    /// An ended operation holds no plan, so it makes nothing wait.
    Ended(Arc<Outcome>),
    Interrupted(Restored),
}

/// What a run takes to its thread.
struct Start {
    work: Work,
    failures: Vec<Listed>,
    choice: Option<Choice>,
    run: Run,
}

/// A submit for an operation that is not prepared: it does not exist, or
/// it is still preparing, already running or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operation is not prepared")]
pub struct NotPrepared;

/// Continue or Dismiss for an operation that is not interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operation is not interrupted")]
pub struct NotInterrupted;

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
        for (saved, restored) in saved::read(&saved_path) {
            let id = OperationId(operations.next_id);
            operations.next_id += 1;
            let operation = Tracked::new(
                Arc::new(restored.request.clone()),
                restored.settings,
                Phase::Interrupted(restored),
            );
            operations.by_id.insert(id, operation);
            saved_operations.insert(id, saved);
        }
        Queue {
            shared: Arc::new(Shared {
                operations: Mutex::new(operations),
                wake: Box::new(wake),
                index,
                saved_path,
                saved: Mutex::new(saved_operations),
                closing: AtomicBool::new(false),
                threads: Mutex::default(),
            }),
        }
    }

    /// Starts preparing `request` on a queue thread and returns its id at
    /// once.
    pub fn prepare(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let id = {
            let mut operations = self.shared.lock();
            let id = OperationId(operations.next_id);
            operations.next_id += 1;
            let operation = Tracked::new(Arc::new(request), settings, Phase::Preparing);
            operations.by_id.insert(id, operation);
            id
        };
        Shared::prepare(&self.shared, id);
        (self.shared.wake)();
        id
    }

    /// Starts a prepared operation as soon as no earlier one makes it wait.
    /// `choice` is the person's answer for the conflicts, if they were asked.
    pub fn submit(&self, id: OperationId, choice: Option<Choice>) -> Result<(), NotPrepared> {
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
            self.shared.enqueue(&mut operations, id, *ready, choice)
        };
        self.shared.start(starts);
        (self.shared.wake)();
        Ok(())
    }

    /// Cancels an operation. One that has not started ends at once; a
    /// running one stops within one `CopyFile2` chunk, or one delete and its
    /// retries, also while it is paused.
    pub fn cancel(&self, id: OperationId) {
        let starts = {
            let mut operations = self.shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return;
            };
            operation.cancel.store(true, Ordering::Relaxed);
            operation.pause.wake();
            operation.io.cancel();
            match operation.phase {
                Phase::Preparing | Phase::Prepared(_) | Phase::Waiting { .. } => {
                    operation.phase = Phase::Ended(Arc::new(Outcome::ended(End::Cancelled)));
                    let starts = self.shared.schedule(&mut operations);
                    operations.taken = taken(&operations);
                    starts
                }
                Phase::Running { .. } | Phase::Ended(_) | Phase::Interrupted(_) => return,
            }
        };
        // A Continue cancelled before its run started ends too.
        self.shared.unsave(id);
        self.shared.start(starts);
        (self.shared.wake)();
    }

    /// Removes an ended operation, once no list of the window needs it.
    pub fn remove(&self, id: OperationId) {
        let mut operations = self.shared.lock();
        if let Some(Tracked {
            phase: Phase::Ended(_),
            ..
        }) = operations.by_id.get(&id)
        {
            operations.by_id.remove(&id);
            operations.taken = taken(&operations);
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
                choice: restored.choice,
                run: Run::Continue {
                    started: restored.started,
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
            let mut operations = self.shared.lock();
            let Some(Tracked {
                phase: Phase::Interrupted(_),
                ..
            }) = operations.by_id.get(&id)
            else {
                return Err(NotInterrupted);
            };
            operations.by_id.remove(&id);
        }
        self.shared.unsave(id);
        (self.shared.wake)();
        Ok(())
    }

    /// Every operation: the running ones in start order, then the waiting
    /// ones in run order, then the rest in the order they were prepared.
    pub fn operations(&self) -> Vec<Operation> {
        let operations = self.shared.lock();
        let mut listed: Vec<((u8, u64), Operation)> = (operations.by_id.iter())
            .map(|(&id, operation)| {
                let (place, state) = match &operation.phase {
                    Phase::Preparing => ((2, id.0), State::Preparing),
                    Phase::Prepared(ready) => ((2, id.0), State::Prepared(ready.prepared)),
                    Phase::Waiting { on, order, .. } => {
                        let (on, reason) = on.expect(
                            "schedule sets it under the same lock; it skips only while the queue drops, when nothing reads states",
                        );
                        let total = operation.total;
                        ((1, *order), State::Waiting { on, reason, total })
                    }
                    Phase::Running { started, .. } => {
                        let progress = &operation.progress;
                        let running = Running {
                            items: progress.items(),
                            bytes: progress.bytes(),
                            total: operation.total,
                            renames: operation.renames,
                            paused: operation.pause.is_paused(),
                            current: progress.current().map(|file| current(&file)),
                        };
                        ((0, *started), State::Running(running))
                    }
                    Phase::Ended(outcome) => ((2, id.0), State::Ended(Arc::clone(outcome))),
                    Phase::Interrupted(_) => ((2, id.0), State::Interrupted),
                };
                let request = Arc::clone(&operation.request);
                (place, Operation { id, request, state })
            })
            .collect();
        listed.sort_unstable_by_key(|&(place, _)| place);
        listed.into_iter().map(|(_, operation)| operation).collect()
    }

    /// Pauses or resumes a running operation. A paused one stops within one
    /// `CopyFile2` chunk, one ring block, or one entry, and still makes other
    /// operations wait. Does nothing to an operation that is not running.
    pub fn set_paused(&self, id: OperationId, paused: bool) {
        {
            let operations = self.shared.lock();
            let Some(Tracked {
                phase: Phase::Running { .. },
                pause,
                ..
            }) = operations.by_id.get(&id)
            else {
                return;
            };
            pause.set(paused);
        }
        (self.shared.wake)();
    }

    /// Moves the waiting operation `id` to run just before the waiting
    /// operation `before`, or last, and starts what may then start. Refused
    /// when it would pass an operation whose paths it overlaps (D14).
    pub fn move_waiting(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<(), OrderRefused> {
        let starts = {
            let mut operations = self.shared.lock();
            let order = operations.reordered(id, before)?;
            let mut places: Vec<u64> = (order.iter())
                .map(|id| match operations.by_id[id].phase {
                    Phase::Waiting { order, .. } => order,
                    _ => unreachable!("reordered lists waiting operations"),
                })
                .collect();
            places.sort_unstable();
            for (id, place) in order.into_iter().zip(places) {
                if let Some(Tracked {
                    phase: Phase::Waiting { order, .. },
                    ..
                }) = operations.by_id.get_mut(&id)
                {
                    *order = place;
                }
            }
            self.shared.schedule(&mut operations)
        };
        self.shared.start(starts);
        (self.shared.wake)();
        Ok(())
    }

    /// Whether [`Queue::move_waiting`] would move `id`, without moving it.
    pub fn check_move(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<(), OrderRefused> {
        self.shared.lock().reordered(id, before).map(drop)
    }

    /// The taken entries, which the window leaves out of its lists. Take
    /// it once per wake.
    pub fn taken(&self) -> Arc<Taken> {
        Arc::clone(&self.shared.lock().taken)
    }

    /// Exit: stops every operation, a paused one too, so that Continue can
    /// finish it at the next start, and returns at once; the window then
    /// ends the process without dropping the queue. `CopyFile2` deletes what
    /// it wrote when it is stopped, so a file the Windows engine is copying
    /// goes on until the process ends, which leaves it at its final name.
    /// Every operation that runs is already in the saved file.
    pub fn close(&self) {
        self.shared.closing.store(true, Ordering::Relaxed);
        for operation in self.shared.lock().by_id.values() {
            operation.cancel.store(true, Ordering::Relaxed);
            operation.pause.wake();
        }
    }
}

/// Stops every operation as [`Queue::close`] does, cancels the Windows calls
/// its threads wait in, and waits for each to end. A file the Windows engine
/// is copying is then finished, or lost where that cancel stops it.
impl Drop for Queue {
    fn drop(&mut self) {
        self.close();
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

impl Operations {
    /// The waiting operations in run order, with their plans.
    fn waiting(&self) -> Vec<(OperationId, &Plan)> {
        let mut waiting: Vec<(u64, OperationId, &Plan)> = (self.by_id.iter())
            .filter_map(|(&id, operation)| match &operation.phase {
                Phase::Waiting { plan, order, .. } => Some((*order, id, plan)),
                _ => None,
            })
            .collect();
        waiting.sort_unstable_by_key(|&(order, id, _)| (order, id));
        (waiting.into_iter())
            .map(|(_, id, plan)| (id, plan))
            .collect()
    }

    /// The run order with `id` moved before `before`, or last; the one check
    /// of both [`Queue::move_waiting`] and [`Queue::check_move`].
    fn reordered(
        &self,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Result<Vec<OperationId>, OrderRefused> {
        reorder(&self.waiting(), id, before)
    }
}

impl Tracked {
    fn new(request: Arc<Request>, settings: SpeedSettings, phase: Phase) -> Tracked {
        Tracked {
            request,
            settings,
            phase,
            cancel: Arc::default(),
            pause: Arc::default(),
            progress: Arc::default(),
            io: Arc::default(),
            continuing: None,
            submitted: false,
            total: None,
            renames: false,
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Operations> {
        self.operations.lock().expect("no queue thread panics")
    }

    fn spawn(&self, thread: impl FnOnce() + Send + 'static) {
        let mut threads = self.threads.lock().expect("no queue thread panics");
        threads.retain(|thread| !thread.is_finished());
        threads.push(std::thread::spawn(thread));
    }

    /// Prepares the operation `id`, which is preparing, on its own thread.
    fn prepare(shared: &Arc<Shared>, id: OperationId) {
        let (request, settings, continuing, cancel, io) = {
            let operations = shared.lock();
            let operation = &operations.by_id[&id];
            (
                Arc::clone(&operation.request),
                operation.settings,
                operation.continuing,
                Arc::clone(&operation.cancel),
                Arc::clone(&operation.io),
            )
        };
        let thread = Arc::clone(shared);
        shared.spawn(move || {
            let entered = io.enter();
            let first = Rule {
                choice: None,
                run: Run::First { started: None },
            };
            let rule = continuing.unwrap_or(first);
            let prepared = prepare(&request, &settings, rule, &*thread.index, &cancel);
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
                        let outcome = Outcome::ended(End::Refused(refusal));
                        operation.phase = Phase::Ended(Arc::new(outcome));
                        drop(operations);
                        // A refused Continue ends, as any operation does.
                        thread.unsave(id);
                        (thread.wake)();
                        return;
                    }
                    Ok(ready) => match continuing {
                        Some(rule) => thread.enqueue(&mut operations, id, ready, rule.choice),
                        None => {
                            operation.phase = Phase::Prepared(Box::new(ready));
                            Vec::new()
                        }
                    },
                }
            };
            thread.start(starts);
            (thread.wake)();
        });
    }

    /// Submits a prepared operation and starts what may start.
    fn enqueue(
        &self,
        operations: &mut Operations,
        id: OperationId,
        ready: Ready,
        choice: Option<Choice>,
    ) -> Vec<(OperationId, Box<Start>)> {
        let order = operations.next_order;
        operations.next_order += 1;
        let operation = operations.by_id.get_mut(&id).expect("the caller found it");
        operation.submitted = true;
        operation.total = ready.prepared.total;
        operation.renames = match &ready.work {
            Work::Transfer(transfer) => transfer.moving && transfer.copied.is_empty(),
            Work::Rename { .. } => true,
            Work::Delete { .. } | Work::NewFolder { .. } => false,
        };
        operation.phase = Phase::Waiting {
            plan: ready.plan,
            start: Box::new(Start {
                work: ready.work,
                failures: ready.failures,
                choice,
                run: (operation.continuing).map_or(Run::First { started: None }, |rule| rule.run),
            }),
            order,
            on: None,
        };
        let starts = self.schedule(operations);
        operations.taken = taken(operations);
        starts
    }

    /// Starts each waiting operation that no running operation, and no
    /// operation before it in the run order, makes wait, and records what the
    /// others wait for. Nothing starts while the queue closes.
    fn schedule(&self, operations: &mut Operations) -> Vec<(OperationId, Box<Start>)> {
        if self.closing.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let decided = {
            let running: Vec<(OperationId, &Plan)> = (operations.by_id.iter())
                .filter_map(|(&id, operation)| match &operation.phase {
                    Phase::Running { plan, .. } => Some((id, plan)),
                    _ => None,
                })
                .collect();
            let waiting = operations.waiting();
            let on = waits(&running, &waiting);
            (waiting.into_iter().map(|(id, _)| id))
                .zip(on)
                .collect::<Vec<_>>()
        };
        let mut starts = Vec::new();
        for (id, blocker) in decided {
            let started = operations.next_start;
            let operation = operations.by_id.get_mut(&id).expect("listed as waiting");
            if blocker.is_some() {
                if let Phase::Waiting { on, .. } = &mut operation.phase {
                    *on = blocker;
                }
                continue;
            }
            let Phase::Waiting { plan, start, .. } =
                std::mem::replace(&mut operation.phase, Phase::Preparing)
            else {
                unreachable!("listed as waiting");
            };
            operation.phase = Phase::Running { plan, started };
            operations.next_start += 1;
            starts.push((id, start));
        }
        starts
    }

    /// Runs each started operation on its own thread.
    fn start(self: &Arc<Self>, starts: Vec<(OperationId, Box<Start>)>) {
        for (id, mut start) in starts {
            let (request, settings, cancel, pause, progress, io) = {
                let operations = self.lock();
                let operation = &operations.by_id[&id];
                (
                    Arc::clone(&operation.request),
                    operation.settings,
                    Arc::clone(&operation.cancel),
                    Arc::clone(&operation.pause),
                    Arc::clone(&operation.progress),
                    Arc::clone(&operation.io),
                )
            };
            // Before the run writes its first file, and before its thread
            // starts, so a queue that Exit closes has saved every operation
            // that runs.
            let started = match start.run {
                Run::Continue {
                    started: Some(started),
                } => started,
                Run::First { .. } => {
                    let started = system_time();
                    start.run = Run::First {
                        started: Some(started),
                    };
                    started
                }
                Run::Continue { started: None } => system_time(),
            };
            if let Some(saved) = Saved::new(&request, start.choice, started, &settings) {
                self.save(id, saved);
            }
            if let Work::Delete { entries, missing } = &start.work {
                self.count(id, Counting::of(entries, *missing), Arc::clone(&cancel));
            }
            let shared = Arc::clone(self);
            self.spawn(move || {
                let _entered = io.enter();
                let outcome = run(
                    *start,
                    &settings,
                    &cancel,
                    &shared.closing,
                    &pause,
                    &progress,
                    &io,
                );
                if !shared.closing.load(Ordering::Relaxed) {
                    shared.unsave(id);
                }
                let starts = {
                    let mut operations = shared.lock();
                    if let Some(operation) = operations.by_id.get_mut(&id) {
                        operation.phase = Phase::Ended(Arc::new(outcome));
                    }
                    let starts = shared.schedule(&mut operations);
                    operations.taken = taken(&operations);
                    starts
                };
                shared.start(starts);
                (shared.wake)();
            });
        }
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
                    Some(operation) if matches!(operation.phase, Phase::Running { .. }) => {
                        operation.total = Some(total);
                    }
                    _ => return,
                }
            }
            (shared.wake)();
        });
    }

    fn save(&self, id: OperationId, saved: Saved) {
        let mut operations = self.saved.lock().expect("no queue thread panics");
        operations.insert(id, saved);
        // A file that cannot be written loses only the offer to continue.
        let _ = saved::write(&self.saved_path, operations.values());
    }

    fn unsave(&self, id: OperationId) {
        let mut operations = self.saved.lock().expect("no queue thread panics");
        if operations.remove(&id).is_some() {
            let _ = saved::write(&self.saved_path, operations.values());
        }
    }
}

fn current(file: &CurrentFile) -> Current {
    Current {
        source: PathBuf::from(OsString::from_wide(&file.source)),
        destination: file.destination.as_deref().map(plain),
        done: file.done(),
        size: file.size,
    }
}

/// The sources of every submitted delete or move that has not been
/// removed.
fn taken(operations: &Operations) -> Arc<Taken> {
    let mut taken = Taken::default();
    for operation in operations
        .by_id
        .values()
        .filter(|operation| operation.submitted)
    {
        if let Request::Delete { sources } | Request::Move { sources, .. } = &*operation.request {
            for source in sources {
                taken.insert(source);
            }
        }
    }
    Arc::new(taken)
}

/// Runs an operation on its own thread, with its workers.
fn run(
    start: Start,
    settings: &SpeedSettings,
    cancel: &AtomicBool,
    closing: &AtomicBool,
    pause: &Pause,
    progress: &Progress,
    io: &IoThreads,
) -> Outcome {
    let Start {
        work,
        mut failures,
        choice,
        run,
    } = start;
    let rule = Rule { choice, run };
    let mut disk_full = false;
    let mut ring = RingCounts::default();
    match work {
        Work::Delete { entries, missing } => {
            // A selected entry that no longer exists counts as deleted.
            progress.add_items(missing);
            let workers = settings.delete_workers.get();
            failures.extend(walk::delete(entries, workers, cancel, pause, progress, io));
        }
        Work::Transfer(transfer) => {
            match run_transfer(transfer, rule, cancel, closing, pause, progress, io) {
                Ok(copied) => {
                    failures.extend(copied.listed);
                    disk_full = copied.disk_full;
                    ring = copied.ring;
                }
                Err(end) => {
                    return Outcome {
                        end,
                        items: progress.items(),
                        bytes: progress.bytes(),
                        listed: failures,
                        ring,
                    };
                }
            }
        }
        Work::Rename {
            path,
            from,
            to,
            name,
        } => {
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
    let end = if disk_full {
        End::DiskFull
    } else if cancel.load(Ordering::Relaxed) {
        End::Cancelled
    } else {
        End::Complete
    };
    Outcome {
        end,
        items: progress.items(),
        bytes: progress.bytes(),
        listed: failures,
        ring,
    }
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
        missing,
        renamed,
        copied,
        moving,
        plan,
        target: target_volume,
        needed,
    } = transfer;
    // The run compares prepare's byte count with the free space again, and
    // does not count again.
    if let Some(needed) = needed {
        check_space(&destination, needed).map_err(End::Refused)?;
    }
    let mut page = vec![0u64; LISTING_WORDS];
    let names = (destination.names(&mut page))
        .map_err(|error| End::Refused(crate::Refusal::DestinationUnreadable(error)))?;
    let target = Arc::new(Target {
        verbatim: destination.verbatim,
        names: Mutex::new(names),
        made: false,
    });
    let mut listed = Vec::new();
    for path in missing {
        // After a crash Continue meets exactly the entries that already
        // moved; on a first run the engine knows it moved nothing, so a
        // source gone meanwhile is never reported as moved.
        let name = path
            .rsplit(|&unit| unit == BACKSLASH)
            .next()
            .unwrap_or(&path);
        let names = target.names.lock().expect("no worker panics");
        let keep_both = rule.choice == Some(Choice::KeepBoth);
        let numbered = |is_folder| names.numbered(name, is_folder).next().is_some();
        let arrived =
            names.get(name).is_some() || (keep_both && (numbered(false) || numbered(true)));
        drop(names);
        if moving && !matches!(rule.run, Run::First { .. }) && arrived {
            progress.add_items(1);
        } else {
            listed.push(failure(&path, ERROR_FILE_NOT_FOUND));
        }
    }
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
        disk_full: false,
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
