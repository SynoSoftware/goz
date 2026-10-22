//! The check of the two gozlerd.exe files that Use local and Settings >
//! Index show (DESIGN.md: Daemon failures): which file runs, in which order,
//! and when a check runs again. `app::local_service` reads and runs the
//! files through the closures of [`check`], and holds the [`Cache`].

use super::fetch::{Beside, Local, LocalService, ProgramFile, Programs};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A gozlerd.exe that can be read, with its build. The last write time and
/// the length change with each build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) path: PathBuf,
    pub(crate) modified: SystemTime,
    pub(crate) length: u64,
}

/// The files a check reads, each when it can be read: the gozlerd.exe beside
/// the window and the program the service's configuration names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Files {
    pub(crate) local: Option<Found>,
    pub(crate) registered: Option<Found>,
}

/// What running a program for its answer gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Printed {
    Text(String),
    /// It did not end in time. A first run can wait for an antivirus scan,
    /// so a later run may answer.
    TimedOut,
    /// It did not start or ended with an error, which a later run repeats.
    Failed,
}

/// What a check found, and whether a program it ran was ended at the time
/// limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub(crate) programs: Programs,
    pub(crate) timed_out: bool,
}

/// What `files` hold. The service's program prints its version. A gozlerd.exe
/// with the service program's bytes is that build, and nothing more runs. Any
/// other runs only when `is_paired` accepts it, so a file that the setup's
/// build did not pair with the window never runs. A paired one prints its
/// version and its `protocol`.
///
/// `same_bytes` reads both files, so it is called only for files of one
/// length. `print` runs a program with one argument.
pub(crate) fn check(
    files: &Files,
    same_bytes: &dyn Fn(&Path, &Path) -> bool,
    is_paired: &dyn Fn(&Path) -> bool,
    print: &dyn Fn(&Path, &str) -> Printed,
) -> Checked {
    let timed_out = Cell::new(false);
    let printed = |program: &Path, argument: &str| match print(program, argument) {
        Printed::Text(text) => Some(text),
        Printed::TimedOut => {
            timed_out.set(true);
            None
        }
        Printed::Failed => None,
    };
    let programs = programs_of(files, same_bytes, is_paired, &printed);
    Checked {
        programs,
        timed_out: timed_out.get(),
    }
}

fn programs_of(
    files: &Files,
    same_bytes: &dyn Fn(&Path, &Path) -> bool,
    is_paired: &dyn Fn(&Path) -> bool,
    print: &dyn Fn(&Path, &str) -> Option<String>,
) -> Programs {
    // What `gozlerd.exe --version` prints after the program's name, such as
    // `0.3.0`.
    let version = |program: &Path| {
        let printed = print(program, "--version")?;
        printed.trim().strip_prefix("gozlerd ").map(str::to_owned)
    };
    let service = files.registered.as_ref().map(|registered| ProgramFile {
        path: registered.path.clone(),
        version: version(&registered.path),
        modified: filetime(registered.modified),
    });
    let mut programs = Programs {
        service,
        ..Programs::UNKNOWN
    };
    let Some(local) = &files.local else {
        return programs;
    };
    if let Some(registered) = &files.registered
        && local.length == registered.length
        && same_bytes(&local.path, &registered.path)
    {
        programs.beside = Beside::SameAsService(local.path.clone());
        return programs;
    }
    let paired = is_paired(&local.path);
    programs.beside = Beside::Other(ProgramFile {
        path: local.path.clone(),
        version: if paired { version(&local.path) } else { None },
        modified: filetime(local.modified),
    });
    if !paired {
        programs.local = Local::Mismatched;
        return programs;
    }
    let installed = programs
        .service
        .as_ref()
        .and_then(|service| service.version.clone());
    if let Some(offered) = print(&local.path, "protocol")
        .and_then(|protocol| LocalService::from_protocol(local.path.clone(), &protocol, installed))
    {
        programs.local = Local::Offered(offered);
    }
    programs
}

/// `time` as a FILETIME, which the window's date formats read.
fn filetime(time: SystemTime) -> i64 {
    let ms = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    };
    gozler_core::output::filetime::unix_ms_to_filetime(ms)
}

/// The latest [`Files`] and their check. A check runs once for each build of
/// the files, because the status poll asks every two seconds, and a developer
/// who rebuilds gozlerd.exe expects the new build to count. A check that
/// ended a program at the time limit runs once more, then stays as it is, so
/// a program that never answers does not run at every status.
pub(crate) struct Cache(Option<(Files, Entry)>);

