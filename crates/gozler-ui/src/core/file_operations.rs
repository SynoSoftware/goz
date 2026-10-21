//! The File operations window's decisions (file-manager-spec.md: File
//! operations, step 5): which operation the card shows, every text of the
//! card and of the queue list, and the speed, which the window works out
//! from the counts it polls, because the engine reports counts only.
//!
//! The engine's queue owns the operations, their order and which ones wait;
//! this module only words what the queue reports.

use super::captions::{Guard, Question, QuestionBody};
use super::commands::{Mark, MenuNode};
use super::glyph::Glyph;
use super::layout::{Dpi, Px, Rect};
use super::panel::same_folder;
use super::text::{Numbers, Strings, bytes, name_of, without_access_key};
use gozler_fileops::{
    Choice, Destination, End, Listed, OperationId, Outcome, Prepared, Reason, Refusal, Request,
    RingCounts, Running, State, Total,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// The time left for `remaining` at `per_second`, as "32 min left", or
/// `None` while nothing moves.
pub(crate) fn time_left(strings: &Strings, remaining: u64, per_second: f64) -> Option<String> {
    if per_second < 1.0 {
        return None;
    }
    let seconds = (remaining as f64 / per_second).ceil() as u64;
    let (hours, minutes) = (seconds / 3600, seconds % 3600 / 60);
    Some(match (hours, minutes) {
        (0, 0) => strings.format("file_operations.left_seconds", &[&seconds.to_string()]),
        (0, _) => {
            // Rounded up, so "1 min left" never shows for 90 seconds.
            let minutes = seconds.div_ceil(60);
            strings.format("file_operations.left_minutes", &[&minutes.to_string()])
        }
        (hours, minutes) => strings.format(
            "file_operations.left_hours",
            &[&hours.to_string(), &minutes.to_string()],
        ),
    })
}

/// A speed in bytes, as "5.10 MB/s".
pub(crate) fn bytes_per_second(strings: &Strings, numbers: &dyn Numbers, speed: f64) -> String {
    strings.format(
        "file_operations.per_second",
        &[&bytes(strings, numbers, speed as u64)],
    )
}

/// What the window shows: its title, the card of the running operation,
/// and the queue list's rows, in the queue's order.
#[derive(Debug, PartialEq)]
pub(crate) struct View {
    pub(crate) title: String,
    /// `None` while no operation runs.
    pub(crate) card: Option<Card>,
    pub(crate) rows: Vec<Row>,
    /// The header over the list, such as "Queue · 3 waiting".
    pub(crate) queue: String,
}

/// The running operation the card shows: the first one the queue runs.
#[derive(Debug, PartialEq)]
pub(crate) struct Card {
    pub(crate) id: OperationId,
    pub(crate) glyph: Glyph,
    /// "Copy", or "Copy · paused".
    pub(crate) title: String,
    /// The time left and the speeds, "Calculating…" before the first
    /// speed, or "Paused".
    pub(crate) status: String,
    pub(crate) paused: bool,
    /// "Copying" before the current file.
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
    pub(crate) file_bar: Option<Measure>,
    pub(crate) total_bar: Bar,
    /// "3,812 of 31,233 files", or "6,240 deleted".
    pub(crate) items: String,
    /// "1.11 GB of 9.27 GB", or empty when no bytes move.
    pub(crate) bytes: String,
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

/// The whole operation's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bar {
    /// With its percentage beside it.
    Measured(Measure),
    /// A delete, which never knows its total.
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
    /// Running beside the card's operation.
    Running {
        paused: bool,
    },
    /// Only a waiting operation moves in the queue.
    Waiting,
    /// Preparing, or prepared and waiting for the person's answer.
    Starting,
    Interrupted,
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
}

impl QueueAction {
    const ALL: [QueueAction; 7] = [
        QueueAction::Pause,
        QueueAction::Resume,
        QueueAction::Cancel,
        QueueAction::MoveUp,
        QueueAction::MoveDown,
        QueueAction::Continue,
        QueueAction::Dismiss,
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
        }
    }

    pub(crate) fn glyph(self) -> Glyph {
        match self {
            QueueAction::Pause => Glyph::Pause,
            QueueAction::Resume | QueueAction::Continue => Glyph::Play,
            QueueAction::Cancel | QueueAction::Dismiss => Glyph::Cancel,
            QueueAction::MoveUp => Glyph::ChevronUp,
            QueueAction::MoveDown => Glyph::ChevronDown,
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

/// The waiting rows' operations, in the queue's order.
fn waiting_ids(rows: &[Row]) -> impl Iterator<Item = OperationId> + '_ {
    (rows.iter())
        .filter(|row| row.phase == RowPhase::Waiting)
        .map(|row| row.id)
}

/// Where a row dropped at `gap`, a place between the rows, lands: the gap
/// before the first waiting row from there, which it goes before, or else
/// the gap after the last waiting row, with `None` for the end of the
/// waiting operations. The queue lists the running operations first and
/// the waiting ones next, and moves only the waiting ones.
pub(crate) fn landing(rows: &[Row], gap: usize) -> (usize, Option<OperationId>) {
    let waiting = |row: &Row| row.phase == RowPhase::Waiting;
    match (rows.iter().enumerate().skip(gap)).find(|(_, row)| waiting(row)) {
        Some((index, row)) => (index, Some(row.id)),
        None => {
            let after = (rows.iter().rposition(waiting)).map_or(gap, |last| last + 1);
            (after, None)
        }
    }
}

/// Whether the waiting row `id`, put before `before`, stays where it is:
/// just before itself, or before the waiting row after it.
pub(crate) fn stays_in_place(rows: &[Row], id: OperationId, before: Option<OperationId>) -> bool {
    let next = waiting_ids(rows)
        .skip_while(|waiting| *waiting != id)
        .nth(1);
    before == Some(id) || before == next
}

/// The place before which Move up or Move down puts the waiting row `id`,
/// or `None` when it is first or last among the waiting rows already. The
/// inner `None` is the end of the queue.
pub(crate) fn move_step(rows: &[Row], id: OperationId, up: bool) -> Option<Option<OperationId>> {
    let waiting: Vec<OperationId> = waiting_ids(rows).collect();
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
        Request::Move { .. } => Glyph::MoveToFolder,
        Request::Delete { .. } => Glyph::Delete,
        Request::NewFolder { .. } => Glyph::Folder,
    }
}

/// The entry and its new name when `request` moves one entry to a new name
/// in its own folder, which renames it, as `F6` with a new name does
/// (file-manager-spec.md: Copy and Move).
fn renamed(request: &Request) -> Option<(String, String)> {
    let Request::Move {
        sources,
        destination: Destination::Named { folder, name },
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
pub(crate) const ROW_GLYPHS: [Glyph; 5] = [
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

fn ends_of(strings: &Strings, operation: &Operation) -> Ends {
    let entries = |sources: &[PathBuf]| match sources {
        [one] => text_of(one),
        _ => folder_or(
            strings,
            operation.folder,
            "file_operations.several_folders_inline",
        ),
    };
    if let Some((path, name)) = renamed(operation.request) {
        let to = joined(parent_of(&path), &name);
        return Ends {
            from: path,
            to: Some(to),
        };
    }
    match operation.request {
        Request::Copy {
            sources,
            destination,
        }
        | Request::Move {
            sources,
            destination,
        } => Ends {
            from: entries(sources),
            to: Some(text_of(&destination.path())),
        },
        Request::Delete { sources } => Ends {
            from: entries(sources),
            to: None,
        },
        Request::NewFolder { path } => Ends {
            from: text_of(path),
            to: None,
        },
    }
}

/// The folder that holds every source of a copy, a move or a delete, also
/// of one; `None` when they are on different drives or servers, and for a
/// rename and a new folder.
pub(crate) fn sources_folder(request: &Request) -> Option<String> {
    match request {
        _ if renamed(request).is_some() => None,
        Request::Copy { sources, .. }
        | Request::Move { sources, .. }
        | Request::Delete { sources } => {
            common_folder(sources.iter().map(|path| path.to_string_lossy()))
        }
        Request::NewFolder { .. } => None,
    }
}

/// `folder`, or the text `several` when the sources are on different drives
/// or servers: "Several folders" on its own, or "several folders" inside a
/// sentence.
fn folder_or(strings: &Strings, folder: Option<&str>, several: &str) -> String {
    folder.unwrap_or_else(|| strings.text(several)).to_owned()
}

/// Where the card says an operation reads and writes, which stays the same
/// for the whole operation while the file line names each file: as
/// [`ends_of`] says, except that a copy, a move and a delete read from the
/// folder that holds their entries, also for one.
fn card_ends(strings: &Strings, operation: &Operation) -> Ends {
    let from = || folder_or(strings, operation.folder, "file_operations.several_folders");
    match operation.request {
        Request::Copy { destination, .. } | Request::Move { destination, .. } => Ends {
            from: from(),
            to: Some(text_of(&destination.path())),
        },
        Request::Delete { .. } => Ends {
            from: from(),
            to: None,
        },
        _ => ends_of(strings, operation),
    }
}

/// An operation as the window reads it from the queue.
pub(crate) struct Operation<'a> {
    pub(crate) id: OperationId,
    pub(crate) request: &'a Request,
    pub(crate) state: &'a State,
    /// [`sources_folder`] of `request`, which the window works out once per
    /// operation, because the sources never change, and reading every one
    /// of a large operation's on each refresh stalls the window.
    pub(crate) folder: Option<&'a str>,
}

/// The window's view of `operations`, in the queue's order, with the speeds
/// that `meters` worked out for the running ones.
pub(crate) fn view(
    strings: &Strings,
    numbers: &dyn Numbers,
    operations: &[Operation],
    meters: &HashMap<OperationId, Meter>,
) -> View {
    let speed = |id| meters.get(&id).and_then(Meter::speed);
    let card_id = operations
        .iter()
        .find(|operation| matches!(operation.state, State::Running(_)))
        .map(|operation| operation.id);
    let card = operations
        .iter()
        .find_map(|operation| match &operation.state {
            State::Running(running) if Some(operation.id) == card_id => Some(card(
                strings,
                numbers,
                operation,
                running,
                speed(operation.id),
            )),
            _ => None,
        });
    let rows: Vec<Row> = operations
        .iter()
        .filter(|operation| Some(operation.id) != card_id)
        .filter_map(|operation| row(strings, numbers, operation, operations, speed(operation.id)))
        .collect();
    let title = match &card {
        Some(card) => {
            let done = match card.total_bar {
                Bar::Measured(measure) => {
                    strings.format("file_operations.percent", &[&measure.percent().to_string()])
                }
                Bar::Moving => card.items.clone(),
            };
            strings.format("file_operations.title_running", &[&done, &card.title])
        }
        None => strings.text("file_operations.title").to_owned(),
    };
    // A row that runs beside the card's operation or was interrupted does
    // not wait, and its own line says what it does.
    let waiting = rows
        .iter()
        .filter(|row| matches!(row.phase, RowPhase::Waiting | RowPhase::Starting))
        .count() as u64;
    let queue = if waiting == 0 {
        strings.text("file_operations.queue_none").to_owned()
    } else {
        strings.count("file_operations.queue", waiting, numbers, &[])
    };
    View {
        title,
        card,
        queue,
        rows,
    }
}

/// Whether a running operation's progress is in bytes: a copy or a move
/// across volumes with data to move. A move by rename and a copy of empty
/// files count their entries.
fn moves_bytes(running: &Running) -> bool {
    !running.renames && running.total.is_some_and(|total| total.bytes > 0)
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
pub(crate) fn taskbar(operations: &[gozler_fileops::Operation]) -> Taskbar {
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
        return Taskbar::Moving;
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
    if running.iter().all(|r| r.paused) {
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
    let kind = kind_of(operation.request);
    let ends = card_ends(strings, operation);
    let measure = measure_of(running);
    let moves_bytes = moves_bytes(running);
    let mut title = strings.text(kind.name).to_owned();
    if running.paused {
        title = strings.format("file_operations.paused_title", &[&title]);
    }
    // The line always holds a text, as File Explorer's does, so the card
    // never changes its shape when the first speed arrives.
    let status = if running.paused {
        strings.text("file_operations.paused").to_owned()
    } else {
        speed
            .map(|speed| status(strings, numbers, running, measure, moves_bytes, speed))
            .filter(|status| !status.is_empty())
            .unwrap_or_else(|| strings.text("file_operations.calculating").to_owned())
    };
    let (file, file_tip, file_bar) = match &running.current {
        Some(current) => {
            let source = text_of(&current.source);
            let file = relative_to(&source, &ends.from).to_owned();
            let tip = match &current.destination {
                Some(destination) => format!("{source}\n→ {}", text_of(destination)),
                None => source.clone(),
            };
            let bar = moves_bytes.then_some(Measure {
                done: current.done,
                total: current.size,
            });
            (file, tip, bar)
        }
        // A delete and a move by rename report no current file, so the
        // line names their one selected entry, as it does before a copy's
        // first file.
        None => {
            let one = match operation.request {
                Request::Copy { sources, .. }
                | Request::Move { sources, .. }
                | Request::Delete { sources } => match sources.as_slice() {
                    [one] => text_of(one),
                    _ => String::new(),
                },
                Request::NewFolder { .. } => String::new(),
            };
            (
                relative_to(&one, &ends.from).to_owned(),
                one,
                moves_bytes.then_some(Measure { done: 0, total: 0 }),
            )
        }
    };
    let (items, bytes_text) = match done_of(running) {
        Some((done, total)) => (
            strings.count(
                "file_operations.items_of",
                total.items,
                numbers,
                &[&numbers.integer(done.items)],
            ),
            if moves_bytes {
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
            items_done(strings, numbers, operation.request, running.items),
            String::new(),
        ),
    };
    Card {
        id: operation.id,
        glyph: glyph_of(operation.request),
        title,
        status,
        paused: running.paused,
        doing: strings.text(kind.doing).to_owned(),
        file,
        file_tip,
        from: ends.from,
        to: ends.to,
        file_bar,
        total_bar: measure.map_or(Bar::Moving, Bar::Measured),
        items,
        bytes: bytes_text,
    }
}

/// The card's status line: the time left, the bytes per second and the
/// items per second, as far as the operation measures them.
fn status(
    strings: &Strings,
    numbers: &dyn Numbers,
    running: &Running,
    measure: Option<Measure>,
    moves_bytes: bool,
    speed: Speed,
) -> String {
    let left = measure.and_then(|measure| {
        let remaining = measure.total.saturating_sub(measure.done);
        let per_second = if moves_bytes {
            speed.bytes
        } else {
            speed.items
        };
        time_left(strings, remaining, per_second)
    });
    let per_second = moves_bytes.then(|| bytes_per_second(strings, numbers, speed.bytes));
    // The bytes per second say all about one file.
    let items = running
        .total
        .is_none_or(|total| total.items > 1)
        .then(|| items_per_second(strings, numbers, speed));
    joined_parts(strings, left.into_iter().chain(per_second).chain(items))
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
) -> Option<Row> {
    let ends = ends_of(strings, operation);
    let kind = kind_of(operation.request);
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
                None => items_done(strings, numbers, operation.request, running.items),
            };
            let detail = if running.paused {
                joined_parts(
                    strings,
                    [strings.text("file_operations.paused").to_owned(), progress],
                )
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
            let size = size_of_total(strings, numbers, operation.request, *total);
            let detail = match operations.iter().find(|other| other.id == *on) {
                Some(other) => {
                    let ends = ends_of(strings, other);
                    let place = match (other.request, ends.to) {
                        (Request::Copy { .. } | Request::Move { .. }, Some(to)) => to,
                        _ => ends.from,
                    };
                    RowLine::Paths {
                        key: kind_of(other.request).waiting_for,
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
        State::Prepared(prepared) => {
            let size = match prepared.total {
                Some(total) => size_of_total(strings, numbers, operation.request, Some(total)),
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
            let detail = match operation.request {
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
        State::Ended(_) => return None,
    };
    Some(Row {
        id: operation.id,
        glyph: glyph_of(operation.request),
        title: RowLine::Paths {
            key: kind.row,
            paths: [Some(ends.from), ends.to].into_iter().flatten().collect(),
            after: String::new(),
        },
        detail,
        phase,
    })
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
        (None, Request::NewFolder { .. }) => String::new(),
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
        Request::NewFolder { .. } => return String::new(),
    };
    strings.count(key, items, numbers, &[])
}

/// The operation that ended last, with its request and its outcome.
pub(crate) type LastEnded = (OperationId, Arc<Request>, Arc<Outcome>);

/// The file operation the status bar's middle group shows in both modes:
/// the newest running one, else the one that ended last, which stays until
/// another starts, because after Hide it is the only sign that the
/// operation finished (file-manager-spec.md: File operations, steps 5 and
/// 6).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StatusOperation {
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

impl StatusOperation {
    pub(crate) fn id(&self) -> OperationId {
        match self {
            StatusOperation::Running { id, .. } | StatusOperation::Ended { id, .. } => *id,
        }
    }

    /// The newest of the running `operations`, else the newest that ended
    /// and is not `reported`, such as one whose failures the window is about
    /// to report, else `ended`, the one reported last. A delete or a move
    /// stays in the queue after its report while a list keeps it, so only
    /// `ended` tells which operation ended last. Each operation in
    /// `numbered` was answered with Keep both.
    pub(crate) fn of(
        operations: &[gozler_fileops::Operation],
        ended: Option<&LastEnded>,
        reported: &HashSet<OperationId>,
        numbered: &HashSet<OperationId>,
    ) -> Option<StatusOperation> {
        let newest = |running: bool| {
            (operations.iter())
                .filter(|operation| match operation.state {
                    State::Running(_) => running,
                    State::Ended(_) => !running && !reported.contains(&operation.id),
                    _ => false,
                })
                .max_by_key(|operation| operation.id)
        };
        let ended_status =
            |id, request: &Arc<Request>, outcome: &Arc<Outcome>| StatusOperation::Ended {
                id,
                request: Arc::clone(request),
                outcome: Arc::clone(outcome),
                numbered: numbered.contains(&id),
            };
        match newest(true).or_else(|| newest(false)) {
            Some(operation) => match &operation.state {
                State::Running(running) => Some(StatusOperation::Running {
                    id: operation.id,
                    request: Arc::clone(&operation.request),
                    running: running.clone(),
                }),
                State::Ended(outcome) => {
                    Some(ended_status(operation.id, &operation.request, outcome))
                }
                _ => None,
            },
            None => ended.map(|(id, request, outcome)| ended_status(*id, request, outcome)),
        }
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
    let paused = running
        .paused
        .then(|| strings.text("file_operations.paused").to_owned());
    let text = joined_parts(
        strings,
        [doing_line(strings, numbers, request), progress]
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
            sources: one(),
            destination: destination(),
        },
        Request::Move {
            sources: several(),
            destination: destination(),
        },
        Request::Delete { sources: several() },
        Request::Move {
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
                items: total,
                bytes: total,
                passed: 0,
                total: known,
                renames: false,
                paused: true,
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
                end,
                items: total,
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
        } => strings.format(
            "file_operations.status_copying",
            &[&what(sources), &text_of(&destination.path())],
        ),
        Request::Move {
            sources,
            destination,
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
    let cancelled = match outcome.end {
        End::Complete => false,
        End::Cancelled => true,
        End::DiskFull | End::Refused(_) => return String::new(),
    };
    let failed = (outcome.listed.iter())
        .filter(|listed| matches!(listed.reason, Reason::Failed(_) | Reason::Skipped))
        .count() as u64;
    let done = outcome.items.saturating_sub(failed);
    let count = |key: &str, to: &[&str]| strings.count(key, done, numbers, to);
    // A rename and a new folder are one step, which succeeded or failed;
    // cancelled, it did nothing, as the person chose, so it shows no line.
    if let Some((path, name)) = renamed(request) {
        return match (cancelled, failed, numbered) {
            (true, ..) => String::new(),
            (false, 0, false) => {
                strings.format("file_operations.ended_rename", &[name_of(&path), &name])
            }
            (false, 0, true) => {
                strings.format("file_operations.ended_rename_numbered", &[name_of(&path)])
            }
            (false, _, _) => strings.format("file_operations.rename_failed", &[name_of(&path)]),
        };
    }
    let line = match request {
        Request::Copy { destination, .. } | Request::Move { destination, .. } => {
            let moves = matches!(request, Request::Move { .. });
            let key = match (moves, cancelled) {
                (false, false) => "file_operations.ended_copy",
                (false, true) => "file_operations.cancelled_copy",
                (true, false) => "file_operations.ended_move",
                (true, true) => "file_operations.cancelled_move",
            };
            count(key, &[&text_of(&destination.path())])
        }
        Request::Delete { .. } if cancelled => count("file_operations.cancelled_delete", &[]),
        Request::Delete { .. } => count("file_operations.ended_delete", &[]),
        Request::NewFolder { .. } if cancelled => return String::new(),
        Request::NewFolder { path } => {
            let key = match failed {
                0 => "file_operations.ended_new_folder",
                _ => "file_operations.new_folder_failed",
            };
            return strings.format(key, &[&text_of(path)]);
        }
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

/// A folder's own name, or a drive's root whole, such as `C:\`.
fn folder_name(path: &str) -> &str {
    match name_of(path) {
        "" => path,
        name => name,
    }
}

/// The deepest folder that holds every path of `paths`, compared as Windows
/// compares names, ignoring case; `None` when they share no drive or
/// server. It allocates nothing for each path, because an operation can
/// have 100,000 sources.
fn common_folder(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Option<String> {
    let mut paths = paths.into_iter();
    let first = paths.next()?;
    let first = parent_of(first.as_ref());
    // How many of `first`'s parts every path shares.
    let mut common = first.split('\\').count();
    for path in paths {
        common = (first.split('\\').zip(parent_of(path.as_ref()).split('\\')))
            .take(common)
            .take_while(|(a, b)| same_name(a, b))
            .count();
    }
    let parts = || first.split('\\').take(common);
    // Two drives share nothing, and two servers share only the `\\` that
    // starts each path.
    if parts().all(str::is_empty) {
        return None;
    }
    let end = parts().map(str::len).sum::<usize>() + common - 1;
    let folder = &first[..end];
    Some(if folder.ends_with(':') {
        format!("{folder}\\")
    } else {
        folder.to_owned()
    })
}

/// Whether `a` and `b` are one name to Windows, which ignores case.
fn same_name(a: &str, b: &str) -> bool {
    a == b || (a.chars().flat_map(char::to_lowercase)).eq(b.chars().flat_map(char::to_lowercase))
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

/// A question with several answers, of which the person picks one or
/// cancels: the conflict question (file-manager-spec.md: File operations,
/// step 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Choices {
    pub(crate) title: String,
    pub(crate) instruction: String,
    pub(crate) body: String,
    /// Each answer a command link, in this order.
    pub(crate) answers: Vec<String>,
}

/// The conflict question's answers, in the order of its links.
const CONFLICT_CHOICES: [Choice; 3] = [Choice::Replace, Choice::Skip, Choice::KeepBoth];

/// Whether the person answers a [`conflict_question`] before the prepared
/// copy or move runs.
pub(crate) fn asks(request: &Request, prepared: &Prepared) -> bool {
    matches!(request, Request::Copy { .. } | Request::Move { .. })
        && (prepared.conflicts)
            .is_some_and(|found| found.files + found.merges + found.same_name > 0)
}

/// The choice of the conflict question's answer `index`.
pub(crate) fn conflict_choice(index: usize) -> Option<Choice> {
    CONFLICT_CHOICES.get(index).copied()
}

/// The conflict question of a copy or a move whose prepare found files at
/// the destination with the names it writes, selected folders that merge
/// into folders of their names, or selected entries that share a name;
/// `None` when it found none, so nobody is asked. As File Explorer's
/// question does, it names the folders by their own names, so a long path
/// never widens the dialog. Without a total from gozlerd it counts the
/// selected entries, as File Explorer does before it has counted. `folder`
/// is the request's [`sources_folder`].
pub(crate) fn conflict_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    prepared: &Prepared,
    folder: Option<&str>,
) -> Option<Choices> {
    let found = prepared.conflicts?;
    let (key, destination) = match request {
        Request::Copy { destination, .. } => ("file_operations.conflict_copying", destination),
        Request::Move { destination, .. } => ("file_operations.conflict_moving", destination),
        Request::Delete { .. } | Request::NewFolder { .. } => return None,
    };
    let from = folder_or(strings, folder, "file_operations.several_folders_inline");
    let to = text_of(&destination.path());
    let items = prepared
        .total
        .map_or(prepared.entries as u64, |total| total.items);
    let doing = strings.count(key, items, numbers, &[folder_name(&from), folder_name(&to)]);
    let mut found = [
        ("file_operations.conflicts", found.files),
        ("file_operations.merges", found.merges),
        ("file_operations.same_name", found.same_name),
    ]
    .into_iter()
    .filter(|&(_, count)| count > 0)
    .map(|(key, count)| strings.count(key, count, numbers, &[]));
    let instruction = found.next()?;
    let body = [doing]
        .into_iter()
        .chain(found)
        .collect::<Vec<_>>()
        .join("\n");
    Some(Choices {
        title: strings.text(kind_of(request).name).to_owned(),
        instruction,
        body,
        answers: [
            "file_operations.replace",
            "file_operations.skip",
            "file_operations.keep_both",
        ]
        .iter()
        .map(|key| strings.text(key).to_owned())
        .collect(),
    })
}

/// The Delete question (file-manager-spec.md: File operations, step 2): the
/// one entry's name, or the count, because Select all reaches rows the
/// person never saw.
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

/// The Exit question while `operations` run or wait (file-manager-spec.md:
/// File operations, step 8), or `None` while none does: the running ones
/// stop and are offered again as Interrupted, and the waiting ones, which
/// are not saved, never run. A prepared operation waits for its answer.
pub(crate) fn exit_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    operations: &[gozler_fileops::Operation],
) -> Option<Question> {
    let (mut running, mut waiting) = (0, 0);
    for operation in operations {
        match operation.state {
            State::Running(_) => running += 1,
            State::Preparing | State::Prepared(_) | State::Waiting { .. } => waiting += 1,
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

/// What the failure dialog shows (file-manager-spec.md: File operations,
/// step 7): each entry that failed or was skipped, with its reason, and how
/// many items were done, so the person knows that the result is partial.
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
            let reason = match listed.reason {
                Reason::Failed(code) => windows_text(code),
                Reason::Skipped => strings.text("file_operations.reason_skipped").to_owned(),
                Reason::SourceKept => strings
                    .text("file_operations.reason_source_kept")
                    .to_owned(),
                Reason::SourceChanged => strings
                    .text("file_operations.reason_source_changed")
                    .to_owned(),
            };
            [text_of(&listed.path), reason]
        })
        .collect();
    let count = rows.len() as u64;
    let done = strings.count("file_operations.done", outcome.items, numbers, &[]);
    let (instruction, body) = match outcome.end {
        End::Complete | End::Cancelled => (
            strings.count("file_operations.failed", count, numbers, &[]),
            joined_sentences([done, strings.text("file_operations.failed_body").to_owned()]),
        ),
        End::DiskFull => (strings.text("file_operations.disk_full").to_owned(), done),
        End::Refused(refusal) => (
            strings.text("file_operations.not_started").to_owned(),
            refusal_text(strings, numbers, refusal, windows_text),
        ),
    };
    Some(Failure {
        title: strings.text(kind_of(request).name).to_owned(),
        instruction,
        body,
        rows,
    })
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

/// Why the engine did not start an operation.
fn refusal_text(
    strings: &Strings,
    numbers: &dyn Numbers,
    refusal: Refusal,
    windows_text: &dyn Fn(u32) -> String,
) -> String {
    match refusal {
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

/// `dip` at `dpi`, rounded as the main window's layout rounds.
fn px(dip: i32, dpi: Dpi) -> Px {
    Px(dip).rescale(Dpi::new(96).expect("96 is not zero"), dpi)
}

/// The window's edges, and the card's inner margin.
const MARGIN: i32 = 11;
/// Between related controls, such as the card's lines.
const RELATED: i32 = 4;
/// Between groups, such as the card and the queue's header.
const GROUP: i32 = 11;
/// An icon button: the card's Pause and Cancel, and the queue's buttons.
const ICON_BUTTON: i32 = 28;
/// The card's icon of the operation's kind.
pub(crate) const CARD_ICON: i32 = 20;
/// The card's rounded corners, as Windows 11 rounds a card.
pub(crate) const CARD_CORNER: i32 = 8;
const FILE_BAR: i32 = 8;
const TOTAL_BAR: i32 = 15;
/// The queue list's icons, about as large as the card's, so they do not
/// outweigh a tile's two lines of text.
pub(crate) const ROW_ICON: i32 = 22;
/// The queue list is this many rows tall and scrolls the rest, so nothing
/// moves when operations come and go (DESIGN.md: Nothing moves by itself).
const LIST_ROWS: i32 = 4;
/// A row of the queue list: two lines and the tile's padding.
const ROW_PADDING: i32 = 12;
/// The narrowest window, which still shows a path's start and end.
const MIN_WIDTH: i32 = 360;
/// The window's width when it first opens.
pub(crate) const OPEN_WIDTH: i32 = 480;

/// What the layout measures: the fonts' line heights and the widths of the
/// captions that a longer translation widens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metrics {
    pub(crate) dpi: Dpi,
    /// A line of the message font.
    pub(crate) line: Px,
    /// A line of the card's title font.
    pub(crate) title_line: Px,
    /// The widest of "Copying" and its kind's words, "From" and "To".
    pub(crate) label_width: Px,
    /// The widest percentage, "100 %".
    pub(crate) percent_width: Px,
    /// Hide's width, which fits its icon and caption.
    pub(crate) hide_width: Px,
}

/// Where every control of the window goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) card: Rect,
    pub(crate) icon: Rect,
    pub(crate) title: Rect,
    pub(crate) status: Rect,
    pub(crate) pause: Rect,
    pub(crate) cancel: Rect,
    pub(crate) doing: Rect,
    pub(crate) file: Rect,
    pub(crate) file_bar: Rect,
    pub(crate) from_label: Rect,
    pub(crate) from: Rect,
    pub(crate) to_label: Rect,
    pub(crate) to: Rect,
    pub(crate) total_bar: Rect,
    pub(crate) percent: Rect,
    pub(crate) items: Rect,
    pub(crate) bytes: Rect,
    pub(crate) toggle: Rect,
    pub(crate) queue: Rect,
    pub(crate) resume: Rect,
    pub(crate) up: Rect,
    pub(crate) down: Rect,
    pub(crate) queue_cancel: Rect,
    /// Empty while the queue is collapsed.
    pub(crate) list: Rect,
    pub(crate) hint: Rect,
    pub(crate) hide: Rect,
}

impl Layout {
    /// Places the controls from the top of a client area `client_width`
    /// wide: the list [`LIST_ROWS`] rows tall while `queue_open`, the hint's
    /// line only while `hint_shown`, and Hide under them.
    pub(crate) fn compute(
        client_width: Px,
        m: Metrics,
        queue_open: bool,
        hint_shown: bool,
    ) -> Layout {
        let d = |dip| px(dip, m.dpi);
        let margin = d(MARGIN);
        let width = client_width - margin - margin;
        let button = d(ICON_BUTTON);
        let line = m.line + d(2);
        // Between the card's three lines, and between the file line and its
        // bar.
        let tight = Px(d(RELATED).0 / 2);
        let rect = |x: Px, y: Px, width: Px, height: Px| Rect {
            x,
            y,
            width: width.max(Px(0)),
            height,
        };
        let mut l = Layout::default();

        // The card: its title row, the three lines, the bar and the counts,
        // with a group's gap between them.
        let inner_x = margin + margin;
        let inner_width = width - margin - margin;
        let mut y = margin + margin;
        let heading = m.title_line + m.line;
        let row = button.max(heading);
        let centred = |height: Px| y + Px((row - height).0 / 2);
        // The icon's left edge lines up with the labels under it.
        l.icon = rect(inner_x, centred(d(CARD_ICON)), d(CARD_ICON), d(CARD_ICON));
        l.cancel = rect(
            inner_x + inner_width - button,
            centred(button),
            button,
            button,
        );
        l.pause = rect(l.cancel.x - button, l.cancel.y, button, button);
        let text_x = l.icon.right() + d(RELATED) + d(RELATED);
        let text_width = l.pause.x - d(RELATED) - text_x;
        l.title = rect(text_x, centred(heading), text_width, m.title_line);
        l.status = rect(text_x, l.title.bottom(), text_width, m.line);
        y = y + row + d(GROUP);
        let value_x = inner_x + m.label_width + d(GROUP);
        let value_width = inner_x + inner_width - value_x;
        l.doing = rect(inner_x, y, m.label_width, m.line);
        l.file = rect(value_x, y, value_width, m.line);
        y = y + m.line + tight;
        l.file_bar = rect(value_x, y, value_width, d(FILE_BAR));
        y = y + d(FILE_BAR) + tight;
        l.from_label = rect(inner_x, y, m.label_width, m.line);
        l.from = rect(value_x, y, value_width, m.line);
        y = y + m.line + tight;
        l.to_label = rect(inner_x, y, m.label_width, m.line);
        l.to = rect(value_x, y, value_width, m.line);
        y = y + m.line + d(GROUP);
        let percent_x = inner_x + inner_width - m.percent_width;
        l.total_bar = rect(inner_x, y, percent_x - d(RELATED) - inner_x, d(TOTAL_BAR));
        l.percent = rect(
            percent_x,
            y + Px((d(TOTAL_BAR) - m.line).0 / 2),
            m.percent_width,
            m.line,
        );
        y = y + d(TOTAL_BAR) + d(RELATED);
        // The items under the bar's start, and the bytes ending under its
        // end.
        let half = Px(l.total_bar.width.0 / 2);
        l.items = rect(inner_x, y, half, m.line);
        l.bytes = rect(inner_x + half, y, l.total_bar.width - half, m.line);
        y = y + m.line + margin;
        l.card = rect(margin, margin, width, y - margin);

        // The queue's header: the toggle, the count, then its buttons.
        y = l.card.bottom() + d(GROUP);
        l.toggle = rect(margin, y, button, button);
        l.queue_cancel = rect(margin + width - button, y, button, button);
        l.down = rect(l.queue_cancel.x - button, y, button, button);
        l.up = rect(l.down.x - button, y, button, button);
        l.resume = rect(l.up.x - button, y, button, button);
        let queue_x = l.toggle.right() + d(RELATED);
        l.queue = rect(
            queue_x,
            y + Px((button - line).0 / 2),
            l.resume.x - d(RELATED) - queue_x,
            line,
        );
        y = y + button;

        // The list and its hint, which the hint's width also measures while
        // it shows nothing, then Hide at the right.
        if queue_open {
            y = y + d(RELATED);
            let list_height = Px(Layout::row_height(m).0 * LIST_ROWS) + d(4);
            l.list = rect(margin, y, width, list_height);
            y = l.list.bottom();
        }
        let hint_height = if queue_open && hint_shown {
            line
        } else {
            Px(0)
        };
        l.hint = rect(margin, y + d(RELATED), width, hint_height);
        if hint_height > Px(0) {
            y = l.hint.bottom();
        }
        y = y + d(GROUP);
        l.hide = rect(
            margin + width - m.hide_width,
            y,
            m.hide_width,
            m.line + d(8),
        );
        l
    }

    /// The window's client height, which ends a margin under Hide.
    pub(crate) fn height(m: Metrics, queue_open: bool, hint_shown: bool) -> Px {
        let layout = Layout::compute(px(OPEN_WIDTH, m.dpi), m, queue_open, hint_shown);
        layout.hide.bottom() + px(MARGIN, m.dpi)
    }

    /// The narrowest client width.
    pub(crate) fn min_width(m: Metrics) -> Px {
        px(MIN_WIDTH, m.dpi)
    }

    /// A row of the queue list.
    pub(crate) fn row_height(m: Metrics) -> Px {
        Px(m.line.0 * 2).max(px(ROW_ICON, m.dpi)) + px(ROW_PADDING, m.dpi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::tests::{English, english};
    use gozler_fileops::{Conflicts, Destination};

    /// After Hide the middle group's line is the only sign that an
    /// operation finished, so it counts what the operation did, apart from
    /// what failed or was skipped (file-manager-spec.md: File operations,
    /// step 6).
    #[test]
    fn an_ended_operation_counts_what_it_did_and_what_failed() {
        let ended = StatusOperation::Ended {
            id: OperationId::for_tests(1),
            request: Arc::new(Request::Copy {
                sources: vec![PathBuf::from(r"C:\f\a"), PathBuf::from(r"C:\f\b")],
                destination: Destination::Folder(PathBuf::from(r"D:\Backup")),
            }),
            outcome: Arc::new(Outcome {
                end: End::Complete,
                items: 3_311,
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

    /// `F6` renames by moving the entry to a new name in its own folder, and
    /// the window has no other rename, so only that move can read as one.
    /// A cancelled rename did nothing, and Keep both may have written a
    /// numbered name that the window does not know, so neither line names
    /// the new name.
    #[test]
    fn a_move_to_a_new_name_in_its_own_folder_is_a_rename() {
        let move_to = |folder: &str| {
            Arc::new(Request::Move {
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
                end,
                items: 1,
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
        assert_eq!(
            line(r"D:\g", End::Complete, false),
            r"Moved 1 item to D:\g\report-old.pdf"
        );
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
            bytes: 0,
            listed: Vec::new(),
            ring: RingCounts::default(),
        });
        let request = Arc::new(Request::Delete {
            sources: vec![PathBuf::from(r"C:\f\a")],
        });
        let queued = |id| gozler_fileops::Operation {
            id: OperationId::for_tests(id),
            request: Arc::clone(&request),
            state: State::Ended(Arc::clone(&outcome)),
        };
        let last = |id| {
            (
                OperationId::for_tests(id),
                Arc::clone(&request),
                Arc::clone(&outcome),
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

    #[test]
    fn the_time_left_rounds_up_to_the_unit_it_shows() {
        let strings = english();
        let left = |remaining, speed| time_left(&strings, remaining, speed);
        let message = |key, values: &[&str]| Some(strings.format(key, values));
        assert_eq!(
            left(45, 1.0),
            message("file_operations.left_seconds", &["45"])
        );
        assert_eq!(
            left(90, 1.0),
            message("file_operations.left_minutes", &["2"])
        );
        assert_eq!(
            left(7_500, 1.0),
            message("file_operations.left_hours", &["2", "5"])
        );
        assert_eq!(left(100, 0.0), None);
    }

    /// The bytes done include the passed files' bytes, and neither count
    /// shows more than gozlerd's total, which a delete can outrun.
    #[test]
    fn done_counts_passed_bytes_and_never_exceeds_the_total() {
        let running = |items, bytes, passed| Running {
            items,
            bytes,
            passed,
            total: Some(Total {
                items: 10_000,
                bytes: 100,
            }),
            renames: false,
            paused: false,
            current: None,
        };
        let done = |running: Running| done_of(&running).map(|(of, _)| (of.items, of.bytes));
        assert_eq!(done(running(10, 10, 80)), Some((10, 90)));
        assert_eq!(done(running(10_050, 90, 30)), Some((10_000, 100)));
    }

    #[test]
    fn the_from_folder_holds_every_source() {
        assert_eq!(
            common_folder([r"C:\Photos\2026\a.jpg", r"C:\photos\2025\b.jpg"]).as_deref(),
            Some(r"C:\Photos")
        );
        assert_eq!(
            common_folder([r"C:\a.txt", r"C:\b\c.txt"]).as_deref(),
            Some(r"C:\")
        );
        assert_eq!(common_folder([r"C:\a.txt", r"D:\b.txt"]), None);
        assert_eq!(
            common_folder([r"\\one\share\a.txt", r"\\two\share\b.txt"]),
            None
        );
        assert_eq!(
            relative_to(r"C:\Photos\2026\a.jpg", r"C:\photos"),
            r"2026\a.jpg"
        );
        assert_eq!(relative_to(r"C:\a.txt", r"C:\"), "a.txt");
        assert_eq!(relative_to(r"D:\a.txt", r"C:\Photos"), r"D:\a.txt");
    }

    /// A full path in the conflict question once widened the dialog to
    /// 1,400 px, so the question names each folder by its own name, as
    /// File Explorer's does.
    #[test]
    fn the_conflict_question_names_the_folders_only() {
        let strings = english();
        let request = Request::Copy {
            sources: vec![
                PathBuf::from(r"C:\Users\user\Photos\2026\a.jpg"),
                PathBuf::from(r"C:\Users\user\Photos\2026\b.jpg"),
            ],
            destination: Destination::Folder(PathBuf::from(r"V:\Backup\Photos")),
        };
        let prepared = Prepared {
            entries: 2,
            total: Some(Total {
                items: 4_210,
                bytes: 1,
            }),
            conflicts: Some(Conflicts {
                files: 12,
                ..Conflicts::default()
            }),
        };
        let folder = sources_folder(&request);
        let question =
            conflict_question(&strings, &English, &request, &prepared, folder.as_deref())
                .expect("12 conflicts ask");
        assert!(!question.body.contains('\\'), "{}", question.body);
        assert!(question.body.contains("2026") && question.body.contains("Photos"));
    }

    /// Prepare reads no file inside a folder that merges, so a merge alone
    /// asks: without the question, nobody chose, and the run would skip
    /// every file of the same name inside it.
    #[test]
    fn a_merge_alone_asks() {
        let strings = english();
        let request = Request::Copy {
            sources: vec![PathBuf::from(r"C:\Users\user\Photos")],
            destination: Destination::Folder(PathBuf::from(r"V:\Backup")),
        };
        let prepared = Prepared {
            entries: 1,
            total: None,
            conflicts: Some(Conflicts {
                merges: 1,
                ..Conflicts::default()
            }),
        };
        assert!(conflict_question(&strings, &English, &request, &prepared, None).is_some());
    }

    /// The queue moves only waiting operations, after the running ones and
    /// before the interrupted ones, so a drop or Move up and Move down land
    /// among the waiting rows, and a drop that leaves a row where it is
    /// moves nothing.
    #[test]
    fn a_moved_row_lands_among_the_waiting_rows() {
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
        assert_eq!(landing(&rows, 0), (1, Some(id(2))));
        assert_eq!(landing(&rows, 3), (3, Some(id(4))));
        assert_eq!(landing(&rows, 5), (4, None));
        assert!(stays_in_place(&rows, id(3), Some(id(3))));
        assert!(stays_in_place(&rows, id(3), Some(id(4))));
        assert!(stays_in_place(&rows, id(4), None));
        assert!(!stays_in_place(&rows, id(3), Some(id(2))));
        assert_eq!(move_step(&rows, id(3), true), Some(Some(id(2))));
        assert_eq!(move_step(&rows, id(3), false), Some(None));
        assert_eq!(move_step(&rows, id(2), false), Some(Some(id(4))));
        assert_eq!(move_step(&rows, id(2), true), None);
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
