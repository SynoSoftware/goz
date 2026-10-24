//! Bounded UI workers. Each slot retains one executing call and one pending
//! input; native cancellation is best-effort and never substitutes for completion.

use super::wake::OpenWaker;
use crate::core::work::{
    self, Id, Interest, Pass, Replacement, Retirement, Slot, Started, Waiting,
};
use gozler_client::CancelToken;
use std::marker::PhantomData;
use std::os::windows::io::AsRawHandle;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::CancelSynchronousIo;

mod shell;

/// Constructed only inside a worker; neither UI code nor an inline CPU pass
/// can acquire the capability to call a marked blocking function.
pub(crate) struct OffThread(PhantomData<Rc<()>>);

pub(crate) fn spawn<T: Send + 'static>(
    run: impl FnOnce(&OffThread) -> T + Send + 'static,
) -> JoinHandle<T> {
    std::thread::spawn(move || run(&OffThread(PhantomData)))
}

struct Running {
    cancel: CancelToken,
    thread: Arc<JoinHandle<()>>,
}

impl Running {
    fn cancel(&self) {
        self.cancel.cancel();
        self.interrupt_io();
    }

    fn interrupt_io(&self) {
        // SAFETY: the JoinHandle keeps this native thread handle valid.
        let _ = unsafe { CancelSynchronousIo(HANDLE(self.thread.as_raw_handle())) };
    }
}

pub(crate) struct Task<I, O> {
    slot: Slot<I>,
    running: Option<Running>,
    sender: SyncSender<(Id, O)>,
    replies: Receiver<(Id, O)>,
    waker: OpenWaker,
    execution: Execution<I, O>,
    input_size: fn(&I) -> usize,
    active_items: usize,
    retired: Retirement,
    interest: Interest,
    since: Option<Instant>,
}

enum Execution<I, O> {
    Worker(fn(I, &OffThread, &CancelToken) -> O),
    Pass(fn(I, &CancelToken) -> O),
    Shell {
        run: fn(I, &super::shell::Session, &OffThread, &CancelToken) -> O,
        thread: Option<shell::Thread<I>>,
    },
}

impl<I: Send + 'static, O: Send + 'static> Task<I, O> {
    pub(crate) fn new(
        waker: OpenWaker,
        replacement: Replacement,
        run: fn(I, &OffThread, &CancelToken) -> O,
    ) -> Self {
        Self::with_execution(waker, replacement, Execution::Worker(run))
    }

    pub(crate) fn with_size(mut self, items: fn(&I) -> usize) -> Self {
        self.input_size = items;
        self
    }

