//! File operations' presentation decisions (DESIGN.md: File operations): which
//! operation the card shows, every text of the card and of the queue list, and
//! the speed, which the window works out from the counts it polls, because the
//! engine reports counts only.
//!
//! The engine's queue owns the operations, their order and which ones wait;
//! this module only words what the queue reports.

pub(super) mod batch;
mod layout;
#[cfg(windows)]
pub(crate) use layout::{CARD_ICON, Layout, Metrics, ROW_ICON};

use super::captions::{Guard, Question, QuestionBody};
use super::commands::{Mark, MenuNode};
use super::glyph::Glyph;
use super::layout::Px;
use super::panel::same_folder;
use super::text::{Numbers, Strings, bytes, name_of, without_access_key};
use gozler_fileops::{
    Destination, End, Listed, NoRoom, Operation, OperationId, Outcome, Reason, Refusal, Request,
    RingCounts, Running, SourcesFolder, State, Total,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn policy_name(strings: &Strings, policy: gozler_fileops::Policy) -> &str {
    use gozler_fileops::Policy;
    strings.text(match policy {
        Policy::UpdateChanged => "copy_policy.update_changed",
        Policy::UpdateNewer => "copy_policy.update_newer",
        Policy::KeepBoth => "copy_policy.keep_both",
        Policy::Skip => "copy_policy.skip",
        Policy::Replace => "file_operations.replace",
    })
}

pub(crate) fn policy_names(strings: &Strings) -> [&str; 4] {
    gozler_fileops::Policy::ALL.map(|policy| policy_name(strings, policy))
}

/// How far back the speed looks: long enough that one slow file does not
/// make the speed jump, short enough that a change shows within seconds.
const SPEED_SPAN: Duration = Duration::from_secs(5);

/// How long the counts must run before a speed shows, because two polls
/// half a second apart give a speed that jumps.
const SPEED_START: Duration = Duration::from_secs(1);

/// The counts of one running operation as the window polled them, from
/// which it works out the speed.
#[derive(Debug, Default)]
pub(crate) struct Meter {
    samples: VecDeque<Sample>,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    items: u64,
    bytes: u64,
}

/// Items and bytes per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Speed {
    pub(crate) items: f64,
    pub(crate) bytes: f64,
}

impl Meter {
    /// Adds the counts polled `at`.
    pub(crate) fn record(&mut self, at: Instant, items: u64, bytes: u64) {
        self.samples.push_back(Sample { at, items, bytes });
        while let Some(first) = self.samples.front()
            && at.duration_since(first.at) > SPEED_SPAN
        {
            self.samples.pop_front();
        }
    }

    /// Forgets the counts, so a paused operation's speed starts again when
    /// it resumes instead of counting the pause.
    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    /// The speed over the last [`SPEED_SPAN`], once the counts have run for
    /// [`SPEED_START`].
    pub(crate) fn speed(&self) -> Option<Speed> {
        let (first, last) = (self.samples.front()?, self.samples.back()?);
        let span = last.at.duration_since(first.at);
        if span < SPEED_START {
            return None;
        }
        let seconds = span.as_secs_f64();
        Some(Speed {
            items: last.items.saturating_sub(first.items) as f64 / seconds,
            bytes: last.bytes.saturating_sub(first.bytes) as f64 / seconds,
        })
    }
}

/// A speed in bytes, as "5.10 MB/s".
pub(crate) fn bytes_per_second(strings: &Strings, numbers: &dyn Numbers, speed: f64) -> String {
    strings.format(
        "file_operations.per_second",
        &[&bytes(strings, numbers, speed as u64)],
    )
}

/// What the window shows: its title, the selected operation's card,
/// and the queue list's rows, in the queue's order.
#[derive(Debug, PartialEq)]
pub(crate) struct View {
    pub(crate) title: String,
    /// `None` when the selection is no longer available.
    pub(crate) card: Option<Card>,
    /// What the card says when its selection is unavailable.
    pub(crate) idle: String,
    pub(crate) rows: Vec<Row>,
    /// The header over the list, such as "Queue · 3 waiting".
    pub(crate) queue: String,
    pub(crate) report: Option<Report>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Detail {
    Progress(OperationId),
    Report(OperationId),
}

impl Detail {
    pub(crate) fn id(self) -> OperationId {
        match self {
            Detail::Progress(id) | Detail::Report(id) => id,
        }
    }
}

/// The immutable result backing the virtual report list; formatting one
/// requested row never copies or measures the other failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) id: OperationId,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) outcome: Arc<Outcome>,
}

pub(crate) fn is_transfer(request: &Request) -> bool {
    request.is_transfer()
}

/// The selected operation's progress, independently of the status target.
#[derive(Debug, PartialEq)]
pub(crate) struct Card {
    pub(crate) id: OperationId,
    pub(crate) glyph: Glyph,
    /// The operation's command.
    pub(crate) title: String,
    /// Its current phase; the rates have their own controls.
    pub(crate) status: String,
    /// Stops bar animation for manual Pause and priority admission alike.
    pub(crate) paused: bool,
    /// The current file's label.
    pub(crate) doing: String,
    /// The current file relative to [`Card::from`]; while there is none,
    /// the one selected entry, or empty for more.
    pub(crate) file: String,
    /// The current file's full source path, and its destination path.
    pub(crate) file_tip: String,
    pub(crate) from: String,
    /// `None` for a delete and a new folder.
    pub(crate) to: Option<String>,
    /// The current file's bar; `None` for a delete and a move by rename,
    /// which move no data.
    pub(crate) file_bar: Option<Bar>,
    pub(crate) file_bytes: String,
    pub(crate) total_bar: Bar,
    /// "3,812 of 31,233 files", or "6,240 deleted".
    pub(crate) items: String,
    /// "1.11 GB of 9.27 GB", or empty when no bytes move.
    pub(crate) bytes: String,
    pub(crate) phase: RowPhase,
    pub(crate) rate: String,
    pub(crate) files_rate: String,
}

/// `done` of `total`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Measure {
    pub(crate) done: u64,
    pub(crate) total: u64,
}

impl Measure {
    /// The whole percent done, rounded down, so 100 % shows only at the end.
    pub(crate) fn percent(self) -> u64 {
        if self.total == 0 {
            return 0;
        }
        u64::try_from(u128::from(self.done.min(self.total)) * 100 / u128::from(self.total))
            .expect("a percentage is at most 100")
    }
}

/// Progress whose total may not yet be known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bar {
    /// With its percentage beside it.
    Measured(Measure),
    /// The total is unknown, or combines incomparable units.
    Moving,
}

/// What the taskbar button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taskbar {
    None,
    Measured(Measure),
    /// Only deletes run, which have no total.
    Moving,
    /// Every running operation is paused.
    Paused(Measure),
}

/// One row of the queue list.
#[derive(Debug, PartialEq)]
pub(crate) struct Row {
    pub(crate) id: OperationId,
    pub(crate) glyph: Glyph,
    /// "Copy D:\Photos\2026 → V:\Backup".
    pub(crate) title: RowLine,
    /// "Waiting for the copy to V:\Backup · 4,210 files, 18.4 GB".
    pub(crate) detail: RowLine,
    pub(crate) phase: RowPhase,
}

/// A line of a row, whose paths lose their middle to fit the tile.
#[derive(Debug, PartialEq)]
pub(crate) enum RowLine {
    /// A line without a path, which the list cuts at its end.
    Plain(String),
    /// The text `key` filled with one path or two, then `after`.
    Paths {
        key: &'static str,
        paths: Vec<String>,
        after: String,
    },
}

impl RowLine {
    /// The line with its whole paths.
    pub(crate) fn whole(&self, strings: &Strings) -> String {
        match self {
            RowLine::Plain(text) => text.clone(),
            RowLine::Paths { paths, .. } => self.with(
                strings,
                &paths.iter().map(String::as_str).collect::<Vec<_>>(),
            ),
        }
    }

    /// The line in `width`, as `width_of` measures text: each path that
    /// does not fit loses its middle, as `shorten` shortens a path to a
    /// width, so its start and its last folder stay. Of two paths, one
    /// shorter than half the room stays whole, and the other gets the rest.
    pub(crate) fn fitted(
        &self,
        strings: &Strings,
        width: Px,
        width_of: &dyn Fn(&str) -> Px,
        shorten: &dyn Fn(&str, Px) -> String,
    ) -> String {
        let whole = self.whole(strings);
        let RowLine::Paths { paths, .. } = self else {
            return whole;
        };
        if width_of(&whole) <= width {
            return whole;
        }
        let room = width - width_of(&self.with(strings, &vec![""; paths.len()]));
        let rooms = match paths.as_slice() {
            [_] => vec![room],
            [first, second] => {
                let half = Px(room.0 / 2);
                let (first, second) = (width_of(first), width_of(second));
                if first <= half {
                    vec![first, room - first]
                } else if second <= half {
                    vec![room - second, second]
                } else {
                    vec![half, room - half]
                }
            }
            _ => return whole,
        };
        let shortened: Vec<String> = paths
            .iter()
            .zip(rooms)
            .map(|(path, room)| shorten(path, room))
            .collect();
        self.with(
            strings,
            &shortened.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }

    /// The line with `paths` in place of its own.
    fn with(&self, strings: &Strings, paths: &[&str]) -> String {
        match self {
            RowLine::Plain(text) => text.clone(),
            RowLine::Paths { key, after, .. } => strings.format(key, paths) + after,
        }
    }
}

/// What a row's operation is doing, which decides what its menu and the
/// queue's buttons offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowPhase {
    Running {
        /// The person's Pause, independently of priority admission.
        paused: bool,
    },
    /// Accepted work that has not started or yielded to another transfer.
    Waiting,
    /// Preparing, or prepared and waiting for the person's answer.
    Starting,
    Interrupted,
    Completed {
        report: bool,
    },
    Cancelling,
}

/// What the person can do to a row's operation, from its menu, the queue's
/// buttons or the `Delete` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueAction {
    Pause,
    Resume,
    Cancel,
    MoveUp,
    MoveDown,
    Continue,
    Dismiss,
    Report,
    Clear,
}

impl QueueAction {
    pub(crate) const ALL: [QueueAction; 9] = [
        QueueAction::Pause,
        QueueAction::Resume,
        QueueAction::Cancel,
        QueueAction::MoveUp,
        QueueAction::MoveDown,
        QueueAction::Continue,
        QueueAction::Dismiss,
        QueueAction::Report,
        QueueAction::Clear,
    ];

