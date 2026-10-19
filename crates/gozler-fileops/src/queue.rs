//! The engine's queue: the one owner of which operations exist, of which
//! wait for each other (D14), of the saved file (D19), and of every engine
//! thread.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use gozler_winfs::{Existing, create_folder, rename};

use crate::conflicts::{Choice, Rule};
use crate::copy::{Copying, copy};
use crate::delete::walk;
use crate::io_threads::IoThreads;
use crate::listing::{BACKSLASH, LISTING_WORDS};
use crate::moves::{Moving, move_by_rename};
use crate::operation::{End, Listed, OperationId, Outcome, Progress, Request, State, Wait};
use crate::place::Target;
use crate::prepare::{Ready, Transfer, Work, check_space, prepare};
use crate::receive::failure;
use crate::saved::{self, Restored, Saved};
use crate::settings::SpeedSettings;
use crate::taken::Taken;
use crate::waits::{Plan, wait};
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
    saved_path: PathBuf,
    /// The operations in the saved file. Its lock also orders the file's
    /// writes.
    saved: Mutex<BTreeMap<OperationId, Saved>>,
    /// Set while the queue is dropped: a run that it cancels stays saved,
    /// so it is offered as interrupted at the next start, which is the
    /// truth.
    closing: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct Operations {
    next_id: u64,
    next_order: u64,
    by_id: BTreeMap<OperationId, Operation>,
    taken: Arc<Taken>,
}

struct Operation {
    request: Arc<Request>,
    settings: SpeedSettings,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    io: Arc<IoThreads>,
    /// Continue's prepare: it submits with the interrupted run's choice as
    /// soon as it is prepared.
    continuing: Option<Option<Choice>>,
    /// It was submitted, so its sources are taken while it lives.
    submitted: bool,
}

