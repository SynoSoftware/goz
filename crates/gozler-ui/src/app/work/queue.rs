//! Ordered access to Queue from the common work owner. The UI retains published
//! snapshots and reserves ids; only the session worker calls engine decisions.

use crate::core::file_operations::ExitCounts;
use crate::core::work::{INLINE_ITEMS, Retirement};
use crate::win::wake::OpenWaker;
use crate::win::work::OffThread;
use gozler_fileops::{
    Comparison, Operation, OperationId, OrderRefused, Queue, Request, ReservedId, SpeedSettings,
    Taken, TransferLimit,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};

const BATCH: usize = 32;

#[derive(Clone, Copy)]
pub(in crate::app) enum CheckPurpose {
    Up,
    Down,
    Drag,
}

impl CheckPurpose {
    fn index(self) -> usize {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::Drag => 2,
        }
    }
}

#[derive(Clone, Copy)]
struct Check {
    serial: u64,
    revision: u64,
    id: OperationId,
    before: Option<OperationId>,
}

enum Command {
    Prepare(ReservedId, Request, SpeedSettings),
    Accept(ReservedId, Request, SpeedSettings),
    Resolve(OperationId, Request),
    FailIntake(OperationId),
    Submit(OperationId),
    Synchronize(OperationId),
    Cancel(OperationId),
    CancelAll,
    Remove(OperationId),
    Pause(OperationId, bool),
    Continue(OperationId),
    Dismiss(OperationId),
    Move(OperationId, Option<OperationId>),
    Limit(TransferLimit),
    Release(Arc<Comparison>),
    Check(CheckPurpose),
    Refresh,
    Close(Sender<io::Result<()>>),
}

pub(in crate::app) enum Accepted {
    Resolved(OperationId, bool),
    Submitted(OperationId, bool),
    Moved(OperationId, bool),
}

enum Reply {
    Accepted(Accepted),
    Created(OperationId),
    Synchronized(OperationId, bool),
    Checked(CheckPurpose, Check, Result<(), OrderRefused>),
}

struct Update {
    snapshot: Option<Arc<Snapshot>>,
    replies: Vec<Reply>,
}

struct Snapshot {
    operations: Arc<[Operation]>,
    by_id: HashMap<OperationId, usize>,
    taken: Arc<Taken>,
    exit: ExitCounts,
}

impl Snapshot {
    fn read(queue: &Queue) -> Self {
        // Taken precedes operations, matching its established observation contract.
        let taken = queue.taken();
        let operations: Arc<[Operation]> = queue.operations().into();
        let by_id = operations
            .iter()
            .enumerate()
            .map(|(at, operation)| (operation.id, at))
            .collect();
        Self {
            exit: ExitCounts::of(&operations),
            operations,
            by_id,
            taken,
        }
    }
}

pub(in crate::app) struct Ready {
    seed: Seed,
    session: Session,
}

struct Seed {
    queue: Arc<ManuallyDrop<Queue>>,
    sender: Sender<Command>,
    refresh: Arc<AtomicBool>,
    checks: [SyncSender<Check>; 3],
    replies: Receiver<Update>,
    snapshot: Arc<Snapshot>,
    waker: OpenWaker,
}

pub(in crate::app) struct Session {
    // Dropping Queue joins native transfers and can remove partial output;
    // process exit must instead preserve that output for Continue.
    queue: Arc<ManuallyDrop<Queue>>,
    commands: Receiver<Command>,
    refresh: Arc<AtomicBool>,
    checks: [Receiver<Check>; 3],
    replies: SyncSender<Update>,
    waker: OpenWaker,
}

pub(in crate::app) fn restore(path: PathBuf, waker: OpenWaker, _worker: &OffThread) -> Ready {
    let (sender, commands) = channel();
    let refresh = Arc::new(AtomicBool::new(false));
    let wake_sender = sender.clone();
    let wake_refresh = Arc::clone(&refresh);
    let queue = Arc::new(ManuallyDrop::new(Queue::new(path, move || {
        request_refresh(&wake_sender, &wake_refresh);
    })));
    let snapshot = Arc::new(Snapshot::read(&queue));
    let (replies, incoming) = sync_channel(1);
    let [(up, up_read), (down, down_read), (drag, drag_read)] =
        std::array::from_fn(|_| sync_channel(1));
    Ready {
        seed: Seed {
            queue: Arc::clone(&queue),
            sender,
            refresh: Arc::clone(&refresh),
            checks: [up, down, drag],
            replies: incoming,
            snapshot,
            waker: waker.clone(),
        },
        session: Session {
            queue,
            commands,
            refresh,
            checks: [up_read, down_read, drag_read],
            replies,
            waker,
        },
    }
}