    /// Its ID in the row's context menu, which is never 0.
    pub(crate) fn id(self) -> u16 {
        let index = QueueAction::ALL
            .iter()
            .position(|action| *action == self)
            .expect("every action is in ALL");
        u16::try_from(index).expect("ALL is short") + 1
    }

    pub(crate) fn from_id(id: u16) -> Option<QueueAction> {
        QueueAction::ALL
            .get(usize::from(id).checked_sub(1)?)
            .copied()
    }

    fn label(self) -> &'static str {
        match self {
            QueueAction::Pause => "file_operations.menu_pause",
            QueueAction::Resume => "file_operations.menu_resume",
            QueueAction::Cancel => "file_operations.menu_cancel",
            QueueAction::MoveUp => "file_operations.menu_move_up",
            QueueAction::MoveDown => "file_operations.menu_move_down",
            QueueAction::Continue => "file_operations.menu_continue",
            QueueAction::Dismiss => "file_operations.menu_dismiss",
            QueueAction::Report => "file_operations.report",
            QueueAction::Clear => "button.clear",
        }
    }

    /// Its caption on a button of the queue's header.
    pub(crate) fn caption(self) -> &'static str {
        match self {
            QueueAction::Pause => "file_operations.pause",
            QueueAction::Resume => "file_operations.resume",
            QueueAction::Cancel => "file_operations.cancel",
            QueueAction::MoveUp => "file_operations.move_up",
            QueueAction::MoveDown => "file_operations.move_down",
            QueueAction::Continue => "file_operations.continue",
            QueueAction::Dismiss => "file_operations.dismiss",
            QueueAction::Report => "file_operations.report",
            QueueAction::Clear => "button.clear",
        }
    }

    pub(crate) fn glyph(self) -> Glyph {
        match self {
            QueueAction::Pause => Glyph::Pause,
            QueueAction::Resume | QueueAction::Continue => Glyph::Play,
            QueueAction::Cancel | QueueAction::Dismiss => Glyph::Cancel,
            QueueAction::MoveUp => Glyph::ChevronUp,
            QueueAction::MoveDown => Glyph::ChevronDown,
            QueueAction::Report => Glyph::Document,
            QueueAction::Clear => Glyph::EraseTool,
        }
    }
}

impl RowPhase {
    /// The row's context menu, in its order; a separator goes between the
    /// groups.
    pub(crate) fn menu(self) -> Vec<Vec<QueueAction>> {
        use QueueAction as A;
        match self {
            RowPhase::Running { paused: false } => vec![vec![A::Pause, A::Cancel]],
            RowPhase::Running { paused: true } => vec![vec![A::Resume, A::Cancel]],
            RowPhase::Waiting => vec![vec![A::Cancel], vec![A::MoveUp, A::MoveDown]],
            RowPhase::Starting => vec![vec![A::Cancel]],
            RowPhase::Interrupted => vec![vec![A::Continue, A::Dismiss]],
            RowPhase::Completed { report: true } => vec![vec![A::Report, A::Clear]],
            RowPhase::Completed { report: false } => vec![vec![A::Clear]],
            RowPhase::Cancelling => Vec::new(),
        }
    }

    /// What the queue's header buttons do to the selected row.
    pub(crate) fn buttons(self) -> RowButtons {
        use QueueAction as A;
        let (run, end) = match self {
            RowPhase::Running { paused: false } => (Some(A::Pause), A::Cancel),
            RowPhase::Running { paused: true } => (Some(A::Resume), A::Cancel),
            RowPhase::Waiting | RowPhase::Starting => (None, A::Cancel),
            RowPhase::Interrupted => (Some(A::Continue), A::Dismiss),
            RowPhase::Completed { report } => (report.then_some(A::Report), A::Clear),
            RowPhase::Cancelling => (None, A::Cancel),
        };
        RowButtons { run, end }
    }
}

/// The queue's header buttons for the selected row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowButtons {
    /// Pause, Resume or Continue; `None` hides the button.
    pub(crate) run: Option<QueueAction>,
    /// Cancel, or Dismiss for an interrupted operation.
    pub(crate) end: QueueAction,
}

fn movable_ids(rows: &[Row]) -> impl Iterator<Item = OperationId> + '_ {
    (rows.iter())
        .filter(|row| matches!(row.phase, RowPhase::Waiting | RowPhase::Running { .. }))
        .map(|row| row.id)
}

/// Where a row dropped at `gap`, a place between the rows, lands: the gap
/// before the next running or waiting operation, or the end of their order.
pub(crate) fn landing(rows: &[Row], gap: usize) -> (usize, Option<OperationId>) {
    let waiting = |row: &Row| matches!(row.phase, RowPhase::Waiting | RowPhase::Running { .. });
    match (rows.iter().enumerate().skip(gap)).find(|(_, row)| waiting(row)) {
        Some((index, row)) => (index, Some(row.id)),
        None => {
            let after = (rows.iter().rposition(waiting)).map_or(gap, |last| last + 1);
            (after, None)
        }
    }
}

/// Whether putting `id` before `before` leaves its order unchanged.
pub(crate) fn stays_in_place(rows: &[Row], id: OperationId, before: Option<OperationId>) -> bool {
    let next = movable_ids(rows)
        .skip_while(|waiting| *waiting != id)
        .nth(1);
    before == Some(id) || before == next
}

/// The place before which Move up or Move down puts `id`,
/// or `None` when it is first or last already. The
/// inner `None` is the end of the queue.
pub(crate) fn move_step(rows: &[Row], id: OperationId, up: bool) -> Option<Option<OperationId>> {
    let waiting: Vec<OperationId> = movable_ids(rows).collect();
    let index = waiting.iter().position(|w| *w == id)?;
    if up {
        Some(Some(*waiting.get(index.checked_sub(1)?)?))
    } else {
        waiting.get(index + 1)?;
        Some(waiting.get(index + 2).copied())
    }
}

/// The context menu of a row in `phase`.
pub(crate) fn row_menu(strings: &Strings, phase: RowPhase) -> Vec<MenuNode> {
    let mut nodes = Vec::new();
    for (index, group) in phase.menu().into_iter().enumerate() {
        if index > 0 {
            nodes.push(MenuNode::Separator);
        }
        nodes.extend(group.into_iter().map(|action| MenuNode::Item {
            id: action.id(),
            text: strings.text(action.label()).to_owned(),
            mark: Mark::None,
            glyph: Some(action.glyph()),
        }));
    }
    nodes
}

/// The icon of an operation of the kind of `request`.
pub(crate) fn glyph_of(request: &Request) -> Glyph {
    match request {
        _ if renamed(request).is_some() => Glyph::Rename,
        Request::Copy { .. } => Glyph::Copy,
        Request::Synchronize { .. } => Glyph::Refresh,
        Request::Move { .. } => Glyph::MoveToFolder,
        Request::Delete { .. } => Glyph::Delete,
        Request::NewFolder { .. } => Glyph::Folder,
    }
}

/// The entry and its new name when `request` moves one entry to a new name
/// in its own folder, which renames it, as `F6` with a new name does
/// (DESIGN.md: Copy and Move).
fn renamed(request: &Request) -> Option<(String, String)> {
    let Request::Move {
        sources,
        destination: Destination::Named { folder, name },
        ..
    } = request
    else {
        return None;
    };
    let [source] = sources.as_slice() else {
        return None;
    };
    let source = text_of(source);
    (same_folder(parent_of(&source), &text_of(folder)))
        .then(|| (source, name.to_string_lossy().into_owned()))
}

/// The icons the queue list shows, in the order [`icon_index`] numbers.
pub(crate) const ROW_GLYPHS: [Glyph; 6] = [
    Glyph::Refresh,
    Glyph::Copy,
    Glyph::MoveToFolder,
    Glyph::Delete,
    Glyph::Rename,
    Glyph::Folder,
];

/// The index of `glyph` among [`ROW_GLYPHS`].
pub(crate) fn icon_index(glyph: Glyph) -> usize {
    ROW_GLYPHS
        .iter()
        .position(|g| *g == glyph)
        .expect("every operation's glyph is a row glyph")
}

/// The words of one kind of operation.
struct Kind {
    /// "Copy", the card's title.
    name: &'static str,
    /// "Copying", before the current file.
    doing: &'static str,
    /// "Copy {0} → {1}", a row's title.
    row: &'static str,
    /// "Waiting for the copy to {0}".
    waiting_for: &'static str,
}

fn kind_of(request: &Request) -> Kind {
    let kind = |name, doing, row, waiting_for| Kind {
        name,
        doing,
        row,
        waiting_for,
    };
    match request {
        _ if renamed(request).is_some() => kind(
            "file_operations.rename",
            "file_operations.renaming",
            "file_operations.row_rename",
            "file_operations.waiting_for_rename",
        ),
        Request::Copy { .. } => kind(
            "file_operations.copy",
            "file_operations.copying",
            "file_operations.row_copy",
            "file_operations.waiting_for_copy",
        ),
        Request::Move { .. } => kind(
            "file_operations.move",
            "file_operations.moving",
            "file_operations.row_move",
            "file_operations.waiting_for_move",
        ),
        Request::Synchronize { .. } => kind(
            "sync.synchronize",
            "sync.synchronizing",
            "sync.row",
            "sync.waiting_for",
        ),
        Request::Delete { .. } => kind(
            "file_operations.delete",
            "file_operations.deleting",
            "file_operations.row_delete",
            "file_operations.waiting_for_delete",
        ),
        Request::NewFolder { .. } => kind(
            "file_operations.new_folder",
            "file_operations.creating",
            "file_operations.row_new_folder",
            "file_operations.waiting_for_new_folder",
        ),
    }
}

fn doing(request: &Request, running: &Running) -> &'static str {
    if running.phase == gozler_fileops::RunningPhase::Comparing {
        "sync.comparing"
    } else {
        kind_of(request).doing
    }
}

