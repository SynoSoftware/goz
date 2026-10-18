//! Every way a gozler run fails, each with its one exit code and its message.

use gozler_core::escompat::{EsFatal, ExitCode};
use gozler_core::proto::{ProtoError, SERVICE_NAME};
use gozler_core::query::QueryError;
use std::io;
use std::path::PathBuf;

/// A failure that ends a gozler run. [`Failure::exit_code`] is the one place
/// that maps a failure to its code, and [`Failure::report`] the one place that
/// writes it to stderr.
#[derive(Debug)]
// Outside Windows the client never connects, so the daemon's failures are
// never built there.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Failure {
    /// The command line is refused.
    Argv(EsFatal),
    /// An argument is not valid Unicode, so it could never match a name as
    /// typed; it carries the argument with the invalid part replaced.
    ArgumentNotUnicode(String),
    /// The query cannot run as written.
    Query(QueryError),
    /// The folder a search is scoped to does not exist, so an empty answer
    /// would read as "no such file" instead of "wrong folder".
    MissingFolder(String),
    /// The `-export-*` file cannot be created.
    ExportCreate { path: String, error: io::Error },
    /// Writing the results, the status or the help failed; `to` is the export
    /// file or `stdout`.
    Output { to: String, error: io::Error },
    /// No pipe server exists, and the gozler service is not running.
    DaemonNotRunning(NotRunning),
    /// No pipe server exists, but the gozler service is running, starting,
    /// stopping or paused, so the pipe may open soon.
    DaemonNotReady,
    /// gozler was built for a platform that has no gozlerd.
    #[cfg_attr(windows, allow(dead_code))]
    NotWindows,
    /// A pipe server exists but is not owned by SYSTEM or Administrators, so
    /// it may be another user's program posing as the daemon.
    Untrusted,
    /// The pipe could not be opened, or the daemon's reply was malformed,
    /// late or not the reply asked for.
    Communication(String),
    /// The daemon refused the request.
    Daemon { code: ProtoError, message: String },
    /// A failure no other variant describes.
    Internal(String),
}

impl Failure {
    pub(crate) fn exit_code(&self) -> ExitCode {
        match self {
            Failure::Argv(fatal) => fatal.exit_code,
            Failure::ArgumentNotUnicode(_) | Failure::Query(_) | Failure::MissingFolder(_) => {
                ExitCode::InvalidInput
            }
            Failure::ExportCreate { .. } | Failure::Output { .. } => ExitCode::OutputFailed,
            Failure::DaemonNotRunning(_) | Failure::NotWindows => ExitCode::DaemonNotRunning,
            Failure::DaemonNotReady => ExitCode::Communication,
            Failure::Untrusted => ExitCode::Untrusted,
            Failure::Communication(_) => ExitCode::Communication,
            Failure::Daemon { code, .. } => match code {
                ProtoError::BadQuery => ExitCode::InvalidInput,
                ProtoError::Unsupported => ExitCode::Communication,
                ProtoError::Overflow => ExitCode::Internal,
            },
            Failure::Internal(_) => ExitCode::Internal,
        }
    }

    /// Writes the failure to stderr, with what to do next where that helps.
    pub(crate) fn report(&self) {
        match self {
            Failure::Argv(fatal) => {
                eprintln!("gozler: {fatal}");
                if fatal.exit_code == ExitCode::UnknownSwitch {
                    eprintln!("gozler: run gozler --help for every option");
                }
            }
            Failure::ArgumentNotUnicode(arg) => {
                eprintln!("gozler: an argument is not valid Unicode: {arg}");
            }
            Failure::Query(error) => {
                eprintln!("gozler: {error}");
                if matches!(error, QueryError::UnknownFunction(_)) {
                    eprintln!("gozler: run gozler --help for every search function");
                }
            }
            Failure::MissingFolder(folder) => eprintln!("gozler: {folder} does not exist"),
            Failure::ExportCreate { path, error } => {
                eprintln!("gozler: cannot create {path}: {error}")
            }
            Failure::Output { to, error } => eprintln!("gozler: could not write to {to}: {error}"),
            Failure::DaemonNotRunning(why) => why.report(),
            Failure::DaemonNotReady => {
                eprintln!("gozler: the {SERVICE_NAME} service is not ready; try again shortly")
            }
            Failure::NotWindows => {
                eprintln!(
                    "gozler: gozler runs only on Windows, because it searches NTFS and ReFS volumes"
                )
            }
            Failure::Untrusted => eprintln!(
                "gozler: the pipe server is not the trusted gozlerd daemon (owner is not \
                 SYSTEM/Administrators), so gozler did not send the query. If this is \
                 expected, pass --insecure-no-server-check."
            ),
            Failure::Communication(message)
            | Failure::Daemon { message, .. }
            | Failure::Internal(message) => eprintln!("gozler: {message}"),
        }
    }
}

/// Why the gozler service is not running, as Windows reports it.
#[derive(Debug)]
// Outside Windows the client never asks the Service Control Manager.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum NotRunning {
    Stopped,
    Disabled,
    /// No gozler service exists. `folder` holds the running gozler.exe, and
    /// `gozlerd_beside` says whether gozlerd.exe is there too.
    NotInstalled {
        folder: PathBuf,
        gozlerd_beside: bool,
    },
    /// The service state or the folder of gozler.exe could not be read.
    Unknown(String),
}

