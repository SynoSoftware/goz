//! A fix of the service (DESIGN.md: Service fixes): the step it is in, and
//! how it ended. A fix either runs commands as administrator and waits for
//! the service, or downloads the latest release's setup and runs it; each
//! kind has its own steps, events and results, so a step of one never pairs
//! with the other. `app::work::fixes` runs the fix through the work owner and
//! reports each [`FixEvent`]; the model holds the one [`Fixer`], so the
//! list's fix button, the Index page's and the status bar that owns its
//! feedback follow that fix.

use super::results::Fix;
use super::update::Setup;
use gozler_client::Repair;
use gozler_core::proto::SERVICE_NAME;
use std::path::{Path, PathBuf};

/// A fix that runs commands as administrator, then waits for the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandFix {
    /// Starts the service, after turning it on when it is disabled.
    Start,
    Install,
    /// Points the service at the gozlerd.exe beside the window.
    UseLocal,
}

/// A fix that downloads the latest release's setup and runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetupFix {
    /// Over a service older than the window, or when a newer release is
    /// available.
    Update,
    /// When the service is not installed and no `gozlerd.exe` is beside
    /// the window.
    Download,
}

/// What a fix's thread does: run a repair's or Use local's commands, ask
/// GitHub for the latest release's setup, or download the setup the person
/// accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FixWork {
    Commands(Vec<String>),
    FindSetup,
    Download(Setup),
}