enum Entry {
    Running {
        /// The timed-out check's programs, while this check follows it.
        first: Option<Box<Programs>>,
    },
    Done {
        programs: Box<Programs>,
        /// The check timed out and has not been repeated.
        retry_due: bool,
    },
}

/// The answer to a status that asks for the programs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    /// [`Programs::UNKNOWN`] while a build's first check runs or starts,
    /// and that check's programs while it runs once more, so Settings
    /// keeps showing them.
    pub(crate) programs: Programs,
    /// The caller starts a check of the asked files and reports it with
    /// [`Cache::finish`].
    pub(crate) start: bool,
}

impl Cache {
    pub(crate) const fn new() -> Cache {
        Cache(None)
    }

    /// What a status gets for `files`. Files that cannot be read need no
    /// check.
    pub(crate) fn ask(&mut self, files: &Files) -> Asked {
        let unknown = |start| Asked {
            programs: Programs::UNKNOWN,
            start,
        };
        if files.local.is_none() && files.registered.is_none() {
            return unknown(false);
        }
        if let Some((latest, entry)) = &mut self.0
            && *latest == *files
        {
            return match entry {
                Entry::Running { first } => Asked {
                    programs: first.as_deref().cloned().unwrap_or(Programs::UNKNOWN),
                    start: false,
                },
                Entry::Done {
                    programs,
                    retry_due: false,
                } => Asked {
                    programs: (**programs).clone(),
                    start: false,
                },
                Entry::Done {
                    programs,
                    retry_due: true,
                } => {
                    let first = std::mem::replace(programs, Box::new(Programs::UNKNOWN));
                    let programs = (*first).clone();
                    *entry = Entry::Running { first: Some(first) };
                    Asked {
                        programs,
                        start: true,
                    }
                }
            };
        }
        self.0 = Some((files.clone(), Entry::Running { first: None }));
        unknown(true)
    }

