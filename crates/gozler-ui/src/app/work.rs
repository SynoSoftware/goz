//! Routes UI work to bounded native workers, apart from Queue's engine lifetime.

use super::dialogs::TransferStart;
use crate::core::clipboard::{ClipboardData, ClipboardFormat, Publication, folder_of};
use crate::core::fetch::{List, LocalService};
use crate::core::model::ScopeCheck;
use crate::core::open::{self, Commands, Entries, EntryAction, Selected};
use crate::core::operations::{Followed, Following, PreparedFailure, Reporting};
use crate::core::panel::{
    Calculated, Calculation, CommandPlace, Counted, Counting, Finding, Found, Side,
};
use crate::core::search_list::{Keep, Kept};
use crate::core::selection::{Filter, Filtered, Telling, Told};
use crate::core::settings::{Changes, Settings};
use crate::core::work::{Id, Replacement, Retirement};
use crate::win::clipboard::{self, ClipboardBlock, CopyAttempt};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use gozler_core::proto::ResultItem;
use gozler_fileops::Taken;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

pub(super) mod drops;
pub(super) mod elevated;
pub(super) mod fixes;
pub(super) mod folders;
pub(super) mod indexed;
mod local_service;
pub(super) mod paste;
pub(super) mod programs;
mod queue;
pub(super) mod requests;
pub(super) mod tray;
pub(super) mod viewers;
pub(super) use queue::{
    Accepted as QueueAccepted, CheckPurpose, Client as QueueClient, Ready as QueueReady,
};

struct Save {
    path: PathBuf,
    changes: Changes,
    report_failure: bool,
}

struct Follow {
    input: Following,
    numbers: crate::win::locale::WindowsNumbers,
}

struct Report {
    input: Reporting,
    numbers: crate::win::locale::WindowsNumbers,
}

pub(super) const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

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
pub(super) enum ShellTarget {
    Repository,
    Services,
}

