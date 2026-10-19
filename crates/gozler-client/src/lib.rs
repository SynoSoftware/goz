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
//! [`ClientError::repair`] says what makes a failed request work again, which
//! `gozler` prints and the window runs. [`folder_is_missing`] is the rule that
//! refuses a scope folder that does not exist, which both callers check before
//! they send a scoped search.
//!
//! The error types, the repairs and the folder check build on every OS, so
//! each caller's handling of every failure is tested on Ubuntu too; the pipe
//! is Windows only.

#[cfg(windows)]
mod client;

#[cfg(windows)]
pub use client::Client;

use gozler_core::proto::{ProtoError, SERVICE_NAME};
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
    /// older than the client. `update_from` is this program's folder when
    /// gozlerd.exe is in it, so a repair can replace the service from there.
    TooOld { update_from: Option<PathBuf> },
    /// The daemon refused the request; `code` says why, and `message` is
    /// the daemon's text.
    Refused { code: ProtoError, message: String },
    /// The daemon closed the connection before its reply was complete.
    Lost,
    /// The request's [`CancelToken`] was set.
    Cancelled,
}

/// Why the gozler service is not running, as Windows reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Where the gozler setup is published.
pub const RELEASES: &str = "https://github.com/SynoSoftware/goz/releases";

/// What makes a failed request work again. `gozler` prints it and the window
/// runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Repair {
    StartService,
    /// Sets the disabled service to start automatically, and starts it.
    TurnOnService,
    /// Installs the service from the gozlerd.exe in `folder`.
    InstallService {
        folder: PathBuf,
    },
    /// Stops the older service, and installs and starts the gozlerd.exe in
    /// `folder` in its place.
    UpdateService {
        folder: PathBuf,
    },
    /// Run the gozler setup from [`RELEASES`].
    Download,
}

impl ClientError {
    /// The repair for this failure, or `None` when no repair fixes it.
    pub fn repair(&self) -> Option<Repair> {
        match self {
            ClientError::NotRunning(why) => why.repair(),
            ClientError::TooOld {
                update_from: Some(folder),
            } => Some(Repair::UpdateService {
                folder: folder.clone(),
            }),
            // The setup installs gozlerd and gozler together.
            ClientError::TooOld { update_from: None } => Some(Repair::Download),
            ClientError::NotReady
            | ClientError::Communication(_)
            | ClientError::Untrusted
            | ClientError::Refused { .. }
            | ClientError::Lost
            | ClientError::Cancelled => None,
        }
    }
}

impl NotRunning {
    /// The repair for this state, or `None` when Windows did not say why
    /// the service is not running.
    pub fn repair(&self) -> Option<Repair> {
        Some(match self {
            NotRunning::Stopped => Repair::StartService,
            NotRunning::Disabled => Repair::TurnOnService,
            NotRunning::NotInstalled {
                folder,
                gozlerd_beside: true,
            } => Repair::InstallService {
                folder: folder.clone(),
            },
            NotRunning::NotInstalled {
                gozlerd_beside: false,
                ..
            } => Repair::Download,
            NotRunning::Unknown(_) => return None,
        })
    }
}

impl Repair {
    /// The commands that make the repair, in order, each run as
    /// administrator; `None` for [`Repair::Download`]. Each is alone on its
    /// line so that it can be copied into cmd, Windows PowerShell or
    /// PowerShell 7, and the lines joined with `&&` also run in `cmd /c`.
    pub fn commands(&self) -> Option<Vec<String>> {
        Some(match self {
            Repair::StartService => vec![format!("net start {SERVICE_NAME}")],
            // `sc.exe`, because PowerShell reads `sc` as Set-Content, and two
            // lines, because Windows PowerShell 5.1 has no `&&`.
            Repair::TurnOnService => vec![
                format!("sc.exe config {SERVICE_NAME} start= auto"),
                format!("net start {SERVICE_NAME}"),
            ],
            // An administrator terminal opens in System32, and the folder is
            // not always on PATH. pushd also changes the drive in cmd.
            Repair::InstallService { folder } => vec![
                format!("pushd \"{}\"", folder.display()),
                r".\gozlerd.exe install".to_string(),
            ],
            // install points the service at this gozlerd.exe and starts it,
            // but does not restart a service that runs.
            Repair::UpdateService { folder } => vec![
                format!("net stop {SERVICE_NAME}"),
                format!("pushd \"{}\"", folder.display()),
                r".\gozlerd.exe install".to_string(),
            ],
            Repair::Download => return None,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A person pastes these lines into cmd, Windows PowerShell or
    /// PowerShell 7, and the window runs them in `cmd /c`, so a line that
    /// works in only one of them, such as `sc` for Set-Content in
    /// PowerShell, fails a repair that no other test runs.
    #[test]
    fn each_state_has_its_repair_and_commands() {
        let folder = PathBuf::from(r"C:\Program Files\Gozler");
        let commands = |why: NotRunning| why.repair().and_then(|repair| repair.commands());
        assert_eq!(
            commands(NotRunning::Stopped),
            Some(vec!["net start gozler".to_string()])
        );
        assert_eq!(
            commands(NotRunning::Disabled),
            Some(vec![
                "sc.exe config gozler start= auto".to_string(),
                "net start gozler".to_string()
            ])
        );
        assert_eq!(
            commands(NotRunning::NotInstalled {
                folder: folder.clone(),
                gozlerd_beside: true
            }),
            Some(vec![
                r#"pushd "C:\Program Files\Gozler""#.to_string(),
                r".\gozlerd.exe install".to_string()
            ])
        );
        let missing = NotRunning::NotInstalled {
            folder,
            gozlerd_beside: false,
        };
        assert_eq!(missing.repair(), Some(Repair::Download));
        assert_eq!(
            NotRunning::Unknown("access is denied".into()).repair(),
            None
        );
        // A daemon older than the client is replaced from the gozlerd.exe
        // beside the client; without one, both programs offer the setup.
        let older = |update_from| ClientError::TooOld { update_from }.repair();
        assert_eq!(older(None), Some(Repair::Download));
        assert_eq!(
            older(Some(PathBuf::from(r"C:\Program Files\Gozler"))).and_then(|r| r.commands()),
            Some(vec![
                "net stop gozler".to_string(),
                r#"pushd "C:\Program Files\Gozler""#.to_string(),
                r".\gozlerd.exe install".to_string()
            ])
        );
    }
}