fn request_refresh(sender: &Sender<Command>, pending: &AtomicBool) {
    if !pending.swap(true, Ordering::AcqRel) {
        let _ = sender.send(Command::Refresh);
    }
}

impl Session {
    pub(in crate::app) fn run(self, _worker: &OffThread) {
        while let Ok(first) = self.commands.recv() {
            let mut command = first;
            let mut publish = false;
            let mut replies = Vec::new();
            for index in 0..BATCH {
                let refresh_snapshot = !matches!(&command, Command::Release(_) | Command::Check(_));
                match command {
                    Command::Close(saved) => {
                        let result = self.queue.close().recv().unwrap_or_else(|_| {
                            Err(io::Error::other(
                                "the recovery writer ended without acknowledging",
                            ))
                        });
                        let _ = saved.send(result);
                        return;
                    }
                    Command::Prepare(id, request, settings) => {
                        let created = id.id();
                        self.queue.prepare_reserved(id, request, settings);
                        replies.push(Reply::Created(created));
                    }
                    Command::Accept(id, request, settings) => {
                        let created = id.id();
                        self.queue.accept_reserved(id, request, settings);
                        replies.push(Reply::Created(created));
                    }
                    Command::Resolve(id, request) => {
                        let accepted = self.queue.resolve(id, request);
                        replies.push(Reply::Accepted(Accepted::Resolved(id, accepted)));
                    }
                    Command::FailIntake(id) => self.queue.fail_intake(id),
                    Command::Submit(id) => {
                        let accepted = self.queue.submit(id).is_ok();
                        replies.push(Reply::Accepted(Accepted::Submitted(id, accepted)));
                    }
                    Command::Synchronize(id) => {
                        let accepted = self.queue.synchronize(id).is_ok();
                        replies.push(Reply::Synchronized(id, accepted));
                    }
                    Command::Cancel(id) => self.queue.cancel(id),
                    Command::CancelAll => self.queue.cancel_all(),
                    Command::Remove(id) => self.queue.remove(id),
                    Command::Pause(id, paused) => self.queue.set_paused(id, paused),
                    Command::Continue(id) => {
                        let _ = self.queue.resume(id);
                    }
                    Command::Dismiss(id) => {
                        let _ = self.queue.dismiss(id);
                    }
                    Command::Move(id, before) => {
                        let accepted = self.queue.move_waiting(id, before).is_ok();
                        replies.push(Reply::Accepted(Accepted::Moved(id, accepted)));
                    }
                    Command::Limit(limit) => self.queue.set_transfer_limit(limit),
                    Command::Release(comparison) => {
                        drop(comparison);
                    }
                    Command::Check(purpose) => {
                        if let Ok(check) = self.checks[purpose.index()].try_recv() {
                            let answer = self.queue.check_move(check.id, check.before);
                            replies.push(Reply::Checked(purpose, check, answer));
                        }
                    }
                    Command::Refresh => {
                        self.refresh.store(false, Ordering::Release);
                    }
                }
                publish |= refresh_snapshot;
                if index + 1 < BATCH
                    && let Ok(next) = self.commands.try_recv()
                {
                    command = next;
                } else {
                    break;
                }
            }
            if publish || !replies.is_empty() {
                let snapshot = publish.then(|| Arc::new(Snapshot::read(&self.queue)));
                let _ = self.replies.send(Update { snapshot, replies });
                self.waker.deliver(|| true);
            }
        }
    }
}

struct CheckAnswer {
    check: Check,
    result: Option<Result<(), OrderRefused>>,
}

struct Cache {
    snapshot: Arc<Snapshot>,
    revision: u64,
    serial: u64,
    checks: [Option<CheckAnswer>; 3],
    pending: HashSet<OperationId>,
    synchronized: HashMap<OperationId, Option<bool>>,
    retired: Retirement,
}

#[derive(Clone)]
pub(in crate::app) struct Client {
    queue: Arc<ManuallyDrop<Queue>>,
    sender: Sender<Command>,
    refresh: Arc<AtomicBool>,
    checks: [SyncSender<Check>; 3],
    replies: Rc<RefCell<Option<Receiver<Update>>>>,
    cache: Rc<RefCell<Cache>>,
    closed: Rc<Cell<bool>>,
    waker: OpenWaker,
}

