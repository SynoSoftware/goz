//! The File operations window (file-manager-spec.md: File operations, step
//! 5): a window of its own, which the main window owns, with a card for the
//! running operation and the queue list under it. It never blocks Gozler:
//! closing it only hides it, and the operations go on.
//!
//! It owns the engine's queue. The queue calls its wake function on an
//! engine thread after each change of an operation's state; the wake only
//! posts a message, and the window then reads the states. A running
//! operation's counts change without a wake, so the window polls them
//! twice a second while one runs.

use crate::core::captions::{ButtonIcon, DialogCaptions, DialogItem};
use crate::core::file_operations::{
    self, Bar, CARD_CORNER, CARD_ICON, Failure, LastEnded, Layout, Meter, Metrics, OPEN_WIDTH,
    QueueAction, ROW_GLYPHS, ROW_ICON, Row, RowButtons, RowLine, RowPhase, StatusOperation,
    Taskbar, View, icon_index,
};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::layout::{Px, Rect, Size, button_content, button_room};
use crate::core::text::Strings;
use crate::win::clipboard::ClipboardFiles;
use crate::win::controls::{Button, Label, ProgressBar, ReportList, Tooltip};
use crate::win::dialog::{self, DialogMessage, DialogProc, Opened};
use crate::win::file_operations::{
    ERASE_BACKGROUND, OPERATION_CHANGED, PathLabel, QueueList, QueueNotify, TAKEN_CHANGED,
    TaskbarProgress, TaskbarState, Tile, paint_card, set_client_size, show_drag_cursor,
    show_paused, window_size,
};
use crate::win::gdi::{Font, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::menu::Menu;
use crate::win::notify::{Command, FlatGround};
use crate::win::wake::OpenWaker;
use crate::win::window::{self, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_fileops::{
    Choice, OperationId, OrderRefused, Outcome, Prepared, Queue, Request, Side, SpeedSettings,
    State, Taken,
};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::LRESULT;

const WINDOW_CLASS: &str = "gozler.ui.file_operations";

/// The timer that polls a running operation's counts.
const POLL: usize = 1;
/// Twice a second, as the engine's counters expect to be read.
const POLL_MS: u32 = 500;

/// Opens the window when an operation still runs [`OPEN_AFTER`] after it
/// started.
const OPEN: usize = 3;

const OPEN_AFTER: Duration = Duration::from_secs(2);

/// The timer that scrolls the queue list while a dragged row is held above
/// or below it.
const DRAG_SCROLL: usize = 2;
/// A row each tenth of a second, so a long queue scrolls quickly and a
/// short hold moves it a row or two.
const DRAG_SCROLL_MS: u32 = 100;

/// `IDCANCEL`, which `Esc` sends through `IsDialogMessageW`.
const ESCAPE: u16 = 2;

/// The window, as the main window holds it.
pub(super) struct FileOperations {
    window: Rc<Window>,
}

struct Controls {
    title: Label,
    status: Label,
    pause: Button,
    cancel: Button,
    doing: Label,
    file: PathLabel,
    file_bar: ProgressBar,
    from_label: Label,
    from: PathLabel,
    to_label: Label,
    to: PathLabel,
    total_bar: ProgressBar,
    percent: Label,
    items: Label,
    bytes: Label,
    toggle: Button,
    queue: Label,
    /// Pause, Resume or Continue for the selected row.
    run: Button,
    up: Button,
    down: Button,
    queue_cancel: Button,
    list: QueueList,
    hint: Label,
    hide: Button,
    tips: Tooltip,
}

impl Controls {
    /// The labels on the card, which draw on the window colour; the
    /// `muted` ones step back in gray.
    fn card_labels(&self) -> [(Hwnd, bool); 11] {
        [
            (self.title.hwnd(), false),
            (self.status.hwnd(), true),
            (self.doing.hwnd(), true),
            (self.file.hwnd(), false),
            (self.from_label.hwnd(), true),
            (self.from.hwnd(), false),
            (self.to_label.hwnd(), true),
            (self.to.hwnd(), false),
            (self.percent.hwnd(), true),
            (self.items.hwnd(), true),
            (self.bytes.hwnd(), true),
        ]
    }

    fn text_controls(&self) -> Vec<Hwnd> {
        let mut controls: Vec<Hwnd> = self.card_labels().iter().map(|(hwnd, _)| *hwnd).collect();
        controls.extend([
            self.queue.hwnd(),
            self.list.hwnd(),
            self.hint.hwnd(),
            self.hide.hwnd(),
        ]);
        controls
    }
}

struct Fonts {
    ui: Font,
    title: Font,
}

/// A row being dragged to another place in the queue.
#[derive(Clone, Copy)]
struct Drag {
    id: OperationId,
    /// The gap the row would go to, while the queue allows it.
    gap: Option<usize>,
}

struct Window {
    hwnd: Cell<Option<Hwnd>>,
    owner: Hwnd,
    queue: OnceCell<Queue>,
    /// Wakes the window from the engine's threads, which outlive it, until
    /// `WM_DESTROY` closes it.
    waker: OnceCell<OpenWaker>,
    strings: RefCell<Strings>,
    numbers: RefCell<WindowsNumbers>,
    controls: OnceCell<Controls>,
    fonts: RefCell<Option<Fonts>>,
    layout: Cell<Layout>,
    meters: RefCell<HashMap<OperationId, Meter>>,
    /// Each operation's [`file_operations::Operation::folder`].
    folders: RefCell<HashMap<OperationId, Option<String>>>,
    /// The operations the window has seen, so an interrupted one shows the
    /// window at once.
    known: RefCell<HashSet<OperationId>>,
    /// When each live operation started, which the window opens 2 seconds
    /// after. An operation whose conflict question is open has none, so the
    /// time the person takes to answer does not count.
    started: RefCell<HashMap<OperationId, Instant>>,
    /// The operations that were live when the person hid the window, which
    /// do not show it again.
    hidden: RefCell<HashSet<OperationId>>,
    /// The operation that ended last, with its request and outcome, which
    /// the owner's status bar shows after the queue let it go.
    last_ended: RefCell<Option<LastEnded>>,
    /// The operations whose conflict question Keep both answered, whose
    /// status line cannot name an entry's numbered name.
    numbered: RefCell<HashSet<OperationId>>,
    /// What the owner's status bar shows, as the window last told it.
    status: RefCell<Option<StatusOperation>>,
    /// What the window shows now.
    view: RefCell<Option<View>>,
    selected: Cell<Option<OperationId>>,
    queue_open: Cell<bool>,
    drag: Cell<Option<Drag>>,
    taskbar: TaskbarProgress,
    /// A conflict question or a failure dialog is open. Its modal loop
    /// delivers wakes, which then only refresh what the window shows.
    asking: Cell<bool>,
    /// The clipboard's files that Paste prepares an operation from, until
    /// it is submitted or ends.
    pastes: RefCell<HashMap<OperationId, ClipboardFiles>>,
    /// Ended deletes and moves whose failures were reported, which stay in
    /// the queue while a list of the window keeps them (fast-copy-delete-spec.md:
    /// Taken entries).
    reported: RefCell<HashSet<OperationId>>,
    /// The queue's taken entries as the window last read them.
    taken: RefCell<Arc<Taken>>,
    /// Every operation that ended since the owner last took them, with its
    /// request, which the owner's panels compare with their folders.
    ended: RefCell<Vec<(OperationId, Arc<Request>)>>,
    /// The ended operations already put in `ended`, so each goes once.
    told: RefCell<HashSet<OperationId>>,
}

/// An operation that waits for the window.
enum Next {
    /// Prepared, so the person answers the conflicts, if any.
    Answer {
        id: OperationId,
        request: Arc<Request>,
        prepared: Prepared,
    },
    /// Ended, so the window reports what failed, if anything.
    Report {
        id: OperationId,
        request: Arc<Request>,
        outcome: Arc<Outcome>,
    },
}

impl FileOperations {
    /// Creates the queue, whose saved operations are in `saved`, and the
    /// window, hidden; it shows itself when an operation starts, or at once
    /// when the saved file holds interrupted operations.
    pub(super) fn open(
        owner: Hwnd,
        saved: &Path,
        strings: Strings,
    ) -> windows::core::Result<FileOperations> {
        window::register_class::<Window>(WINDOW_CLASS, crate::core::ids::IconId::Gozler.id())?;
        let window = Rc::new(Window {
            hwnd: Cell::new(None),
            owner,
            queue: OnceCell::new(),
            waker: OnceCell::new(),
            strings: RefCell::new(strings),
            numbers: RefCell::new(WindowsNumbers::new()),
            controls: OnceCell::new(),
            fonts: RefCell::new(None),
            layout: Cell::new(Layout::default()),
            meters: RefCell::new(HashMap::new()),
            folders: RefCell::new(HashMap::new()),
            known: RefCell::new(HashSet::new()),
            started: RefCell::new(HashMap::new()),
            hidden: RefCell::new(HashSet::new()),
            last_ended: RefCell::new(None),
            numbered: RefCell::new(HashSet::new()),
            status: RefCell::new(None),
            view: RefCell::new(None),
            selected: Cell::new(None),
            queue_open: Cell::new(true),
            drag: Cell::new(None),
            taken: RefCell::new(Arc::default()),
            taskbar: TaskbarProgress::new(owner),
            asking: Cell::new(false),
            pastes: RefCell::new(HashMap::new()),
            reported: RefCell::new(HashSet::new()),
            ended: RefCell::new(Vec::new()),
            told: RefCell::new(HashSet::new()),
        });
        let hwnd = window::create_owned_window(WINDOW_CLASS, Rc::clone(&window), owner)?;
        window.hwnd.set(Some(hwnd));
        window::set_modeless(Some(hwnd));
        let waker = OpenWaker::new(hwnd);
        let _ = window.waker.set(waker.clone());
        let queue = Queue::new(saved, move || waker.deliver(|| true));
        let _ = window.queue.set(queue);
        window.create_controls(hwnd)?;
        window.apply_fonts(hwnd);
        window.apply_strings();
        let metrics = window.metrics(hwnd);
        set_client_size(
            hwnd,
            Size {
                width: px(OPEN_WIDTH, hwnd),
                height: Layout::height(metrics, true, false),
            },
        );
        window.refresh(hwnd);
        window.select_first_interrupted();
        Ok(FileOperations { window })
    }

    /// The entries that deletes and moves take, which the search list
    /// shows blank. The owner gets [`TAKEN_CHANGED`] after it changes.
    pub(super) fn taken(&self) -> Arc<Taken> {
        Arc::clone(&self.window.taken.borrow())
    }

    /// Prepares `request`; the window asks about its conflicts when the
    /// count is ready, and then submits it. `paste` holds the clipboard's
    /// files that the request copies or moves, which a submitted move of
    /// cut files reports as moved.
    pub(super) fn start(
        &self,
        request: Request,
        settings: SpeedSettings,
        paste: Option<ClipboardFiles>,
    ) -> OperationId {
        let id = self.window.queue().prepare(request, settings);
        if let Some(paste) = paste {
            self.window.pastes.borrow_mut().insert(id, paste);
        }
        id
    }

    /// Cancels the operation `id`, as the status bar's Cancel does.
    pub(super) fn cancel(&self, id: OperationId) {
        if let Some(hwnd) = self.window.hwnd.get() {
            self.window.act(hwnd, id, QueueAction::Cancel);
        }
    }

    /// The operation the owner's status bar shows, after
    /// [`OPERATION_CHANGED`].
    pub(super) fn status(&self) -> Option<StatusOperation> {
        self.window.status.borrow().clone()
    }

    /// The operations of every kind that ended since the last call, each
    /// with its request, which the owner's lists compare with what they
    /// show. The owner takes them with the taken entries it then shows, after
    /// [`TAKEN_CHANGED`].
    pub(super) fn take_ended(&self) -> Vec<(OperationId, Arc<Request>)> {
        std::mem::take(&mut self.window.ended.borrow_mut())
    }

    /// Removes the reported deletes and moves that no list keeps, as
    /// `keeps` says, and has the owner read the taken entries again. One
    /// the owner has not taken yet stays, because no list has decided
    /// whether it keeps it.
    pub(super) fn release(&self, keeps: impl Fn(OperationId) -> bool) {
        let released: Vec<OperationId> = {
            let untold = self.window.ended.borrow();
            let keeps = |id: OperationId| keeps(id) || untold.iter().any(|(e, _)| *e == id);
            let mut reported = self.window.reported.borrow_mut();
            let released = reported.iter().copied().filter(|id| !keeps(*id)).collect();
            reported.retain(|id| keeps(*id));
            released
        };
        if released.is_empty() {
            return;
        }
        for id in released {
            self.window.queue().remove(id);
        }
        // As `refresh` does, so an operation that ended since then reaches
        // the lists before a snapshot shows it as ended.
        self.window.read_taken();
        self.window.read_ended(&self.window.queue().operations());
    }

    /// The operations that run, and those that wait, which Exit stops
    /// (file-manager-spec.md: File operations, step 8).
    pub(super) fn running_and_waiting(&self) -> (u64, u64) {
        let mut counts = (0, 0);
        for entry in self.window.queue().operations() {
            match entry.state {
                State::Running(_) => counts.0 += 1,
                State::Preparing | State::Prepared(_) | State::Waiting { .. } => counts.1 += 1,
                State::Ended(_) | State::Interrupted => {}
            }
        }
        counts
    }

    /// Exit: stops every operation so that Continue finishes it at the next
    /// start. The caller then ends the process without dropping `self`,
    /// because dropping the queue waits for every Windows copy, and a copy
    /// that Windows stops deletes its file.
    pub(super) fn stop_all(&self) {
        self.window.queue().close();
    }

    /// Shows the window in front, as View > File operations does.
    pub(super) fn show(&self) {
        if let Some(hwnd) = self.window.hwnd.get() {
            hwnd.bring_to_front();
        }
    }

    /// Reads the queue again, after a command changed it.
    pub(super) fn refresh(&self) {
        if let Some(hwnd) = self.window.hwnd.get() {
            self.window.refresh(hwnd);
        }
    }

    /// Words the window in the language of `strings`.
    pub(super) fn set_strings(&self, strings: Strings) {
        self.window.strings.replace(strings);
        self.window.apply_strings();
        // The labels and Hide are as wide as their new words.
        if let Some(hwnd) = self.window.hwnd.get() {
            self.window.layout(hwnd);
        }
        self.refresh();
    }

    /// Shows or hides Hide's icon, after icons were shown or hidden.
    pub(super) fn apply_glyphs(&self) {
        self.window.apply_glyphs();
    }

    /// Destroys the window.
    pub(super) fn close(&self) {
        if let Some(hwnd) = self.window.hwnd.take() {
            hwnd.destroy();
        }
    }
}

/// `dip` at the DPI of `hwnd`.
fn px(dip: i32, hwnd: Hwnd) -> Px {
    Px(dip).rescale(
        crate::core::layout::Dpi::new(96).expect("96 is not zero"),
        hwnd.dpi(),
    )
}

impl Window {
    fn queue(&self) -> &Queue {
        self.queue.get().expect("open sets the queue")
    }

    fn controls(&self) -> &Controls {
        self.controls.get().expect("open creates the controls")
    }

    fn create_controls(&self, hwnd: Hwnd) -> windows::core::Result<()> {
        let id = |control: ControlId| control.id();
        let label = |control| Label::create_line(hwnd, id(control));
        let path = |control| PathLabel::create(hwnd, id(control));
        let button = |control| Button::create(hwnd, id(control));
        let bar = |control| ProgressBar::create(hwnd, id(control));
        let pause = button(ControlId::FileOperationsPause)?;
        let cancel = button(ControlId::FileOperationsCancel)?;
        let file = path(ControlId::FileOperationsFile)?;
        let from = path(ControlId::FileOperationsFrom)?;
        let to = path(ControlId::FileOperationsTo)?;
        let toggle = button(ControlId::FileOperationsToggle)?;
        let run = button(ControlId::FileOperationsResume)?;
        let up = button(ControlId::FileOperationsMoveUp)?;
        let down = button(ControlId::FileOperationsMoveDown)?;
        let queue_cancel = button(ControlId::FileOperationsQueueCancel)?;
        let hide = button(ControlId::FileOperationsHide)?;
        let tips = Tooltip::create(
            hwnd,
            &[
                pause.hwnd(),
                cancel.hwnd(),
                file.hwnd(),
                from.hwnd(),
                to.hwnd(),
                toggle.hwnd(),
                run.hwnd(),
                up.hwnd(),
                down.hwnd(),
                queue_cancel.hwnd(),
                hide.hwnd(),
            ],
        )?;
        let controls = Controls {
            title: label(ControlId::FileOperationsTitle)?,
            status: label(ControlId::FileOperationsStatus)?,
            pause,
            cancel,
            doing: label(ControlId::FileOperationsDoing)?,
            file,
            file_bar: bar(ControlId::FileOperationsFileBar)?,
            from_label: label(ControlId::FileOperationsFromLabel)?,
            from,
            to_label: label(ControlId::FileOperationsToLabel)?,
            to,
            total_bar: bar(ControlId::FileOperationsTotalBar)?,
            percent: Label::create_right_line(hwnd, id(ControlId::FileOperationsPercent))?,
            items: label(ControlId::FileOperationsItems)?,
            bytes: Label::create_right_line(hwnd, id(ControlId::FileOperationsBytes))?,
            toggle,
            queue: label(ControlId::FileOperationsQueue)?,
            run,
            up,
            down,
            queue_cancel,
            list: QueueList::create(hwnd, id(ControlId::FileOperationsList))?,
            hint: label(ControlId::FileOperationsHint)?,
            hide,
            tips,
        };
        let _ = self.controls.set(controls);
        Ok(())
    }

    /// Creates the fonts for the window's DPI and gives them to the
    /// controls, then the list its icons at that DPI.
    fn apply_fonts(&self, hwnd: Hwnd) {
        let Ok(ui) = Font::message(hwnd.dpi()) else {
            return;
        };
        let title = ui.heading().unwrap_or_else(|_| {
            Font::message(hwnd.dpi()).expect("the message font was just created")
        });
        let controls = self.controls();
        for control in controls.text_controls() {
            control.set_font(&ui);
        }
        controls.title.hwnd().set_font(&title);
        controls.list.set_icons(&ROW_GLYPHS, px(ROW_ICON, hwnd));
        self.apply_glyphs();
        // The controls hold the old fonts until they get the new ones.
        self.fonts.replace(Some(Fonts { ui, title }));
    }

    /// Gives Hide its icon, at the window's DPI, in the system colours of
    /// now and only while icons show.
    fn apply_glyphs(&self) {
        self.controls()
            .hide
            .set_icon(ButtonIcon::of("file_operations.hide", false));
    }

    /// The texts that do not change with the operations.
    fn apply_strings(&self) {
        let strings = self.strings.borrow();
        let controls = self.controls();
        let text = |key| strings.text(key);
        for (button, key) in [
            (controls.cancel, "file_operations.cancel"),
            (controls.up, "file_operations.move_up"),
            (controls.down, "file_operations.move_down"),
            (controls.queue_cancel, "file_operations.cancel"),
        ] {
            button.hwnd().set_text(text(key));
            controls.tips.set(button.hwnd(), text(key));
        }
        controls.hide.hwnd().set_text(text("file_operations.hide"));
        controls
            .tips
            .set(controls.hide.hwnd(), text("file_operations.hide_tip"));
        controls
            .from_label
            .hwnd()
            .set_text(text("file_operations.from"));
        controls
            .to_label
            .hwnd()
            .set_text(text("file_operations.to"));
        self.apply_toggle();
    }

    /// The toggle's name and tip, which say what a click does.
    fn apply_toggle(&self) {
        let strings = self.strings.borrow();
        let controls = self.controls();
        let key = if self.queue_open.get() {
            "file_operations.hide_queue"
        } else {
            "file_operations.show_queue"
        };
        controls.toggle.hwnd().set_text(strings.text(key));
        controls.tips.set(controls.toggle.hwnd(), strings.text(key));
        controls.toggle.hwnd().invalidate();
    }

    fn metrics(&self, hwnd: Hwnd) -> Metrics {
        let fonts = self.fonts.borrow();
        let strings = self.strings.borrow();
        let dpi = hwnd.dpi();
        let Some(fonts) = fonts.as_ref() else {
            return Metrics {
                dpi,
                line: px(16, hwnd),
                title_line: px(20, hwnd),
                label_width: px(60, hwnd),
                percent_width: px(36, hwnd),
                hide_width: px(75, hwnd),
            };
        };
        let ui = TextMeter::new(&fonts.ui);
        let title = TextMeter::new(&fonts.title);
        let label_width = [
            "file_operations.copying",
            "file_operations.moving",
            "file_operations.deleting",
            "file_operations.renaming",
            "file_operations.creating",
            "file_operations.from",
            "file_operations.to",
        ]
        .iter()
        .map(|key| ui.width(strings.text(key)))
        .max()
        .unwrap_or_default();
        let percent_width = ui.width(&strings.format("file_operations.percent", &["100"]));
        // Room for the icon also while icons are hidden, so Hide keeps its
        // width when they show.
        let hide_caption = ui.width(strings.text("file_operations.hide"));
        let hide_width =
            button_room(button_content(hide_caption, true, dpi), dpi).max(px(75, hwnd));
        Metrics {
            dpi,
            line: ui.height(),
            title_line: title.height(),
            label_width,
            percent_width,
            hide_width,
        }
    }

    fn layout(&self, hwnd: Hwnd) {
        let metrics = self.metrics(hwnd);
        let layout = Layout::compute(
            hwnd.client_size().width,
            metrics,
            self.queue_open.get(),
            self.hint_shown(),
        );
        self.layout.set(layout);
        let c = self.controls();
        let open = self.queue_open.get();
        let moves = [
            (c.title.hwnd(), layout.title),
            (c.status.hwnd(), layout.status),
            (c.pause.hwnd(), layout.pause),
            (c.cancel.hwnd(), layout.cancel),
            (c.doing.hwnd(), layout.doing),
            (c.file.hwnd(), layout.file),
            (c.file_bar.hwnd(), layout.file_bar),
            (c.from_label.hwnd(), layout.from_label),
            (c.from.hwnd(), layout.from),
            (c.to_label.hwnd(), layout.to_label),
            (c.to.hwnd(), layout.to),
            (c.total_bar.hwnd(), layout.total_bar),
            (c.percent.hwnd(), layout.percent),
            (c.items.hwnd(), layout.items),
            (c.bytes.hwnd(), layout.bytes),
            (c.toggle.hwnd(), layout.toggle),
            (c.queue.hwnd(), layout.queue),
            (c.run.hwnd(), layout.resume),
            (c.up.hwnd(), layout.up),
            (c.down.hwnd(), layout.down),
            (c.queue_cancel.hwnd(), layout.queue_cancel),
            (c.list.hwnd(), layout.list),
            (c.hint.hwnd(), layout.hint),
            (c.hide.hwnd(), layout.hide),
        ];
        win::move_children(&moves);
        for control in [
            c.list.hwnd(),
            c.hint.hwnd(),
            c.up.hwnd(),
            c.down.hwnd(),
            c.queue_cancel.hwnd(),
        ] {
            control.set_visible(open);
        }
        // Less the scroll bar, so a long queue scrolls without a
        // horizontal bar.
        c.list.set_tile_width(layout.list.width - px(4 + 17, hwnd));
        self.show_tiles();
        hwnd.invalidate();
    }

    /// Reads the queue's states and shows them.
    fn refresh(&self, hwnd: Hwnd) {
        self.read_taken();
        let entries = self.queue().operations();
        self.read_ended(&entries);
        let now = Instant::now();
        {
            let mut meters = self.meters.borrow_mut();
            meters.retain(|id, _| {
                entries
                    .iter()
                    .any(|entry| entry.id == *id && matches!(entry.state, State::Running(_)))
            });
            for entry in &entries {
                if let State::Running(running) = &entry.state {
                    let meter = meters.entry(entry.id).or_default();
                    if running.paused {
                        meter.clear();
                    } else {
                        meter.record(now, running.items, running.bytes);
                    }
                }
            }
        }
        let view = {
            let mut folders = self.folders.borrow_mut();
            folders.retain(|id, _| entries.iter().any(|entry| entry.id == *id));
            for entry in &entries {
                folders
                    .entry(entry.id)
                    .or_insert_with(|| file_operations::sources_folder(&entry.request));
            }
            let operations: Vec<file_operations::Operation> = entries
                .iter()
                .map(|entry| file_operations::Operation {
                    id: entry.id,
                    request: &entry.request,
                    state: &entry.state,
                    folder: folders[&entry.id].as_deref(),
                })
                .collect();
            file_operations::view(
                &self.strings.borrow(),
                &*self.numbers.borrow(),
                &operations,
                &self.meters.borrow(),
            )
        };
        let interrupted = {
            let mut known = self.known.borrow_mut();
            let interrupted = entries.iter().any(|entry| {
                matches!(entry.state, State::Interrupted) && !known.contains(&entry.id)
            });
            *known = entries.iter().map(|entry| entry.id).collect();
            interrupted
        };
        let due = self.open_due(&entries, now);
        let status = {
            let last_ended = self.last_ended.borrow();
            let mut numbered = self.numbered.borrow_mut();
            numbered.retain(|id| {
                entries.iter().any(|entry| entry.id == *id)
                    || last_ended.as_ref().is_some_and(|(last, ..)| last == id)
            });
            StatusOperation::of(
                &entries,
                last_ended.as_ref(),
                &self.reported.borrow(),
                &numbered,
            )
        };
        if *self.status.borrow() != status {
            self.status.replace(status);
            self.owner.post(OPERATION_CHANGED);
        }
        let running = view.card.is_some();
        let empty = view.card.is_none() && view.rows.is_empty();
        self.show_view(hwnd, view);
        if running {
            hwnd.set_timer(POLL, POLL_MS);
        } else {
            hwnd.kill_timer(POLL);
        }
        // The window opens when an operation still runs 2 seconds after it
        // started, so a short one shows only in the owner's status bar, and
        // closes when the last one ends; the person can hide it at any time
        // (file-manager-spec.md: File operations, step 5).
        hwnd.kill_timer(OPEN);
        if empty {
            hwnd.hide();
        } else if !hwnd.is_visible() {
            match due {
                _ if interrupted => hwnd.set_visible(true),
                Some(due) if due <= now => hwnd.set_visible(true),
                Some(due) => {
                    let wait = due.duration_since(now).as_millis();
                    hwnd.set_timer(OPEN, u32::try_from(wait).unwrap_or(u32::MAX).max(1));
                }
                None => {}
            }
        }
        self.handle_next(hwnd);
    }

    /// The moment the earliest live operation that the person did not hide
    /// has run for [`OPEN_AFTER`], after the start moments are brought up to
    /// date with `entries`; `None` when none is live.
    fn open_due(&self, entries: &[gozler_fileops::Operation], now: Instant) -> Option<Instant> {
        let mut started = self.started.borrow_mut();
        let mut hidden = self.hidden.borrow_mut();
        hidden.retain(|id| entries.iter().any(|entry| entry.id == *id));
        let live: HashSet<OperationId> = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.state,
                    State::Preparing | State::Waiting { .. } | State::Running(_)
                )
            })
            .map(|entry| entry.id)
            .collect();
        started.retain(|id, _| live.contains(id));
        for id in live {
            started.entry(id).or_insert(now);
        }
        started
            .iter()
            .filter(|(id, _)| !hidden.contains(id))
            .map(|(_, at)| *at + OPEN_AFTER)
            .min()
    }

    /// Hides the window; the operations that have not ended now do not show
    /// it again, also one whose conflict question is open, which starts
    /// after the answer, and a later one does.
    fn hide(&self, hwnd: Hwnd) {
        let unended = (self.queue().operations().into_iter())
            .filter(|entry| !matches!(entry.state, State::Ended(_)))
            .map(|entry| entry.id);
        self.hidden.borrow_mut().extend(unended);
        hwnd.kill_timer(OPEN);
        hwnd.hide();
    }

    /// Answers each prepared operation and reports each ended one, one at a
    /// time, because each may open a modal dialog.
    fn handle_next(&self, hwnd: Hwnd) {
        if self.asking.replace(true) {
            return;
        }
        let mut handled = false;
        while let Some(next) = self.next() {
            handled = true;
            match next {
                Next::Answer {
                    id,
                    request,
                    prepared,
                } => self.answer(id, &request, prepared),
                Next::Report {
                    id,
                    request,
                    outcome,
                } => self.report(id, &request, &outcome),
            }
        }
        self.asking.set(false);
        if handled {
            self.refresh(hwnd);
        }
    }

    fn next(&self) -> Option<Next> {
        let reported = self.reported.borrow();
        self.queue()
            .operations()
            .into_iter()
            .find_map(|entry| match entry.state {
                State::Prepared(prepared) => Some(Next::Answer {
                    id: entry.id,
                    request: entry.request,
                    prepared,
                }),
                State::Ended(outcome) if !reported.contains(&entry.id) => Some(Next::Report {
                    id: entry.id,
                    request: entry.request,
                    outcome,
                }),
                _ => None,
            })
    }

    /// Asks the conflict question when prepare found conflicts, then
    /// submits the operation with the answer, or cancels it.
    fn answer(&self, id: OperationId, request: &Request, prepared: Prepared) {
        let question = file_operations::conflict_question(
            &self.strings.borrow(),
            &*self.numbers.borrow(),
            request,
            &prepared,
        );
        let choice = match question {
            None => None,
            // The question runs its own message loop, so nothing is
            // borrowed here.
            Some(question) => match win::dialog::choose(self.owner, &question) {
                Some(index) => file_operations::conflict_choice(index),
                None => {
                    self.queue().cancel(id);
                    return;
                }
            },
        };
        let submitted = self.queue().submit(id, choice).is_ok();
        if submitted && choice == Some(Choice::KeepBoth) {
            self.numbered.borrow_mut().insert(id);
        }
        let paste = self.pastes.borrow_mut().remove(&id);
        if let Some(paste) = paste
            && submitted
            && paste.cut
            && matches!(request, Request::Move { .. })
        {
            paste.moved(self.owner);
        }
    }

    /// Shows what failed in an ended operation, then removes it, unless it
    /// deleted or moved entries, which the owner's lists may still keep
    /// blank; the owner then decides ([`FileOperations::release`]).
    fn report(&self, id: OperationId, request: &Arc<Request>, outcome: &Arc<Outcome>) {
        // An operation that ended after the last read is told here, before
        // a copy leaves the queue.
        self.tell_ended(id, request);
        self.last_ended
            .replace(Some((id, Arc::clone(request), Arc::clone(outcome))));
        self.pastes.borrow_mut().remove(&id);
        let failure = file_operations::failure(
            &self.strings.borrow(),
            &*self.numbers.borrow(),
            request,
            outcome,
            &|code| win::error_text(&std::io::Error::from_raw_os_error(code as i32)),
        );
        if let Some(failure) = failure.as_ref().filter(|failure| failure.rows.is_empty()) {
            // A refusal, or a full disk before any entry failed, lists
            // nothing, so the message stands alone.
            let text = format!("{}\n\n{}", failure.instruction, failure.body);
            dialog::error_box(Some(self.owner), &text, &failure.title);
        } else if let Some(failure) = failure {
            let strings = self.strings.borrow();
            let mut captions = DialogCaptions::new(DialogId::FileOperationsFailed, &strings);
            captions.title.clone_from(&failure.title);
            let dialog = FailureDialog {
                failure,
                columns: [
                    strings.text("file_operations.column_item").to_owned(),
                    strings.text("file_operations.column_reason").to_owned(),
                ],
            };
            drop(strings);
            // The dialog runs its own message loop, so nothing is borrowed
            // here.
            let _ = dialog::run_modal(
                self.owner,
                DialogId::FileOperationsFailed,
                &captions,
                &dialog,
            );
        }
        let took =
            outcome.items > 0 && matches!(**request, Request::Delete { .. } | Request::Move { .. });
        if took {
            self.reported.borrow_mut().insert(id);
            self.owner.post(TAKEN_CHANGED);
        } else {
            self.queue().remove(id);
        }
    }

    fn show_view(&self, hwnd: Hwnd, view: View) {
        let c = self.controls();
        hwnd.set_text(&view.title);
        let strings = self.strings.borrow();
        match &view.card {
            Some(card) => {
                c.title.hwnd().set_text(&card.title);
                c.status.hwnd().set_text(&card.status);
                c.doing.hwnd().set_text(&card.doing);
                c.file.hwnd().set_text(&card.file);
                c.tips.set(c.file.hwnd(), &card.file_tip);
                c.from.hwnd().set_text(&card.from);
                c.tips.set(c.from.hwnd(), &card.from);
                let to = card.to.as_deref().unwrap_or_default();
                c.to.hwnd().set_text(to);
                c.tips.set(c.to.hwnd(), to);
                for control in [
                    c.from_label.hwnd(),
                    c.total_bar.hwnd(),
                    c.pause.hwnd(),
                    c.cancel.hwnd(),
                ] {
                    control.set_visible(true);
                }
                c.to_label.hwnd().set_visible(card.to.is_some());
                c.file_bar.hwnd().set_visible(card.file_bar.is_some());
                if let Some(bar) = card.file_bar {
                    c.file_bar.set_measured(bar.done, bar.total);
                    show_paused(c.file_bar, card.paused);
                }
                match card.total_bar {
                    Bar::Measured(measure) => {
                        c.total_bar.set_measured(measure.done, measure.total);
                        show_paused(c.total_bar, card.paused);
                        c.percent.hwnd().set_text(
                            &strings.format(
                                "file_operations.percent",
                                &[&measure.percent().to_string()],
                            ),
                        );
                    }
                    Bar::Moving => {
                        c.total_bar.set_moving(!card.paused);
                        c.percent.hwnd().set_text("");
                    }
                }
                c.items.hwnd().set_text(&card.items);
                c.bytes.hwnd().set_text(&card.bytes);
                let pause = if card.paused {
                    "file_operations.resume"
                } else {
                    "file_operations.pause"
                };
                c.pause.hwnd().set_text(strings.text(pause));
                c.tips.set(c.pause.hwnd(), strings.text(pause));
                // The button draws its glyph, which follows the caption.
                c.pause.hwnd().invalidate();
            }
            None => {
                c.title
                    .hwnd()
                    .set_text(strings.text("file_operations.nothing_running"));
                for label in [
                    c.status.hwnd(),
                    c.doing.hwnd(),
                    c.file.hwnd(),
                    c.from.hwnd(),
                    c.to.hwnd(),
                    c.percent.hwnd(),
                    c.items.hwnd(),
                    c.bytes.hwnd(),
                ] {
                    label.set_text("");
                }
                for path in [c.file, c.from, c.to] {
                    c.tips.set(path.hwnd(), "");
                }
                // Only the title stays, so the card shows nothing that
                // waits for a value.
                for control in [
                    c.from_label.hwnd(),
                    c.to_label.hwnd(),
                    c.file_bar.hwnd(),
                    c.total_bar.hwnd(),
                    c.pause.hwnd(),
                    c.cancel.hwnd(),
                ] {
                    control.set_visible(false);
                }
            }
        }
        c.queue.hwnd().set_text(&view.queue);
        if !self
            .selected
            .get()
            .is_some_and(|id| view.rows.iter().any(|row| row.id == id))
        {
            self.selected.set(None);
        }
        self.taskbar.show(match view.taskbar {
            Taskbar::None => TaskbarState::None,
            Taskbar::Moving => TaskbarState::Moving,
            Taskbar::Measured(m) => TaskbarState::Measured {
                done: m.done,
                total: m.total,
            },
            Taskbar::Paused(m) => TaskbarState::Paused {
                done: m.done,
                total: m.total,
            },
        });
        let icon_changed = self
            .view
            .borrow()
            .as_ref()
            .map(|old| old.card.as_ref().map(|card| card.glyph))
            != Some(view.card.as_ref().map(|card| card.glyph));
        drop(strings);
        // Stored first: the list reports the selection it is given at once,
        // and the window reads the row it selects from the view.
        self.view.replace(Some(view));
        self.show_tiles();
        self.update_queue_buttons();
        if icon_changed {
            hwnd.invalidate();
        }
    }

    /// Shows the view's rows in the list, with the selected row selected.
    /// Each title fits its tile's first line, its paths shortened in the
    /// middle, so the second line keeps the row's state; the row's tooltip
    /// gives the whole title.
    fn show_tiles(&self) {
        let c = self.controls();
        let view = self.view.borrow();
        let rows: &[Row] = view.as_ref().map_or(&[], |view| &view.rows);
        let selected = self
            .selected
            .get()
            .and_then(|id| rows.iter().position(|row| row.id == id));
        let strings = self.strings.borrow();
        let fonts = self.fonts.borrow();
        let meter = fonts.as_ref().map(|fonts| TextMeter::new(&fonts.ui));
        let show = |width: Option<Px>| {
            let fitted = |line: &RowLine| match (&meter, width) {
                (Some(meter), Some(width)) => line.fitted(
                    &strings,
                    width,
                    &|text| meter.plain_width(text),
                    &|path, width| meter.compact_path(path, width),
                ),
                _ => line.whole(&strings),
            };
            let lines: Vec<(String, String)> = rows
                .iter()
                .map(|row| (fitted(&row.title), fitted(&row.detail)))
                .collect();
            let tiles: Vec<Tile> = rows
                .iter()
                .zip(&lines)
                .map(|(row, (title, detail))| Tile {
                    icon: icon_index(row.glyph),
                    title,
                    detail,
                })
                .collect();
            c.list.set_tiles(&tiles, selected);
        };
        // The list measures a tile once it holds one.
        let width = c.list.text_width().or_else(|| {
            show(None);
            c.list.text_width()
        });
        show(width);
    }

    /// Puts each operation that ended since the last read in `ended`, and
    /// tells the owner. They are read after the taken entries, so each
    /// operation that those show ended is among them.
    fn read_ended(&self, entries: &[gozler_fileops::Operation]) {
        self.told
            .borrow_mut()
            .retain(|id| entries.iter().any(|entry| entry.id == *id));
        for entry in entries {
            if matches!(entry.state, State::Ended(_)) {
                self.tell_ended(entry.id, &entry.request);
            }
        }
    }

    /// Puts the ended operation `id` in `ended`, once, and tells the owner.
    fn tell_ended(&self, id: OperationId, request: &Arc<Request>) {
        if self.told.borrow_mut().insert(id) {
            self.ended.borrow_mut().push((id, Arc::clone(request)));
            self.owner.post(TAKEN_CHANGED);
        }
    }

    /// Reads the queue's taken entries, and tells the owner when they
    /// changed.
    fn read_taken(&self) {
        let taken = self.queue().taken();
        if !Arc::ptr_eq(&taken, &self.taken.borrow()) {
            self.taken.replace(taken);
            self.owner.post(TAKEN_CHANGED);
        }
    }

    /// The selected row, as the window shows it.
    fn selected_row<T>(&self, use_row: impl FnOnce(&Row, &[Row]) -> T) -> Option<T> {
        let view = self.view.borrow();
        let rows = &view.as_ref()?.rows;
        let id = self.selected.get()?;
        let row = rows.iter().find(|row| row.id == id)?;
        Some(use_row(row, rows))
    }

    /// Where a row dropped at `gap`, a place between the rows, lands: the
    /// gap before the first waiting row from there, which it goes before,
    /// or else the gap after the last waiting row, with `None` for the end
    /// of the waiting operations. The queue lists the running operations
    /// first and the waiting ones next, and moves only the waiting ones.
    fn landing(rows: &[Row], gap: usize) -> (usize, Option<OperationId>) {
        let waiting = |row: &&Row| row.phase == RowPhase::Waiting;
        match rows
            .iter()
            .enumerate()
            .skip(gap)
            .find(|(_, row)| waiting(row))
        {
            Some((index, row)) => (index, Some(row.id)),
            None => {
                let after = rows
                    .iter()
                    .rposition(|row| waiting(&row))
                    .map_or(gap, |last| last + 1);
                (after, None)
            }
        }
    }

    /// The place before which Move up or Move down puts the waiting row
    /// `id`, or `None` when it is first or last among the waiting rows
    /// already. The inner `None` is the end of the queue.
    fn step(rows: &[Row], id: OperationId, up: bool) -> Option<Option<OperationId>> {
        let waiting: Vec<OperationId> = rows
            .iter()
            .filter(|row| row.phase == RowPhase::Waiting)
            .map(|row| row.id)
            .collect();
        let index = waiting.iter().position(|w| *w == id)?;
        if up {
            Some(Some(*waiting.get(index.checked_sub(1)?)?))
        } else {
            waiting.get(index + 1)?;
            Some(waiting.get(index + 2).copied())
        }
    }

    /// Whether the queue lets the selected row move up or down, and the
    /// hint that names why not.
    fn can_step(&self, up: bool) -> Result<bool, OrderRefused> {
        let step =
            self.selected_row(|row, rows| Window::step(rows, row.id, up).map(|b| (row.id, b)));
        match step.flatten() {
            Some((id, before)) => self.queue().check_move(id, before).map(|()| true),
            None => Ok(false),
        }
    }

    /// The selected row, and what the queue's header buttons do to it.
    fn selected_buttons(&self) -> Option<(OperationId, RowButtons)> {
        self.selected_row(|row, _| (row.id, row.phase.buttons()))
    }

    /// Selects the first interrupted row while no operation runs, so its
    /// Continue and Dismiss show at once when the window opens.
    fn select_first_interrupted(&self) {
        let first = self
            .view
            .borrow()
            .as_ref()
            .filter(|view| view.card.is_none())
            .and_then(|view| {
                view.rows
                    .iter()
                    .find(|row| row.phase == RowPhase::Interrupted)
                    .map(|row| row.id)
            });
        if first.is_some() {
            self.selected.set(first);
            self.show_tiles();
            self.update_queue_buttons();
        }
    }

    /// Enables the queue's buttons for the selected row, and shows the
    /// hint when the queue refuses one of its moves.
    fn update_queue_buttons(&self) {
        let c = self.controls();
        let buttons = self.selected_buttons().map(|(_, buttons)| buttons);
        let up = self.can_step(true);
        let down = self.can_step(false);
        c.up.hwnd().set_enabled(matches!(up, Ok(true)));
        c.down.hwnd().set_enabled(matches!(down, Ok(true)));
        c.queue_cancel.hwnd().set_enabled(buttons.is_some());
        let strings = self.strings.borrow();
        let end = buttons.map_or(QueueAction::Cancel, |buttons| buttons.end);
        c.queue_cancel.hwnd().set_text(strings.text(end.caption()));
        c.tips
            .set(c.queue_cancel.hwnd(), strings.text(end.caption()));
        let run = buttons.and_then(|buttons| buttons.run);
        if let Some(run) = run {
            c.run.hwnd().set_text(strings.text(run.caption()));
            c.tips.set(c.run.hwnd(), strings.text(run.caption()));
        }
        c.run
            .hwnd()
            .set_visible(self.queue_open.get() && run.is_some());
        let refused = up.err().or(down.err());
        drop(strings);
        if self.drag.get().is_none() {
            self.show_hint(refused);
        }
        for button in [c.run, c.up, c.down, c.queue_cancel] {
            button.hwnd().invalidate();
        }
    }

    /// Shows why the queue refused a move, or clears the hint.
    fn show_hint(&self, refused: Option<OrderRefused>) {
        let text = refused
            .and_then(|refused| {
                let OrderRefused::Overlaps { other, side } = refused else {
                    // The queue changed since the window last read it; the
                    // next refresh shows the change.
                    return None;
                };
                let view = self.view.borrow();
                let strings = self.strings.borrow();
                let other = view.as_ref()?.rows.iter().find(|row| row.id == other)?;
                let key = match side {
                    Side::After => "file_operations.must_stay_after",
                    Side::Before => "file_operations.must_stay_before",
                };
                let fonts = self.fonts.borrow();
                let Some(meter) = fonts.as_ref().map(|fonts| TextMeter::new(&fonts.ui)) else {
                    return Some(strings.format(key, &[&other.title.whole(&strings)]));
                };
                // The operation's paths give way in their middle until the
                // sentence fits the hint's line.
                let width = self.controls().hint.hwnd().client_size().width;
                let mut room = width;
                loop {
                    let title = other.title.fitted(
                        &strings,
                        room,
                        &|text| meter.plain_width(text),
                        &|path, width| meter.compact_path(path, width),
                    );
                    let text = strings.format(key, &[&title]);
                    if room.0 <= 0 || meter.plain_width(&text) <= width {
                        return Some(text);
                    }
                    room = Px(room.0 * 9 / 10);
                }
            })
            .unwrap_or_default();
        let shown = self.hint_shown();
        self.controls().hint.hwnd().set_text(&text);
        if shown != !text.is_empty()
            && let Some(hwnd) = self.hwnd.get()
        {
            self.fit_height(hwnd);
        }
    }

    /// Does `action` to the operation `id`.
    fn act(&self, hwnd: Hwnd, id: OperationId, action: QueueAction) {
        let queue = self.queue();
        let rows_step = |up| {
            self.selected_row(|_, rows| Window::step(rows, id, up))
                .flatten()
        };
        match action {
            QueueAction::Pause => queue.set_paused(id, true),
            QueueAction::Resume => queue.set_paused(id, false),
            QueueAction::Cancel => queue.cancel(id),
            QueueAction::Continue => {
                let _ = queue.resume(id);
            }
            QueueAction::Dismiss => {
                let _ = queue.dismiss(id);
            }
            QueueAction::MoveUp | QueueAction::MoveDown => {
                if let Some(before) = rows_step(action == QueueAction::MoveUp) {
                    let refused = queue.move_waiting(id, before).err();
                    self.show_hint(refused);
                }
            }
        }
        self.refresh(hwnd);
        if matches!(action, QueueAction::MoveUp | QueueAction::MoveDown)
            && let Some(index) = self
                .selected_row(|_, rows| rows.iter().position(|row| row.id == id))
                .flatten()
        {
            // The moved row stays in view where the list scrolls.
            self.controls().list.ensure_visible(index);
        }
    }

    fn on_control(&self, hwnd: Hwnd, event: crate::win::notify::ControlEvent) {
        let c = self.controls();
        let card = self
            .view
            .borrow()
            .as_ref()
            .and_then(|view| view.card.as_ref().map(|card| (card.id, card.paused)));
        if c.pause.clicked(event) {
            if let Some((id, paused)) = card {
                let action = if paused {
                    QueueAction::Resume
                } else {
                    QueueAction::Pause
                };
                self.act(hwnd, id, action);
            }
        } else if c.cancel.clicked(event) {
            if let Some((id, _)) = card {
                self.act(hwnd, id, QueueAction::Cancel);
            }
        } else if c.toggle.clicked(event) {
            self.toggle_queue(hwnd);
        } else if c.hide.clicked(event) {
            self.hide(hwnd);
        } else if let Some((id, buttons)) = self.selected_buttons() {
            if c.queue_cancel.clicked(event) {
                self.act(hwnd, id, buttons.end);
            } else if c.run.clicked(event) {
                if let Some(run) = buttons.run {
                    self.act(hwnd, id, run);
                }
            } else if c.up.clicked(event) {
                self.act(hwnd, id, QueueAction::MoveUp);
            } else if c.down.clicked(event) {
                self.act(hwnd, id, QueueAction::MoveDown);
            }
        }
    }

    /// Collapses or expands the queue; the window loses or gets back the
    /// list's height, so the card keeps its place and size.
    fn toggle_queue(&self, hwnd: Hwnd) {
        self.queue_open.set(!self.queue_open.get());
        self.apply_toggle();
        self.fit_height(hwnd);
        self.update_queue_buttons();
    }

    /// The client height of [`Layout::height`], which the window keeps, so
    /// the person sets only its width.
    fn fitted_height(&self, hwnd: Hwnd) -> Px {
        Layout::height(self.metrics(hwnd), self.queue_open.get(), self.hint_shown())
    }

    /// Whether the hint shows a refusal, which gives it a line.
    fn hint_shown(&self) -> bool {
        !self.controls().hint.hwnd().text().is_empty()
    }

    /// Gives the window its [`Self::fitted_height`] and places the
    /// controls. A window that grows past the bottom of the work area moves
    /// up, so Hide stays on the screen.
    fn fit_height(&self, hwnd: Hwnd) {
        let client = hwnd.client_size();
        let height = self.fitted_height(hwnd);
        if client.height != height {
            set_client_size(
                hwnd,
                Size {
                    width: client.width,
                    height,
                },
            );
            let outer = hwnd.screen_rect();
            let area = hwnd.work_area();
            let below = outer.bottom() - area.bottom();
            if area.height > Px(0) && below > Px(0) {
                hwnd.set_rect(Rect {
                    y: (outer.y - below).max(area.y),
                    ..outer
                });
            }
        }
        self.layout(hwnd);
    }

    fn show_menu(&self, hwnd: Hwnd, point: Option<(i32, i32)>) {
        let list = self.controls().list;
        let index = match point {
            Some(point) => list.tile_at(point),
            None => list.selected(),
        };
        let Some(index) = index else {
            return;
        };
        let Some((id, phase)) = self
            .view
            .borrow()
            .as_ref()
            .and_then(|view| view.rows.get(index))
            .map(|row| (row.id, row.phase))
        else {
            return;
        };
        self.selected.set(Some(id));
        self.update_queue_buttons();
        let Some(point) = point.or_else(|| list.menu_point(index)) else {
            return;
        };
        let items = file_operations::row_menu(&self.strings.borrow(), phase);
        let states = [
            (
                QueueAction::MoveUp.id(),
                matches!(self.can_step(true), Ok(true)),
                false,
            ),
            (
                QueueAction::MoveDown.id(),
                matches!(self.can_step(false), Ok(true)),
                false,
            ),
        ];
        // The menu runs its own message loop, so nothing is borrowed here.
        let chosen = Menu::track_popup(hwnd, &items, point, &states, None);
        if let Some(action) = chosen.and_then(QueueAction::from_id) {
            self.act(hwnd, id, action);
        }
    }

    fn begin_drag(&self, hwnd: Hwnd) {
        let Some((id, phase)) = self.selected_row(|row, _| (row.id, row.phase)) else {
            return;
        };
        if phase != RowPhase::Waiting {
            return;
        }
        self.drag.set(Some(Drag { id, gap: None }));
        hwnd.capture_mouse();
        hwnd.set_timer(DRAG_SCROLL, DRAG_SCROLL_MS);
    }

    /// Scrolls the list a row while the dragged row is held above or below
    /// it, and marks the gap the scroll brings under the pointer.
    fn drag_scroll(&self, hwnd: Hwnd) {
        if self.drag.get().is_some()
            && let Some(point) = hwnd.cursor_point()
            && self.controls().list.scroll_toward(point)
        {
            self.drag_to(point);
        }
    }

    /// Marks the gap under `point` with a line, where the queue
    /// allows the drop, and says why where it does not.
    fn drag_to(&self, point: (i32, i32)) {
        let Some(drag) = self.drag.get() else {
            return;
        };
        let list = self.controls().list;
        let gap = list.gap_at(point);
        let checked = gap.and_then(|gap| {
            self.selected_row(|_, rows| {
                let (gap, before) = Window::landing(rows, gap);
                // Just before itself, or before the waiting row after it,
                // is where it already is.
                let next = rows
                    .iter()
                    .filter(|row| row.phase == RowPhase::Waiting)
                    .skip_while(|row| row.id != drag.id)
                    .nth(1)
                    .map(|row| row.id);
                let unchanged = before == Some(drag.id) || before == next;
                (gap, before, unchanged)
            })
        });
        let (gap, refused) = match checked {
            Some((_, _, true)) | None => (None, None),
            Some((gap, before, false)) => match self.queue().check_move(drag.id, before) {
                Ok(()) => (Some(gap), None),
                Err(refused) => (None, Some(refused)),
            },
        };
        if gap != drag.gap {
            list.hwnd().invalidate();
        }
        show_drag_cursor(refused.is_some());
        self.show_hint(refused);
        self.drag.set(Some(Drag { gap, ..drag }));
    }

    fn drop_drag(&self, hwnd: Hwnd) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        window::release_mouse();
        hwnd.kill_timer(DRAG_SCROLL);
        self.controls().list.hwnd().invalidate();
        if let Some(gap) = drag.gap {
            let before = self
                .selected_row(|_, rows| Window::landing(rows, gap).1)
                .flatten();
            let refused = self.queue().move_waiting(drag.id, before).err();
            self.show_hint(refused);
            self.refresh(hwnd);
        }
    }

    fn end_drag(&self) {
        if let Some(hwnd) = self.hwnd.get() {
            hwnd.kill_timer(DRAG_SCROLL);
        }
        if self.drag.take().is_some() {
            self.controls().list.hwnd().invalidate();
        }
    }
}