/// Where an operation reads and where it writes, as a row's title shows
/// them: a copy, a move or a delete of one selected entry names it, and of
/// more the folder that holds all of them; a copy and a move also name the
/// destination folder, a rename the old and the new path, and a new folder
/// its path.
#[derive(Debug, PartialEq)]
struct Ends {
    from: String,
    to: Option<String>,
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `name` in the folder `folder`.
fn joined(folder: &str, name: &str) -> String {
    if folder.ends_with('\\') {
        format!("{folder}{name}")
    } else {
        format!("{folder}\\{name}")
    }
}

fn ends_of(strings: &Strings, numbers: &dyn Numbers, operation: &Operation) -> Ends {
    let entries = |sources: &[PathBuf]| match sources {
        [one] => text_of(one),
        _ => sources_text(
            strings,
            numbers,
            &operation.folder,
            sources.len(),
            "file_operations.several_folders_inline",
        ),
    };
    if let Some((path, name)) = renamed(&operation.request) {
        let to = joined(parent_of(&path), &name);
        return Ends {
            from: path,
            to: Some(to),
        };
    }
    match &*operation.request {
        Request::Copy {
            sources,
            destination,
            ..
        }
        | Request::Move {
            sources,
            destination,
            ..
        } => Ends {
            from: entries(sources),
            to: Some(text_of(&destination.path())),
        },
        Request::Delete { sources } => Ends {
            from: entries(sources),
            to: None,
        },
        Request::Synchronize {
            source,
            destination,
            ..
        } => Ends {
            from: text_of(source),
            to: Some(text_of(destination)),
        },
        Request::NewFolder { path } => Ends {
            from: text_of(path),
            to: None,
        },
    }
}

/// The folder that holds an operation's `count` sources; the text
/// `several` when they are on different drives or servers: "Several
/// folders" on its own, or "several folders" inside a sentence; and their
/// count, "4,210 items", until the queue has worked the folder out.
fn sources_text(
    strings: &Strings,
    numbers: &dyn Numbers,
    folder: &SourcesFolder,
    count: usize,
    several: &str,
) -> String {
    match folder {
        SourcesFolder::Shared(folder) => folder.clone(),
        SourcesFolder::Several => strings.text(several).to_owned(),
        SourcesFolder::Pending => {
            strings.count("file_operations.size_items", count as u64, numbers, &[])
        }
    }
}

/// Where the card says an operation reads and writes, which stays the same
/// for the whole operation while the file line names each file: as
/// [`ends_of`] says, except that a copy, a move and a delete read from the
/// folder that holds their entries, also for one.
fn card_ends(strings: &Strings, numbers: &dyn Numbers, operation: &Operation) -> Ends {
    let from = |sources: &[PathBuf]| {
        sources_text(
            strings,
            numbers,
            &operation.folder,
            sources.len(),
            "file_operations.several_folders",
        )
    };
    match &*operation.request {
        Request::Copy {
            sources,
            destination,
            ..
        }
        | Request::Move {
            sources,
            destination,
            ..
        } => Ends {
            from: from(sources),
            to: Some(text_of(&destination.path())),
        },
        Request::Delete { sources } => Ends {
            from: from(sources),
            to: None,
        },
        Request::NewFolder { .. } | Request::Synchronize { .. } => {
            ends_of(strings, numbers, operation)
        }
    }
}

/// The window's view of `operations`, in the queue's order, with the speeds
/// that `meters` worked out for the running ones.
pub(crate) fn view(
    strings: &Strings,
    numbers: &dyn Numbers,
    operations: &[Operation],
    meters: &HashMap<OperationId, Meter>,
    detail: Option<Detail>,
) -> View {
    let speed = |id| meters.get(&id).and_then(Meter::speed);
    let transfers: Vec<_> = operations
        .iter()
        .filter(|o| is_transfer(&o.request))
        .collect();
    let selected = match detail {
        Some(detail) => transfers.iter().copied().find(|o| o.id == detail.id()),
        None => transfers
            .iter()
            .copied()
            .find(|o| matches!(&o.state, State::Running(running) if running.is_active()))
            .or_else(|| transfers.first().copied()),
    };
    let rows: Vec<Row> = transfers
        .iter()
        .map(|operation| row(strings, numbers, operation, operations, speed(operation.id)))
        .collect();
    let card = selected
        .map(|operation| card_for(strings, numbers, operation, operations, speed(operation.id)));
    let report = selected.and_then(|operation| {
        if detail != Some(Detail::Report(operation.id)) {
            return None;
        }
        let State::Ended(outcome) = &operation.state else {
            return None;
        };
        Some(Report {
            id: operation.id,
            title: operation_title(strings, numbers, &operation.request),
            summary: match &outcome.end {
                End::Complete | End::Cancelled => strings.count(
                    "file_operations.failed",
                    outcome.listed.len() as u64,
                    numbers,
                    &[],
                ),
                End::DiskFull(no_room) => joined_sentences([
                    strings.text("file_operations.disk_full").to_owned(),
                    no_room_text(strings, numbers, no_room),
                ]),
                End::Refused(refusal) => {
                    refusal_text(strings, numbers, *refusal, &|code| code.to_string())
                }
            },
            outcome: Arc::clone(outcome),
        })
    });
    let preparing =
        (operations.iter()).any(|operation| matches!(operation.state, State::Preparing));
    let idle = strings.text(if preparing {
        "file_operations.preparing"
    } else {
        "file_operations.nothing_running"
    });
    // A row that runs beside the card's operation or was interrupted does
    // not wait, and its own line says what it does.
    let queue = strings.count("file_operations.jobs", rows.len() as u64, numbers, &[]);
    View {
        title: strings.text("file_operations.transfers").to_owned(),
        card,
        idle: idle.to_owned(),
        queue,
        rows,
        report,
    }
}

fn card_for(
    strings: &Strings,
    numbers: &dyn Numbers,
    operation: &Operation,
    operations: &[Operation],
    speed: Option<Speed>,
) -> Card {
    if let State::Running(running) = &operation.state {
        return card(strings, numbers, operation, running, speed);
    }
    let ends = card_ends(strings, numbers, operation);
    let row = row(strings, numbers, operation, operations, speed);
    let phase = row.phase;
    let total = match &operation.state {
        State::Prepared(p) => p.total,
        State::Waiting { total, .. } => *total,
        _ => None,
    };
    let (total_bar, items, bytes_text) = match &operation.state {
        State::Ended(outcome) => (
            Bar::Measured(Measure { done: 1, total: 1 }),
            strings.count("file_operations.done", outcome.completed, numbers, &[]),
            bytes(strings, numbers, outcome.bytes),
        ),
        _ => (
            total.map_or(Bar::Moving, |total| {
                Bar::Measured(Measure {
                    done: 0,
                    total: if total.bytes > 0 {
                        total.bytes
                    } else {
                        total.items
                    },
                })
            }),
            String::new(),
            String::new(),
        ),
    };
    Card {
        id: operation.id,
        glyph: glyph_of(&operation.request),
        title: operation_title(strings, numbers, &operation.request),
        status: match &operation.state {
            State::Ended(outcome) => strings
                .text(match outcome.end {
                    End::Cancelled => "file_operations.cancelled",
                    End::Refused(_) => "file_operations.not_started",
                    _ if reports(outcome) => "file_operations.completed_errors",
                    _ => "file_operations.completed",
                })
                .to_owned(),
            _ => row.detail.whole(strings),
        },
        paused: false,
        doing: strings.text("file_operations.file").to_owned(),
        file: String::new(),
        file_tip: String::new(),
        from: ends.from,
        to: ends.to,
        file_bar: None,
        file_bytes: String::new(),
        total_bar,
        items,
        bytes: bytes_text,
        phase,
        rate: String::new(),
        files_rate: String::new(),
    }
}

/// Whether a running operation's progress is in bytes: a copy or a move
/// across volumes with data to move. A move by rename and a copy of empty
/// files count their entries.
pub(super) fn moves_bytes(running: &Running) -> bool {
    running.phase != gozler_fileops::RunningPhase::Synchronizing
        && !running.renames
        && running.total.is_some_and(|total| total.bytes > 0)
}

/// What a running operation has done, and its total; `None` without a
/// total. Each count done is at most its total, because gozlerd's total can
/// miss entries, such as those a delete removed before gozlerd answered.
fn done_of(running: &Running) -> Option<(Total, Total)> {
    let total = running.total?;
    let done = Total {
        items: running.items.min(total.items),
        bytes: (running.bytes + running.passed).min(total.bytes),
    };
    Some((done, total))
}

/// How much of a running operation is done: its bytes where it moves bytes,
/// otherwise its entries, and nothing measured without a total.
fn measure_of(running: &Running) -> Option<Measure> {
    let (done, total) = done_of(running)?;
    Some(if moves_bytes(running) {
        Measure {
            done: done.bytes,
            total: total.bytes,
        }
    } else {
        Measure {
            done: done.items,
            total: total.items,
        }
    })
}

/// Every running operation's progress together, as File Explorer shows its
/// copies on the taskbar button: measured while any running operation
/// knows its total, moving while none does.
pub(crate) fn taskbar(operations: &[Operation]) -> Taskbar {
    let running: Vec<&Running> = operations
        .iter()
        .filter_map(|operation| match &operation.state {
            State::Running(running) => Some(running),
            _ => None,
        })
        .collect();
    if running.is_empty() {
        return Taskbar::None;
    }
    let measured: Vec<Measure> = running.iter().filter_map(|r| measure_of(r)).collect();
    if measured.is_empty() {
        return if running.iter().any(|r| r.is_active()) {
            Taskbar::Moving
        } else {
            Taskbar::None
        };
    }
    // In thousandths of each operation, so a small copy beside a large
    // one still moves the bar.
    let done: u64 = measured
        .iter()
        .map(|m| u128::from(m.done) * 1000 / u128::from(m.total.max(1)))
        .map(|done| done as u64)
        .sum();
    let total = Measure {
        done,
        total: 1000 * measured.len() as u64,
    };
    if running.iter().all(|r| !r.is_active()) {
        Taskbar::Paused(total)
    } else {
        Taskbar::Measured(total)
    }
}

fn card(
    strings: &Strings,
    numbers: &dyn Numbers,
    operation: &Operation,
    running: &Running,
    speed: Option<Speed>,
) -> Card {
    let ends = card_ends(strings, numbers, operation);
    let measure = measure_of(running);
    let moves_bytes = moves_bytes(running);
    let title = operation_title(strings, numbers, &operation.request);
    let status = strings
        .text(held(running).unwrap_or_else(|| doing(&operation.request, running)))
        .to_owned();
    let (file, file_tip, file_bar) = match &running.current {
        Some(current) => {
            let source = text_of(&current.source);
            let file = relative_to(&source, &ends.from).to_owned();
            let tip = match &current.destination {
                Some(destination) => format!("{source}\n→ {}", text_of(destination)),
                None => source.clone(),
            };
            let bar = current
                .destination
                .is_some()
                .then_some(Bar::Measured(Measure {
                    done: current.done,
                    total: current.size,
                }));
            (file, tip, bar)
        }
        // A delete and a move by rename report no current file, so the
        // line names their one selected entry, as it does before a copy's
        // first file.
        None => {
            let one = match &*operation.request {
                Request::Copy { sources, .. }
                | Request::Move { sources, .. }
                | Request::Delete { sources } => match sources.as_slice() {
                    [one] => text_of(one),
                    _ => String::new(),
                },
                Request::NewFolder { .. } | Request::Synchronize { .. } => String::new(),
            };
            (
                relative_to(&one, &ends.from).to_owned(),
                one,
                moves_bytes.then_some(Bar::Moving),
            )
        }
    };
    let (items, bytes_text) = counts(strings, numbers, &operation.request, running);
    Card {
        id: operation.id,
        glyph: glyph_of(&operation.request),
        title,
        status,
        paused: !running.is_active(),
        doing: strings.text("file_operations.file").to_owned(),
        file,
        file_tip,
        from: ends.from,
        to: ends.to,
        file_bytes: match file_bar {
            Some(Bar::Measured(m)) => strings.format(
                "file_operations.bytes_of",
                &[
                    &bytes(strings, numbers, m.done),
                    &bytes(strings, numbers, m.total),
                ],
            ),
            Some(Bar::Moving) => strings.text("file_operations.total_unknown").to_owned(),
            None => String::new(),
        },
        file_bar,
        total_bar: measure.map_or(Bar::Moving, Bar::Measured),
        items,
        bytes: bytes_text,
        phase: RowPhase::Running {
            paused: running.paused,
        },
        rate: speed
            .filter(|_| running.is_active())
            .map_or_else(String::new, |s| bytes_per_second(strings, numbers, s.bytes)),
        files_rate: speed
            .filter(|_| running.is_active())
            .map_or_else(String::new, |s| items_per_second(strings, numbers, s)),
    }
}

fn operation_title(strings: &Strings, numbers: &dyn Numbers, request: &Request) -> String {
    let sources = match request {
        Request::Copy { sources, .. } | Request::Move { sources, .. } => sources.as_slice(),
        _ => return strings.text(kind_of(request).name).to_owned(),
    };
    let name = match sources {
        [one] => name_of(&text_of(one)).to_owned(),
        _ => strings.count(
            "file_operations.size_items",
            sources.len() as u64,
            numbers,
            &[],
        ),
    };
    let title = strings.format(
        "file_operations.running_label",
        &[strings.text(kind_of(request).name), &name],
    );
    match request.policy() {
        Some(policy) => joined_parts(strings, [title, policy_name(strings, policy).to_owned()]),
        None => title,
    }
}

fn held(running: &Running) -> Option<&'static str> {
    if running.paused {
        Some("file_operations.paused")
    } else if running.yielded {
        Some("file_operations.waiting")
    } else {
        None
    }
}

