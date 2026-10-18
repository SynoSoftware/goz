//! Blocking named-pipe client. Opens `\\.\pipe\gozler-v1` and exchanges framed
//! messages with the daemon using the shared codec from `gozler-core::proto`:
//! JSON requests, and tagged responses whose results pages carry a binary body.

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::AsHandle;
use std::time::{Duration, Instant};

use gozler_core::proto::{
    DaemonStatus, FrameDecoder, MAX_SERVER_FRAME, PIPE_NAME, PROTO_VERSION, ProtoError,
    QueryResults, QueryTotals, Request, Response, SearchRequest, decode_response_frame,
    encode_request,
};

use crate::failure::Failure;

/// Opening the pipe with an anonymous impersonation level (`SECURITY_ANONYMOUS`,
/// value 0, so only the SQOS-present bit is set) prevents a rogue server that
/// squatted the name from impersonating this client's token. It does NOT
/// authenticate the server: see [`Client::connect`], which verifies the pipe's
/// owner is the elevated daemon before any request is sent.
const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;

/// How long the daemon may stay silent while a reply is due before the client
/// gives up, so a wedged-but-alive daemon cannot hang it forever. It measures
/// silence, not the whole reply, because a large export streams for longer.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// Poll interval while waiting for a reply (see [`Client::next_response`]).
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// One connection to the daemon, which serves its requests one at a time.
pub(crate) struct Client {
    pipe: File,
    /// Bytes read past the last whole response.
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Client {
    /// Connects to the running daemon, unless `insecure` verifies the pipe
    /// server's identity, and agrees on [`PROTO_VERSION`] with the daemon
    /// before returning.
    ///
    /// Returns [`Failure::DaemonNotRunning`] if no pipe exists (daemon absent),
    /// [`Failure::Untrusted`] if a server is present but not owned by the
    /// elevated daemon, or [`Failure::Communication`] for other open errors
    /// and for a daemon older than this client.
    pub(crate) fn connect(insecure: bool) -> Result<Self, Failure> {
        use std::os::windows::fs::OpenOptionsExt;

        // Win32 error codes (gozler-cli intentionally has no windows-sys dep).
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
                .custom_flags(SECURITY_SQOS_PRESENT)
                .open(PIPE_NAME)
            {
                Ok(pipe) => break pipe,
                Err(e) => match e.raw_os_error() {
                    // Pipe does not exist: the daemon really is not running.
                    Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => {
                        return Err(Failure::DaemonNotRunning);
                    }
                    // All instances momentarily busy: brief backoff and retry
                    // (~500 ms budget) before giving up.
                    Some(ERROR_PIPE_BUSY) if attempts < 20 => {
                        attempts += 1;
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    // Busy past the retry budget, ERROR_ACCESS_DENIED, or anything
                    // unexpected: the pipe is present but we could not open it,
                    // so surface the real OS error rather than "not running".
                    _ => {
                        return Err(Failure::Communication(format!(
                            "cannot open {PIPE_NAME}: {e}"
                        )));
                    }
                },
            }
        };

        // Authenticate the server BEFORE sending any request: a non-elevated
        // local user can pre-create `\\.\pipe\gozler-v1` before the daemon
        // starts and serve spoofed results. Require the pipe be owned by
        // SYSTEM / Administrators (created by the elevated daemon). Fail closed
        // on error.
        if !insecure {
            match gozler_winfs::pipe_server_is_trusted(pipe.as_handle()) {
                Ok(true) => {}
                Ok(false) | Err(_) => return Err(Failure::Untrusted),
            }
        }

        let mut client = Self {
            pipe,
            decoder: FrameDecoder::new(MAX_SERVER_FRAME),
            // Heap, and larger than one 64 KiB pipe read: a broad query
            // streams hundreds of MB, so fewer, bigger reads cut syscall
            // overhead, and the main thread's 1 MiB stack stays safe.
            buf: vec![0u8; 256 * 1024],
        };
        client.hello()?;
        Ok(client)
    }

    /// Asks the daemon for this client's protocol version alone. A daemon
    /// older than the client refuses it; without the question, an old daemon
    /// would refuse each request as malformed, which reads as invalid input.
    fn hello(&mut self) -> Result<(), Failure> {
        self.send(&Request::Hello {
            proto_min: PROTO_VERSION,
            proto_max: PROTO_VERSION,
            client: concat!("gozler ", env!("CARGO_PKG_VERSION")).to_string(),
        })?;
        match self.next_response()? {
            Response::Hello { .. } => Ok(()),
            Response::Error {
                code: ProtoError::Unsupported,
                ..
            } => Err(Failure::Communication(
                "the running gozlerd is older than this client; install the matching gozlerd"
                    .into(),
            )),
            _ => Err(Failure::Communication("unexpected reply to hello".into())),
        }
    }

    /// Asks for the daemon's state. Fails only with
    /// [`Failure::Communication`].
    pub(crate) fn status(&mut self) -> Result<DaemonStatus, Failure> {
        self.send(&Request::Status)?;
        match self.next_response()? {
            Response::Status(status) => Ok(status),
            _ => Err(Failure::Communication("unexpected reply to status".into())),
        }
    }

    /// Runs a query and hands each results page to `page` as it arrives, so
    /// the caller prints the first rows while the daemon still sends the
    /// rest. Stops at the first error `page` returns.
    ///
    /// Fails with [`Failure::Daemon`] when the daemon refuses the query, and
    /// with [`Failure::Communication`] when its reply breaks off.
    pub(crate) fn query(
        &mut self,
        request: SearchRequest,
        mut page: impl FnMut(QueryResults) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        self.send(&Request::Search(request))?;
        loop {
            match self.next_response()? {
                Response::Results(results) => {
                    let more = results.more;
                    page(results)?;
                    if !more {
                        return Ok(());
                    }
                }
                Response::Error { code, message } => {
                    return Err(Failure::Daemon { code, message });
                }
                _ => {
                    return Err(Failure::Communication(
                        "unexpected reply from daemon".into(),
                    ));
                }
            }
        }
    }

    /// Runs a search that asks for [`Returns::Totals`], with the failures of
    /// [`Self::query`].
    ///
    /// [`Returns::Totals`]: gozler_core::proto::Returns::Totals
    pub(crate) fn totals(&mut self, request: SearchRequest) -> Result<QueryTotals, Failure> {
        self.send(&Request::Search(request))?;
        match self.next_response()? {
            Response::Totals(totals) => Ok(totals),
            Response::Error { code, message } => Err(Failure::Daemon { code, message }),
            _ => Err(Failure::Communication(
                "unexpected reply from daemon".into(),
            )),
        }
    }

    fn send(&mut self, request: &Request) -> Result<(), Failure> {
        let mut out = Vec::new();
        encode_request(request, &mut out);
        self.pipe
            .write_all(&out)
            .map_err(|e| Failure::Communication(e.to_string()))
    }

    /// Reads the next framed response. The wait is enforced by polling
    /// `PeekNamedPipe` rather than issuing a blocking read, so a
    /// wedged-but-alive daemon cannot hang the client past [`READ_DEADLINE`]
    /// of silence.
    fn next_response(&mut self) -> Result<Response, Failure> {
        let mut deadline = Instant::now() + READ_DEADLINE;
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => {
                    return decode_response_frame(&frame)
                        .map_err(|e| Failure::Communication(e.to_string()));
                }
                Ok(None) => {}
                Err(e) => return Err(Failure::Communication(e.to_string())),
            }
            let avail = gozler_winfs::pipe_bytes_available(self.pipe.as_handle())
                .map_err(|_| Failure::Communication("daemon closed the connection".into()))?;
            if avail == 0 {
                if Instant::now() > deadline {
                    return Err(Failure::Communication(
                        "daemon did not reply in time".into(),
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            let n = self
                .pipe
                .read(&mut self.buf)
                .map_err(|e| Failure::Communication(e.to_string()))?;
            if n == 0 {
                return Err(Failure::Communication(
                    "daemon closed the connection".into(),
                ));
            }
            self.decoder.feed(&self.buf[..n]);
            deadline = Instant::now() + READ_DEADLINE;
        }
    }
}
