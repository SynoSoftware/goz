//! The named-pipe query server.
//!
//! A tokio multi-thread runtime accepts connections on `\\.\pipe\gozler-v1`,
//! frames requests/responses with the shared sans-io codec from
//! `gozler-core::proto`, and answers each search through `gozler-core`'s
//! [`ResultRows`] and [`search_totals`], which read each volume through its
//! [`VolumeState`]. A search can also keep its rows as a result snapshot,
//! which later requests read by its id from one daemon-wide table
//! ([`crate::result_snapshots`]). Indexing itself (bootstrap + tail) stays
//! on dedicated OS threads; tokio only serves the pipe.

use std::ops::Deref;
use std::os::windows::io::AsHandle;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gozler_core::include::IndexSettings;
use gozler_core::index::VolumeIndex;
use gozler_core::proto::{
    DaemonStatus, MAX_CLIENT_FRAME, MemPair, OLDEST_PROTO_VERSION, PIPE_NAME, PROTO_VERSION,
    ProtoError, Request, Response, ResultSnapshotId, Returns, Rows, SentFolders,
    SnapshotRowsRequest, SortSnapshotRequest, VolumeMemory, VolumeStatus, encode_page,
    encode_response_frame, encode_snapshot_page,
};
use gozler_core::query::{
    CompiledQuery, HeldVolume, MatchSums, ResultRows, ResultSnapshot, RowsSpec, TotalsError,
    search_totals,
};
use gozler_core::scope::VolumeMounts;
use gozler_winfs::{PipeSecurity, build_pipe_security};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{Semaphore, watch};

use crate::include::Include;
use crate::result_snapshots::{Kept, ResultSnapshots, live_volumes};
use crate::volume_state::{VolumeRead, VolumeRegistry, VolumeSet, VolumeState};

/// The result snapshots of every client, each with its volumes held weakly.
type Snapshots = ResultSnapshots<Weak<VolumeState>>;

/// Cap on concurrently-served pipe connections. A silent/hostile local client
/// cannot pin more than this many tasks + read buffers + pipe instances.
const MAX_CONNECTIONS: usize = 64;
/// Pages in flight between the query encoder and the pipe writer. Each page
/// holds [`gozler_core::proto::PAGE_ROWS`] rows, so the backlog bounds a full
/// export's buffered bytes instead of buffering the entire response.
const QUERY_PAGE_BACKLOG: usize = 4;
/// Idle/half-open connections are dropped after this long with no request, so a
/// connected-but-silent client cannot block a task forever. The one-shot CLI
/// client always writes immediately after connecting, well within this window.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// A connection whose client takes none of a reply for this long is dropped,
/// so a client that stops reading cannot hold its result set and its
/// connection slot forever. It is long because a person who pauses the
/// console (a selection in the window, `| more`) also stops the client
/// reading, and waiting only keeps those resources a while longer.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Pause after a failed pipe connect before listening again. When connects
/// keep failing (a broken pipe, or a local user who connects and closes in a
/// loop), it holds the accept loop to a bounded rate instead of a busy
/// loop. A client that connects during the pause waits for it, not
/// longer: the next instance already exists, and its connect completes at once.
const CONNECT_RETRY_PAUSE: Duration = Duration::from_millis(10);
/// At most one log line per this period for failed connects; it counts the
/// failures since the last one, so the service log cannot grow with them.
const CONNECT_WARN_EVERY: Duration = Duration::from_secs(60);

/// Runs the pipe server until stopped. Blocks on a tokio runtime.
///
/// Stops on either Ctrl+C (console mode) or `shutdown` flipping to `true` (the
/// service-control handler's Stop/Shutdown path). Keep at least one
/// `watch::Sender` alive for the duration in console mode: if every sender
/// drops, `changed()` resolves and would end the loop prematurely.
pub(crate) fn run(
    volumes: Arc<VolumeRegistry>,
    include: Arc<Include>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let security = build_pipe_security().context("building pipe security descriptor")?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    let snapshots = Arc::new(ResultSnapshots::new());
    rt.block_on(accept_loop(volumes, include, snapshots, security, shutdown))
}

async fn accept_loop(
    volumes: Arc<VolumeRegistry>,
    include: Arc<Include>,
    snapshots: Arc<Snapshots>,
    security: PipeSecurity,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut server = create_instance(&security, true)?;
    let conn_limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    tracing::info!(pipe = PIPE_NAME, "query server listening");
    let mut connect_failures = 0u64;
    let mut last_connect_warning: Option<Instant> = None;
    loop {
        tokio::select! {
            r = server.connect() => {
                if let Err(e) = r {
                    // One client's failed connect (for example a client that
                    // closed the pipe before the server saw it) ends only that
                    // connection. The instance is replaced, the new one first,
                    // so the pipe always has a listener.
                    connect_failures += 1;
                    if last_connect_warning.is_none_or(|t| t.elapsed() >= CONNECT_WARN_EVERY) {
                        tracing::warn!(
                            error = %e,
                            failures = connect_failures,
                            "pipe connect failed; still listening"
                        );
                        connect_failures = 0;
                        last_connect_warning = Some(Instant::now());
                    }
                    server = create_instance(&security, false)?;
                    tokio::time::sleep(CONNECT_RETRY_PAUSE).await;
                    continue;
                }
                let connected = server;
                // Create the next instance before handling, so there is never
                // a window with no listener for a client to race into.
                server = create_instance(&security, false)?;
                // Bound concurrent connections. try_acquire (non-blocking) keeps
                // the accept loop responsive to shutdown under load: at the cap
                // we drop the newest connection rather than pin an extra one.
                match Arc::clone(&conn_limit).try_acquire_owned() {
                    Ok(permit) => {
                        let vols = volumes.clone();
                        let include = include.clone();
                        let snapshots = snapshots.clone();
                        tokio::spawn(async move {
                            let _permit = permit; // held for the connection's lifetime
                            if let Err(e) = handle_connection(connected, vols, include, snapshots).await {
                                tracing::debug!(error = %e, "connection ended");
                            }
                        });
                    }
                    Err(_) => {
                        tracing::warn!(max = MAX_CONNECTIONS, "connection limit reached; dropping client");
                        drop(connected);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl+C received; shutting down");
                return Ok(());
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    tracing::info!("stop requested; shutting down");
                    return Ok(());
                }
            }
        }
    }
}

fn create_instance(security: &PipeSecurity, first: bool) -> Result<NamedPipeServer> {
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    opts.reject_remote_clients(true);
    // SAFETY: `security.attributes_ptr()` points at a live SECURITY_ATTRIBUTES
    // owned by `security`, which outlives this call and every returned instance.
    let server =
        unsafe { opts.create_with_security_attributes_raw(PIPE_NAME, security.attributes_ptr()) }
            .with_context(|| format!("creating pipe instance {PIPE_NAME}"))?;
    Ok(server)
}

async fn handle_connection(
    mut server: NamedPipeServer,
    volumes: Arc<VolumeRegistry>,
    include: Arc<Include>,
    snapshots: Arc<Snapshots>,
) -> Result<()> {
    // The user a result snapshot counts against. A client whose user cannot
    // be read, such as one that already exited, counts as one user with
    // every other such client.
    let user: Arc<[u8]> = gozler_winfs::pipe_client_user(server.as_handle())
        .unwrap_or_else(|e| {
            tracing::debug!(error = %e, "could not read the client's user");
            Vec::new()
        })
        .into();
    let mut decoder = gozler_core::proto::FrameDecoder::new(MAX_CLIENT_FRAME);
    let mut read_buf = vec![0u8; 16 * 1024];
    loop {
        // Reap idle/half-open connections: a connected client that never sends a
        // request is dropped after IDLE_TIMEOUT, freeing its task/buffer/pipe.
        let n = match tokio::time::timeout(IDLE_TIMEOUT, server.read(&mut read_buf)).await {
            Ok(r) => r?,
            Err(_) => return Ok(()),
        };
        if n == 0 {
            return Ok(());
        }
        decoder.feed(&read_buf[..n]);
        loop {
            let frame = match decoder.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => return Err(anyhow::anyhow!("frame error: {e}")),
            };
            // Query execution plus result serialization is the heaviest CPU
            // work in the process and holds a read lock, so every request runs
            // on the blocking pool rather than stalling an async worker (and
            // thus the accept loop). Replies stream through a bounded channel:
            // a large export never exists as one buffer, at most
            // QUERY_PAGE_BACKLOG pages are in flight, and encoding overlaps the
            // pipe writes instead of preceding them.
            let vols = volumes.current();
            let include = include.clone();
            let snapshots = snapshots.clone();
            let user = user.clone();
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(QUERY_PAGE_BACKLOG);
            let encoder = tokio::task::spawn_blocking(move || {
                // A failed send means the receiver is gone (client vanished or
                // write error): stop encoding.
                serve(&vols, &include, &snapshots, &user, &frame, &mut |reply| {
                    tx.blocking_send(reply).is_ok()
                });
            });
            let mut write_err: Option<std::io::Error> = None;
            while let Some(reply) = rx.recv().await {
                if let Err(e) = write_reply(&mut server, &reply).await {
                    write_err = Some(e);
                    break;
                }
            }
            // Closing the channel unblocks a still-encoding task; it aborts at
            // its next emit.
            drop(rx);
            encoder.await.context("request task panicked")?;
            if let Some(e) = write_err {
                return Err(e.into());
            }
        }
    }
}