pub(crate) fn transfer_caption(
    strings: &Strings,
    numbers: &dyn Numbers,
    operation: &StatusOperation,
    speed: Option<Speed>,
    bar: Bar,
    errors: u8,
    report: Option<OperationId>,
) -> super::status_transfer::Caption {
    use super::captions::JobProgress;
    use super::status_transfer::{Action, Caption};
    let (kind, state, paused) = match operation {
        StatusOperation::Running {
            request, running, ..
        } => (
            kind_of(request).name,
            held(running).unwrap_or_else(|| doing(request, running)),
            Some(running.paused),
        ),
        StatusOperation::Pending { request, phase, .. } => (
            kind_of(request).name,
            match phase {
                Pending::Preparing => "file_operations.preparing",
                Pending::Waiting => "file_operations.waiting",
                Pending::Cancelling => "file_operations.cancelling",
                Pending::Interrupted => "file_operations.interrupted",
                Pending::Review => "sync.review",
            },
            None,
        ),
        StatusOperation::Ended {
            request, outcome, ..
        } => (
            kind_of(request).name,
            if errors > 0 {
                "file_operations.completed_errors"
            } else if outcome.end == End::Cancelled {
                "file_operations.cancelled"
            } else {
                "file_operations.completed"
            },
            None,
        ),
        StatusOperation::Summary { completion, .. } => (
            match completion.kind {
                Completed::Copy(_) => "file_operations.copy",
                Completed::Synchronize(_) => "sync.synchronize",
                _ => "file_operations.move",
            },
            if errors > 0 {
                "file_operations.completed_errors"
            } else if completion.end == End::Cancelled {
                "file_operations.cancelled"
            } else {
                "file_operations.completed"
            },
            None,
        ),
    };
    let ended = matches!(
        operation,
        StatusOperation::Ended { .. } | StatusOperation::Summary { .. }
    );
    let kind = if ended && errors > 0 {
        match errors {
            1 => "file_operations.copy",
            2 => "file_operations.move",
            3 => "file_operations.copy_move",
            4 => "sync.synchronize",
            5 => "sync.copy_sync",
            6 => "sync.move_sync",
            _ => "sync.copy_move_sync",
        }
    } else {
        kind
    };
    let active = matches!(operation, StatusOperation::Running {running,..} if running.is_active());
    let (activity, status) = if active {
        let StatusOperation::Running { request, .. } = operation else {
            unreachable!("only a running operation can be unpaused")
        };
        let sources = match &**request {
            Request::Copy { sources, .. } | Request::Move { sources, .. } => sources.as_slice(),
            _ => &[],
        };
        let name = match sources {
            [one] => name_of(&text_of(one)).to_owned(),
            _ => strings.count(
                "file_operations.size_items",
                sources.len() as u64,
                numbers,
                &[],
            ),
        };
        let name = match &**request {
            Request::Synchronize { destination, .. } => name_of(&text_of(destination)).to_owned(),
            _ => name,
        };
        (
            strings.format(
                "file_operations.running_label",
                &[strings.text(state), &name],
            ),
            name,
        )
    } else {
        (
            strings.format(
                "file_operations.state_label",
                &[strings.text(kind), strings.text(state)],
            ),
            strings.text(state).to_owned(),
        )
    };
    let (progress, percent) = match bar {
        _ if matches!(
            operation,
            StatusOperation::Pending {
                phase: Pending::Interrupted | Pending::Review,
                ..
            }
        ) =>
        {
            (JobProgress::None, String::new())
        }
        Bar::Measured(m) => (
            JobProgress::Measured {
                done: m.done,
                total: m.total,
            },
            strings.format("file_operations.percent", &[&m.percent().to_string()]),
        ),
        Bar::Moving => (
            JobProgress::Unmeasured,
            strings.text("file_operations.total_unknown").to_owned(),
        ),
    };
    let action = |command: QueueAction| Action {
        text: strings.text(command.caption()).to_owned(),
        glyph: command.glyph(),
        command,
    };
    let rate = speed.filter(|_| active);
    Caption {
        id: if ended {
            report.unwrap_or_else(|| operation.id())
        } else {
            operation.id()
        },
        name: strings.text(kind).to_owned(),
        status,
        activity: activity.clone(),
        glyph: Some(match kind {
            "file_operations.copy" => Glyph::Copy,
            "sync.synchronize" | "sync.copy_sync" | "sync.move_sync" | "sync.copy_move_sync" => {
                Glyph::Refresh
            }
            _ => Glyph::MoveToFolder,
        }),
        progress,
        paused: matches!(operation, StatusOperation::Running { running, .. } if !running.is_active()),
        percent,
        byte_rate: rate.map_or_else(String::new, |s| bytes_per_second(strings, numbers, s.bytes)),
        file_rate: rate.map_or_else(String::new, |s| items_per_second(strings, numbers, s)),
        pause: paused.map(|paused| {
            action(if paused {
                QueueAction::Resume
            } else {
                QueueAction::Pause
            })
        }),
        cancel: operation.can_cancel().then(|| action(QueueAction::Cancel)),
        report: (ended && report.is_some()).then(|| action(QueueAction::Report)),
        summary_tip: activity,
    }
}

fn counts(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    running: &Running,
) -> (String, String) {
    match done_of(running) {
        Some((done, total)) => (
            strings.count(
                "file_operations.items_of",
                total.items,
                numbers,
                &[&numbers.integer(done.items)],
            ),
            if !running.renames && total.bytes > 0 {
                strings.format(
                    "file_operations.bytes_of",
                    &[
                        &bytes(strings, numbers, done.bytes),
                        &bytes(strings, numbers, total.bytes),
                    ],
                )
            } else {
                String::new()
            },
        ),
        None => (
            items_done(strings, numbers, request, running.items),
            String::new(),
        ),
    }
}

/// A speed in entries, as "120 items/s".
fn items_per_second(strings: &Strings, numbers: &dyn Numbers, speed: Speed) -> String {
    strings.count(
        "file_operations.items_per_second",
        speed.items.round() as u64,
        numbers,
        &[],
    )
}

/// `parts` joined with the separator " · ".
fn joined_parts(strings: &Strings, parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(strings.text("file_operations.separator"))
}