impl Ready {
    pub(in crate::app) fn adopt(self) -> (Client, Session) {
        let Seed {
            queue,
            sender,
            refresh,
            checks,
            replies,
            snapshot,
            waker,
        } = self.seed;
        let client = Client {
            queue,
            sender,
            refresh,
            checks,
            replies: Rc::new(RefCell::new(Some(replies))),
            cache: Rc::new(RefCell::new(Cache {
                snapshot,
                revision: 0,
                serial: 0,
                checks: Default::default(),
                pending: HashSet::new(),
                synchronized: HashMap::new(),
                retired: Retirement::default(),
            })),
            closed: Rc::new(Cell::new(false)),
            waker,
        };
        (client, self.session)
    }
}

impl Client {
    fn send(&self, command: Command) {
        if !self.closed.get() {
            self.sender
                .send(command)
                .unwrap_or_else(|_| panic!("the queue session owns accepted commands until close"));
        }
    }

    pub(in crate::app) fn update(&self) -> Vec<Accepted> {
        let mut found = Vec::new();
        let update = self
            .replies
            .borrow()
            .as_ref()
            .and_then(|replies| replies.try_recv().ok());
        if let Some(update) = update {
            if let Some(snapshot) = update.snapshot {
                let mut cache = self.cache.borrow_mut();
                let old = std::mem::replace(&mut cache.snapshot, snapshot);
                cache.revision += 1;
                cache.retired.push(old, INLINE_ITEMS + 1);
            }
            for reply in update.replies {
                match reply {
                    Reply::Checked(purpose, check, result) => {
                        let mut cache = self.cache.borrow_mut();
                        if check.revision == cache.revision
                            && let Some(current) = &mut cache.checks[purpose.index()]
                            && current.check.serial == check.serial
                        {
                            current.result = Some(result);
                        }
                    }
                    Reply::Synchronized(id, accepted) => {
                        if let Some(answer) = self.cache.borrow_mut().synchronized.get_mut(&id) {
                            *answer = Some(accepted);
                        }
                    }
                    Reply::Created(id) => {
                        self.cache.borrow_mut().pending.remove(&id);
                    }
                    Reply::Accepted(reply) => found.push(reply),
                }
            }
        }
        found
    }

    pub(in crate::app) fn take_retired(&self) -> Retirement {
        std::mem::take(&mut self.cache.borrow_mut().retired)
    }
    pub(in crate::app) fn snapshot(&self) -> Arc<[Operation]> {
        Arc::clone(&self.cache.borrow().snapshot.operations)
    }
    pub(in crate::app) fn taken(&self) -> Arc<Taken> {
        Arc::clone(&self.cache.borrow().snapshot.taken)
    }
    pub(in crate::app) fn exit_counts(&self) -> ExitCounts {
        let cache = self.cache.borrow();
        cache.snapshot.exit.with_pending(cache.pending.len())
    }
    pub(in crate::app) fn operation(&self, id: OperationId) -> Option<Operation> {
        let cache = self.cache.borrow();
        cache
            .snapshot
            .by_id
            .get(&id)
            .map(|&at| cache.snapshot.operations[at].clone())
    }
    pub(in crate::app) fn is_pending(&self, id: OperationId) -> bool {
        self.cache.borrow().pending.contains(&id)
    }
    pub(in crate::app) fn refresh(&self) {
        if !self.closed.get() {
            request_refresh(&self.sender, &self.refresh);
        }
    }

    pub(in crate::app) fn prepare(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let reserved = self.queue.reserve_id();
        let id = reserved.id();
        self.cache.borrow_mut().pending.insert(id);
        self.send(Command::Prepare(reserved, request, settings));
        id
    }
    pub(in crate::app) fn accept(&self, request: Request, settings: SpeedSettings) -> OperationId {
        let reserved = self.queue.reserve_id();
        let id = reserved.id();
        self.cache.borrow_mut().pending.insert(id);
        self.send(Command::Accept(reserved, request, settings));
        id
    }
    pub(in crate::app) fn resolve(&self, id: OperationId, request: Request) {
        self.send(Command::Resolve(id, request));
    }
    pub(in crate::app) fn fail_intake(&self, id: OperationId) {
        self.send(Command::FailIntake(id));
    }
    pub(in crate::app) fn submit(&self, id: OperationId) {
        self.send(Command::Submit(id));
    }
    pub(in crate::app) fn synchronize(&self, id: OperationId) {
        self.cache.borrow_mut().synchronized.insert(id, None);
        self.send(Command::Synchronize(id));
    }
    pub(in crate::app) fn take_synchronized(&self, id: OperationId) -> Option<bool> {
        let mut cache = self.cache.borrow_mut();
        let answer = cache.synchronized.get(&id).copied().flatten()?;
        cache.synchronized.remove(&id);
        Some(answer)
    }
    pub(in crate::app) fn cancel(&self, id: OperationId) {
        self.cache.borrow_mut().synchronized.remove(&id);
        self.send(Command::Cancel(id));
    }
    pub(in crate::app) fn cancel_all(&self) {
        self.send(Command::CancelAll);
    }
    pub(in crate::app) fn remove(&self, id: OperationId) {
        self.cache.borrow_mut().synchronized.remove(&id);
        self.send(Command::Remove(id));
    }
    pub(in crate::app) fn set_paused(&self, id: OperationId, paused: bool) {
        self.send(Command::Pause(id, paused));
    }
    pub(in crate::app) fn resume(&self, id: OperationId) {
        self.send(Command::Continue(id));
    }
    pub(in crate::app) fn dismiss(&self, id: OperationId) {
        self.send(Command::Dismiss(id));
    }
    pub(in crate::app) fn move_waiting(&self, id: OperationId, before: Option<OperationId>) {
        self.send(Command::Move(id, before));
    }
    pub(in crate::app) fn set_transfer_limit(&self, limit: TransferLimit) {
        self.send(Command::Limit(limit));
    }
    pub(in crate::app) fn release_comparison(&self, comparison: Arc<Comparison>) {
        self.send(Command::Release(comparison));
    }

