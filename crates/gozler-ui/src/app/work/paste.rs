//! Accepted Paste reads are ordered; their retained apartment outlives feedback.

use crate::core::work::{INLINE_ITEMS, Replacement, Retirement};
use crate::win::clipboard::{ClipboardFiles, Completion, CompletionSender, Read};
use crate::win::shell::Session;
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use gozler_fileops::OperationId;
use std::collections::{HashMap, LinkedList};
use std::sync::mpsc::Receiver;

struct Reading {
    id: OperationId,
    read: Read,
    wanted: CancelToken,
    batch: CancelToken,
}

pub(in crate::app) struct Work {
    reading: Task<Reading, (OperationId, Option<ClipboardFiles>)>,
    pending: LinkedList<Reading>,
    accepted: HashMap<OperationId, CancelToken>,
    batch: CancelToken,
    finishing: Task<LinkedList<Completion>, ()>,
    completion: CompletionSender,
    completions: Receiver<Completion>,
    retired: Retirement,
    waker: OpenWaker,
}

impl Work {
    pub(in crate::app) fn new(waker: OpenWaker) -> Self {
        let (completion, completions) = CompletionSender::new(waker.clone());
        Self {
            reading: reader(waker.clone()),
            pending: LinkedList::new(),
            accepted: HashMap::new(),
            batch: CancelToken::default(),
            finishing: finisher(waker.clone()),
            completion,
            completions,
            retired: Retirement::default(),
            waker,
        }
    }

    pub(in crate::app) fn read(&mut self, id: OperationId, sequence: u32) {
        let wanted = CancelToken::default();
        self.accepted.insert(id, wanted.clone());
        self.pending.push_back(Reading {
            id,
            read: Read {
                sequence,
                completion: self.completion.clone(),
            },
            wanted,
            batch: self.batch.clone(),
        });
        self.start();
    }

    fn start(&mut self) {
        if !self.reading.is_running()
            && let Some(input) = self.pending.pop_front()
        {
            self.reading.submit(input, |_, _| {
                unreachable!("accepted reads wait in their FIFO")
            });
        }
    }

    pub(in crate::app) fn take(&mut self) -> Option<(OperationId, Option<ClipboardFiles>)> {
        self.finish_sources();
        let answer = self.reading.take().map(|(_, answer)| answer);
        self.start();
        if let Some((id, mut files)) = answer {
            if self.accepted.remove(&id).is_some() {
                return Some((id, files));
            }
            if let Some(files) = &mut files {
                files.release_source();
            }
            self.retired.push(files, INLINE_ITEMS + 1);
        }
        None
    }

    pub(in crate::app) fn cancel(&mut self, id: OperationId) {
        if let Some(wanted) = self.accepted.remove(&id) {
            wanted.cancel();
        }
    }

    pub(in crate::app) fn cancel_all(&mut self) {
        self.batch.cancel();
        self.batch = CancelToken::default();
        let accepted = std::mem::take(&mut self.accepted);
        self.retired.push(accepted, INLINE_ITEMS + 1);
        let pending = std::mem::take(&mut self.pending);
        self.retired.push(pending, INLINE_ITEMS + 1);
    }

    fn finish_sources(&mut self) {
        let mut completions = LinkedList::new();
        for _ in 0..INLINE_ITEMS {
            let Ok(completion) = self.completions.try_recv() else {
                break;
            };
            completions.push_back(completion);
        }
        if completions.len() == INLINE_ITEMS {
            self.waker.deliver(|| true);
        }
        if !completions.is_empty() {
            submit_completions(&mut self.finishing, completions);
        }
        let _ = self.finishing.take();
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        let mut retired = std::mem::take(&mut self.retired);
        retired.append(self.reading.take_retired());
        retired.append(self.finishing.take_retired());
        retired
    }

    /// Run separately from essential persistence; a provider may never return.
    pub(in crate::app) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static {
        self.cancel_all();
        self.reading.cancel();
        self.finish_sources();
        let retired = self.take_retired();
        let mut reading = std::mem::replace(&mut self.reading, reader(self.waker.clone()));
        let mut finishing = std::mem::replace(&mut self.finishing, finisher(self.waker.clone()));
        let (sender, receiver) = CompletionSender::new(self.waker.clone());
        let old_sender = std::mem::replace(&mut self.completion, sender);
        let completions = std::mem::replace(&mut self.completions, receiver);
        drop(old_sender);
        move |worker| {
            drop(retired);
            drain_completions(&mut finishing, &completions, worker);
            while reading.is_running() {
                drop(reading.wait(worker));
                drop(reading.take_retired());
                drain_completions(&mut finishing, &completions, worker);
            }
            drain_completions(&mut finishing, &completions, worker);
            // All returned source leases have released on the feedback lane;
            // only now may the original data object's apartment stop pumping.
            drop(reading);
            drop(finishing);
        }
    }
}

fn reader(waker: OpenWaker) -> Task<Reading, (OperationId, Option<ClipboardFiles>)> {
    Task::shell(waker, Replacement::Finish, read).with_size(|_| INLINE_ITEMS + 1)
}

fn read(
    input: Reading,
    session: &Session,
    worker: &OffThread,
    cancel: &CancelToken,
) -> (OperationId, Option<ClipboardFiles>) {
    let wanted =
        || !input.wanted.is_cancelled() && !input.batch.is_cancelled() && !cancel.is_cancelled();
    let files = if wanted() {
        input.read.run(session, worker)
    } else {
        None
    };
    (input.id, if wanted() { files } else { None })
}

fn finisher(waker: OpenWaker) -> Task<LinkedList<Completion>, ()> {
    Task::shell(
        waker,
        Replacement::Finish,
        |batch: LinkedList<Completion>, session, worker, _| {
            for completion in batch {
                completion.run(session, worker);
            }
        },
    )
}

fn submit_completions(task: &mut Task<LinkedList<Completion>, ()>, batch: LinkedList<Completion>) {
    task.submit(batch, |mut old, mut next| {
        old.append(&mut next);
        old
    });
}

fn drain_completions(
    task: &mut Task<LinkedList<Completion>, ()>,
    receiver: &Receiver<Completion>,
    worker: &OffThread,
) {
    let batch = receiver.try_iter().collect::<LinkedList<_>>();
    if !batch.is_empty() {
        submit_completions(task, batch);
    }
    while task.is_running() {
        let _ = task.wait(worker);
    }
}