fn row(
    strings: &Strings,
    numbers: &dyn Numbers,
    operation: &Operation,
    operations: &[Operation],
    speed: Option<Speed>,
) -> Row {
    let ends = ends_of(strings, numbers, operation);
    let kind = kind_of(&operation.request);
    let (detail, phase) = match &operation.state {
        State::Running(running) => {
            let progress = match measure_of(running) {
                Some(measure) if moves_bytes(running) => strings.format(
                    "file_operations.bytes_of",
                    &[
                        &bytes(strings, numbers, measure.done),
                        &bytes(strings, numbers, measure.total),
                    ],
                ),
                Some(measure) => strings.count(
                    "file_operations.items_of",
                    measure.total,
                    numbers,
                    &[&numbers.integer(measure.done)],
                ),
                None => items_done(strings, numbers, &operation.request, running.items),
            };
            let detail = if let Some(state) = held(running) {
                joined_parts(strings, [strings.text(state).to_owned(), progress])
            } else {
                let rate = speed.map(|speed| {
                    if moves_bytes(running) {
                        bytes_per_second(strings, numbers, speed.bytes)
                    } else {
                        items_per_second(strings, numbers, speed)
                    }
                });
                joined_parts(strings, [progress].into_iter().chain(rate))
            };
            (
                RowLine::Plain(detail),
                RowPhase::Running {
                    paused: running.paused,
                },
            )
        }
        State::Waiting { on, total, .. } => {
            let size = size_of_total(strings, numbers, &operation.request, *total);
            let detail = match operations.iter().find(|other| other.id == *on) {
                Some(other) => {
                    let ends = ends_of(strings, numbers, other);
                    let place = match (&*other.request, ends.to) {
                        (Request::Copy { .. } | Request::Move { .. }, Some(to)) => to,
                        _ => ends.from,
                    };
                    RowLine::Paths {
                        key: kind_of(&other.request).waiting_for,
                        paths: vec![place],
                        after: if size.is_empty() {
                            size
                        } else {
                            strings.text("file_operations.separator").to_owned() + &size
                        },
                    }
                }
                None => RowLine::Plain(size),
            };
            (detail, RowPhase::Waiting)
        }
        State::Preparing => (
            RowLine::Plain(strings.text("file_operations.preparing").to_owned()),
            RowPhase::Starting,
        ),
        State::Compared(_) => (
            RowLine::Plain(strings.text("sync.review").to_owned()),
            RowPhase::Starting,
        ),
        State::Cancelling => (
            RowLine::Plain(strings.text("file_operations.cancelling").to_owned()),
            RowPhase::Cancelling,
        ),
        State::Prepared(prepared) => {
            let size = match prepared.total {
                Some(total) => size_of_total(strings, numbers, &operation.request, Some(total)),
                None => strings.count(
                    "file_operations.selected",
                    prepared.entries as u64,
                    numbers,
                    &[],
                ),
            };
            (
                RowLine::Plain(joined_parts(
                    strings,
                    [strings.text("file_operations.prepared").to_owned(), size],
                )),
                RowPhase::Starting,
            )
        }
        State::Interrupted => {
            let interrupted = strings.text("file_operations.interrupted").to_owned();
            // A copy and a move leave unfinished files that Continue
            // finishes (fast-copy-delete-spec.md: What a stopped operation
            // leaves).
            let detail = match &*operation.request {
                Request::Copy { .. } | Request::Move { .. } => joined_parts(
                    strings,
                    [
                        interrupted,
                        strings.text("file_operations.dismiss_leaves").to_owned(),
                    ],
                ),
                _ => interrupted,
            };
            (RowLine::Plain(detail), RowPhase::Interrupted)
        }
        State::Ended(outcome) => (
            RowLine::Plain(joined_parts(
                strings,
                reports(outcome)
                    .then(|| strings.text("file_operations.report_available").to_owned())
                    .into_iter()
                    .chain([ended_line(
                        strings,
                        numbers,
                        &operation.request,
                        outcome,
                        false,
                    )]),
            )),
            RowPhase::Completed {
                report: reports(outcome),
            },
        ),
    };
    Row {
        id: operation.id,
        glyph: glyph_of(&operation.request),
        title: RowLine::Paths {
            key: kind.row,
            paths: [Some(ends.from), ends.to].into_iter().flatten().collect(),
            after: String::new(),
        },
        detail,
        phase,
    }
}

/// A waiting operation's size, as gozlerd gave its total: "4,210 items,
/// 18.4 GB" for a copy, "4,210 items" for a move by rename, or the
/// selected entries when gozlerd had no total.
fn size_of_total(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    total: Option<Total>,
) -> String {
    match (total, request) {
        (None, _) if renamed(request).is_some() => String::new(),
        (Some(total), _) if total.bytes > 0 => strings.count(
            "file_operations.size_items_bytes",
            total.items,
            numbers,
            &[&bytes(strings, numbers, total.bytes)],
        ),
        (Some(total), _) => strings.count("file_operations.size_items", total.items, numbers, &[]),
        (
            None,
            Request::Copy { sources, .. }
            | Request::Move { sources, .. }
            | Request::Delete { sources },
        ) => strings.count(
            "file_operations.selected",
            sources.len() as u64,
            numbers,
            &[],
        ),
        (None, Request::NewFolder { .. } | Request::Synchronize { .. }) => String::new(),
    }
}

/// The items a running operation without a total has done: "6,240
/// deleted", "6,240 copied"; nothing for a rename or a new folder, which is
/// one step.
fn items_done(strings: &Strings, numbers: &dyn Numbers, request: &Request, items: u64) -> String {
    let key = match request {
        _ if renamed(request).is_some() => return String::new(),
        Request::Copy { .. } => "file_operations.copied",
        Request::Move { .. } => "file_operations.moved",
        Request::Delete { .. } => "file_operations.deleted",
        Request::Synchronize { .. } => "sync.processed",
        Request::NewFolder { .. } => return String::new(),
    };
    strings.count(key, items, numbers, &[])
}

pub(crate) type LastEnded = (OperationId, Completion);

/// The status retains counts and at most two paths, independently of the
/// selected entries and the report's potentially large failure list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Completion {
    kind: Completed,
    end: End,
    items: u64,
    failed: u64,
    pub(crate) numbered: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Completed {
    Copy(PathBuf),
    Move(PathBuf),
    Synchronize(PathBuf),
    Rename { source: String, name: String },
    Delete,
    NewFolder(PathBuf),
}

impl Completion {
    pub(crate) fn of(request: &Request, outcome: &Outcome, numbered: bool) -> Self {
        let kind = match request {
            _ if renamed(request).is_some() => {
                let (source, name) = renamed(request).expect("the request is a rename");
                Completed::Rename { source, name }
            }
            Request::Copy { destination, .. } => Completed::Copy(destination.path()),
            Request::Move { destination, .. } => Completed::Move(destination.path()),
            Request::Synchronize { destination, .. } => Completed::Synchronize(destination.clone()),
            Request::Delete { .. } => Completed::Delete,
            Request::NewFolder { path } => Completed::NewFolder(path.clone()),
        };
        Self {
            kind,
            end: outcome.end.clone(),
            items: outcome.completed,
            failed: outcome.listed.len() as u64,
            numbered,
        }
    }
}

/// A status snapshot. Operations owns which live work or retained outcome
/// remains visible; transfers retain only reports the person has not cleared.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StatusOperation {
    Summary {
        id: OperationId,
        completion: Completion,
    },
    Pending {
        id: OperationId,
        request: Arc<Request>,
        phase: Pending,
    },
    Running {
        id: OperationId,
        request: Arc<Request>,
        running: Running,
    },
    Ended {
        id: OperationId,
        request: Arc<Request>,
        outcome: Arc<Outcome>,
        /// Keep both answered its conflict question, so an entry may have
        /// a numbered name, which the window does not know.
        numbered: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Preparing,
    Waiting,
    Cancelling,
    Interrupted,
    Review,
}

impl StatusOperation {
    pub(crate) fn id(&self) -> OperationId {
        match self {
            StatusOperation::Summary { id, .. } => *id,
            StatusOperation::Running { id, .. }
            | StatusOperation::Ended { id, .. }
            | StatusOperation::Pending { id, .. } => *id,
        }
    }

    pub(crate) fn can_cancel(&self) -> bool {
        matches!(
            self,
            StatusOperation::Running { .. }
                | StatusOperation::Pending {
                    phase: Pending::Preparing | Pending::Waiting | Pending::Review,
                    ..
                }
        )
    }

    pub(crate) fn is_transfer(&self) -> bool {
        let request = match self {
            Self::Summary { completion, .. } => {
                return matches!(
                    completion.kind,
                    Completed::Copy(_)
                        | Completed::Move(_)
                        | Completed::Synchronize(_)
                        | Completed::Rename { .. }
                );
            }
            Self::Running { request, .. }
            | Self::Ended { request, .. }
            | Self::Pending { request, .. } => request,
        };
        is_transfer(request)
    }

    /// The newest of the running `operations`, else the newest that ended
    /// and is not `reported`, such as one whose failures the window is about
    /// to report, else `ended`, the one reported last. A delete or a move
    /// stays in the queue after its report while a list keeps it, so only
    /// `ended` tells which operation ended last. Dormant interrupted work is
    /// shown only when no live work or completion remains. Each operation
    /// in `numbered` was answered with Keep both.
    pub(crate) fn of(
        operations: &[Operation],
        ended: Option<&LastEnded>,
        reported: &HashSet<OperationId>,
        numbered: &HashSet<OperationId>,
    ) -> Option<StatusOperation> {
        let newest = |running: bool| {
            (operations.iter()).find(|operation| match &operation.state {
                State::Running(current) => running && current.is_active(),
                State::Ended(_) => !running && !reported.contains(&operation.id),
                _ => false,
            })
        };
        let ended_status =
            |id, request: &Arc<Request>, outcome: &Arc<Outcome>| StatusOperation::Ended {
                id,
                request: Arc::clone(request),
                outcome: Arc::clone(outcome),
                numbered: numbered.contains(&id),
            };
        let pending = || {
            operations.iter().find(|operation| {
                matches!(
                    operation.state,
                    State::Preparing
                        | State::Prepared(_)
                        | State::Compared(_)
                        | State::Waiting { .. }
                        | State::Cancelling
                )
            })
        };
        let current = newest(true)
            .or_else(|| {
                operations.iter().find(|operation| {
                    matches!(&operation.state, State::Running(running) if running.paused && !running.yielded)
                })
            })
            .or_else(pending)
            .or_else(|| {
                operations
                    .iter()
                    .find(|op| matches!(op.state, State::Running(_)))
            })
            .or_else(|| newest(false));
        if let Some(operation) = current {
            return Some(match &operation.state {
                State::Running(running) => StatusOperation::Running {
                    id: operation.id,
                    request: Arc::clone(&operation.request),
                    running: running.clone(),
                },
                State::Ended(outcome) => ended_status(operation.id, &operation.request, outcome),
                state => StatusOperation::Pending {
                    id: operation.id,
                    request: Arc::clone(&operation.request),
                    phase: match state {
                        State::Cancelling => Pending::Cancelling,
                        State::Waiting { .. } => Pending::Waiting,
                        State::Interrupted => Pending::Interrupted,
                        State::Compared(_) => Pending::Review,
                        _ => Pending::Preparing,
                    },
                },
            });
        }
        ended
            .map(|(id, completion)| StatusOperation::Summary {
                id: *id,
                completion: completion.clone(),
            })
            .or_else(|| {
                operations
                    .iter()
                    .find(|operation| matches!(operation.state, State::Interrupted))
                    .map(|operation| StatusOperation::Pending {
                        id: operation.id,
                        request: Arc::clone(&operation.request),
                        phase: Pending::Interrupted,
                    })
            })
    }
}

