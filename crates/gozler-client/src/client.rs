//! The connection: JSON requests and tagged responses over the pipe, whose
//! results pages carry a binary body.

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::AsHandle;
use std::time::{Duration, Instant};

use gozler_core::include::IndexSettings;
use gozler_core::proto::{
    DaemonStatus, FrameDecoder, MAX_SERVER_FRAME, PROTO_VERSION, ProtoError, QueryResults,
    QueryTotals, Request, Response, ResultSnapshotId, SERVICE_NAME, SearchRequest,
    decode_response_frame, encode_request,
};
use gozler_core::query::ResultNames;
use gozler_winfs::ServiceState;

use crate::{CancelToken, ClientError, NotRunning};

/// The pipe is opened at the identification level, so gozlerd can tell which
/// user is searching, and so which entries it may show, but neither gozlerd
/// nor a rogue server that squatted the name can act as this client. gozlerd
/// refuses the searches of a client that opens the pipe anonymously. The
/// level does NOT authenticate the server: see [`Client::connect`], which
/// verifies the pipe's owner is the elevated daemon before any request is
/// sent.
const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

/// How long the daemon may stay silent while a reply is due before the client
/// gives up, so a wedged-but-alive daemon cannot hang it forever. It measures
/// silence, not the whole reply, because a large export streams for longer.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// Poll interval while waiting for a reply (see [`Client::next_response`]).
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The failure for a pipe that does not exist, from the state of the gozler
/// service: a service that is running or changing state may open the pipe
/// soon, so only a service that is not running means gozlerd is not running.
fn missing_pipe() -> ClientError {
    let why = match gozler_winfs::service_state(SERVICE_NAME) {
        Ok(ServiceState::Running | ServiceState::Changing) => return ClientError::NotReady,
        Ok(ServiceState::Stopped) => NotRunning::Stopped,
        Ok(ServiceState::Disabled) => NotRunning::Disabled,
        Ok(ServiceState::NotInstalled) => match std::env::current_exe() {
            Ok(exe) => NotRunning::NotInstalled {
                gozlerd_beside: exe.with_file_name("gozlerd.exe").is_file(),
                folder: exe
                    .parent()
                    .expect("the path of an executable names its folder")
                    .to_path_buf(),
            },
            Err(error) => NotRunning::Unknown(format!("cannot find gozler.exe: {error}")),
        },
        Err(error) => NotRunning::Unknown(error.to_string()),
    };
    ClientError::NotRunning(why)
}

/// One connection to the daemon, which serves its requests one at a time.
pub struct Client {
    pipe: File,
    /// Bytes read past the last whole response.
    decoder: FrameDecoder,
    /// The folders that the current reply's results pages have sent.
    names: ResultNames,
    buf: Vec<u8>,
    cancel: CancelToken,
    /// The daemon's name and version, from its reply to the hello.
    server: String,
}

