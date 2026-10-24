//! Owns the UI's Queue client and the shared inspector's native transfer controls.
//! Engine wakes reach the main window; only that window accepts snapshots and
//! runs lifecycle decisions. The child content has no independent placement,
//! activation or message loop. Reports read immutable outcomes on demand.

use super::work::{CheckPurpose, QueueAccepted, QueueClient};
use crate::core::captions::{ButtonIcon, DialogCaptions, DialogItem};
use crate::core::file_operations::{
    self, Bar, CARD_ICON, Detail, Failure, Layout, Metrics, QueueAction, ROW_GLYPHS, ROW_ICON, Row,
    RowButtons, RowLine, RowPhase, Rows, Taskbar, View, icon_index,
};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::layout::{Px, Rect, button_content, button_room};
use crate::core::operations::{QueueStep, Source, SourceStep};
use crate::core::text::Strings;
use crate::core::work::Retirement;
use crate::win::clipboard::ClipboardFiles;
use crate::win::controls::{Button, GlyphLabel, Label, ListColumn, ListView, ProgressBar, Tooltip};
use crate::win::dialog::{self, DialogMessage, DialogProc, Opened};
use crate::win::drop_target::DropSource;
use crate::win::file_operations::{
    PathLabel, QUEUE_CHANGED, QueueList, QueueNotify, TaskbarProgress, Tile, show_drag_cursor,
    show_paused,
};
use crate::win::gdi::{Font, TextMeter};
use crate::win::menu::Menu;
use crate::win::notify::{Command, ListNotify};
use crate::win::window::{self, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_fileops::{Operation, OperationId, OrderRefused, Request, SpeedSettings, Taken};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
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
    queue: QueueClient,
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
    file_bar_moving: Cell<bool>,
    from_label: Label,
    from: PathLabel,
    to_label: Label,
    to: PathLabel,
    total_bar: ProgressBar,
    total_bar_moving: Cell<bool>,
    percent: Label,
    items: Label,
    bytes: Label,
    queue: Label,
    list: QueueList,
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

struct TileApply {
    view: Arc<View>,
    next: usize,
    revision: u64,
    fit: u64,
    apply_selection: bool,
    again: bool,
}

struct Pane {
    actions: Rc<RefCell<Vec<PaneAction>>>,
    hwnd: Cell<Option<Hwnd>>,
    queue: QueueClient,
    owner: Hwnd,
    strings: RefCell<Strings>,
    controls: OnceCell<Controls>,
    fonts: RefCell<Option<Fonts>>,
    layout: Cell<Layout>,
    metrics: Cell<Option<Metrics>>,
    /// What the window shows now.
    view: RefCell<Option<Arc<View>>>,
    retired: RefCell<Retirement>,
    tiles: RefCell<Option<TileApply>>,
    tile_revision: Cell<u64>,
    tile_fit: Cell<u64>,
    tile_posted: Cell<bool>,
    applying_tiles: Cell<bool>,
    tiles_blocked: Cell<bool>,
    flipped: Cell<bool>,
    maximized: Cell<bool>,
    drag: Cell<Option<Drag>>,
    /// The row that Move up or Move down moved, which the next view keeps
    /// in sight.
    moved: Cell<Option<OperationId>>,
    order_menu: Cell<Option<(OperationId, Menu)>>,
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
    /// Adopts a fully restored queue. Transfer controls stay lazy; `error_log`
    /// gets the reason when they cannot open.
    pub(super) fn new(
        owner: Hwnd,
        queue: QueueClient,
        strings: Strings,
        error_log: PathBuf,
    ) -> FileOperations {
        FileOperations {
            owner,
            queue,
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
    pub(super) fn snapshot(&self) -> Arc<[Operation]> {
        self.queue.snapshot()
    }

    pub(super) fn exit_counts(&self) -> file_operations::ExitCounts {
        self.queue.exit_counts()
    }

    pub(super) fn operation(&self, id: OperationId) -> Option<Operation> {
        self.queue.operation(id)
    }

    pub(super) fn synchronize(&self, id: OperationId) {
        self.queue.synchronize(id);
    }

    pub(super) fn take_synchronized(&self, id: OperationId) -> Option<bool> {
        self.queue.take_synchronized(id)
    }

    pub(super) fn is_pending(&self, id: OperationId) -> bool {
        self.queue.is_pending(id)
    }

    pub(super) fn request_refresh(&self) {
        self.queue.refresh();
    }

    pub(super) fn take_retired(&self) -> Retirement {
        let mut retired = self.queue.take_retired();
        if let Some(window) = self.made() {
            retired.append(std::mem::take(&mut *window.retired.borrow_mut()));
        }
        retired
    }

    pub(super) fn update(&self) -> Vec<QueueAccepted> {
        let accepted = self.queue.update();
        for reply in &accepted {
            if let QueueAccepted::Moved(id, true) = reply
                && let Some(window) = self.made()
            {
                window.moved.set(Some(*id));
            }
        }
        if let Some(window) = self.made()
            && window.drag.get().is_some()
            && let Some(point) = window.hwnd.get().and_then(Hwnd::cursor_point)
        {
            window.drag_to(point);
        }
        if let Some(window) = self.made() {
            window.refresh_order_menu();
        }
        accepted
    }

    pub(super) fn release_comparison(&self, comparison: Arc<gozler_fileops::Comparison>) {
        self.queue.release_comparison(comparison);
    }

    pub(super) fn set_transfer_limit(&self, limit: gozler_fileops::TransferLimit) {
        self.queue.set_transfer_limit(limit);
    }

    /// Prepares `request` with its accepted policy; the main window submits
    /// it when ready. `source` is where the files
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

    pub(super) fn resolve(&self, id: OperationId, request: Request, source: Option<FilesSource>) {
        if let Some(source) = source {
            self.sources.borrow_mut().insert(id, source);
        }
        self.queue.resolve(id, request);
    }

    pub(super) fn resolved(&self, id: OperationId, accepted: bool) -> Option<Source> {
        if !accepted {
            let source = self.sources.borrow_mut().remove(&id);
            if let Some(FilesSource::Drop(source)) = source {
                source.end(false);
            }
        }
        self.sources.borrow().get(&id).map(FilesSource::kind)
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

    pub(super) fn run(&self, step: QueueStep) {
        match step {
            QueueStep::Submit { id } => self.queue.submit(id),
            QueueStep::Remove(id) => self.queue.remove(id),
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
                self.queue.resume(id);
            }
            QueueAction::Dismiss => {
                self.queue.dismiss(id);
            }
            QueueAction::MoveUp | QueueAction::MoveDown => {
                if let Some(window) = self.made() {
                    let before = window.view.borrow().as_ref().and_then(|view| {
                        file_operations::move_step(&view.rows, id, action == QueueAction::MoveUp)
                    });
                    if let Some(before) = before {
                        self.queue.move_waiting(id, before);
                    }
                }
            }
            QueueAction::Report | QueueAction::Clear => {}
        }
    }

    pub(super) fn move_before(&self, id: OperationId, before: Option<OperationId>) {
        self.queue.move_waiting(id, before);
    }

    /// Updates explicitly opened content without activating the main window.
    pub(super) fn show(&self, view: Arc<View>) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(hwnd) = window.hwnd.get() else {
            return;
        };
        window.show_view(hwnd, view);
        hwnd.set_visible(true);
    }

    pub(super) fn prepare_place(
        &self,
        area: Option<Rect>,
        flipped: bool,
        maximized: bool,
        view: Option<Arc<View>>,
        moves: &mut Vec<(Hwnd, Rect)>,
        shown: &mut Vec<(Hwnd, bool)>,
    ) {
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
        if area.is_some()
            && let Some(view) = view
        {
            window.show_view(hwnd, view);
        }
        if let Some(area) = area {
            let size = hwnd.client_size();
            // A size change lays out the children through WM_SIZE; a move
            // alone must still apply a changed flip before showing the pane.
            if size.width == area.width && size.height == area.height {
                window.layout(hwnd);
            }
            moves.push((hwnd, area));
        }
        shown.push((hwnd, area.is_some()));
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
        if window.tiles_blocked.get() {
            return true;
        }
        let Some((id, buttons)) = window.selected_buttons() else {
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
            && buttons.run == Some(QueueAction::Report)
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
    pub(super) fn refresh(&self, view: Arc<View>) {
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
            window.show_tiles(false, true);
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
        let saved = self.queue.close();
        // The closed queue ends no more operations, so each source hears
        // that its operation failed.
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        drop(sources);
        if let Some(window) = self.made() {
            let old = window.view.take();
            window
                .retired
                .borrow_mut()
                .push(old, crate::core::work::INLINE_ITEMS + 1);
            window
                .retired
                .borrow_mut()
                .push(window.tiles.take(), crate::core::work::INLINE_ITEMS + 1);
            window.tile_revision.set(window.tile_revision.get() + 1);
            if let Some(hwnd) = window.hwnd.take() {
                hwnd.destroy();
            }
        }
        saved
    }

    /// The native child pane, made only when explicitly opened.
    fn window(&self) -> Option<&Rc<Pane>> {
        let made = self.window.get_or_init(|| {
            let strings = self.strings.borrow().clone();
            match Pane::open(
                self.owner,
                self.queue.clone(),
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
pub(super) fn show_failure(owner: Hwnd, strings: &Strings, failure: Failure) -> Failure {
    if failure.rows.is_empty() {
        // A refusal, or a full disk before any entry failed, lists
        // nothing, so the message stands alone.
        let text = format!("{}\n\n{}", failure.instruction, failure.body);
        dialog::error_box(Some(owner), &text, &failure.title);
        return failure;
    }
    let mut captions = DialogCaptions::new(DialogId::FileOperationsFailed, strings);
    captions.title.clone_from(&failure.title);
    let dialog = FailureDialog {
        failure,
        list: OnceCell::new(),
        columns: [
            strings.text("file_operations.column_item").to_owned(),
            strings.text("file_operations.column_reason").to_owned(),
        ],
    };
    let _ = dialog::run_modal(owner, DialogId::FileOperationsFailed, &captions, &dialog);
    dialog.failure
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
        queue: QueueClient,
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
            retired: RefCell::default(),
            tiles: RefCell::new(None),
            tile_revision: Cell::new(0),
            tile_fit: Cell::new(0),
            tile_posted: Cell::new(false),
            applying_tiles: Cell::new(false),
            tiles_blocked: Cell::new(false),
            flipped: Cell::new(true),
            maximized: Cell::new(false),
            drag: Cell::new(None),
            moved: Cell::new(None),
            order_menu: Cell::new(None),
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
            file_bar_moving: Cell::new(false),
            from_label: label(ControlId::FileOperationsFromLabel)?,
            from,
            to_label: label(ControlId::FileOperationsToLabel)?,
            to,
            total_bar: bar(ControlId::FileOperationsTotalBar)?,
            total_bar_moving: Cell::new(false),
            percent: right(ControlId::FileOperationsPercent)?,
            items: label(ControlId::FileOperationsItems)?,
            bytes: right(ControlId::FileOperationsBytes)?,
            queue: label(ControlId::FileOperationsQueue)?,
            list: QueueList::create(hwnd, id(ControlId::FileOperationsList))?,
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
        self.show_tiles(false, true);
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
        let view = self.view.borrow().clone();
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
        let moves = [
            (c.heading.hwnd(), layout.heading),
            (c.count.hwnd(), layout.count),
            (c.all.hwnd(), layout.all),
            (c.flip.hwnd(), layout.flip),
            (c.maximize.hwnd(), layout.maximize),
            (c.hide.hwnd(), layout.hide),
            (c.queue.hwnd(), layout.queue),
            (c.list.hwnd(), layout.list),
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
        let mut shown = vec![
            (c.list.hwnd(), queue),
            (c.flip.hwnd(), multiple && card.is_some()),
            (c.all.hwnd(), multiple),
            (c.queue.hwnd(), queue),
            (c.title.hwnd(), card.is_some() || !queue),
            (c.status.hwnd(), card.is_some()),
            (
                c.icon.hwnd(),
                card.is_some() && crate::win::gdi::icons_shown(),
            ),
        ];
        for control in [
            c.from_label.hwnd(),
            c.from.hwnd(),
            c.to_label.hwnd(),
            c.to.hwnd(),
            c.overall.hwnd(),
            c.percent.hwnd(),
            c.items.hwnd(),
            c.bytes.hwnd(),
            c.rate.hwnd(),
        ] {
            shown.push((control, card.is_some() && !report));
        }
        let meter = card.is_some_and(|card| card.file_bar.is_some()) && !report;
        for control in [
            c.file_bar.hwnd(),
            c.file_percent.hwnd(),
            c.file_bytes.hwnd(),
        ] {
            shown.push((control, meter));
        }
        let current = card.is_some_and(|card| !card.file.is_empty()) && !report;
        for control in [c.doing.hwnd(), c.file.hwnd()] {
            shown.push((control, current));
        }
        let buttons = card.map(|card| card.phase.buttons());
        shown.extend([
            (c.total_bar.hwnd(), card.is_some() && !report),
            (c.report.hwnd(), report),
            (c.summary.hwnd(), report),
            (
                c.pause.hwnd(),
                report || buttons.is_some_and(|b| b.run.is_some()),
            ),
            (c.cancel.hwnd(), report || card.is_some()),
            (
                c.files_rate.hwnd(),
                card.is_some()
                    && !report
                    && layout.files_rate.width > Px(0)
                    && layout.title.width > px(70, hwnd),
            ),
        ]);
        c.cancel
            .hwnd()
            .set_enabled(report || card.is_some_and(|c| c.phase != RowPhase::Cancelling));
        win::place_children(if old_layout != layout { &moves } else { &[] }, &shown);
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
            self.show_tiles(false, true);
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

    fn show_view(&self, hwnd: Hwnd, view: Arc<View>) {
        let previous = self.view.borrow().clone();
        if previous.as_ref().is_some_and(|old| Arc::ptr_eq(old, &view)) {
            return;
        }
        let shape = |view: &View| {
            (
                view.card.as_ref().map(|card| card.id),
                view.report.is_some(),
                view.rows.is_empty(),
                view.rows.len() > 1,
            )
        };
        if previous
            .as_ref()
            .is_none_or(|old| shape(old) != shape(&view))
        {
            hwnd.draw_now_after(|| self.apply_view(hwnd, view, previous));
        } else {
            self.apply_view(hwnd, view, previous);
        }
    }

    fn apply_view(&self, hwnd: Hwnd, view: Arc<View>, previous: Option<Arc<View>>) {
        let c = self.controls();
        let strings = self.strings.borrow().clone();
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
                        c.file_bar_moving.set(false);
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
                        let moving = !card.paused;
                        if c.file_bar_moving.replace(moving) != moving {
                            c.file_bar.set_moving(moving);
                        }
                        show_text(c.file_percent.hwnd(), "");
                    }
                }
                show_text(c.file_bytes.hwnd(), &card.file_bytes);
                show_paused(c.file_bar, card.paused);
            }
            match card.total_bar {
                Bar::Measured(measure) => {
                    c.total_bar_moving.set(false);
                    c.total_bar.set_measured(measure.done, measure.total);
                    show_text(
                        c.percent.hwnd(),
                        &strings
                            .format("file_operations.percent", &[&measure.percent().to_string()]),
                    );
                }
                Bar::Moving => {
                    let moving = !card.paused;
                    if c.total_bar_moving.replace(moving) != moving {
                        c.total_bar.set_moving(moving);
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
                || !old.rows.same_order(&view.rows)
        });
        let identity_changed = previous
            .as_ref()
            .is_none_or(|old| !old.rows.same_order(&view.rows));
        drop(previous);
        drop(strings);
        if identity_changed {
            self.tiles_blocked.set(true);
            if self.drag.get().is_some() {
                window::release_mouse();
            }
            self.end_drag();
            c.list.hwnd().set_enabled(false);
        }
        let old = self.view.replace(Some(view));
        self.retired
            .borrow_mut()
            .push(old, crate::core::work::INLINE_ITEMS + 1);
        if let Some(id) = self.selected() {
            let _ = self.can_step(id, true);
            let _ = self.can_step(id, false);
        }
        if c.report.count() != report_count {
            c.report.set_count(report_count);
        } else if report_changed {
            c.report.redraw_rows(0..report_count);
        }
        if glyphs_changed {
            self.show_state_glyphs();
        }
        self.layout(hwnd);
        self.show_tiles(apply_selection, false);
    }

    /// Shows the view's rows in the list, with the selected row selected.
    /// Each title fits its tile's first line, its paths shortened in the
    /// middle, so the second line keeps the row's state; the row's tooltip
    /// gives the whole title.
    fn show_tiles(&self, apply_selection: bool, refit: bool) {
        let Some(hwnd) = self.hwnd.get() else {
            return;
        };
        let Some(view) = self.view.borrow().clone() else {
            return;
        };
        if refit {
            self.tile_fit.set(self.tile_fit.get() + 1);
        }
        let pending = self.tiles.take();
        let next = match pending {
            Some(mut pending)
                if pending.fit == self.tile_fit.get()
                    && pending.view.rows.same_order(&view.rows) =>
            {
                pending.again |= !Arc::ptr_eq(&pending.view, &view);
                pending.apply_selection |= apply_selection;
                pending
            }
            pending => {
                self.retired
                    .borrow_mut()
                    .push(pending, crate::core::work::INLINE_ITEMS + 1);
                let revision = self.tile_revision.get() + 1;
                self.tile_revision.set(revision);
                TileApply {
                    view,
                    next: 0,
                    revision,
                    fit: self.tile_fit.get(),
                    apply_selection,
                    again: false,
                }
            }
        };
        self.tiles.replace(Some(next));
        if !self.tile_posted.replace(true) {
            hwnd.post(window::DEFERRED);
        }
    }

    fn apply_tiles(&self, hwnd: Hwnd) {
        self.tile_posted.set(false);
        if self.applying_tiles.replace(true) {
            return;
        }
        let pending = self.tiles.take();
        if let Some(mut pending) = pending {
            let (revision, fit) = (pending.revision, pending.fit);
            let current = || {
                self.hwnd.get() == Some(hwnd)
                    && self.tile_revision.get() == revision
                    && self.tile_fit.get() == fit
            };
            let list = self.controls().list;
            let strings = self.strings.borrow().clone();
            let mut remaining = crate::core::work::INLINE_ITEMS;
            while remaining > 0 && current() && list.count() > pending.view.rows.len() {
                list.remove_last();
                remaining -= 1;
            }
            while remaining > 0 && current() && pending.next < pending.view.rows.len() {
                let row = &pending.view.rows[pending.next];
                let width = list.text_width();
                let (title, detail) = {
                    let fonts = self.fonts.borrow();
                    let meter = fonts.as_ref().map(|fonts| TextMeter::new(&fonts.ui));
                    let fitted = |line: &RowLine| match (&meter, width) {
                        (Some(meter), Some(width)) => line.fitted(
                            &strings,
                            width,
                            &|text| meter.plain_width(text),
                            &|path, width| meter.compact_path(path, width),
                        ),
                        _ => line.whole(&strings),
                    };
                    (fitted(&row.title), fitted(&row.detail))
                };
                if !list.set_tile(
                    pending.next,
                    &Tile {
                        icon: icon_index(row.glyph),
                        title: &title,
                        detail: &detail,
                    },
                    &current,
                ) {
                    break;
                }
                // The first item supplies the native text width; fit it before advancing.
                if width.is_some() || list.text_width().is_none() {
                    pending.next += 1;
                }
                remaining -= 1;
            }
            if current()
                && list.count() == pending.view.rows.len()
                && pending.next == pending.view.rows.len()
            {
                let latest = self.view.borrow().clone();
                if let Some(latest) = latest.filter(|view| view.rows.same_order(&pending.view.rows))
                {
                    if pending.apply_selection {
                        let selected = latest
                            .card
                            .as_ref()
                            .and_then(|card| latest.rows.position(card.id));
                        list.select(selected, &current);
                    }
                    if current() {
                        self.tiles_blocked.set(false);
                        list.hwnd().set_enabled(true);
                    }
                    if current()
                        && let Some(index) =
                            self.moved.take().and_then(|id| latest.rows.position(id))
                    {
                        list.ensure_visible(index);
                    }
                }
                let again = pending.again;
                self.retired
                    .borrow_mut()
                    .push(pending, crate::core::work::INLINE_ITEMS + 1);
                if again && self.hwnd.get().is_some() {
                    self.show_tiles(false, false);
                }
            } else if current() {
                self.tiles.replace(Some(pending));
                if !self.tile_posted.replace(true) {
                    hwnd.post(window::DEFERRED);
                }
            } else {
                self.retired
                    .borrow_mut()
                    .push(pending, crate::core::work::INLINE_ITEMS + 1);
            }
        }
        self.applying_tiles.set(false);
        if self.hwnd.get() == Some(hwnd) {
            if self.tiles.borrow().is_some() && !self.tile_posted.replace(true) {
                hwnd.post(window::DEFERRED);
            }
            self.owner.post(QUEUE_CHANGED);
        }
    }

    /// The selected row, as the window shows it.
    fn selected_row<T>(&self, use_row: impl FnOnce(&Row, &Rows) -> T) -> Option<T> {
        if self.tiles_blocked.get() || self.applying_tiles.get() {
            return None;
        }
        let index = self.controls().list.selected()?;
        let view = self.view.borrow();
        let rows = &view.as_ref()?.rows;
        let row = rows.get(index)?;
        Some(use_row(row, rows))
    }

    /// Whether the queue still lets the selected row move up or down.
    fn can_step(&self, id: OperationId, up: bool) -> Result<bool, OrderRefused> {
        let view = self.view.borrow();
        let step = view
            .as_ref()
            .and_then(|view| file_operations::move_step(&view.rows, id, up));
        match step {
            Some(before) => self
                .queue
                .check_move(
                    if up {
                        CheckPurpose::Up
                    } else {
                        CheckPurpose::Down
                    },
                    id,
                    before,
                )
                .map_or(Ok(false), |answer| answer.map(|()| true)),
            None => Ok(false),
        }
    }

    /// Commands from the list act on its current highlight, even while the
    /// follower is still preparing that row's card.
    fn selected_buttons(&self) -> Option<(OperationId, RowButtons)> {
        self.selected_row(|row, _| (row.id, row.phase))
            .filter(|(_, phase)| *phase != RowPhase::Cancelling)
            .map(|(id, phase)| (id, phase.buttons()))
    }

    fn card_buttons(&self) -> Option<(OperationId, RowButtons)> {
        let view = self.view.borrow();
        let card = view.as_ref()?.card.as_ref()?;
        (card.phase != RowPhase::Cancelling).then(|| (card.id, card.phase.buttons()))
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
        } else if let Some((id, buttons)) = self.card_buttons() {
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

    fn show_menu(&self, hwnd: Hwnd, point: Option<(i32, i32)>) {
        if self.tiles_blocked.get() {
            return;
        }
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
        let chosen = Menu::track_popup_live(hwnd, &items, point, &states, None, |menu| {
            self.order_menu.set(menu.map(|menu| (id, menu)));
        });
        if let Some(action) = chosen.and_then(QueueAction::from_id) {
            self.act(id, action);
        }
    }

    fn refresh_order_menu(&self) {
        if let Some((id, menu)) = self.order_menu.get() {
            for (action, up) in [(QueueAction::MoveUp, true), (QueueAction::MoveDown, false)] {
                menu.set_state(
                    action.id(),
                    matches!(self.can_step(id, up), Ok(true)),
                    false,
                );
            }
        }
    }

    fn begin_drag(&self, hwnd: Hwnd) {
        if self.tiles_blocked.get() {
            return;
        }
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
            Some((gap, before, false)) => {
                match self.queue.check_move(CheckPurpose::Drag, drag.id, before) {
                    Some(Ok(())) => (Some(gap), None),
                    Some(Err(refused)) => (None, Some(refused)),
                    None => (None, None),
                }
            }
        };
        if gap != drag.gap {
            list.set_insert_mark(gap);
        }
        show_drag_cursor(refused.is_some());
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
    list: OnceCell<ListView>,
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
        if let Some(list) = ListView::of_dialog(dialog, ControlId::FailedList.id()) {
            let width = list.hwnd().client_size().width;
            let path = Px(width.0 * 3 / 5);
            for (index, (header, width)) in
                self.columns.iter().zip([path, width - path]).enumerate()
            {
                list.insert_column(
                    index,
                    &ListColumn {
                        header,
                        width,
                        right_aligned: false,
                    },
                );
            }
            let _ = self.list.set(list);
            list.set_count(self.failure.rows.len() as u64);
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

    fn list_cell(&self, id: u16, row: u64, column: usize) -> Option<String> {
        if id != ControlId::FailedList.id() {
            return None;
        }
        let text = self
            .failure
            .rows
            .get(usize::try_from(row).ok()?)?
            .get(column)?;
        if column == 0
            && let Some(list) = self.list.get()
            && let Ok(font) = Font::of(list.hwnd())
        {
            let room = list.column_width(0)
                - crate::core::layout::column_sizing::cell_width(
                    Px(0),
                    Px(0),
                    true,
                    list.hwnd().dpi(),
                );
            return Some(TextMeter::new(&font).compact_path(text, room));
        }
        Some(text.clone())
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
            Message::Deferred => {
                self.apply_tiles(hwnd);
                Some(LRESULT(0))
            }
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
                let notification = c.list.notification(notification)?;
                if self.tiles_blocked.get() || self.applying_tiles.get() {
                    return Some(LRESULT(0));
                }
                match notification {
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