/// The middle group's line for `operation`, with the bar beside a running
/// one: "Copying Projects to D:\Backup · 38 %", or "Copied 3,310 items to
/// D:\Backup". A refused operation, and one the full disk stopped, show no
/// line, because their message box already said why.
pub(crate) fn status_line(
    strings: &Strings,
    numbers: &dyn Numbers,
    operation: &StatusOperation,
) -> (String, Option<Bar>) {
    match operation {
        StatusOperation::Summary { completion, .. } => {
            (completion_line(strings, numbers, completion), None)
        }
        StatusOperation::Pending { request, phase, .. } => {
            let key = match phase {
                Pending::Preparing => "file_operations.preparing",
                Pending::Waiting => "file_operations.waiting",
                Pending::Cancelling => "file_operations.cancelling",
                Pending::Interrupted => "file_operations.interrupted",
                Pending::Review => "sync.review",
            };
            let text = joined_parts(
                strings,
                [
                    doing_line(strings, numbers, request),
                    strings.text(key).to_owned(),
                ],
            );
            (
                text,
                (!matches!(phase, Pending::Interrupted | Pending::Review)).then_some(Bar::Moving),
            )
        }
        StatusOperation::Running {
            request, running, ..
        } => {
            let (text, bar) = running_line(strings, numbers, request, running);
            (text, Some(bar))
        }
        StatusOperation::Ended {
            request,
            outcome,
            numbered,
            ..
        } => (
            ended_line(strings, numbers, request, outcome, *numbered),
            None,
        ),
    }
}

/// A running operation's line: what it does, how far it is, and whether it
/// is paused.
fn running_line(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    running: &Running,
) -> (String, Bar) {
    let measure = measure_of(running);
    let progress = match measure {
        Some(measure) => {
            strings.format("file_operations.percent", &[&measure.percent().to_string()])
        }
        None => items_done(strings, numbers, request, running.items),
    };
    let paused = held(running).map(|key| strings.text(key).to_owned());
    let text = joined_parts(
        strings,
        [
            if running.is_active() {
                if running.phase == gozler_fileops::RunningPhase::Comparing {
                    strings.text("sync.comparing").to_owned()
                } else {
                    doing_line(strings, numbers, request)
                }
            } else {
                operation_title(strings, numbers, request)
            },
            progress,
        ]
        .into_iter()
        .chain(paused),
    );
    (text, measure.map_or(Bar::Moving, Bar::Measured))
}

/// The middle group's lines at their widest, running and ended, with
/// `total` items, which the layout sizes the group for. The paths are
/// short samples, because the window shortens a long one to fit.
pub(crate) fn status_widest(
    strings: &Strings,
    numbers: &dyn Numbers,
    total: u64,
) -> [Vec<String>; 2] {
    let one = || vec![PathBuf::from(r"C:\Users\Projects")];
    let several = || vec![PathBuf::new(); 2];
    let destination = || Destination::Folder(PathBuf::from(r"D:\Backup"));
    let requests = [
        Request::Copy {
            policy: gozler_fileops::Policy::default(),
            sources: one(),
            destination: destination(),
        },
        Request::Move {
            policy: gozler_fileops::Policy::default(),
            sources: several(),
            destination: destination(),
        },
        Request::Delete { sources: several() },
        Request::Move {
            policy: gozler_fileops::Policy::default(),
            sources: one(),
            destination: Destination::Named {
                folder: PathBuf::from(r"C:\Users"),
                name: "Projects 2026".into(),
            },
        },
        Request::NewFolder {
            path: PathBuf::from(r"D:\Backup\Projects 2026"),
        },
    ];
    let running = requests.iter().flat_map(|request| {
        let measured = Some(Total {
            items: total,
            bytes: total,
        });
        [measured, None].map(|known| {
            let running = Running {
                phase: gozler_fileops::RunningPhase::Executing,
                items: total,
                bytes: total,
                passed: 0,
                total: known,
                renames: false,
                paused: true,
                yielded: false,
                current: None,
            };
            running_line(strings, numbers, request, &running).0
        })
    });
    let skipped = Listed {
        path: PathBuf::new(),
        reason: Reason::Skipped,
    };
    let ended = requests.iter().flat_map(|request| {
        // One skipped item is enough for the failed part's widest form:
        // its count is the same digits as the done count's.
        [(End::Complete, 0), (End::Complete, 1), (End::Cancelled, 1)].map(|(end, failed)| {
            let outcome = Outcome {
                succeeded: end == End::Complete && failed == 0,
                end,
                items: total,
                completed: total.saturating_sub(failed as u64),
                bytes: 0,
                listed: vec![skipped.clone(); failed],
                ring: RingCounts::default(),
            };
            ended_line(strings, numbers, request, &outcome, false)
        })
    });
    [running.collect(), ended.collect()]
}

/// What a running operation does: "Copying Projects to D:\Backup",
/// "Deleting 12 items".
fn doing_line(strings: &Strings, numbers: &dyn Numbers, request: &Request) -> String {
    let what = |sources: &[PathBuf]| match sources {
        [one] => name_of(&text_of(one)).to_owned(),
        _ => strings.count(
            "file_operations.size_items",
            sources.len() as u64,
            numbers,
            &[],
        ),
    };
    if let Some((path, name)) = renamed(request) {
        return strings.format("file_operations.status_renaming", &[name_of(&path), &name]);
    }
    match request {
        Request::Copy {
            sources,
            destination,
            ..
        } => strings.format(
            "file_operations.status_copying",
            &[&what(sources), &text_of(&destination.path())],
        ),
        Request::Move {
            sources,
            destination,
            ..
        } => strings.format(
            "file_operations.status_moving",
            &[&what(sources), &text_of(&destination.path())],
        ),
        Request::Delete { sources } => {
            strings.format("file_operations.status_deleting", &[&what(sources)])
        }
        Request::NewFolder { path } => {
            strings.format("file_operations.status_creating", &[&text_of(path)])
        }
        Request::Synchronize { destination, .. } => {
            strings.format("sync.running", &[&text_of(destination)])
        }
    }
}

/// What an ended operation did, with the count of the items it finished
/// and, after them, how many failed or were skipped; `numbered` as
/// [`StatusOperation::Ended`] has it.
fn ended_line(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    outcome: &Outcome,
    numbered: bool,
) -> String {
    completion_line(
        strings,
        numbers,
        &Completion::of(request, outcome, numbered),
    )
}

fn completion_line(strings: &Strings, numbers: &dyn Numbers, completion: &Completion) -> String {
    let cancelled = match completion.end {
        End::Complete => false,
        End::Cancelled => true,
        End::DiskFull(_) | End::Refused(_) => {
            return strings.text("file_operations.could_not_finish").to_owned();
        }
    };
    let failed = completion.failed;
    let done = completion.items;
    let count = |key: &str, to: &[&str]| strings.count(key, done, numbers, to);
    // A rename and a new folder are one step, which succeeded or failed;
    // cancelled, it did nothing, as the person chose, so it shows no line.
    if let Completed::Rename { source: path, name } = &completion.kind {
        return match (cancelled, failed, completion.numbered) {
            (true, ..) => String::new(),
            (false, 0, false) => {
                strings.format("file_operations.ended_rename", &[name_of(path), name])
            }
            (false, 0, true) => {
                strings.format("file_operations.ended_rename_numbered", &[name_of(path)])
            }
            (false, _, _) => strings.format("file_operations.rename_failed", &[name_of(path)]),
        };
    }
    let line = match &completion.kind {
        Completed::Copy(destination) | Completed::Move(destination) => {
            let moves = matches!(completion.kind, Completed::Move(_));
            let key = match (moves, cancelled) {
                (false, false) => "file_operations.ended_copy",
                (false, true) => "file_operations.cancelled_copy",
                (true, false) => "file_operations.ended_move",
                (true, true) => "file_operations.cancelled_move",
            };
            count(key, &[&text_of(destination)])
        }
        Completed::Delete if cancelled => count("file_operations.cancelled_delete", &[]),
        Completed::Synchronize(destination) => {
            let key = if cancelled {
                "sync.cancelled"
            } else {
                "sync.completed"
            };
            strings.format(key, &[&text_of(destination)])
        }
        Completed::Delete => count("file_operations.ended_delete", &[]),
        Completed::NewFolder(_) if cancelled => return String::new(),
        Completed::NewFolder(path) => {
            let key = match failed {
                0 => "file_operations.ended_new_folder",
                _ => "file_operations.new_folder_failed",
            };
            return strings.format(key, &[&text_of(path)]);
        }
        Completed::Rename { .. } => unreachable!("renames returned their own line"),
    };
    let failed =
        (failed > 0).then(|| strings.count("file_operations.ended_failed", failed, numbers, &[]));
    joined_parts(strings, std::iter::once(line).chain(failed))
}

/// The parent folder of a Windows path, without its last `\`; a drive's
/// root keeps its `\`.
fn parent_of(path: &str) -> &str {
    match path.trim_end_matches('\\').rfind('\\') {
        Some(cut) if path[..cut].ends_with(':') => &path[..=cut],
        Some(cut) => &path[..cut],
        None => path,
    }
}

/// `path` relative to the folder `from`, or `path` whole when it is not
/// inside it.
fn relative_to<'a>(path: &'a str, from: &str) -> &'a str {
    let from = from.trim_end_matches('\\');
    match path.get(..from.len()) {
        Some(start) if start.eq_ignore_ascii_case(from) && path[from.len()..].starts_with('\\') => {
            &path[from.len() + 1..]
        }
        _ => path,
    }
}

/// The Delete question (DESIGN.md: File operations): the one entry's name, or
/// the count, because Select all reaches rows the person never saw.
pub(crate) fn delete_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    sources: &[PathBuf],
) -> Question {
    let count = sources.len() as u64;
    let instruction = match sources {
        [one] => strings.format("file_operations.delete_name", &[name_of(&text_of(one))]),
        _ => strings.count("file_operations.delete_items", count, numbers, &[]),
    };
    Question {
        title: strings.text("file_operations.delete").to_owned(),
        instruction,
        body: QuestionBody::Text(strings.count("file_operations.delete_body", count, numbers, &[])),
        footer: None,
        confirm: strings.text("file_operations.delete_button").to_owned(),
        cancel: strings.text("button.cancel").to_owned(),
        guard: Guard::CancelFirst,
    }
}

