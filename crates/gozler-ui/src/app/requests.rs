//! The requests to the gozler service: one thread per [`Fetch`], each with
//! its own connection and cancel token, and one channel for the replies
//! (design rules 3 to 5).

use crate::core::fetch::{Ask, Chunk, ChunkAsk, DaemonFailure, Fetch, Fetched, Outcome, Purpose};
use crate::win::wake::Waker;
use gozler_client::{CancelToken, Client, ClientError};
use gozler_core::proto::PIPE_NAME;
use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

struct Running {
    cancel: CancelToken,
    thread: JoinHandle<()>,
}

/// The live request of each purpose, and the cancelled ones that have not
/// been joined yet. A cancelled thread still holds a [`Waker`] until its next
/// poll, so it is joined before the window's handle goes (rule 5).
pub(super) struct Requests {
    live: RefCell<[Option<Running>; Purpose::ALL.len()]>,
    cancelled: RefCell<Vec<Running>>,
    sender: Sender<Fetched>,
    replies: Receiver<Fetched>,
    waker: Waker,
}

impl Requests {
    pub(super) fn new(waker: Waker) -> Requests {
        let (sender, replies) = channel();
        Requests {
            live: RefCell::new([const { None }; Purpose::ALL.len()]),
            cancelled: RefCell::new(Vec::new()),
            sender,
            replies,
            waker,
        }
    }

    /// Cancels the live request of `fetch`'s purpose and no other, then runs
    /// `fetch` on a new thread.
    pub(super) fn start(&self, fetch: Fetch) {
        let purpose = fetch.purpose();
        let cancel = CancelToken::default();
        let thread = {
            let (cancel, sender, waker) = (cancel.clone(), self.sender.clone(), self.waker.clone());
            std::thread::spawn(move || {
                let generation = fetch.generation;
                if let Some(outcome) = run(fetch.ask, cancel) {
                    // The window drops the receiver only after joining this
                    // thread, so the send cannot fail while it matters.
                    let _ = sender.send(Fetched {
                        purpose,
                        generation,
                        outcome,
                    });
                    waker.wake();
                }
            })
        };
        let old = self.live.borrow_mut()[slot(purpose)].replace(Running { cancel, thread });
        if let Some(old) = old {
            old.cancel.cancel();
            self.cancelled.borrow_mut().push(old);
        }
        self.join_finished();
    }

    /// Every reply that has arrived, without waiting.
    pub(super) fn drain(&self) -> Vec<Fetched> {
        self.join_finished();
        self.replies.try_iter().collect()
    }

    /// Cancels every request and joins every thread. Each thread checks its
    /// token every 5 ms while it waits for the service, so this returns
    /// within one poll (design: Exit during a search).
    pub(super) fn shut_down(&self) {
        let mut all: Vec<Running> = self.cancelled.borrow_mut().drain(..).collect();
        all.extend(self.live.borrow_mut().iter_mut().filter_map(Option::take));
        for running in &all {
            running.cancel.cancel();
        }
        for running in all {
            let _ = running.thread.join();
        }
    }

    fn join_finished(&self) {
        let finished: Vec<Running> = {
            let mut cancelled = self.cancelled.borrow_mut();
            let (finished, running) = cancelled.drain(..).partition(|r| r.thread.is_finished());
            *cancelled = running;
            finished
        };
        for running in finished {
            let _ = running.thread.join();
        }
    }
}

fn slot(purpose: Purpose) -> usize {
    Purpose::ALL
        .iter()
        .position(|p| *p == purpose)
        .expect("ALL holds every purpose")
}

/// Connects, asks and gathers the reply; `None` once cancelled, because a
/// cancelled request's reply is never read.
fn run(ask: Ask, cancel: CancelToken) -> Option<Outcome> {
    let result = Client::connect(PIPE_NAME, true, cancel).and_then(|mut client| match ask {
        Ask::Status => client.status().map(Outcome::Status),
        Ask::Rows(ask) => rows(&mut client, &ask).map(Outcome::Rows),
    });
    match result {
        Ok(outcome) => Some(outcome),
        Err(ClientError::Cancelled) => None,
        Err(error) => Some(Outcome::Failed(failure(error))),
    }
}

/// Every page of one chunk.
fn rows(client: &mut Client, ask: &ChunkAsk) -> Result<Chunk, ClientError> {
    let mut chunk = Chunk {
        start: ask.start,
        rows: Vec::new(),
        total: 0,
        volumes_incomplete: false,
        metadata_pending: false,
    };
    client.query(ask.request(), |page| {
        chunk.total = page.total;
        chunk.volumes_incomplete |= page.volumes_incomplete;
        chunk.metadata_pending |= page.metadata_pending;
        chunk.rows.extend(page.items);
        Ok::<(), ClientError>(())
    })?;
    Ok(chunk)
}

/// The failure the window shows for the client's error (design: Shared
/// code); a pipe that cannot be opened or read shows as a lost connection.
fn failure(error: ClientError) -> DaemonFailure {
    match error {
        ClientError::NotRunning(why) => DaemonFailure::NotRunning(why),
        ClientError::NotReady => DaemonFailure::NotReady,
        ClientError::Untrusted => DaemonFailure::NotGozler,
        ClientError::TooOld => DaemonFailure::TooOld,
        ClientError::Refused { message, .. } => DaemonFailure::Refused { detail: message },
        ClientError::Communication(_) | ClientError::Lost | ClientError::Cancelled => {
            DaemonFailure::Lost
        }
    }
}