/// The commands that point the service at the gozlerd.exe at `program`
/// and restart it. `install` comes first and only changes the service's
/// configuration while it runs, so a failed `install` leaves the service
/// running its old program; the restart then starts `program`.
fn use_local_commands(program: &Path) -> Vec<String> {
    let folder = program
        .parent()
        .expect("the local gozlerd.exe is a file in the window's folder")
        .to_owned();
    let mut commands = Repair::InstallService { folder }
        .commands()
        .expect("an install has commands");
    commands.push(format!("net stop {SERVICE_NAME}"));
    commands.push(format!("net start {SERVICE_NAME}"));
    commands
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandStep {
    /// Windows' permission prompt shows.
    AskingWindows,
    RunningCommands,
    /// The commands ended well, and the status poll asks the service until
    /// it answers or `app::work::fixes`' wait ends.
    WaitingForService,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetupStep {
    /// GitHub is asked for its latest release.
    FindingRelease,
    /// The window asks the person whether to download `setup`.
    Confirming { setup: Setup },
    Downloading {
        version: String,
        bytes: u64,
        size: u64,
    },
    /// The downloaded file's SHA-256 is compared with GitHub's.
    CheckingFile { version: String },
    /// The window starts the checked setup at `path` and closes, because
    /// the setup does not run while a Gozler window is open. The setup asks
    /// Windows for permission itself, after the window has closed.
    StartingSetup { version: String, path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandResult {
    /// The service answers.
    Done,
    Cancelled,
    /// The person said no to Windows' permission prompt.
    Refused,
    /// Windows could not start or watch the commands, in its own words.
    Failed(String),
    /// The commands ended with this exit code, which is not 0, and the
    /// last thing they said, which is Windows' reason.
    CommandsFailed {
        code: u32,
        said: Option<String>,
    },
    /// The service did not answer within the wait.
    NoAnswer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetupResult {
    Cancelled,
    /// The person chose not to download the setup.
    Declined,
    /// The latest release has no setup whose SHA-256 GitHub gives, and a
    /// setup that cannot be checked is not run.
    NoSetup,
    /// GitHub or the connection failed.
    DownloadFailed,
    /// The setup could not be written to `folder`, for the reason `why`
    /// in Windows' words.
    NotSaved {
        folder: PathBuf,
        why: String,
    },
    /// The downloaded file's SHA-256 is not GitHub's, so it was deleted.
    Damaged,
    /// Windows could not start the setup, in its own words.
    Failed(String),
}

/// What a commands fix's thread reports, in this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandEvent {
    /// Windows gave permission, and the commands run.
    Allowed,
    /// The commands ended well, and the wait for the service starts.
    Succeeded,
    End(CommandResult),
}

/// What a setup fix's thread reports, in this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetupEvent {
    /// The latest release's setup, which the person is asked about.
    Found(Setup),
    /// This many bytes of the setup are written.
    Downloaded(u64),
    /// The whole setup is written, and its check starts.
    Checking,
    /// The checked setup at this path, which the window starts.
    Ready(PathBuf),
    End(SetupResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FixEvent {
    Commands(CommandEvent),
    Setup(SetupEvent),
}

/// A fix's step while it runs, or its result once it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FixState<Step, Result> {
    Running(Step),
    Ended(Result),
}

impl<Step, Result> FixState<Step, Result> {
    /// Moves a running fix to the state `after` gives for its step; an
    /// ended fix, or a step that `after` gives nothing for, stays. Returns
    /// whether it moved.
    fn advance(&mut self, after: impl FnOnce(&Step) -> Option<FixState<Step, Result>>) -> bool {
        let FixState::Running(step) = self else {
            return false;
        };
        match after(step) {
            Some(next) => {
                *self = next;
                true
            }
            None => false,
        }
    }
}

impl CommandStep {
    /// The state after `event`, or `None` for an event this step does not
    /// expect.
    fn after(&self, event: CommandEvent) -> Option<FixState<CommandStep, CommandResult>> {
        Some(match (self, event) {
            (_, CommandEvent::End(result)) => FixState::Ended(result),
            (CommandStep::AskingWindows, CommandEvent::Allowed) => {
                FixState::Running(CommandStep::RunningCommands)
            }
            (CommandStep::RunningCommands, CommandEvent::Succeeded) => {
                FixState::Running(CommandStep::WaitingForService)
            }
            _ => return None,
        })
    }

    /// Whether Cancel stops the fix in this step. The permission prompt is
    /// Windows', and a half-run `sc.exe` command is worse than a finished
    /// one.
    pub(crate) fn can_cancel(&self) -> bool {
        *self == CommandStep::WaitingForService
    }
}

impl SetupStep {
    /// The state after `event`, or `None` for an event this step does not
    /// expect.
    fn after(&self, event: SetupEvent) -> Option<FixState<SetupStep, SetupResult>> {
        Some(match (self, event) {
            (_, SetupEvent::End(result)) => FixState::Ended(result),
            (SetupStep::FindingRelease, SetupEvent::Found(setup)) => {
                FixState::Running(SetupStep::Confirming { setup })
            }
            (SetupStep::Downloading { version, size, .. }, SetupEvent::Downloaded(bytes)) => {
                FixState::Running(SetupStep::Downloading {
                    version: version.clone(),
                    bytes,
                    size: *size,
                })
            }
            (SetupStep::Downloading { version, .. }, SetupEvent::Checking) => {
                FixState::Running(SetupStep::CheckingFile {
                    version: version.clone(),
                })
            }
            (SetupStep::CheckingFile { version }, SetupEvent::Ready(path)) => {
                FixState::Running(SetupStep::StartingSetup {
                    version: version.clone(),
                    path,
                })
            }
            _ => return None,
        })
    }

    /// The question is the window's to close, and the setup is Windows'
    /// to start; cancellation during hashing prevents the setup starting.
    pub(crate) fn can_cancel(&self) -> bool {
        matches!(
            self,
            SetupStep::FindingRelease
                | SetupStep::Downloading { .. }
                | SetupStep::CheckingFile { .. }
        )
    }
}

/// The fix that runs, or the last fix's result, which stays until the next
/// fix starts, as an export's result does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fixer {
    Idle,
    Commands {
        fix: CommandFix,
        state: FixState<CommandStep, CommandResult>,
    },
    Setup {
        fix: SetupFix,
        state: FixState<SetupStep, SetupResult>,
    },
}

impl Fixer {
    pub(crate) fn running(&self) -> bool {
        matches!(
            self,
            Fixer::Commands {
                state: FixState::Running(_),
                ..
            } | Fixer::Setup {
                state: FixState::Running(_),
                ..
            }
        )
    }

    /// Whether [`Self::cancel`] would stop the fix in its step.
    pub(crate) fn can_cancel(&self) -> bool {
        match self {
            Fixer::Commands {
                state: FixState::Running(step),
                ..
            } => step.can_cancel(),
            Fixer::Setup {
                state: FixState::Running(step),
                ..
            } => step.can_cancel(),
            Fixer::Idle
            | Fixer::Commands {
                state: FixState::Ended(_),
                ..
            }
            | Fixer::Setup {
                state: FixState::Ended(_),
                ..
            } => false,
        }
    }

    pub(crate) fn waits_for_service(&self) -> bool {
        matches!(
            self,
            Fixer::Commands {
                state: FixState::Running(CommandStep::WaitingForService),
                ..
            }
        )
    }

    /// Starts `fix`, unless one runs, because two fixes would run at once,
    /// or the window does it itself, as Open Services. Returns the work its
    /// thread starts with.
    pub(crate) fn start(&mut self, fix: &Fix) -> Option<FixWork> {
        if self.running() {
            return None;
        }
        let commands = |fix| Fixer::Commands {
            fix,
            state: FixState::Running(CommandStep::AskingWindows),
        };
        let setup = |fix| Fixer::Setup {
            fix,
            state: FixState::Running(SetupStep::FindingRelease),
        };
        let (fixer, work) = match fix {
            Fix::Repair(repair) => match repair.commands() {
                Some(lines) => {
                    let fix = match repair {
                        Repair::InstallService { .. } => CommandFix::Install,
                        _ => CommandFix::Start,
                    };
                    (commands(fix), FixWork::Commands(lines))
                }
                None => (setup(SetupFix::Download), FixWork::FindSetup),
            },
            Fix::UseLocal(local) => (
                commands(CommandFix::UseLocal),
                FixWork::Commands(use_local_commands(&local.program)),
            ),
            Fix::Update => (setup(SetupFix::Update), FixWork::FindSetup),
            Fix::OpenServices => return None,
        };
        *self = fixer;
        Some(work)
    }

    /// The setup the fix asks the person about, while it asks, and the
    /// fix it is for.
    pub(crate) fn confirming(&self) -> Option<(SetupFix, &Setup)> {
        match self {
            Fixer::Setup {
                fix,
                state: FixState::Running(SetupStep::Confirming { setup }),
            } => Some((*fix, setup)),
            _ => None,
        }
    }

    /// The checked setup the window starts, once the fix reached that step.
    pub(crate) fn starting_setup(&self) -> Option<&Path> {
        match self {
            Fixer::Setup {
                state: FixState::Running(SetupStep::StartingSetup { path, .. }),
                ..
            } => Some(path),
            _ => None,
        }
    }

    /// Whether the fix ended because the latest release has no setup to
    /// install, which a message box says.
    pub(crate) fn found_no_setup(&self) -> bool {
        matches!(
            self,
            Fixer::Setup {
                state: FixState::Ended(SetupResult::NoSetup),
                ..
            }
        )
    }

    /// The person answered whether to download the setup the fix asks
    /// about. Returns the setup to download; a fix that was not asking
    /// stays as it is.
    pub(crate) fn answer(&mut self, download: bool) -> Option<Setup> {
        let Fixer::Setup { state, .. } = self else {
            return None;
        };
        let FixState::Running(SetupStep::Confirming { setup }) = state else {
            return None;
        };
        let setup = setup.clone();
        *state = if download {
            FixState::Running(SetupStep::Downloading {
                version: setup.version.clone(),
                bytes: 0,
                size: setup.size,
            })
        } else {
            FixState::Ended(SetupResult::Declined)
        };
        download.then_some(setup)
    }

    /// Moves the fix on as `event` says. Returns whether it moved; the
    /// event of an ended fix, such as the end of a wait the person
    /// cancelled, changes nothing.
    pub(crate) fn accept(&mut self, event: FixEvent) -> bool {
        match (self, event) {
            (Fixer::Commands { state, .. }, FixEvent::Commands(event)) => {
                state.advance(|step| step.after(event))
            }
            (Fixer::Setup { state, .. }, FixEvent::Setup(event)) => {
                state.advance(|step| step.after(event))
            }
            _ => false,
        }
    }

    /// The service answered a status request. Returns whether that ended
    /// the fix. An answer before the commands end changes nothing, because
    /// the commands may still fail.
    pub(crate) fn service_answered(&mut self) -> bool {
        let Fixer::Commands { state, .. } = self else {
            return false;
        };
        state.advance(|step| {
            (*step == CommandStep::WaitingForService)
                .then_some(FixState::Ended(CommandResult::Done))
        })
    }

    /// Ends the fix as cancelled, if its step can stop. Returns whether it
    /// ended.
    pub(crate) fn cancel(&mut self) -> bool {
        match self {
            Fixer::Idle => false,
            Fixer::Commands { state, .. } => state.advance(|step| {
                step.can_cancel()
                    .then_some(FixState::Ended(CommandResult::Cancelled))
            }),
            Fixer::Setup { state, .. } => state.advance(|step| {
                step.can_cancel()
                    .then_some(FixState::Ended(SetupResult::Cancelled))
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(step: CommandStep) -> Fixer {
        Fixer::Commands {
            fix: CommandFix::Start,
            state: FixState::Running(step),
        }
    }

    fn setup(step: SetupStep) -> Fixer {
        Fixer::Setup {
            fix: SetupFix::Download,
            state: FixState::Running(step),
        }
    }

    fn command_event(event: CommandEvent) -> FixEvent {
        FixEvent::Commands(event)
    }

    fn setup_event(event: SetupEvent) -> FixEvent {
        FixEvent::Setup(event)
    }

    /// A fix that ran its commands but reported nothing, or that a Cancel
    /// left half-run, would leave the status bar wrong about the service
    /// (DESIGN.md: Service fixes).
    #[test]
    fn a_fix_moves_through_its_steps_to_each_result() {
        let mut fix = Fixer::Idle;
        let start = Fix::Repair(Repair::StartService);
        assert_eq!(
            fix.start(&start),
            Some(FixWork::Commands(vec!["net start gozler".into()]))
        );
        assert_eq!(fix.start(&start), None, "one fix runs at a time");
        // The prompt and the commands cannot stop; an early answer from
        // the service ends nothing.
        assert!(!fix.cancel());
        assert!(fix.accept(command_event(CommandEvent::Allowed)));
        assert!(!fix.cancel());
        assert!(!fix.service_answered());
        assert!(fix.accept(command_event(CommandEvent::Succeeded)));
        assert!(fix.waits_for_service());
        assert!(fix.service_answered());
        assert_eq!(
            fix,
            Fixer::Commands {
                fix: CommandFix::Start,
                state: FixState::Ended(CommandResult::Done)
            }
        );

        // A cancelled wait ignores the end of the wait that follows, and a
        // new fix starts after any result.
        let mut fix = commands(CommandStep::WaitingForService);
        assert!(fix.cancel());
        let timed_out = CommandEvent::End(CommandResult::NoAnswer);
        assert!(!fix.accept(command_event(timed_out)));
        assert_eq!(
            fix,
            Fixer::Commands {
                fix: CommandFix::Start,
                state: FixState::Ended(CommandResult::Cancelled)
            }
        );
        assert!(fix.start(&Fix::Update).is_some());
    }

    /// cmd runs these lines, and no other test runs them: `install` must
    /// come before the stop, so that a failed `install` leaves the service
    /// running.
    #[test]
    fn use_local_installs_then_restarts_the_service() {
        // `/` separates folders on every OS that CI tests on, `\` only on
        // Windows.
        assert_eq!(
            use_local_commands(Path::new("C:/Source/goz/target/debug/gozlerd.exe")),
            [
                r#"pushd "C:/Source/goz/target/debug""#,
                r".\gozlerd.exe install",
                "net stop gozler",
                "net start gozler",
            ]
        );
    }

    /// A download that a Cancel or a failed check left would run a setup
    /// nobody checked, one the person did not accept would download
    /// anyway, and a step that ignored an event would hang.
    #[test]
    fn a_download_finds_asks_writes_checks_and_starts_the_setup() {
        let found = Setup {
            version: "0.4.0".into(),
            older: false,
            name: "gozler-0.4.0-x64-Setup.exe".into(),
            host: "github.com".into(),
            path: "/s/gozler-0.4.0-x64-Setup.exe".into(),
            size: 10,
            sha256: [0; 32],
        };
        let mut fix = Fixer::Idle;
        assert_eq!(
            fix.start(&Fix::Repair(Repair::Download)),
            Some(FixWork::FindSetup)
        );
        assert!(fix.accept(setup_event(SetupEvent::Found(found.clone()))));
        assert_eq!(fix.confirming(), Some((SetupFix::Download, &found)));
        assert!(!fix.cancel(), "the question is the window's to close");
        let mut declined = fix.clone();
        assert_eq!(declined.answer(false), None);
        assert!(!declined.running());
        assert_eq!(fix.answer(true), Some(found));
        assert!(fix.accept(setup_event(SetupEvent::Downloaded(4))));
        assert_eq!(
            fix,
            setup(SetupStep::Downloading {
                version: "0.4.0".into(),
                bytes: 4,
                size: 10,
            })
        );
        assert!(fix.accept(setup_event(SetupEvent::Checking)));
        let mut cancelled = fix.clone();
        assert!(cancelled.cancel());
        assert!(!cancelled.accept(setup_event(SetupEvent::Ready("cancelled.exe".into()))));
        assert_eq!(cancelled.starting_setup(), None);
        let ready = SetupEvent::Ready(PathBuf::from("setup.exe"));
        assert!(fix.accept(setup_event(ready)));
        assert_eq!(fix.starting_setup(), Some(Path::new("setup.exe")));
        assert!(!fix.cancel(), "the setup is starting");

        let mut fix = setup(SetupStep::FindingRelease);
        assert!(fix.cancel());
        let failed = SetupEvent::End(SetupResult::DownloadFailed);
        assert!(!fix.accept(setup_event(failed)));
        assert_eq!(
            fix,
            Fixer::Setup {
                fix: SetupFix::Download,
                state: FixState::Ended(SetupResult::Cancelled)
            }
        );
    }
}