/// The Exit question while `operations` run or wait (DESIGN.md: File
/// operations), or `None` while none does: the running ones stop and are
/// offered again as Interrupted, and the waiting ones, which are not saved,
/// never run. A prepared operation waits for its answer.
pub(crate) fn exit_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    operations: &[Operation],
) -> Option<Question> {
    let (mut running, mut waiting) = (0, 0);
    for operation in operations {
        match operation.state {
            State::Running(_) | State::Cancelling => running += 1,
            State::Preparing | State::Prepared(_) | State::Compared(_) | State::Waiting { .. } => {
                waiting += 1
            }
            State::Ended(_) | State::Interrupted => {}
        }
    }
    if running + waiting == 0 {
        return None;
    }
    let (instruction, body) = if running > 0 {
        let mut body = strings.count("file_operations.exit_body", running, numbers, &[]);
        if waiting > 0 {
            body.push(' ');
            body.push_str(&strings.count("file_operations.exit_waiting", waiting, numbers, &[]));
        }
        (
            strings.count("file_operations.exit", running, numbers, &[]),
            body,
        )
    } else {
        (
            strings.count("file_operations.exit_waiting_only", waiting, numbers, &[]),
            strings.text("file_operations.exit_waiting_body").to_owned(),
        )
    };
    Some(Question {
        title: without_access_key(strings.text("command.exit")),
        instruction,
        body: QuestionBody::Text(body),
        footer: None,
        confirm: strings.text("file_operations.exit_button").to_owned(),
        cancel: strings.text("button.cancel").to_owned(),
        guard: Guard::CancelFirst,
    })
}

/// What the failure dialog shows (DESIGN.md: File operations): each entry that
/// failed or was skipped, with its reason, and how many items were done, so the
/// person knows that the result is partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) title: String,
    pub(crate) instruction: String,
    pub(crate) body: String,
    /// Each entry's path and reason.
    pub(crate) rows: Vec<[String; 2]>,
}

/// The failure dialog of an operation that ended as `outcome`, or `None`
/// when nothing failed or was skipped. A cancelled operation shows what
/// failed before the cancel. `windows_text` words a Windows error code.
pub(crate) fn failure(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    outcome: &Outcome,
    windows_text: &dyn Fn(u32) -> String,
) -> Option<Failure> {
    if !reports(outcome) {
        return None;
    }
    let rows: Vec<[String; 2]> = outcome
        .listed
        .iter()
        .map(|listed| {
            [
                text_of(&listed.path),
                reason_text(strings, listed.reason, windows_text),
            ]
        })
        .collect();
    let count = rows.len() as u64;
    let done = strings.count("file_operations.done", outcome.items, numbers, &[]);
    let (instruction, body) = match &outcome.end {
        End::Complete | End::Cancelled => (
            strings.count("file_operations.failed", count, numbers, &[]),
            joined_sentences([done, strings.text("file_operations.failed_body").to_owned()]),
        ),
        End::DiskFull(no_room) => (
            strings.text("file_operations.disk_full").to_owned(),
            joined_sentences([no_room_text(strings, numbers, no_room), done]),
        ),
        End::Refused(refusal) => (
            strings.text("file_operations.not_started").to_owned(),
            refusal_text(strings, numbers, *refusal, windows_text),
        ),
    };
    Some(Failure {
        title: strings.text(kind_of(request).name).to_owned(),
        instruction,
        body,
        rows,
    })
}

impl Report {
    pub(crate) fn summary(
        &self,
        strings: &Strings,
        windows_text: &dyn Fn(u32) -> String,
    ) -> String {
        match &self.outcome.end {
            End::Refused(Refusal::RecoveryUnavailable(code)) => strings.format(
                "file_operations.recovery_unavailable",
                &[&windows_text(*code)],
            ),
            End::Refused(Refusal::DestinationUnreadable(code)) => strings.format(
                "file_operations.refused_unreadable",
                &[&windows_text(*code)],
            ),
            _ => self.summary.clone(),
        }
    }

    pub(crate) fn cell(
        &self,
        strings: &Strings,
        row: usize,
        column: usize,
        windows_text: &dyn Fn(u32) -> String,
    ) -> String {
        let Some(listed) = self.outcome.listed.get(row) else {
            return String::new();
        };
        match column {
            0 => text_of(&listed.path),
            1 => reason_text(strings, listed.reason, windows_text),
            _ => String::new(),
        }
    }
}

fn reason_text(strings: &Strings, reason: Reason, windows_text: &dyn Fn(u32) -> String) -> String {
    match reason {
        Reason::Failed(code) | Reason::RecoveryFailed(code) | Reason::CleanupFailed(code) => {
            windows_text(code)
        }
        Reason::Skipped => strings.text("file_operations.reason_skipped").to_owned(),
        Reason::SynchronizationChanged => strings.text("sync.changed").to_owned(),
        Reason::SourceKept => strings
            .text("file_operations.reason_source_kept")
            .to_owned(),
        Reason::SourceChanged => strings
            .text("file_operations.reason_source_changed")
            .to_owned(),
    }
}

/// Whether an operation that ended as `outcome` has a [`failure`] dialog:
/// something failed or was skipped, the disk filled, or the engine refused
/// the operation.
pub(crate) fn reports(outcome: &Outcome) -> bool {
    !(matches!(outcome.end, End::Complete | End::Cancelled) && outcome.listed.is_empty())
}

fn joined_sentences(sentences: impl IntoIterator<Item = String>) -> String {
    sentences.into_iter().collect::<Vec<_>>().join(" ")
}

/// Which file found the destination disk full and how much room it needs,
/// with the free space only when it is less, as [`NoRoom::free`] says why.
fn no_room_text(strings: &Strings, numbers: &dyn Numbers, no_room: &NoRoom) -> String {
    let path = text_of(&no_room.path);
    let name = name_of(&path);
    let size = bytes(strings, numbers, no_room.size);
    match no_room.free.filter(|&free| free < no_room.size) {
        Some(free) => strings.format(
            "file_operations.disk_full_needs_free",
            &[name, &size, &bytes(strings, numbers, free)],
        ),
        None => strings.format("file_operations.disk_full_needs", &[name, &size]),
    }
}

