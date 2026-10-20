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
use super::text::{Numbers, Strings, bytes, name_of, without_access_key};
use gozler_fileops::{
    Choice, End, OperationId, Outcome, Prepared, Reason, Refusal, Request, Running, State, Total,
};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
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
    /// What the taskbar button shows for every running operation together.
    pub(crate) taskbar: Taskbar,
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
        Request::Copy { .. } => Glyph::Copy,
        Request::Move { .. } => Glyph::MoveToFolder,
        Request::Delete { .. } => Glyph::Delete,
        Request::Rename { .. } => Glyph::Rename,
        Request::NewFolder { .. } => Glyph::Folder,
    }
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
        Request::Rename { .. } => kind(
            "file_operations.rename",
            "file_operations.renaming",
            "file_operations.row_rename",
            "file_operations.waiting_for_rename",
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

fn ends_of(strings: &Strings, request: &Request) -> Ends {
    let entries = |sources: &[PathBuf]| match sources {
        [one] => text_of(one),
        many => folder_of(strings, many, "file_operations.several_folders_inline"),
    };
    match request {
        Request::Copy {
            sources,
            destination,
        }
        | Request::Move {
            sources,
            destination,
        } => Ends {
            from: entries(sources),
            to: Some(text_of(destination)),
        },
        Request::Delete { sources } => Ends {
            from: entries(sources),
            to: None,
        },
        Request::Rename { path, name } => {
            let path = text_of(path);
            let to = joined(parent_of(&path), &name.to_string_lossy());
            Ends {
                from: path,
                to: Some(to),
            }
        }
        Request::NewFolder { path } => Ends {
            from: text_of(path),
            to: None,
        },
    }
}

/// The folder that holds every one of `sources`, also of one, or the text
/// `several` when they are on different drives or servers: "Several
/// folders" on its own, or "several folders" inside a sentence.
fn folder_of(strings: &Strings, sources: &[PathBuf], several: &str) -> String {
    let paths: Vec<String> = sources.iter().map(|path| text_of(path)).collect();
    common_folder(paths.iter().map(String::as_str))
        .unwrap_or_else(|| strings.text(several).to_owned())
}

/// Where the card says an operation reads and writes, which stays the same
/// for the whole operation while the file line names each file: as
/// [`ends_of`] says, except that a copy, a move and a delete read from the
/// folder that holds their entries, also for one.
fn card_ends(strings: &Strings, request: &Request) -> Ends {
    match request {
        Request::Copy {
            sources,
            destination,
        }
        | Request::Move {
            sources,
            destination,
        } => Ends {
            from: folder_of(strings, sources, "file_operations.several_folders"),
            to: Some(text_of(destination)),
        },
        Request::Delete { sources } => Ends {
            from: folder_of(strings, sources, "file_operations.several_folders"),
            to: None,
        },
        _ => ends_of(strings, request),
    }
}

/// An operation as the window reads it from the queue.
pub(crate) struct Operation<'a> {
    pub(crate) id: OperationId,
    pub(crate) request: &'a Request,
    pub(crate) state: &'a State,
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
        taskbar: taskbar(operations),
    }
}

/// Whether a running operation's progress is in bytes: a copy or a move
/// across volumes with data to move. A move by rename and a copy of empty
/// files count their entries.
fn moves_bytes(running: &Running) -> bool {
    !running.renames && running.total.is_some_and(|total| total.bytes > 0)
}

/// How much of a running operation is done: its bytes where it moves bytes,
/// otherwise its entries, and nothing measured without a total.
fn measure_of(running: &Running) -> Option<Measure> {
    let total = running.total?;
    Some(if moves_bytes(running) {
        Measure {
            done: running.bytes,
            total: total.bytes,
        }
    } else {
        Measure {
            done: running.items,
            total: total.items,
        }
    })
}