/// Writes reply bytes to the client, failing with `TimedOut` when the client
/// has not taken them within [`WRITE_TIMEOUT`].
async fn write_reply(server: &mut NamedPipeServer, bytes: &[u8]) -> std::io::Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, server.write_all(bytes))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "client stopped reading"))?
}

/// Answers one request frame from a client running as `user`, handing each
/// framed reply to `emit` as soon as it is encoded. `emit` returning `false`
/// means the client is gone; encoding stops.
fn serve(
    volumes: &VolumeSet,
    include: &Include,
    snapshots: &Snapshots,
    user: &[u8],
    frame: &[u8],
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let search = match gozler_core::proto::decode_request(frame) {
        Ok(Request::Search(search)) => search,
        Ok(Request::Hello {
            proto_min,
            proto_max,
            ..
        }) => return reply(emit, hello(volumes, proto_min, proto_max)),
        Ok(Request::Status) => {
            return reply(emit, Response::Status(build_status(volumes, include)));
        }
        Ok(Request::SettingsSaved) => {
            include.reload();
            return reply(emit, Response::SettingsApplied);
        }
        Ok(Request::CountIncluded(settings)) => {
            let entries = count_included(volumes, &settings);
            return reply(emit, Response::IncludedCount { entries });
        }
        Ok(Request::ResultSnapshotRows(read)) => {
            return read_snapshot(volumes, snapshots, &read, emit);
        }
        Ok(Request::SortResultSnapshot(sort)) => {
            return sort_snapshot(volumes, snapshots, user, &sort, emit);
        }
        Err(e) => return reply(emit, refusal(format!("malformed request: {e}"))),
    };
    if let Err(off) = include.applied().allows(&search.query) {
        return reply(emit, refusal(off.to_string()));
    }
    let query = match CompiledQuery::new(&search.query) {
        Ok(query) => query,
        Err(e) => return reply(emit, refusal(e.to_string())),
    };
    // In `VolumeSet` order, the order every request takes the volumes' locks
    // in.
    let targets = volumes.to_vec();
    let scope = search.scope.as_deref();
    match search.returns {
        Returns::Rows(wanted) => match ResultRows::search(targets, &query, scope, wanted.spec()) {
            Ok(rows) => stream_rows(rows, &wanted, emit),
            Err(not_indexed) => reply(emit, refusal(not_indexed.to_string())),
        },
        Returns::Totals => reply(emit, totals(targets, &query, scope)),
        Returns::ResultSnapshot { rows, replaces } => {
            let every_row = RowsSpec {
                offset: 0,
                limit: None,
                ..rows.spec()
            };
            match ResultRows::search(targets, &query, scope, every_row) {
                Ok(found) => {
                    let made = Made {
                        user,
                        replaces,
                        cap: snapshot_cap(volumes),
                    };
                    make_snapshot(snapshots, made, found, &rows, emit);
                }
                Err(not_indexed) => reply(emit, refusal(not_indexed.to_string())),
            }
        }
    }
}

/// Who makes a result snapshot, and what makes room for it.
struct Made<'a> {
    user: &'a [u8],
    replaces: Option<ResultSnapshotId>,
    /// The most rows all result snapshots hold.
    cap: u64,
}

/// The most rows all result snapshots hold: twice the entries of every
/// index, two empty searches.
fn snapshot_cap(volumes: &VolumeSet) -> u64 {
    2 * volumes.iter().map(|v| v.index().len() as u64).sum::<u64>()
}

/// Keeps `found`, a search for every row, as a result snapshot, answers its
/// id, and streams the rows that `rows`' offset and limit ask for.
fn make_snapshot(
    snapshots: &Snapshots,
    made: Made<'_>,
    found: ResultRows<Arc<VolumeState>>,
    rows: &Rows,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let (snapshot, volumes) = match found.into_snapshot() {
        Ok(kept) => kept,
        Err(too_many) => return reply(emit, refusal(too_many.to_string())),
    };
    let id = match gozler_winfs::random_u128() {
        Ok(bits) => ResultSnapshotId(bits),
        Err(e) => return reply(emit, refusal(format!("no id for a result snapshot: {e}"))),
    };
    let snapshot = Arc::new(snapshot);
    snapshots.keep(
        Kept {
            id,
            user: made.user.to_vec(),
            last_read: Instant::now(),
            snapshot: snapshot.clone(),
            volumes: volumes.iter().map(Arc::downgrade).collect(),
        },
        made.replaces,
        made.cap,
    );
    let total = snapshot.total();
    let mut made_frame = Vec::new();
    encode_response_frame(&Response::ResultSnapshot { id, total }, &mut made_frame);
    if !emit(made_frame) {
        return;
    }
    let read = SnapshotRowsRequest {
        id,
        offset: rows.offset,
        limit: rows.limit,
        want_size: rows.want_size,
        want_mtime: rows.want_mtime,
        want_created: rows.want_created,
        want_attributes: rows.want_attributes,
    };
    let sums = rows.want_sums.then(|| snapshot.sums()).flatten();
    stream_snapshot(&snapshot, volumes, &read, sums, emit);
}