impl Client {
    /// Connects to the daemon's pipe `pipe_name`, checks that the elevated
    /// daemon owns it when `check_owner` is set, and agrees on
    /// [`PROTO_VERSION`] with the daemon before returning. Every wait of
    /// this connection, its connect included, ends with
    /// [`ClientError::Cancelled`] once `cancel` is set.
    ///
    /// Fails with [`ClientError::NotRunning`] or [`ClientError::NotReady`],
    /// from the state of the gozler service, if no pipe exists,
    /// [`ClientError::Untrusted`] if a server is present but not owned by
    /// the elevated daemon, [`ClientError::TooOld`] for a daemon older than
    /// this client, and [`ClientError::Communication`] for other open errors.
    pub fn connect(
        pipe_name: &str,
        check_owner: bool,
        cancel: CancelToken,
    ) -> Result<Self, ClientError> {
        use std::os::windows::fs::OpenOptionsExt;

        // Win32 error codes (gozler-client intentionally has no windows-sys
        // dep).
        const ERROR_FILE_NOT_FOUND: i32 = 2;
        const ERROR_PATH_NOT_FOUND: i32 = 3;
        const ERROR_PIPE_BUSY: i32 = 231;

        // The daemon keeps a single listening instance and recreates it only
        // after accepting a client, so a burst of concurrent clients can
        // momentarily find no free instance (ERROR_PIPE_BUSY). That is transient
        // and retryable: the daemon IS running, so it must not be reported as
        // "unreachable".
        let mut attempts = 0u32;
        let pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
                .open(pipe_name)
            {
                Ok(pipe) => break pipe,
                Err(e) => match e.raw_os_error() {
                    Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => {
                        return Err(missing_pipe());
                    }
                    // All instances momentarily busy: brief backoff and retry.
                    Some(ERROR_PIPE_BUSY) if attempts < 20 => {
                        if cancel.is_cancelled() {
                            return Err(ClientError::Cancelled);
                        }
                        attempts += 1;
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    // Busy past the retry budget, ERROR_ACCESS_DENIED, or anything
                    // unexpected: the pipe is present but we could not open it,
                    // so surface the real OS error rather than "not running".
                    _ => {
                        return Err(ClientError::Communication(format!(
                            "cannot open {pipe_name}: {e}"
                        )));
                    }
                },
            }
        };

        // Authenticate the server BEFORE sending any request, because a
        // non-elevated user can squat the pipe name (the `pipe_trust` module
        // of gozler-winfs describes the threat). Fail closed on error.
        if check_owner {
            match gozler_winfs::pipe_server_is_trusted(pipe.as_handle()) {
                Ok(true) => {}
                Ok(false) | Err(_) => return Err(ClientError::Untrusted),
            }
        }

        let mut client = Self {
            pipe,
            decoder: FrameDecoder::new(MAX_SERVER_FRAME),
            names: ResultNames::default(),
            // Heap, and larger than one 64 KiB pipe read: a broad query
            // streams hundreds of MB, so fewer, bigger reads cut syscall
            // overhead, and the main thread's 1 MiB stack stays safe.
            buf: vec![0u8; 256 * 1024],
            cancel,
            server: String::new(),
        };
        client.server = client.hello()?;
        Ok(client)
    }

    /// Asks the daemon for this client's protocol version alone, and
    /// returns the daemon's name and version. A daemon older than the client
    /// refuses it; without the question, an old daemon would refuse each
    /// request as malformed, which reads as invalid input.
    fn hello(&mut self) -> Result<String, ClientError> {
        self.send(&Request::Hello {
            proto_min: PROTO_VERSION,
            proto_max: PROTO_VERSION,
            client: concat!("gozler ", env!("CARGO_PKG_VERSION")).to_string(),
        })?;
        match self.next_response()? {
            Response::Hello { server, .. } => Ok(server),
            Response::Error {
                code: ProtoError::Unsupported,
                ..
            } => Err(ClientError::TooOld),
            _ => Err(ClientError::Communication(
                "unexpected reply to hello".into(),
            )),
        }
    }

    /// The daemon's name and version, such as `gozlerd 0.4.0 (2026-10-22
    /// 07:09)`.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Asks for the daemon's state, with the folder links when they differ
    /// from the ones whose fingerprint is `links_held` ([`Request::Status`]).
    pub fn status(&mut self, links_held: Option<u64>) -> Result<DaemonStatus, ClientError> {
        self.send(&Request::Status { links_held })?;
        match self.next_response()? {
            Response::Status(status) => Ok(status),
            _ => Err(ClientError::Communication(
                "unexpected reply to status".into(),
            )),
        }
    }

    /// Runs a query and hands each results page to `page` as it arrives, so
    /// the caller shows the first rows while the daemon still sends the
    /// rest. Stops at the first error `page` returns.
    ///
    /// Fails with [`ClientError::Refused`] when the daemon refuses the query.
    pub fn query<E: From<ClientError>>(
        &mut self,
        request: SearchRequest,
        page: impl FnMut(QueryResults) -> Result<(), E>,
    ) -> Result<(), E> {
        self.rows(Request::Search(request), |_| {}, page)
    }

