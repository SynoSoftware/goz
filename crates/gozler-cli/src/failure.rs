//! Every way a gozler run fails, each with its one exit code and its message.

use gozler_client::{ClientError, NotRunning, RELEASES, Repair};
use gozler_core::escompat::{EsFatal, ExitCode};
use gozler_core::proto::{ProtoError, SERVICE_NAME};
use gozler_core::query::QueryError;
use std::io;

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
    /// gozler was built for a platform that has no gozlerd.
    #[cfg_attr(windows, allow(dead_code))]
    NotWindows,
    /// Talking to the daemon failed.
    Client(ClientError),
    /// A failure no other variant describes.
    Internal(String),
}

impl From<ClientError> for Failure {
    fn from(error: ClientError) -> Self {
        Failure::Client(error)
    }
}

impl Failure {
    pub(crate) fn exit_code(&self) -> ExitCode {
        match self {
            Failure::Argv(fatal) => fatal.exit_code,
            Failure::ArgumentNotUnicode(_) | Failure::Query(_) | Failure::MissingFolder(_) => {
                ExitCode::InvalidInput
            }
            Failure::ExportCreate { .. } | Failure::Output { .. } => ExitCode::OutputFailed,
            Failure::NotWindows => ExitCode::DaemonNotRunning,
            Failure::Client(error) => match error {
                ClientError::NotRunning(_) => ExitCode::DaemonNotRunning,
                ClientError::NotReady
                | ClientError::Communication(_)
                | ClientError::TooOld
                | ClientError::Lost => ExitCode::Communication,
                ClientError::Untrusted => ExitCode::Untrusted,
                ClientError::Refused { code, .. } => match code {
                    ProtoError::BadQuery => ExitCode::InvalidInput,
                    ProtoError::Unsupported => ExitCode::Communication,
                    ProtoError::Overflow => ExitCode::Internal,
                },
                // gozler never cancels a request.
                ClientError::Cancelled => ExitCode::Internal,
            },
            Failure::Internal(_) => ExitCode::Internal,
        }
    }

    /// Writes the failure to stderr, with what to do next where that helps.
    pub(crate) fn report(&self) {
        eprint!("{}", self.message());
    }

    /// The lines [`Self::report`] writes.
    fn message(&self) -> String {
        let line = |text: &str| format!("gozler: {text}\n");
        match self {
            Failure::Argv(fatal) => {
                let mut text = line(&fatal.to_string());
                if fatal.exit_code == ExitCode::UnknownSwitch {
                    text.push_str(&line("run gozler --help for every option"));
                }
                text
            }
            Failure::ArgumentNotUnicode(arg) => {
                line(&format!("an argument is not valid Unicode: {arg}"))
            }
            Failure::Query(error) => {
                let mut text = line(&error.to_string());
                if matches!(error, QueryError::UnknownFunction(_)) {
                    text.push_str(&line("run gozler --help for every search function"));
                }
                text
            }
            Failure::MissingFolder(folder) => line(&format!("{folder} does not exist")),
            Failure::ExportCreate { path, error } => {
                line(&format!("cannot create {path}: {error}"))
            }
            Failure::Output { to, error } => line(&format!("could not write to {to}: {error}")),
            Failure::NotWindows => {
                line("gozler runs only on Windows, because it searches NTFS and ReFS volumes")
            }
            Failure::Client(error) => match error {
                ClientError::NotRunning(why) => {
                    let mut text = line(&problem(why));
                    if let Some(repair) = why.repair() {
                        let (how, lines) = repair_lines(&repair);
                        text.push_str(how);
                        text.push('\n');
                        for command in &lines {
                            text.push_str(&format!("  {command}\n"));
                        }
                    }
                    text
                }
                ClientError::NotReady => line(&format!(
                    "the {SERVICE_NAME} service is not ready; try again shortly"
                )),
                ClientError::Untrusted => line(
                    "the pipe server is not the trusted gozlerd daemon (owner is not \
                     SYSTEM/Administrators), so gozler did not send the query. If this is \
                     expected, pass --insecure-no-server-check.",
                ),
                ClientError::TooOld => line(
                    "the running gozlerd is older than this client; install the matching gozlerd",
                ),
                ClientError::Lost => line("daemon closed the connection"),
                ClientError::Cancelled => line("the request was cancelled"),
                ClientError::Communication(message) | ClientError::Refused { message, .. } => {
                    line(message)
                }
            },
            Failure::Internal(message) => line(message),
        }
    }
}