    /// Records the end of the check of `files`; dropped when a newer build
    /// replaced them meanwhile.
    pub(crate) fn finish(&mut self, files: &Files, checked: Checked) {
        if let Some((latest, entry)) = &mut self.0
            && *latest == *files
            && let Entry::Running { first } = entry
        {
            let retried = first.is_some();
            *entry = Entry::Done {
                programs: Box::new(checked.programs),
                retry_due: checked.timed_out && !retried,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gozler_core::proto::PROTO_VERSION;
    use std::cell::RefCell;
    use std::time::Duration;

    fn found(path: &str, length: u64) -> Found {
        Found {
            path: PathBuf::from(path),
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
            length,
        }
    }

    fn files(local_length: u64, registered_length: u64) -> Files {
        Files {
            local: Some(found("local.exe", local_length)),
            registered: Some(found("service.exe", registered_length)),
        }
    }

    /// What `check` asked of its closures.
    struct Log {
        printed: Vec<String>,
        compared: bool,
    }

    /// Runs `check` with files that `same` makes equal or not, a pairing
    /// that `paired` decides, and the answers `answer` gives to a program's
    /// argument.
    fn run(
        files: &Files,
        same: bool,
        paired: bool,
        answer: impl Fn(&str) -> Printed,
    ) -> (Checked, Log) {
        let printed = RefCell::new(Vec::new());
        let compared = Cell::new(false);
        let checked = check(
            files,
            &|_, _| {
                compared.set(true);
                same
            },
            &|_| paired,
            &|program, argument| {
                printed
                    .borrow_mut()
                    .push(format!("{} {argument}", program.display()));
                answer(argument)
            },
        );
        (
            checked,
            Log {
                printed: printed.into_inner(),
                compared: compared.get(),
            },
        )
    }

    fn version_text(_: &str) -> Printed {
        Printed::Text("gozlerd 0.4.0".into())
    }

    /// A gozlerd.exe that the setup's build did not pair with the window
    /// could be anything, so it must not run, not even for its version.
    #[test]
    fn an_unpaired_gozlerd_never_runs() {
        let (checked, log) = run(&files(10, 20), false, false, version_text);
        assert_eq!(checked.programs.local, Local::Mismatched);
        assert!(matches!(
            checked.programs.beside,
            Beside::Other(ProgramFile { version: None, .. })
        ));
        assert_eq!(log.printed, ["service.exe --version"]);
    }

    /// A gozlerd.exe with the service program's bytes is not offered over it,
    /// and running it would only repeat the service program's answers.
    #[test]
    fn a_gozlerd_with_the_service_programs_bytes_does_not_run() {
        let (checked, log) = run(&files(10, 10), true, true, version_text);
        assert_eq!(
            checked.programs.beside,
            Beside::SameAsService(PathBuf::from("local.exe"))
        );
        assert_eq!(checked.programs.local, Local::NotOffered);
        assert_eq!(log.printed, ["service.exe --version"]);
    }

    /// Files of different lengths cannot be equal, and reading two
    /// executables costs more than comparing their lengths.
    #[test]
    fn files_of_different_lengths_are_never_read() {
        let (_, log) = run(&files(10, 20), true, false, version_text);
        assert!(!log.compared);
        let (_, log) = run(&files(10, 10), false, false, version_text);
        assert!(log.compared);
    }

    /// The offer carries the service program's version, so that program
    /// prints before the local one makes an offer.
    #[test]
    fn a_paired_gozlerd_is_offered_over_the_service_programs_version() {
        let (checked, log) = run(&files(10, 20), false, true, |argument| match argument {
            "protocol" => Printed::Text(format!("1 {PROTO_VERSION} 0.5.0")),
            _ => Printed::Text("gozlerd 0.4.0".into()),
        });
        assert_eq!(
            log.printed,
            [
                "service.exe --version",
                "local.exe --version",
                "local.exe protocol"
            ]
        );
        assert_eq!(
            checked.programs.local,
            Local::Offered(LocalService {
                program: PathBuf::from("local.exe"),
                version: "0.5.0".into(),
                installed: Some("0.4.0".into()),
            })
        );
        assert!(!checked.timed_out);
    }

    /// Only a program ended at the time limit may answer at another try; one
    /// that failed repeats its failure.
    #[test]
    fn only_a_timeout_is_reported_as_one() {
        let answer = |result: Printed| {
            move |argument: &str| match argument {
                "protocol" => result.clone(),
                _ => Printed::Text("gozlerd 0.4.0".into()),
            }
        };
        let (checked, _) = run(&files(10, 20), false, true, answer(Printed::TimedOut));
        assert!(checked.timed_out);
        assert_eq!(checked.programs.local, Local::NotOffered);
        let (checked, _) = run(&files(10, 20), false, true, answer(Printed::Failed));
        assert!(!checked.timed_out);
    }

    fn checked(timed_out: bool) -> Checked {
        Checked {
            programs: Programs {
                beside: Beside::SameAsService(PathBuf::from("local.exe")),
                ..Programs::UNKNOWN
            },
            timed_out,
        }
    }

    /// The status poll asks every two seconds, and each check runs the
    /// programs.
    #[test]
    fn a_finished_check_answers_until_the_build_changes() {
        let current = files(10, 20);
        let mut cache = Cache::new();
        assert!(cache.ask(&current).start);
        assert!(!cache.ask(&current).start);
        cache.finish(&current, checked(false));
        let asked = cache.ask(&current);
        assert!(!asked.start);
        assert_eq!(asked.programs, checked(false).programs);
        let rebuilt = files(11, 20);
        assert!(cache.ask(&rebuilt).start);
        // The older check ends after the newer files replaced it.
        cache.finish(&current, checked(false));
        assert!(!cache.ask(&rebuilt).start);
        assert_eq!(cache.ask(&rebuilt).programs, Programs::UNKNOWN);
    }

    /// One slow first run (an antivirus scan) must not hide Use local until
    /// the window restarts, and a program that never answers must not run at
    /// every status.
    #[test]
    fn a_timed_out_check_runs_once_more_and_is_then_kept() {
        let current = files(10, 20);
        let mut cache = Cache::new();
        assert!(cache.ask(&current).start);
        cache.finish(&current, checked(true));
        assert!(cache.ask(&current).start);
        // Settings > Index keeps the first check's programs meanwhile.
        assert_eq!(cache.ask(&current).programs, checked(true).programs);
        cache.finish(&current, checked(true));
        let asked = cache.ask(&current);
        assert!(!asked.start);
        assert_eq!(asked.programs, checked(true).programs);
        // A new build earns its own retry.
        let rebuilt = files(11, 20);
        assert!(cache.ask(&rebuilt).start);
        cache.finish(&rebuilt, checked(true));
        assert!(cache.ask(&rebuilt).start);
    }
}