/// Streams the rows of the result snapshot that `read` names, or says it is
/// gone.
fn read_snapshot(
    volumes: &VolumeSet,
    snapshots: &Snapshots,
    read: &SnapshotRowsRequest,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let found = snapshots.get(read.id, Instant::now());
    let Some((snapshot, held)) = found else {
        return reply(emit, snapshot_gone());
    };
    let Some(held) = live_volumes(&held, volumes) else {
        return reply(emit, snapshot_gone());
    };
    stream_snapshot(&snapshot, held, read, None, emit);
}

/// Sorts the result snapshot that `sort` names again with no search, keeps
/// the sorted one in its place, answers its id, and streams the rows that
/// `sort.rows`' offset and limit ask for; or says it is gone.
fn sort_snapshot(
    volumes: &VolumeSet,
    snapshots: &Snapshots,
    user: &[u8],
    sort: &SortSnapshotRequest,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let found = snapshots.get(sort.id, Instant::now());
    let Some((snapshot, held)) = found else {
        return reply(emit, snapshot_gone());
    };
    let Some(held) = live_volumes(&held, volumes) else {
        return reply(emit, snapshot_gone());
    };
    let Ok(sorted) = snapshot.sort_again(held, sort.rows.sort) else {
        return reply(emit, snapshot_gone());
    };
    let made = Made {
        user,
        replaces: Some(sort.id),
        cap: snapshot_cap(volumes),
    };
    make_snapshot(snapshots, made, sorted, &sort.rows, emit);
}

/// Streams the rows of `snapshot` that `read` asks for, from `volumes`, its
/// volumes, as [`stream_rows`] streams a search's; each page carries `sums`.
/// An index replaced meanwhile ends the reply with the gone error.
fn stream_snapshot(
    snapshot: &ResultSnapshot,
    volumes: Vec<Arc<VolumeState>>,
    read: &SnapshotRowsRequest,
    sums: Option<MatchSums>,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let mut rows = snapshot.read(volumes, read.offset, read.limit);
    let mut sent = SentFolders::default();
    loop {
        match encode_snapshot_page(&mut rows, read, sums, &mut sent) {
            Ok((page, more)) => {
                if !emit(page) || !more {
                    return;
                }
            }
            Err(_) => return reply(emit, snapshot_gone()),
        }
    }
}

/// The reply to a read of a result snapshot that no longer answers.
fn snapshot_gone() -> Response {
    Response::Error {
        code: ProtoError::ResultSnapshotGone,
        message: "the result snapshot is gone; search again".into(),
    }
}

/// Encodes one tagged reply frame and hands it to `emit`.
fn reply(emit: &mut dyn FnMut(Vec<u8>) -> bool, response: Response) {
    let mut out = Vec::new();
    encode_response_frame(&response, &mut out);
    let _ = emit(out);
}

/// The reply that refuses a request the daemon cannot run as sent.
fn refusal(message: String) -> Response {
    Response::Error {
        code: ProtoError::BadQuery,
        message,
    }
}

/// Streams `rows` as one or more binary results-page frames (paged by
/// [`ResultsPage::push`](gozler_core::proto::ResultsPage::push), `more` set
/// on all but the last). An empty result set still emits one `more: false`
/// page so the client always sees a terminal frame. No full path is built:
/// each page sends the folders its rows need that no earlier page sent, and
/// each row its folder and its name. A page releases the volumes' locks
/// before it is sent, so the pipe and a slow client never hold a lock.
fn stream_rows(
    mut rows: ResultRows<Arc<VolumeState>>,
    wanted: &Rows,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
) {
    let mut sent = SentFolders::default();
    loop {
        let (page, more) = encode_page(&mut rows, wanted, &mut sent);
        if !emit(page) || !more {
            return;
        }
    }
}

/// The reply to a [`Returns::Totals`] search.
fn totals(volumes: Vec<Arc<VolumeState>>, query: &CompiledQuery, scope: Option<&str>) -> Response {
    match search_totals(volumes, query, scope) {
        Ok(totals) => Response::Totals(totals.into()),
        Err(TotalsError::NotIndexed(not_indexed)) => refusal(not_indexed.to_string()),
        Err(TotalsError::Overflow(overflow)) => Response::Error {
            code: ProtoError::Overflow,
            message: overflow.to_string(),
        },
    }
}

/// Answers version negotiation with the highest version both sides speak.
///
/// This daemon serves every version from [`OLDEST_PROTO_VERSION`] to
/// [`PROTO_VERSION`], so a client whose range misses all of them is refused.
/// Answering `PROTO_VERSION.min(proto_max)` without that check would answer
/// `proto: 0` to a `proto_max: 0` client, agreeing on a version neither side
/// implements.
fn hello(volumes: &VolumeSet, proto_min: u16, proto_max: u16) -> Response {
    let proto = PROTO_VERSION.min(proto_max);
    if proto < proto_min || proto < OLDEST_PROTO_VERSION {
        return Response::Error {
            code: ProtoError::Unsupported,
            message: format!(
                "client speaks protocol {proto_min}..={proto_max}; this daemon speaks \
                 {OLDEST_PROTO_VERSION}..={PROTO_VERSION}"
            ),
        };
    }
    Response::Hello {
        proto,
        server: format!("gozlerd {}", gozler_core::escompat::version()),
        ready: all_volumes_live(volumes),
    }
}

/// Whether every volume's index is as complete as gozler can make it, via
/// [`gozler_core::types::VolumePhase::is_complete`]. The same bar
/// [`DaemonStatus::volumes_incomplete`] applies, so `Hello.ready` and a `Status`
/// reply can never disagree about whether the index is whole.
fn all_volumes_live(volumes: &VolumeSet) -> bool {
    volumes.iter().all(|v| v.phase().is_complete())
}

/// How many entries the indexed volumes would hold under `settings`. A
/// volume that is not indexed now adds nothing, and neither does what the
/// applied settings left out, because the index does not hold it.
fn count_included(volumes: &VolumeSet, settings: &IndexSettings) -> u64 {
    volumes
        .iter()
        .map(|v| {
            let rules = settings.rules_for(&v.guid);
            if rules.leaves_out_volume() {
                0
            } else {
                v.read().index().count_kept(&rules)
            }
        })
        .sum()
}