/// The state in a word or two, as the card for bare `gozler` shows it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn state(why: &NotRunning) -> &'static str {
    match why {
        NotRunning::Stopped => "Stopped",
        NotRunning::Disabled => "Disabled",
        NotRunning::NotInstalled { .. } => "Not installed",
        NotRunning::Unknown(_) => "Not running",
    }
}

/// What is wrong, as the first line of the message says it.
pub(crate) fn problem(why: &NotRunning) -> String {
    match why {
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

/// How to make `repair`: what the lines below do, then each command or link
/// alone on its line, so that it can be copied.
pub(crate) fn repair_lines(repair: &Repair) -> (&'static str, Vec<String>) {
    let how = match repair {
        Repair::StartService => "To start it, open a terminal as administrator and run:",
        Repair::TurnOnService => "To turn it on, open a terminal as administrator and run:",
        Repair::InstallService { .. } => "To install it, open a terminal as administrator and run:",
        Repair::Download => "To install it, run the gozler setup from:",
    };
    let lines = repair
        .commands()
        .unwrap_or_else(|| vec![RELEASES.to_string()]);
    (how, lines)
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
    use std::path::PathBuf;

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
            (Failure::NotWindows, 8),
            (Failure::Internal(String::new()), 1),
        ];
        for (failure, code) in cases {
            assert_eq!(failure.exit_code().code(), code, "{failure:?}");
        }
    }

    /// Each way a request to the daemon fails, and a scope folder that does
    /// not exist, exits with its code and starts its message with the line
    /// that scripts and readers know. A new client error or a changed mapping
    /// shows here.
    #[test]
    fn each_client_error_exits_with_its_code_and_message() {
        let not_installed = |gozlerd_beside| NotRunning::NotInstalled {
            folder: PathBuf::from(r"C:\Program Files\gozler"),
            gozlerd_beside,
        };
        let refused = |code| ClientError::Refused {
            code,
            message: "the daemon's text".into(),
        };
        let cases = [
            (
                Failure::from(ClientError::NotRunning(NotRunning::Stopped)),
                8,
                "gozler: the gozler service is stopped",
            ),
            (
                ClientError::NotRunning(NotRunning::Disabled).into(),
                8,
                "gozler: the gozler service is disabled",
            ),
            (
                ClientError::NotRunning(not_installed(true)).into(),
                8,
                "gozler: the gozler service is not installed",
            ),
            (
                ClientError::NotRunning(not_installed(false)).into(),
                8,
                "gozler: the gozler service is not installed",
            ),
            (
                ClientError::NotRunning(NotRunning::Unknown("access is denied".into())).into(),
                8,
                "gozler: gozlerd is not running, and gozler could not find out why: \
                 access is denied",
            ),
            (
                ClientError::NotReady.into(),
                7,
                "gozler: the gozler service is not ready; try again shortly",
            ),
            (
                ClientError::Communication("daemon did not reply in time".into()).into(),
                7,
                "gozler: daemon did not reply in time",
            ),
            (
                ClientError::Untrusted.into(),
                3,
                "gozler: the pipe server is not the trusted gozlerd daemon (owner is not \
                 SYSTEM/Administrators), so gozler did not send the query. If this is \
                 expected, pass --insecure-no-server-check.",
            ),
            (
                ClientError::TooOld.into(),
                7,
                "gozler: the running gozlerd is older than this client; install the \
                 matching gozlerd",
            ),
            (
                refused(ProtoError::BadQuery).into(),
                4,
                "gozler: the daemon's text",
            ),
            (
                refused(ProtoError::Unsupported).into(),
                7,
                "gozler: the daemon's text",
            ),
            (
                refused(ProtoError::Overflow).into(),
                1,
                "gozler: the daemon's text",
            ),
            (
                ClientError::Lost.into(),
                7,
                "gozler: daemon closed the connection",
            ),
            (
                Failure::MissingFolder(r"X:\no-such-folder".into()),
                4,
                r"gozler: X:\no-such-folder does not exist",
            ),
        ];
        for (failure, code, first_line) in cases {
            assert_eq!(failure.exit_code().code(), code, "{failure:?}");
            assert_eq!(
                failure.message().lines().next(),
                Some(first_line),
                "{failure:?}"
            );
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
