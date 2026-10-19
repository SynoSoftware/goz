//! The engine's queue: the one owner of which operations exist, and of
//! every engine thread.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::delete::walk;
use crate::operation::{End, OperationId, Outcome, Progress, Request, State};
use crate::receive::{Received, receive};
use crate::settings::SpeedSettings;

/// The queue the window talks to. Dropping it cancels every operation and
/// joins every thread, so no engine thread outlives it.
pub struct Queue {
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

struct Shared {
    operations: Mutex<Operations>,
    /// Called after each change of an operation's state, never under the
    /// queue's lock, so it may read the states at once.
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Operations {
    next_id: u64,
    by_id: BTreeMap<OperationId, Operation>,
}

struct Operation {
    request: Request,
    settings: SpeedSettings,
    phase: Phase,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
}

enum Phase {
    Preparing,
    Prepared(Received),
    Running,
    Ended(Arc<Outcome>),
}

/// A submit for an operation that is not prepared: it does not exist, or
/// it is still preparing, already running or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operation is not prepared")]
pub struct NotPrepared;

impl Queue {
    /// `wake` is called on an engine thread after each change of an
    /// operation's state.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Queue {
        Queue {
            shared: Arc::new(Shared {
                operations: Mutex::default(),
                wake: Box::new(wake),
            }),
            threads: Mutex::default(),
        }
    }

    /// Starts preparing `request` on a queue thread and returns its id at
    /// once.
    pub fn prepare(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let cancel = Arc::new(AtomicBool::new(false));
        let id = {
            let mut operations = self.shared.lock();
            let id = OperationId(operations.next_id);
            operations.next_id += 1;
            operations.by_id.insert(
                id,
                Operation {
                    request: request.clone(),
                    settings,
                    phase: Phase::Preparing,
                    cancel: Arc::clone(&cancel),
                    progress: Arc::default(),
                },
            );
            id
        };
        let shared = Arc::clone(&self.shared);
        self.spawn(move || {
            let received = match &request {
                Request::Delete { sources } => receive(sources, None, &cancel),
            };
            let mut operations = shared.lock();
            if let Some(operation) = operations.by_id.get_mut(&id)
                && matches!(operation.phase, Phase::Preparing)
            {
                operation.phase =
                    Phase::Prepared(received.expect("a delete has no destination to refuse"));
            }
            drop(operations);
            (shared.wake)();
        });
        self.wake();
        id
    }

    /// Starts a prepared operation.
    pub fn submit(&self, id: OperationId) -> Result<(), NotPrepared> {
        let (received, request, settings, cancel, progress) = {
            let mut operations = self.shared.lock();
            let operation = operations.by_id.get_mut(&id).ok_or(NotPrepared)?;
            if !matches!(operation.phase, Phase::Prepared(_)) {
                return Err(NotPrepared);
            }
            let Phase::Prepared(received) = std::mem::replace(&mut operation.phase, Phase::Running)
            else {
                unreachable!("the phase was just matched");
            };
            (
                received,
                operation.request.clone(),
                operation.settings,
                Arc::clone(&operation.cancel),
                Arc::clone(&operation.progress),
            )
        };
        let shared = Arc::clone(&self.shared);
        self.spawn(move || {
            let outcome = run(received, &request, &settings, &cancel, &progress);
            if let Some(operation) = shared.lock().by_id.get_mut(&id) {
                operation.phase = Phase::Ended(Arc::new(outcome));
            }
            (shared.wake)();
        });
        self.wake();
        Ok(())
    }

    /// Cancels an operation. One that has not started ends at once; a
    /// running one stops within one delete and its retries.
    pub fn cancel(&self, id: OperationId) {
        let mut operations = self.shared.lock();
        let Some(operation) = operations.by_id.get_mut(&id) else {
            return;
        };
        operation.cancel.store(true, Ordering::Relaxed);
        if matches!(operation.phase, Phase::Preparing | Phase::Prepared(_)) {
            operation.phase = Phase::Ended(Arc::new(Outcome {
                end: End::Cancelled,
                items: 0,
                failures: Vec::new(),
            }));
            drop(operations);
            self.wake();
        }
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
        }
    }

    /// Every operation's state, in the order they were prepared.
    pub fn operations(&self) -> Vec<(OperationId, State)> {
        let operations = self.shared.lock();
        (operations.by_id.iter())
            .map(|(&id, operation)| {
                let state = match &operation.phase {
                    Phase::Preparing => State::Preparing,
                    Phase::Prepared(received) => State::Prepared {
                        entries: received.entries.len(),
                    },
                    Phase::Running => State::Running {
                        items: operation.progress.items(),
                    },
                    Phase::Ended(outcome) => State::Ended(Arc::clone(outcome)),
                };
                (id, state)
            })
            .collect()
    }

    fn spawn(&self, thread: impl FnOnce() + Send + 'static) {
        let mut threads = self.threads.lock().expect("no queue thread panics");
        threads.retain(|thread| !thread.is_finished());
        threads.push(std::thread::spawn(thread));
    }

    fn wake(&self) {
        (self.shared.wake)();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Operations> {
        self.operations.lock().expect("no queue thread panics")
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        for operation in self.shared.lock().by_id.values() {
            operation.cancel.store(true, Ordering::Relaxed);
        }
        let threads = std::mem::take(self.threads.get_mut().expect("no queue thread panics"));
        for thread in threads {
            let _ = thread.join();
        }
    }
}

/// Runs an operation on its own thread, with its workers.
fn run(
    received: Received,
    request: &Request,
    settings: &SpeedSettings,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Outcome {
    let mut failures = received.failures;
    match request {
        Request::Delete { .. } => {
            // A selected entry that no longer exists counts as deleted.
            progress.add_items(received.missing);
            let workers = settings.delete_workers.get();
            failures.extend(walk::delete(received.entries, workers, cancel, progress));
        }
    }
    Outcome {
        end: if cancel.load(Ordering::Relaxed) {
            End::Cancelled
        } else {
            End::Complete
        },
        items: progress.items(),
        failures,
    }
}