/// The failure dialog (file-manager-spec.md: File operations, step 7):
/// what went wrong, how many items were done, and each entry that failed
/// or was skipped with its reason.
struct FailureDialog {
    failure: Failure,
    columns: [String; 2],
}

impl DialogProc for FailureDialog {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(instruction) = dialog::item(dialog, ControlId::FailedInstruction.id()) {
            instruction.set_text(&self.failure.instruction);
            if let Ok(font) = Font::of(instruction).and_then(|font| font.instruction()) {
                instruction.set_font(&font);
                fonts.push(font);
            }
        }
        if let Some(body) = dialog::item(dialog, ControlId::FailedBody.id()) {
            body.set_text(&self.failure.body);
        }
        if let Some(list) = ReportList::of_dialog(dialog, ControlId::FailedList.id()) {
            let [item, reason] = &self.columns;
            list.set_columns(&[(item, 60, false), (reason, 40, false)]);
            list.set_rows(&self.failure.rows, None);
            list.fit_columns(&[item, reason], &self.failure.rows);
        }
        Opened {
            focus: DialogItem::Cancel.id(),
            fonts,
        }
    }

    fn message(&self, _dialog: Hwnd, _message: DialogMessage) -> bool {
        false
    }

    fn path_column(&self, id: u16) -> Option<usize> {
        (id == ControlId::FailedList.id()).then_some(0)
    }
}