fn build_status(volumes: &VolumeSet, include: &Include) -> DaemonStatus {
    let mut incomplete = false;
    let vols = volumes
        .iter()
        .map(|v| {
            let read = v.read();
            if !read.phase().is_complete() {
                incomplete = true;
            }
            let index = read.index();
            let stats = index.stats();
            let mem = index.memory();
            let pair = |c: gozler_core::index::ComponentBytes| MemPair {
                used: c.used,
                alloc: c.allocated,
            };
            VolumeStatus {
                guid: v.guid.clone(),
                mounts: v.mounts.clone(),
                phase: read.phase().clone(),
                progress: read.progress,
                entries: index.len() as u64,
                generation: index.generation(),
                metadata_pending: read.metadata_pending(),
                journal_max_bytes: read.journal_max_bytes(),
                placeholders_created: stats.placeholders_created,
                delete_of_unknown: stats.delete_of_unknown,
                stale_slots: stats.stale_slots,
                link_reconciles_dropped: stats.link_reconciles_dropped,
                memory: Some(VolumeMemory {
                    entries: pair(mem.entries),
                    arena_raw: pair(mem.arena_raw),
                    arena_folded: pair(mem.arena_folded),
                    frn_map: pair(mem.frn_map),
                    name_tables: pair(mem.name_tables),
                    dir_children: pair(mem.dir_children),
                    frn_map_kind: mem.frn_map_kind.to_string(),
                }),
            }
        })
        .collect();
    let self_mem = gozler_winfs::self_memory().unwrap_or_default();
    DaemonStatus {
        volumes: vols,
        volumes_incomplete: incomplete,
        process_working_set: self_mem.working_set,
        process_private_bytes: self_mem.private_bytes,
        include: Some((*include.applied()).clone()),
        left_out: include.left_out(),
    }
}

/// A volume's index, held through [`VolumeState::read`] for a search.
pub(crate) struct HeldIndex<'a>(VolumeRead<'a>);

impl Deref for HeldIndex<'_> {
    type Target = VolumeIndex;

    fn deref(&self) -> &VolumeIndex {
        self.0.index()
    }
}

impl gozler_core::query::IndexedVolume for VolumeState {
    type Index<'a> = HeldIndex<'a>;

    fn read(&self) -> HeldVolume<HeldIndex<'_>> {
        let read = VolumeState::read(self);
        HeldVolume {
            complete: read.phase().is_complete(),
            metadata_pending: read.metadata_pending(),
            index: HeldIndex(read),
        }
    }

    fn mounts(&self) -> VolumeMounts<'_> {
        self.mounted()
    }
}

#[cfg(test)]
mod tests {
    use super::{Snapshots, build_status, serve};
    use crate::include::Include;
    use crate::result_snapshots::ResultSnapshots;
    use crate::volume_state::{Indexed, VolumeSet, VolumeState};
    use gozler_core::include::{DriveChoice, FolderChoice, IndexSettings};
    use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
    use gozler_core::proto::{
        FrameDecoder, MAX_SERVER_FRAME, PAGE_ROWS, PROTO_VERSION, ProtoError, Request, Response,
        Returns, Rows, SearchRequest, decode_response_frame, encode_request,
    };
    use gozler_core::query::{ParseOptions, ResultNames, parse_query_with};
    use gozler_core::types::{Frn, SortKey, SortSpec, VolumePhase};
    use gozler_core::usn::record::{
        FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE,
    };
    use std::sync::Arc;

    fn dir_record(frn: u128, parent: u128, name: &str) -> ParsedUsnRecord {
        ParsedUsnRecord {
            frn: Frn(frn),
            parent_frn: Frn(parent),
            reason: USN_REASON_FILE_CREATE,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            name: name.as_bytes().to_vec(),
        }
    }