    pub(crate) fn background(mut self) -> Self {
        self.interest = Interest::Background;
        self
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<Waiting> {
        let waiting = self.waiting(now)?;
        if waiting == Waiting::Expired {
            self.cancel();
        }
        Some(waiting)
    }

    pub(crate) fn waiting(&self, now: Instant) -> Option<Waiting> {
        self.since
            .map(|since| self.interest.after(now.saturating_duration_since(since)))
    }

    /// Call only for progress from the current input, after checking its identity.
    pub(crate) fn progress(&mut self, now: Instant) {
        if let Some(since) = &mut self.since {
            *since = now;
        }
    }

    pub(crate) fn pass(
        waker: OpenWaker,
        replacement: Replacement,
        items: fn(&I) -> usize,
        run: fn(I, &CancelToken) -> O,
    ) -> Self {
        Self::with_execution(waker, replacement, Execution::Pass(run)).with_size(items)
    }

    pub(crate) fn shell(
        waker: OpenWaker,
        replacement: Replacement,
        run: fn(I, &super::shell::Session, &OffThread, &CancelToken) -> O,
    ) -> Self {
        Self::with_execution(waker, replacement, Execution::Shell { run, thread: None })
    }

    fn with_execution(
        waker: OpenWaker,
        replacement: Replacement,
        execution: Execution<I, O>,
    ) -> Self {
        let (sender, replies) = sync_channel(1);
        Self {
            slot: Slot::new(replacement),
            running: None,
            sender,
            replies,
            waker,
            execution,
            input_size: |_| 0,
            active_items: 0,
            retired: Retirement::default(),
            interest: Interest::Awaited,
            since: None,
        }
    }

    pub(crate) fn submit(&mut self, input: I, combine: impl FnOnce(I, I) -> I) -> Id {
        self.submit_with(input, |old, new, _| combine(old, new))
    }

    pub(crate) fn replace(&mut self, input: I) -> Id {
        let size = self.input_size;
        self.submit_with(input, |old, new, retired| {
            let items = size(&old);
            retired.push(old, items);
            new
        })
    }

    fn submit_with(&mut self, input: I, combine: impl FnOnce(I, I, &mut Retirement) -> I) -> Id {
        self.since = Some(Instant::now());
        if self.slot.replacement() == Replacement::Cancel
            && let Some(running) = &self.running
        {
            running.cancel();
        }
        let retired = &mut self.retired;
        if let Some(started) = self
            .slot
            .submit(input, |old, new| combine(old, new, retired))
        {
            self.start(started);
        }
        if self.running.is_some() {
            self.waker.deliver(|| true);
        }
        self.slot
            .current()
            .expect("submitted work has a current id")
    }

    pub(crate) fn cancel(&mut self) {
        self.since = None;
        if let Some(input) = self.slot.cancel() {
            let items = (self.input_size)(&input);
            self.retired.push(input, items);
        }
        if let Some(running) = &self.running {
            running.cancel();
        }
    }

    /// Interrupts a native call without ending a persistent worker's lifetime.
    pub(crate) fn interrupt_io(&self) {
        if let Some(running) = &self.running {
            running.interrupt_io();
        }
    }

    pub(crate) fn take(&mut self) -> Option<(Id, O)> {
        let (id, output) = self.replies.try_recv().ok()?;
        self.complete(id, output).map(|output| (id, output))
    }

    pub(crate) fn is_current(&self, id: Id) -> bool {
        self.slot.is_current(id)
    }

    pub(crate) fn current(&self) -> Option<Id> {
        self.slot.current()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.slot.is_running()
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        std::mem::take(&mut self.retired)
    }

    /// After cancellation, dispose of queued and racing replies off UI.
    /// This task must not be submitted or read again.
    pub(crate) fn close_delivery(&mut self) -> Retirement {
        let (_, empty) = sync_channel(1);
        let replies = std::mem::replace(&mut self.replies, empty);
        let mut retired = self.take_retired();
        retired.push(replies, work::INLINE_ITEMS + 1);
        retired
    }

    /// Waits off UI without releasing the worker's apartment. An obsolete
    /// completion can return None while admitting the current pending input.
    pub(crate) fn wait(&mut self, _worker: &OffThread) -> Option<(Id, O)> {
        if !self.slot.is_running() {
            return None;
        }
        let (id, output) = self.replies.recv().ok()?;
        self.complete(id, output).map(|output| (id, output))
    }

    /// Essential finalization only, on a shutdown worker after delivery closes.
    /// It drains the admitted write and its coalesced successor without a UI join.
    pub(crate) fn finish(mut self, _worker: &OffThread) {
        while self.slot.is_running() {
            let Ok((id, output)) = self.replies.recv() else {
                break;
            };
            let _ = self.complete(id, output);
        }
    }

    /// Keeps an accepted mutation alive at exit without admitting pending reads.
    pub(crate) fn finish_current(mut self, worker: &OffThread) {
        drop(self.slot.cancel());
        self.finish(worker);
    }

    fn complete(&mut self, id: Id, output: O) -> Option<O> {
        let items = self.active_items;
        self.running.take();
        let (accepted, next) = self.slot.complete(id);
        if !self.slot.is_running() {
            self.since = None;
        }
        if let Some(next) = next {
            self.start(next);
        }
        if accepted {
            Some(output)
        } else {
            self.retired.push(output, items);
            None
        }
    }

    fn start(&mut self, started: Started<I>) {
        self.active_items = (self.input_size)(&started.input);
        let cancel = CancelToken::default();
        let worker_cancel = cancel.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        let thread = match &mut self.execution {
            Execution::Worker(run) => {
                let run = *run;
                Some(Arc::new(spawn(move |capability| {
                    let output = run(started.input, capability, &worker_cancel);
                    deliver(&sender, &waker, started.id, output);
                })))
            }
            Execution::Pass(run) => {
                let run = *run;
                match work::pass(self.active_items, started.input, |input| {
                    run(input, &worker_cancel)
                }) {
                    Pass::Ready(output) => {
                        deliver(&sender, &waker, started.id, output);
                        None
                    }
                    Pass::Deferred(input) => Some(Arc::new(spawn(move |_| {
                        let output = run(input, &worker_cancel);
                        deliver(&sender, &waker, started.id, output);
                    }))),
                }
            }
            Execution::Shell { run, thread } => {
                let thread = thread.get_or_insert_with(|| shell::Thread::new(*run, sender, waker));
                Some(thread.submit(started, worker_cancel))
            }
        };
        self.running = thread.map(|thread| Running { cancel, thread });
    }
}

fn deliver<O>(sender: &SyncSender<(Id, O)>, waker: &OpenWaker, id: Id, output: O) {
    if sender.try_send((id, output)).is_ok() {
        waker.deliver(|| true);
    }
}

impl<I, O> Drop for Task<I, O> {
    fn drop(&mut self) {
        if let Some(running) = &self.running {
            running.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows::Win32::Foundation::{HWND, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;

    #[test]
    fn progress_delays_expiry_without_releasing_an_unfinished_slot() {
        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut task = Task::pass(waker, Replacement::Cancel, |()| 0, |(), _| ()).background();
        let id = task.replace(());
        let start = Instant::now();
        task.since = Some(start);
        task.progress(start + Duration::from_secs(9));
        assert_eq!(
            task.tick(start + Duration::from_secs(10)),
            Some(Waiting::Working)
        );
        assert_eq!(
            task.tick(start + Duration::from_secs(19)),
            Some(Waiting::Expired)
        );
        assert!(task.is_running());
        assert!(!task.is_current(id));
        task.progress(start + Duration::from_secs(20));
        assert_eq!(task.tick(start + Duration::from_secs(20)), None);
        assert!(task.take().is_none());
        assert!(!task.is_running());
    }

    #[test]
    fn closing_delivery_releases_a_queued_answer_off_the_window_thread() {
        struct Answer(std::sync::mpsc::Sender<std::thread::ThreadId>);
        impl Drop for Answer {
            fn drop(&mut self) {
                let _ = self.0.send(std::thread::current().id());
            }
        }
        let (sender, dropped) = std::sync::mpsc::channel();
        let calling = std::thread::current().id();
        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut task = Task::pass(waker, Replacement::Cancel, |_| 0, |answer, _| answer);
        task.replace(Answer(sender));
        task.cancel();
        let retired = task.close_delivery();
        drop(task);
        spawn(move |_| drop(retired));
        assert_ne!(
            dropped
                .recv_timeout(Duration::from_secs(5))
                .expect("the answer is disposed"),
            calling
        );
    }

    #[test]
    fn a_large_pass_leaves_the_caller_and_keeps_the_inline_answer() {
        fn sum(
            (_, values): (usize, [u64; 2]),
            _cancel: &CancelToken,
        ) -> (u64, std::thread::ThreadId) {
            (values.into_iter().sum(), std::thread::current().id())
        }

        let calling = std::thread::current().id();
        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut task = Task::pass(waker, Replacement::Cancel, |(items, _)| *items, sum);
        task.replace((work::INLINE_ITEMS, [3, 5]));
        let (_, answer) = task
            .take()
            .expect("an inline pass replies before returning");
        assert_eq!(answer, (8, calling));

        task.replace((work::INLINE_ITEMS + 1, [3, 5]));
        let (id, answer) = task.replies.recv_timeout(Duration::from_secs(5)).unwrap();
        let (sum, thread) = task.complete(id, answer).unwrap();
        assert_eq!(sum, 8);
        assert_ne!(thread, calling);
    }

    #[test]
    fn superseded_and_cancelled_large_values_are_retired_off_the_caller() {
        struct Payload {
            tag: u8,
            released: Option<Receiver<()>>,
            dropped: std::sync::mpsc::Sender<(u8, std::thread::ThreadId)>,
        }
        impl Drop for Payload {
            fn drop(&mut self) {
                let _ = self.dropped.send((self.tag, std::thread::current().id()));
            }
        }
        fn run(mut input: Payload, _worker: &OffThread, _cancel: &CancelToken) -> Payload {
            if let Some(released) = input.released.take() {
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            input
        }

        let calling = std::thread::current().id();
        let (drop_sender, dropped) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let payload = |tag, released| Payload {
            tag,
            released,
            dropped: drop_sender.clone(),
        };
        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut task = Task::new(waker, Replacement::Cancel, run)
            .with_size(|_| crate::core::work::INLINE_ITEMS + 1);
        task.replace(payload(1, Some(released)));
        task.replace(payload(2, None));
        task.replace(payload(3, None));
        task.cancel();
        let mut retired = task.take_retired();
        assert!(dropped.try_recv().is_err());
        release.send(()).unwrap();
        let (id, output) = task.replies.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(task.complete(id, output).is_none());
        retired.append(task.take_retired());
        assert!(dropped.try_recv().is_err());
        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut retirement =
            Task::pass(waker, Replacement::Finish, Retirement::items, |input, _| {
                drop(input);
            });
        retirement.replace(retired);
        let (id, ()) = retirement
            .replies
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(retirement.complete(id, ()).is_some());
        let mut tags = Vec::new();
        for _ in 0..3 {
            let (tag, thread) = dropped.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_ne!(thread, calling);
            tags.push(tag);
        }
        tags.sort();
        assert_eq!(tags, [1, 2, 3]);
    }

    #[test]
    fn shell_invocations_reuse_one_apartment_until_the_slot_closes() {
        fn run(
            (): (),
            _session: &crate::win::shell::Session,
            _worker: &OffThread,
            _cancel: &CancelToken,
        ) -> std::thread::ThreadId {
            std::thread::current().id()
        }

        let waker = OpenWaker::new(crate::win::Hwnd(HWND::default()));
        waker.close();
        let mut task = Task::shell(waker, Replacement::Cancel, run);
        task.submit((), |_, new| new);
        let (id, first) = task.replies.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(task.complete(id, first).is_some());
        task.submit((), |_, new| new);
        let (id, second) = task.replies.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(task.complete(id, second).is_some());
        assert_eq!(first, second);
        let Execution::Shell {
            thread: Some(thread),
            ..
        } = &task.execution
        else {
            panic!("shell input starts its retained apartment");
        };
        let thread = thread.handle();
        drop(task);
        // SAFETY: the retained JoinHandle keeps this native handle valid. This
        // test thread waits for cleanup; no product window or shell UI is opened.
        let ended = unsafe { WaitForSingleObject(HANDLE(thread.as_raw_handle()), 5_000) };
        assert_eq!(ended, WAIT_OBJECT_0);
    }
}
