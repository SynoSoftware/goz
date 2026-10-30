//! Typed UI work groups share admission, retirement and one explicit close;
//! Queue retains its own engine scheduling and recovery.

use crate::core::fetch::{Generation, List, Purpose};
use crate::core::panel::Side;
use crate::core::work::{Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use gozler_fileops::Taken;
use std::collections::LinkedList;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

mod clipboard;
pub(super) mod drops;
pub(super) mod elevated;
pub(super) mod fixes;
pub(super) mod folders;
pub(super) mod indexed;
mod lists;
mod local_service;
mod operations;
mod panels;
pub(super) mod paste;
pub(super) mod programs;
mod queue;
pub(super) mod requests;
mod settings;
mod shell;
pub(super) mod tray;
pub(super) mod viewers;

pub(super) use clipboard::{CopyFailure, CopyNotice};
pub(super) use operations::{PathAction, PathPurpose, PreparedPaths};
pub(super) use queue::{
    Accepted as QueueAccepted, CheckPurpose, Client as QueueClient, Ready as QueueReady,
};
#[cfg(test)]
pub(crate) use settings::write_settings;
pub(super) use settings::{RegistryAnswer, RegistryRead, load_settings};
pub(super) use shell::{OpenTarget, Opened, ShellPurpose, ShellTarget};

pub(super) const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

type Diagnostics = LinkedList<(PathBuf, String)>;

#[derive(Clone, Copy)]
pub(super) enum ReleasePurpose {
    Update,
    ServiceVersion,
}

impl ReleasePurpose {
    fn slot(self) -> usize {
        match self {
            Self::Update => 0,
            Self::ServiceVersion => 1,
        }
    }
}

pub(super) enum ReleaseAnswer {
    Newer(Option<String>),
    Older(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    Main,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Awaited {
    Request(Purpose),
    Selection(List),
    Matching(Side),
    Shell(ShellPurpose),
    Paths(PathPurpose),
    Clipboard,
    Pairing,
    Settings,
    Recovery,
    Listing(Side),
}

impl Awaited {
    pub(super) fn can_cancel(self) -> bool {
        !matches!(self, Self::Shell(ShellPurpose::Setup) | Self::Recovery)
    }
}

pub(super) struct Poll {
    pub(super) active: bool,
    pub(super) expired_requests: Vec<(Purpose, Generation)>,
    pub(super) expired_hover: Vec<drops::Expired>,
}

pub(super) struct Work {
    pub(super) drops: drops::Work,
    pub(super) requests: requests::Requests,
    pub(super) programs: programs::Checks,
    pub(super) folders: folders::Work,
    pub(super) fixes: fixes::Fixes,
    pub(super) elevated: elevated::Saves,
    pub(super) indexed: indexed::Work,
    pub(super) paste: paste::Work,
    pub(super) tray: Option<tray::Work>,
    pub(super) lists: lists::Work,
    pub(super) panels: panels::Work,
    pub(super) clipboard: clipboard::Work,
    pub(super) settings: settings::Work,
    pub(super) shell: shell::Work,
    pub(super) operations: operations::Work,
    pub(super) queue: queue::Work,
    releases: [Task<ReleasePurpose, ReleaseAnswer>; 2],
    retiring: Option<Task<Retirement, ()>>,
    diagnostics: Option<Task<Diagnostics, ()>>,
    watchdog: Option<(
        crate::win::work::watchdog::Thread,
        crate::win::watchdog::Endpoint,
    )>,
}

impl Work {
    pub(super) fn new(waker: OpenWaker, error_log: PathBuf) -> Self {
        let watchdog = crate::win::watchdog::Endpoint::new().ok().map(|endpoint| {
            let monitor = crate::win::work::watchdog::Thread::start(
                endpoint.delivery(),
                error_log.with_file_name("ui-slow.log"),
            );
            (monitor, endpoint)
        });
        Self {
            drops: drops::Work::new(waker.clone()),
            requests: requests::Requests::new(waker.clone(), error_log),
            programs: programs::Checks::new(waker.clone()),
            folders: folders::Work::new(waker.clone()),
            fixes: fixes::Fixes::new(waker.clone()),
            elevated: elevated::Saves::new(waker.clone()),
            indexed: indexed::Work::new(waker.clone()),
            paste: paste::Work::new(waker.clone()),
            tray: tray::Work::new(waker.clone(), crate::core::ids::IconId::Gozler.id()).ok(),
            lists: lists::Work::new(waker.clone()),
            panels: panels::Work::new(waker.clone()),
            clipboard: clipboard::Work::new(waker.clone()),
            settings: settings::Work::new(waker.clone()),
            shell: shell::Work::new(waker.clone()),
            operations: operations::Work::new(waker.clone()),
            queue: queue::Work::new(waker.clone()),
            releases: [
                Task::new(waker.clone(), Replacement::Cancel, check_release).background(),
                Task::new(waker.clone(), Replacement::Cancel, check_release),
            ],
            retiring: Some(Task::pass(
                waker.clone(),
                Replacement::Finish,
                Retirement::items,
                retire,
            )),
            diagnostics: Some(
                Task::new(waker.clone(), Replacement::Finish, write_diagnostics)
                    .with_size(Diagnostics::len),
            ),
            watchdog,
        }
    }

    pub(super) fn tick(&mut self, now: Instant) -> Poll {
        let requests = self.requests.tick(now);
        let expired_requests = requests
            .iter()
            .filter_map(|(purpose, generation, waiting)| {
                (*waiting == Waiting::Expired).then_some((*purpose, *generation))
            })
            .collect();
        let (mut active, expired_hover) = self.drops.tick(now);
        active |= requests
            .iter()
            .any(|(_, _, waiting)| *waiting != Waiting::Expired);
        active |= self.programs.tick(now);
        active |= self.lists.tick(now);
        active |= self.panels.tick(now);
        active |= self.clipboard.tick(now);
        active |= self.settings.tick(now);
        active |= self.shell.tick(now);
        active |= self.operations.tick(now);
        active |= self.queue.tick(now);
        active |= self.folders.tick(now);
        active |= self.fixes.tick(now);
        active |= self.elevated.tick(now);
        active |= self.indexed.tick(now);
        for task in &mut self.releases {
            active |= task.tick(now).is_some();
        }
        self.retire_groups();
        Poll {
            active,
            expired_requests,
            expired_hover,
        }
    }

    pub(super) fn slow(&self, list: List, selection: bool, now: Instant) -> Option<Awaited> {
        if let Some(purpose) = [Purpose::Selection(list), Purpose::Results(list)]
            .into_iter()
            .find(|purpose| self.requests.waiting(*purpose, now) == Some(Waiting::Slow))
        {
            return Some(Awaited::Request(purpose));
        }
        if selection && self.lists.slow(list, now) {
            return Some(Awaited::Selection(list));
        }
        if let Some(purpose) = self.shell.slow(list, now) {
            return Some(Awaited::Shell(purpose));
        }
        if let Some(purpose) = self.operations.slow_paths(list, now) {
            return Some(Awaited::Paths(purpose));
        }
        (list == List::Search && self.clipboard.waiting(now) == Some(Waiting::Slow))
            .then_some(Awaited::Clipboard)
    }

    pub(super) fn cancel_awaited(&mut self, awaited: Awaited) {
        match awaited {
            Awaited::Request(purpose) => self.requests.cancel(purpose),
            Awaited::Selection(list) => self.lists.cancel_selection(list),
            Awaited::Matching(side) => self.panels.cancel_matching(side),
            Awaited::Shell(purpose) => self.shell.cancel(purpose),
            Awaited::Paths(purpose) => self.operations.cancel_paths(purpose),
            Awaited::Clipboard => self.clipboard.cancel(),
            Awaited::Pairing => self.settings.cancel_pairing(),
            Awaited::Settings => self.settings.cancel_read_registry(),
            Awaited::Recovery => {}
            Awaited::Listing(side) => {
                self.panels.cancel(side);
                self.folders.cancel_read(side);
            }
        }
        self.retire_groups();
    }

    pub(super) fn global_waiting(
        &self,
        surface: Surface,
        now: Instant,
    ) -> Option<(Awaited, Waiting)> {
        if let Some(waiting) = self.settings.pairing_waiting(surface, now) {
            return Some((Awaited::Pairing, waiting));
        }
        if let Some((purpose, waiting)) = self.shell.global_waiting(surface, now) {
            return Some((Awaited::Shell(purpose), waiting));
        }
        if surface == Surface::Main {
            if let Some(waiting) = self.settings.registry_waiting(now) {
                return Some((Awaited::Settings, waiting));
            }
            return self
                .queue
                .waiting(now)
                .map(|waiting| (Awaited::Recovery, waiting));
        }
        None
    }

    pub(super) fn fix_waiting(&self, now: Instant) -> Option<Waiting> {
        self.fixes
            .waiting(now)
            .or_else(|| self.releases[ReleasePurpose::ServiceVersion.slot()].waiting(now))
            .or_else(|| self.shell.setup_waiting(now))
    }

    pub(super) fn check_release(&mut self, purpose: ReleasePurpose) {
        self.releases[purpose.slot()].replace(purpose);
    }

    pub(super) fn take_releases(&mut self) -> Vec<ReleaseAnswer> {
        self.releases
            .iter_mut()
            .filter_map(|task| task.take().map(|(_, answer)| answer))
            .collect()
    }

    pub(super) fn retire_taken(&mut self, taken: Arc<Taken>) {
        let items = taken.len();
        let mut retired = Retirement::default();
        retired.push(taken, items);
        self.retire(retired);
    }

    pub(super) fn retire(&mut self, retired: Retirement) {
        // Nested handlers can retire payloads after close transferred this task.
        let Some(task) = self.retiring.as_mut() else {
            if !retired.is_empty() {
                let _ = crate::win::work::spawn(move |_| drop(retired));
            }
            return;
        };
        let _ = task.take();
        if !retired.is_empty() {
            task.submit(retired, Retirement::combine);
        }
    }

    pub(super) fn log_error(&mut self, path: PathBuf, text: String) {
        let Some(task) = self.diagnostics.as_mut() else {
            return;
        };
        let _ = task.take();
        task.submit([(path, text)].into(), |mut older, mut newer| {
            older.append(&mut newer);
            older
        });
        let retired = task.take_retired();
        self.retire(retired);
    }

    fn retire_groups(&mut self) {
        let mut retired = self.requests.take_retired();
        if let Some(task) = self.diagnostics.as_mut() {
            let _ = task.take();
            retired.append(task.take_retired());
        }
        retired.append(self.drops.take_retired());
        retired.append(self.paste.take_retired());
        if let Some(tray) = &mut self.tray {
            retired.append(tray.take_retired());
        }
        retired.append(self.programs.take_retired());
        retired.append(self.lists.take_retired());
        retired.append(self.panels.take_retired());
        retired.append(self.clipboard.take_retired());
        retired.append(self.settings.take_retired());
        retired.append(self.shell.take_retired());
        retired.append(self.operations.take_retired());
        retired.append(self.queue.take_retired());
        retired.append(self.folders.take_retired());
        retired.append(self.fixes.take_retired());
        retired.append(self.elevated.take_retired());
        retired.append(self.indexed.take_retired());
        self.retire(retired);
    }

    /// Returns independent persistence and accepted-mutation cleanup. Neither
    /// closure may run on UI, and cleanup must not delay essential writes.
    pub(super) fn close(
        &mut self,
    ) -> (
        impl FnOnce(&OffThread) + Send + 'static + use<>,
        impl FnOnce(&OffThread) + Send + 'static + use<>,
    ) {
        drop(self.watchdog.take());
        let export = self.requests.close();
        crate::win::work::spawn(self.paste.close());
        if let Some(tray) = &mut self.tray {
            crate::win::work::spawn(tray.close());
        }
        let fixes = crate::win::work::spawn(self.fixes.close());
        let elevated = crate::win::work::spawn(self.elevated.close());
        let source_started = Instant::now();
        let (source_finished, source_completion) = std::sync::mpsc::sync_channel(1);
        let drops = self.drops.close();
        crate::win::work::spawn(move |worker| {
            drops(worker);
            let _ = source_finished.try_send(());
        });
        let mut retired = self.programs.close();
        retired.append(self.folders.close());
        retired.append(self.indexed.close());
        retired.append(self.lists.close());
        retired.append(self.panels.close());
        retired.append(self.clipboard.close());
        retired.append(self.shell.close());
        retired.append(self.operations.close());
        let (settings_retired, settings) = self.settings.close();
        retired.append(settings_retired);
        let (queue_retired, queue) = self.queue.close();
        retired.append(queue_retired);
        for task in &mut self.releases {
            task.cancel();
            retired.append(task.close_delivery());
        }
        self.retire(retired);
        self.retire_groups();
        let retiring = self.retiring.take();
        let diagnostics = self.diagnostics.take();
        crate::win::work::spawn(move |worker| {
            if let Some(diagnostics) = diagnostics {
                diagnostics.finish(worker);
            }
        });
        crate::win::work::spawn(move |worker| {
            if let Some(retiring) = retiring {
                retiring.finish(worker);
            }
        });
        let persist = move |worker: &OffThread| {
            settings(worker);
            queue(worker);
        };
        let cleanup = move |worker: &OffThread| {
            export(worker);
            let _ = fixes.join();
            let _ = elevated.join();
            // Other cleanup consumes this same grace; timing out releases only
            // the shutdown wait, not an uninterruptible native COM call.
            let left = crate::core::work::source_completion_wait(source_started.elapsed());
            let _ = source_completion.recv_timeout(left);
        };
        (persist, cleanup)
    }
}

fn write_diagnostics(messages: Diagnostics, _worker: &OffThread, _: &CancelToken) {
    for (path, text) in messages {
        crate::app::write_log(&path, &text);
    }
}

pub(super) fn finish_shutdown(
    worker: &OffThread,
    persist: impl FnOnce(&OffThread),
    recovery: Option<std::sync::mpsc::Receiver<io::Result<()>>>,
    cleanup: std::thread::JoinHandle<()>,
    error_log: &Path,
) {
    persist(worker);
    if let Some(finished) = recovery {
        match finished.recv() {
            Ok(Ok(())) => {}
            result => crate::app::write_log(
                error_log,
                &format!("File operation recovery state was not saved at Exit: {result:?}"),
            ),
        }
    }
    let _ = cleanup.join();
}

fn check_release(purpose: ReleasePurpose, _worker: &OffThread, _: &CancelToken) -> ReleaseAnswer {
    let reply = crate::win::update::latest_release();
    match purpose {
        ReleasePurpose::Update => ReleaseAnswer::Newer(reply.and_then(|reply| {
            crate::core::update::newer_release(&reply, gozler_core::escompat::version())
        })),
        ReleasePurpose::ServiceVersion => ReleaseAnswer::Older(
            reply
                .and_then(|reply| {
                    crate::core::update::latest_is_older(&reply, gozler_core::escompat::version())
                })
                .unwrap_or(false),
        ),
    }
}

fn retire(retired: Retirement, _cancel: &CancelToken) {
    drop(retired);
}
