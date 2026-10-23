//! Owns the engine Queue and the shared inspector's native transfer controls.
//! Engine wakes reach the main window; only that window accepts snapshots and
//! runs lifecycle decisions. The child content has no independent placement,
//! activation or message loop. Reports read immutable outcomes on demand.

use crate::core::captions::{ButtonIcon, DialogCaptions, DialogItem};
use crate::core::file_operations::{
    self, Bar, CARD_ICON, Detail, Failure, Layout, Metrics, QueueAction, ROW_GLYPHS, ROW_ICON, Row,
    RowButtons, RowLine, RowPhase, Taskbar, View, icon_index,
};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::layout::{Px, Rect, button_content, button_room};
use crate::core::operations::{QueueStep, Source, SourceStep};
use crate::core::text::Strings;
use crate::win::clipboard::ClipboardFiles;
use crate::win::controls::{
    Button, GlyphLabel, Label, ListColumn, ListView, ProgressBar, ReportList, Tooltip,
};
use crate::win::dialog::{self, DialogMessage, DialogProc, Opened};
use crate::win::drop_target::DropSource;
use crate::win::file_operations::{
    PathLabel, QUEUE_CHANGED, QueueList, QueueNotify, TaskbarProgress, Tile, show_drag_cursor,
    show_paused,
};
use crate::win::gdi::{Font, TextMeter};
use crate::win::menu::Menu;
use crate::win::notify::{Command, ListNotify};
use crate::win::wake::OpenWaker;
use crate::win::window::{self, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_fileops::{
    Operation, OperationId, OrderRefused, Queue, Request, Side, SpeedSettings, Taken,
};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
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

/// The queue and the transfer controls in the shared inspector.
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
    /// The transfer content, made the first time it shows; `None`
    /// inside when it could not open, and the operations then show only in
    /// the status bar and on the taskbar button.
    window: OnceCell<Option<Rc<Pane>>>,
    /// Where a failure to create the transfer controls is written.
    error_log: PathBuf,
    /// Where the files of each copy or move came from, until the source
    /// hears what became of them.
    sources: RefCell<HashMap<OperationId, FilesSource>>,
    /// Made at the first operation, because the taskbar's interface costs
    /// start-up time.
    taskbar: OnceCell<TaskbarProgress>,
    actions: Rc<RefCell<Vec<PaneAction>>>,
}

#[derive(Clone, Copy)]
pub(super) enum PaneAction {
    Select(Detail),
    Command(OperationId, QueueAction),
    Move(OperationId, Option<OperationId>),
    CancelAll,
    CloseReport,
    Collapse,
    Maximize,
    Flip,
}

struct Controls {
    heading: Label,
    count: Label,
    all: Button,
    flip: Button,
    maximize: Button,
    rate: Label,
    files_rate: Label,
    file_percent: Label,
    file_bytes: Label,
    overall: Label,
    report: ListView,
    summary: Label,
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
    queue: Label,
    list: QueueList,
    hint: Label,
    hide: Button,
    tips: Tooltip,
}

