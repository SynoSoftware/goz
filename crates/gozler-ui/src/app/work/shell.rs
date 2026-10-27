//! Purpose-local shell invocations retain their STA execution and source.

use super::{REPOSITORY, Surface};
use crate::core::clipboard::folder_of;
use crate::core::fetch::List;
use crate::core::open::{Commands, Entries, EntryAction, Selected};
use crate::core::work::{Id, Replacement, Retirement, Waiting};
use crate::win::folder;
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ShellTarget {
    Repository,
    Services,
}

impl ShellTarget {
    pub(in crate::app) fn address(self) -> &'static str {
        match self {
            Self::Repository => REPOSITORY,
            Self::Services => "services.msc",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ShellPurpose {
    Target(Surface, ShellTarget),
    Entries(List, EntryAction),
    Commander(List),
    Setup,
}

enum Invocation {
    Target(Surface, ShellTarget),
    Setup(PathBuf),
    Entries {
        list: List,
        action: EntryAction,
        entries: Entries,
        commands: Commands,
    },
    /// Show in Commander, which waits until the entry's folder opens
    /// (DESIGN.md: Searching, "where Open goes").
    Commander {
        list: List,
        entry: Selected,
    },
}

impl Invocation {
    fn items(&self) -> usize {
        match self {
            Self::Entries { entries, .. } => entries.items(),
            Self::Target(..) | Self::Setup(_) | Self::Commander { .. } => 0,
        }
    }

    fn purpose(&self) -> ShellPurpose {
        match self {
            Self::Target(surface, target) => ShellPurpose::Target(*surface, *target),
            Self::Setup(_) => ShellPurpose::Setup,
            Self::Entries { list, action, .. } => ShellPurpose::Entries(*list, *action),
            Self::Commander { list, .. } => ShellPurpose::Commander(*list),
        }
    }
}

pub(in crate::app) enum OpenTarget {
    Command(String),
    Path(Vec<u16>),
}

pub(in crate::app) struct OpenFailure {
    pub(in crate::app) target: OpenTarget,
    pub(in crate::app) error: io::Error,
}

#[derive(Default)]
pub(in crate::app) struct Opened {
    pub(in crate::app) failure: Option<OpenFailure>,
    pub(in crate::app) missing: Option<Vec<u16>>,
    /// The entry whose folder opened, which the window shows in Commander.
    pub(in crate::app) commander: Option<Selected>,
}

pub(in crate::app) struct Work {
    tasks: Vec<(ShellPurpose, Task<Invocation, Opened>)>,
    setup_surface: Surface,
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            tasks: List::ALL
                .into_iter()
                .flat_map(|list| EntryAction::ALL.map(|action| ShellPurpose::Entries(list, action)))
                .chain(List::ALL.map(ShellPurpose::Commander))
                .chain(
                    [Surface::Main, Surface::Settings]
                        .into_iter()
                        .flat_map(|surface| {
                            [ShellTarget::Repository, ShellTarget::Services]
                                .map(|target| ShellPurpose::Target(surface, target))
                        }),
                )
                .chain([ShellPurpose::Setup])
                .map(|purpose| {
                    let replacement = if purpose == ShellPurpose::Setup {
                        Replacement::Finish
                    } else {
                        Replacement::Cancel
                    };
                    (
                        purpose,
                        Task::shell(waker.clone(), replacement, invoke)
                            .with_size(Invocation::items),
                    )
                })
                .collect(),
            setup_surface: Surface::Main,
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn open(&mut self, target: ShellTarget, surface: Surface) {
        self.invoke(Invocation::Target(surface, target));
    }

    pub(in crate::app) fn start_setup(&mut self, path: PathBuf, surface: Surface) {
        self.setup_surface = surface;
        self.invoke(Invocation::Setup(path));
    }

    pub(in crate::app) fn open_entries(
        &mut self,
        list: List,
        action: EntryAction,
        entries: Entries,
        commands: Commands,
    ) {
        self.invoke(Invocation::Entries {
            list,
            action,
            entries,
            commands,
        });
    }

    pub(in crate::app) fn show_in_commander(&mut self, list: List, entry: Selected) {
        self.invoke(Invocation::Commander { list, entry });
    }

    fn invoke(&mut self, invocation: Invocation) {
        let purpose = invocation.purpose();
        let task = &mut self
            .tasks
            .iter_mut()
            .find(|(each, _)| *each == purpose)
            .expect("every shell purpose has a slot")
            .1;
        task.replace(invocation);
        let retired = task.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_opened(&mut self) -> Vec<(ShellPurpose, Id, Opened)> {
        let mut retired = Retirement::default();
        let opened = self
            .tasks
            .iter_mut()
            .filter_map(|(target, task)| {
                let opened = task.take();
                retired.append(task.take_retired());
                let (id, opened) = opened?;
                (*target == ShellPurpose::Setup
                    || opened.failure.is_some()
                    || opened.missing.is_some()
                    || opened.commander.is_some())
                .then_some((*target, id, opened))
            })
            .collect();
        self.retired.append(retired);
        opened
    }

    pub(in crate::app) fn is_current(&self, purpose: ShellPurpose, id: Id) -> bool {
        self.tasks
            .iter()
            .any(|(each, task)| *each == purpose && task.is_current(id))
    }

    pub(super) fn slow(&self, list: List, now: Instant) -> Option<ShellPurpose> {
        self.tasks.iter().find_map(|(purpose, task)| {
            (matches!(purpose, ShellPurpose::Entries(source, _) | ShellPurpose::Commander(source)
                if *source == list)
                && task.waiting(now) == Some(Waiting::Slow))
            .then_some(*purpose)
        })
    }

    pub(super) fn global_waiting(
        &self,
        surface: Surface,
        now: Instant,
    ) -> Option<(ShellPurpose, Waiting)> {
        self.tasks.iter().find_map(|(purpose, task)| {
            let owner = match purpose {
                ShellPurpose::Target(owner, _) => *owner,
                ShellPurpose::Setup => self.setup_surface,
                ShellPurpose::Entries(..) | ShellPurpose::Commander(_) => return None,
            };
            (owner == surface)
                .then(|| task.waiting(now).map(|waiting| (*purpose, waiting)))
                .flatten()
        })
    }

    pub(super) fn setup_waiting(&self, now: Instant) -> Option<Waiting> {
        self.tasks.iter().find_map(|(purpose, task)| {
            (*purpose == ShellPurpose::Setup)
                .then(|| task.waiting(now))
                .flatten()
        })
    }

    pub(super) fn cancel(&mut self, purpose: ShellPurpose) {
        self.tasks
            .iter_mut()
            .find(|(source, _)| *source == purpose)
            .expect("every shell purpose has a slot")
            .1
            .cancel();
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.tasks.iter_mut().fold(false, |active, (_, task)| {
            task.tick(now).is_some() || active
        })
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        for (_, task) in &mut self.tasks {
            self.retired.append(task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        for (_, task) in &mut self.tasks {
            task.cancel();
            self.retired.append(task.close_delivery());
        }
        self.take_retired()
    }
}

fn invoke(
    invocation: Invocation,
    session: &crate::win::shell::Session,
    _worker: &OffThread,
    cancel: &CancelToken,
) -> Opened {
    if cancel.is_cancelled() {
        return Opened::default();
    }
    match invocation {
        Invocation::Setup(path) => Opened {
            failure: session.start_program(&path).err().map(|error| OpenFailure {
                target: OpenTarget::Path(path.as_os_str().encode_wide().collect()),
                error,
            }),
            ..Opened::default()
        },
        Invocation::Target(_, target) => Opened {
            failure: session
                .open(target.address())
                .err()
                .map(|error| OpenFailure {
                    target: OpenTarget::Path(target.address().encode_utf16().collect()),
                    error,
                }),
            ..Opened::default()
        },
        Invocation::Commander { entry, .. } => {
            // Only a name that is gone is missing: a file in a folder that
            // Windows refuses, such as one in C:\Windows\Prefetch, gets
            // the folder's reason below.
            let gone = folder::names(&entry.path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
            if gone {
                return Opened {
                    missing: Some(entry.path),
                    ..Opened::default()
                };
            }
            let folder = if entry.is_folder {
                entry.path.clone()
            } else {
                folder_of(&entry.path).to_vec()
            };
            match folder::lists(&folder) {
                Ok(()) => Opened {
                    commander: Some(entry),
                    ..Opened::default()
                },
                Err(error) => Opened {
                    failure: Some(OpenFailure {
                        target: OpenTarget::Path(folder),
                        error,
                    }),
                    ..Opened::default()
                },
            }
        }
        Invocation::Entries {
            list,
            action,
            entries,
            commands,
        } => {
            let mut opened = Opened::default();
            if cancel.is_cancelled() {
                return opened;
            }
            let entries = entries
                .resolve()
                .into_iter()
                .take_while(|_| !cancel.is_cancelled());
            let entries: Vec<_> = if list == List::Search {
                entries
                    .filter(|entry| {
                        if std::fs::symlink_metadata(OsString::from_wide(&entry.path)).is_ok() {
                            true
                        } else {
                            opened.missing.get_or_insert_with(|| entry.path.clone());
                            false
                        }
                    })
                    .collect()
            } else {
                entries.collect()
            };
            opened.failure = act_on_entries(session, action, entries, &commands, cancel).err();
            opened
        }
    }
}

fn act_on_entries(
    session: &crate::win::shell::Session,
    action: EntryAction,
    entries: Vec<Selected>,
    commands: &Commands,
    cancel: &CancelToken,
) -> Result<(), OpenFailure> {
    let mut files = Vec::new();
    let mut failure = None;
    for entry in entries.into_iter().take_while(|_| !cancel.is_cancelled()) {
        if let Some(line) = commands.expand(action, &entry) {
            session
                .run_command(&line.program, &line.parameters)
                .map_err(|error| OpenFailure {
                    target: OpenTarget::Command(line.program),
                    error,
                })?;
            continue;
        }
        let result = match action {
            EntryAction::Open => session.open_entry(&entry.path),
            EntryAction::Properties => session.properties(&entry.path),
            EntryAction::Explore if entry.is_folder => session.explore(&entry.path),
            EntryAction::OpenFileLocation | EntryAction::Explore => {
                files.push(entry);
                continue;
            }
        };
        if let Err(error) = result {
            failure.get_or_insert(OpenFailure {
                target: OpenTarget::Path(entry.path),
                error,
            });
        }
    }
    let mut folders: Vec<(Vec<u16>, Vec<Vec<u16>>)> = Vec::new();
    for Selected { path, .. } in files {
        let folder = folder_of(&path).to_vec();
        match folders.iter_mut().find(|(each, _)| *each == folder) {
            Some((_, entries)) => entries.push(path),
            None => folders.push((folder, vec![path])),
        }
    }
    for (folder, entries) in folders.into_iter().take_while(|_| !cancel.is_cancelled()) {
        if let Err(error) = session.show_in_folder(&folder, &entries) {
            failure.get_or_insert(OpenFailure {
                target: OpenTarget::Path(folder),
                error,
            });
        }
    }
    match failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}
