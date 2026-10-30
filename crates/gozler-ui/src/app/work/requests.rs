//! Purpose-local service requests, with bounded interim replies separate from
//! completion so a full progress slot cannot lose an export's final result.

use crate::core::fetch::{
    Ask, Chunk, ChunkAsk, DaemonFailure, ExportAsk, Fetch, Fetched, Generation, Outcome, Programs,
    Purpose, SelectionAsk, SelectionRows, WriteFailure,
};
use crate::core::work::{INLINE_ITEMS, Interest, Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::{CancelToken, Client, ClientError};
use gozler_core::output::ResultWriter;
use gozler_core::proto::{PIPE_NAME, ProtoError, QueryResults, Returns};
use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

const EXPORT_PROGRESS: Duration = Duration::from_millis(500);

struct Request {
    fetch: Fetch,
    sender: SyncSender<Fetched>,
    waker: OpenWaker,
    error_log: Arc<Path>,
    programs: Programs,
}

struct Group {
    task: Task<Request, Option<Fetched>>,
    sender: SyncSender<Fetched>,
    interim: Receiver<Fetched>,
    current: Option<Generation>,
}

pub(in crate::app) struct Requests {
    groups: Vec<Group>,
    waker: OpenWaker,
    error_log: Arc<Path>,
    retired: Retirement,
    programs: Programs,
}

impl Requests {
    pub(super) fn new(waker: OpenWaker, error_log: PathBuf) -> Self {
        let groups = Purpose::ALL
            .map(|purpose| {
                let (sender, interim) = sync_channel(1);
                // Reply size is unknown until the service answers; disposal
                // of a cancelled reply must not run on the window thread.
                let task = Task::new(waker.clone(), Replacement::Cancel, execute)
                    .with_size(|_| INLINE_ITEMS + 1);
                let task = match purpose.interest() {
                    Interest::Awaited => task,
                    Interest::Background => task.background(),
                };
                Group {
                    task,
                    sender,
                    interim,
                    current: None,
                }
            })
            .into();
        Self {
            groups,
            waker,
            error_log: error_log.into(),
            retired: Retirement::default(),
            programs: Programs::UNKNOWN,
        }
    }

    pub(in crate::app) fn start(&mut self, fetch: Fetch) {
        let group = &mut self.groups[slot(fetch.purpose())];
        if let Ok(fetched) = group.interim.try_recv() {
            self.retired.push(fetched, INLINE_ITEMS + 1);
        }
        group.current = Some(fetch.generation);
        group.task.replace(Request {
            fetch,
            sender: group.sender.clone(),
            waker: self.waker.clone(),
            error_log: Arc::clone(&self.error_log),
            programs: self.programs.clone(),
        });
        self.retired.append(group.task.take_retired());
    }

    pub(in crate::app) fn set_programs(&mut self, programs: Programs) {
        self.programs = programs;
    }

    pub(in crate::app) fn current(&self, purpose: Purpose) -> Option<Generation> {
        self.groups[slot(purpose)].current
    }

    pub(in crate::app) fn cancel(&mut self, purpose: Purpose) {
        let group = &mut self.groups[slot(purpose)];
        group.current = None;
        group.task.cancel();
        self.retired.append(group.task.take_retired());
    }

    pub(in crate::app) fn drain(&mut self) -> Vec<Fetched> {
        let mut replies = Vec::new();
        let now = Instant::now();
        for group in &mut self.groups {
            if let Ok(fetched) = group.interim.try_recv() {
                if group.current == Some(fetched.generation) {
                    group.task.progress(now);
                    replies.push(fetched);
                } else {
                    self.retired.push(fetched, INLINE_ITEMS + 1);
                }
            }
            if let Some((_, fetched)) = group.task.take() {
                group.current = None;
                if let Some(fetched) = fetched {
                    replies.push(fetched);
                }
            }
            self.retired.append(group.task.take_retired());
        }
        replies
    }

    pub(in crate::app) fn waiting(&self, purpose: Purpose, now: Instant) -> Option<Waiting> {
        let group = &self.groups[slot(purpose)];
        group.current?;
        group.task.waiting(now)
    }

    pub(super) fn tick(&mut self, now: Instant) -> Vec<(Purpose, Generation, Waiting)> {
        let mut states = Vec::new();
        for (purpose, group) in Purpose::ALL.into_iter().zip(&mut self.groups) {
            if let Some(generation) = group.current
                && let Some(waiting) = group.task.tick(now)
            {
                states.push((purpose, generation, waiting));
                if waiting == Waiting::Expired {
                    group.current = None;
                }
            }
        }
        states
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        std::mem::take(&mut self.retired)
    }

    /// Close the shared wake lifetime before HWND destruction. Run the returned
    /// cleanup independently of essential writes: an export's file may be slow.
    pub(super) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static + use<> {
        let mut export = None;
        let mut retired = self.take_retired();
        for (purpose, mut group) in Purpose::ALL
            .into_iter()
            .zip(std::mem::take(&mut self.groups))
        {
            group.task.cancel();
            if purpose == Purpose::Export {
                export = Some(group.task);
            } else {
                retired.append(group.task.close_delivery());
            }
            retired.push(group.interim, INLINE_ITEMS + 1);
        }
        move |worker| {
            drop(retired);
            if let Some(export) = export {
                export.finish(worker);
            }
        }
    }
}

fn slot(purpose: Purpose) -> usize {
    Purpose::ALL
        .iter()
        .position(|candidate| *candidate == purpose)
        .expect("ALL holds every purpose")
}

fn execute(input: Request, _worker: &OffThread, cancel: &CancelToken) -> Option<Fetched> {
    let purpose = input.fetch.purpose();
    let generation = input.fetch.generation;
    let fetched = |outcome| Fetched {
        purpose,
        generation,
        outcome,
    };
    let reply = |outcome| {
        input
            .waker
            .deliver(|| input.sender.try_send(fetched(outcome)).is_ok());
    };
    run(
        input.fetch.ask,
        cancel,
        &reply,
        &input.error_log,
        &input.programs,
    )
    .map(fetched)
}

/// Connects, asks and gathers the reply, which `reply` takes before the end
/// when a request answers more than once; `None` once cancelled, because a
/// cancelled request's reply is never read.
fn run(
    ask: Ask,
    cancel: &CancelToken,
    reply: &dyn Fn(Outcome),
    error_log: &Path,
    programs: &Programs,
) -> Option<Outcome> {
    let result =
        Client::connect(PIPE_NAME, true, cancel.clone()).and_then(|mut client| match ask {
            Ask::Status { links_held } => client.status(Some(links_held)).map(|status| {
                // The hello names the service as `gozlerd <version>`.
                let server = client.server();
                let version = server.strip_prefix("gozlerd ").unwrap_or(server);
                Outcome::Status(status, version.to_owned(), programs.clone())
            }),
            Ask::Rows(_, ask) => rows(&mut client, &ask, Some(reply)).map(Outcome::Rows),
            Ask::Program(ask) | Ask::FolderSizes(_, ask) => {
                rows(&mut client, &ask, None).map(Outcome::Rows)
            }
            Ask::Selection(_, ask) => selection(&mut client, ask).map(Outcome::Selection),
            Ask::FolderSize(search) => client
                .totals(search.request(Returns::Totals))
                .map(Outcome::Totals),
            Ask::Export(ask) => Ok(export(
                &mut client,
                &ask,
                cancel,
                reply,
                error_log,
                programs,
            )),
            Ask::CountIncluded(settings) => {
                client.count_included(settings).map(Outcome::IncludedCount)
            }
            Ask::SettingsSaved => client.settings_saved().map(|()| Outcome::SettingsApplied),
        });
    match result {
        Ok(outcome) => Some(outcome),
        Err(ClientError::Cancelled) => None,
        Err(ClientError::Refused {
            code: ProtoError::ResultSnapshotGone,
            ..
        }) => Some(Outcome::ResultSnapshotGone),
        Err(error) => Some(Outcome::Failed(failure(error, programs))),
    }
    .filter(|_| !cancel.is_cancelled())
}

/// Every page of one chunk. With `first`, the first page also goes there
/// as soon as it arrives, when more pages follow, so the list shows rows
/// before the whole chunk has arrived.
pub(super) fn rows(
    client: &mut Client,
    ask: &ChunkAsk,
    first: Option<&dyn Fn(Outcome)>,
) -> Result<Chunk, ClientError> {
    let mut all: Option<QueryResults> = None;
    let made = Cell::new(None);
    client.rows(
        ask.request(),
        |id| made.set(Some(id)),
        |page| {
            match &mut all {
                None => {
                    if let Some(first) = first
                        && page.more
                    {
                        first(Outcome::Rows(Chunk {
                            start: ask.start,
                            rows: page.items.clone().into(),
                            total: page.total,
                            volumes_incomplete: page.volumes_incomplete,
                            metadata_pending: page.metadata_pending,
                            sums: page.sums,
                            rest_follows: true,
                            made: made.get(),
                        }));
                    }
                    all = Some(page);
                }
                Some(all) => all.add_page(page),
            }
            Ok::<(), ClientError>(())
        },
    )?;
    let all = all.ok_or_else(|| ClientError::Communication("the reply had no page".into()))?;
    Ok(Chunk {
        start: ask.start,
        rows: all.items.into(),
        total: all.total,
        volumes_incomplete: all.volumes_incomplete,
        metadata_pending: all.metadata_pending,
        sums: all.sums,
        rest_follows: false,
        made: made.get(),
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
    let mut made = None;
    client.rows(
        ask.span.request(),
        |id| made = Some(id),
        |page| {
            for item in page.items {
                if ask.keeps(position) {
                    rows.push((position, item));
                }
                position += 1;
            }
            Ok::<(), ClientError>(())
        },
    )?;
    Ok(SelectionRows::new(&ask.positions, rows, ask.cached, made))
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
    error_log: &Path,
    programs: &Programs,
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
    if let Err(error) = std::fs::remove_file(&ask.path)
        && error.kind() != io::ErrorKind::NotFound
    {
        crate::app::write_log(
            error_log,
            &format!(
                "Partial export {} was not deleted: {error}",
                ask.path.display()
            ),
        );
    }
    match stopped {
        ExportStop::Write(error) => Outcome::ExportFailed(WriteFailure::from(&error)),
        ExportStop::Client(error) => Outcome::Failed(failure(error, programs)),
    }
}

/// The failure the window shows for the client's error (DESIGN.md: Shared
/// code); a pipe that cannot be opened or read shows as a lost connection.
fn failure(error: ClientError, programs: &Programs) -> DaemonFailure {
    match error {
        ClientError::NotRunning(why) => DaemonFailure::NotRunning(why),
        ClientError::NotReady => DaemonFailure::NotReady,
        ClientError::Untrusted => DaemonFailure::NotGozler,
        ClientError::TooOld => DaemonFailure::TooOld {
            programs: programs.clone(),
        },
        ClientError::Refused { message, .. } => DaemonFailure::Refused { detail: message },
        ClientError::Communication(_) | ClientError::Lost | ClientError::Cancelled => {
            DaemonFailure::Lost
        }
    }
}