impl Controls {
    fn card_labels(&self) -> [Hwnd; 17] {
        [
            self.icon.hwnd(),
            self.title.hwnd(),
            self.status.hwnd(),
            self.doing.hwnd(),
            self.file.hwnd(),
            self.from_label.hwnd(),
            self.from.hwnd(),
            self.to_label.hwnd(),
            self.to.hwnd(),
            self.overall.hwnd(),
            self.percent.hwnd(),
            self.items.hwnd(),
            self.bytes.hwnd(),
            self.file_percent.hwnd(),
            self.file_bytes.hwnd(),
            self.rate.hwnd(),
            self.files_rate.hwnd(),
        ]
    }
    fn text_controls(&self) -> Vec<Hwnd> {
        self.card_labels()
            .into_iter()
            .chain([
                self.heading.hwnd(),
                self.count.hwnd(),
                self.queue.hwnd(),
                self.list.hwnd(),
                self.hint.hwnd(),
                self.hide.hwnd(),
                self.all.hwnd(),
                self.flip.hwnd(),
                self.maximize.hwnd(),
                self.pause.hwnd(),
                self.cancel.hwnd(),
                self.report.hwnd(),
                self.summary.hwnd(),
            ])
            .collect()
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

struct Pane {
    actions: Rc<RefCell<Vec<PaneAction>>>,
    hwnd: Cell<Option<Hwnd>>,
    queue: Rc<ManuallyDrop<Queue>>,
    owner: Hwnd,
    strings: RefCell<Strings>,
    controls: OnceCell<Controls>,
    fonts: RefCell<Option<Fonts>>,
    layout: Cell<Layout>,
    metrics: Cell<Option<Metrics>>,
    /// What the window shows now.
    view: RefCell<Option<View>>,
    flipped: Cell<bool>,
    maximized: Cell<bool>,
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
    Drop(DropSource),
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
            actions: Rc::default(),
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

    pub(super) fn accept(&self, request: Request, settings: SpeedSettings) -> OperationId {
        self.queue.accept(request, settings)
    }

    pub(super) fn resolve(
        &self,
        id: OperationId,
        request: Request,
        source: Option<FilesSource>,
    ) -> bool {
        if let Some(source) = source {
            self.sources.borrow_mut().insert(id, source);
        }
        let accepted = self.queue.resolve(id, request);
        if !accepted {
            let source = self.sources.borrow_mut().remove(&id);
            if let Some(FilesSource::Drop(source)) = source {
                source.end(false);
            }
        }
        accepted
    }

    pub(super) fn cancel_all(&self) {
        self.queue.cancel_all();
    }

    pub(super) fn take_actions(&self) -> Vec<PaneAction> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }

    pub(super) fn fail_intake(&self, id: OperationId) {
        self.queue.fail_intake(id);
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
                match source {
                    Some(FilesSource::Drop(dropped)) => dropped.end(succeeded),
                    Some(FilesSource::Paste(files)) if succeeded && files.cut => files.moved(),
                    _ => {}
                }
            }
        }
    }

    pub(super) fn act(&self, id: OperationId, action: QueueAction) {
        match action {
            QueueAction::Pause => self.queue.set_paused(id, true),
            QueueAction::Resume => self.queue.set_paused(id, false),
            QueueAction::Cancel => self.queue.cancel(id),
            QueueAction::Continue => {
                let _ = self.queue.resume(id);
            }
            QueueAction::Dismiss => {
                let _ = self.queue.dismiss(id);
            }
            QueueAction::MoveUp | QueueAction::MoveDown => {
                if let Some(window) = self.made() {
                    let before = window.view.borrow().as_ref().and_then(|view| {
                        file_operations::move_step(&view.rows, id, action == QueueAction::MoveUp)
                    });
                    if let Some(before) = before {
                        let refused = self.queue.move_waiting(id, before).err();
                        if refused.is_none() {
                            window.moved.set(Some(id));
                        }
                        window.show_hint(refused);
                    }
                }
            }
            QueueAction::Report | QueueAction::Clear => {}
        }
    }

    pub(super) fn move_before(&self, id: OperationId, before: Option<OperationId>) {
        let refused = self.queue.move_waiting(id, before).err();
        if let Some(window) = self.made() {
            window.show_hint(refused);
        }
    }

    /// Updates explicitly opened content without activating the main window.
    pub(super) fn show(&self, view: View) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(hwnd) = window.hwnd.get() else {
            return;
        };
        window.show_view(hwnd, view);
        hwnd.set_visible(true);
    }

    pub(super) fn place(&self, area: Option<Rect>, flipped: bool, maximized: bool) {
        let Some(window) = (if area.is_some() {
            self.window()
        } else {
            self.made()
        }) else {
            return;
        };
        let Some(hwnd) = window.hwnd.get() else {
            return;
        };
        window.flipped.set(flipped);
        if window.maximized.replace(maximized) != maximized {
            window.apply_strings();
        }
        if let Some(area) = area {
            win::move_children(&[(hwnd, area)]);
            window.layout(hwnd);
        }
        hwnd.set_visible(area.is_some());
    }

    pub(super) fn key(&self, target: Hwnd, key: crate::core::commands::Keystroke) -> bool {
        use crate::core::commands::Key;
        if !self.contains(target) {
            return false;
        }
        let Some(window) = self.made() else {
            return false;
        };
        if key.key == Key::Escape && !key.alt && !key.ctrl {
            window.post(PaneAction::Collapse);
            return true;
        }
        if target != window.controls().list.hwnd() {
            return false;
        }
        let Some(id) = window.selected() else {
            return false;
        };
        if key.alt && !key.ctrl && !key.shift && matches!(key.key, Key::Up | Key::Down) {
            window.act(
                id,
                if key.key == Key::Up {
                    QueueAction::MoveUp
                } else {
                    QueueAction::MoveDown
                },
            );
            return true;
        }
        if key.key == Key::Enter
            && !key.ctrl
            && !key.alt
            && !key.shift
            && window
                .selected_buttons()
                .is_some_and(|(_, b)| b.run == Some(QueueAction::Report))
        {
            window.act(id, QueueAction::Report);
            return true;
        }
        false
    }

    pub(super) fn contains(&self, control: Hwnd) -> bool {
        self.made()
            .and_then(|w| w.hwnd.get())
            .is_some_and(|hwnd| hwnd.is_parent_of(control))
    }

    pub(super) fn focus(&self) {
        if let Some(window) = self.made() {
            let c = window.controls();
            if c.list.hwnd().is_visible() {
                c.list.hwnd().focus();
            } else {
                c.hide.hwnd().focus();
            }
        }
    }

    /// Shows `snapshot` in the window, if it was made.
    pub(super) fn refresh(&self, view: View) {
        if let Some(window) = self.made()
            && let Some(hwnd) = window.hwnd.get()
        {
            window.show_view(hwnd, view);
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

    /// Words the pane in the language of `strings`; the operations' own
    /// texts follow at the next [`Self::refresh`].
    pub(super) fn set_strings(&self, strings: Strings) {
        self.strings.replace(strings.clone());
        let Some(window) = self.made() else {
            return;
        };
        window.strings.replace(strings);
        window.metrics.set(None);
        window.apply_strings();
        // The labels and Hide are as wide as their new words.
        if let Some(hwnd) = window.hwnd.get() {
            window.layout(hwnd);
            window.show_tiles(false);
            let report = window.controls().report;
            report.redraw_rows(0..report.count());
        }
    }

    /// Shows or hides Hide's icon, after icons were shown or hidden.
    pub(super) fn apply_glyphs(&self) {
        if let Some(window) = self.made()
            && let Some(hwnd) = window.hwnd.get()
        {
            window.apply_fonts(hwnd);
            window.layout(hwnd);
        }
    }

    /// The owner's `WM_DESTROY`: no wake reaches the owner any more, every
    /// operation stops so that Continue finishes it at the next start, and
    /// the window goes. The process then ends without dropping the queue.
    pub(super) fn close(&self) -> std::sync::mpsc::Receiver<std::io::Result<()>> {
        self.waker.close();
        let saved = self.queue.close();
        // The closed queue ends no more operations, so each source hears
        // that its operation failed.
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        drop(sources);
        if let Some(hwnd) = self.made().and_then(|window| window.hwnd.take()) {
            hwnd.destroy();
        }
        saved
    }

    /// The native child pane, made only when explicitly opened.
    fn window(&self) -> Option<&Rc<Pane>> {
        let made = self.window.get_or_init(|| {
            let strings = self.strings.borrow().clone();
            match Pane::open(
                self.owner,
                Rc::clone(&self.queue),
                strings,
                Rc::clone(&self.actions),
            ) {
                Ok(window) => Some(window),
                Err(error) => {
                    super::write_log(
                        &self.error_log,
                        &format!("The transfer pane did not open: {error}"),
                    );
                    None
                }
            }
        });
        made.as_ref()
    }

    /// The native child pane, if it was made.
    fn made(&self) -> Option<&Rc<Pane>> {
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

fn show_text(control: Hwnd, text: &str) {
    if control.text() != text {
        control.set_text(text);
    }
}

impl Pane {
    /// Creates hidden transfer content within the owner's inspector.
    fn open(
        owner: Hwnd,
        queue: Rc<ManuallyDrop<Queue>>,
        strings: Strings,
        actions: Rc<RefCell<Vec<PaneAction>>>,
    ) -> windows::core::Result<Rc<Pane>> {
        window::register_class::<Pane>(
            WINDOW_CLASS,
            crate::core::ids::IconId::Gozler.id(),
            window::Background::Dialog,
        )?;
        let window = Rc::new(Pane {
            actions,
            hwnd: Cell::new(None),
            queue,
            owner,
            strings: RefCell::new(strings),
            controls: OnceCell::new(),
            fonts: RefCell::new(None),
            layout: Cell::new(Layout::default()),
            metrics: Cell::new(None),
            view: RefCell::new(None),
            flipped: Cell::new(true),
            maximized: Cell::new(false),
            drag: Cell::new(None),
            refused: Cell::new(None),
            moved: Cell::new(None),
        });
        let hwnd = window::create_child_window(WINDOW_CLASS, Rc::clone(&window), owner)?;
        window.hwnd.set(Some(hwnd));
        window.create_controls(hwnd)?;
        window.apply_fonts(hwnd);
        window.apply_strings();
        Ok(window)
    }

    fn controls(&self) -> &Controls {
        self.controls.get().expect("open creates the controls")
    }

    fn create_controls(&self, hwnd: Hwnd) -> windows::core::Result<()> {
        let id = |control: ControlId| control.id();
        let label = |control| Label::create_line(hwnd, id(control));
        let right = |control| Label::create_right_line(hwnd, id(control));
        let path = |control| PathLabel::create(hwnd, id(control));
        let button = |control| Button::create(hwnd, id(control));
        let bar = |control| ProgressBar::create(hwnd, id(control));
        let pause = button(ControlId::FileOperationsPause)?;
        let cancel = button(ControlId::FileOperationsCancel)?;
        let hide = button(ControlId::FileOperationsHide)?;
        let all = button(ControlId::FileOperationsAll)?;
        let flip = button(ControlId::FileOperationsFlip)?;
        let maximize = button(ControlId::FileOperationsMaximize)?;
        let file = path(ControlId::FileOperationsFile)?;
        let from = path(ControlId::FileOperationsFrom)?;
        let to = path(ControlId::FileOperationsTo)?;
        let tips = Tooltip::create(
            hwnd,
            &[
                pause.hwnd(),
                cancel.hwnd(),
                hide.hwnd(),
                all.hwnd(),
                flip.hwnd(),
                maximize.hwnd(),
                file.hwnd(),
                from.hwnd(),
                to.hwnd(),
            ],
        )?;
        let c = Controls {
            heading: label(ControlId::FileOperationsHeading)?,
            count: label(ControlId::FileOperationsCount)?,
            all,
            flip,
            maximize,
            rate: right(ControlId::FileOperationsRate)?,
            files_rate: right(ControlId::FileOperationsFilesRate)?,
            file_percent: right(ControlId::FileOperationsFilePercent)?,
            file_bytes: right(ControlId::FileOperationsFileBytes)?,
            overall: label(ControlId::FileOperationsOverall)?,
            report: ListView::create(hwnd, id(ControlId::FileOperationsReport))?,
            summary: label(ControlId::FileOperationsSummary)?,
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
            percent: right(ControlId::FileOperationsPercent)?,
            items: label(ControlId::FileOperationsItems)?,
            bytes: right(ControlId::FileOperationsBytes)?,
            queue: label(ControlId::FileOperationsQueue)?,
            list: QueueList::create(hwnd, id(ControlId::FileOperationsList))?,
            hint: label(ControlId::FileOperationsHint)?,
            hide,
            tips,
        };
        let _ = self.controls.set(c);
        self.controls().report.show_info_tips();
        for i in 0..2 {
            self.controls().report.insert_column(
                i,
                &ListColumn {
                    header: "",
                    width: px(160, hwnd),
                    right_aligned: false,
                },
            );
        }
        Ok(())
    }

    /// Creates the fonts for the window's DPI and gives them to the
    /// controls, then the list its icons at that DPI.
    fn apply_fonts(&self, hwnd: Hwnd) {
        self.metrics.set(None);
        let Ok(ui) = Font::message(hwnd.dpi()) else {
            return;
        };
        let title = ui.semibold().unwrap_or_else(|_| {
            Font::message(hwnd.dpi()).expect("the message font was just created")
        });
        let controls = self.controls();
        for control in controls.text_controls() {
            control.set_font(&ui);
        }
        for control in [
            controls.title.hwnd(),
            controls.heading.hwnd(),
            controls.queue.hwnd(),
            controls.doing.hwnd(),
            controls.from_label.hwnd(),
            controls.to_label.hwnd(),
            controls.overall.hwnd(),
            controls.percent.hwnd(),
            controls.file_percent.hwnd(),
            controls.rate.hwnd(),
        ] {
            control.set_font(&title);
        }
        self.apply_glyphs();
        // The controls hold the old fonts until they get the new ones.
        self.fonts.replace(Some(Fonts { ui, title }));
        self.show_tiles(false);
    }

    fn apply_glyphs(&self) {
        let c = self.controls();
        c.list.set_icons(&ROW_GLYPHS, px(ROW_ICON, c.list.hwnd()));
        for (button, glyph) in [
            (c.hide, Glyph::ChevronDown),
            (c.all, Glyph::Cancel),
            (c.flip, Glyph::Switch),
            (
                c.maximize,
                if self.maximized.get() {
                    Glyph::ChromeRestore
                } else {
                    Glyph::ChromeMaximize
                },
            ),
        ] {
            if button.hwnd() == c.all.hwnd() {
                button.set_icon(Some(ButtonIcon::Glyph(glyph)));
            } else {
                button.set_lone_icon(crate::win::gdi::icons_shown().then_some(glyph));
            }
        }
        self.show_state_glyphs();
    }

    fn show_state_glyphs(&self) {
        let c = self.controls();
        let view = self.view.borrow();
        if let Some(card) = view.as_ref().and_then(|v| v.card.as_ref()) {
            c.icon.show_glyph(card.glyph, px(CARD_ICON, c.icon.hwnd()));
            let buttons = card.phase.buttons();
            if let Some(run) = buttons.run {
                c.pause.set_icon(Some(ButtonIcon::Glyph(run.glyph())));
            }
            c.cancel
                .set_icon(Some(ButtonIcon::Glyph(buttons.end.glyph())));
            if view.as_ref().is_some_and(|v| v.report.is_some()) {
                c.pause.set_icon(Some(ButtonIcon::Glyph(Glyph::Cancel)));
                c.cancel.set_icon(Some(ButtonIcon::Glyph(Glyph::EraseTool)));
            }
        }
    }

    fn apply_strings(&self) {
        let strings = self.strings.borrow();
        let c = self.controls();
        for (control, key) in [
            (c.heading.hwnd(), "file_operations.transfers"),
            (c.from_label.hwnd(), "file_operations.from"),
            (c.to_label.hwnd(), "file_operations.to"),
            (c.overall.hwnd(), "file_operations.overall"),
            (c.queue.hwnd(), "file_operations.queue_none"),
            (c.all.hwnd(), "file_operations.cancel_all"),
            (c.hide.hwnd(), "file_operations.hide"),
            (c.flip.hwnd(), "file_operations.flip"),
            (
                c.maximize.hwnd(),
                if self.maximized.get() {
                    "file_operations.restore"
                } else {
                    "file_operations.maximize"
                },
            ),
        ] {
            show_text(control, strings.text(key));
        }
        for button in [c.all, c.hide, c.flip, c.maximize] {
            c.tips.set(button.hwnd(), &button.hwnd().text());
        }
        for (index, key) in [
            "file_operations.column_item",
            "file_operations.column_reason",
        ]
        .into_iter()
        .enumerate()
        {
            c.report.set_header(index, strings.text(key));
        }
        c.tips
            .set(c.hide.hwnd(), strings.text("file_operations.hide_tip"));
        drop(strings);
        self.apply_glyphs();
    }

    fn metrics(&self, hwnd: Hwnd) -> Metrics {
        if let Some(metrics) = self.metrics.get() {
            return metrics;
        }
        let fonts = self.fonts.borrow();
        let strings = self.strings.borrow();
        let dpi = hwnd.dpi();
        let Some(fonts) = fonts.as_ref() else {
            return Metrics {
                dpi,
                icons: crate::win::gdi::icons_shown(),
                line: px(16, hwnd),
                title_line: px(20, hwnd),
                heading_width: px(100, hwnd),
                label_width: px(60, hwnd),
                percent_width: px(36, hwnd),
                hide_width: px(75, hwnd),
            };
        };
        let ui = TextMeter::new(&fonts.ui);
        let title = TextMeter::new(&fonts.title);
        let label_width = [
            "file_operations.file",
            "file_operations.overall",
            "file_operations.from",
            "file_operations.to",
        ]
        .iter()
        .map(|key| title.width(strings.text(key)))
        .max()
        .unwrap_or_default();
        let percent_width = title
            .width(&strings.format("file_operations.percent", &["100"]))
            .max(title.width(strings.text("file_operations.total_unknown")));
        // Room for the icon also while icons are hidden, so Hide keeps its
        // width when they show.
        let hide_caption = QueueAction::ALL
            .iter()
            .map(|action| ui.width(strings.text(action.caption())))
            .chain(
                [
                    "file_operations.hide",
                    "file_operations.maximize",
                    "file_operations.restore",
                    "file_operations.flip",
                    "file_operations.cancel_all",
                ]
                .map(|key| ui.width(strings.text(key))),
            )
            .max()
            .unwrap_or_default();
        let hide_width =
            button_room(button_content(hide_caption, true, dpi), dpi).max(px(75, hwnd));
        let metrics = Metrics {
            dpi,
            icons: crate::win::gdi::icons_shown(),
            line: ui.height(),
            title_line: title.height(),
            heading_width: title.width(strings.text("file_operations.transfers")),
            label_width,
            percent_width,
            hide_width,
        };
        self.metrics.set(Some(metrics));
        metrics
    }

    fn layout(&self, hwnd: Hwnd) {
        let view = self.view.borrow();
        let multiple = view.as_ref().is_some_and(|v| v.rows.len() > 1);
        let report = view.as_ref().is_some_and(|v| v.report.is_some());
        let card = view.as_ref().and_then(|v| v.card.as_ref());
        let queue =
            multiple || (card.is_none() && view.as_ref().is_some_and(|v| !v.rows.is_empty()));
        let mut layout = Layout::compute(
            hwnd.client_size(),
            self.metrics(hwnd),
            multiple,
            self.flipped.get(),
            card.is_some(),
        );
        if report {
            let gap = px(8, hwnd);
            let line = self.metrics(hwnd).line + px(3, hwnd);
            let button_y = layout.summary.y - gap - layout.cancel.height;
            layout.cancel.y = button_y;
            layout.pause.y = button_y;
            layout.report.height = (button_y - gap - layout.report.y).max(line);
        }
        let old_layout = self.layout.replace(layout);
        let c = self.controls();
        if old_layout != layout {
            let moves = [
                (c.heading.hwnd(), layout.heading),
                (c.count.hwnd(), layout.count),
                (c.all.hwnd(), layout.all),
                (c.flip.hwnd(), layout.flip),
                (c.maximize.hwnd(), layout.maximize),
                (c.hide.hwnd(), layout.hide),
                (c.queue.hwnd(), layout.queue),
                (c.list.hwnd(), layout.list),
                (c.hint.hwnd(), layout.hint),
                (c.icon.hwnd(), layout.icon),
                (c.title.hwnd(), layout.title),
                (c.status.hwnd(), layout.status),
                (c.rate.hwnd(), layout.rate),
                (c.files_rate.hwnd(), layout.files_rate),
                (c.doing.hwnd(), layout.doing),
                (c.file.hwnd(), layout.file),
                (c.file_bar.hwnd(), layout.file_bar),
                (c.file_percent.hwnd(), layout.file_percent),
                (c.file_bytes.hwnd(), layout.file_bytes),
                (c.from_label.hwnd(), layout.from_label),
                (c.from.hwnd(), layout.from),
                (c.to_label.hwnd(), layout.to_label),
                (c.to.hwnd(), layout.to),
                (c.overall.hwnd(), layout.overall),
                (c.total_bar.hwnd(), layout.total_bar),
                (c.percent.hwnd(), layout.percent),
                (c.items.hwnd(), layout.items),
                (c.bytes.hwnd(), layout.bytes),
                (c.pause.hwnd(), layout.pause),
                (c.cancel.hwnd(), layout.cancel),
                (c.report.hwnd(), layout.report),
                (c.summary.hwnd(), layout.summary),
            ];
            win::move_children(&moves);
        }
        c.list.hwnd().set_visible(queue);
        c.flip.hwnd().set_visible(multiple && card.is_some());
        c.all.hwnd().set_visible(multiple);
        c.hint.hwnd().set_visible(queue && self.hint_shown());
        c.queue.hwnd().set_visible(queue && !self.hint_shown());
        for control in c.card_labels() {
            control.set_visible(card.is_some() && !report);
        }
        for control in [c.title.hwnd(), c.status.hwnd()] {
            control.set_visible(card.is_some());
        }
        c.icon
            .hwnd()
            .set_visible(card.is_some() && crate::win::gdi::icons_shown());
        let current = card.is_some_and(|card| card.file_bar.is_some()) && !report;
        for control in [
            c.file_bar.hwnd(),
            c.file_percent.hwnd(),
            c.file_bytes.hwnd(),
            c.doing.hwnd(),
            c.file.hwnd(),
        ] {
            control.set_visible(current);
        }
        c.total_bar.hwnd().set_visible(card.is_some() && !report);
        c.report.hwnd().set_visible(report);
        c.summary.hwnd().set_visible(report);
        let buttons = card.map(|card| card.phase.buttons());
        c.pause
            .hwnd()
            .set_visible(report || buttons.is_some_and(|b| b.run.is_some()));
        c.cancel.hwnd().set_visible(report || card.is_some());
        c.cancel
            .hwnd()
            .set_enabled(report || card.is_some_and(|c| c.phase != RowPhase::Cancelling));
        c.files_rate.hwnd().set_visible(
            card.is_some()
                && !report
                && layout.files_rate.width > Px(0)
                && layout.title.width > px(70, hwnd),
        );
        if old_layout.list != layout.list {
            c.list.set_tile_width(layout.list.width - px(21, hwnd));
        }
        let widths = [
            Px(layout.report.width.0 * 60 / 100),
            Px(layout.report.width.0 * 40 / 100) - px(20, hwnd),
        ];
        if widths
            .iter()
            .enumerate()
            .any(|(i, &width)| c.report.column_width(i) != width)
        {
            c.report.hwnd().draw_once_after(|| {
                for (i, &width) in widths.iter().enumerate() {
                    if c.report.column_width(i) != width {
                        c.report.set_column_width(i, width);
                    }
                }
            });
        }
        drop(view);
        if old_layout.list != layout.list {
            self.show_tiles(false);
        }
    }

    fn post(&self, action: PaneAction) {
        self.actions.borrow_mut().push(action);
        self.owner.post(QUEUE_CHANGED);
    }

    fn hide(&self, _hwnd: Hwnd) {
        self.post(PaneAction::Collapse);
    }

    fn selected(&self) -> Option<OperationId> {
        self.view.borrow().as_ref()?.card.as_ref().map(|c| c.id)
    }

    fn select(&self, id: OperationId) {
        if self.selected() != Some(id) {
            self.post(PaneAction::Select(Detail::Progress(id)));
        }
    }

    fn show_view(&self, hwnd: Hwnd, view: View) {
        let c = self.controls();
        let strings = self.strings.borrow();
        let previous = self.view.borrow();
        let old_card = previous.as_ref().and_then(|v| v.card.as_ref());
        let old_report = previous.as_ref().and_then(|v| v.report.as_ref());
        show_text(c.count.hwnd(), &view.queue);
        if let Some(card) = &view.card {
            if view.report.is_none() {
                show_text(c.title.hwnd(), &card.title);
            }
            show_text(c.status.hwnd(), &card.status);
            show_text(c.rate.hwnd(), &card.rate);
            show_text(c.files_rate.hwnd(), &card.files_rate);
            show_text(c.doing.hwnd(), &card.doing);
            show_text(c.file.hwnd(), &card.file);
            if old_card.is_none_or(|old| old.file_tip != card.file_tip) {
                c.tips.set(c.file.hwnd(), &card.file_tip);
            }
            show_text(c.from.hwnd(), &card.from);
            if old_card.is_none_or(|old| old.from != card.from) {
                c.tips.set(c.from.hwnd(), &card.from);
            }
            show_text(c.to.hwnd(), card.to.as_deref().unwrap_or_default());
            if old_card.is_none_or(|old| old.to != card.to) {
                c.tips
                    .set(c.to.hwnd(), card.to.as_deref().unwrap_or_default());
            }
            if let Some(bar) = card.file_bar {
                match bar {
                    Bar::Measured(measure) => {
                        c.file_bar.set_measured(measure.done, measure.total);
                        show_text(
                            c.file_percent.hwnd(),
                            &strings.format(
                                "file_operations.percent",
                                &[&measure.percent().to_string()],
                            ),
                        );
                    }
                    Bar::Moving => {
                        if old_card.is_none_or(|old| {
                            old.file_bar != Some(Bar::Moving) || old.paused != card.paused
                        }) {
                            c.file_bar.set_moving(!card.paused);
                        }
                        show_text(c.file_percent.hwnd(), "");
                    }
                }
                show_text(c.file_bytes.hwnd(), &card.file_bytes);
                show_paused(c.file_bar, card.paused);
            }
            match card.total_bar {
                Bar::Measured(measure) => {
                    c.total_bar.set_measured(measure.done, measure.total);
                    show_text(
                        c.percent.hwnd(),
                        &strings
                            .format("file_operations.percent", &[&measure.percent().to_string()]),
                    );
                }
                Bar::Moving => {
                    if old_card
                        .is_none_or(|old| old.total_bar != Bar::Moving || old.paused != card.paused)
                    {
                        c.total_bar.set_moving(!card.paused);
                    }
                    show_text(
                        c.percent.hwnd(),
                        strings.text("file_operations.total_unknown"),
                    );
                }
            }
            show_paused(c.total_bar, card.paused);
            show_text(c.items.hwnd(), &card.items);
            show_text(c.bytes.hwnd(), &card.bytes);
            if view.report.is_none() {
                let buttons = card.phase.buttons();
                if let Some(run) = buttons.run {
                    show_text(c.pause.hwnd(), strings.text(run.caption()));
                }
                show_text(c.cancel.hwnd(), strings.text(buttons.end.caption()));
            }
        } else {
            show_text(c.title.hwnd(), &view.idle);
        }
        let report_count = view
            .report
            .as_ref()
            .map_or(0, |r| r.outcome.listed.len() as u64);
        let report_changed = view.report.as_ref().is_some_and(|report| {
            old_report.is_none_or(|old| !Arc::ptr_eq(&old.outcome, &report.outcome))
        });
        if let Some(report) = &view.report {
            show_text(c.title.hwnd(), &report.title);
            show_text(c.pause.hwnd(), strings.text("file_operations.close"));
            show_text(c.cancel.hwnd(), strings.text("file_operations.clear"));
            let text =
                |code| crate::win::error_text(&std::io::Error::from_raw_os_error(code as i32));
            show_text(c.summary.hwnd(), &report.summary(&strings, &text));
        }
        let glyphs_changed = old_card.map(|c| (c.glyph, c.phase.buttons()))
            != view.card.as_ref().map(|c| (c.glyph, c.phase.buttons()))
            || old_report.is_some() != view.report.is_some();
        let apply_selection = previous.as_ref().is_none_or(|old| {
            old.card.as_ref().map(|c| c.id) != view.card.as_ref().map(|c| c.id)
                || !old
                    .rows
                    .iter()
                    .map(|row| row.id)
                    .eq(view.rows.iter().map(|row| row.id))
        });
        drop(previous);
        drop(strings);
        let moved = self
            .moved
            .take()
            .and_then(|id| view.rows.iter().position(|r| r.id == id));
        self.view.replace(Some(view));
        if c.report.count() != report_count {
            c.report.set_count(report_count);
        } else if report_changed {
            c.report.redraw_rows(0..report_count);
        }
        if glyphs_changed {
            self.show_state_glyphs();
        }
        self.layout(hwnd);
        self.show_tiles(apply_selection);
        if let Some(index) = moved {
            c.list.ensure_visible(index);
        }
    }

    /// Shows the view's rows in the list, with the selected row selected.
    /// Each title fits its tile's first line, its paths shortened in the
    /// middle, so the second line keeps the row's state; the row's tooltip
    /// gives the whole title.
    fn show_tiles(&self, apply_selection: bool) {
        let c = self.controls();
        let view = self.view.borrow();
        let rows: &[Row] = view.as_ref().map_or(&[], |view| &view.rows);
        let selected = self
            .selected()
            .and_then(|id| rows.iter().position(|row| row.id == id));
        let strings = self.strings.borrow();
        let fonts = self.fonts.borrow();
        let meter = fonts.as_ref().map(|fonts| TextMeter::new(&fonts.ui));
        let show = |width: Option<Px>, apply_selection| {
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
            c.list.set_tiles(&tiles, selected, apply_selection);
        };
        // The list measures a tile once it holds one.
        let width = c.list.text_width().or_else(|| {
            show(None, false);
            c.list.text_width()
        });
        show(width, apply_selection);
    }

    /// The selected row, as the window shows it.
    fn selected_row<T>(&self, use_row: impl FnOnce(&Row, &[Row]) -> T) -> Option<T> {
        let view = self.view.borrow();
        let rows = &view.as_ref()?.rows;
        let id = self.selected()?;
        let row = rows.iter().find(|row| row.id == id)?;
        Some(use_row(row, rows))
    }

    /// Whether the queue lets the selected row move up or down, and the
    /// hint that names why not.
    fn can_step(&self, id: OperationId, up: bool) -> Result<bool, OrderRefused> {
        let view = self.view.borrow();
        let step = view
            .as_ref()
            .and_then(|view| file_operations::move_step(&view.rows, id, up));
        match step {
            Some(before) => self.queue.check_move(id, before).map(|()| true),
            None => Ok(false),
        }
    }

    /// The selected row, and what the queue's header buttons do to it.
    fn selected_buttons(&self) -> Option<(OperationId, RowButtons)> {
        self.selected_row(|row, _| (row.id, row.phase))
            .filter(|(_, phase)| *phase != RowPhase::Cancelling)
            .map(|(id, phase)| (id, phase.buttons()))
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
            self.layout(hwnd);
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

    fn act(&self, id: OperationId, action: QueueAction) {
        self.post(PaneAction::Command(id, action));
    }

    fn on_control(&self, hwnd: Hwnd, event: crate::win::notify::ControlEvent) {
        let c = self.controls();
        if c.hide.clicked(event) {
            self.hide(hwnd);
        } else if c.all.clicked(event) {
            self.post(PaneAction::CancelAll);
        } else if c.maximize.clicked(event) {
            self.post(PaneAction::Maximize);
        } else if c.flip.clicked(event) {
            self.post(PaneAction::Flip);
        } else if let Some((id, buttons)) = self.selected_buttons() {
            let report = self
                .view
                .borrow()
                .as_ref()
                .is_some_and(|v| v.report.is_some());
            if c.pause.clicked(event) {
                if report {
                    self.post(PaneAction::CloseReport);
                } else if let Some(action) = buttons.run {
                    self.act(id, action);
                }
            } else if c.cancel.clicked(event) {
                self.act(
                    id,
                    if report {
                        QueueAction::Clear
                    } else {
                        buttons.end
                    },
                );
            }
        }
    }

    fn hint_shown(&self) -> bool {
        self.refused.get().is_some()
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
        self.select(id);
        let Some(point) = point.or_else(|| list.menu_point(index)) else {
            return;
        };
        let items = file_operations::row_menu(&self.strings.borrow(), phase);
        let states = [
            (
                QueueAction::MoveUp.id(),
                matches!(self.can_step(id, true), Ok(true)),
                false,
            ),
            (
                QueueAction::MoveDown.id(),
                matches!(self.can_step(id, false), Ok(true)),
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
            self.post(PaneAction::Move(drag.id, before));
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

impl WindowProc for Pane {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Size => {
                if self.controls.get().is_some() {
                    self.layout(hwnd);
                }
                Some(LRESULT(0))
            }
            Message::DpiChanged { .. } => {
                self.apply_fonts(hwnd);
                self.layout(hwnd);
                Some(LRESULT(0))
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
            Message::Notify(mut notification) => {
                let c = self.controls.get()?;
                if let Some(tip) = c.report.info_tip(&mut notification) {
                    let row = tip.row()?;
                    let view = self.view.borrow();
                    let report = view.as_ref()?.report.as_ref()?;
                    let strings = self.strings.borrow();
                    let text = |code| {
                        crate::win::error_text(&std::io::Error::from_raw_os_error(code as i32))
                    };
                    return Some(tip.set(&format!(
                        "{}\n{}",
                        report.cell(&strings, row, 0, &text),
                        report.cell(&strings, row, 1, &text)
                    )));
                }
                if notification.sender() == c.report.hwnd() {
                    let notify = c.report.notification(notification)?;
                    if let ListNotify::GetDispInfo(mut info) = notify {
                        let view = self.view.borrow();
                        if let Some(report) = view.as_ref().and_then(|v| v.report.as_ref()) {
                            let text = report.cell(
                                &self.strings.borrow(),
                                info.row() as usize,
                                info.column(),
                                &|code| {
                                    crate::win::error_text(&std::io::Error::from_raw_os_error(
                                        code as i32,
                                    ))
                                },
                            );
                            info.set_text(&text);
                        }
                        return Some(LRESULT(0));
                    }
                    return None;
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
                        if let Some(id) = id {
                            self.select(id);
                        }
                        None
                    }
                    QueueNotify::Activated => {
                        if let Some((id, buttons)) = self.selected_buttons()
                            && buttons.run == Some(QueueAction::Report)
                        {
                            self.act(id, QueueAction::Report);
                        }
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
            _ => None,
        }
    }
}
