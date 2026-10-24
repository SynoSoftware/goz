//! One preview, one effect pass and serialized accepted source calls.

use crate::core::drop_target::{Dragged, DropEffect, DropKeys, DropMenu, SourceEffects};
use crate::core::work::{INLINE_ITEMS, Id, Replacement, Retirement, Waiting};
use crate::win::drop_target::{Completion, CompletionSender, DropData, Prepared, Setup};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use gozler_core::proto::ResultItem;
use gozler_fileops::OperationId;
use std::collections::LinkedList;
use std::path::PathBuf;
use std::sync::{Arc, mpsc::Receiver};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Expired {
    Preview(Id),
    Evaluation(Id),
}

pub(crate) struct Evaluation {
    pub(crate) dragged: Arc<Dragged>,
    pub(crate) folder: ResultItem,
    pub(crate) keys: DropKeys,
    pub(crate) allowed: SourceEffects,
}

pub(crate) struct Evaluated {
    pub(crate) effect: Option<DropEffect>,
    pub(crate) menu: Option<DropMenu>,
}

enum Call {
    Setup(OperationId, Setup),
    Read(OperationId, DropData),
}

pub(crate) enum Answer {
    Setup(OperationId, Option<Prepared>),
    Read(OperationId, Vec<PathBuf>),
}

pub(in crate::app) struct Work {
    preview: Task<DropData, Arc<Dragged>>,
    effect: Task<Evaluation, Evaluated>,
    calls: Task<Call, Answer>,
    pending: LinkedList<Call>,
    finishing: Task<LinkedList<Completion>, ()>,
    completion: CompletionSender,
    completions: Receiver<Completion>,
    waker: OpenWaker,
}

impl Work {
    pub(in crate::app) fn new(waker: OpenWaker) -> Self {
        let (completion, completions) = CompletionSender::new(waker.clone());
        Self {
            preview: Task::new(
                waker.clone(),
                Replacement::Cancel,
                |data: DropData, worker, _| Arc::new(Dragged::new(data.read(worker))),
            )
            .with_size(|data| data.items().max(INLINE_ITEMS + 1))
            .background(),
            effect: Task::pass(
                waker.clone(),
                Replacement::Cancel,
                |input: &Evaluation| input.dragged.items(),
                |input, _| {
                    let effect =
                        input
                            .dragged
                            .effect(Some(&input.folder), input.keys, input.allowed);
                    let menu = effect
                        .map(|effect| input.dragged.menu(&input.folder, effect, input.allowed));
                    Evaluated { effect, menu }
                },
            )
            .background(),
            calls: Task::new(waker.clone(), Replacement::Finish, call),
            pending: LinkedList::new(),
            finishing: Task::new(
                waker.clone(),
                Replacement::Finish,
                |completions: LinkedList<Completion>, worker, _| {
                    for completion in completions {
                        completion.run(worker);
                    }
                },
            ),
            completion,
            completions,
            waker,
        }
    }

    pub(in crate::app) fn tick(&mut self, now: Instant) -> (bool, Vec<Expired>) {
        let mut active = false;
        let mut expired = Vec::new();
        let preview = self.preview.current();
        match self.preview.tick(now) {
            Some(Waiting::Expired) => {
                if let Some(id) = preview {
                    expired.push(Expired::Preview(id));
                }
            }
            Some(_) => active = true,
            None => {}
        }
        let evaluation = self.effect.current();
        match self.effect.tick(now) {
            Some(Waiting::Expired) => {
                if let Some(id) = evaluation {
                    expired.push(Expired::Evaluation(id));
                }
            }
            Some(_) => active = true,
            None => {}
        }
        (active, expired)
    }

    pub(in crate::app) fn preview(&mut self, data: DropData) -> Id {
        self.preview.replace(data)
    }
    pub(in crate::app) fn take_preview(&mut self) -> Option<(Id, Arc<Dragged>)> {
        self.preview.take()
    }
    pub(in crate::app) fn evaluate(&mut self, input: Evaluation) -> Id {
        self.effect.replace(input)
    }
    pub(in crate::app) fn take_effect(&mut self) -> Option<(Id, Evaluated)> {
        self.effect.take()
    }

    pub(in crate::app) fn leave(&mut self) {
        self.preview.cancel();
        self.effect.cancel();
    }

    pub(in crate::app) fn setup(&mut self, id: OperationId, data: DropData, effect: DropEffect) {
        self.pending.push_back(Call::Setup(
            id,
            Setup {
                data,
                effect,
                completion: self.completion.clone(),
            },
        ));
        self.start();
    }

    pub(in crate::app) fn read(&mut self, id: OperationId, data: DropData) {
        self.pending.push_back(Call::Read(id, data));
        self.start();
    }

    fn start(&mut self) {
        if !self.calls.is_running()
            && let Some(call) = self.pending.pop_front()
        {
            self.calls.submit(call, |_, _| {
                unreachable!("accepted calls wait in their FIFO")
            });
        }
    }

    pub(in crate::app) fn take(&mut self) -> Option<Answer> {
        let answer = self.calls.take().map(|(_, answer)| answer);
        self.start();
        self.finish_sources();
        answer
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
            self.finishing.submit(completions, |mut old, mut new| {
                old.append(&mut new);
                old
            });
        }
        let _ = self.finishing.take();
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        let mut retired = self.preview.take_retired();
        retired.append(self.effect.take_retired());
        retired.append(self.calls.take_retired());
        retired.append(self.finishing.take_retired());
        retired
    }

    /// Nonessential finalization; run separately from recovery persistence.
    pub(in crate::app) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static {
        self.leave();
        self.finish_sources();
        let mut retired = self.preview.close_delivery();
        retired.append(self.effect.close_delivery());
        self.calls.cancel();
        let calls = std::mem::replace(
            &mut self.calls,
            Task::new(self.waker.clone(), Replacement::Finish, call),
        );
        let pending = std::mem::take(&mut self.pending);
        retired.push(pending, INLINE_ITEMS + 1);
        let (sender, receiver) = CompletionSender::new(self.waker.clone());
        let old_sender = std::mem::replace(&mut self.completion, sender);
        let completions = std::mem::replace(&mut self.completions, receiver);
        drop(old_sender);
        let replacement = Task::new(
            self.waker.clone(),
            Replacement::Finish,
            |batch: LinkedList<Completion>, worker, _| {
                for completion in batch {
                    completion.run(worker);
                }
            },
        );
        let finishing = std::mem::replace(&mut self.finishing, replacement);
        move |worker| {
            drop(retired);
            finishing.finish(worker);
            for completion in completions.try_iter() {
                completion.run(worker);
            }
            calls.finish_current(worker);
            for completion in completions.try_iter() {
                completion.run(worker);
            }
        }
    }
}

fn call(call: Call, worker: &OffThread, _cancel: &CancelToken) -> Answer {
    match call {
        Call::Setup(id, setup) => Answer::Setup(id, setup.run(worker)),
        Call::Read(id, data) => Answer::Read(id, data.read(worker)),
    }
}