/// Why the engine did not start an operation.
pub(crate) fn refusal_text(
    strings: &Strings,
    numbers: &dyn Numbers,
    refusal: Refusal,
    windows_text: &dyn Fn(u32) -> String,
) -> String {
    match refusal {
        Refusal::SynchronizationRootsOverlap => strings.text("sync.overlap").to_owned(),
        Refusal::SynchronizationChanged => strings.text("sync.changed").to_owned(),
        Refusal::ComparisonFailed(code) => strings.format("sync.failed", &[&windows_text(code)]),
        Refusal::SourceUnavailable => strings
            .text("file_operations.source_unavailable")
            .to_owned(),
        Refusal::RecoveryUnavailable(code) => strings.format(
            "file_operations.recovery_unavailable",
            &[&windows_text(code)],
        ),
        Refusal::DestinationInsideSource => {
            strings.text("file_operations.refused_inside").to_owned()
        }
        Refusal::DestinationMissing => strings.text("file_operations.refused_missing").to_owned(),
        Refusal::DestinationUnreadable(code) => {
            strings.format("file_operations.refused_unreadable", &[&windows_text(code)])
        }
        Refusal::TooLittleSpace { needed, free } => strings.format(
            "file_operations.refused_space",
            &[
                &bytes(strings, numbers, needed),
                &bytes(strings, numbers, free),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::tests::{English, english};
    use gozler_fileops::Destination;

    /// After Hide the middle group's line is the only sign that an
    /// operation finished, so it counts what the operation did, apart from
    /// what failed or was skipped (DESIGN.md: File operations).
    #[test]
    fn an_ended_operation_counts_what_it_did_and_what_failed() {
        let ended = StatusOperation::Ended {
            id: OperationId::for_tests(1),
            request: Arc::new(Request::Copy {
                policy: gozler_fileops::Policy::default(),
                sources: vec![PathBuf::from(r"C:\f\a"), PathBuf::from(r"C:\f\b")],
                destination: Destination::Folder(PathBuf::from(r"D:\Backup")),
            }),
            outcome: Arc::new(Outcome {
                end: End::Complete,
                items: 3_311,
                completed: 3_310,
                succeeded: false,
                bytes: 0,
                listed: vec![Listed {
                    path: PathBuf::from(r"C:\f\a\x"),
                    reason: Reason::Failed(5),
                }],
                ring: RingCounts::default(),
            }),
            numbered: false,
        };
        assert_eq!(
            status_line(&english(), &English, &ended),
            (
                r"Copied 3,310 items to D:\Backup · 1 failed or skipped".to_owned(),
                None
            )
        );
    }

    /// A full disk names the file that did not fit, so a copy that stops
    /// says why the free space the panel shows was not enough. A free space
    /// read after other files' partial copies were deleted can exceed the
    /// file's size, and "only" would then be false.
    #[test]
    fn a_full_disk_names_the_file_and_the_room_it_needs() {
        let body = |free| {
            let outcome = Outcome {
                end: End::DiskFull(NoRoom {
                    path: PathBuf::from(r"Y:\backup\image.mrimg"),
                    size: 198_572_310_960,
                    free,
                }),
                items: 2,
                completed: 2,
                succeeded: false,
                bytes: 0,
                listed: Vec::new(),
                ring: RingCounts::default(),
            };
            let request = Request::Copy {
                policy: gozler_fileops::Policy::default(),
                sources: vec![PathBuf::from(r"Y:\backup")],
                destination: Destination::Folder(PathBuf::from(r"C:\b")),
            };
            failure(&english(), &English, &request, &outcome, &|_| String::new())
                .expect("a full disk reports")
                .body
        };
        assert_eq!(
            body(Some(117_706_141_696)),
            "image.mrimg needs 184 GB, but the destination has only 109 GB free. \
             2 items are done."
        );
        assert_eq!(
            body(Some(200 << 30)),
            "image.mrimg needs 184 GB. 2 items are done."
        );
    }

    /// `F6` renames by moving the entry to a new name in its own folder, and
    /// the window has no other rename, so only that move can read as one.
    /// A cancelled rename did nothing, and Keep both may have written a
    /// numbered name that the window does not know, so neither line names
    /// the new name.
    #[test]
    fn a_move_to_a_new_name_in_its_own_folder_is_a_rename() {
        let move_to = |folder: &str| {
            Arc::new(Request::Move {
                policy: gozler_fileops::Policy::default(),
                sources: vec![PathBuf::from(r"C:\f\report.pdf")],
                destination: Destination::Named {
                    folder: PathBuf::from(folder),
                    name: "report-old.pdf".into(),
                },
            })
        };
        let ended = |folder, end, numbered| StatusOperation::Ended {
            id: OperationId::for_tests(1),
            request: move_to(folder),
            outcome: Arc::new(Outcome {
                succeeded: end == End::Complete,
                end,
                items: 1,
                completed: 1,
                bytes: 0,
                listed: Vec::new(),
                ring: RingCounts::default(),
            }),
            numbered,
        };
        let line = |folder, end, numbered| {
            status_line(&english(), &English, &ended(folder, end, numbered)).0
        };
        assert_eq!(
            line(r"C:\F\", End::Complete, false),
            "Renamed report.pdf to report-old.pdf"
        );
        assert_eq!(line(r"C:\f", End::Complete, true), "Renamed report.pdf");
        assert_eq!(line(r"C:\f", End::Cancelled, false), "");
        // The new path joins with the OS's separator, which is `\` only on
        // Windows, where the window runs.
        if cfg!(windows) {
            assert_eq!(
                line(r"D:\g", End::Complete, false),
                r"Moved 1 item to D:\g\report-old.pdf"
            );
        }
    }

    /// An operation stays in the queue until the window reports it, and a
    /// delete or a move also after its report while a list keeps it. The
    /// middle group shows the operation that ended last: the one not yet
    /// reported, else the one reported last, whatever their ids.
    #[test]
    fn the_status_shows_the_operation_that_ended_last() {
        let outcome = Arc::new(Outcome {
            end: End::Complete,
            items: 1,
            completed: 1,
            succeeded: true,
            bytes: 0,
            listed: Vec::new(),
            ring: RingCounts::default(),
        });
        let request = Arc::new(Request::Delete {
            sources: vec![PathBuf::from(r"C:\f\a")],
        });
        let queued = |id| Operation {
            id: OperationId::for_tests(id),
            request: Arc::clone(&request),
            folder: SourcesFolder::Pending,
            state: State::Ended(Arc::clone(&outcome)),
        };
        let last = |id| {
            (
                OperationId::for_tests(id),
                Completion::of(&request, &outcome, false),
            )
        };
        let shown = |queued_id, reported: &[u64], ended| {
            let reported = reported.iter().map(|&id| OperationId::for_tests(id));
            StatusOperation::of(
                &[queued(queued_id)],
                Some(&last(ended)),
                &reported.collect(),
                &HashSet::new(),
            )
            .map(|status| status.id())
        };
        assert_eq!(shown(2, &[], 1), Some(OperationId::for_tests(2)));
        assert_eq!(shown(7, &[7], 5), Some(OperationId::for_tests(5)));
    }

    /// A recovery-file error does not undo a successfully copied item.
    #[test]
    fn a_recovery_error_preserves_completed_item_counts() {
        let request = Request::Delete {
            sources: vec![r"C:\f\a".into()],
        };
        let outcome = Outcome::new(
            End::Complete,
            1,
            0,
            vec![Listed {
                path: "operations.json".into(),
                reason: Reason::RecoveryFailed(5),
            }],
            RingCounts::default(),
        );
        assert_eq!(
            completion_line(
                &english(),
                &English,
                &Completion::of(&request, &outcome, false)
            ),
            "Deleted 1 item · 1 failed or skipped"
        );
    }

    #[test]
    fn the_speed_counts_the_last_seconds_once_a_second_has_run() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut meter = Meter::default();
        meter.record(at(0), 0, 0);
        meter.record(at(500), 50, 5_000);
        assert_eq!(meter.speed(), None);
        meter.record(at(1_000), 100, 10_000);
        assert_eq!(
            meter.speed(),
            Some(Speed {
                items: 100.0,
                bytes: 10_000.0
            })
        );
        // Ten seconds on, the first samples no longer count: 200 items in
        // the last 5 seconds.
        for second in 2..=10 {
            meter.record(at(second * 1_000), 100 + (second - 1) * 40, 0);
        }
        assert_eq!(meter.speed().map(|speed| speed.items), Some(40.0));
    }

    /// The bytes done include the passed files' bytes, and neither count
    /// shows more than gozlerd's total, which a delete can outrun.
    #[test]
    fn done_counts_passed_bytes_and_never_exceeds_the_total() {
        let running = |items, bytes, passed| Running {
            phase: gozler_fileops::RunningPhase::Executing,
            items,
            bytes,
            passed,
            total: Some(Total {
                items: 10_000,
                bytes: 100,
            }),
            renames: false,
            paused: false,
            yielded: false,
            current: None,
        };
        let done = |running: Running| done_of(&running).map(|(of, _)| (of.items, of.bytes));
        assert_eq!(done(running(10, 10, 80)), Some((10, 90)));
        assert_eq!(done(running(10_050, 90, 30)), Some((10_000, 100)));
    }

    #[test]
    fn synchronization_progress_includes_pending_mirror_deletions() {
        let total = Some(Total {
            items: 2,
            bytes: 100,
        });
        let mut operation = Operation {
            id: OperationId::for_tests(1),
            request: Arc::new(Request::Synchronize {
                source: PathBuf::from(r"C:\source"),
                destination: PathBuf::from(r"D:\backup"),
                mirror: true,
            }),
            folder: SourcesFolder::Shared(r"C:\source".into()),
            state: State::Waiting {
                on: OperationId::for_tests(2),
                reason: gozler_fileops::Wait::Disk,
                total,
            },
        };
        let mut batch = batch::Batch::default();
        batch.record(std::slice::from_ref(&operation));
        assert_eq!(batch.bar(), Bar::Measured(Measure { done: 0, total: 2 }));
        let running = Running {
            phase: gozler_fileops::RunningPhase::Synchronizing,
            items: 1,
            bytes: 100,
            passed: 0,
            total,
            renames: false,
            paused: false,
            yielded: false,
            current: Some(gozler_fileops::Current {
                source: PathBuf::from(r"C:\source\a.txt"),
                destination: Some(PathBuf::from(r"D:\backup\a.txt")),
                done: 100,
                size: 100,
            }),
        };
        operation.state = State::Running(running.clone());
        batch.record(std::slice::from_ref(&operation));
        assert_eq!(batch.bar(), Bar::Measured(Measure { done: 1, total: 2 }));
        let card = card(&english(), &English, &operation, &running, None);
        assert_eq!(
            card.file_bar,
            Some(Bar::Measured(Measure {
                done: 100,
                total: 100
            }))
        );
    }

    #[test]
    fn a_file_is_named_relative_to_the_from_folder() {
        assert_eq!(
            relative_to(r"C:\Photos\2026\a.jpg", r"C:\photos"),
            r"2026\a.jpg"
        );
        assert_eq!(relative_to(r"C:\a.txt", r"C:\"), "a.txt");
        assert_eq!(relative_to(r"D:\a.txt", r"C:\Photos"), r"D:\a.txt");
    }

    #[test]
    fn a_waiting_row_can_be_promoted_before_running_work() {
        let id = OperationId::for_tests;
        let row = |n, phase| Row {
            id: id(n),
            glyph: Glyph::Copy,
            title: RowLine::Plain(String::new()),
            detail: RowLine::Plain(String::new()),
            phase,
        };
        let rows = [
            row(1, RowPhase::Running { paused: false }),
            row(2, RowPhase::Waiting),
            row(3, RowPhase::Waiting),
            row(4, RowPhase::Waiting),
            row(5, RowPhase::Interrupted),
        ];
        assert_eq!(landing(&rows, 0), (0, Some(id(1))));
        assert_eq!(landing(&rows, 3), (3, Some(id(4))));
        assert_eq!(landing(&rows, 5), (4, None));
        assert!(stays_in_place(&rows, id(3), Some(id(3))));
        assert!(stays_in_place(&rows, id(3), Some(id(4))));
        assert!(stays_in_place(&rows, id(4), None));
        assert!(!stays_in_place(&rows, id(3), Some(id(2))));
        assert_eq!(move_step(&rows, id(3), true), Some(Some(id(2))));
        assert_eq!(move_step(&rows, id(3), false), Some(None));
        assert_eq!(move_step(&rows, id(2), false), Some(Some(id(4))));
        assert_eq!(move_step(&rows, id(2), true), Some(Some(id(1))));
        assert_eq!(move_step(&rows, id(4), false), None);
    }

    /// A title wider than its tile wraps and hides the row's state on the
    /// second line, so the fitted title never exceeds its width, and a
    /// short path stays whole while the long one gives way.
    #[test]
    fn a_row_title_fits_its_width_and_keeps_the_short_path() {
        let strings = english();
        let width_of = |text: &str| Px(text.chars().count() as i32);
        // Keeps the start and the end, as Windows shortens a path.
        let shorten = |path: &str, width: Px| {
            let chars: Vec<char> = path.chars().collect();
            let keep = usize::try_from(width.0).unwrap_or(0);
            if chars.len() <= keep {
                return path.to_owned();
            }
            let head = keep.saturating_sub(1) / 2;
            let tail = keep.saturating_sub(1) - head;
            let mut text: String = chars[..head].iter().collect();
            text.push('…');
            text.extend(&chars[chars.len() - tail..]);
            text
        };
        let title = RowLine::Paths {
            key: "file_operations.row_copy",
            paths: vec![
                r"D:\Photos\2026\Summer\Beach".to_owned(),
                r"V:\Backup".to_owned(),
            ],
            after: String::new(),
        };
        let whole = strings.format(
            "file_operations.row_copy",
            &[r"D:\Photos\2026\Summer\Beach", r"V:\Backup"],
        );
        assert_eq!(title.fitted(&strings, Px(80), &width_of, &shorten), whole);
        let width = Px(whole.chars().count() as i32 - 16);
        let fitted = title.fitted(&strings, width, &width_of, &shorten);
        assert!(width_of(&fitted) <= width, "{fitted}");
        assert!(
            fitted.contains(r"V:\Backup") && fitted.contains('…'),
            "{fitted}"
        );
    }
}