/// Where the gozler setup is published.
const RELEASES: &str = "https://github.com/SynoSoftware/goz/releases";

/// How to start a service that is not running: what the lines below do, then
/// each command or link alone on its line, so that it can be copied.
pub(crate) struct Repair {
    pub(crate) how: &'static str,
    pub(crate) lines: Vec<String>,
}

impl NotRunning {
    /// The state in a word or two, as the card for bare `gozler` shows it.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn state(&self) -> &'static str {
        match self {
            NotRunning::Stopped => "Stopped",
            NotRunning::Disabled => "Disabled",
            NotRunning::NotInstalled { .. } => "Not installed",
            NotRunning::Unknown(_) => "Not running",
        }
    }

    /// What is wrong, as the first line of the message says it.
    pub(crate) fn problem(&self) -> String {
        match self {
            NotRunning::Stopped => format!("the {SERVICE_NAME} service is stopped"),
            NotRunning::Disabled => format!("the {SERVICE_NAME} service is disabled"),
            NotRunning::NotInstalled { .. } => {
                format!("the {SERVICE_NAME} service is not installed")
            }
            NotRunning::Unknown(error) => {
                format!("gozlerd is not running, and gozler could not find out why: {error}")
            }
        }
    }

    /// What starts the service, or `None` when gozler does not know why it
    /// is not running.
    pub(crate) fn repair(&self) -> Option<Repair> {
        let repair = match self {
            NotRunning::Stopped => Repair {
                how: "To start it, open a terminal as administrator and run:",
                lines: vec![format!("net start {SERVICE_NAME}")],
            },
            // `sc.exe`, because PowerShell reads `sc` as Set-Content, and two
            // lines, because Windows PowerShell 5.1 has no `&&`.
            NotRunning::Disabled => Repair {
                how: "To turn it on, open a terminal as administrator and run:",
                lines: vec![
                    format!("sc.exe config {SERVICE_NAME} start= auto"),
                    format!("net start {SERVICE_NAME}"),
                ],
            },
            // An administrator terminal opens in System32, and the folder is
            // not always on PATH. pushd also changes the drive in cmd.
            NotRunning::NotInstalled {
                folder,
                gozlerd_beside: true,
            } => Repair {
                how: "To install it, open a terminal as administrator and run:",
                lines: vec![
                    format!("pushd \"{}\"", folder.display()),
                    r".\gozlerd.exe install".to_string(),
                ],
            },
            NotRunning::NotInstalled {
                gozlerd_beside: false,
                ..
            } => Repair {
                how: "To install it, run the gozler setup from:",
                lines: vec![RELEASES.to_string()],
            },
            NotRunning::Unknown(_) => return None,
        };
        Some(repair)
    }

    /// Writes what is wrong to stderr, and below it what starts the service.
    fn report(&self) {
        eprintln!("gozler: {}", self.problem());
        if let Some(repair) = self.repair() {
            eprintln!("{}", repair.how);
            for line in &repair.lines {
                eprintln!("  {line}");
            }
        }
    }
}

/// Maps the result of writing output to the run's result. A reader that
/// closes early (`gozler * | head`) is a clean stop, not a failure.
pub(crate) fn output(written: io::Result<()>, to: &str) -> Result<(), Failure> {
    match written {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(Failure::Output {
            to: to.to_string(),
            error,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gozler_core::escompat::parse_argv;

    fn argv_failure(args: &[&str]) -> Failure {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        match parse_argv(&args) {
            Err(fatal) => Failure::Argv(fatal),
            Ok(command) => panic!("{args:?} parsed as {command:?}"),
        }
    }

    fn io_error() -> io::Error {
        io::Error::from(io::ErrorKind::StorageFull)
    }

    /// Each failure exits with its code from the exit-code table the help
    /// prints, which scripts read.
    #[test]
    fn each_failure_exits_with_its_code() {
        let daemon = |code| Failure::Daemon {
            code,
            message: String::new(),
        };
        let cases = [
            (argv_failure(&["-bogus"]), 6),
            (argv_failure(&["-n"]), 4),
            (argv_failure(&["-n", "x"]), 4),
            (Failure::ArgumentNotUnicode("a\u{FFFD}".into()), 4),
            (Failure::Query(QueryError::UnknownFunction("dm".into())), 4),
            (
                Failure::ExportCreate {
                    path: "out.csv".into(),
                    error: io_error(),
                },
                5,
            ),
            (
                output(Err(io_error()), "stdout").expect_err("a failed write"),
                5,
            ),
            (Failure::DaemonNotRunning(NotRunning::Stopped), 8),
            (Failure::DaemonNotReady, 7),
            (Failure::NotWindows, 8),
            (Failure::Untrusted, 3),
            (
                Failure::Communication("daemon did not reply in time".into()),
                7,
            ),
            (daemon(ProtoError::BadQuery), 4),
            (daemon(ProtoError::Unsupported), 7),
            (daemon(ProtoError::Overflow), 1),
            (Failure::Internal(String::new()), 1),
        ];
        for (failure, code) in cases {
            assert_eq!(failure.exit_code().code(), code, "{failure:?}");
        }
    }

    /// A reader that stops reading ends the output without a failure, so
    /// `gozler * | head` exits as if every line was read.
    #[test]
    fn a_closed_reader_is_not_a_failure() {
        let broken = io::Error::from(io::ErrorKind::BrokenPipe);
        assert!(output(Err(broken), "stdout").is_ok());
    }
}