    /// Sends `request`, whose reply is results pages, and hands them to
    /// `page` as [`Self::query`] does. A search for a result snapshot is
    /// answered first with its id, which goes to `made`.
    ///
    /// Fails with [`ClientError::Refused`] when the daemon refuses the
    /// request, with [`ProtoError::ResultSnapshotGone`] for a result snapshot
    /// it no longer keeps.
    ///
    /// [`ProtoError::ResultSnapshotGone`]: gozler_core::proto::ProtoError::ResultSnapshotGone
    pub fn rows<E: From<ClientError>>(
        &mut self,
        request: Request,
        mut made: impl FnMut(ResultSnapshotId),
        mut page: impl FnMut(QueryResults) -> Result<(), E>,
    ) -> Result<(), E> {
        self.send(&request)?;
        loop {
            match self.next_response()? {
                Response::ResultSnapshot { id, .. } => made(id),
                Response::Results(results) => {
                    let more = results.more;
                    page(results)?;
                    if !more {
                        return Ok(());
                    }
                }
                Response::Error { code, message } => {
                    return Err(ClientError::Refused { code, message }.into());
                }
                _ => {
                    return Err(
                        ClientError::Communication("unexpected reply from daemon".into()).into(),
                    );
                }
            }
        }
    }

    /// Runs a search that asks for [`Returns::Totals`], with the failures of
    /// [`Self::query`].
    ///
    /// [`Returns::Totals`]: gozler_core::proto::Returns::Totals
    pub fn totals(&mut self, request: SearchRequest) -> Result<QueryTotals, ClientError> {
        self.send(&Request::Search(request))?;
        match self.next_response()? {
            Response::Totals(totals) => Ok(totals),
            Response::Error { code, message } => Err(ClientError::Refused { code, message }),
            _ => Err(ClientError::Communication(
                "unexpected reply from daemon".into(),
            )),
        }
    }

    /// Tells the daemon that an administrator saved the Include settings, so
    /// it reads and applies them.
    pub fn settings_saved(&mut self) -> Result<(), ClientError> {
        self.send(&Request::SettingsSaved)?;
        match self.next_response()? {
            Response::SettingsApplied => Ok(()),
            Response::Error { code, message } => Err(ClientError::Refused { code, message }),
            _ => Err(ClientError::Communication(
                "unexpected reply to settings saved".into(),
            )),
        }
    }

    /// How many entries the index would keep under `settings`.
    pub fn count_included(&mut self, settings: IndexSettings) -> Result<u64, ClientError> {
        self.send(&Request::CountIncluded(settings))?;
        match self.next_response()? {
            Response::IncludedCount { entries } => Ok(entries),
            Response::Error { code, message } => Err(ClientError::Refused { code, message }),
            _ => Err(ClientError::Communication(
                "unexpected reply to count included".into(),
            )),
        }
    }

    /// Writes one request. It never waits for the daemon: a request is a few
    /// hundred bytes, and the pipe holds 64 KiB that the daemon has not read.
    fn send(&mut self, request: &Request) -> Result<(), ClientError> {
        // Every reply names only the folders it sent itself.
        self.names = ResultNames::default();
        let mut out = Vec::new();
        encode_request(request, &mut out);
        self.pipe
            .write_all(&out)
            .map_err(|e| ClientError::Communication(e.to_string()))
    }