    pub(in crate::app) fn check_move(
        &self,
        purpose: CheckPurpose,
        id: OperationId,
        before: Option<OperationId>,
    ) -> Option<Result<(), OrderRefused>> {
        let mut cache = self.cache.borrow_mut();
        let index = purpose.index();
        if let Some(known) = &cache.checks[index]
            && known.check.revision == cache.revision
            && known.check.id == id
            && known.check.before == before
        {
            return known.result;
        }
        cache.serial += 1;
        let check = Check {
            serial: cache.serial,
            revision: cache.revision,
            id,
            before,
        };
        match self.checks[index].try_send(check) {
            Ok(()) => {
                cache.checks[index] = Some(CheckAnswer {
                    check,
                    result: None,
                });
                self.send(Command::Check(purpose));
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                return None;
            }
        }
        None
    }

    pub(in crate::app) fn close(&self) -> Receiver<io::Result<()>> {
        self.waker.close();
        let mut cache = self.cache.borrow_mut();
        if let Some(replies) = self.replies.borrow_mut().take() {
            cache.retired.push(replies, INLINE_ITEMS + 1);
        }
        let empty = Arc::new(Snapshot {
            operations: Arc::from([]),
            by_id: HashMap::new(),
            taken: Arc::default(),
            exit: ExitCounts::default(),
        });
        let snapshot = std::mem::replace(&mut cache.snapshot, empty);
        cache.retired.push(snapshot, INLINE_ITEMS + 1);
        let pending = std::mem::take(&mut cache.pending);
        let synchronized = std::mem::take(&mut cache.synchronized);
        cache.retired.push(pending, INLINE_ITEMS + 1);
        cache.retired.push(synchronized, INLINE_ITEMS + 1);
        let (saved, answer) = channel();
        self.send(Command::Close(saved));
        self.closed.set(true);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::work;
    use gozler_fileops::{End, Refusal, State};
    use std::time::Duration;

    #[test]
    fn terminal_close_drains_accepted_commands_with_a_full_delivery_slot() {
        let waker = OpenWaker::closed();
        let ready = work::spawn(move |worker| restore(PathBuf::new(), waker, worker))
            .join()
            .unwrap();
        let (client, session) = ready.adopt();
        session
            .replies
            .send(Update {
                snapshot: None,
                replies: Vec::new(),
            })
            .ok()
            .unwrap();
        let engine = Arc::clone(&client.queue);
        let id = client.accept(
            Request::Delete {
                sources: vec![PathBuf::from(r"C:\selected")],
            },
            SpeedSettings::automatic(),
        );
        for _ in 0..BATCH {
            client.set_paused(id, false);
        }
        client.fail_intake(id);
        let running = work::spawn(move |worker| session.run(worker));
        let saved = client.close();
        let retired = client.take_retired();
        work::spawn(move |_| drop(retired)).join().unwrap();
        saved
            .recv_timeout(Duration::from_secs(5))
            .expect("terminal close reaches recovery despite full UI delivery")
            .unwrap();
        running.join().unwrap();
        let operation = engine
            .operation(id)
            .expect("accepted intake survives closing delivery");
        assert!(
            matches!(operation.state, State::Ended(ref outcome) if outcome.end == End::Refused(Refusal::SourceUnavailable))
        );
        drop(operation);
        drop(client);
        let queue = Arc::try_unwrap(engine)
            .ok()
            .expect("the session released its engine reference");
        drop(ManuallyDrop::into_inner(queue));
    }
}