/// Every running operation's progress together, as File Explorer shows its
/// copies on the taskbar button: measured while any running operation
/// knows its total, moving while none does.
fn taskbar(operations: &[Operation]) -> Taskbar {
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
        .map(|m| u128::from(m.done.min(m.total)) * 1000 / u128::from(m.total.max(1)))
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
    let ends = card_ends(strings, operation.request);
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
                Request::Rename { .. } | Request::NewFolder { .. } => String::new(),
            };
            (
                relative_to(&one, &ends.from).to_owned(),
                one,
                moves_bytes.then_some(Measure { done: 0, total: 0 }),
            )
        }
    };
    let (items, bytes_text) = match running.total {
        Some(total) => (
            strings.count(
                "file_operations.items_of",
                total.items,
                numbers,
                &[&numbers.integer(running.items)],
            ),
            if moves_bytes {
                strings.format(
                    "file_operations.bytes_of",
                    &[
                        &bytes(strings, numbers, running.bytes),
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
    let ends = ends_of(strings, operation.request);
    let kind = kind_of(operation.request);
    let (detail, phase) = match &operation.state {
        State::Running(running) => {
            let progress = match measure_of(running) {
                Some(_) if moves_bytes(running) => strings.format(
                    "file_operations.bytes_of",
                    &[
                        &bytes(strings, numbers, running.bytes),
                        &bytes(strings, numbers, running.total.map_or(0, |t| t.bytes)),
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
                    let ends = ends_of(strings, other.request);
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
        (None, Request::Rename { .. } | Request::NewFolder { .. }) => String::new(),
    }
}

/// The items a running operation without a total has done: "6,240
/// deleted", "6,240 copied"; nothing for a rename or a new folder, which is
/// one step.
fn items_done(strings: &Strings, numbers: &dyn Numbers, request: &Request, items: u64) -> String {
    let key = match request {
        Request::Copy { .. } => "file_operations.copied",
        Request::Move { .. } => "file_operations.moved",
        Request::Delete { .. } => "file_operations.deleted",
        Request::Rename { .. } | Request::NewFolder { .. } => return String::new(),
    };
    strings.count(key, items, numbers, &[])
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
/// server.
fn common_folder<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut paths = paths.into_iter();
    let first = paths.next()?;
    let mut common: Vec<&str> = parent_of(first).split('\\').collect();
    for path in paths {
        let parts: Vec<&str> = parent_of(path).split('\\').collect();
        let same = common
            .iter()
            .zip(&parts)
            .take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
            .count();
        common.truncate(same);
    }
    // Two drives share nothing, and two servers share only the `\\` that
    // starts each path.
    if common.iter().all(|part| part.is_empty()) {
        return None;
    }
    let folder = common.join("\\");
    Some(if folder.ends_with(':') {
        folder + "\\"
    } else {
        folder
    })
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
/// selected entries, as File Explorer does before it has counted.
pub(crate) fn conflict_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    request: &Request,
    prepared: &Prepared,
) -> Option<Choices> {
    let found = prepared.conflicts?;
    let (key, sources, destination) = match request {
        Request::Copy {
            sources,
            destination,
        } => ("file_operations.conflict_copying", sources, destination),
        Request::Move {
            sources,
            destination,
        } => ("file_operations.conflict_moving", sources, destination),
        Request::Delete { .. } | Request::Rename { .. } | Request::NewFolder { .. } => {
            return None;
        }
    };
    let from = folder_of(strings, sources, "file_operations.several_folders_inline");
    let to = text_of(destination);
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

/// The Exit question while operations run or wait (file-manager-spec.md:
/// File operations, step 8): `running` operations stop and are offered
/// again as Interrupted, and `waiting` ones, which are not saved, never run.
pub(crate) fn exit_question(
    strings: &Strings,
    numbers: &dyn Numbers,
    running: u64,
    waiting: u64,
) -> Question {
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
    Question {
        title: without_access_key(strings.text("command.exit")),
        instruction,
        body: QuestionBody::Text(body),
        footer: None,
        confirm: strings.text("file_operations.exit_button").to_owned(),
        cancel: strings.text("button.cancel").to_owned(),
        guard: Guard::CancelFirst,
    }
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
        End::Complete | End::Cancelled if rows.is_empty() => return None,
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
    use gozler_fileops::Conflicts;

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
        assert_eq!(left(45, 1.0).as_deref(), Some("45 s left"));
        assert_eq!(left(90, 1.0).as_deref(), Some("2 min left"));
        assert_eq!(left(7_500, 1.0).as_deref(), Some("2 h 5 min left"));
        assert_eq!(left(100, 0.0), None);
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
            destination: PathBuf::from(r"V:\Backup\Photos"),
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
        let question =
            conflict_question(&strings, &English, &request, &prepared).expect("12 conflicts ask");
        assert_eq!(
            question.instruction,
            "The destination already has 12 files with the same names."
        );
        assert_eq!(question.body, "Copying 4,210 items from 2026 to Photos");
    }

    /// Prepare reads no file inside a folder that merges, so a merge alone
    /// asks: without the question, nobody chose, and the run would skip
    /// every file of the same name inside it.
    #[test]
    fn a_merge_alone_asks() {
        let strings = english();
        let request = Request::Copy {
            sources: vec![PathBuf::from(r"C:\Users\user\Photos")],
            destination: PathBuf::from(r"V:\Backup"),
        };
        let prepared = Prepared {
            entries: 1,
            total: None,
            conflicts: Some(Conflicts {
                merges: 1,
                ..Conflicts::default()
            }),
        };
        let question =
            conflict_question(&strings, &English, &request, &prepared).expect("a merge asks");
        assert_eq!(question.body, "Copying 1 item from user to Backup");
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
        assert_eq!(
            title.fitted(&strings, Px(80), &width_of, &shorten),
            r"Copy D:\Photos\2026\Summer\Beach → V:\Backup"
        );
        assert_eq!(
            title.fitted(&strings, Px(28), &width_of, &shorten),
            r"Copy D:\Ph…Beach → V:\Backup"
        );
    }
}