    /// `C:\Windows\System32` as an index, built through the public API only.
    fn windows_index() -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        index.insert_enum(&dir_record(100, NTFS_ROOT_FRN.0, "Windows"));
        index.insert_enum(&dir_record(101, 100, "System32"));
        index
    }

    fn volume(mount: &str, index: VolumeIndex, phase: VolumePhase) -> Arc<VolumeState> {
        let indexed = Indexed {
            index,
            metadata_pending: false,
            journal_max_bytes: None,
            built: true,
        };
        Arc::new(VolumeState::in_state(mount, indexed, phase))
    }

    fn live_volume(mount: &str) -> Arc<VolumeState> {
        volume(mount, windows_index(), VolumePhase::Live)
    }

    /// An index whose name order is the REVERSE of its path order: `a\z.log`
    /// sorts after `z\a.log` by name but before it by path. A fixture where the
    /// two agree would pass even with the name key computed wrongly.
    fn merge_index() -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        index.insert_enum(&dir_record(100, NTFS_ROOT_FRN.0, "a"));
        index.insert_enum(&dir_record(101, NTFS_ROOT_FRN.0, "z"));
        for (frn, parent, name) in [(102, 100, "z.log"), (103, 101, "a.log")] {
            let mut rec = dir_record(frn, parent, name);
            rec.attributes = 0;
            index.insert_enum(&rec);
        }
        index
    }

    fn set(vols: &[Arc<VolumeState>]) -> VolumeSet {
        Arc::new(vols.to_vec())
    }

    /// The default Include settings, which keep everything.
    fn defaults() -> Include {
        Include::new(IndexSettings::default(), None, Box::new(|| {}))
    }

    /// A Snapshot volume must NOT taint results.
    ///
    /// It has no USN journal and never can (Windows refuses to create one on a
    /// recovery partition), so its index is finished: no waiting improves it and
    /// no action fixes it. Flagging it would put an always-on "results may be
    /// incomplete" warning on every query, which nobody reads when it finally
    /// matters.
    #[test]
    fn snapshot_volumes_do_not_taint_results() {
        let vols = set(&[
            live_volume(r"C:\"),
            volume(r"D:\", windows_index(), VolumePhase::Snapshot),
        ]);
        assert!(
            !build_status(&vols, &defaults()).volumes_incomplete,
            "a fully indexed journal-less volume must not flag results incomplete"
        );
        assert!(
            !results(&encode_query_binary(&vols, query("System32")))[0].volumes_incomplete,
            "and no query page may flag it either"
        );
        let Response::Hello { ready, .. } =
            answer(&vols, hello_request(PROTO_VERSION, PROTO_VERSION))
        else {
            panic!("must negotiate")
        };
        assert!(ready, "a snapshot volume is ready to serve");
    }

    /// The honesty bit: any volume whose index is NOT as complete as gozler can make
    /// it must mark results incomplete, or the user trusts a partial index.
    #[test]
    fn status_is_incomplete_unless_every_volume_is_live() {
        assert!(!build_status(&set(&[live_volume(r"C:\")]), &defaults()).volumes_incomplete);

        for phase in [
            VolumePhase::Bootstrapping,
            VolumePhase::Rescanning,
            VolumePhase::Offline,
            VolumePhase::Failed {
                reason: "test".into(),
            },
        ] {
            let vols = set(&[live_volume(r"C:\"), volume(r"D:\", windows_index(), phase)]);
            assert!(
                build_status(&vols, &defaults()).volumes_incomplete,
                "one non-Live volume must taint the whole status"
            );
        }
    }

    /// The preview counts what each volume would keep: a folder left out
    /// takes everything below it, and a drive left out counts nothing.
    #[test]
    fn counting_included_entries_follows_the_settings() {
        let vols = set(&[live_volume(r"C:\")]);
        let all = vols[0].read().index().len() as u64;
        let count = |settings| match answer(&vols, Request::CountIncluded(settings)) {
            Response::IncludedCount { entries } => entries,
            other => panic!("an entry count, not {other:?}"),
        };
        let drive = |indexed, folders| IndexSettings {
            drives: vec![DriveChoice {
                volume: vols[0].guid.clone(),
                name: String::new(),
                indexed,
                folders,
            }],
            ..IndexSettings::default()
        };
        let windows = FolderChoice {
            path: "Windows".into(),
            indexed: false,
        };

        assert_eq!(count(IndexSettings::default()), all);
        assert_eq!(
            count(drive(true, vec![windows])),
            all - 2,
            "Windows and System32 are left out"
        );
        assert_eq!(count(drive(false, Vec::new())), 0);
    }

    /// The payload of the frame a client sends for `request`.
    fn request_frame(request: &Request) -> Vec<u8> {
        let mut frame = Vec::new();
        encode_request(request, &mut frame);
        frame.split_off(4)
    }

    /// Every frame the daemon replies to `request` with, back to back as the
    /// pipe carries them.
    fn reply_bytes(volumes: &VolumeSet, request: &Request) -> Vec<u8> {
        reply_bytes_in(volumes, &ResultSnapshots::new(), request)
    }

    /// [`reply_bytes`] with the result snapshots of earlier requests.
    fn reply_bytes_in(volumes: &VolumeSet, snapshots: &Snapshots, request: &Request) -> Vec<u8> {
        let mut out = Vec::new();
        serve(
            volumes,
            &defaults(),
            snapshots,
            b"",
            &request_frame(request),
            &mut |reply| {
                out.extend_from_slice(&reply);
                true
            },
        );
        out
    }

    /// A search as a test writes it: query text, which [`search`] parses as
    /// gozler does, and the rows it asks for.
    struct TextQuery {
        query: String,
        scope: Option<String>,
        sort: SortSpec,
        offset: u32,
        limit: Option<u32>,
        want_size: bool,
        want_mtime: bool,
        match_case: bool,
    }

    fn search(q: TextQuery) -> Request {
        let options = ParseOptions {
            match_case: q.match_case,
            ..ParseOptions::default()
        };
        Request::Search(SearchRequest {
            query: parse_query_with(&q.query, options).expect("the test's query parses"),
            scope: q.scope,
            returns: Returns::Rows(Rows {
                sort: q.sort,
                offset: q.offset,
                limit: q.limit,
                want_size: q.want_size,
                want_mtime: q.want_mtime,
                want_created: false,
                want_attributes: false,
                want_sums: false,
            }),
        })
    }

    /// The reply to the search `q`.
    fn encode_query_binary(volumes: &VolumeSet, q: TextQuery) -> Vec<u8> {
        reply_bytes(volumes, &search(q))
    }

    /// The one frame the daemon replies to `request` with.
    fn answer(volumes: &VolumeSet, request: Request) -> Response {
        let mut frames = decode_frames(&reply_bytes(volumes, &request));
        assert_eq!(frames.len(), 1, "one reply");
        frames.remove(0)
    }

    fn query(q: &str) -> TextQuery {
        TextQuery {
            query: q.to_string(),
            scope: None,
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            want_size: true,
            want_mtime: true,
            match_case: false,
        }
    }

    /// Splits the daemon's own output for one request back into responses.
    /// The wire is `[len:u32-le][payload]` repeated, and every decoder used
    /// here is public gozler-core API, with one table of folders for the
    /// reply, so the test reads exactly what the CLI would.
    fn decode_frames(bytes: &[u8]) -> Vec<gozler_core::proto::Response> {
        let mut out = Vec::new();
        let mut names = ResultNames::default();
        let mut i = 0;
        while i < bytes.len() {
            let len = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
            i += 4;
            out.push(decode_response_frame(&bytes[i..i + len], &mut names).expect("decodes"));
            i += len;
        }
        out
    }

    fn results(bytes: &[u8]) -> Vec<gozler_core::proto::QueryResults> {
        decode_frames(bytes)
            .into_iter()
            .map(|r| match r {
                gozler_core::proto::Response::Results(r) => r,
                other => panic!("expected Results, got {other:?}"),
            })
            .collect()
    }

    /// The documented terminal-frame invariant. A client reads until it sees
    /// `more: false`, so a zero-match query that emitted no frame at all would
    /// hang it rather than return nothing.
    #[test]
    fn empty_result_set_still_emits_one_terminal_frame() {
        let vols = set(&[live_volume(r"C:\")]);
        let pages = results(&encode_query_binary(&vols, query("no-such-file-anywhere")));
        assert_eq!(pages.len(), 1, "exactly one frame");
        assert!(!pages[0].more, "the only frame must be terminal");
        assert_eq!(pages[0].items.len(), 0);
        assert_eq!(pages[0].total, 0);
    }

    /// A request that fails to decode must come back as a single tagged JSON
    /// error, never as an empty results page (which would read as "no
    /// matches").
    #[test]
    fn a_malformed_request_returns_an_error_frame_not_an_empty_page() {
        let vols = set(&[live_volume(r"C:\")]);
        let mut replies = Vec::new();
        serve(
            &vols,
            &defaults(),
            &ResultSnapshots::new(),
            b"",
            br#"{"t":"Search","query":"#,
            &mut |reply| {
                replies.extend_from_slice(&reply);
                true
            },
        );
        let frames = decode_frames(&replies);
        assert_eq!(frames.len(), 1);
        assert!(
            matches!(frames[0], gozler_core::proto::Response::Error { .. }),
            "got {:?}",
            frames[0]
        );
    }

    /// A search for the totals of `query` below `scope`.
    fn totals_search(query: &str, scope: Option<&str>) -> Request {
        Request::Search(SearchRequest {
            query: parse_query_with(query, ParseOptions::default()).expect("the query parses"),
            scope: scope.map(str::to_string),
            returns: Returns::Totals,
        })
    }

    /// A total past 64 bits, which only a corrupt size in the index can make,
    /// answers `Overflow`, which gozler exits by as an internal error, rather
    /// than a wrapped total that looks valid.
    #[test]
    fn a_total_past_64_bits_answers_overflow() {
        use gozler_core::layout::{LayoutFile, LayoutName};
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        for (frn, size) in [(100, u64::MAX - 1), (101, 2)] {
            index.enrich(&LayoutFile {
                frn: Frn(frn),
                attributes: 0x20,
                size: Some(size),
                mtime_ft: Some(5),
                created_ft: None,
                names: vec![LayoutName {
                    parent_frn: NTFS_ROOT_FRN,
                    name: format!("{frn}.bin").into_bytes(),
                    dos_only: false,
                }],
            });
        }
        let vols = set(&[volume(r"C:\", index, VolumePhase::Live)]);
        match answer(&vols, totals_search("", None)) {
            Response::Error { code, .. } => assert_eq!(code, ProtoError::Overflow),
            other => panic!("an overflow must be refused, got {other:?}"),
        }
    }

    /// The sums a window asks for reach it through the pipe, over every
    /// match and not only the page's rows, and a folder inside a matching
    /// folder adds nothing to the folders' bytes.
    #[test]
    fn a_search_that_wants_sums_gets_them_over_the_pipe() {
        use gozler_core::layout::{LayoutFile, LayoutName};
        use gozler_core::query::MatchSums;
        let mut index = windows_index();
        index.enrich(&LayoutFile {
            frn: Frn(102),
            attributes: 0x20,
            size: Some(5),
            mtime_ft: Some(5),
            created_ft: None,
            names: vec![LayoutName {
                parent_frn: Frn(101),
                name: b"kernel32.dll".to_vec(),
                dos_only: false,
            }],
        });
        index.compute_folder_sizes();
        let vols = set(&[volume(r"C:", index, VolumePhase::Live)]);
        let mut request = search(TextQuery {
            limit: Some(1),
            ..query("")
        });
        let Request::Search(SearchRequest {
            returns: Returns::Rows(rows),
            ..
        }) = &mut request
        else {
            unreachable!("search asks for rows")
        };
        rows.want_sums = true;
        let pages = results(&reply_bytes(&vols, &request));
        let last = pages.last().expect("one page at least");
        assert_eq!(last.items.len(), 1, "the limit cuts the rows");
        assert_eq!(
            last.sums,
            Some(MatchSums {
                files: 1,
                file_bytes: 5,
                files_without_size: 0,
                folders: 2,
                folder_bytes: 5,
                folder_bytes_at_least: false,
            })
        );
    }

    /// A result snapshot answers its id and the rows asked for, a later
    /// read by that id answers the rows at its offset with the same total,
    /// a sort answers a new one in the new order, which replaces it, and an
    /// id the daemon no longer keeps answers the gone error, which the
    /// window answers with a new search.
    #[test]
    fn a_result_snapshot_is_read_by_its_id_until_it_is_gone() {
        use gozler_core::proto::{SnapshotRowsRequest, SortSnapshotRequest};
        use gozler_core::types::SortDir;
        let vols = set(&[live_volume(r"C:\")]);
        let snapshots = ResultSnapshots::new();
        let made = Request::Search(SearchRequest {
            query: parse_query_with("", ParseOptions::default()).expect("parses"),
            scope: None,
            returns: Returns::ResultSnapshot {
                rows: Rows {
                    sort: SortSpec::default_for(SortKey::Name),
                    offset: 0,
                    limit: Some(1),
                    want_size: false,
                    want_mtime: false,
                    want_created: false,
                    want_attributes: false,
                    want_sums: false,
                },
                replaces: None,
            },
        });
        let frames = decode_frames(&reply_bytes_in(&vols, &snapshots, &made));
        let Response::ResultSnapshot { id, total: 2 } = frames[0] else {
            panic!("the id of 2 rows first: {:?}", frames[0])
        };
        let paths = |frame: &Response| match frame {
            Response::Results(page) => (
                page.total,
                page.items
                    .iter()
                    .map(|i| i.path.clone())
                    .collect::<Vec<_>>(),
            ),
            other => panic!("a results page: {other:?}"),
        };
        assert_eq!(frames.len(), 2);
        assert_eq!(
            paths(&frames[1]),
            (2, vec![r"C:\Windows\System32".to_string()])
        );

        let read = |id| {
            Request::ResultSnapshotRows(SnapshotRowsRequest {
                id,
                offset: 1,
                limit: None,
                want_size: false,
                want_mtime: false,
                want_created: false,
                want_attributes: false,
            })
        };
        let frames = decode_frames(&reply_bytes_in(&vols, &snapshots, &read(id)));
        assert_eq!(paths(&frames[0]), (2, vec![r"C:\Windows".to_string()]));

        let sort = Request::SortResultSnapshot(SortSnapshotRequest {
            id,
            rows: Rows {
                sort: SortSpec {
                    key: SortKey::Name,
                    dir: SortDir::Desc,
                },
                offset: 0,
                limit: None,
                want_size: false,
                want_mtime: false,
                want_created: false,
                want_attributes: false,
                want_sums: false,
            },
        });
        let frames = decode_frames(&reply_bytes_in(&vols, &snapshots, &sort));
        let Response::ResultSnapshot {
            id: sorted,
            total: 2,
        } = frames[0]
        else {
            panic!("the id of the sorted rows first: {:?}", frames[0])
        };
        assert_ne!(sorted, id);
        assert_eq!(
            paths(&frames[1]),
            (
                2,
                vec![
                    r"C:\Windows".to_string(),
                    r"C:\Windows\System32".to_string()
                ]
            )
        );
        let frames = decode_frames(&reply_bytes_in(&vols, &snapshots, &read(id)));
        assert!(
            matches!(
                frames[..],
                [Response::Error {
                    code: ProtoError::ResultSnapshotGone,
                    ..
                }]
            ),
            "{frames:?}"
        );
    }

    /// A scope on no indexed volume answers `BadQuery`, which gozler exits by
    /// as invalid input, for rows and totals alike, because an empty answer
    /// would read as "no such file".
    #[test]
    fn a_scope_on_no_indexed_volume_answers_bad_query() {
        let vols = set(&[live_volume(r"C:\")]);
        let mut rows = query("System32");
        rows.scope = Some(r"E:\x".to_string());
        for request in [search(rows), totals_search("System32", Some(r"E:\x"))] {
            match answer(&vols, request) {
                Response::Error { code, .. } => assert_eq!(code, ProtoError::BadQuery),
                other => panic!("a scope on no volume must be refused, got {other:?}"),
            }
        }
    }

    /// Paging must set `more` on every frame but the last, and `total` must be
    /// the whole match count on every page, not the page's own length.
    #[test]
    fn pages_carry_more_until_the_last_and_a_stable_total() {
        let n = PAGE_ROWS as u64 + 1;
        let vols = set(&[volume(r"C:\", paging_index(), VolumePhase::Live)]);

        let pages = results(&encode_query_binary(&vols, query("hit")));
        assert_eq!(pages.len(), 2, "PAGE_ROWS + 1 matches must span two frames");
        assert!(pages[0].more, "first page must signal more");
        assert!(!pages[1].more, "last page must be terminal");
        assert_eq!(pages[0].items.len(), PAGE_ROWS as usize);
        assert_eq!(pages[1].items.len(), 1);
        assert_eq!(pages[0].total, n, "total is the whole match count");
        assert_eq!(pages[1].total, n, "total must not drift across pages");
    }

    /// One folder holding [`PAGE_ROWS`] + 1 matches of `hit`, so a reply
    /// takes two pages.
    fn paging_index() -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        index.insert_enum(&dir_record(100, NTFS_ROOT_FRN.0, "many"));
        for i in 0..=u128::from(PAGE_ROWS) {
            let mut rec = dir_record(1000 + i, 100, &format!("hit{i:05}.log"));
            rec.attributes = 0;
            index.insert_enum(&rec);
        }
        index
    }

    /// A volume whose index gets new entry indexes between two pages sends
    /// no more rows, and the pages after it say the answer is incomplete, or
    /// the client takes the short answer as whole.
    #[test]
    fn an_index_changed_between_pages_marks_the_answer_incomplete() {
        let vol = volume(r"C:\", paging_index(), VolumePhase::Live);
        let vols = set(&[Arc::clone(&vol)]);
        let mut out = Vec::new();
        serve(
            &vols,
            &defaults(),
            &ResultSnapshots::new(),
            b"",
            &request_frame(&search(query("hit"))),
            &mut |reply| {
                if out.is_empty() {
                    let mut index = vol.index_mut();
                    let mut gone = dir_record(1000, 100, "");
                    gone.reason = gozler_core::usn::record::USN_REASON_FILE_DELETE;
                    index.apply_batch(&[gone]);
                    index.optimize_storage();
                }
                out.extend_from_slice(&reply);
                true
            },
        );
        let pages = results(&out);
        assert_eq!(pages.len(), 2);
        assert!(!pages[0].volumes_incomplete);
        assert!(pages[1].volumes_incomplete, "the rows stopped");
        assert!(pages[1].items.is_empty());
        assert_eq!(pages[1].total, u64::from(PAGE_ROWS), "the last row is out");
    }

    /// Every page must fit the 16 MiB frame cap the client decodes with, or
    /// the client refuses it and the search fails with exit 7. A page sends
    /// the folders its rows need, and [`PAGE_ROWS`] rows that each sit in a
    /// folder chain of their own pass that cap; any local user can create
    /// them.
    #[test]
    fn pages_of_long_paths_fit_the_clients_frame_cap() {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        // Each row sits below 17 folders of its own with 250-character
        // names, so PAGE_ROWS rows would need 18 MB of folders.
        let mut frn = 100;
        for i in 0..PAGE_ROWS as u64 {
            let mut parent = NTFS_ROOT_FRN.0;
            for _ in 0..17 {
                index.insert_enum(&dir_record(frn, parent, &"d".repeat(250)));
                parent = frn;
                frn += 1;
            }
            let mut rec = dir_record(frn, parent, &format!("hit{i:05}.log"));
            rec.attributes = 0;
            index.insert_enum(&rec);
            frn += 1;
        }
        let vols = set(&[volume(r"C:\", index, VolumePhase::Live)]);

        let mut decoder = FrameDecoder::new(MAX_SERVER_FRAME);
        decoder.feed(&encode_query_binary(&vols, query("hit")));
        let mut names = ResultNames::default();
        let mut pages = Vec::new();
        while let Some(frame) = decoder
            .next_frame()
            .expect("every frame fits the client's cap")
        {
            match decode_response_frame(&frame, &mut names).expect("decodes") {
                Response::Results(page) => pages.push(page),
                other => panic!("expected Results, got {other:?}"),
            }
        }
        assert_eq!(
            pages.len(),
            2,
            "the byte cap splits the rows over two pages"
        );
        assert!(pages[0].more, "first page must signal more");
        assert!(!pages[1].more, "last page must be terminal");
        let names: Vec<String> = pages
            .iter()
            .flat_map(|page| &page.items)
            .map(|item| item.path.rsplit('\\').next().unwrap().to_string())
            .collect();
        let expected: Vec<String> = (0..PAGE_ROWS).map(|i| format!("hit{i:05}.log")).collect();
        assert_eq!(names, expected, "every row arrives once, in order");
    }

    /// When the page consumer reports it is gone (`emit` returns false, i.e.
    /// the client vanished or the pipe write failed), the encoder must stop
    /// instead of encoding every remaining page into the void.
    #[test]
    fn streaming_stops_when_the_consumer_goes_away() {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        index.insert_enum(&dir_record(100, NTFS_ROOT_FRN.0, "many"));
        for i in 0..(PAGE_ROWS as u64 * 3) {
            let mut rec = dir_record(1000 + u128::from(i), 100, &format!("hit{i:05}.log"));
            rec.attributes = 0;
            index.insert_enum(&rec);
        }
        let vols = set(&[volume(r"C:\", index, VolumePhase::Live)]);

        let mut emitted = 0usize;
        serve(
            &vols,
            &defaults(),
            &ResultSnapshots::new(),
            b"",
            &request_frame(&search(query("hit"))),
            &mut |_page| {
                emitted += 1;
                false // consumer gone after the first page
            },
        );
        assert_eq!(
            emitted, 1,
            "a dead consumer must stop the encoder after the rejected page"
        );
    }

    /// `want_size: false` must suppress the value even when the index holds one,
    /// and the mount prefix must be prepended to every emitted path.
    #[test]
    fn flags_suppress_metadata_and_paths_carry_the_mount_prefix() {
        let vols = set(&[live_volume(r"C:\")]);
        let mut q = query("System32");
        q.want_size = false;
        q.want_mtime = false;

        let pages = results(&encode_query_binary(&vols, q));
        let item = &pages[0].items[0];
        assert!(item.size.is_none(), "want_size: false must suppress size");
        assert!(
            item.mtime_ft.is_none(),
            "want_mtime: false must suppress mtime"
        );
        assert!(
            item.path.starts_with(r"C:\"),
            "path {:?} must carry the mount prefix",
            item.path
        );
    }

    /// A volume that is not Live must taint every page, not just the first, so a
    /// client that only reads the last frame still learns the set is partial.
    #[test]
    fn incomplete_volumes_taint_every_page() {
        let vols = set(&[
            live_volume(r"C:\"),
            volume(r"D:\", windows_index(), VolumePhase::Rescanning),
        ]);
        let pages = results(&encode_query_binary(&vols, query("System32")));
        assert!(
            pages.iter().all(|p| p.volumes_incomplete),
            "every page must report the index is incomplete"
        );
    }

    /// Offset past the end is a legal request and must terminate cleanly rather
    /// than emit nothing.
    #[test]
    fn offset_and_limit_slice_the_result_set() {
        let vols = set(&[live_volume(r"C:\")]);

        let mut q = query("System32");
        q.offset = 500;
        let pages = results(&encode_query_binary(&vols, q));
        assert_eq!(pages.len(), 1);
        assert!(!pages[0].more);
        assert_eq!(
            pages[0].items.len(),
            0,
            "offset past the end yields no items"
        );

        let mut q = query("Windows");
        q.limit = Some(1);
        let pages = results(&encode_query_binary(&vols, q));
        assert_eq!(pages[0].items.len(), 1);
    }

    /// Two volumes' hits merge into one global name order. The fixture's name
    /// order is the reverse of its path order, so a merge keyed on the wrong
    /// thing cannot coincidentally produce the right answer.
    #[test]
    fn name_sort_merges_two_volumes_into_one_global_order() {
        let vols = set(&[
            volume(r"C:\", merge_index(), VolumePhase::Live),
            volume(r"D:\", merge_index(), VolumePhase::Live),
        ]);
        let pages = results(&encode_query_binary(&vols, query("log")));
        let names: Vec<&str> = pages[0]
            .items
            .iter()
            .map(|i| i.path.rsplit('\\').next().unwrap())
            .collect();

        assert_eq!(
            names,
            ["a.log", "a.log", "z.log", "z.log"],
            "merged pages must be globally ordered by name, not by path or by volume"
        );
    }

    /// Every emitted path, in wire order, across all pages.
    fn paths(bytes: &[u8]) -> Vec<String> {
        results(bytes)
            .into_iter()
            .flat_map(|p| p.items.into_iter().map(|i| i.path))
            .collect()
    }

    fn hello_request(proto_min: u16, proto_max: u16) -> Request {
        Request::Hello {
            proto_min,
            proto_max,
            client: "test".to_string(),
        }
    }

    /// Negotiation answers this build's version to a client whose range holds
    /// it, and refuses a client of an older protocol rather than answer a
    /// cheerful `ready: true` and let the mismatch surface as garbled frames
    /// later: gozlerd serves no older protocol.
    #[test]
    fn hello_answers_this_version_or_refuses() {
        let vols = set(&[live_volume(r"C:\")]);

        for range in [(1, PROTO_VERSION), (PROTO_VERSION, PROTO_VERSION)] {
            match answer(&vols, hello_request(range.0, range.1)) {
                Response::Hello { proto, .. } => assert_eq!(proto, PROTO_VERSION, "{range:?}"),
                other => panic!("{range:?} must negotiate, got {other:?}"),
            }
        }

        // An older client: its results pages lay out full paths.
        match answer(&vols, hello_request(PROTO_VERSION - 1, PROTO_VERSION - 1)) {
            Response::Error { code, .. } => assert_eq!(code, ProtoError::Unsupported),
            other => panic!("an older protocol must be refused, got {other:?}"),
        }

        // Floor above us: the client needs a protocol this build cannot speak.
        match answer(&vols, hello_request(PROTO_VERSION + 1, PROTO_VERSION + 2)) {
            Response::Error { code, .. } => assert_eq!(code, ProtoError::Unsupported),
            other => panic!("a floor above this version must be refused, got {other:?}"),
        }

        // Ceiling below us: no overlap either, and `proto: 0` would be a
        // version neither side implements.
        match answer(&vols, hello_request(0, 0)) {
            Response::Error { code, .. } => assert_eq!(code, ProtoError::Unsupported),
            other => panic!("proto_max: 0 must be refused, got {other:?}"),
        }
    }

    /// `Hello.ready` and `Status.volumes_incomplete` answer the same question,
    /// so they must never contradict each other.
    #[test]
    fn hello_ready_never_contradicts_status() {
        for vols in [
            set(&[live_volume(r"C:\")]),
            set(&[
                live_volume(r"C:\"),
                volume(r"D:\", windows_index(), VolumePhase::Bootstrapping),
            ]),
        ] {
            let Response::Hello { ready, .. } =
                answer(&vols, hello_request(PROTO_VERSION, PROTO_VERSION))
            else {
                panic!("a client of this protocol must negotiate")
            };
            assert_eq!(
                ready,
                !build_status(&vols, &defaults()).volumes_incomplete,
                "ready must track the same bar as volumes_incomplete"
            );
        }
    }

    /// Every way two hits can tie or nearly tie: es path order against a byte
    /// compare (`a~qx7zv` against `a\qx7zv`), equal sizes and dates, an unknown
    /// size, names equal after folding, and one file with two links of the
    /// same name.
    fn order_index() -> VolumeIndex {
        use gozler_core::layout::{LayoutFile, LayoutName};
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        index.insert_enum(&dir_record(100, NTFS_ROOT_FRN.0, "P"));
        index.insert_enum(&dir_record(101, 100, "a"));
        index.insert_enum(&dir_record(102, 100, "a b"));
        let mut sizeless = dir_record(205, 102, "nosize.qx7zv");
        sizeless.attributes = 0;
        index.insert_enum(&sizeless);
        for (frn, links, size) in [
            (200, &[(100, "a~qx7zv")][..], 10),
            (201, &[(101, "qx7zv")][..], 10),
            (202, &[(102, "QX7ZV")][..], 30),
            (203, &[(101, "link.qx7zv"), (102, "link.qx7zv")][..], 10),
        ] {
            index.enrich(&LayoutFile {
                frn: Frn(frn),
                attributes: 0x20,
                size: Some(size),
                mtime_ft: Some(5),
                created_ft: None,
                names: links
                    .iter()
                    .map(|&(parent, name)| LayoutName {
                        parent_frn: Frn(parent),
                        name: name.as_bytes().to_vec(),
                        dos_only: false,
                    })
                    .collect(),
            });
        }
        index
    }

    /// `-n K` prints the first K rows of the same query without `-n`, under
    /// every sort, on one volume and across two, whose runs are merged.
    #[test]
    fn a_limited_query_is_a_prefix_of_the_unlimited_one() {
        use gozler_core::types::SortDir;
        for vols in [
            set(&[volume(r"C:\", order_index(), VolumePhase::Live)]),
            set(&[
                volume(r"C:\", order_index(), VolumePhase::Live),
                volume(r"C:\mnt\", order_index(), VolumePhase::Live),
            ]),
        ] {
            for key in [
                SortKey::Name,
                SortKey::Path,
                SortKey::Size,
                SortKey::DateModified,
            ] {
                for dir in [SortDir::Asc, SortDir::Desc] {
                    let page = |limit| {
                        let mut q = query("qx7zv");
                        q.sort = SortSpec { key, dir };
                        q.limit = limit;
                        paths(&encode_query_binary(&vols, q))
                    };
                    let unlimited = page(None);
                    assert_eq!(unlimited.len(), 6 * vols.len());
                    for k in 0..=unlimited.len() {
                        assert_eq!(
                            page(Some(k as u32)),
                            unlimited[..k],
                            "{key:?}/{dir:?} on {} volume(s): -n {k}",
                            vols.len()
                        );
                    }
                }
            }
        }
    }

    /// `-case` (the request's `match_case`) makes matching case-sensitive, and
    /// the `-path` scope follows the same value whether it comes from the
    /// switch or from a `case:` directive.
    #[test]
    fn the_case_switch_reaches_the_matcher_and_the_scope() {
        let vols = set(&[live_volume(r"C:\")]);
        let run = |text: &str, match_case: bool, scope: Option<&str>| {
            let mut q = query(text);
            q.match_case = match_case;
            q.scope = scope.map(str::to_string);
            paths(&encode_query_binary(&vols, q))
        };
        assert!(run("system32", true, None).is_empty());
        assert_eq!(run("System32", true, None), [r"C:\Windows\System32"]);
        assert_eq!(run("system32", false, None), [r"C:\Windows\System32"]);
        // `case:0` overrides the switch.
        assert_eq!(run("system32 case:0", true, None), [r"C:\Windows\System32"]);

        assert!(run("case: System32", false, Some(r"C:\windows")).is_empty());
        assert_eq!(
            run("case: System32", false, Some(r"C:\Windows")),
            [r"C:\Windows\System32"]
        );
    }
}
