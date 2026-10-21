//! The first window's file operations (DESIGN.md: File operations): the
//! engine's queue, and the File operations window, a
//! window of its own that the main window owns, with a card for the running
//! operation and the queue list under it. The window never blocks Gozler:
//! closing it only hides it, and the operations go on.
//!
//! The queue calls its wake function on an engine thread after each change
//! of an operation's state; the wake only posts [`QUEUE_CHANGED`] to the
//! main window, which then reads the states and runs what
//! `core::operations` decides, also when this window shows. A running
//! operation's counts change without a wake, so the main window polls them
//! twice a second while one runs.

use crate::core::captions::{ButtonIcon, DialogCaptions, DialogItem};
use crate::core::file_operations::{
    self, Bar, CARD_ICON, Failure, Layout, Meter, Metrics, OPEN_WIDTH, QueueAction, ROW_GLYPHS,
    ROW_ICON, Row, RowButtons, RowLine, RowPhase, Taskbar, View, icon_index,
};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::layout::{Px, Rect, Size, button_content, button_room};
use crate::core::operations::{QueueStep, Source, SourceStep};
use crate::core::text::Strings;
use crate::win::clipboard::ClipboardFiles;
use crate::win::controls::{Button, GlyphLabel, Label, ProgressBar, ReportList, Tooltip};
use crate::win::dialog::{self, DialogMessage, DialogProc, Opened};
use crate::win::drop_target::AsyncDrop;
use crate::win::file_operations::{
    PathLabel, QUEUE_CHANGED, QueueList, QueueNotify, TaskbarProgress, Tile, set_client_size,
    show_drag_cursor, show_paused, tell_hidden, window_size,
};
use crate::win::gdi::{Font, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::menu::Menu;
use crate::win::notify::Command;
use crate::win::wake::OpenWaker;
use crate::win::window::{self, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_fileops::{
    Operation, OperationId, OrderRefused, Queue, Request, Side, SpeedSettings, State, Taken,
};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use windows::Win32::Foundation::LRESULT;

const WINDOW_CLASS: &str = "gozler.ui.file_operations";

/// The timer that scrolls the queue list while a dragged row is held above
/// or below it.
const DRAG_SCROLL: usize = 2;
/// A row each tenth of a second, so a long queue scrolls quickly and a
/// short hold moves it a row or two.
const DRAG_SCROLL_MS: u32 = 100;

/// `IDCANCEL`, which `Esc` sends through `IsDialogMessageW`.
const ESCAPE: u16 = 2;

/// The queue and the File operations window, as the main window holds them.
pub(super) struct FileOperations {
    owner: Hwnd,
    /// Never dropped, because dropping it waits for every Windows copy, and
    /// a copy that Windows stops deletes its file, while the process ending
    /// leaves that file for Continue.
    queue: Rc<ManuallyDrop<Queue>>,
    /// Wakes the owner from the engine's threads, which outlive it, until
    /// [`FileOperations::close`].
    waker: OpenWaker,
    strings: RefCell<Strings>,
    /// The File operations window, made the first time it shows; `None`
    /// inside when it could not open, and the operations then show only in
    /// the status bar and on the taskbar button.
    window: OnceCell<Option<Rc<Window>>>,
    /// Where the window's failure to open is written.
    error_log: PathBuf,
    /// Where the files of each copy or move came from, until the source
    /// hears what became of them.
    sources: RefCell<HashMap<OperationId, FilesSource>>,
    /// Made at the first operation, because the taskbar's interface costs
    /// start-up time.
    taskbar: OnceCell<TaskbarProgress>,
}

struct Controls {
    icon: GlyphLabel,
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
    /// The labels of the running operation; the `muted` ones step back in
    /// gray.
    fn card_labels(&self) -> [(Hwnd, bool); 12] {
        [
            (self.icon.hwnd(), false),
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
    queue: Rc<ManuallyDrop<Queue>>,
    owner: Hwnd,
    strings: RefCell<Strings>,
    numbers: RefCell<WindowsNumbers>,
    controls: OnceCell<Controls>,
    fonts: RefCell<Option<Fonts>>,
    layout: Cell<Layout>,
    meters: RefCell<HashMap<OperationId, Meter>>,
    /// What the window shows now.
    view: RefCell<Option<View>>,
    selected: Cell<Option<OperationId>>,
    queue_open: Cell<bool>,
    drag: Cell<Option<Drag>>,
    /// Why the queue refused the move the person tried or points at, which
    /// the hint names and which gives the hint its line.
    refused: Cell<Option<OrderRefused>>,
    /// The row that Move up or Move down moved, which the next view keeps
    /// in sight.
    moved: Cell<Option<OperationId>>,
}

/// Where the files that an operation copies or moves came from.
pub(super) enum FilesSource {
    /// The clipboard, which Paste read; a move of cut files tells it that
    /// they moved.
    Paste(ClipboardFiles),
    /// A drop whose source keeps its files until the operation ends.
    Drop(AsyncDrop),
}

impl FilesSource {
    pub(super) fn kind(&self) -> Source {
        match self {
            FilesSource::Paste(files) => Source::Paste { cut: files.cut },
            FilesSource::Drop(_) => Source::Drop,
        }
    }
}

impl FileOperations {
    /// Opens the queue, whose saved operations are in `saved`, and has
    /// `owner` read them at once, so interrupted ones show. The window opens
    /// the first time it shows; `error_log` gets the reason when it cannot.
    pub(super) fn new(
        owner: Hwnd,
        saved: &Path,
        strings: Strings,
        error_log: PathBuf,
    ) -> FileOperations {
        let waker = OpenWaker::posting(owner, QUEUE_CHANGED);
        let wakes = waker.clone();
        let queue = Queue::new(saved, move || wakes.deliver(|| true));
        owner.post(QUEUE_CHANGED);
        FileOperations {
            owner,
            queue: Rc::new(ManuallyDrop::new(queue)),
            waker,
            strings: RefCell::new(strings),
            window: OnceCell::new(),
            error_log,
            sources: RefCell::new(HashMap::new()),
            taskbar: OnceCell::new(),
        }
    }

    /// The entries that deletes and moves take, which the lists leave out.
    /// Read before [`Self::snapshot`], so every operation they have as
    /// ended is in it.
    pub(super) fn taken(&self) -> Arc<Taken> {
        self.queue.taken()
    }

    /// Every operation in the queue now.
    pub(super) fn snapshot(&self) -> Vec<Operation> {
        self.queue.operations()
    }

    /// Prepares `request`; the main window asks about its conflicts when the
    /// count is ready, and then submits it. `source` is where the files
    /// that the request copies or moves came from, which hears what became
    /// of them.
    pub(super) fn start(
        &self,
        request: Request,
        settings: SpeedSettings,
        source: Option<FilesSource>,
    ) -> OperationId {
        let id = self.queue.prepare(request, settings);
        if let Some(source) = source {
            self.sources.borrow_mut().insert(id, source);
        }
        id
    }

    /// Runs `step` on the queue. Returns whether the queue accepted it,
    /// which only a submit can refuse.
    pub(super) fn run(&self, step: QueueStep) -> bool {
        match step {
            QueueStep::Submit { id, choice } => self.queue.submit(id, choice).is_ok(),
            QueueStep::Cancel(id) => {
                self.queue.cancel(id);
                true
            }
            QueueStep::Remove(id) => {
                self.queue.remove(id);
                true
            }
        }
    }

    /// Tells an operation's source what `step` says became of its files.
    /// Calls the source, which can be another program.
    pub(super) fn tell_source(&self, step: SourceStep) {
        match step {
            SourceStep::Pasted { id, moved } => {
                let source = self.sources.borrow_mut().remove(&id);
                if let Some(FilesSource::Paste(files)) = source
                    && moved
                {
                    files.moved();
                }
            }
            SourceStep::Ended { id, succeeded } => {
                let source = self.sources.borrow_mut().remove(&id);
                if let Some(FilesSource::Drop(dropped)) = source {
                    dropped.end(succeeded);
                }
            }
        }
    }

    /// Shows the window with `snapshot`, whose operations' source folders
    /// are in `folders`, and with `select` selected; `front` brings it to
    /// the front, as View > File operations does.
    pub(super) fn show(
        &self,
        snapshot: &[Operation],
        folders: &HashMap<OperationId, Option<String>>,
        select: Option<OperationId>,
        front: bool,
    ) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(hwnd) = window.hwnd.get() else {
            return;
        };
        if select.is_some() {
            window.selected.set(select);
        }
        window.show_snapshot(hwnd, snapshot, folders);
        if front {
            hwnd.bring_to_front();
        } else {
            hwnd.set_visible(true);
        }
    }

    /// Shows `snapshot` in the window, if it was made.
    pub(super) fn refresh(
        &self,
        snapshot: &[Operation],
        folders: &HashMap<OperationId, Option<String>>,
    ) {
        if let Some(window) = self.made()
            && let Some(hwnd) = window.hwnd.get()
        {
            window.show_snapshot(hwnd, snapshot, folders);
        }
    }

    /// Whether the window is on the screen.
    pub(super) fn is_shown(&self) -> bool {
        (self.made().and_then(|window| window.hwnd.get())).is_some_and(Hwnd::is_visible)
    }

    pub(super) fn hide(&self) {
        if let Some(hwnd) = self.made().and_then(|window| window.hwnd.get()) {
            hwnd.hide();
        }
    }

    /// Shows `state` on the owner's taskbar button.
    pub(super) fn show_taskbar(&self, state: Taskbar) {
        if state == Taskbar::None && self.taskbar.get().is_none() {
            return;
        }
        (self.taskbar)
            .get_or_init(|| TaskbarProgress::new(self.owner))
            .show(state);
    }

    /// Explorer made the owner's taskbar button again, which shows no
    /// progress until the next [`Self::show_taskbar`].
    pub(super) fn forget_taskbar(&self) {
        if let Some(taskbar) = self.taskbar.get() {
            taskbar.forget();
        }
    }

    /// Words the window in the language of `strings`; the operations' own
    /// texts follow at the next [`Self::refresh`].
    pub(super) fn set_strings(&self, strings: Strings) {
        self.strings.replace(strings.clone());
        let Some(window) = self.made() else {
            return;
        };
        window.strings.replace(strings);
        window.apply_strings();
        // The labels and Hide are as wide as their new words.
        if let Some(hwnd) = window.hwnd.get() {
            window.layout(hwnd);
        }
    }

    /// Shows or hides Hide's icon, after icons were shown or hidden.
    pub(super) fn apply_glyphs(&self) {
        if let Some(window) = self.made() {
            window.apply_glyphs();
        }
    }

    /// The owner's `WM_DESTROY`: no wake reaches the owner any more, every
    /// operation stops so that Continue finishes it at the next start, and
    /// the window goes. The process then ends without dropping the queue.
    pub(super) fn close(&self) {
        self.waker.close();
        self.queue.close();
        // The closed queue ends no more operations, so each source hears
        // that its operation failed.
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        drop(sources);
        if let Some(hwnd) = self.made().and_then(|window| window.hwnd.take()) {
            hwnd.destroy();
        }
    }

    /// The window, made now if it was not.
    fn window(&self) -> Option<&Rc<Window>> {
        let made = self.window.get_or_init(|| {
            let strings = self.strings.borrow().clone();
            match Window::open(self.owner, Rc::clone(&self.queue), strings) {
                Ok(window) => Some(window),
                Err(error) => {
                    super::write_log(
                        &self.error_log,
                        &format!("The File operations window did not open: {error}"),
                    );
                    None
                }
            }
        });
        made.as_ref()
    }

    /// The window, if it was made.
    fn made(&self) -> Option<&Rc<Window>> {
        self.window.get().and_then(Option::as_ref)
    }
}

/// Shows what failed in an ended operation: the failure dialog, or a
/// message box when nothing is listed. It runs its own message loop.
pub(super) fn show_failure(owner: Hwnd, strings: &Strings, failure: Failure) {
    if failure.rows.is_empty() {
        // A refusal, or a full disk before any entry failed, lists
        // nothing, so the message stands alone.
        let text = format!("{}\n\n{}", failure.instruction, failure.body);
        dialog::error_box(Some(owner), &text, &failure.title);
        return;
    }
    let mut captions = DialogCaptions::new(DialogId::FileOperationsFailed, strings);
    captions.title.clone_from(&failure.title);
    let dialog = FailureDialog {
        failure,
        columns: [
            strings.text("file_operations.column_item").to_owned(),
            strings.text("file_operations.column_reason").to_owned(),
        ],
    };
    let _ = dialog::run_modal(owner, DialogId::FileOperationsFailed, &captions, &dialog);
}

/// `dip` at the DPI of `hwnd`.
fn px(dip: i32, hwnd: Hwnd) -> Px {
    Px(dip).rescale(
        crate::core::layout::Dpi::new(96).expect("96 is not zero"),
        hwnd.dpi(),
    )
}

impl Window {
    /// Creates the window, hidden, owned by `owner`.
    fn open(
        owner: Hwnd,
        queue: Rc<ManuallyDrop<Queue>>,
        strings: Strings,
    ) -> windows::core::Result<Rc<Window>> {
        window::register_class::<Window>(
            WINDOW_CLASS,
            crate::core::ids::IconId::Gozler.id(),
            window::Background::Dialog,
        )?;
        let window = Rc::new(Window {
            hwnd: Cell::new(None),
            queue,
            owner,
            strings: RefCell::new(strings),
            numbers: RefCell::new(WindowsNumbers::new()),
            controls: OnceCell::new(),
            fonts: RefCell::new(None),
            layout: Cell::new(Layout::default()),
            meters: RefCell::new(HashMap::new()),
            view: RefCell::new(None),
            selected: Cell::new(None),
            queue_open: Cell::new(true),
            drag: Cell::new(None),
            refused: Cell::new(None),
            moved: Cell::new(None),
        });
        let hwnd = window::create_owned_window(WINDOW_CLASS, Rc::clone(&window), owner)?;
        window.hwnd.set(Some(hwnd));
        window::set_modeless(Some(hwnd));
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
        Ok(window)
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
            icon: GlyphLabel::create(hwnd, id(ControlId::FileOperationsIcon))?,
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

    /// Gives each button and the card its icon at the window's DPI and in
    /// the system colours of now, so a change of either calls this again.
    /// Hide shows its icon only while icons show; the other buttons show
    /// only their icon.
    fn apply_glyphs(&self) {
        self.controls()
            .hide
            .set_icon(ButtonIcon::of("file_operations.hide", false));
        self.show_state_glyphs();
    }

    /// Gives the card and the buttons whose icon follows the operations,
    /// such as Pause or Resume, the icon of what the window shows now.
    fn show_state_glyphs(&self) {
        let c = self.controls();
        let card = self
            .view
            .borrow()
            .as_ref()
            .and_then(|view| view.card.as_ref().map(|card| (card.glyph, card.paused)));
        if let Some((glyph, _)) = card {
            c.icon.show_glyph(glyph, px(CARD_ICON, c.icon.hwnd()));
        }
        let paused = card.is_some_and(|(_, paused)| paused);
        let run = self
            .selected_buttons()
            .and_then(|(_, buttons)| buttons.run)
            .map_or(Glyph::Play, QueueAction::glyph);
        let toggle = if self.queue_open.get() {
            Glyph::ChevronDown
        } else {
            Glyph::ChevronRight
        };
        for (button, glyph) in [
            (c.pause, if paused { Glyph::Play } else { Glyph::Pause }),
            (c.cancel, Glyph::Cancel),
            (c.toggle, toggle),
            (c.run, run),
            (c.up, Glyph::ChevronUp),
            (c.down, Glyph::ChevronDown),
            (c.queue_cancel, Glyph::Cancel),
        ] {
            button.set_lone_icon(Some(glyph));
        }
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
            (c.icon.hwnd(), layout.icon),
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

    /// Shows `snapshot`, whose operations' source folders are in
    /// `folders`, with the speeds of the running ones from the counts the
    /// window saw before.
    fn show_snapshot(
        &self,
        hwnd: Hwnd,
        snapshot: &[Operation],
        folders: &HashMap<OperationId, Option<String>>,
    ) {
        let now = Instant::now();
        {
            let mut meters = self.meters.borrow_mut();
            meters.retain(|id, _| {
                (snapshot.iter())
                    .any(|entry| entry.id == *id && matches!(entry.state, State::Running(_)))
            });
            for entry in snapshot {
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
            let operations: Vec<file_operations::Operation> = (snapshot.iter())
                .map(|entry| file_operations::Operation {
                    id: entry.id,
                    request: &entry.request,
                    state: &entry.state,
                    folder: folders.get(&entry.id).and_then(Option::as_deref),
                })
                .collect();
            file_operations::view(
                &self.strings.borrow(),
                &*self.numbers.borrow(),
                &operations,
                &self.meters.borrow(),
            )
        };
        self.show_view(hwnd, view);
    }

    /// Hides the window, and tells the owner, whose operations that have
    /// not ended then do not show it again.
    fn hide(&self, hwnd: Hwnd) {
        hwnd.hide();
        tell_hidden(self.owner);
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
                    c.icon.hwnd(),
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
                    c.icon.hwnd(),
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
        let moved =
            (self.moved.take()).and_then(|id| view.rows.iter().position(|row| row.id == id));
        drop(strings);
        // Stored first: the list reports the selection it is given at once,
        // and the window reads the row it selects from the view.
        self.view.replace(Some(view));
        self.show_tiles();
        self.update_queue_buttons();
        if let Some(index) = moved {
            // The moved row stays in view where the list scrolls.
            c.list.ensure_visible(index);
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

    /// The selected row, as the window shows it.
    fn selected_row<T>(&self, use_row: impl FnOnce(&Row, &[Row]) -> T) -> Option<T> {
        let view = self.view.borrow();
        let rows = &view.as_ref()?.rows;
        let id = self.selected.get()?;
        let row = rows.iter().find(|row| row.id == id)?;
        Some(use_row(row, rows))
    }

    /// Whether the queue lets the selected row move up or down, and the
    /// hint that names why not.
    fn can_step(&self, up: bool) -> Result<bool, OrderRefused> {
        let step = self.selected_row(|row, rows| {
            file_operations::move_step(rows, row.id, up).map(|before| (row.id, before))
        });
        match step.flatten() {
            Some((id, before)) => self.queue.check_move(id, before).map(|()| true),
            None => Ok(false),
        }
    }

    /// The selected row, and what the queue's header buttons do to it.
    fn selected_buttons(&self) -> Option<(OperationId, RowButtons)> {
        self.selected_row(|row, _| (row.id, row.phase.buttons()))
    }

    /// Enables the queue's buttons for the selected row, shows the hint
    /// when the queue refuses one of its moves, and gives the buttons and
    /// the card the icons of what the window shows now.
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
        self.show_state_glyphs();
    }

    /// Shows why the queue refused a move, or clears the hint.
    fn show_hint(&self, refused: Option<OrderRefused>) {
        let shown = self.hint_shown();
        self.refused.set(refused);
        let text = self.hint_text();
        if text.is_none() {
            self.refused.set(None);
        }
        self.controls()
            .hint
            .hwnd()
            .set_text(text.as_deref().unwrap_or_default());
        if shown != self.hint_shown()
            && let Some(hwnd) = self.hwnd.get()
        {
            self.fit_height(hwnd);
        }
    }

    /// The hint's sentence for [`Self::refused`], whose operation's paths
    /// give way in their middle so that the sentence fits the hint's line.
    /// `None` when the refusal names no row: the queue changed since the
    /// window last read it, and the next view shows the change.
    fn hint_text(&self) -> Option<String> {
        let OrderRefused::Overlaps { other, side } = self.refused.get()? else {
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
        let width = self.controls().hint.hwnd().client_size().width;
        let room = width - meter.plain_width(&strings.format(key, &[""]));
        let title = other.title.fitted(
            &strings,
            room,
            &|text| meter.plain_width(text),
            &|path, width| meter.compact_path(path, width),
        );
        Some(strings.format(key, &[&title]))
    }

    /// Does `action` to the operation `id`. The queue then wakes the owner,
    /// which shows what changed.
    fn act(&self, id: OperationId, action: QueueAction) {
        let queue = &self.queue;
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
                let up = action == QueueAction::MoveUp;
                let step = self.selected_row(|_, rows| file_operations::move_step(rows, id, up));
                if let Some(before) = step.flatten() {
                    let refused = queue.move_waiting(id, before).err();
                    if refused.is_none() {
                        self.moved.set(Some(id));
                    }
                    self.show_hint(refused);
                }
            }
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
                self.act(id, action);
            }
        } else if c.cancel.clicked(event) {
            if let Some((id, _)) = card {
                self.act(id, QueueAction::Cancel);
            }
        } else if c.toggle.clicked(event) {
            self.toggle_queue(hwnd);
        } else if c.hide.clicked(event) {
            self.hide(hwnd);
        } else if let Some((id, buttons)) = self.selected_buttons() {
            if c.queue_cancel.clicked(event) {
                self.act(id, buttons.end);
            } else if c.run.clicked(event) {
                if let Some(run) = buttons.run {
                    self.act(id, run);
                }
            } else if c.up.clicked(event) {
                self.act(id, QueueAction::MoveUp);
            } else if c.down.clicked(event) {
                self.act(id, QueueAction::MoveDown);
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
        self.refused.get().is_some()
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
            self.act(id, action);
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
                let (gap, before) = file_operations::landing(rows, gap);
                (
                    gap,
                    before,
                    file_operations::stays_in_place(rows, drag.id, before),
                )
            })
        });
        let (gap, refused) = match checked {
            Some((_, _, true)) | None => (None, None),
            Some((gap, before, false)) => match self.queue.check_move(drag.id, before) {
                Ok(()) => (Some(gap), None),
                Err(refused) => (None, Some(refused)),
            },
        };
        if gap != drag.gap {
            list.set_insert_mark(gap);
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
        self.controls().list.set_insert_mark(None);
        if let Some(gap) = drag.gap {
            let before = self
                .selected_row(|_, rows| file_operations::landing(rows, gap).1)
                .flatten();
            let refused = self.queue.move_waiting(drag.id, before).err();
            self.show_hint(refused);
        }
    }

    fn end_drag(&self) {
        if let Some(hwnd) = self.hwnd.get() {
            hwnd.kill_timer(DRAG_SCROLL);
        }
        if self.drag.take().is_some() {
            self.controls().list.set_insert_mark(None);
        }
    }
}

/// The failure dialog (DESIGN.md: File operations): what went wrong, how many
/// items were done, and each entry that failed or was skipped with its reason.
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
            list.show_paths(0, &self.failure.rows);
            list.show_info_tips();
        }
        Opened {
            focus: DialogItem::Cancel.id(),
            fonts,
        }
    }

    fn message(&self, _dialog: Hwnd, _message: DialogMessage) -> bool {
        false
    }

    /// The entry's whole path and reason, which the row can show cut off.
    fn info_tip(&self, id: u16, row: usize) -> Option<String> {
        let [path, reason] = self
            .failure
            .rows
            .get(row)
            .filter(|_| id == ControlId::FailedList.id())?;
        Some(format!(
            "{path}
{reason}"
        ))
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
            Message::CtlColorStatic { colors, control } => {
                let c = self.controls.get()?;
                c.card_labels()
                    .into_iter()
                    .any(|(label, muted)| muted && label == control)
                    .then(|| colors.muted_on_dialog())
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
                            self.act(id, QueueAction::Cancel);
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
                window::set_modeless(None);
                None
            }
            _ => None,
        }
    }
}
