//! gozler-client: the blocking client of gozlerd's named pipe, which `gozler`
//! and the window share.
//!
//! `Client`, on Windows, opens the pipe, checks that its server is the
//! elevated daemon, agrees on the protocol version and exchanges framed
//! messages with the codec of `gozler_core::proto`. Every failure that a
//! caller handles differently is its own [`ClientError`]; when the pipe does
//! not exist, the error carries the state of the gozler service as Windows
//! reports it. Each request waits for its reply by polling the pipe, so a
//! [`CancelToken`] that another thread sets ends the wait at the next poll.
//! [`folder_is_missing`] is the rule that refuses a scope folder that does not
//! exist, which both callers check before they send a scoped search.
//!
//! The error types and the folder check build on every OS, so each caller's
//! handling of every failure is tested on Ubuntu too; the pipe is Windows
//! only.

#[cfg(windows)]
mod client;

#[cfg(windows)]
pub use client::Client;

use gozler_core::proto::ProtoError;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Why a request to the daemon failed.
#[derive(Debug)]
pub enum ClientError {
    /// No pipe server exists, and the gozler service is not running.
    NotRunning(NotRunning),
    /// No pipe server exists, but the gozler service is running, starting,
    /// stopping or paused, so the pipe may open soon.
    NotReady,
    /// The pipe could not be opened, read or written, or the daemon's reply
    /// was malformed, late or not the reply asked for; carries what failed,
    /// in English.
    Communication(String),
    /// A pipe server exists but is not owned by SYSTEM or Administrators, so
    /// it may be another user's program posing as the daemon.
    Untrusted,
    /// The daemon refused this client's protocol version, because it is
    /// older than the client.
    TooOld,
    /// The daemon refused the request; `code` says why, and `message` is
    /// the daemon's text.
    Refused { code: ProtoError, message: String },
    /// The daemon closed the connection before its reply was complete.
    Lost,
    /// The request's [`CancelToken`] was set.
    Cancelled,
}

/// Why the gozler service is not running, as Windows reports it.
#[derive(Debug)]
pub enum NotRunning {
    Stopped,
    Disabled,
    /// No gozler service exists. `folder` holds the running program, and
    /// `gozlerd_beside` says whether gozlerd.exe is there too.
    NotInstalled {
        folder: PathBuf,
        gozlerd_beside: bool,
    },
    /// The service state or the folder of the running program could not be
    /// read; carries the error, in English.
    Unknown(String),
}

/// Stops a request from another thread: a clone of the token that a
/// `Client` holds. A request that waits for the daemon then returns
/// [`ClientError::Cancelled`] within one poll of the pipe. A token that is
/// never set never cancels.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// Cancels every request that holds a clone of this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Whether the scope folder `folder` does not exist, so a search scoped to
/// it is invalid input: an empty answer would read as "no such file"
/// instead of "wrong folder". Only a folder that the operating system
/// reports as missing counts. A folder the user cannot read still searches,
/// because gozlerd answers from its index and not from the folder.
pub fn folder_is_missing(folder: &str) -> bool {
    std::fs::metadata(folder).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}