enum Phase {
    Preparing,
    Prepared(Box<Ready>),
    Waiting {
        plan: Plan,
        start: Box<Start>,
        /// Its place in submit order.
        order: u64,
        /// Set under the same lock as the phase, by `schedule`.
        on: Option<(OperationId, Wait)>,
    },
    Running {
        plan: Plan,
        order: u64,
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
    continuing: bool,
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
    /// thread after each change of an operation's state.
    pub fn new(saved: impl Into<PathBuf>, wake: impl Fn() + Send + Sync + 'static) -> Queue {
        let saved_path = saved.into();
        let mut operations = Operations::default();
        let mut saved_operations = BTreeMap::new();
        for (saved, restored) in saved::read(&saved_path) {
            let id = OperationId(operations.next_id);
            operations.next_id += 1;
            let operation = Operation::new(
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
            let operation = Operation::new(Arc::new(request), settings, Phase::Preparing);
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
    /// retries.
    pub fn cancel(&self, id: OperationId) {
        let starts = {
            let mut operations = self.shared.lock();
            let Some(operation) = operations.by_id.get_mut(&id) else {
                return;
            };
            operation.cancel.store(true, Ordering::Relaxed);
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
        if let Some(Operation {
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
    /// source's size and last write time is done.
    pub fn resume(&self, id: OperationId) -> Result<(), NotInterrupted> {
        {
            let mut operations = self.shared.lock();
            let operation = operations.by_id.get_mut(&id).ok_or(NotInterrupted)?;
            let Phase::Interrupted(restored) = &operation.phase else {
                return Err(NotInterrupted);
            };
            operation.continuing = Some(restored.choice);
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
            let Some(Operation {
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

    /// Every operation's state, in the order they were prepared.
    pub fn operations(&self) -> Vec<(OperationId, State)> {
        let operations = self.shared.lock();
        (operations.by_id.iter())
            .map(|(&id, operation)| {
                let state = match &operation.phase {
                    Phase::Preparing => State::Preparing,
                    Phase::Prepared(ready) => State::Prepared(ready.prepared),
                    Phase::Waiting { on, .. } => {
                        let (on, reason) = on.expect(
                            "schedule sets it under the same lock; it skips only while the queue drops, when nothing reads states",
                        );
                        State::Waiting { on, reason }
                    }
                    Phase::Running { .. } => State::Running {
                        items: operation.progress.items(),
                        bytes: operation.progress.bytes(),
                    },
                    Phase::Ended(outcome) => State::Ended(Arc::clone(outcome)),
                    Phase::Interrupted(_) => State::Interrupted(Arc::clone(&operation.request)),
                };
                (id, state)
            })
            .collect()
    }

    /// The taken entries, which the window leaves out of its lists. Take
    /// it once per wake.
    pub fn taken(&self) -> Arc<Taken> {
        Arc::clone(&self.shared.lock().taken)
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Relaxed);
        for operation in self.shared.lock().by_id.values() {
            operation.cancel.store(true, Ordering::Relaxed);
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

impl Operation {
    fn new(request: Arc<Request>, settings: SpeedSettings, phase: Phase) -> Operation {
        Operation {
            request,
            settings,
            phase,
            cancel: Arc::default(),
            progress: Arc::default(),
            io: Arc::default(),
            continuing: None,
            submitted: false,
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
            let prepared = prepare(&request, &settings, continuing, &cancel);
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
                        Some(choice) => thread.enqueue(&mut operations, id, ready, choice),
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
        operation.phase = Phase::Waiting {
            plan: ready.plan,
            start: Box::new(Start {
                work: ready.work,
                failures: ready.failures,
                choice,
                continuing: operation.continuing.is_some(),
            }),
            order,
            on: None,
        };
        let starts = self.schedule(operations);
        operations.taken = taken(operations);
        starts
    }

    /// Starts each waiting operation, in submit order, that no earlier
    /// operation which has not ended makes wait, and records what the others
    /// wait for. Nothing starts while the queue closes.
    fn schedule(&self, operations: &mut Operations) -> Vec<(OperationId, Box<Start>)> {
        if self.closing.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut waiting: Vec<(u64, OperationId)> = (operations.by_id.iter())
            .filter_map(|(&id, operation)| match operation.phase {
                Phase::Waiting { order, .. } => Some((order, id)),
                _ => None,
            })
            .collect();
        waiting.sort_unstable();
        let mut starts = Vec::new();
        for (order, id) in waiting {
            let Phase::Waiting { plan, .. } = &operations.by_id[&id].phase else {
                unreachable!("collected as waiting");
            };
            let blocker = (operations.by_id.iter()).find_map(|(&other, operation)| {
                let earlier = match &operation.phase {
                    Phase::Waiting { plan, order: o, .. } | Phase::Running { plan, order: o } => {
                        (*o < order).then_some(plan)
                    }
                    _ => None,
                }?;
                wait(plan, earlier).map(|reason| (other, reason))
            });
            let operation = operations.by_id.get_mut(&id).expect("collected above");
            if blocker.is_some() {
                if let Phase::Waiting { on, .. } = &mut operation.phase {
                    *on = blocker;
                }
                continue;
            }
            let Phase::Waiting { plan, start, .. } =
                std::mem::replace(&mut operation.phase, Phase::Preparing)
            else {
                unreachable!("collected as waiting");
            };
            operation.phase = Phase::Running { plan, order };
            starts.push((id, start));
        }
        starts
    }

    /// Runs each started operation on its own thread.
    fn start(self: &Arc<Self>, starts: Vec<(OperationId, Box<Start>)>) {
        for (id, start) in starts {
            let (request, settings, cancel, progress, io) = {
                let operations = self.lock();
                let operation = &operations.by_id[&id];
                (
                    Arc::clone(&operation.request),
                    operation.settings,
                    Arc::clone(&operation.cancel),
                    Arc::clone(&operation.progress),
                    Arc::clone(&operation.io),
                )
            };
            let shared = Arc::clone(self);
            self.spawn(move || {
                let _entered = io.enter();
                // Before the run changes its first file.
                if let Some(saved) = Saved::new(&request, start.choice, &settings) {
                    shared.save(id, saved);
                }
                let outcome = run(*start, &settings, &cancel, &progress, &io);
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
    progress: &Progress,
    io: &IoThreads,
) -> Outcome {
    let Start {
        work,
        mut failures,
        choice,
        continuing,
    } = start;
    let rule = Rule { choice, continuing };
    let mut disk_full = false;
    match work {
        Work::Delete { entries, missing } => {
            // A selected entry that no longer exists counts as deleted.
            progress.add_items(missing);
            let workers = settings.delete_workers.get();
            failures.extend(walk::delete(entries, workers, cancel, progress, io));
        }
        Work::Transfer(transfer) => match run_transfer(transfer, rule, cancel, progress, io) {
            Ok((listed, full)) => {
                failures.extend(listed);
                disk_full = full;
            }
            Err(end) => {
                return Outcome {
                    end,
                    items: progress.items(),
                    bytes: progress.bytes(),
                    listed: failures,
                };
            }
        },
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
    }
}

/// Runs a copy or a move. Returns what it lists and whether the disk
/// filled, or how it ended before it changed anything.
fn run_transfer(
    transfer: Transfer,
    rule: Rule,
    cancel: &AtomicBool,
    progress: &Progress,
    io: &IoThreads,
) -> Result<(Vec<Listed>, bool), End> {
    let Transfer {
        destination,
        missing,
        renamed,
        copied,
        moving,
        plan,
        sparse,
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
        if moving && rule.continuing && arrived {
            progress.add_items(1);
        } else {
            listed.push(failure(&path, ERROR_FILE_NOT_FOUND));
        }
    }
    if !renamed.is_empty() {
        let moves = Moving {
            rule,
            cancel,
            progress,
        };
        listed.extend(move_by_rename(renamed, &target, &moves));
    }
    let mut disk_full = false;
    if !copied.is_empty() && !cancel.load(Ordering::Relaxed) {
        let copying = Copying {
            plan,
            rule,
            moving,
            sparse,
            cancel,
            progress,
            io,
        };
        let copied = copy(copied, target, &copying);
        listed.extend(copied.listed);
        disk_full = copied.disk_full;
    }
    Ok((listed, disk_full))
}
