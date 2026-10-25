//! Operation follower calculations, report preparation and command paths.

use crate::app::dialogs::TransferStart;
use crate::core::fetch::List;
use crate::core::open::Entries;
use crate::core::operations::{Followed, Following, PreparedFailure, Reporting};
use crate::core::panel::CommandPlace;
use crate::core::work::{Id, Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::Task;
use gozler_client::CancelToken;
use gozler_core::proto::ResultItem;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Instant;

struct Follow {
    input: Following,
    numbers: crate::win::locale::WindowsNumbers,
}

struct Report {
    input: Reporting,
    numbers: crate::win::locale::WindowsNumbers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum PathPurpose {
    Copy(List),
    Move(List),
    Delete(List),
}

impl PathPurpose {
    fn list(self) -> List {
        match self {
            Self::Copy(list) | Self::Move(list) | Self::Delete(list) => list,
        }
    }
}

pub(in crate::app) enum PathAction {
    Transfer {
        moves: bool,
    },
    PanelTransfer {
        moves: bool,
        place: CommandPlace,
        selected: bool,
        start: Box<TransferStart>,
    },
    Delete {
        place: Option<CommandPlace>,
    },
}

impl PathAction {
    fn purpose(&self, list: List) -> PathPurpose {
        match self {
            Self::Transfer { moves: false } | Self::PanelTransfer { moves: false, .. } => {
                PathPurpose::Copy(list)
            }
            Self::Transfer { moves: true } | Self::PanelTransfer { moves: true, .. } => {
                PathPurpose::Move(list)
            }
            Self::Delete { .. } => PathPurpose::Delete(list),
        }
    }
}

struct PreparingPaths {
    action: PathAction,
    entries: Entries,
}

impl PreparingPaths {
    fn items(&self) -> usize {
        self.entries.items()
    }
}

pub(in crate::app) struct PreparedPaths {
    pub(in crate::app) action: PathAction,
    pub(in crate::app) paths: Vec<PathBuf>,
    pub(in crate::app) single_folder: bool,
}

pub(in crate::app) struct Work {
    following: Task<Follow, Followed>,
    reporting: Task<Report, PreparedFailure>,
    paths: Vec<(PathPurpose, Task<PreparingPaths, PreparedPaths>)>,
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            following: Task::pass(
                waker.clone(),
                Replacement::Finish,
                |follow: &Follow| follow.input.items(),
                |follow, _| follow.input.run(&follow.numbers),
            ),
            reporting: Task::pass(
                waker.clone(),
                Replacement::Finish,
                |report: &Report| report.input.items(),
                |report, _| {
                    report.input.run(&report.numbers, &|code| {
                        crate::win::error_text(&io::Error::from_raw_os_error(code as i32))
                    })
                },
            ),
            paths: List::ALL
                .into_iter()
                .flat_map(|list| {
                    [
                        PathPurpose::Copy(list),
                        PathPurpose::Move(list),
                        PathPurpose::Delete(list),
                    ]
                })
                .map(|purpose| {
                    (
                        purpose,
                        Task::pass(
                            waker.clone(),
                            Replacement::Cancel,
                            PreparingPaths::items,
                            prepare_paths,
                        ),
                    )
                })
                .collect(),
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn follow(
        &mut self,
        input: Following,
        numbers: crate::win::locale::WindowsNumbers,
    ) {
        self.following.replace(Follow { input, numbers });
        let retired = self.following.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_followed(&mut self) -> Option<Followed> {
        let answer = self.following.take().map(|(_, answer)| answer);
        let retired = self.following.take_retired();
        self.retired.append(retired);
        answer
    }

    pub(in crate::app) fn prepare_report(
        &mut self,
        input: Reporting,
        numbers: crate::win::locale::WindowsNumbers,
    ) {
        assert!(
            !self.reporting.is_running(),
            "the modal queue prepares one report at a time"
        );
        self.reporting.replace(Report { input, numbers });
        let retired = self.reporting.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn preparing_report(&self) -> bool {
        self.reporting.is_running()
    }

    pub(in crate::app) fn take_report(&mut self) -> Option<PreparedFailure> {
        let answer = self.reporting.take().map(|(_, answer)| answer);
        let retired = self.reporting.take_retired();
        self.retired.append(retired);
        answer
    }

    pub(in crate::app) fn prepare_paths(
        &mut self,
        list: List,
        action: PathAction,
        entries: Entries,
    ) {
        let purpose = action.purpose(list);
        let task = &mut self
            .paths
            .iter_mut()
            .find(|(each, _)| *each == purpose)
            .expect("each path command has a slot")
            .1;
        task.replace(PreparingPaths { action, entries });
        let retired = task.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn paths_are_current(&self, purpose: PathPurpose, id: Id) -> bool {
        self.paths
            .iter()
            .find(|(each, _)| *each == purpose)
            .is_some_and(|(_, task)| task.is_current(id))
    }

    pub(in crate::app) fn take_paths(&mut self) -> Vec<(PathPurpose, Id, PreparedPaths)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for (purpose, task) in &mut self.paths {
            if let Some((id, answer)) = task.take() {
                answers.push((*purpose, id, answer));
            }
            retired.append(task.take_retired());
        }
        self.retired.append(retired);
        answers
    }

    pub(super) fn slow_paths(&self, list: List, now: Instant) -> Option<PathPurpose> {
        self.paths.iter().find_map(|(purpose, task)| {
            (purpose.list() == list && task.waiting(now) == Some(Waiting::Slow)).then_some(*purpose)
        })
    }

    pub(super) fn cancel_paths(&mut self, purpose: PathPurpose) {
        self.paths
            .iter_mut()
            .find(|(source, _)| *source == purpose)
            .expect("each path command has a slot")
            .1
            .cancel();
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = self.following.tick(now).is_some();
        active |= self.reporting.tick(now).is_some();
        for (_, task) in &mut self.paths {
            active |= task.tick(now).is_some();
        }
        active
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        self.retired.append(self.following.take_retired());
        self.retired.append(self.reporting.take_retired());
        for (_, task) in &mut self.paths {
            self.retired.append(task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        self.following.cancel();
        self.reporting.cancel();
        self.retired.append(self.following.close_delivery());
        self.retired.append(self.reporting.close_delivery());
        for (_, task) in &mut self.paths {
            task.cancel();
            self.retired.append(task.close_delivery());
        }
        self.take_retired()
    }
}

fn prepare_paths(input: PreparingPaths, cancel: &CancelToken) -> PreparedPaths {
    let single_folder = input.entries.one().is_some_and(|entry| entry.is_folder);
    let paths = if cancel.is_cancelled() {
        Vec::new()
    } else {
        match input.entries {
            Entries::Results(files) => files
                .iter()
                .take_while(|_| !cancel.is_cancelled())
                .map(os_path)
                .collect(),
            entries => entries
                .resolve()
                .into_iter()
                .take_while(|_| !cancel.is_cancelled())
                .map(|entry| PathBuf::from(OsString::from_wide(&entry.path)))
                .collect(),
        }
    };
    PreparedPaths {
        action: input.action,
        paths,
        single_folder,
    }
}

/// Uses a result's native path when present, avoiding a UTF-16 round trip for
/// each of a large selection's ordinary Unicode paths.
fn os_path(file: &ResultItem) -> PathBuf {
    match &file.path_u16 {
        Some(units) => PathBuf::from(OsString::from_wide(units)),
        None => PathBuf::from(&file.path),
    }
}