impl ShellTarget {
    pub(super) fn address(self) -> &'static str {
        match self {
            Self::Repository => REPOSITORY,
            Self::Services => "services.msc",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ShellPurpose {
    Target(ShellTarget),
    Entries(List, EntryAction),
    Setup,
}

enum Invocation {
    Target(ShellTarget),
    Setup(PathBuf),
    Entries {
        list: List,
        action: EntryAction,
        entries: Entries,
        commands: Commands,
    },
}

impl Invocation {
    fn items(&self) -> usize {
        match self {
            Self::Entries { entries, .. } => entries.items(),
            Self::Target(_) | Self::Setup(_) => 0,
        }
    }

    fn purpose(&self) -> ShellPurpose {
        match self {
            Self::Target(target) => ShellPurpose::Target(*target),
            Self::Setup(_) => ShellPurpose::Setup,
            Self::Entries { list, action, .. } => ShellPurpose::Entries(*list, *action),
        }
    }
}

pub(super) enum OpenTarget {
    Command(String),
    Path(Vec<u16>),
}

pub(super) struct OpenFailure {
    pub(super) target: OpenTarget,
    pub(super) error: io::Error,
}

#[derive(Default)]
pub(super) struct Opened {
    pub(super) failure: Option<OpenFailure>,
    pub(super) missing: Option<Vec<u16>>,
}

pub(super) struct Saved {
    pub(super) report_failure: bool,
    pub(super) result: io::Result<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PathPurpose {
    Copy,
    Move,
    PanelCopy,
    PanelMove,
    Delete,
}

pub(super) enum PathAction {
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
    fn purpose(&self) -> PathPurpose {
        match self {
            Self::Transfer { moves: false } => PathPurpose::Copy,
            Self::Transfer { moves: true } => PathPurpose::Move,
            Self::PanelTransfer { moves: false, .. } => PathPurpose::PanelCopy,
            Self::PanelTransfer { moves: true, .. } => PathPurpose::PanelMove,
            Self::Delete { .. } => PathPurpose::Delete,
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

pub(super) struct PreparedPaths {
    pub(super) action: PathAction,
    pub(super) paths: Vec<PathBuf>,
}

enum Registry {
    Read,
    SignIn { on: bool, log: PathBuf },
}

pub(super) enum RegistryAnswer {
    Read(bool),
    SignIn(io::Result<()>),
}

struct Copy {
    data: CopyData,
    publication: Publication,
    preceding: Arc<AtomicU32>,
}

enum CopyData {
    Rows(Arc<[ResultItem]>, ClipboardFormat),
    Block(ClipboardBlock),
}

impl Copy {
    fn items(&self) -> usize {
        match &self.data {
            CopyData::Rows(files, _) => files.len(),
            CopyData::Block(block) => block.items(),
        }
    }
}

enum Copied {
    Finished,
    Busy(Copy),
    Failed(windows::core::Error),
}

struct CopyRetry {
    id: Id,
    copy: Copy,
    due: Instant,
}

pub(super) enum CopyFailure {
    Busy,
    Native(windows::core::Error),
}

pub(super) enum CopyNotice {
    Retry,
    Failed(Id, CopyFailure),
    Finished,
}

struct QueueStart {
    path: PathBuf,
    waker: OpenWaker,
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
    releases: [Task<ReleasePurpose, ReleaseAnswer>; 2],
    location: Task<String, ScopeCheck>,
    checked_location: Option<String>,
    scope: ScopeCheck,
    saving: Option<Task<Save, Saved>>,
    pairing: Task<LocalService, (LocalService, bool)>,
    labels: Task<Vec<String>, Vec<(String, Option<String>)>>,
    label_prefixes: Vec<String>,
    volume_labels: Vec<(String, Option<String>)>,
    shell: Vec<(ShellPurpose, Task<Invocation, Opened>)>,
    registry: Option<Task<Registry, RegistryAnswer>>,
    registry_write: Option<Id>,
    copying: Task<Copy, Copied>,
    copy_retry: Option<CopyRetry>,
    last_publication: Arc<AtomicU32>,
    queue: Task<QueueStart, Option<QueueReady>>,
    queue_session: Option<Task<queue::Session, ()>>,
    queue_waker: OpenWaker,
    retiring: Option<Task<Retirement, ()>>,
    lists: Vec<ListWork>,
    panels: [Task<Calculation, Calculated>; 2],
    panel_counts: [Task<Counting, Counted>; 2],
    panel_quick: [Task<Finding, Found>; 2],
    following: Task<Follow, Followed>,
    reporting: Task<Report, PreparedFailure>,
    paths: Vec<(PathPurpose, Task<PreparingPaths, PreparedPaths>)>,
}

struct ListWork {
    list: List,
    telling: Task<Telling, Told>,
    keeping: Task<Keep, Kept>,
    filtering: Task<Filter, Filtered>,
}

impl Work {
    pub(super) fn new(waker: OpenWaker, error_log: PathBuf) -> Self {
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
            releases: [
                Task::new(waker.clone(), Replacement::Cancel, check_release).background(),
                Task::new(waker.clone(), Replacement::Cancel, check_release),
            ],
            location: Task::new(waker.clone(), Replacement::Cancel, check_location),
            checked_location: None,
            scope: ScopeCheck::Exists,
            saving: Some(Task::new(waker.clone(), Replacement::Finish, save)),
            pairing: Task::new(waker.clone(), Replacement::Cancel, check_pairing),
            labels: Task::new(waker.clone(), Replacement::Cancel, read_labels).background(),
            label_prefixes: Vec::new(),
            volume_labels: Vec::new(),
            registry: Some(Task::new(waker.clone(), Replacement::Finish, registry)),
            registry_write: None,
            copying: Task::new(waker.clone(), Replacement::Cancel, copy).with_size(Copy::items),
            copy_retry: None,
            last_publication: Arc::new(AtomicU32::new(0)),
            queue: Task::new(waker.clone(), Replacement::Cancel, restore_queue)
                .with_size(|_| crate::core::work::INLINE_ITEMS + 1),
            queue_session: Some(Task::new(
                waker.clone(),
                Replacement::Finish,
                |session: queue::Session, worker, _| session.run(worker),
            )),
            queue_waker: waker.for_message(crate::win::file_operations::QUEUE_CHANGED),
            retiring: Some(Task::pass(
                waker.clone(),
                Replacement::Finish,
                Retirement::items,
                retire,
            )),
            lists: List::ALL
                .into_iter()
                .map(|list| ListWork {
                    list,
                    telling: Task::pass(
                        waker.clone(),
                        Replacement::Finish,
                        Telling::items,
                        |input, _| input.run(),
                    ),
                    keeping: Task::pass(
                        waker.clone(),
                        Replacement::Finish,
                        Keep::items,
                        |input, _| input.run(),
                    ),
                    filtering: Task::pass(
                        waker.clone(),
                        Replacement::Cancel,
                        Filter::retained_items,
                        |input, _| input.run(),
                    ),
                })
                .collect(),
            panels: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Cancel,
                    Calculation::items,
                    |input, _| input.run(),
                )
            }),
            panel_counts: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Cancel,
                    Counting::items,
                    |input, _| input.run(),
                )
            }),
            panel_quick: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Finish,
                    Finding::items,
                    |input, _| input.run(),
                )
            }),
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
            paths: [
                PathPurpose::Copy,
                PathPurpose::Move,
                PathPurpose::PanelCopy,
                PathPurpose::PanelMove,
                PathPurpose::Delete,
            ]
            .into_iter()
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
            shell: List::ALL
                .into_iter()
                .flat_map(|list| EntryAction::ALL.map(|action| ShellPurpose::Entries(list, action)))
                .chain([ShellTarget::Repository, ShellTarget::Services].map(ShellPurpose::Target))
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
        }
    }

    /// Searches use the last observation; typing in an unchanged Location never
    /// reads the disk. Until its first answer, the service can still search it.
    pub(super) fn scope(&mut self, folder: Option<&str>) -> ScopeCheck {
        if self.checked_location.as_deref() != folder {
            self.scope = ScopeCheck::Exists;
            self.checked_location = folder.map(str::to_owned);
            match folder {
                Some(folder) => {
                    self.location.submit(folder.to_owned(), |_, new| new);
                }
                None => self.location.cancel(),
            }
        }
        self.scope
    }

    pub(super) fn refresh_location(&mut self) {
        if let Some(folder) = &self.checked_location {
            self.location.submit(folder.clone(), |_, new| new);
        }
    }

    pub(super) fn take_location(&mut self) -> bool {
        let Some((_, scope)) = self.location.take() else {
            return false;
        };
        let changed = self.scope != scope;
        self.scope = scope;
        changed
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = self.location.tick(now).is_some();
        active |= self.pairing.tick(now).is_some();
        active |= self.labels.tick(now).is_some();
        active |= self.copying.tick(now).is_some();
        active |= self.queue.tick(now).is_some();
        active |= self.following.tick(now).is_some();
        active |= self.reporting.tick(now).is_some();
        active |= self.folders.tick(now);
        active |= self.fixes.tick(now);
        active |= self.elevated.tick(now);
        active |= self.indexed.tick(now);
        for task in &mut self.releases {
            active |= task.tick(now).is_some();
        }
        if let Some(task) = &mut self.saving {
            active |= task.tick(now).is_some();
        }
        if let Some(task) = &mut self.registry {
            active |= task.tick(now).is_some();
        }
        for (_, task) in &mut self.shell {
            active |= task.tick(now).is_some();
        }
        for (_, task) in &mut self.paths {
            active |= task.tick(now).is_some();
        }
        for task in &mut self.panels {
            active |= task.tick(now).is_some();
        }
        for task in &mut self.panel_counts {
            active |= task.tick(now).is_some();
        }
        for task in &mut self.panel_quick {
            active |= task.tick(now).is_some();
        }
        for list in &mut self.lists {
            active |= list.telling.tick(now).is_some();
            active |= list.keeping.tick(now).is_some();
            active |= list.filtering.tick(now).is_some();
        }
        active
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

    pub(super) fn save(&mut self, path: &Path, changes: Changes, report_failure: bool) {
        self.saving
            .as_mut()
            .expect("saving is open until destruction")
            .submit(
                Save {
                    path: path.to_owned(),
                    changes,
                    report_failure,
                },
                |old, mut new| {
                    new.changes = old.changes.combine(new.changes);
                    new.report_failure |= old.report_failure;
                    new
                },
            );
    }

    pub(super) fn take_saved(&mut self) -> Option<Saved> {
        self.saving.as_mut()?.take().map(|(_, saved)| saved)
    }

    pub(super) fn check_pairing(&mut self, local: LocalService) {
        self.pairing.submit(local, |_, new| new);
    }

    pub(super) fn cancel_pairing(&mut self) {
        self.pairing.cancel();
    }

    pub(super) fn take_pairing(&mut self) -> Option<(Id, LocalService, bool)> {
        self.pairing
            .take()
            .map(|(id, (local, paired))| (id, local, paired))
    }

    pub(super) fn pairing_is_current(&self, id: Id) -> bool {
        self.pairing.is_current(id)
    }

    pub(super) fn labels(&mut self, prefixes: Vec<String>) -> Vec<(String, Option<String>)> {
        if prefixes != self.label_prefixes {
            self.label_prefixes = prefixes;
            self.labels
                .submit(self.label_prefixes.clone(), |_, new| new);
            self.volume_labels
                .retain(|(prefix, _)| self.label_prefixes.contains(prefix));
        }
        self.volume_labels.clone()
    }

    pub(super) fn refresh_labels(&mut self) {
        // A timer must not keep cancelling an unchanged read before it answers.
        if !self.label_prefixes.is_empty() && !self.labels.is_running() {
            self.labels
                .submit(self.label_prefixes.clone(), |_, new| new);
        }
    }

    pub(super) fn take_labels(&mut self) -> bool {
        let Some((_, labels)) = self.labels.take() else {
            return false;
        };
        let changed = labels != self.volume_labels;
        self.volume_labels = labels;
        changed
    }

    pub(super) fn open(&mut self, target: ShellTarget) {
        self.invoke(Invocation::Target(target));
    }

    pub(super) fn start_setup(&mut self, path: PathBuf) {
        self.invoke(Invocation::Setup(path));
    }

    pub(super) fn open_entries(
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

    fn invoke(&mut self, invocation: Invocation) {
        let purpose = invocation.purpose();
        let task = &mut self
            .shell
            .iter_mut()
            .find(|(each, _)| *each == purpose)
            .expect("every shell purpose has a slot")
            .1;
        task.replace(invocation);
        let retired = task.take_retired();
        self.retire(retired);
    }

    pub(super) fn take_opened(&mut self) -> Vec<(ShellPurpose, Id, Opened)> {
        let mut retired = Retirement::default();
        let opened = self
            .shell
            .iter_mut()
            .filter_map(|(target, task)| {
                let opened = task.take();
                retired.append(task.take_retired());
                let (id, opened) = opened?;
                (*target == ShellPurpose::Setup
                    || opened.failure.is_some()
                    || opened.missing.is_some())
                .then_some((*target, id, opened))
            })
            .collect();
        self.retire(retired);
        opened
    }

    pub(super) fn opening_is_current(&self, purpose: ShellPurpose, id: Id) -> bool {
        self.shell
            .iter()
            .any(|(each, task)| *each == purpose && task.is_current(id))
    }

    pub(super) fn read_registry(&mut self) {
        self.registry
            .as_mut()
            .expect("registry is open until destruction")
            .submit(Registry::Read, |_, new| new);
    }

    pub(super) fn registry_is_current(&self, id: Id) -> bool {
        self.registry
            .as_ref()
            .is_some_and(|task| task.is_current(id))
    }

    pub(super) fn set_sign_in(&mut self, on: bool, log: PathBuf) -> Id {
        assert!(
            self.registry_write.is_none(),
            "Settings awaits its accepted write"
        );
        let id = self
            .registry
            .as_mut()
            .expect("registry is open until destruction")
            .submit(Registry::SignIn { on, log }, |_, new| new);
        self.registry_write = Some(id);
        id
    }

    pub(super) fn take_registry(&mut self) -> Option<(Id, RegistryAnswer)> {
        let answer = self.registry.as_mut()?.take()?;
        if self.registry_write == Some(answer.0) {
            self.registry_write = None;
        }
        Some(answer)
    }

    pub(super) fn copy(&mut self, files: Arc<[ResultItem]>, format: ClipboardFormat) {
        self.clear_copy_retry();
        self.copying.replace(Copy {
            data: CopyData::Rows(files, format),
            publication: Publication::new(clipboard::sequence()),
            preceding: Arc::clone(&self.last_publication),
        });
        self.retire_copies();
    }

    pub(super) fn cancel_copy(&mut self) {
        self.clear_copy_retry();
        self.copying.cancel();
        self.retire_copies();
    }

    pub(super) fn retry_copy(&mut self) -> Option<Duration> {
        let retry = self.copy_retry.as_ref()?;
        if !self.copying.is_current(retry.id) {
            self.clear_copy_retry();
            return None;
        }
        let left = retry.due.saturating_duration_since(Instant::now());
        if !left.is_zero() {
            return Some(left);
        }
        let retry = self.copy_retry.take().expect("the current retry is due");
        self.copying.replace(retry.copy);
        self.retire_copies();
        None
    }

    pub(super) fn take_copied(&mut self) -> Option<CopyNotice> {
        let answer = self.copying.take();
        self.retire_copies();
        let (id, answer) = answer?;
        Some(match answer {
            Copied::Finished => CopyNotice::Finished,
            Copied::Failed(error) => CopyNotice::Failed(id, CopyFailure::Native(error)),
            Copied::Busy(mut copy) => {
                if copy.publication.retry() {
                    self.copy_retry = Some(CopyRetry {
                        id,
                        copy,
                        due: Instant::now() + crate::core::clipboard::RETRY_AFTER,
                    });
                    CopyNotice::Retry
                } else {
                    self.retire_copy(copy);
                    CopyNotice::Failed(id, CopyFailure::Busy)
                }
            }
        })
    }

    pub(super) fn copy_is_current(&self, id: Id) -> bool {
        self.copying.is_current(id)
    }

    fn clear_copy_retry(&mut self) {
        if let Some(retry) = self.copy_retry.take() {
            self.retire_copy(retry.copy);
        }
    }

    fn retire_copy(&mut self, copy: Copy) {
        let items = copy.items();
        let mut retired = Retirement::default();
        retired.push(copy, items);
        self.retire(retired);
    }

    fn retire_copies(&mut self) {
        let retired = self.copying.take_retired();
        self.retire(retired);
    }

    pub(super) fn retire_taken(&mut self, taken: Arc<Taken>) {
        let items = taken.len();
        let mut retired = Retirement::default();
        retired.push(taken, items);
        self.retire(retired);
    }

    pub(super) fn retire(&mut self, retired: Retirement) {
        // A reentrant handler may unwind after destruction transferred the
        // worker to finalization; there is no visible UI left to protect.
        let Some(task) = self.retiring.as_mut() else {
            return;
        };
        let _ = task.take();
        if !retired.is_empty() {
            task.submit(retired, Retirement::combine);
        }
    }

    pub(super) fn restore_queue(&mut self, path: PathBuf) {
        self.queue.submit(
            QueueStart {
                path,
                waker: self.queue_waker.clone(),
            },
            |_, new| new,
        );
    }

    pub(super) fn take_queue(&mut self) -> Option<QueueReady> {
        self.queue.take().and_then(|(_, queue)| queue)
    }

    pub(super) fn open_queue(&mut self, ready: QueueReady) -> QueueClient {
        let (client, session) = ready.adopt();
        self.queue_session
            .as_mut()
            .expect("the work owner remains open during queue adoption")
            .replace(session);
        client
    }

    pub(super) fn keep(&mut self, list: List, input: Keep) {
        let task = self
            .lists
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        task.keeping.replace(input);
        let retired = task.keeping.take_retired();
        self.retire(retired);
    }

    pub(super) fn tell(&mut self, list: List, input: Telling) {
        let work = self
            .lists
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        work.telling.replace(input);
        let retired = work.telling.take_retired();
        self.retire(retired);
    }

    pub(super) fn take_told(&mut self) -> Vec<(List, Told)> {
        let mut answers = Vec::new();
        let mut retired = Retirement::default();
        for work in &mut self.lists {
            if let Some((_, answer)) = work.telling.take() {
                answers.push((work.list, answer));
            }
            retired.append(work.telling.take_retired());
        }
        self.retire(retired);
        answers
    }

    pub(super) fn filter(&mut self, list: List, input: Filter) {
        let task = self
            .lists
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        task.filtering.replace(input);
        let retired = task.filtering.take_retired();
        self.retire(retired);
    }

    pub(super) fn calculate_panel(&mut self, side: Side, input: Calculation) {
        let task = &mut self.panels[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retire(retired);
    }

    pub(super) fn take_panels(&mut self) -> Vec<(Side, Calculated)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.panels[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retire(retired);
        answers
    }

    pub(super) fn count_panel(&mut self, side: Side, input: Counting) {
        let task = &mut self.panel_counts[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retire(retired);
    }

    pub(super) fn find_quick(&mut self, side: Side, input: Finding) {
        let task = &mut self.panel_quick[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retire(retired);
    }

    pub(super) fn follow(&mut self, input: Following, numbers: crate::win::locale::WindowsNumbers) {
        self.following.replace(Follow { input, numbers });
        let retired = self.following.take_retired();
        self.retire(retired);
    }

    pub(super) fn take_followed(&mut self) -> Option<Followed> {
        let answer = self.following.take().map(|(_, answer)| answer);
        let retired = self.following.take_retired();
        self.retire(retired);
        answer
    }

    pub(super) fn prepare_report(
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
        self.retire(retired);
    }

    pub(super) fn preparing_report(&self) -> bool {
        self.reporting.is_running()
    }

    pub(super) fn take_report(&mut self) -> Option<PreparedFailure> {
        let answer = self.reporting.take().map(|(_, answer)| answer);
        let retired = self.reporting.take_retired();
        self.retire(retired);
        answer
    }

    pub(super) fn take_quick(&mut self) -> Vec<(Side, Found)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.panel_quick[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retire(retired);
        answers
    }

    pub(super) fn prepare_paths(&mut self, action: PathAction, entries: Entries) {
        let purpose = action.purpose();
        let task = &mut self
            .paths
            .iter_mut()
            .find(|(each, _)| *each == purpose)
            .expect("each path command has a slot")
            .1;
        task.replace(PreparingPaths { action, entries });
        let retired = task.take_retired();
        self.retire(retired);
    }

    pub(super) fn paths_are_current(&self, purpose: PathPurpose, id: Id) -> bool {
        self.paths
            .iter()
            .find(|(each, _)| *each == purpose)
            .is_some_and(|(_, task)| task.is_current(id))
    }

    pub(super) fn take_paths(&mut self) -> Vec<(PathPurpose, Id, PreparedPaths)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for (purpose, task) in &mut self.paths {
            if let Some((id, answer)) = task.take() {
                answers.push((*purpose, id, answer));
            }
            retired.append(task.take_retired());
        }
        self.retire(retired);
        answers
    }

    pub(super) fn take_panel_counts(&mut self) -> Vec<(Side, Counted)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.panel_counts[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retire(retired);
        answers
    }

    pub(super) fn take_kept(&mut self) -> Vec<(List, Kept)> {
        let mut retired = Retirement::default();
        let answers = self
            .lists
            .iter_mut()
            .filter_map(|work| {
                let answer = work.keeping.take().map(|(_, answer)| (work.list, answer));
                retired.append(work.keeping.take_retired());
                answer
            })
            .collect();
        self.retire(retired);
        answers
    }

    pub(super) fn take_filtered(&mut self) -> Vec<(List, Filtered)> {
        let mut retired = Retirement::default();
        let answers = self
            .lists
            .iter_mut()
            .filter_map(|work| {
                let answer = work.filtering.take().map(|(_, answer)| (work.list, answer));
                retired.append(work.filtering.take_retired());
                answer
            })
            .collect();
        self.retire(retired);
        answers
    }

    /// Returns independent persistence and accepted-mutation cleanup. Neither
    /// closure may run on UI, and cleanup must not delay essential writes.
    pub(super) fn close(
        &mut self,
    ) -> (
        impl FnOnce(&OffThread) + Send + 'static,
        impl FnOnce(&OffThread) + Send + 'static,
    ) {
        let export = self.requests.close();
        let paste = self.paste.close();
        crate::win::work::spawn(paste);
        if let Some(tray) = &mut self.tray {
            crate::win::work::spawn(tray.close());
        }
        let fixes = crate::win::work::spawn(self.fixes.close());
        let elevated = crate::win::work::spawn(self.elevated.close());
        let drops = self.drops.close();
        crate::win::work::spawn(move |worker| drops(worker));
        self.location.cancel();
        self.pairing.cancel();
        self.labels.cancel();
        self.cancel_copy();
        self.queue.cancel();
        self.following.cancel();
        self.reporting.cancel();
        let mut retired = Retirement::default();
        retired.append(self.programs.close());
        retired.append(self.folders.close());
        retired.append(self.indexed.close());
        for task in &mut self.releases {
            task.cancel();
            retired.append(task.close_delivery());
        }
        retired.append(self.location.close_delivery());
        retired.append(self.pairing.close_delivery());
        retired.append(self.labels.close_delivery());
        retired.append(self.copying.close_delivery());
        retired.append(self.queue.close_delivery());
        retired.append(self.following.close_delivery());
        retired.append(self.reporting.close_delivery());
        for task in &mut self.panels {
            task.cancel();
            retired.append(task.close_delivery());
        }
        for task in &mut self.panel_counts {
            task.cancel();
            retired.append(task.close_delivery());
        }
        for task in &mut self.panel_quick {
            task.cancel();
            retired.append(task.close_delivery());
        }
        for (_, task) in &mut self.paths {
            task.cancel();
            retired.append(task.close_delivery());
        }
        for work in &mut self.lists {
            work.telling.cancel();
            retired.append(work.telling.close_delivery());
            work.keeping.cancel();
            work.filtering.cancel();
            retired.append(work.keeping.close_delivery());
            retired.append(work.filtering.close_delivery());
        }
        for (_, task) in &mut self.shell {
            task.cancel();
            retired.append(task.close_delivery());
        }
        self.retire(retired);
        let saving = self.saving.take();
        let retiring = self.retiring.take();
        crate::win::work::spawn(move |worker| {
            if let Some(retiring) = retiring {
                retiring.finish(worker);
            }
        });
        let registry = self.registry.take();
        let registry_write = self.registry_write.take();
        let registry = registry_write.and(registry);
        let queue_session = self.queue_session.take();
        let persist = move |worker: &OffThread| {
            if let Some(saving) = saving {
                saving.finish(worker);
            }
            if let Some(registry) = registry {
                registry.finish_current(worker);
            }
            if let Some(session) = queue_session {
                session.finish(worker);
            }
        };
        let cleanup = move |worker: &OffThread| {
            export(worker);
            let _ = fixes.join();
            let _ = elevated.join();
        };
        (persist, cleanup)
    }
}

pub(super) fn load_settings(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .map_or_else(|_| Settings::default(), |text| Settings::parse(&text))
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

pub(crate) fn write_settings(path: &Path, changes: Changes, _worker: &OffThread) -> io::Result<()> {
    let text = std::fs::read_to_string(path).ok();
    let text = changes.apply(text.as_deref());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

fn save(save: Save, worker: &OffThread, _cancel: &CancelToken) -> Saved {
    let result = write_settings(&save.path, save.changes, worker);
    if let Err(error) = &result {
        // The UI may close after accepting this reply but before showing its
        // deferred error; preserve the failure without depending on that HWND.
        super::write_log(
            &save.path.with_file_name("ui-error.log"),
            &format!("Settings were not saved: {error}"),
        );
    }
    Saved {
        result,
        report_failure: save.report_failure,
    }
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

fn check_location(folder: String, _worker: &OffThread, cancel: &CancelToken) -> ScopeCheck {
    if !cancel.is_cancelled() && gozler_client::folder_is_missing(&folder) {
        ScopeCheck::Missing
    } else {
        ScopeCheck::Exists
    }
}

fn registry(input: Registry, worker: &OffThread, _cancel: &CancelToken) -> RegistryAnswer {
    match input {
        Registry::Read => RegistryAnswer::Read(crate::win::shell::starts_at_sign_in(worker)),
        Registry::SignIn { on, log } => {
            let result = std::env::current_exe()
                .and_then(|program| crate::win::shell::set_start_at_sign_in(on, &program, worker));
            if let Err(error) = &result {
                super::write_log(&log, &format!("Start at sign-in was not saved: {error}"));
            }
            RegistryAnswer::SignIn(result)
        }
    }
}

fn copy(copy: Copy, worker: &OffThread, cancel: &CancelToken) -> Copied {
    if cancel.is_cancelled() {
        return Copied::Finished;
    }
    let block = match copy.data {
        CopyData::Rows(files, format) => {
            let Some(block) = ClipboardBlock::new(&ClipboardData::new(format, &files)) else {
                return Copied::Finished;
            };
            block
        }
        CopyData::Block(block) => block,
    };
    match block.set(&copy.publication, &copy.preceding, cancel, worker) {
        CopyAttempt::Published | CopyAttempt::Superseded => Copied::Finished,
        CopyAttempt::Busy(block) => Copied::Busy(Copy {
            data: CopyData::Block(block),
            publication: copy.publication,
            preceding: copy.preceding,
        }),
        CopyAttempt::Failed(error) => Copied::Failed(error),
    }
}

fn check_pairing(
    local: LocalService,
    _worker: &OffThread,
    cancel: &CancelToken,
) -> (LocalService, bool) {
    let paired = !cancel.is_cancelled() && local_service::is_paired(&local.program);
    (local, paired)
}

fn read_labels(
    prefixes: Vec<String>,
    worker: &OffThread,
    cancel: &CancelToken,
) -> Vec<(String, Option<String>)> {
    prefixes
        .into_iter()
        .take_while(|_| !cancel.is_cancelled())
        .map(|prefix| {
            let label = crate::win::shell::volume_label(&prefix, worker);
            (prefix, label)
        })
        .collect()
}

fn retire(retired: Retirement, _cancel: &CancelToken) {
    drop(retired);
}

fn prepare_paths(input: PreparingPaths, cancel: &CancelToken) -> PreparedPaths {
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

fn restore_queue(
    start: QueueStart,
    worker: &OffThread,
    cancel: &CancelToken,
) -> Option<QueueReady> {
    if cancel.is_cancelled() {
        return None;
    }
    Some(queue::restore(start.path, start.waker, worker))
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
            missing: None,
        },
        Invocation::Target(target) => Opened {
            failure: session
                .open(target.address())
                .err()
                .map(|error| OpenFailure {
                    target: OpenTarget::Path(target.address().encode_utf16().collect()),
                    error,
                }),
            missing: None,
        },
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
        let command: &str = match action {
            EntryAction::Open if entry.is_folder => &commands.folders,
            EntryAction::Open => &commands.files,
            EntryAction::OpenFileLocation => &commands.location,
            EntryAction::Explore | EntryAction::Properties => "",
        };
        if let Some(line) = open::expand(command, &String::from_utf16_lossy(&entry.path)) {
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
