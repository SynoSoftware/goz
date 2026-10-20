//! The requests to the gozler service: one thread per [`Fetch`], each with
//! its own connection and cancel token, and one channel for the replies
//! (DESIGN.md: window rules 3 to 5).

use crate::core::fetch::{
    Ask, Chunk, ChunkAsk, DaemonFailure, ExportAsk, Fetch, Fetched, Outcome, Purpose, SelectionAsk,
    SelectionRows, WriteFailure,
};
use crate::win::wake::Waker;
use gozler_client::{CancelToken, Client, ClientError};
use gozler_core::output::ResultWriter;
use gozler_core::proto::{PIPE_NAME, QueryResults, Returns};
use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufWriter};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often an export reports the rows it has written, which the status
/// bar's middle group shows (DESIGN.md: Export).
const EXPORT_PROGRESS: Duration = Duration::from_millis(500);

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
                let reply = |outcome: Outcome| {
                    // The window drops the receiver only after joining this
                    // thread, so the send cannot fail while it matters.
                    let _ = sender.send(Fetched {
                        purpose,
                        generation,
                        outcome,
                    });
                    waker.wake();
                };
                if let Some(outcome) = run(fetch.ask, &cancel, &reply) {
                    reply(outcome);
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

    /// Cancels the live request of `purpose`, whose reply nobody reads.
    pub(super) fn cancel(&self, purpose: Purpose) {
        let old = self.live.borrow_mut()[slot(purpose)].take();
        if let Some(old) = old {
            old.cancel.cancel();
            self.cancelled.borrow_mut().push(old);
        }
    }

    /// Every reply that has arrived, without waiting.
    pub(super) fn drain(&self) -> Vec<Fetched> {
        self.join_finished();
        self.replies.try_iter().collect()
    }

    /// Cancels every request and joins every thread. Each thread checks its
    /// token every 5 ms while it waits for the service, so this returns
    /// within one poll (DESIGN.md: Requests).
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

/// Connects, asks and gathers the reply, which `reply` takes before the end
/// when a request answers more than once; `None` once cancelled, because a
/// cancelled request's reply is never read.
fn run(ask: Ask, cancel: &CancelToken, reply: &dyn Fn(Outcome)) -> Option<Outcome> {
    let result =
        Client::connect(PIPE_NAME, true, cancel.clone()).and_then(|mut client| match ask {
            Ask::Status => client.status().map(Outcome::Status),
            Ask::Rows(ask) | Ask::Program(ask) => rows(&mut client, &ask).map(Outcome::Rows),
            Ask::Selection(ask) => selection(&mut client, ask).map(Outcome::Selection),
            Ask::FolderSize(search) => client
                .totals(search.request(Returns::Totals))
                .map(Outcome::Totals),
            Ask::Export(ask) => Ok(export(&mut client, &ask, cancel, reply)),
            Ask::CountIncluded(settings) => {
                client.count_included(settings).map(Outcome::IncludedCount)
            }
            Ask::SettingsSaved => client.settings_saved().map(|()| Outcome::SettingsApplied),
        });
    match result {
        Ok(outcome) => Some(outcome),
        Err(ClientError::Cancelled) => None,
        Err(error) => Some(Outcome::Failed(failure(error))),
    }
    .filter(|_| !cancel.is_cancelled())
}

/// Every page of one chunk.
fn rows(client: &mut Client, ask: &ChunkAsk) -> Result<Chunk, ClientError> {
    let mut all: Option<QueryResults> = None;
    client.query(ask.request(), |page| {
        match &mut all {
            None => all = Some(page),
            Some(all) => all.add_page(page),
        }
        Ok::<(), ClientError>(())
    })?;
    let all = all.ok_or_else(|| ClientError::Communication("the reply had no page".into()))?;
    Ok(Chunk {
        start: ask.start,
        rows: all.items,
        total: all.total,
        volumes_incomplete: all.volumes_incomplete,
        metadata_pending: all.metadata_pending,
        sums: all.sums,
    })
}

/// The rows of the selection's span that it keeps; it drops every other
/// row as its page arrives, so it holds at most the selection and the
/// cache.
fn selection(client: &mut Client, ask: SelectionAsk) -> Result<SelectionRows, ClientError> {
    // Sized up front, because growing a vector of 100,000 rows copies it
    // and holds both copies for a moment (DESIGN.md: Memory budget).
    let kept: u64 = ask
        .positions
        .iter()
        .chain(&ask.cached)
        .map(|r| r.end - r.start)
        .sum();
    let mut rows = Vec::with_capacity(usize::try_from(kept).unwrap_or(0));
    let mut position = ask.span.start;
    client.query(ask.span.request(), |page| {
        for item in page.items {
            if ask.keeps(position) {
                rows.push((position, item));
            }
            position += 1;
        }
        Ok::<(), ClientError>(())
    })?;
    Ok(SelectionRows {
        rows,
        cached: ask.cached,
    })
}

/// Why an export stopped before its end.
enum ExportStop {
    Client(ClientError),
    Write(io::Error),
}

impl From<ClientError> for ExportStop {
    fn from(error: ClientError) -> ExportStop {
        ExportStop::Client(error)
    }
}

/// Writes every page to the export's file as it arrives, and reports the
/// rows written about twice a second. The file is created first, as
/// `gozler -export-csv` creates it, and a cancelled or failed export
/// deletes it (DESIGN.md: Export).
fn export(
    client: &mut Client,
    ask: &ExportAsk,
    cancel: &CancelToken,
    reply: &dyn Fn(Outcome),
) -> Outcome {
    let stopped = match File::create(&ask.path) {
        Err(error) => return Outcome::ExportFailed(WriteFailure::from(&error)),
        Ok(file) => {
            let mut writer = ResultWriter::new(
                ask.format.clone(),
                jiff::tz::TimeZone::system(),
                BufWriter::new(file),
            );
            let mut written = 0;
            let mut reported = Instant::now();
            let pages = client.query(ask.request(), |page| {
                written += page.items.len() as u64;
                writer.page(page).map_err(ExportStop::Write)?;
                if reported.elapsed() >= EXPORT_PROGRESS {
                    reported = Instant::now();
                    reply(Outcome::ExportProgress { rows: written });
                }
                Ok::<(), ExportStop>(())
            });
            match pages.and_then(|()| writer.finish().map_err(ExportStop::Write)) {
                // Cancel export that comes after the last page still leaves
                // no file, as the person asked.
                Ok(()) if !cancel.is_cancelled() => return Outcome::Exported { rows: written },
                Ok(()) => ExportStop::Client(ClientError::Cancelled),
                Err(stop) => stop,
            }
        }
    };
    let _ = std::fs::remove_file(&ask.path);
    match stopped {
        ExportStop::Write(error) => Outcome::ExportFailed(WriteFailure::from(&error)),
        ExportStop::Client(error) => Outcome::Failed(failure(error)),
    }
}

/// The failure the window shows for the client's error (DESIGN.md: Shared
/// code); a pipe that cannot be opened or read shows as a lost connection.
fn failure(error: ClientError) -> DaemonFailure {
    match error {
        ClientError::NotRunning(why) => DaemonFailure::NotRunning(why),
        ClientError::NotReady => DaemonFailure::NotReady,
        ClientError::Untrusted => DaemonFailure::NotGozler,
        ClientError::TooOld => DaemonFailure::TooOld {
            local: super::local_service::local_service(),
        },
        ClientError::Refused { message, .. } => DaemonFailure::Refused { detail: message },
        ClientError::Communication(_) | ClientError::Lost | ClientError::Cancelled => {
            DaemonFailure::Lost
        }
    }
}