impl WindowProc for Window {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Size => {
                if self.controls.get().is_some() {
                    self.layout(hwnd);
                }
                Some(LRESULT(0))
            }
            Message::GetMinMaxInfo(mut info) => {
                if self.controls.get().is_some() {
                    let least = window_size(
                        hwnd,
                        Size {
                            width: Layout::min_width(self.metrics(hwnd)),
                            height: self.fitted_height(hwnd),
                        },
                    );
                    info.set_min_track_size(least);
                    info.set_max_track_height(least.height);
                }
                Some(LRESULT(0))
            }
            Message::DpiChanged { suggested, .. } => {
                hwnd.set_rect(suggested);
                self.apply_fonts(hwnd);
                self.fit_height(hwnd);
                Some(LRESULT(0))
            }
            Message::Paint => {
                let layout = self.layout.get();
                let glyph = self
                    .view
                    .borrow()
                    .as_ref()
                    .and_then(|view| view.card.as_ref().map(|card| card.glyph));
                paint_card(
                    hwnd,
                    layout.card,
                    px(CARD_CORNER, hwnd),
                    glyph.map(|glyph| (glyph, layout.icon, px(CARD_ICON, hwnd))),
                );
                Some(LRESULT(0))
            }
            Message::Other(ERASE_BACKGROUND) => Some(LRESULT(1)),
            Message::CtlColorStatic { colors, control } => {
                let c = self.controls.get()?;
                let (_, muted) = c
                    .card_labels()
                    .into_iter()
                    .find(|(label, _)| *label == control)?;
                Some(if muted {
                    colors.muted_text()
                } else {
                    colors.window_background()
                })
            }
            Message::Command(Command::Menu(ESCAPE)) => {
                if self.drag.get().is_some() {
                    window::release_mouse();
                } else {
                    self.hide(hwnd);
                }
                Some(LRESULT(0))
            }
            Message::Command(Command::Control(event)) => {
                self.on_control(hwnd, event);
                Some(LRESULT(0))
            }
            Message::Notify(notification) => {
                let c = self.controls.get()?;
                let paused = self
                    .view
                    .borrow()
                    .as_ref()
                    .and_then(|view| view.card.as_ref().map(|card| card.paused))
                    .unwrap_or(false);
                let queue_open = self.queue_open.get();
                let run = self
                    .selected_buttons()
                    .and_then(|(_, buttons)| buttons.run)
                    .map_or(Glyph::Play, QueueAction::glyph);
                let flat = [
                    (
                        c.pause,
                        if paused { Glyph::Play } else { Glyph::Pause },
                        FlatGround::Window,
                    ),
                    (c.cancel, Glyph::Cancel, FlatGround::Window),
                    (
                        c.toggle,
                        if queue_open {
                            Glyph::ChevronDown
                        } else {
                            Glyph::ChevronRight
                        },
                        FlatGround::Face,
                    ),
                    (c.run, run, FlatGround::Face),
                    (c.up, Glyph::ChevronUp, FlatGround::Face),
                    (c.down, Glyph::ChevronDown, FlatGround::Face),
                    (c.queue_cancel, Glyph::Cancel, FlatGround::Face),
                ];
                for (button, glyph, ground) in flat {
                    if let Some(answer) = button.draw_flat(&notification, glyph, ground) {
                        return Some(answer);
                    }
                }
                match c.list.notification(notification)? {
                    QueueNotify::SelectionChanged => {
                        let index = c.list.selected();
                        let id = index.and_then(|index| {
                            self.view
                                .borrow()
                                .as_ref()
                                .and_then(|view| view.rows.get(index).map(|row| row.id))
                        });
                        self.selected.set(id);
                        self.update_queue_buttons();
                        None
                    }
                    QueueNotify::BeginDrag => {
                        self.begin_drag(hwnd);
                        None
                    }
                    QueueNotify::DeleteKey => {
                        // Dismiss cannot be undone, so only the button
                        // dismisses, not a key pressed by mistake.
                        if let Some((id, buttons)) = self.selected_buttons()
                            && buttons.end == QueueAction::Cancel
                        {
                            self.act(hwnd, id, QueueAction::Cancel);
                        }
                        None
                    }
                    QueueNotify::EmptyText(empty) => {
                        Some(empty.set(self.strings.borrow().text("file_operations.queue_empty")))
                    }
                    QueueNotify::InfoTip(tip) => {
                        let view = self.view.borrow();
                        let row = tip.row().and_then(|row| view.as_ref()?.rows.get(row));
                        let strings = self.strings.borrow();
                        let text = row.map_or_else(String::new, |row| {
                            format!(
                                "{}\n{}",
                                row.title.whole(&strings),
                                row.detail.whole(&strings)
                            )
                        });
                        Some(tip.set(&text))
                    }
                    QueueNotify::CustomDraw(draw) => {
                        Some(c.list.draw(draw, self.drag.get().and_then(|drag| drag.gap)))
                    }
                    QueueNotify::Other => None,
                }
            }
            Message::ContextMenu { from, point } => {
                let list = self.controls.get()?.list;
                if from != list.hwnd() {
                    return None;
                }
                self.show_menu(hwnd, point);
                Some(LRESULT(0))
            }
            Message::MouseMove { point } => {
                self.drag_to(point);
                Some(LRESULT(0))
            }
            Message::LeftButtonUp => {
                self.drop_drag(hwnd);
                Some(LRESULT(0))
            }
            Message::CaptureChanged => {
                self.end_drag();
                Some(LRESULT(0))
            }
            Message::Timer(DRAG_SCROLL) => {
                self.drag_scroll(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(POLL | OPEN) | Message::Wake => {
                self.refresh(hwnd);
                Some(LRESULT(0))
            }
            Message::SysColorChange => {
                self.apply_fonts(hwnd);
                hwnd.invalidate();
                None
            }
            Message::Close => {
                self.hide(hwnd);
                Some(LRESULT(0))
            }
            Message::Destroy => {
                if let Some(waker) = self.waker.get() {
                    waker.close();
                }
                window::set_modeless(None);
                hwnd.kill_timer(POLL);
                hwnd.kill_timer(OPEN);
                self.taskbar.show(TaskbarState::None);
                None
            }
            _ => None,
        }
    }
}