    /// Reads the next framed response. The wait is enforced by polling
    /// `PeekNamedPipe` rather than issuing a blocking read, so a
    /// wedged-but-alive daemon cannot hang the client past [`READ_DEADLINE`]
    /// of silence, and a cancelled request stops at the next poll.
    fn next_response(&mut self) -> Result<Response, ClientError> {
        let mut deadline = Instant::now() + READ_DEADLINE;
        loop {
            if self.cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            match self.decoder.next_frame() {
                Ok(Some(frame)) => {
                    return decode_response_frame(&frame, &mut self.names)
                        .map_err(|e| ClientError::Communication(e.to_string()));
                }
                Ok(None) => {}
                Err(e) => return Err(ClientError::Communication(e.to_string())),
            }
            let avail = gozler_winfs::pipe_bytes_available(self.pipe.as_handle())
                .map_err(|_| ClientError::Lost)?;
            if avail == 0 {
                if Instant::now() > deadline {
                    return Err(ClientError::Communication(
                        "daemon did not reply in time".into(),
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            let n = self
                .pipe
                .read(&mut self.buf)
                .map_err(|e| ClientError::Communication(e.to_string()))?;
            if n == 0 {
                return Err(ClientError::Lost);
            }
            self.decoder.feed(&self.buf[..n]);
            deadline = Instant::now() + READ_DEADLINE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gozler_core::proto::{PageEnd, ResultsPage};
    use tokio::net::windows::named_pipe::ServerOptions;

    /// A pipe name that no other test or program uses.
    fn pipe_name(test: &str) -> String {
        format!(r"\\.\pipe\gozler-client-test-{test}-{}", std::process::id())
    }

    /// Connects to `pipe_name` with a token that is already cancelled, and
    /// returns the error, if any. The outcome shows whether the wait checked
    /// the token, because a wait that ignores it ends in another error.
    fn connect_cancelled(pipe_name: &str) -> Option<ClientError> {
        let cancel = CancelToken::default();
        cancel.cancel();
        Client::connect(pipe_name, false, cancel).err()
    }

    fn reactor() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("a tokio runtime")
    }

    /// A daemon that accepts the connection and never answers the hello:
    /// without the token check, the client waits for [`READ_DEADLINE`].
    #[test]
    fn a_cancelled_request_stops_waiting_for_the_reply() {
        let runtime = reactor();
        let _reactor = runtime.enter();
        let name = pipe_name("silent");
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .expect("the test pipe is created");
        let error = connect_cancelled(&name);
        assert!(matches!(error, Some(ClientError::Cancelled)), "{error:?}");
    }

    /// A pipe whose only instance another client holds answers every open
    /// with ERROR_PIPE_BUSY: without the token check, the client retries for
    /// about 500 ms and then fails with `Communication`.
    #[test]
    fn a_cancelled_connect_stops_retrying_a_busy_pipe() {
        let runtime = reactor();
        let _reactor = runtime.enter();
        let name = pipe_name("busy");
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&name)
            .expect("the test pipe is created");
        let _first = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("the first client takes the only instance");
        let error = connect_cancelled(&name);
        assert!(matches!(error, Some(ClientError::Cancelled)), "{error:?}");
    }

    /// A daemon whose results pages have all arrived before the caller
    /// cancels: without the token check ahead of the decoder, the client
    /// hands the caller every page already received, so a cancelled export
    /// keeps writing rows.
    #[test]
    fn a_cancelled_query_stops_at_pages_already_received() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .build()
            .expect("a tokio runtime");
        let name = pipe_name("buffered");
        let mut server = {
            let _reactor = runtime.enter();
            ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .expect("the test pipe is created")
        };
        runtime.spawn(async move {
            use tokio::io::AsyncWriteExt;
            server.connect().await.expect("the client connects");
            let mut replies = Vec::new();
            let hello = Response::Hello {
                proto: PROTO_VERSION,
                server: "test".into(),
                ready: true,
            };
            gozler_core::proto::encode_response_frame(&hello, &mut replies);
            for _ in 0..50 {
                replies.extend(ResultsPage::new(0, false, 0).finish(PageEnd {
                    total: 50,
                    more: true,
                    volumes_incomplete: false,
                }));
            }
            server
                .write_all(&replies)
                .await
                .expect("the replies are sent");
            std::future::pending::<()>().await;
        });

        let cancel = CancelToken::default();
        let mut client = Client::connect(&name, false, cancel.clone()).expect("connected");
        let request = SearchRequest {
            query: gozler_core::query::parse_query("x").expect("a valid query"),
            scope: None,
            returns: gozler_core::proto::Returns::Totals,
        };
        let mut pages = 0;
        let result = client.query(request, |_| {
            pages += 1;
            cancel.cancel();
            Ok::<(), ClientError>(())
        });
        assert!(matches!(result, Err(ClientError::Cancelled)), "{result:?}");
        assert_eq!(pages, 1);
    }
}
