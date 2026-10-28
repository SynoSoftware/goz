//! The main window and its message handlers.
//!
//! Window rule 1 (DESIGN.md): a handler changes the model in a statement of its
//! own, or binds a borrow with `let` only in a block that makes no Win32 call,
//! and calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.
//!
//! The update coordinator applies structural effects, refreshes only the
//! touched areas, then delivers ordered actions. Native message reentry
//! supersedes stale presentation; accepted file commands remain owned by
//! their delivery and are consumed once. [`Message::Deferred`] repairs
//! interrupted areas.

use super::dialogs::{
    About, CustomDate, CustomSize, SearchSyntax, Transfer, TransferAction, TransferMode,
    TransferStart,
};
use super::file_operations::{FileOperations, FilesSource};
use super::icons::Icons;
use super::include::{Elevated, NotSaved};
use super::settings::SettingsDialog;
use super::status_bar::{Entry as StatusEntry, Slot as StatusSlot, StatusBar};
use super::viewer::Viewers;
use super::work::{
    Awaited, CopyFailure, CopyNotice, OpenTarget, Opened, PathAction, PathPurpose, PreparedPaths,
    QueueReady, RegistryAnswer, RegistryRead, ReleaseAnswer, ReleasePurpose, ShellPurpose,
    ShellTarget, Surface,
};
use crate::core::captions::{
    self, ButtonIcon, DialogCaptions, DialogItem, FilterCaptions, IndexCaptions, Inspected,
    InspectorCaptions, JobCaption, ListCaption, MODIFIED_ROW, StatusCaptions, StatusWidest,
    WindowCaptions,
};
use crate::core::clipboard::ClipboardFormat;
use crate::core::commands::{
    self, CommandButton, CommandId, Focus, HIDE_DETAILS, INSPECTOR_BUTTONS, INSPECTOR_COPY_FOLDER,
    INSPECTOR_COPY_NAME, INSPECTOR_COPY_PATH, KeyAction, Keystroke, MenuItem, SHOW_DETAILS, State,
    TabStop,
};
use crate::core::dates::{DateFormat, Dates};
use crate::core::drop_target::{Dragged, DropEffect, DropKeys, DropMenu, SourceEffects};
use crate::core::fetch::{
    ChunkAsk, DaemonFailure, Fetch, List, LocalService, Purpose, WriteFailure,
};
use crate::core::file_operations::{QueueAction, delete_question, exit_question};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::include::{
    AfterOk, IncludeDrive, SummaryRow, connected_drives, connected_volumes, include_drives,
    preview, summary_rows,
};
use crate::core::layout::column_sizing::{ColumnFit, ColumnSizing};
use crate::core::layout::{
    self, Body, CaptionWidths, ControlSizes, Dpi, FilterLayout, InspectorLayout, Layout,
    MessageArea, Px, Rect, ScrollRequest, Size, StatusLayout, ToolHeights,
};
use crate::core::model::{
    Accepted, Areas, Column, Effect, ExportError, ExportFormat, Instance, Job, Model, SaveScope,
    ScopeCheck, SearchBoxEnter,
};
use crate::core::open::{Commands, Entries, EntryAction, OpenRoute, Selected};
use crate::core::operations::{
    OperationDialog, OperationProgress, OperationSteps, PreparedFailure, QueueStep, Reporting,
    SourceStep,
};
use crate::core::panel::{self, Side};
use crate::core::query::{
    AttributeChoice, DateModified, Filters, Kind, MatchMode, Matching, RangeItem, SizeFilter,
    date_items, size_items,
};
use crate::core::results::{Fix, ListButton};
use crate::core::search_list::Deselect;
use crate::core::selection::{DRAG_LIMIT, RowSelection, SELECTION_LIMIT, Selection, wide_path};
use crate::core::settings::{Choices, Mode, SettingsPage, SettingsValues};
use crate::core::shortcuts::{CommandShortcuts, ShowShortcut};
use crate::core::text::{LanguageChoice, Numbers, Strings, without_access_key};
use crate::core::work::{Id, Retirement, Waiting};
use crate::win::clipboard;
use crate::win::controls::{
    Button, CheckBox, Divider, DropDownList, Edit, FileIconLabel, IconList, Label, ListClosing,
    ListColumn, ListView, ScrollEdge, Tooltip,
};
use crate::win::dialog::{DialogEnd, DialogProc};
use crate::win::drop_target::{self, DropData, DropHandler, DropRegistration, DropSource};
use crate::win::file_operations::QUEUE_CHANGED;
use crate::win::gdi::{Font, ImageList, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::menu::Accelerators;
use crate::win::notify::{
    Command, ControlEvent, CustomDraw, DispInfo, DrawAnswer, DrawStage, InfoTip, ListNotify,
};
use crate::win::shell::{self, FileType};
use crate::win::shortcut::{self, RegisterError, Registration};
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::tray::TrayEvent;
use crate::win::wake::OpenWaker;
use crate::win::window::{DEFERRED, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_core::include::IndexSettings;
use gozler_core::proto::ResultItem;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortSpec};
use gozler_fileops::{Destination, OperationId, Request, Taken};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::ops::Range;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use windows::Win32::Foundation::LRESULT;
use windows::Win32::UI::WindowsAndMessaging::BROADCAST_QUERY_DENY;

mod drops;
mod filters;
mod inspector;
mod key_bar;
mod panels;
mod search;
mod selection;
mod settings;
mod shortcuts;
mod synchronize;
mod tray;

#[cfg(test)]
mod navigation_tests;

use drops::DragIn;
use filters::{FilterPane, RangeList};
use inspector::Inspector;
use key_bar::{KEY_TIPS, KeyBar};
use panels::{PanelControls, Panels};
use search::{Search, SearchRow};
use settings::SettingsBridge;
pub(in crate::app) use shortcuts::Prepared as PreparedShortcuts;
use shortcuts::Shortcuts;
use tray::Tray;

mod timer;
mod update;

use timer::Timer;

const SEARCH_DELAY_MS: u32 = 150;
/// A search that answers within this time keeps the previous counts until
/// its own arrive, so the status bar changes once (DESIGN.md: Searching).
const SEARCHING_NAMED_MS: u32 = 300;
const STATUS_POLL_MS: u32 = 2_000;
const SELECTION_DELAY_MS: u32 = 150;
const AGE_REFRESH_MS: u32 = 60_000;
/// A row each tenth of a second, as the transfer queue
/// scrolls while a row is dragged past its edge.
const DROP_SCROLL_MS: u32 = 100;
const SAVE_DELAY_MS: u32 = 1_000;
/// Twice a second, as the engine's counters expect to be read.
const OPERATIONS_POLL_MS: u32 = 500;

/// The controls, created in `WM_CREATE` in Tab order; Windows destroys them
/// with the window.
struct Controls {
    search: Search,
    filters: FilterPane,
    inspector: Inspector,
    panels: Panels,
    key_bar: KeyBar,
    status: StatusBar,
    /// As many as [`layout::DIVIDERS`]; those the layout leaves out hide.
    dividers: Vec<Divider>,
}

impl Controls {
    fn create(parent: Hwnd) -> windows::core::Result<Controls> {
        let message_buttons = [
            Button::create(parent, ControlId::MessageButton1.id())?,
            Button::create(parent, ControlId::MessageButton2.id())?,
        ];
        // Show filters is created first, as it stands first.
        let show_filters = icon_tool(parent, ControlId::ShowFilters, Glyph::ClosePane)?;
        let row = SearchRow::create(parent)?;
        let filters = FilterPane::create(parent, show_filters)?;
        let controls = Controls {
            search: Search::create(parent, row, message_buttons)?,
            filters,
            inspector: Inspector::create(parent)?,
            panels: Panels::create(parent)?,
            key_bar: KeyBar::create(parent)?,
            status: StatusBar::for_window(parent)?,
            dividers: (0..layout::DIVIDERS)
                .map(|_| Divider::create(parent))
                .collect::<windows::core::Result<_>>()?,
        };
        controls.search.message.hwnd().set_visible(false);
        // The search box starts empty; its changes enable Clear.
        controls
            .search
            .row
            .tools
            .set_enabled(ControlId::ClearSearch.id(), false);
        for button in controls.search.message_buttons {
            button.hwnd().set_visible(false);
        }
        Ok(controls)
    }

    /// The toolbars, whose tooltips the window gives.
    fn toolbars(&self) -> impl Iterator<Item = Toolbar> {
        let inspector = &self.inspector;
        [
            self.search.row.tools,
            self.filters.show,
            self.filters.tools,
            inspector.hide,
            inspector.copy_name,
            inspector.copy_folder,
        ]
        .into_iter()
        .chain(self.status.show_details())
    }

    /// Gives every control the window's font, at creation and on a monitor
    /// with another DPI.
    fn apply_fonts(&self, fonts: &Fonts) {
        let controls = [
            self.search.row.input.hwnd(),
            self.search.row.tools.hwnd(),
            self.search.row.match_label.hwnd(),
            self.search.row.match_mode.hwnd(),
            self.search.message.hwnd(),
            self.search.message_buttons[0].hwnd(),
            self.search.message_buttons[1].hwnd(),
            self.search.list.hwnd(),
        ];
        let pane = self.filters.controls();
        let inspector = self.inspector.controls();
        let status = self.status.text_controls();
        let panels = self.panels.sides.iter().flat_map(PanelControls::hwnds);
        let key_bar = self.key_bar.buttons.map(Button::hwnd);
        for control in controls
            .into_iter()
            .chain(pane)
            .chain(inspector)
            .chain(status)
            .chain(panels)
            .chain(key_bar)
        {
            control.set_font(&fonts.ui);
        }
        self.inspector.heading.hwnd().set_font(&fonts.heading);
        self.filters.title.hwnd().set_font(&fonts.semibold);
        self.filters.tools.hwnd().set_font(&fonts.ui);
    }

    /// Gives the buttons and the Kind list their icons, and the toolbars'
    /// buttons the `heights` of the controls beside them, at creation, on a
    /// monitor with another DPI, when the system colours change and when
    /// icons are shown or hidden, after the fonts, which reset a toolbar's
    /// buttons. The list message's buttons get theirs with their captions.
    fn apply_glyphs(&self, heights: ToolHeights) {
        let inspector = &self.inspector;
        for toolbar in [
            self.search.row.tools,
            self.filters.show,
            self.filters.tools,
            inspector.hide,
        ] {
            toolbar.show_images(heights.row);
        }
        for copy in [inspector.copy_name, inspector.copy_folder] {
            copy.show_images(heights.copy);
        }
        if let Some(show) = self.status.show_details() {
            show.show_images(heights.status);
        }
        self.filters.kind.set_icons(&Kind::ALL.map(Kind::glyph));
        for (button, control) in inspector.text_buttons() {
            control.set_icon(button.command.glyph().map(ButtonIcon::Glyph));
        }
        for (index, control) in self.key_bar.buttons.iter().enumerate() {
            let command = commands::key_bar_command(index);
            control.set_icon(command.glyph().map(ButtonIcon::Glyph));
        }
        for (control, key) in [
            (self.filters.choose_folder, "filter.choose_folder"),
            (self.panels.sides[0].retry, "panel.retry"),
            (self.panels.sides[1].retry, "panel.retry"),
        ] {
            control.set_icon(ButtonIcon::of(key, false));
        }
        self.status.show_cancel_icon();
    }

    /// What the layout needs that only the controls know, once
    /// [`Self::apply_glyphs`] and the window's texts have set them up.
    fn sizes(&self) -> ControlSizes {
        ControlSizes {
            search_tools: self.search.row.tools.width(),
            filter_tools: self.filters.tools.width(),
            kind_row: self.filters.kind.row_size(),
            location_row: self.filters.location.row_size().height,
        }
    }
}

/// A toolbar of one icon-only button, such as Show filters, which takes
/// the button's ID.
fn icon_tool(parent: Hwnd, id: ControlId, glyph: Glyph) -> windows::core::Result<Toolbar> {
    let id = id.id();
    Toolbar::create(parent, id, &[Tool::Icon { id, glyph }])
}

/// The icon of a Clear captioned with the catalogue text `key`.
fn clear_glyph(key: &str) -> Glyph {
    captions::button_glyph(key).expect("every Clear has an icon")
}

/// The controls' font, its bold form for the matched part of a cell, its
/// semibold form for the filter pane's title, and the inspector's heading
/// font.
struct Fonts {
    ui: Font,
    bold: Font,
    semibold: Font,
    heading: Font,
    struck: Font,
}

impl Fonts {
    fn new(dpi: Dpi) -> windows::core::Result<Fonts> {
        let ui = Font::message(dpi)?;
        let bold = ui.bold()?;
        let semibold = ui.semibold()?;
        let heading = ui.heading()?;
        let struck = ui.struck_out()?;
        Ok(Fonts {
            ui,
            bold,
            semibold,
            heading,
            struck,
        })
    }
}

/// What the layout needs that only measuring the text and the controls
/// gives.
#[derive(Clone, Copy)]
struct Metrics {
    dpi: Dpi,
    font_height: Px,
    captions: CaptionWidths,
}

impl Metrics {
    /// Measures after the controls have their fonts, icons and texts.
    fn measure(fonts: &Fonts, captions: &WindowCaptions, controls: &Controls, dpi: Dpi) -> Metrics {
        let meter = TextMeter::new(&fonts.ui);
        Metrics {
            dpi,
            font_height: meter.height(),
            captions: CaptionWidths::measure(captions, &|text| meter.width(text), controls.sizes()),
        }
    }
}

/// What `WM_CREATE` makes, which every later handler uses: the window
/// procedure calls no other handler before it exists.
struct Created {
    controls: Controls,
    /// Replaced only after every control has the new fonts, because
    /// `WM_SETFONT` does not take ownership.
    fonts: RefCell<Fonts>,
    metrics: Cell<Metrics>,
    /// The client size the status bar last fitted itself to and the height
    /// it took, because each fit erases and draws the whole bar again
    /// (DESIGN.md: Painting). `None` after measuring, which may change its
    /// font or its minimum height.
    status_fit: Cell<Option<(Size, Px)>>,
    /// `None` while the window moves to a monitor with another DPI.
    layout: RefCell<Option<Layout>>,
    work: RefCell<super::work::Work>,
    /// Shared delivery lifetime, closed before this window releases its HWND.
    waker: OpenWaker,
    images: Option<ImageList>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Running,
    ForSetup,
    ForReset,
}

pub(crate) struct MainWindow {
    created: OnceCell<Created>,
    model: RefCell<Model>,
    numbers: RefCell<WindowsNumbers>,
    dates: RefCell<Dates>,
    /// The control that had the focus when the window was deactivated.
    focus_on_activate: Cell<Option<Hwnd>>,
    settings_file: PathBuf,
    settings_failure_shown: Cell<bool>,
    ending_session: Cell<bool>,
    show_message: u32,
    icons: Icons,
    /// Whether the status timer runs.
    polling: Cell<bool>,
    update_revision: Cell<u64>,
    pending_refresh: Cell<Areas>,
    pending_effects: RefCell<VecDeque<Effect>>,
    /// The rows each list's notifications told it selects, in the order of
    /// [`List::ALL`] ([`MainWindow::selected_rows`]).
    told: [RefCell<selection::NativeSelection>; 3],
    selection_timer: Cell<bool>,
    operation_timer: Cell<bool>,
    ending: Cell<Ending>,
    /// The tooltip of each toolbar button, by its ID, which a toolbar asks
    /// for when the pointer rests on the button.
    tool_tips: RefCell<Vec<(u16, String)>>,
    /// While the person drags the splitter: the pointer's client `y` and the
    /// inspector's height when the drag started.
    splitter_drag: Cell<Option<(i32, Px)>>,
    /// The person's command shortcuts, which Settings replaces; the message
    /// loop translates with the table of the moment.
    accelerators: RefCell<Rc<Accelerators>>,
    /// `TaskbarCreated`, after which the tray icon is added again.
    taskbar_created: u32,
    /// `TaskbarButtonCreated`, after which the operations' progress is
    /// shown on the new button.
    taskbar_button_created: u32,
    /// A start of another program asks this window to exit
    /// ([`super::EXIT_MESSAGE`]).
    exit_message: u32,
    pub(super) tray: Tray,
    pub(super) shortcuts: Shortcuts,
    pub(super) settings: SettingsBridge,
    /// A Paste whose thread reads the clipboard.
    pasting: RefCell<HashMap<OperationId, PathBuf>>,
    /// The file operations' queue and the transfer pane, which
    /// only the first window has (DESIGN.md: Interrupted operations).
    file_operations: OnceCell<FileOperations>,
    /// The open Viewers (DESIGN.md: The Viewer).
    viewers: Viewers,
    /// The window's registration as a drop target, which only a window with
    /// file operations has, so another window accepts no drop.
    drop_target: RefCell<Option<DropRegistration>>,
    /// While files are dragged over the window.
    drag_in: RefCell<Option<DragIn>>,
    /// The drops whose paths a thread still reads from their source.
    dropping: RefCell<HashMap<OperationId, drops::Dropping>>,
    /// The modal steps that wait for [`Message::Deferred`], in the order
    /// they came.
    modal_steps: RefCell<VecDeque<ModalStep>>,
    /// Whether a modal step runs, whose loop the later steps wait for.
    modal_running: Cell<bool>,
    operation_delivery: RefCell<Option<OperationDelivery>>,
    delivering_operations: Cell<bool>,
    shutdown: RefCell<Option<win::window::Shutdown>>,
}

struct OperationDelivery {
    queue: std::vec::IntoIter<QueueStep>,
    panels: std::vec::IntoIter<crate::core::panel::PanelFollow>,
    dialogs: std::vec::IntoIter<OperationDialog>,
    progress: Option<OperationProgress>,
    sources: std::vec::IntoIter<SourceStep>,
}

enum OperationEffect {
    Queue(QueueStep),
    Panel(crate::core::panel::PanelFollow),
    Dialog(OperationDialog),
    Progress(OperationProgress),
    Source(SourceStep),
}

impl OperationDelivery {
    fn new(steps: OperationSteps) -> Self {
        Self {
            queue: steps.queue.into_iter(),
            panels: steps.panels.into_iter(),
            dialogs: steps.dialogs.into_iter(),
            progress: Some(steps.progress),
            sources: steps.sources.into_iter(),
        }
    }

    fn next(&mut self) -> Option<OperationEffect> {
        self.queue
            .next()
            .map(OperationEffect::Queue)
            .or_else(|| self.panels.next().map(OperationEffect::Panel))
            .or_else(|| self.dialogs.next().map(OperationEffect::Dialog))
            .or_else(|| self.progress.take().map(OperationEffect::Progress))
            .or_else(|| self.sources.next().map(OperationEffect::Source))
    }

    fn is_empty(&self) -> bool {
        self.queue.len() + self.panels.len() + self.dialogs.len() + self.sources.len() == 0
            && self.progress.is_none()
    }
}

/// A dialog, or another call with its own message loop, that a reply, a
/// panel's disk event or a file operation starts. It runs from
/// [`Message::Deferred`], once everything that the same wake brought has
/// applied, because its loop dispatches the next wakes (DESIGN.md: window
/// rule 13).
enum ModalStep {
    /// Asks whether to download the setup that the fix found.
    ConfirmSetup,
    /// Says that the latest release has no setup to install.
    ExplainNoSetup,
    ExportFailed {
        path: PathBuf,
        error: ExportError,
    },
    SettingsNotSaved(std::io::Error),
    WindowsShortcutFailed(std::io::Error),
    OpenSettings(Id, RegistryRead),
    CopyFailed(Id, CopyFailure),
    Opened(ShellPurpose, crate::core::work::Id, Opened),
    PreparedPaths(PathPurpose, Id, PreparedPaths),
    Paired(crate::core::work::Id, LocalService, bool),
    /// A panel's folder could not be opened.
    PanelRefused(panels::Refused),
    /// Gives `Enter` to the first row of `ask`'s search, which `Enter` in
    /// the search box waited for, while the list still shows it.
    EnterFirst(ChunkAsk),
    /// A file operation's retained failure dialog.
    Operation(OperationDialog),
    PreparedFailure(PreparedFailure),
    Synchronize(OperationId),
}

/// The folder row that dragged files would be dropped into. The drop goes
/// to this entry even when a results page replaces the rows before it
/// lands, because COM's wait loop can deliver one while `Drop` calls the
/// source.
struct DropFolder {
    row: u64,
    folder: ResultItem,
}

impl MainWindow {
    /// Creates the window, starts the first search and the first status
    /// request, and shows the window, or with `sign_in` starts as the `Run`
    /// value asks (DESIGN.md: Window lifetime). The window owns its state; the
    /// message loop reads its accelerators.
    pub(crate) fn open(
        class: &str,
        model: Model,
        settings_file: PathBuf,
        show_message: u32,
        sign_in: bool,
    ) -> (Hwnd, Rc<MainWindow>) {
        let placement = model.placement();
        let accelerators = accelerator_table(&model.choices().shortcuts, model.mode())
            .expect("the accelerator table builds");
        let viewers = Viewers::new(model.viewer());
        let dates = Dates::new(model.choices().date_format, jiff::Zoned::now());
        let instance = model.instance();
        let window = Rc::new(MainWindow {
            created: OnceCell::new(),
            model: RefCell::new(model),
            numbers: RefCell::new(WindowsNumbers::new()),
            dates: RefCell::new(dates),
            focus_on_activate: Cell::new(None),
            settings_file,
            settings_failure_shown: Cell::new(false),
            ending_session: Cell::new(false),
            show_message,
            icons: Icons::default(),
            polling: Cell::new(false),
            update_revision: Cell::new(0),
            pending_refresh: Cell::new(Areas::NONE),
            pending_effects: RefCell::new(VecDeque::new()),
            told: Default::default(),
            selection_timer: Cell::new(false),
            operation_timer: Cell::new(false),
            ending: Cell::new(Ending::Running),
            tool_tips: RefCell::new(Vec::new()),
            splitter_drag: Cell::new(None),
            accelerators: RefCell::new(Rc::new(accelerators)),
            taskbar_created: win::instance::register_message("TaskbarCreated"),
            taskbar_button_created: win::instance::register_message("TaskbarButtonCreated"),
            exit_message: win::instance::register_message(super::EXIT_MESSAGE),
            tray: Tray::new(),
            shortcuts: Shortcuts::new(),
            settings: SettingsBridge::new(),
            pasting: RefCell::new(HashMap::new()),
            shutdown: RefCell::new(None),
            file_operations: OnceCell::new(),
            viewers,
            drop_target: RefCell::new(None),
            drag_in: RefCell::new(None),
            dropping: RefCell::new(HashMap::new()),
            modal_steps: RefCell::new(VecDeque::new()),
            modal_running: Cell::new(false),
            operation_delivery: RefCell::new(None),
            delivering_operations: Cell::new(false),
        });
        let hwnd = win::window::create_window(class, Rc::clone(&window), placement)
            .expect("the main window opens");
        win::present::set_main_window(hwnd);
        // Only a status reply names the volumes, so it is asked for beside
        // the first search (DESIGN.md: Requests).
        window.search(hwnd, List::Search);
        window.ask_status();
        if window.model.borrow().mode() == Mode::Commander {
            let accepted = window.controls().panels.start_panels(&window, hwnd);
            window.apply(hwnd, accepted);
        }
        if instance == Instance::First {
            window.shortcuts.start(&window, hwnd);
            window.check_for_updates();
            window.open_file_operations(hwnd);
            let registration =
                drop_target::register(hwnd, Rc::clone(&window) as Rc<dyn DropHandler>);
            window.drop_target.replace(registration);
        }
        window.tray.update_tray(&window, hwnd);
        let keeps_running = window.model.borrow().choices().keep_running;
        // Showing the window activates it, and WM_ACTIVATE focuses the
        // search box.
        match (sign_in, keeps_running) {
            (true, true) => window.tray.unshown.set(true),
            (true, false) => win::window::show_minimized(hwnd, placement),
            (false, _) => win::window::show(hwnd, placement),
        }
        (hwnd, window)
    }

    /// The accelerator table of the moment, which the message loop holds
    /// until the command it sends returns.
    pub(crate) fn accelerators(&self) -> Rc<Accelerators> {
        Rc::clone(&self.accelerators.borrow())
    }

    fn created(&self) -> &Created {
        self.created
            .get()
            .expect("the window procedure runs a handler only after WM_CREATE made the controls")
    }

    fn controls(&self) -> &Controls {
        &self.created().controls
    }

    fn log_error(&self, text: String) {
        self.created()
            .work
            .borrow_mut()
            .log_error(self.settings_file.with_file_name("ui-error.log"), text);
    }

    fn instance(&self) -> Instance {
        self.model.borrow().instance()
    }

    fn on_create(&self, hwnd: Hwnd) -> bool {
        let dpi = hwnd.dpi();
        let (Ok(controls), Ok(fonts)) = (Controls::create(hwnd), Fonts::new(dpi)) else {
            return false;
        };
        controls.apply_fonts(&fonts);
        win::gdi::show_icons(self.model.borrow().choices().show_icons);
        controls.apply_glyphs(ToolHeights::new(dpi, TextMeter::new(&fonts.ui).height()));
        let (columns, order) = {
            let model = self.model.borrow();
            (model.columns().to_vec(), model.column_order())
        };
        // The fill when the window shows gives the widths.
        for (index, column) in columns.into_iter().enumerate() {
            controls.search.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: Px(0),
                    right_aligned: column.row().right_aligned,
                },
            );
        }
        controls.search.list.set_column_order(&order);
        let images = ImageList::system_small();
        if let Some(images) = &images {
            controls.search.list.set_small_images(images);
            for panel in &controls.panels.sides {
                panel.list.set_small_images(images);
            }
        }
        let captions = self.captions();
        // Measured again once the controls have their texts.
        let metrics = Metrics::measure(&fonts, &captions, &controls, dpi);
        let waker = OpenWaker::new(hwnd);
        let _ = self.created.set(Created {
            controls,
            fonts: RefCell::new(fonts),
            metrics: Cell::new(metrics),
            status_fit: Cell::new(None),
            layout: RefCell::new(None),
            work: RefCell::new(super::work::Work::new(
                waker.clone(),
                self.settings_file.with_file_name("ui-error.log"),
            )),
            waker,
            images,
        });
        self.apply_text(hwnd, &captions);
        self.measure(&captions, dpi);
        self.layout(hwnd);
        true
    }

    /// Gives the controls and the memory figures their icons at `dpi`, in
    /// the system colours and with Show icons of now, after the fonts.
    fn apply_glyphs(&self, dpi: Dpi) {
        let created = self.created();
        let font_height = TextMeter::new(&created.fonts.borrow().ui).height();
        created
            .controls
            .apply_glyphs(ToolHeights::new(dpi, font_height));
        if let Some(operations) = self.file_operations.get() {
            operations.apply_glyphs();
        }
        self.update_status();
    }

    /// Measures the texts and the controls again at `dpi`, after their
    /// texts, their fonts or their icons changed.
    fn measure(&self, captions: &WindowCaptions, dpi: Dpi) {
        let created = self.created();
        let metrics = Metrics::measure(&created.fonts.borrow(), captions, &created.controls, dpi);
        created.metrics.set(metrics);
        created.status_fit.set(None);
    }

    /// Every caption, from the model's facts and the current strings
    /// (DESIGN.md: window rule 12).
    fn captions(&self) -> WindowCaptions {
        WindowCaptions::new(&self.model.borrow(), &*self.numbers.borrow())
    }

    /// Sets every caption, then measures them and lays out again. Runs
    /// after a change to the language or to the regional format.
    fn refresh_text(&self, hwnd: Hwnd) {
        let captions = self.captions();
        self.apply_text(hwnd, &captions);
        self.measure(&captions, hwnd.dpi());
        self.layout(hwnd);
    }

    /// Loads the language of `choice`, gives it to the model and sets every
    /// caption in it (DESIGN.md: Language). The Language menu
    /// and Settings' OK and Apply call it; the typed text, the results, the
    /// selection and the focus stay as they are.
    fn set_language(&self, hwnd: Hwnd, choice: LanguageChoice) {
        let language = choice.resolve(&win::locale::display_languages());
        let accepted = (self.model.borrow_mut()).set_language(choice, Strings::load(language));
        super::set_panic_report(|report| report.language = Some(language));
        self.apply(hwnd, accepted);
        self.refresh_text(hwnd);
        if let Some(operations) = self.file_operations.get() {
            operations.set_strings(Strings::load(language));
            self.refresh_operations();
        }
        self.viewers.set_strings(&Strings::load(language));
    }

    fn apply_text(&self, hwnd: Hwnd, captions: &WindowCaptions) {
        {
            let mut dates = self.dates.borrow_mut();
            dates.format = self.model.borrow().choices().date_format;
            dates.set_time(jiff::Zoned::now());
        }
        self.follow_dates(hwnd);
        let controls = self.controls();
        hwnd.set_text(&captions.title);
        let _ = win::menu::replace_menu_bar(hwnd, &captions.menu_bar);
        controls.search.set_captions(self, captions);
        controls.filters.set_captions(&captions.filters);
        controls.filters.show_filters(self, &captions.filters);
        controls.inspector.set_captions(self, captions);
        controls.status.set_cancel_caption(&captions.status_cancel);
        controls
            .status
            .set_progress_captions(&captions.status_progress);
        controls
            .status
            .set_show_details_caption(&captions.show_details.caption);
        let filters = &captions.filters;
        self.tool_tips.replace(vec![
            (
                ControlId::ClearSearch.id(),
                without_access_key(&captions.clear_search),
            ),
            (ControlId::ShowFilters.id(), filters.show.clone()),
            (ControlId::ClearFilters.id(), filters.clear_tip.clone()),
            (ControlId::HideFilters.id(), filters.hide.clone()),
            (
                INSPECTOR_COPY_NAME.control.id(),
                captions.copy_name.tip.clone(),
            ),
            (
                INSPECTOR_COPY_FOLDER.control.id(),
                captions.copy_folder.tip.clone(),
            ),
            (HIDE_DETAILS.control.id(), captions.hide_details.tip.clone()),
            (
                ControlId::StatusShowDetails.id(),
                captions.show_details.tip.clone(),
            ),
        ]);
        self.show_status(&captions.status);
        self.controls()
            .search
            .show_message(self, captions.message.as_ref());
        self.controls().panels.apply_panel_text(self, captions);
        self.update_bound_controls();
    }

    fn update_bound_controls(&self) {
        let controls = self.controls();
        controls.filters.refresh_commands(self);
        controls.inspector.refresh_commands(self);
        controls.search.refresh_commands(self);
    }

    fn update_status(&self) {
        let status = StatusCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        self.show_status(&status);
    }

    fn show_status(&self, status: &StatusCaptions) {
        let controls = self.controls();
        let mut job = status.job.clone();
        let settings_fix = self.settings.open.get().is_some_and(|open| open.follow_fix)
            && self.model.borrow().job() == Job::Fix;
        if settings_fix {
            job = JobCaption {
                transfer: job.transfer,
                transfer_only: true,
                ..JobCaption::default()
            };
        } else {
            let now = Instant::now();
            let model = self.model.borrow();
            match model.job() {
                Job::Fix if self.fix_waiting(now) == Some(Waiting::Slow) => {
                    job.text = crate::core::work::waiting(model.strings(), None);
                }
                Job::Export
                    if self.request_waiting(Purpose::Export, now) == Some(Waiting::Slow) =>
                {
                    if let crate::core::model::Export::Running { path, .. } = model.export() {
                        job.text = crate::core::work::waiting(model.strings(), path.to_str());
                    }
                }
                _ => {}
            }
            if let Some(awaited) = self.main_waiting() {
                job.text = crate::core::work::waiting(model.strings(), None);
                job.progress = captions::JobProgress::Unmeasured;
                job.cancel = awaited.can_cancel();
                job.transfer_only = false;
            }
        }
        let cancel = self
            .model
            .borrow()
            .strings()
            .text("button.cancel")
            .to_owned();
        controls.status.set_cancel_caption(&cancel);
        let layout = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .map(|layout| layout.status);
        // Cancel hides when the job ends, and a hidden control would keep
        // the focus but take no key.
        let mut entries = vec![
            (StatusSlot::Left, StatusEntry::Text(&status.view)),
            (
                StatusSlot::Right,
                StatusEntry::Values {
                    parts: &status.right,
                    memory: status.memory.as_ref(),
                },
            ),
        ];
        if !job.transfer_only {
            entries.push((StatusSlot::Center, StatusEntry::Work(&job)));
        }
        entries.extend(
            status
                .indexing
                .iter()
                .map(|caption| (StatusSlot::Center, StatusEntry::Indexing(caption))),
        );
        if let Some(transfer) = &job.transfer {
            entries.push((StatusSlot::Center, StatusEntry::Transfer(transfer)));
        }
        if controls.status.render(&entries, layout.as_ref()) {
            self.home().focus();
        }
    }

    fn work_surface(&self) -> Surface {
        if self.settings.open.get().is_some() {
            Surface::Settings
        } else {
            Surface::Main
        }
    }

    fn slow_global(&self, surface: Surface) -> Option<Awaited> {
        self.created()
            .work
            .borrow()
            .global_waiting(surface, Instant::now())
            .and_then(|(awaited, waiting)| (waiting == Waiting::Slow).then_some(awaited))
    }

    fn main_waiting(&self) -> Option<Awaited> {
        {
            let model = self.model.borrow();
            if model.can_cancel_job()
                || model.fix().running()
                || matches!(
                    model.operation(),
                    Some(crate::core::file_operations::StatusOperation::Pending {
                        phase: crate::core::file_operations::Pending::Cancelling,
                        ..
                    })
                )
            {
                return None;
            }
        }
        self.slow_global(Surface::Main)
    }

    pub(super) fn request_waiting(&self, purpose: Purpose, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().requests.waiting(purpose, now)
    }

    pub(super) fn sign_in_waiting(&self, id: Id, now: Instant) -> Option<Waiting> {
        self.created()
            .work
            .borrow()
            .settings
            .sign_in_waiting(id, now)
    }

    pub(super) fn elevated_waiting(&self, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().elevated.waiting(now)
    }

    pub(super) fn fix_waiting(&self, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().fix_waiting(now)
    }

    pub(super) fn cancel_settings_request(&self, hwnd: Hwnd, purpose: Purpose) {
        let accepted = match purpose {
            Purpose::Program => {
                self.settings.total_commander.take();
                self.model.borrow_mut().forget_total_commander()
            }
            Purpose::IncludeCount => self.model.borrow_mut().forget_included_count(),
            _ => return,
        };
        self.apply(hwnd, accepted);
        self.settings.post_settings_reply();
    }

    pub(super) fn cancel_settings_fix(&self, hwnd: Hwnd) {
        self.created().work.borrow_mut().settings.cancel_pairing();
        let accepted = self.model.borrow_mut().cancel_fix();
        self.apply(hwnd, accepted);
    }

    /// The main window's status bar's Cancel: stops the job
    /// that the bar shows, as File > Cancel does.
    pub(super) fn cancel_job(&self, hwnd: Hwnd) {
        if self.cancel_main_waiting(hwnd) {
            return;
        }
        self.run(hwnd, MenuItem::Run(CommandId::Cancel));
    }

    fn cancel_main_waiting(&self, hwnd: Hwnd) -> bool {
        let Some(awaited) = self.main_waiting() else {
            return false;
        };
        if awaited.can_cancel() {
            self.created().work.borrow_mut().cancel_awaited(awaited);
            self.poll_work(hwnd);
        }
        true
    }

    /// Runs a failure's fix (DESIGN.md: Service fixes). The list's button
    /// and the Index page's call it, with the main window as `hwnd`, which
    /// owns Windows' permission prompt and outlives the fix's thread.
    pub(super) fn run_fix(&self, hwnd: Hwnd, fix: Fix) {
        self.created().work.borrow_mut().settings.cancel_pairing();
        match &fix {
            Fix::OpenServices => {
                self.created()
                    .work
                    .borrow_mut()
                    .shell
                    .open(ShellTarget::Services, self.work_surface());
                self.settings.post_settings_reply();
                return;
            }
            Fix::UseLocal(local) => {
                let question = captions::use_local_question(self.model.borrow().strings(), local);
                if !win::dialog::ask(self.fix_owner(hwnd), &question) {
                    return;
                }
                self.created()
                    .work
                    .borrow_mut()
                    .settings
                    .check_pairing(local.clone(), self.work_surface());
                self.settings.post_settings_reply();
                return;
            }
            Fix::Repair(_) | Fix::Update => {}
        }
        self.start_fix(hwnd, &fix);
    }

    fn start_fix(&self, hwnd: Hwnd, fix: &Fix) {
        if let Some(mut open) = self.settings.open.get() {
            open.follow_fix = true;
            self.settings.open.set(Some(open));
        }
        let accepted = self.model.borrow_mut().start_fix(fix);
        self.apply(hwnd, accepted);
    }

    /// Runs the fix that `shown` gives, the one a button of the Index
    /// page's Service group shows, such as [`Model::service_fix`].
    pub(super) fn run_shown_fix(&self, hwnd: Hwnd, shown: fn(&Model) -> Option<Fix>) {
        let fix = shown(&self.model.borrow());
        if let Some(fix) = fix {
            self.run_fix(hwnd, fix);
        }
    }

    /// The window that owns a fix's question or message: the Settings
    /// dialog while it is open, because the main window under it takes no
    /// input, else the main window.
    fn fix_owner(&self, hwnd: Hwnd) -> Hwnd {
        self.settings.open.get().map_or(hwnd, |open| open.dialog)
    }

    /// Asks the person whether to download the setup the fix found, in a
    /// task dialog, and goes on as they answer (DESIGN.md: Service fixes).
    fn confirm_setup(&self, hwnd: Hwnd) {
        let question = captions::setup_question(&self.model.borrow(), &*self.numbers.borrow());
        let Some(question) = question else {
            return;
        };
        let download = win::dialog::ask(self.fix_owner(hwnd), &question);
        let accepted = self.model.borrow_mut().answer_setup(download);
        self.apply(hwnd, accepted);
    }

    /// Says why the fix found no setup to install, so it downloads nothing,
    /// unless another fix started since.
    fn explain_no_setup(&self, hwnd: Hwnd) {
        let text = {
            let model = self.model.borrow();
            (model.fix().found_no_setup())
                .then(|| model.strings().text("fixing.no_setup").to_owned())
        };
        if let Some(text) = text {
            self.error_box(self.fix_owner(hwnd), &text);
        }
    }

    fn run_setup(&self, setup: PathBuf) {
        self.created()
            .work
            .borrow_mut()
            .shell
            .start_setup(setup, self.work_surface());
        self.settings.post_settings_reply();
    }

    /// Closes the window after its message loop has ended, when the setup
    /// asked for it in [`MainWindow::run_setup`].
    pub(super) fn close_for_setup(&self, hwnd: Hwnd) {
        if self.ending.get() == Ending::ForSetup {
            self.exit(hwnd);
        }
    }

    /// Setup can end the message loop before destroying the window, so its
    /// caller also holds the process open for its essential writers.
    pub(super) fn finish_shutdown(&self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.finish();
        }
    }

    /// The service's report, which the Index page's Show report shows.
    pub(super) fn service_report(&self) -> Option<String> {
        captions::service_report(&self.model.borrow())
    }

    fn update_message(&self) {
        let message = self.list_message();
        self.controls().search.show_message(self, message.as_ref());
    }

    fn list_message(&self) -> Option<ListCaption> {
        let slow = self.slow_list_work(List::Search);
        if slow.is_some() && self.model.borrow().mode() == Mode::Search {
            let model = self.model.borrow();
            let path = match slow {
                Some(Awaited::Request(_) | Awaited::Selection(_)) => {
                    model.filters().location.as_deref()
                }
                _ => None,
            };
            let text = crate::core::work::waiting(model.strings(), path);
            Some(ListCaption {
                text,
                buttons: vec![captions::MessageButton {
                    label: model.strings().text("button.cancel").to_owned(),
                    icon: ButtonIcon::of("button.cancel", false),
                    enabled: true,
                    tooltip: None,
                }],
            })
        } else {
            captions::list_message(&self.model.borrow())
        }
    }

    pub(super) fn slow_list_work(&self, list: List) -> Option<Awaited> {
        let selecting = {
            let model = self.model.borrow();
            model.list(list).is_some_and(|list| {
                matches!(
                    list.selection(),
                    Selection::Reading { .. } | Selection::Filtering { .. }
                )
            })
        };
        let selection = selecting
            || self.selection_command_pending(list)
            || self.native_selection_waiting(list);
        let work = self.created().work.borrow();
        let now = Instant::now();
        if let List::Panel(side) = list
            && self
                .model
                .borrow()
                .panels()
                .panel(side)
                .reading_generation()
                .is_some()
            && work.panels.waiting(side, now) == Some(Waiting::Slow)
        {
            return Some(Awaited::Listing(side));
        }
        work.slow(list, selection, now)
    }

    pub(super) fn cancel_list_work(&self, hwnd: Hwnd, list: List, awaited: Awaited) {
        if self.slow_list_work(list) != Some(awaited) {
            return;
        }
        if let Awaited::Listing(side) = awaited {
            let generation = self
                .model
                .borrow()
                .panels()
                .panel(side)
                .reading_generation();
            if let Some(generation) = generation {
                self.controls()
                    .panels
                    .cancel_read(self, hwnd, side, generation);
            }
            self.poll_work(hwnd);
            return;
        }
        if !matches!(awaited, Awaited::Request(_)) {
            self.created().work.borrow_mut().cancel_awaited(awaited);
        }
        let selection = matches!(
            awaited,
            Awaited::Request(Purpose::Selection(_)) | Awaited::Selection(_)
        );
        let accepted = match awaited {
            Awaited::Request(Purpose::Selection(_)) | Awaited::Selection(_) => {
                let empty: RowSelection = Vec::new().into();
                self.forget_selection_action(list);
                self.model.borrow_mut().set_selection(list, empty, None)
            }
            Awaited::Request(Purpose::Results(_)) => self.model.borrow_mut().cancel_results(list),
            _ => Accepted {
                touched: Areas::NONE,
                effects: Vec::new(),
            },
        };
        self.apply(hwnd, accepted);
        if selection {
            self.list_control(list).select_only(None);
            self.seed_native_selection(list, Vec::new().into());
            if let List::Panel(side) = list {
                self.panel(side).changed.set(panels::ListChanged::Selection);
                self.controls().panels.read_selection(self, side, true);
            }
        }
        if awaited == Awaited::Clipboard {
            hwnd.kill_timer(Timer::Clipboard.id());
        }
        self.update_message();
        if let List::Panel(side) = list {
            self.controls().panels.show_panel_message(self, side);
        }
        self.poll_work(hwnd);
    }

    /// Returns the places for a list's `message` below its `header` and
    /// `buttons` below the text, centred for a panel's message.
    fn message_moves(
        &self,
        list: Rect,
        header: Px,
        message: Hwnd,
        text: &str,
        buttons: &[Hwnd],
        centred: bool,
    ) -> Vec<(Hwnd, Rect)> {
        let created = self.created();
        let metrics = created.metrics.get();
        let area = MessageArea::new(list, header, metrics.dpi);
        let (text, rects) = {
            let fonts = created.fonts.borrow();
            let meter = TextMeter::new(&fonts.ui);
            area.place(
                meter.wrapped_height(text, area.text_width()),
                metrics.font_height,
                buttons.len(),
                metrics.captions.message_button_width(metrics.dpi),
                centred,
            )
        };
        let mut moves = vec![(message, text)];
        moves.extend(buttons.iter().copied().zip(rects));
        moves
    }

    /// Places every control where `Layout::compute` says (DESIGN.md:
    /// window rule 7), and shows only the controls of the shown mode.
    fn layout(&self, hwnd: Hwnd) {
        let created = self.created();
        let controls = &created.controls;
        let metrics = created.metrics.get();
        let client = hwnd.client_size();
        let status_height = match created.status_fit.get() {
            Some((fitted, height)) if fitted == client => height,
            _ => {
                let height = (controls.status)
                    .fit(StatusLayout::min_height(metrics.dpi, metrics.font_height));
                created.status_fit.set(Some((client, height)));
                height
            }
        };
        let (mode, panes) = {
            let model = self.model.borrow();
            (model.mode(), model.panes())
        };
        let mut captions = metrics.captions;
        if panes.inspector && panes.content == crate::core::layout::InspectorContent::Transfers {
            captions.transfers_height = self
                .file_operations
                .get()
                .and_then(FileOperations::min_height)
                .unwrap_or_default();
        }
        let layout = Layout::compute(
            client,
            metrics.dpi,
            metrics.font_height,
            status_height,
            &captions,
            mode,
            &panes,
        );
        let minimum = layout.min_client;
        let grow = (client.width < minimum.width || client.height < minimum.height)
            && created.layout.borrow().as_ref().is_none_or(|previous| {
                minimum.width > previous.min_client.width
                    || minimum.height > previous.min_client.height
            });
        let search = layout.search();
        let filters = search.and_then(|search| search.filters.as_ref());
        let show_filters = search.and_then(|search| search.show_filters);
        let transfers = panes.content == crate::core::layout::InspectorContent::Transfers;
        let maximized = panes.inspector && transfers && panes.maximized;
        let placed_inspector = layout.inspector();
        let selection_inspector = placed_inspector.filter(|_| !transfers);
        let strip = [
            controls.search.row.input.hwnd(),
            controls.search.row.tools.hwnd(),
            controls.search.row.match_label.hwnd(),
            controls.search.row.match_mode.hwnd(),
            controls.search.list.hwnd(),
        ];
        let mut moves = Vec::new();
        let mut shown = Vec::new();
        match &layout.body {
            Body::Search(placed) => moves.extend(strip.into_iter().zip([
                placed.search,
                placed.search_tools,
                placed.match_label,
                // A drop-down list's height includes its open list: ten
                // times the box's height fits Match's five modes.
                Rect {
                    height: Px(placed.match_mode.height.0 * 10),
                    ..placed.match_mode
                },
                placed.list,
            ])),
            Body::Commander { key_bar, .. } => {
                moves.extend(
                    controls
                        .key_bar
                        .buttons
                        .map(Button::hwnd)
                        .into_iter()
                        .zip(*key_bar),
                );
                for (area, index) in KEY_TIPS.into_iter().enumerate() {
                    controls.key_bar.tips.place_area(area, key_bar[index]);
                }
            }
        }
        let pane = &controls.filters;
        if let Some(filters) = filters {
            moves.extend([
                (pane.title.hwnd(), filters.title),
                (pane.tools.hwnd(), filters.tools),
                (pane.pane, filters.pane),
            ]);
        }
        if let Some(show) = show_filters {
            moves.push((pane.show.hwnd(), show));
        }
        let inspector = &controls.inspector;
        let inspector_moved = created
            .layout
            .borrow()
            .as_ref()
            .is_none_or(|previous| previous.inspector() != placed_inspector);
        // The inspector's texts are fitted again only when it moved or its
        // size changed, because fitting them measures each text. Their
        // controls move in this batch with the others (DESIGN.md: Painting).
        let inspector_fit = selection_inspector
            .filter(|_| inspector_moved)
            .map(|placed| self.controls().inspector.inspector_fit(self, placed));
        if let Some(placed) = selection_inspector {
            moves.push((inspector.icon.hwnd(), placed.icon));
            moves.push((inspector.hide.hwnd(), placed.hide));
            moves.push((inspector.copy_path.hwnd(), placed.copy_path));
            moves.extend(
                inspector
                    .buttons
                    .iter()
                    .map(|b| b.hwnd())
                    .zip(placed.buttons),
            );
            for ((label, _), (label_rect, _)) in inspector.fields.iter().zip(&placed.fields) {
                moves.push((label.hwnd(), *label_rect));
            }
            if let Some(note) = placed.note {
                moves.push((inspector.note.hwnd(), note));
            }
        }
        moves.extend(
            controls
                .dividers
                .iter()
                .map(|divider| divider.hwnd())
                .zip(layout.dividers.iter().copied()),
        );
        for control in strip {
            shown.push((control, search.is_some() && !maximized));
        }
        for side in Side::BOTH {
            let placed = match &layout.body {
                Body::Commander { panels, .. } => Some(&panels[side.index()]),
                Body::Search(_) => None,
            };
            controls
                .panels
                .prepare_panel(self, side, placed, &mut moves, &mut shown);
        }
        for button in controls.key_bar.buttons {
            shown.push((button.hwnd(), search.is_none() && !maximized));
        }
        let visible = filters.is_some() && !maximized;
        for control in [pane.title.hwnd(), pane.tools.hwnd(), pane.pane] {
            shown.push((control, visible));
        }
        shown.push((pane.show.hwnd(), show_filters.is_some() && !maximized));
        let visible = selection_inspector.is_some();
        for control in [
            inspector.icon.hwnd(),
            inspector.heading.hwnd(),
            inspector.hide.hwnd(),
            inspector.copy_path.hwnd(),
        ] {
            shown.push((control, visible));
        }
        shown.extend(inspector.buttons.map(|button| (button.hwnd(), visible)));
        inspector.prepare_fields(
            self,
            selection_inspector,
            inspector_fit,
            &mut moves,
            &mut shown,
        );
        for (index, divider) in controls.dividers.iter().enumerate() {
            shown.push((divider.hwnd(), index < layout.dividers.len() && !maximized));
        }
        let message = self.list_message();
        let message_focus_lost = controls.search.prepare_message(
            self,
            message.as_ref(),
            search.map(|placed| placed.list),
            &mut moves,
            &mut shown,
        );
        if let Some(operations) = self.file_operations.get() {
            let view = self.model.borrow().operation_view();
            operations.prepare_place(
                placed_inspector.filter(|_| transfers).map(|i| i.area),
                panes.flipped,
                maximized,
                view,
                &mut moves,
                &mut shown,
            );
        }
        let search_width = search.map(|placed| {
            (
                placed.list.width,
                placed.list.width < controls.search.list.hwnd().screen_rect().width,
            )
        });
        let panel_widths = match &layout.body {
            Body::Commander { panels, .. } => std::array::from_fn(|index| {
                let list = controls.panels.sides[index].list;
                let old_width = list.hwnd().screen_rect().width;
                (
                    list.fill_width() + panels[index].list.width - old_width,
                    panels[index].list.width < old_width,
                )
            }),
            Body::Search(_) => [(Px(0), false); 2],
        };
        let fill_columns = |before_move| {
            if let Some((width, narrows)) = search_width {
                if narrows == before_move {
                    controls.search.fill_on_resize(self, width);
                }
            } else {
                let widths = std::array::from_fn(|index| {
                    let (planned, narrows) = panel_widths[index];
                    if before_move {
                        narrows.then_some(planned)
                    } else {
                        let actual = controls.panels.sides[index].list.fill_width();
                        (!narrows || actual != planned).then_some(actual)
                    }
                });
                // Border drags keep the existing fits of the other columns.
                if controls.search.column_sizing.borrow().is_dragging() {
                    for (side, width) in Side::BOTH.into_iter().zip(widths) {
                        if let Some(width) = width {
                            controls.panels.fill_name_column_at(side, width);
                        }
                    }
                } else {
                    controls.panels.fill_panel_columns_at(self, widths);
                }
            }
        };
        // Fit shrinking columns before the move can introduce a horizontal
        // scroll bar; widening makes room before columns grow.
        fill_columns(true);
        win::place_children(&moves, &shown);
        fill_columns(false);
        if let Some(filters) = filters {
            self.controls()
                .filters
                .place_filters(filters, self.controls().filters.scroll.get());
            pane.kind.fill_column();
            // The paths are shortened to the list's new width.
            self.controls().filters.update_locations(self);
        }
        let status = StatusCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        controls
            .status
            .place_in_window(&layout.status, &status.right, status.memory.as_ref());
        controls.status.set_visible(!maximized);
        created.layout.replace(Some(layout));
        if message_focus_lost {
            controls.search.row.input.hwnd().focus();
        }
        self.update_status();
        if grow && !hwnd.is_minimized() {
            let outer = win::window::window_size(hwnd, minimum, metrics.dpi);
            let rect = hwnd.screen_rect();
            hwnd.set_rect(Rect {
                width: rect.width.max(outer.width),
                height: rect.height.max(outer.height),
                ..rect
            });
        }
    }

    fn on_dpi_changed(&self, hwnd: Hwnd, dpi: Dpi, suggested: Rect) {
        let created = self.created();
        let controls = &created.controls;
        let Ok(fonts) = Fonts::new(dpi) else { return };
        controls.apply_fonts(&fonts);
        drop(created.fonts.replace(fonts));
        self.apply_glyphs(dpi);
        self.controls()
            .search
            .column_sizing
            .borrow_mut()
            .dpi_changed();
        let captions = self.captions();
        let _ = win::menu::replace_menu_bar(hwnd, &captions.menu_bar);
        self.measure(&captions, dpi);
        // The minimum size is in the old DPI's pixels until the layout
        // runs again, so it does not hold the window to the old size.
        created.layout.replace(None);
        hwnd.set_rect(suggested);
        self.layout(hwnd);
    }

    /// Which control of the window `control` is, as the command table asks.
    fn focus_of(&self, control: Option<Hwnd>) -> Focus {
        let controls = self.controls();
        if let Some(value) = control.and_then(|c| controls.inspector.value(c)) {
            return Focus::Value {
                text_selected: value.has_selected_text(),
            };
        }
        match control {
            Some(control) if control == controls.search.row.input.hwnd() => Focus::SearchBox {
                text_selected: controls.search.row.input.has_selected_text(),
            },
            Some(control) if control == controls.search.list.hwnd() => Focus::List,
            Some(control) if self.controls().panels.panel_list(control).is_some() => Focus::Panel,
            Some(control) if self.controls().panels.panel_path(control).is_some() => Focus::PathBox,
            Some(control) if self.controls().panels.panel_search(control).is_some() => {
                Focus::PanelSearch
            }
            // An open list takes Enter itself, to close.
            Some(control)
                if self
                    .controls()
                    .panels
                    .panel_look_in(control)
                    .is_some_and(|side| !self.panel(side).look_in.is_open()) =>
            {
                Focus::LookIn
            }
            // An open list takes Enter itself, to close.
            Some(control)
                if controls
                    .filters
                    .range_of(control)
                    .is_some_and(|range| !controls.filters.range(range).is_open()) =>
            {
                Focus::RangeFilter
            }
            _ => Focus::Other,
        }
    }

    fn focus(&self) -> Focus {
        self.focus_of(Hwnd::focused())
    }

    /// The inspector value that has the focus, if one has it.
    fn focused_value(&self) -> Option<Edit> {
        Hwnd::focused().and_then(|focused| self.controls().inspector.value(focused))
    }

    /// Each item's ID, whether it is enabled and whether it is checked, from
    /// the command table.
    fn item_states(&self, items: &[MenuItem]) -> Vec<(u16, bool, bool)> {
        self.item_states_for(items, self.command_context())
    }

    fn item_states_for(
        &self,
        items: &[MenuItem],
        context: selection::CommandContext,
    ) -> Vec<(u16, bool, bool)> {
        let focus = context.focus;
        let model = self.model.borrow();
        let pending = self.selection_command_pending(context.list);
        let global_cancel = self.main_waiting().is_some_and(Awaited::can_cancel);
        let state = State {
            model: &model,
            operations_ready: self.file_operations.get().is_some(),
            focus,
            list: context.list,
        };
        items
            .iter()
            .map(|item| {
                (
                    item.id(),
                    (item.enabled(&state)
                        || (*item == MenuItem::Run(CommandId::Cancel) && global_cancel))
                        && !(pending
                            && (item.reads_selection(focus) || item.writes_selection(focus))),
                    item.checked(&state),
                )
            })
            .collect()
    }

    /// Sets every menu item's enabled state and mark from the command
    /// table, after the selection of the action that opened the menu is
    /// read.
    fn update_menu(&self, hwnd: Hwnd) {
        self.controls().search.read_list_selection(self, hwnd);
        self.controls().panels.read_panel_selection(self);
        let Some(bar) = win::menu::Menu::of(hwnd) else {
            return;
        };
        let mode = self.model.borrow().mode();
        let items = commands::items(&commands::menu_bar(mode));
        for (id, enabled, checked) in self.item_states(&items) {
            bar.set_state(id, enabled, checked);
        }
    }

    /// Runs one menu item or shortcut; every surface calls this (DESIGN.md:
    /// window rule 9). It asks the command table again whether the item is
    /// enabled, because the state can change while a menu is open (DESIGN.md:
    /// T11).
    fn run(&self, hwnd: Hwnd, item: MenuItem) {
        let context = self.command_context();
        self.run_in_context(hwnd, item, context);
    }

    fn run_in_context(&self, hwnd: Hwnd, item: MenuItem, context: selection::CommandContext) {
        if !self.selection_context_current(context) {
            return;
        }
        let writing = item.writes_selection(context.focus)
            && (self.native_update_active(context.list)
                || self.native_deselection_pending(context.list)
                || self.selection_command_pending(context.list));
        if (item.reads_selection(context.focus) || writing)
            && !self.prepare_selection_command(hwnd, item, context)
        {
            return;
        }
        self.run_command(hwnd, item, context);
    }

    fn run_command(&self, hwnd: Hwnd, item: MenuItem, context: selection::CommandContext) {
        if !self.selection_context_current(context) {
            return;
        }
        let selection::CommandContext {
            mode, list, focus, ..
        } = context;
        let enabled = {
            let model = self.model.borrow();
            item.enabled(&State {
                model: &model,
                operations_ready: self.file_operations.get().is_some(),
                focus,
                list,
            }) || (item == MenuItem::Run(CommandId::Cancel)
                && self.main_waiting().is_some_and(Awaited::can_cancel))
        };
        if !enabled {
            return;
        }
        if mode == Mode::Commander {
            // Any key or command closes the box first, so F5 after a
            // quick search copies the focused row.
            self.controls().panels.close_quick_search(self);
        }
        let active = match list {
            List::Panel(side) => side,
            List::Search => self.model.borrow().panels().active(),
        };
        let controls = self.controls();
        let accepted = match item {
            MenuItem::Run(command) => match command {
                CommandId::Exit => {
                    self.exit(hwnd);
                    return;
                }
                CommandId::About => {
                    let about = {
                        let model = self.model.borrow();
                        About::new(self, model.strings(), model.service_version())
                    };
                    self.open_dialog(hwnd, DialogId::About, &about);
                    return;
                }
                CommandId::Open => {
                    if let Some(entries) = self.selected_source(list) {
                        self.open_chosen(list, entries);
                    }
                    return;
                }
                CommandId::ShowInCommander => {
                    if let Some(entry) =
                        self.selected_source(list).and_then(|entries| entries.one())
                    {
                        self.show_in_commander(list, entry);
                    }
                    return;
                }
                CommandId::OpenFileLocation => {
                    return self.act_on_selected(list, EntryAction::OpenFileLocation);
                }
                CommandId::Explore => return self.act_on_selected(list, EntryAction::Explore),
                CommandId::Properties => {
                    return self.act_on_selected(list, EntryAction::Properties);
                }
                CommandId::ExportResults => return self.export(hwnd),
                CommandId::Cancel => {
                    if self.cancel_main_waiting(hwnd) {
                        return;
                    }
                    self.model.borrow_mut().cancel_job()
                }
                CommandId::Copy => match focus {
                    Focus::SearchBox { .. } => {
                        self.cancel_copy(hwnd);
                        return controls.search.row.input.copy();
                    }
                    Focus::Value { .. } => {
                        self.cancel_copy(hwnd);
                        if let Some(value) = self.focused_value() {
                            value.copy();
                        }
                        return;
                    }
                    Focus::List
                    | Focus::RangeFilter
                    | Focus::Panel
                    | Focus::PathBox
                    | Focus::PanelSearch
                    | Focus::LookIn
                    | Focus::Other => {
                        return self.copy(hwnd, ClipboardFormat::Files);
                    }
                },
                CommandId::CopyAsPath => return self.copy(hwnd, ClipboardFormat::Paths),
                CommandId::CopyName => return self.copy(hwnd, ClipboardFormat::Names),
                CommandId::CopyFolder => return self.copy(hwnd, ClipboardFormat::Folders),
                CommandId::SelectAll => match focus {
                    Focus::SearchBox { .. } => return controls.search.row.input.select_all(),
                    Focus::Value { .. } => {
                        if let Some(value) = self.focused_value() {
                            value.select_all();
                        }
                        return;
                    }
                    Focus::PathBox => return self.panel(active).path.select_all(),
                    Focus::PanelSearch => return self.panel(active).search.select_all(),
                    Focus::List
                    | Focus::RangeFilter
                    | Focus::Panel
                    | Focus::LookIn
                    | Focus::Other => {
                        return match mode {
                            Mode::Search => controls.search.list.select_all(),
                            // `..` is let go when the selection is read.
                            Mode::Commander => self.panel(active).list.select_all(),
                        };
                    }
                },
                CommandId::ClearSelection => return self.clear_selection(list),
                CommandId::SelectFiles => {
                    return self
                        .controls()
                        .panels
                        .select_files(self, hwnd, active, true);
                }
                CommandId::UnselectFiles => {
                    return self
                        .controls()
                        .panels
                        .select_files(self, hwnd, active, false);
                }
                CommandId::Find => {
                    if mode == Mode::Commander {
                        return self.controls().panels.find_in_panel(self, hwnd, active);
                    }
                    controls.search.row.input.hwnd().focus();
                    return controls.search.row.input.select_all();
                }
                CommandId::Refresh => {
                    return match mode {
                        Mode::Search => {
                            self.created().work.borrow_mut().lists.refresh_location();
                            self.search(hwnd, List::Search);
                        }
                        Mode::Commander => self.controls().panels.refresh_panel(self, hwnd, active),
                    };
                }
                CommandId::SearchSyntax => {
                    self.open_dialog(hwnd, DialogId::SearchSyntax, &SearchSyntax);
                    return;
                }
                CommandId::SearchInThisFolder => self.model.borrow_mut().search_in_this_folder(),
                CommandId::Filters => self.model.borrow_mut().toggle_filter_pane(),
                CommandId::Inspector => self.model.borrow_mut().toggle_inspector(),
                CommandId::ClearFilters => self.model.borrow_mut().clear_filters(),
                CommandId::Settings => return self.settings.open_settings(self, hwnd),
                CommandId::ShowSearch => return self.show_mode(hwnd, Mode::Search),
                CommandId::ShowCommander => return self.show_mode(hwnd, Mode::Commander),
                CommandId::CopyToFolder | CommandId::MoveToFolder if mode == Mode::Commander => {
                    return self.controls().panels.transfer(
                        self,
                        hwnd,
                        command == CommandId::MoveToFolder,
                        active,
                    );
                }
                CommandId::CopyToFolder => return self.copy_to_folder(hwnd, false, list),
                CommandId::MoveToFolder => return self.copy_to_folder(hwnd, true, list),
                CommandId::Synchronize => return self.synchronize_folders(hwnd, None),
                CommandId::Delete => return self.delete_selected(hwnd, list),
                CommandId::NewFolder => {
                    return self.controls().panels.new_folder(self, hwnd, active);
                }
                CommandId::View => return self.view_focused(hwnd, active),
                // Not available in this version (DESIGN.md: Commands in Two
                // panels).
                CommandId::Edit => return,
                CommandId::Cut => return self.copy(hwnd, ClipboardFormat::CutFiles),
                CommandId::Paste => return self.paste(),
                CommandId::FileOperations => return self.show_operations(None),
                CommandId::Commander => {
                    let mut model = self.model.borrow_mut();
                    let other = match model.mode() {
                        Mode::Search => Mode::Commander,
                        Mode::Commander => Mode::Search,
                    };
                    model.show_mode(other)
                }
                CommandId::Columns | CommandId::Language | CommandId::Match | CommandId::SortBy => {
                    return;
                }
            },
            MenuItem::Column(column) => self.model.borrow_mut().toggle_column(column),
            MenuItem::FitColumn(column) => {
                let index = self
                    .model
                    .borrow()
                    .columns()
                    .iter()
                    .position(|c| *c == column);
                if let Some(index) = index {
                    self.controls().search.fit_column(self, index);
                }
                return;
            }
            MenuItem::FitColumns => {
                for (index, fit) in self
                    .controls()
                    .search
                    .column_fits(self)
                    .into_iter()
                    .enumerate()
                {
                    self.controls()
                        .search
                        .list
                        .set_column_width(index, fit.widest);
                }
                return;
            }
            MenuItem::SortKey(key) if mode == Mode::Commander => {
                return self.controls().panels.change_panel_sort(
                    self,
                    hwnd,
                    active,
                    Some(key),
                    None,
                );
            }
            MenuItem::SortDirection(dir) if mode == Mode::Commander => {
                return self.controls().panels.change_panel_sort(
                    self,
                    hwnd,
                    active,
                    None,
                    Some(dir),
                );
            }
            MenuItem::SortKey(key) => {
                let mut model = self.model.borrow_mut();
                let sort = model.key_sort(key);
                model.sort_by(sort)
            }
            MenuItem::SortDirection(dir) => {
                let mut model = self.model.borrow_mut();
                let key = model.sort().key;
                model.sort_by(SortSpec { key, dir })
            }
            MenuItem::Panel(action) => {
                return self.controls().panels.run_panel_action(self, hwnd, action);
            }
            MenuItem::Language(place) => {
                self.set_language(hwnd, LanguageChoice::at(place));
                self.save_settings(true);
                return;
            }
        };
        self.apply(hwnd, accepted);
    }

    /// Runs the search that `list`'s text and controls ask for now. Only
    /// Search's Location uses its last background observation; a search panel's
    /// folder is the one the panel just showed.
    fn search(&self, hwnd: Hwnd, list: List) {
        let accepted = self.begin_search(hwnd, list);
        self.apply(hwnd, accepted);
    }

    fn begin_search(&self, hwnd: Hwnd, list: List) -> Accepted {
        hwnd.kill_timer(Timer::Search(list).id());
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: jiff::tz::TimeZone::system(),
        };
        let location = match list {
            List::Search => self.model.borrow().location().map(str::to_owned),
            List::Panel(_) => None,
        };
        let scope = match list {
            List::Search => {
                self.created()
                    .work
                    .borrow_mut()
                    .lists
                    .check_location(location.as_deref());
                self.model.borrow().location_check()
            }
            List::Panel(_) => ScopeCheck::Exists,
        };
        self.model.borrow_mut().start_search(list, &start, scope)
    }

    /// Names `list`'s search as running, once it has run
    /// [`SEARCHING_NAMED_MS`] without an answer.
    fn name_search(&self, hwnd: Hwnd, list: List) {
        hwnd.kill_timer(Timer::Searching(list).id());
        let accepted = self.model.borrow_mut().name_search(list);
        self.apply(hwnd, accepted);
    }

    /// Runs the dialog `dialog` with `state`. A dialog that Windows cannot
    /// open leaves a line in `ui-error.log` and the window as it was.
    fn open_dialog<D: DialogProc>(
        &self,
        hwnd: Hwnd,
        dialog: DialogId,
        state: &D,
    ) -> Option<DialogEnd> {
        let captions = DialogCaptions::new(dialog, self.model.borrow().strings());
        match win::dialog::run_modal(hwnd, dialog, &captions, state) {
            Ok(end) => Some(end),
            Err(error) => {
                self.log_error(format!("The {dialog:?} dialog did not open: {error}"));
                None
            }
        }
    }

    /// Asks for status unless a request is in flight.
    fn ask_status(&self) {
        let fetch = self.model.borrow_mut().status_needed();
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// The list of users and groups who see every file changed: the lists
    /// read again, as F5 reads them, so that they hold what the person may
    /// see now, and the status tells the Visibility page. The Search list
    /// searches again also while Commander mode shows, because it keeps its
    /// results until it shows; the panels read their folders when they show.
    fn show_again(&self, hwnd: Hwnd) {
        self.created().work.borrow_mut().lists.refresh_location();
        self.search(hwnd, List::Search);
        if self.model.borrow().mode() == Mode::Commander {
            for side in Side::BOTH {
                self.controls().panels.refresh_panel(self, hwnd, side);
            }
        }
        self.ask_status();
    }

    /// While the status bar shows memory, reads the app's own working set,
    /// one cheap call, to show beside the service's from the same status
    /// (DESIGN.md: Status bar).
    fn read_app_memory(&self) -> Accepted {
        if !self.model.borrow().choices().show_memory {
            return Accepted::default();
        }
        let working_set = gozler_winfs::self_memory().ok().map(|own| own.working_set);
        self.model.borrow_mut().set_app_working_set(working_set)
    }

    fn start(&self, fetch: Fetch) {
        let mut work = self.created().work.borrow_mut();
        if fetch.purpose() == Purpose::Status {
            work.programs.refresh();
        }
        work.requests.start(fetch);
        let retired = work.requests.take_retired();
        work.retire(retired);
    }

    fn cancel_request(&self, purpose: Purpose) {
        let mut work = self.created().work.borrow_mut();
        work.requests.cancel(purpose);
        let retired = work.requests.take_retired();
        work.retire(retired);
    }

    /// Asks for the chunk that `list`'s rows on screen still lack, so every
    /// chunk request comes from the rows visible now (DESIGN.md: Rows on
    /// demand).
    fn fetch_visible(&self, list: List) {
        let visible = self.list_control(list).visible_rows();
        let fetch = self.model.borrow_mut().rows_needed(list, visible);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// The control that shows `list`.
    fn list_control(&self, list: List) -> ListView {
        match list {
            List::Search => self.controls().search.list,
            List::Panel(side) => self.panel(side).list,
        }
    }

    /// The selection timer of `list` ran out: its selected rows are asked
    /// for.
    fn fetch_selection(&self, hwnd: Hwnd, list: List) {
        hwnd.kill_timer(Timer::Selection(list).id());
        let fetch = self.model.borrow_mut().read_selection(list);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    fn retire_model(&self) {
        let mut retired = self.model.borrow_mut().take_retired();
        let mut work = self.created().work.borrow_mut();
        retired.append(work.folders.take_retired());
        retired.append(work.indexed.take_retired());
        work.retire(retired);
    }

    pub(super) fn indexed(&self) -> std::cell::RefMut<'_, super::work::indexed::Work> {
        std::cell::RefMut::map(self.created().work.borrow_mut(), |work| &mut work.indexed)
    }

    /// Starts the save timer when the state that `ui.json` saves changed
    /// (DESIGN.md: Where settings are saved). Accepted transitions request
    /// this observation as an effect; this entry serves panel events and
    /// commands, window moves and [`Message::Deferred`]'s header drag.
    /// Viewer state arrives explicitly from its owner before observation.
    pub(super) fn note_saved_state(&self, hwnd: Hwnd) {
        let changed = {
            let mut model = self.model.borrow_mut();
            model.saved_state_changed()
        };
        if changed {
            hwnd.set_timer(Timer::Save.id(), SAVE_DELAY_MS);
        }
    }

    /// Runs `step` from [`Message::Deferred`], after every reply and panel
    /// event of this wake has applied (DESIGN.md: window rule 13).
    fn defer(&self, hwnd: Hwnd, step: ModalStep) {
        self.modal_steps.borrow_mut().push_back(step);
        hwnd.post(DEFERRED);
    }

    /// Runs the deferred modal steps one at a time. A step's own loop
    /// dispatches wakes, whose steps wait here for it to end, so no dialog
    /// opens over another.
    fn run_modal_steps(&self, hwnd: Hwnd) {
        if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
            return;
        }
        if self.created().work.borrow().operations.preparing_report() {
            return;
        }
        if self.modal_running.replace(true) {
            return;
        }
        loop {
            let step = self.modal_steps.borrow_mut().pop_front();
            let Some(step) = step else { break };
            match step {
                ModalStep::ConfirmSetup => self.confirm_setup(hwnd),
                ModalStep::ExplainNoSetup => self.explain_no_setup(hwnd),
                ModalStep::ExportFailed { path, error } => {
                    self.show_export_failure(hwnd, &path, &error);
                }
                ModalStep::SettingsNotSaved(error) if !self.ending_session.get() => {
                    let text = self.model.borrow().strings().format(
                        "message.settings_not_saved",
                        &[
                            &self.settings_file.display().to_string(),
                            &win::error_text(&error),
                        ],
                    );
                    self.error_box(hwnd, &text);
                }
                ModalStep::SettingsNotSaved(_) => {}
                ModalStep::WindowsShortcutFailed(error) => {
                    let text = self.model.borrow().strings().format(
                        "settings_keyboard.windows_failed",
                        &[&win::error_text(&error)],
                    );
                    self.error_box(hwnd, &text);
                }
                ModalStep::CopyFailed(id, failure) => {
                    if !self.created().work.borrow().clipboard.is_current(id) {
                        continue;
                    }
                    let text = failure.message(self.model.borrow().strings());
                    self.error_box(hwnd, &text);
                }
                ModalStep::PreparedPaths(purpose, id, prepared) => {
                    if !self
                        .created()
                        .work
                        .borrow()
                        .operations
                        .paths_are_current(purpose, id)
                    {
                        self.retire_paths(prepared.paths);
                        continue;
                    }
                    match prepared.action {
                        PathAction::Transfer { moves } => {
                            self.copy_paths_to_folder(
                                hwnd,
                                moves,
                                prepared.paths,
                                prepared.single_folder,
                            );
                        }
                        PathAction::PanelTransfer {
                            moves,
                            place,
                            selected,
                            start,
                        } => {
                            self.transfer_paths(
                                hwnd,
                                moves,
                                place,
                                selected,
                                *start,
                                prepared.paths,
                            );
                        }
                        PathAction::Delete { place } => {
                            self.delete_paths(hwnd, place, prepared.paths)
                        }
                    }
                }
                ModalStep::OpenSettings(id, read) => {
                    if self
                        .created()
                        .work
                        .borrow()
                        .settings
                        .registry_is_current(id)
                        && self.settings.open.get().is_none()
                    {
                        self.settings.show_settings(self, hwnd, read);
                    }
                }
                ModalStep::Opened(purpose, id, opened) => {
                    if !self.created().work.borrow().shell.is_current(purpose, id) {
                        continue;
                    }
                    if let Some(entry) = opened.commander {
                        self.controls().panels.show_entry_in_commander(self, entry);
                        continue;
                    }
                    if purpose == ShellPurpose::Setup {
                        match opened.failure {
                            Some(failure) => {
                                let error = win::error_text(&failure.error);
                                let accepted = self.model.borrow_mut().setup_failed(error);
                                self.apply(hwnd, accepted);
                            }
                            None => {
                                // Setup waits for Gozler to close. Native modal
                                // loops must unwind before its HWND is destroyed.
                                self.ending.set(Ending::ForSetup);
                                win::window::quit_modal_loops();
                            }
                        }
                        continue;
                    }
                    if let Some(failure) = opened.failure {
                        let reason = win::error_text(&failure.error);
                        let text = {
                            let model = self.model.borrow();
                            match failure.target {
                                OpenTarget::Command(program) => model
                                    .strings()
                                    .format("message.open_failed", &[&program, &reason]),
                                OpenTarget::Path(path) => model.strings().format(
                                    "panel.cannot_open",
                                    &[&String::from_utf16_lossy(&path), &reason],
                                ),
                            }
                        };
                        self.error_box(self.fix_owner(hwnd), &text);
                    }
                    if self.created().work.borrow().shell.is_current(purpose, id) {
                        self.report_missing(hwnd, opened.missing);
                    }
                }
                ModalStep::Paired(id, _, _)
                    if !self.created().work.borrow().settings.pairing_is_current(id) => {}
                ModalStep::Paired(_, local, true) => self.start_fix(hwnd, &Fix::UseLocal(local)),
                ModalStep::Paired(_, _, false) => {
                    let (text, title) = {
                        let model = self.model.borrow();
                        let text = |key: &str| model.strings().text(key).to_owned();
                        (
                            text("fixing.use_local_not_paired"),
                            text("fixing.use_local_caption"),
                        )
                    };
                    win::dialog::error_box(Some(self.fix_owner(hwnd)), &text, &title);
                }
                ModalStep::PanelRefused(refused) => {
                    self.controls().panels.refuse(self, hwnd, refused)
                }
                ModalStep::EnterFirst(ask) => {
                    let shown =
                        self.model.borrow().search_list().results().shown_ask() == Some(&ask);
                    if shown {
                        self.controls().search.enter_first_or_focused(self);
                    }
                }
                ModalStep::Operation(dialog) => {
                    if self.operation_dialog(hwnd, dialog) {
                        break;
                    }
                }
                ModalStep::PreparedFailure(prepared) => self.show_prepared_failure(hwnd, prepared),
                ModalStep::Synchronize(id) => self.synchronize_folders(hwnd, Some(id)),
            }
        }
        self.modal_running.set(false);
    }

    /// Builds the menu bar and the accelerator table again, for the shown
    /// mode and the person's shortcuts.
    fn replace_menu_bar(&self, hwnd: Hwnd) {
        let (menu_bar, shortcuts, mode) = {
            let model = self.model.borrow();
            let shortcuts = &model.choices().shortcuts;
            let menu_bar = commands::render(
                &commands::menu_bar(model.mode()),
                model.strings(),
                shortcuts,
                model.mode(),
            );
            (menu_bar, shortcuts.clone(), model.mode())
        };
        let _ = win::menu::replace_menu_bar(hwnd, &menu_bar);
        if let Ok(table) = accelerator_table(&shortcuts, mode) {
            self.accelerators.replace(Rc::new(table));
        }
    }

    /// Starts or stops the one status timer as the model says (DESIGN.md:
    /// Requests).
    fn update_poll(&self, hwnd: Hwnd) {
        let on_screen = hwnd.is_on_screen();
        let polls = self.model.borrow().polls_status(on_screen);
        if polls != self.polling.replace(polls) {
            if polls {
                hwnd.set_timer(Timer::Status.id(), STATUS_POLL_MS);
            } else {
                hwnd.kill_timer(Timer::Status.id());
            }
        }
    }

    /// The window was shown, hidden, minimized or restored, which starts or
    /// stops the memory group's figures; coming on screen asks for fresh
    /// ones at once.
    fn follow_visibility(&self, hwnd: Hwnd) {
        self.follow_dates(hwnd);
        let polled = self.polling.get();
        self.update_poll(hwnd);
        if self.polling.get() && !polled {
            self.ask_status();
        }
    }

    fn follow_dates(&self, hwnd: Hwnd) {
        if self.dates.borrow().format == DateFormat::Relative && hwnd.is_on_screen() {
            self.refresh_dates();
            hwnd.set_timer(Timer::Dates.id(), AGE_REFRESH_MS);
        } else {
            hwnd.kill_timer(Timer::Dates.id());
        }
    }

    fn refresh_dates(&self) {
        self.dates.borrow_mut().set_time(jiff::Zoned::now());
        let mode = self.model.borrow().mode();
        match mode {
            Mode::Search => {
                let list = self.controls().search.list;
                list.redraw_rows(list.visible_rows());
            }
            Mode::Commander => {
                for side in Side::BOTH {
                    let list = self.panel(side).list;
                    list.redraw_rows(list.visible_rows());
                }
            }
        }
    }

    /// Accepts every reply that has arrived, and the newer release that the
    /// update check found.
    fn on_wake(&self, hwnd: Hwnd) {
        self.created().waker.woken();
        if !self.update_live() {
            return;
        }
        self.shortcuts.poll_start(self, hwnd);
        self.settings.post_settings_reply();
        {
            let mut work = self.created().work.borrow_mut();
            work.indexed.poll();
            let retired = work.indexed.take_retired();
            work.retire(retired);
        }
        self.accept_told_selections(hwnd);
        if !self.update_live() {
            return;
        }
        self.apply_taken_deselections(hwnd);
        if !self.update_live() {
            return;
        }
        self.controls().search.read_list_selection(self, hwnd);
        if !self.update_live() {
            return;
        }
        self.controls().panels.read_panel_selection(self);
        if !self.update_live() {
            return;
        }
        let report = self.created().work.borrow_mut().operations.take_report();
        if let Some(report) = report {
            self.modal_steps
                .borrow_mut()
                .push_front(ModalStep::PreparedFailure(report));
            hwnd.post(DEFERRED);
        }
        let followed = self.created().work.borrow_mut().operations.take_followed();
        if let Some(answer) = followed {
            let steps = self.model.borrow_mut().accept_followed(answer);
            self.apply_operation_steps(hwnd, steps);
            if !self.update_live() {
                return;
            }
        }
        let panels = self.created().work.borrow_mut().panels.take_calculated();
        for (side, answer) in panels {
            let update = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_calculation(side, answer);
            if let Some(update) = update {
                self.controls()
                    .panels
                    .apply_panel_update(self, hwnd, side, update);
            }
            self.retire_model();
        }
        let counted = self.created().work.borrow_mut().panels.take_counts();
        for (side, answer) in counted {
            let current = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_count(side, answer);
            if current && self.model.borrow().mode() == Mode::Commander {
                self.controls().panels.show_panel(self, side);
            }
            self.retire_model();
        }
        let found = self.created().work.borrow_mut().panels.take_found();
        for (side, answer) in found {
            let row = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_quick(side, answer);
            if let Some(row) = row {
                self.controls().panels.quick_go(self, side, row);
            }
            self.controls().panels.show_panel(self, side);
            self.retire_model();
        }
        self.start_list_passes();
        let kept = self.created().work.borrow_mut().lists.take_kept();
        if !kept.is_empty() {
            let mut retired = Retirement::default();
            for (list, answer) in kept {
                if !self.update_live() {
                    let items = answer.kept.len();
                    retired.push(answer, items);
                    continue;
                }
                let accepted = self.model.borrow_mut().accept_kept(list, answer);
                self.apply(hwnd, accepted);
            }
            self.created().work.borrow_mut().retire(retired);
            if !self.update_live() {
                return;
            }
            self.release_operations();
        }
        let filtered = self.created().work.borrow_mut().lists.take_filtered();
        let mut retired = Retirement::default();
        for (list, answer) in filtered {
            if !self.update_live() {
                let items = answer.input.retained_items();
                retired.push(answer, items);
                continue;
            }
            let accepted = self.model.borrow_mut().accept_filtered(list, answer);
            self.apply(hwnd, accepted);
        }
        self.created().work.borrow_mut().retire(retired);
        if !self.update_live() {
            return;
        }
        let restored = self.created().work.borrow_mut().queue.take();
        if let Some(restored) = restored {
            self.adopt_queue(hwnd, restored);
        }
        let location = self.created().work.borrow_mut().lists.take_location();
        if let Some((folder, check)) = location {
            let changed = self.model.borrow_mut().observe_location(folder, check);
            if changed {
                self.search(hwnd, List::Search);
            }
        }
        if !self.update_live() {
            return;
        }
        let labels = self.created().work.borrow_mut().settings.take_labels();
        if let Some(labels) = labels {
            self.settings.accept_labels(self, labels);
            self.settings.post_settings_reply();
        }
        let names = self
            .created()
            .work
            .borrow_mut()
            .settings
            .take_account_names();
        if let Some(names) = names {
            self.settings.accept_account_names(names);
        }
        let copied = self.created().work.borrow_mut().clipboard.take();
        match copied {
            Some(CopyNotice::Retry) => hwnd.set_timer(
                Timer::Clipboard.id(),
                crate::core::clipboard::RETRY_AFTER.as_millis() as u32,
            ),
            Some(CopyNotice::Failed(id, error)) => {
                self.defer(hwnd, ModalStep::CopyFailed(id, error))
            }
            Some(CopyNotice::Finished) | None => {}
        }
        loop {
            let answer = self.created().work.borrow_mut().settings.take_registry();
            let Some((id, answer)) = answer else { break };
            match answer {
                RegistryAnswer::Read(read) => self.defer(hwnd, ModalStep::OpenSettings(id, read)),
                RegistryAnswer::SignIn(result) => {
                    self.settings.sign_in_saved.replace(Some((id, result)));
                    self.settings.post_settings_reply();
                }
            }
        }
        let opened = self.created().work.borrow_mut().shell.take_opened();
        for (purpose, id, opened) in opened {
            self.defer(hwnd, ModalStep::Opened(purpose, id, opened));
        }
        let prepared = self.created().work.borrow_mut().operations.take_paths();
        for (purpose, id, prepared) in prepared {
            self.defer(hwnd, ModalStep::PreparedPaths(purpose, id, prepared));
        }
        let paired = self.created().work.borrow_mut().settings.take_pairing();
        if let Some((id, local, paired)) = paired {
            self.defer(hwnd, ModalStep::Paired(id, local, paired));
        }
        loop {
            let saved = self.created().work.borrow_mut().settings.take_saved();
            let Some(saved) = saved else { break };
            if let Some(Err(error)) = saved.windows_shortcut {
                self.defer(hwnd, ModalStep::WindowsShortcutFailed(error));
            }
            if let Err(error) = saved.result
                && saved.report_failure
                && !self.ending_session.get()
                && !self.settings_failure_shown.replace(true)
            {
                self.defer(hwnd, ModalStep::SettingsNotSaved(error));
            }
        }
        let fetched = {
            let mut work = self.created().work.borrow_mut();
            let programs = work.programs.drain();
            work.requests.set_programs(programs);
            let retired = work.programs.take_retired();
            work.retire(retired);
            let fetched = work.requests.drain();
            let retired = work.requests.take_retired();
            work.retire(retired);
            fetched
        };
        let mut retired = Retirement::default();
        for fetched in fetched {
            if !self.update_live() {
                let items = fetched.outcome.items();
                retired.push(fetched, items);
                continue;
            }
            {
                let model = self.model.borrow();
                if model.awaits(fetched.purpose, fetched.generation) {
                    if let crate::core::fetch::Outcome::Status(status, _, _) = &fetched.outcome {
                        self.settings.observe_status(model.service(), status);
                    }
                    if fetched.purpose == Purpose::IncludeCount {
                        self.settings.include_preview_changed();
                    }
                }
            }
            let accepted = self.model.borrow_mut().accept(fetched);
            self.apply(hwnd, accepted);
        }
        self.created().work.borrow_mut().retire(retired);
        if !self.update_live() {
            return;
        }
        let fixes = {
            let mut work = self.created().work.borrow_mut();
            let fixes = work.fixes.drain();
            let retired = work.fixes.take_retired();
            work.retire(retired);
            fixes
        };
        for (generation, event) in fixes {
            let current = self.update_live() && self.model.borrow().awaits_fix(generation);
            if !current {
                let mut work = self.created().work.borrow_mut();
                work.fixes.discard(event);
                let retired = work.fixes.take_retired();
                work.retire(retired);
                continue;
            }
            let accepted = self.model.borrow_mut().accept_fix(generation, event);
            self.apply(hwnd, accepted);
        }
        if !self.update_live() {
            return;
        }
        self.settings.finish_elevated_job(self, hwnd);
        if !self.update_live() {
            return;
        }
        self.finish_paste();
        if !self.update_live() {
            return;
        }
        self.finish_drops();
        if !self.update_live() {
            return;
        }
        self.controls().panels.on_panel_events(self, hwnd);
        if !self.update_live() {
            return;
        }
        let mut tray_events = Vec::new();
        {
            let mut work = self.created().work.borrow_mut();
            if let Some(tray) = &mut work.tray {
                tray.drain();
                while let Some(event) = tray.take_event() {
                    tray_events.push(event);
                }
                let retired = tray.take_retired();
                work.retire(retired);
            }
        }
        for event in tray_events {
            self.tray.on_tray(self, hwnd, event);
            if self.shutdown.borrow().is_some() {
                return;
            }
        }
        let releases = self.created().work.borrow_mut().take_releases();
        for release in releases {
            let accepted = match release {
                ReleaseAnswer::Newer(Some(version)) => {
                    self.model.borrow_mut().set_newer_release(version)
                }
                ReleaseAnswer::Older(older) => {
                    self.model.borrow_mut().set_latest_release_older(older)
                }
                ReleaseAnswer::Newer(None) => continue,
            };
            self.apply(hwnd, accepted);
        }
        if !self.update_live() {
            return;
        }
        self.resume_selection_actions(hwnd);
        if !self.update_live() {
            return;
        }
        self.poll_work(hwnd);
    }

    fn poll_work(&self, hwnd: Hwnd) {
        if self.shutdown.borrow().is_some() {
            return;
        }
        let polled = self.created().work.borrow_mut().tick(Instant::now());
        self.expire_hover(&polled.expired_hover);
        for (purpose, generation) in polled.expired_requests {
            let accepted = self.model.borrow_mut().expire_request(purpose, generation);
            self.apply(hwnd, accepted);
        }
        self.update_message();
        self.update_status();
        if self.model.borrow().mode() == Mode::Commander {
            for side in Side::BOTH {
                self.controls().panels.show_panel_message(self, side);
            }
        }
        if polled.active {
            hwnd.set_timer(Timer::Work.id(), 250);
        } else {
            hwnd.kill_timer(Timer::Work.id());
        }
    }

    /// A control's notification in `WM_COMMAND`.
    fn on_control(&self, hwnd: Hwnd, event: ControlEvent) {
        let controls = self.controls();
        if let Some(input) = controls.status.transfer_input(event) {
            match input {
                crate::core::status_transfer::Input::Toggle => {
                    controls.status.restore_transfer_focus();
                    self.controls().inspector.remember_inspector_focus(self);
                    if self.file_operations.get().is_some() {
                        self.model.borrow_mut().open_transfer_selection(None);
                    }
                    let accepted = self.model.borrow_mut().toggle_transfers();
                    self.apply(hwnd, accepted);
                    self.refresh_operations();
                }
                crate::core::status_transfer::Input::Action(id, QueueAction::Report) => {
                    self.show_operations(Some(id));
                    self.model
                        .borrow_mut()
                        .select_operation(crate::core::file_operations::Detail::Report(id));
                    self.refresh_operations();
                }
                crate::core::status_transfer::Input::Action(id, action) => {
                    self.act_operation(id, action);
                }
            }
            return;
        }
        if controls.inspector.on_control(self, hwnd, event)
            || controls.search.on_control(self, hwnd, event)
            || controls.panels.on_control(self, hwnd, event)
            || controls.key_bar.clicked(self, hwnd, event)
        {
            return;
        }
        if controls.status.cancel_clicked(event) {
            self.cancel_job(hwnd);
        } else if controls
            .status
            .show_details()
            .is_some_and(|show| show.clicked(event) == Some(SHOW_DETAILS.control.id()))
        {
            self.run(hwnd, MenuItem::Run(SHOW_DETAILS.command));
        } else {
            controls.filters.on_filter(self, hwnd, event);
        }
    }

    /// A context menu of `groups` at `point`; the chosen item runs after
    /// the menu closes (DESIGN.md: window rule 9).
    fn context_menu(
        &self,
        hwnd: Hwnd,
        groups: &[Vec<MenuItem>],
        point: (i32, i32),
        default: Option<MenuItem>,
        context: selection::CommandContext,
    ) {
        let items: Vec<MenuItem> = groups.iter().flatten().copied().collect();
        let states = self.item_states_for(&items, context);
        let nodes = commands::render_popup(groups, self.model.borrow().strings());
        let default = default.map(MenuItem::id);
        let chosen = win::menu::Menu::track_popup(hwnd, &nodes, point, &states, default);
        if let Some(item) = chosen.and_then(MenuItem::from_id) {
            self.run_in_context(hwnd, item, context);
        }
    }

    /// The context menu of a row, at `point`, or at the focused row for the
    /// Menu key and Shift+F10. A right-click below the last row shows none
    /// (DESIGN.md: Results list).
    fn row_menu(&self, hwnd: Hwnd, point: Option<(i32, i32)>) {
        if let Some(context) = self.await_search_menu(hwnd, point) {
            self.row_menu_ready(hwnd, point, context);
        }
    }

    fn row_menu_ready(
        &self,
        hwnd: Hwnd,
        point: Option<(i32, i32)>,
        context: selection::CommandContext,
    ) {
        let list = self.controls().search.list;
        let point = match point {
            Some(point) => list.row_at(point).map(|_| point),
            None => list.focused_row_point(),
        };
        let Some(point) = point else { return };
        // The right-click selected its row a moment ago.
        // The first selected row's kind decides which Open shows, and Shift
        // the items shown only with it (DESIGN.md: The Context menu page).
        let first = list.next_selected(None);
        let groups = {
            let model = self.model.borrow();
            let folder = first
                .and_then(|row| model.search_list().results().row(row))
                .is_some_and(|row| row.is_dir);
            commands::shown_row_menu(
                model.choices().context_menu,
                folder,
                win::window::shift_held(),
            )
        };
        if groups.is_empty() {
            return;
        }
        let open = MenuItem::Run(CommandId::Open);
        let default = groups
            .iter()
            .flatten()
            .any(|item| *item == open)
            .then_some(open);
        self.context_menu(hwnd, &groups, point, default, context);
    }

    /// A key that the control `target` gives a meaning (DESIGN.md:
    /// window rule 8). Returns whether the window took it.
    fn on_key(&self, hwnd: Hwnd, target: Hwnd, mut keystroke: Keystroke) -> bool {
        if keystroke == Keystroke::plain(commands::Key::Escape) {
            let list = if target == self.controls().search.list.hwnd() {
                Some(List::Search)
            } else {
                self.controls().panels.panel_list(target).map(List::Panel)
            };
            if let Some(list) = list {
                if let List::Panel(side) = list
                    && self
                        .created()
                        .work
                        .borrow()
                        .folders
                        .waiting(side, Instant::now())
                        == Some(Waiting::Slow)
                {
                    self.controls()
                        .panels
                        .panel_message_clicked(self, hwnd, side);
                    return true;
                }
                if let Some(awaited) = self.slow_list_work(list) {
                    self.cancel_list_work(hwnd, list, awaited);
                    return true;
                }
            }
        }
        if self
            .file_operations
            .get()
            .is_some_and(|ops| ops.key(target, keystroke))
        {
            return true;
        }
        if keystroke.key == commands::Key::Tab
            && (self
                .file_operations
                .get()
                .is_some_and(|ops| ops.contains(target))
                || self.controls().status.contains(target))
        {
            return false;
        }
        // Shift+Space types a space in the quick search box, as Space does,
        // instead of selecting rows in the list.
        let space = Keystroke::plain(commands::Key::Space);
        if keystroke
            == (Keystroke {
                shift: true,
                ..space
            })
            && self
                .controls()
                .panels
                .panel_list(target)
                .is_some_and(|side| {
                    (self.model.borrow().panels().panel(side))
                        .quick_text()
                        .is_some()
                })
        {
            keystroke = space;
        }
        let Some(action) = commands::control_key(self.focus_of(Some(target)), keystroke) else {
            return false;
        };
        let controls = self.controls();
        match action {
            KeyAction::Run(command) => self.run(hwnd, MenuItem::Run(command)),
            KeyAction::FocusList => {
                let count = self.model.borrow().search_list().results().count();
                controls.search.list.focus_left_row(count);
            }
            KeyAction::SearchBoxEnter => {
                let enter = self.model.borrow_mut().enter_in_search_box();
                match enter {
                    SearchBoxEnter::Search => self.search(hwnd, List::Search),
                    SearchBoxEnter::Enter => self.controls().search.enter_first_or_focused(self),
                    SearchBoxEnter::Wait => {}
                }
            }
            KeyAction::ResultEnter => self.controls().search.enter_result(self),
            KeyAction::ClearSearch => controls.search.row.input.hwnd().set_text(""),
            KeyAction::MoveTab { backward } => self.move_tab_focus(hwnd, target, backward),
            KeyAction::CancelJob => {
                if self.model.borrow().job() != Job::Operation {
                    self.run(hwnd, MenuItem::Run(CommandId::Cancel));
                }
            }
            KeyAction::ChooseCustom => {
                if let Some(range) = controls.filters.range_of(target) {
                    self.controls().filters.choose_custom(self, hwnd, range);
                }
            }
            KeyAction::Panel(key) => {
                return self.controls().panels.on_panel_key(self, hwnd, target, key);
            }
            KeyAction::Select(key) => {
                let list = match self.controls().panels.panel_list(target) {
                    Some(side) => List::Panel(side),
                    None => List::Search,
                };
                self.select_key(hwnd, list, key);
            }
        }
        true
    }

    /// Moves the focus from `target` as `Tab`, or `Shift+Tab` when
    /// `backward`, does ([`commands::tab_stop`]), and shows the focus
    /// rectangles, which the window takes from `IsDialogMessageW` with the
    /// key. Entering the search box selects its text, as `Tab` into a
    /// dialog's edit box does.
    fn move_tab_focus(&self, hwnd: Hwnd, target: Hwnd, backward: bool) {
        hwnd.show_focus_cues();
        if self.model.borrow().mode() == Mode::Commander {
            // From a control outside the panels' lists and boxes, such as
            // a drive button, Tab returns to the active panel, because
            // Search's controls are hidden.
            let active = self.model.borrow().panels().active();
            self.controls().panels.activate_panel(self, active, true);
            return;
        }
        let controls = self.controls();
        let from = if target == controls.search.row.input.hwnd() {
            Some(TabStop::SearchBox)
        } else if target == controls.search.list.hwnd() {
            Some(TabStop::Results)
        } else {
            None
        };
        match commands::tab_stop(from, backward) {
            TabStop::SearchBox => {
                controls.search.row.input.hwnd().focus();
                controls.search.row.input.select_all();
            }
            TabStop::Results => controls.search.list.hwnd().focus(),
        }
    }

    /// Whether `point`, in client coordinates, is on the splitter above the
    /// inspector.
    fn on_splitter(&self, point: (i32, i32)) -> bool {
        self.created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::inspector)
            .is_some_and(|inspector| inspector.splitter.contains(point))
    }

    /// Whether `point`, in screen coordinates, is on the list's header.
    fn on_header(&self, point: (i32, i32)) -> bool {
        // A hidden list keeps its header's rectangle, which a panel covers
        // in Commander mode.
        let list = self.controls().search.list;
        list.hwnd().is_visible() && list.header().screen_rect().contains(point)
    }

    fn selected_source(&self, list: List) -> Option<Entries> {
        let model = self.model.borrow();
        match list {
            List::Panel(side) => model.panels().chosen_source(side),
            List::Search => model
                .search_list()
                .selection()
                .shared_files()
                .map(Entries::Results),
        }
    }

    /// Opens `entries` of `list` where they go (DESIGN.md: Searching,
    /// "where Open goes").
    pub(super) fn open_chosen(&self, list: List, entries: Entries) {
        // The row menu's bold Open does what a double-click does, as
        // Windows' guidelines ask, so one folder shows in its panel.
        if let List::Panel(side) = list
            && let Some(folder) = entries.one().filter(|entry| entry.is_folder)
        {
            let folder = String::from_utf16_lossy(&folder.path);
            self.controls().panels.show_folder(self, side, folder);
            return;
        }
        match self.open_commands(list).open_route(list, &entries) {
            OpenRoute::Run => self.open_entries(list, EntryAction::Open, entries),
            OpenRoute::Commander { entry, commanded } => {
                if !commanded.is_empty() {
                    let commanded = Entries::Ready(commanded);
                    self.open_entries(list, EntryAction::Open, commanded);
                }
                self.show_in_commander(list, entry);
            }
        }
    }

    fn act_on_selected(&self, list: List, action: EntryAction) {
        let Some(entries) = self.selected_source(list) else {
            return;
        };
        self.open_entries(list, action, entries);
    }

    /// Shows `entry` in Commander once its folder opens (DESIGN.md:
    /// Searching, "where Open goes").
    fn show_in_commander(&self, list: List, entry: Selected) {
        self.created()
            .work
            .borrow_mut()
            .shell
            .show_in_commander(list, entry);
    }

    /// The open commands of Settings > Context menu that `list` runs.
    fn open_commands(&self, list: List) -> Commands {
        let model = self.model.borrow();
        let choices = model.choices();
        Commands {
            folders: choices.open_folders.clone(),
            files: choices.open_files.clone(),
            location: choices.open_file_location.clone(),
        }
        .for_list(list)
    }

    fn open_entries(&self, list: List, action: EntryAction, entries: Entries) {
        let commands = self.open_commands(list);
        self.created()
            .work
            .borrow_mut()
            .shell
            .open_entries(list, action, entries, commands);
    }

    /// Drags the entries that `list`'s file commands act on out of the
    /// window, as File Explorer drags its own, once they are read
    /// (DESIGN.md: Dragging).
    fn drag_selected(&self, hwnd: Hwnd, list: List) {
        if !self.read_target_selection(hwnd, list) {
            return;
        }
        let selected = self.model.borrow().chosen_count(list);
        if selected > DRAG_LIMIT {
            // Commander mode has no Edit > Copy, so a panel points to F5.
            let key = match list {
                List::Search => "message.drag_limit",
                List::Panel(_) => "message.drag_limit_panel",
            };
            let text = {
                let numbers = self.numbers.borrow();
                let count = |n: usize| numbers.integer(n as u64);
                (self.model.borrow().strings()).format(key, &[&count(DRAG_LIMIT), &count(selected)])
            };
            self.error_box(hwnd, &text);
            return;
        }
        if selected == 0 {
            return;
        }
        // The drag runs its own message loop, so it gets its own copy of the
        // entries and no borrow is held (DESIGN.md: window rule 1).
        let Some(entries) = self.selected_source(list) else {
            return;
        };
        let missing = shell::drag(&entries.dragged());
        self.report_missing(hwnd, missing);
    }

    /// Redraws the rows that a drop's highlight left or reached.
    fn redraw_drop_rows(&self, rows: impl IntoIterator<Item = u64>) {
        let list = self.controls().search.list;
        for row in rows {
            list.redraw_rows(row..row + 1);
        }
    }

    /// Takes away what a drag that left or was dropped showed: the row's
    /// highlight and the scroll timer.
    fn end_drag_in(&self, drag: Option<&DragIn>) {
        let Some(drag) = drag else { return };
        self.redraw_drop_rows(drag.target.as_ref().map(|target| target.row));
        if drag.scrolling.is_some() {
            let window = self.controls().search.list.hwnd().parent();
            window.kill_timer(Timer::DropScroll.id());
        }
    }

    /// Scrolls the list a row while dragged files rest at its edge, and
    /// moves the highlight to the folder row the scroll brings under the
    /// pointer.
    fn drop_scroll(&self, hwnd: Hwnd) {
        // A tick posted before the timer stopped, such as during a drop,
        // finds no scrolling and changes nothing.
        let state = self
            .drag_in
            .borrow()
            .as_ref()
            .and_then(|drag| Some((drag.scrolling?, drag.over?)));
        let Some((edge, (point, keys, allowed))) = state else {
            hwnd.kill_timer(Timer::DropScroll.id());
            return;
        };
        // A source that ended without a drop sends no DragLeave.
        if !win::window::mouse_button_down() {
            self.drag_leave();
            return;
        }
        self.controls().search.list.scroll_row(edge);
        self.drag_over(point, keys, allowed);
    }

    /// Says that `missing`, which the person chose, no longer exists
    /// (DESIGN.md: Feedback).
    fn report_missing(&self, hwnd: Hwnd, missing: Option<Vec<u16>>) {
        let Some(path) = missing else { return };
        let path = String::from_utf16_lossy(&path);
        let text = self
            .model
            .borrow()
            .strings()
            .format("message.no_longer_exists", &[&path]);
        self.error_box(hwnd, &text);
    }

    /// Puts the selected files on the clipboard in `format` (DESIGN.md: C8, C9,
    /// C10). The block holds this command's selection through background retries.
    fn copy(&self, hwnd: Hwnd, format: ClipboardFormat) {
        let files = {
            let model = self.model.borrow();
            model.search_list().selection().shared_files()
        };
        let Some(files) = files else { return };
        hwnd.kill_timer(Timer::Clipboard.id());
        self.created()
            .work
            .borrow_mut()
            .clipboard
            .copy(files, format);
    }

    fn cancel_copy(&self, hwnd: Hwnd) {
        hwnd.kill_timer(Timer::Clipboard.id());
        self.created().work.borrow_mut().clipboard.cancel();
    }

    /// `F3`: opens a Viewer of the active panel's focused file
    /// (DESIGN.md: The Viewer).
    fn view_focused(&self, hwnd: Hwnd, side: Side) {
        let (path, strings) = {
            let model = self.model.borrow();
            let panels = model.panels();
            let Some(entry) = panels.focused(side).filter(|entry| !entry.is_folder) else {
                return;
            };
            (entry.path, model.strings().clone())
        };
        if let Err(error) = self.viewers.open(path, strings, hwnd) {
            self.log_error(format!("The Viewer did not open: {error}"));
        }
    }

    /// Admission stays disabled until the complete recovery read is adopted.
    fn open_file_operations(&self, hwnd: Hwnd) {
        let saved = self.settings_file.with_file_name("operations.json");
        self.created().work.borrow_mut().queue.restore(saved);
        clipboard::listen(hwnd);
        self.model
            .borrow_mut()
            .set_clipboard_holds_files(clipboard::holds_files());
    }

    fn adopt_queue(&self, hwnd: Hwnd, restored: QueueReady) {
        let strings = self.model.borrow().strings().clone();
        let queue = self.created().work.borrow_mut().queue.adopt(restored);
        let operations = FileOperations::new(hwnd, queue, strings);
        operations.set_transfer_limit(self.model.borrow().choices().transfers_per_device);
        let _ = self.file_operations.set(operations);
        self.controls().key_bar.update_key_bar(self);
        // Enrichment may have woken before adoption; explicitly read the restored state.
        hwnd.post(QUEUE_CHANGED);
    }

    /// Prepares `request` in the queue, with the speed settings; `None` in
    /// a window that owns no file operations.
    fn start_operation(
        &self,
        request: Request,
        source: Option<FilesSource>,
    ) -> Option<OperationId> {
        let operations = self.file_operations.get()?;
        let settings = self.model.borrow().choices().speed.settings();
        let kind = source.as_ref().map(FilesSource::kind);
        let id = operations.start(request, settings, source);
        self.model.borrow_mut().file_operation_started(id, kind);
        Some(id)
    }

    fn schedule_operations(&self, hwnd: Hwnd) {
        if self.shutdown.borrow().is_none() && !self.operation_timer.replace(true) {
            hwnd.set_timer(Timer::OperationDelivery.id(), 1);
        }
    }

    /// Reads the queue after it changed, and runs what the model decides:
    /// the queue's steps first, then the lists', the dialogs, which wait
    /// for [`Message::Deferred`], the transfer pane's, and the
    /// sources' last, because a drop's source can deliver messages while
    /// it is told.
    fn on_operations(&self, hwnd: Hwnd) {
        self.continue_operation_steps(hwnd);
        if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
            return;
        }
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        if self.model.borrow().can_receive_operations() {
            for reply in operations.update() {
                match reply {
                    super::work::QueueAccepted::Resolved(id, accepted) => {
                        let source = operations.resolved(id, accepted);
                        if accepted {
                            self.model.borrow_mut().file_operation_started(id, source);
                        }
                    }
                    super::work::QueueAccepted::Submitted(id, true) => {
                        self.model.borrow_mut().operation_submitted(id);
                    }
                    super::work::QueueAccepted::Submitted(_, false)
                    | super::work::QueueAccepted::Moved(_, _) => {}
                }
            }
            let taken = operations.taken();
            self.model.borrow_mut().observe_operations(
                operations.snapshot(),
                Arc::clone(&taken),
                Instant::now(),
            );
            if !self.model.borrow().shows_taken(&taken) {
                self.show_taken(hwnd, taken);
            }
        }
        self.created()
            .work
            .borrow_mut()
            .retire(operations.take_retired());
        use super::file_operations::PaneAction;
        for action in operations.take_actions() {
            match action {
                PaneAction::Select(detail) => self.model.borrow_mut().select_operation(detail),
                PaneAction::Command(id, QueueAction::Report) => self
                    .model
                    .borrow_mut()
                    .select_operation(crate::core::file_operations::Detail::Report(id)),
                PaneAction::Command(id, QueueAction::Clear) => {
                    self.model.borrow_mut().operation_reported(id);
                }
                PaneAction::Command(id, QueueAction::Continue)
                    if operations.operation(id).is_some_and(|operation| {
                        matches!(*operation.request, Request::Synchronize { .. })
                    }) =>
                {
                    self.defer(hwnd, ModalStep::Synchronize(id));
                }
                PaneAction::Command(id, action) => self.act_operation(id, action),
                PaneAction::Move(id, before) => operations.move_before(id, before),
                PaneAction::CancelAll => {
                    self.dropping.borrow_mut().clear();
                    self.cancel_paste(None);
                    operations.cancel_all();
                }
                PaneAction::CloseReport => self.model.borrow_mut().close_operation_report(),
                PaneAction::Log(text) => self.log_error(text),
                PaneAction::Collapse | PaneAction::Maximize | PaneAction::Flip => {
                    let accepted = {
                        let mut model = self.model.borrow_mut();
                        match action {
                            PaneAction::Collapse => model.collapse_inspector(),
                            PaneAction::Maximize => model.maximize_inspector(),
                            PaneAction::Flip => model.flip_transfers(),
                            _ => unreachable!(),
                        }
                    };
                    self.apply(hwnd, accepted);
                }
            }
        }
        let released = self.model.borrow_mut().next_operation_release();
        self.run_queue_steps(released);
        self.retire_model();
        self.start_list_passes();
        if self.model.borrow().can_release_operations() {
            self.schedule_operations(hwnd);
        }
    }

    fn apply_operation_steps(&self, hwnd: Hwnd, steps: OperationSteps) {
        assert!(
            self.operation_delivery.borrow().is_none(),
            "the previous receipt finishes delivery before the next pass"
        );
        self.operation_delivery
            .replace(Some(OperationDelivery::new(steps)));
        self.retire_model();
        for list in List::ALL {
            let pending = self
                .model
                .borrow()
                .list(list)
                .is_some_and(|shown| matches!(shown.selection(), Selection::Filtering { .. }));
            if pending {
                self.apply(
                    hwnd,
                    Accepted {
                        touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                        effects: vec![Effect::InspectAge, Effect::ListPasses],
                    },
                );
            }
        }
        self.schedule_operations(hwnd);
    }

    fn continue_operation_steps(&self, hwnd: Hwnd) {
        if self.delivering_operations.replace(true) {
            return;
        }
        for _ in 0..crate::core::work::INLINE_ITEMS {
            let next = self
                .operation_delivery
                .borrow_mut()
                .as_mut()
                .and_then(OperationDelivery::next);
            let Some(next) = next else {
                break;
            };
            match next {
                OperationEffect::Queue(step) => {
                    if let Some(operations) = self.file_operations.get() {
                        operations.run(step);
                    }
                }
                OperationEffect::Panel(step) => {
                    self.controls()
                        .panels
                        .follow_operations(self, hwnd, &[step])
                }
                OperationEffect::Dialog(dialog) => self.defer(hwnd, ModalStep::Operation(dialog)),
                OperationEffect::Progress(progress) => {
                    self.refresh_operations();
                    self.show_progress(hwnd, progress);
                }
                OperationEffect::Source(step) => {
                    if let Some(operations) = self.file_operations.get() {
                        operations.tell_source(step);
                    }
                }
            }
            if self.shutdown.borrow().is_some() {
                break;
            }
        }
        self.delivering_operations.set(false);
        if self.shutdown.borrow().is_some() {
            return;
        }
        let finished = self
            .operation_delivery
            .borrow()
            .as_ref()
            .is_some_and(OperationDelivery::is_empty);
        if finished {
            let delivery = self.operation_delivery.take();
            let mut retired = Retirement::default();
            retired.push(delivery, crate::core::work::INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            self.start_list_passes();
            hwnd.post(DEFERRED);
        }
        if finished || self.operation_delivery.borrow().is_some() {
            self.schedule_operations(hwnd);
        }
    }

    /// Runs `steps` on the queue. A removal wakes the window once the queue
    /// has taken the removed operation's sources out of the taken entries.
    fn run_queue_steps(&self, steps: Vec<QueueStep>) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        for step in steps {
            operations.run(step);
        }
    }

    fn act_operation(&self, id: OperationId, action: QueueAction) {
        if action == QueueAction::Cancel {
            self.abandon_drop(id);
            self.cancel_paste(Some(id));
        }
        if let Some(operations) = self.file_operations.get() {
            operations.act(id, action);
        }
    }

    /// Lets the queue remove the ended deletes and moves that no list keeps
    /// any more, after a list let go of them.
    fn release_operations(&self) {
        let steps = self.model.borrow_mut().release_operations();
        self.run_queue_steps(steps);
        self.start_list_passes();
        if self.model.borrow().can_release_operations() {
            self.schedule_operations(self.controls().search.list.hwnd().parent());
        }
    }

    fn start_list_passes(&self) {
        self.gate_changed_selections();
        let input =
            if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
                None
            } else {
                self.model.borrow_mut().take_following()
            };
        if let Some(input) = input {
            self.created()
                .work
                .borrow_mut()
                .operations
                .follow(input, self.numbers.borrow().clone());
        }
        for side in Side::BOTH {
            let input = self.model.borrow_mut().panels_mut().take_calculation(side);
            if let Some(input) = input {
                self.created()
                    .work
                    .borrow_mut()
                    .panels
                    .calculate(side, input);
            }
            let input = self.model.borrow_mut().panels_mut().take_count(side);
            if let Some(input) = input {
                self.created().work.borrow_mut().panels.count(side, input);
            }
            let input = self.model.borrow_mut().panels_mut().take_quick(side);
            if let Some(input) = input {
                self.created().work.borrow_mut().panels.find(side, input);
            }
        }
        for list in List::ALL {
            let keep = self.model.borrow_mut().take_keep(list);
            if let Some(input) = keep {
                self.created().work.borrow_mut().lists.keep(list, input);
            }
            let filter = self.model.borrow_mut().take_selection_filter(list);
            if let Some(input) = filter {
                self.created().work.borrow_mut().lists.filter(list, input);
            }
        }
    }

    /// Opens File operations explicitly, with `select` selected.
    fn show_operations(&self, select: Option<OperationId>) {
        self.controls().inspector.remember_inspector_focus(self);
        let accepted = {
            let mut model = self.model.borrow_mut();
            if self.file_operations.get().is_some() {
                model.open_transfer_selection(select);
            }
            model.show_transfers()
        };
        self.apply(self.controls().search.row.input.hwnd().parent(), accepted);
        if let Some(operations) = self.file_operations.get() {
            let view = self.model.borrow().operation_view();
            if let Some(view) = view {
                operations.show(view);
            }
            self.created()
                .work
                .borrow_mut()
                .retire(operations.take_retired());
        }
    }

    /// Refreshes the transfer pane without opening it.
    fn refresh_operations(&self) {
        self.start_list_passes();
        if let Some(operations) = self.file_operations.get() {
            let view = self.model.borrow().operation_view();
            if let Some(view) = view {
                operations.refresh(view);
            }
            self.created()
                .work
                .borrow_mut()
                .retire(operations.take_retired());
        }
    }

    /// The status bar's operation, the taskbar button and the poll of a
    /// running operation's counts follow `progress`.
    fn show_progress(&self, hwnd: Hwnd, progress: OperationProgress) {
        if progress.status_changed {
            self.update_status();
        }
        if let Some(operations) = self.file_operations.get() {
            operations.show_taskbar(progress.taskbar);
        }
        if progress.polls {
            hwnd.set_timer(Timer::OperationsPoll.id(), OPERATIONS_POLL_MS);
        } else {
            hwnd.kill_timer(Timer::OperationsPoll.id());
        }
    }

    /// Reads a running operation's counts, which change without a wake.
    fn poll_operations(&self, _hwnd: Hwnd) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        operations.request_refresh();
        self.refresh_operations();
    }

    /// Shows the retained failure without holding model borrows across its message loop.
    fn operation_dialog(&self, _hwnd: Hwnd, dialog: OperationDialog) -> bool {
        match dialog {
            OperationDialog::Report(id) => {
                let transfer = self.model.borrow().operation_is_transfer(id);
                if transfer {
                    self.show_operations(Some(id));
                    self.model
                        .borrow_mut()
                        .select_operation(crate::core::file_operations::Detail::Report(id));
                    self.refresh_operations();
                    return false;
                }
                let (operation, strings) = {
                    let model = self.model.borrow();
                    (model.operation_report(id), model.strings().clone())
                };
                let Some(operation) = operation else {
                    return false;
                };
                self.created().work.borrow_mut().operations.prepare_report(
                    Reporting { operation, strings },
                    self.numbers.borrow().clone(),
                );
            }
        }
        true
    }

    fn show_prepared_failure(&self, hwnd: Hwnd, mut prepared: PreparedFailure) {
        let current = self
            .model
            .borrow()
            .operation_outcome(prepared.id)
            .is_some_and(|outcome| Arc::ptr_eq(&outcome, &prepared.outcome));
        if current {
            if let Some(failure) = prepared.failure.take() {
                prepared.failure = Some(super::file_operations::show_failure(
                    hwnd,
                    &prepared.strings,
                    failure,
                ));
            }
            if self.shutdown.borrow().is_none() {
                self.model.borrow_mut().operation_reported(prepared.id);
                self.start_list_passes();
                hwnd.post(QUEUE_CHANGED);
            }
        }
        let mut retired = Retirement::default();
        retired.push(prepared, crate::core::work::INLINE_ITEMS + 1);
        self.created().work.borrow_mut().retire(retired);
    }

    fn transfer_request(
        sources: Vec<PathBuf>,
        destination: PathBuf,
        moves: bool,
        policy: gozler_fileops::Policy,
    ) -> Request {
        let destination = Destination::Folder(destination);
        if moves {
            Request::Move {
                policy,
                sources,
                destination,
            }
        } else {
            Request::Copy {
                policy,
                sources,
                destination,
            }
        }
    }

    fn accept_transfer(&self, destination: PathBuf, moves: bool) -> Option<OperationId> {
        let operations = self.file_operations.get()?;
        let settings = self.model.borrow().choices().speed.settings();
        Some(operations.accept(
            Self::transfer_request(
                Vec::new(),
                destination,
                moves,
                self.model.borrow().choices().copy_policy,
            ),
            settings,
        ))
    }

    fn resolve_transfer(
        &self,
        id: OperationId,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        moves: bool,
        from: Option<FilesSource>,
    ) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        if sources.is_empty() {
            operations.fail_intake(id);
            return;
        }
        let policy = self.model.borrow().choices().copy_policy;
        operations.resolve(
            id,
            Self::transfer_request(sources, destination, moves, policy),
            from,
        );
    }

    fn remember_copy_policy(&self, hwnd: Hwnd, policy: gozler_fileops::Policy) {
        self.model.borrow_mut().set_copy_policy(policy);
        self.note_saved_state(hwnd);
    }

    fn prepare_selected(&self, list: List, action: PathAction) {
        let Some(entries) = self.selected_source(list) else {
            return;
        };
        self.created()
            .work
            .borrow_mut()
            .operations
            .prepare_paths(list, action, entries);
    }

    fn retire_paths(&self, paths: Vec<PathBuf>) {
        let items = paths.len();
        let mut retired = Retirement::default();
        retired.push(paths, items);
        self.created().work.borrow_mut().retire(retired);
    }

    fn ask_transfer(
        &self,
        hwnd: Hwnd,
        start: TransferStart,
        moves: bool,
    ) -> Option<(Destination, TransferAction)> {
        let (dialog, captions) = {
            let model = self.model.borrow();
            let strings = model.strings();
            let mut captions = DialogCaptions::new(DialogId::Transfer, strings);
            captions.title = start.title.clone();
            if moves {
                let ok = captions
                    .items
                    .iter_mut()
                    .find(|(id, _)| *id == DialogItem::Ok.id());
                if let Some((_, text)) = ok {
                    *text = strings.text("file_operations.move").to_owned();
                }
            }
            (Transfer::new(strings, start), captions)
        };
        win::dialog::run_modal(hwnd, DialogId::Transfer, &captions, &dialog)
            .ok()
            .and_then(|_| dialog.chosen())
    }

    fn copy_to_folder(&self, _hwnd: Hwnd, moves: bool, list: List) {
        self.prepare_selected(list, PathAction::Transfer { moves });
    }

    fn transfer_paths(
        &self,
        hwnd: Hwnd,
        moves: bool,
        place: panel::CommandPlace,
        selected: bool,
        start: TransferStart,
        sources: Vec<PathBuf>,
    ) {
        if sources.is_empty() {
            return;
        }
        let side = place.side;
        let folder = start.folder.clone();
        let one = start.one.clone();
        let Some((destination, action)) = self.ask_transfer(hwnd, start, moves) else {
            self.retire_paths(sources);
            return;
        };
        let renamed = match (&destination, one) {
            (Destination::Typed(path), Some(from)) if moves => {
                let path = path.to_string_lossy().into_owned();
                (panel::same_folder(&gozler_core::winpath::parent_of(&path), &folder))
                    .then(|| (panel::last_name(&path).to_owned(), from))
            }
            _ => None,
        };
        let id = self.start_transfer(hwnd, moves, sources, destination, action);
        let follows = {
            let model = self.model.borrow();
            model.mode() == Mode::Commander && model.panels().place_is_current(place)
        };
        if !follows {
            return;
        }
        // Submitting clears the selection (DESIGN.md: Copy and Move); Gozler's
        // own rename selects the renamed entry again.
        self.panel(side).list.deselect_all();
        if let (Some((name, from)), Some(id)) = (renamed, id) {
            self.model
                .borrow_mut()
                .panels_mut()
                .await_name(side, &name, Some(&from), selected, id);
        }
    }

    fn start_transfer(
        &self,
        hwnd: Hwnd,
        moves: bool,
        mut sources: Vec<PathBuf>,
        destination: Destination,
        action: TransferAction,
    ) -> Option<OperationId> {
        let policy = match action {
            TransferAction::Transfer(policy) => policy,
            TransferAction::Synchronize { mirror } => {
                let operations = self.file_operations.get()?;
                let source = sources.pop().expect("Sync and Mirror require one folder");
                let settings = self.model.borrow().choices().speed.settings();
                let id = operations.synchronize_to(source, destination, mirror, settings);
                self.model.borrow_mut().file_operation_started(id, None);
                return Some(id);
            }
        };
        let request = match moves {
            true => Request::Move {
                policy,
                sources,
                destination,
            },
            false => Request::Copy {
                policy,
                sources,
                destination,
            },
        };
        self.remember_copy_policy(hwnd, policy);
        self.start_operation(request, None)
    }

    fn copy_paths_to_folder(
        &self,
        hwnd: Hwnd,
        moves: bool,
        sources: Vec<PathBuf>,
        single_folder: bool,
    ) {
        let Some(first) = sources.first() else {
            return;
        };
        let start = {
            let model = self.model.borrow();
            let strings = model.strings();
            let (title, name_line, items_line) = if moves {
                (
                    "file_operations.move_to_title",
                    "transfer.move_name",
                    "transfer.move_items",
                )
            } else {
                (
                    "file_operations.copy_to_title",
                    "transfer.copy_name",
                    "transfer.copy_items",
                )
            };
            let one = (sources.len() == 1).then(|| {
                first
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let line = match &one {
                Some(name) => strings.format(name_line, &[name]),
                None => strings.count(
                    items_line,
                    sources.len() as u64,
                    &*self.numbers.borrow(),
                    &[],
                ),
            };
            let folder = first
                .parent()
                .unwrap_or(first)
                .to_string_lossy()
                .into_owned();
            let text = crate::core::panel::join(model.destination_folder().unwrap_or(&folder), "");
            TransferStart {
                policy: model.choices().copy_policy,
                mode: if moves {
                    TransferMode::Move
                } else {
                    TransferMode::Copy { single_folder }
                },
                title: strings.text(title).to_owned(),
                line,
                selected: (0, text.encode_utf16().count()),
                other: text.clone(),
                text,
                folder,
                one,
            }
        };
        let Some((destination, action)) = self.ask_transfer(hwnd, start, moves) else {
            self.retire_paths(sources);
            return;
        };
        if let Destination::Folder(folder) = &destination {
            let accepted = self
                .model
                .borrow_mut()
                .set_destination_folder(folder.to_string_lossy().into_owned());
            self.apply(hwnd, accepted);
        }
        self.start_transfer(hwnd, moves, sources, destination, action);
    }

    /// Delete, and the list's `Delete` and `Shift+Delete`: the Delete
    /// question, then the permanent delete (DESIGN.md: File operations).
    fn delete_selected(&self, _hwnd: Hwnd, list: List) {
        let place = {
            let model = self.model.borrow();
            match list {
                List::Panel(side) => Some(model.panels().command_place(side)),
                List::Search => None,
            }
        };
        self.prepare_selected(list, PathAction::Delete { place });
    }

    fn delete_paths(
        &self,
        hwnd: Hwnd,
        place: Option<crate::core::panel::CommandPlace>,
        sources: Vec<PathBuf>,
    ) {
        if sources.is_empty() {
            return;
        }
        let question = {
            let model = self.model.borrow();
            delete_question(model.strings(), &*self.numbers.borrow(), &sources)
        };
        // The question runs its own message loop, so nothing is borrowed
        // here.
        if win::dialog::ask(hwnd, &question) {
            self.start_operation(Request::Delete { sources }, None);
            let follows = place.is_some_and(|place| {
                let model = self.model.borrow();
                model.mode() == Mode::Commander
                    && model.panels().active() == place.side
                    && model.panels().place_is_current(place)
            });
            if follows {
                self.controls().panels.focus_after_chosen(self);
            }
        } else {
            self.retire_paths(sources);
        }
    }

    /// Paste on one selected folder: copies the files on the clipboard into
    /// it, or moves them when they were cut (DESIGN.md: Copy), once a
    /// thread has read them.
    fn paste(&self) {
        let Some(entry) = self
            .selected_source(List::Search)
            .and_then(|entries| entries.one())
        else {
            return;
        };
        let destination = PathBuf::from(OsString::from_wide(&entry.path));
        let sequence = clipboard::sequence();
        let Some(id) = self.accept_transfer(destination.clone(), false) else {
            return;
        };
        self.pasting.borrow_mut().insert(id, destination);
        self.created().work.borrow_mut().paste.read(id, sequence);
    }

    /// Paste's files, once their thread has read them from the clipboard.
    fn finish_paste(&self) {
        let answer = {
            let mut work = self.created().work.borrow_mut();
            let answer = work.paste.take();
            let retired = work.paste.take_retired();
            work.retire(retired);
            answer
        };
        let Some((id, mut files)) = answer else {
            return;
        };
        let destination = self.pasting.borrow_mut().remove(&id);
        let Some(destination) = destination else {
            if let Some(files) = &mut files {
                files.release_source();
            }
            let mut retired = Retirement::default();
            retired.push(files, crate::core::work::INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            return;
        };
        let (sources, moves) = files.as_mut().map_or_else(
            || (Vec::new(), false),
            |files| (files.take_paths(), files.cut),
        );
        self.resolve_transfer(
            id,
            sources,
            destination,
            moves,
            files.map(FilesSource::Paste),
        );
    }

    fn cancel_paste(&self, id: Option<OperationId>) {
        let mut work = self.created().work.borrow_mut();
        match id {
            Some(id) => {
                self.pasting.borrow_mut().remove(&id);
                work.paste.cancel(id);
            }
            None => {
                let mut retired = Retirement::default();
                retired.push(self.pasting.take(), crate::core::work::INLINE_ITEMS + 1);
                work.retire(retired);
                work.paste.cancel_all();
            }
        }
        let retired = work.paste.take_retired();
        work.retire(retired);
    }

    /// Shows the rows that deletes and moves have taken, `taken`, struck
    /// through in Search's list and blank in a search panel, and takes them
    /// out of the selections (DESIGN.md: Taken entries).
    fn show_taken(&self, hwnd: Hwnd, taken: Arc<Taken>) {
        let replaced = Arc::clone(self.model.borrow().panels().taken());
        let shown = self.controls().panels.show_panels_taken(self, taken);
        self.created().work.borrow_mut().retire_taken(replaced);
        if shown.search_lost {
            let lost = Accepted {
                touched: Areas::SEARCH | Areas::INSPECTOR | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::ListPasses],
            };
            self.apply(hwnd, lost);
        }
        self.show_list_taken(List::Search, shown.search_deselect);
        self.start_list_passes();
    }

    /// Shows `list`'s taken rows blank, and unhighlights the rows that
    /// `deselect` says, because the list would still draw a taken row it
    /// selects highlighted.
    fn show_list_taken(&self, list: List, deselect: Deselect) {
        let control = self.list_control(list);
        match deselect {
            Deselect::Nothing => {}
            Deselect::All => control.deselect_all(),
            Deselect::Taken => self.request_taken_deselection(list),
        }
        control.redraw_rows(control.visible_rows());
    }

    /// Asks where to export the shown search, and starts the export (DESIGN.md:
    /// Export).
    fn export(&self, hwnd: Hwnd) {
        let (title, file_name, names) = {
            let model = self.model.borrow();
            let strings = model.strings();
            (
                strings.text("export.title").to_owned(),
                strings.text("export.file_name").to_owned(),
                ExportFormat::ALL
                    .map(|f| format!("{} (*.{})", strings.text(f.label()), f.extension())),
            )
        };
        let types: Vec<FileType> = ExportFormat::ALL
            .iter()
            .zip(&names)
            .map(|(format, name)| FileType {
                name,
                extension: format.extension(),
            })
            .collect();
        let Some((path, index)) = shell::save_dialog(hwnd, &title, &file_name, &types) else {
            return;
        };
        let chosen = ExportFormat::ALL
            .get(index)
            .copied()
            .unwrap_or(ExportFormat::Csv);
        let columns = self.model.borrow().columns().len();
        let order = self.controls().search.list.column_order(columns);
        let format = ExportFormat::of(&path, chosen);
        let accepted = self.model.borrow_mut().start_export(path, format, &order);
        self.apply(hwnd, accepted);
    }

    /// The message box for an export that failed (DESIGN.md: T9).
    fn show_export_failure(&self, hwnd: Hwnd, path: &Path, error: &ExportError) {
        let text = {
            let model = self.model.borrow();
            let strings = model.strings();
            let path = path.display().to_string();
            match error {
                ExportError::Write(failure) => {
                    strings.format("message.export_failed", &[&path, &windows_reason(*failure)])
                }
                ExportError::Daemon(failure) => strings.format(
                    "message.export_stopped",
                    &[&path, &captions::failure_text(strings, failure)],
                ),
            }
        };
        self.error_box(hwnd, &text);
    }

    fn error_box(&self, hwnd: Hwnd, text: &str) {
        let title = self
            .model
            .borrow()
            .strings()
            .text("window.title")
            .to_owned();
        win::dialog::error_box(Some(hwnd), text, &title);
    }

    fn show_mode(&self, hwnd: Hwnd, mode: Mode) {
        if hwnd.is_enabled() {
            let accepted = self.model.borrow_mut().show_mode(mode);
            self.apply(hwnd, accepted);
        }
        self.show_window(hwnd);
    }

    /// Brings the window forward with the search box focused and its text
    /// selected, so the person types the next search at once (DESIGN.md: Window
    /// lifetime, C24), or in Commander mode with the active panel's list focused
    /// (DESIGN.md: Commander mode's window). While a dialog is open, the dialog
    /// keeps the focus.
    fn show_window(&self, hwnd: Hwnd) {
        // The columns fill when the window shows (DESIGN.md: Results list),
        // and a hidden window shows at the width it had, which no resize
        // fills. Filling before it shows draws it once.
        if !hwnd.is_visible() {
            let width = self.controls().search.fill_width();
            let widths = (self.controls().search.column_sizing.borrow_mut())
                .window_shown(width, || self.controls().search.column_fits(self));
            self.controls().search.set_column_widths(widths);
        }
        if self.tray.unshown.replace(false) {
            let placement = self.model.borrow().placement();
            win::window::show(hwnd, placement);
        }
        hwnd.bring_to_front();
        self.follow_visibility(hwnd);
        if !hwnd.is_enabled() {
            return;
        }
        self.home().focus();
        let mode = self.model.borrow().mode();
        if mode == Mode::Search {
            self.controls().search.row.input.select_all();
        }
    }

    /// The control that the shown mode focuses first: the search box, or the
    /// active panel's list.
    fn home(&self) -> Hwnd {
        let controls = self.controls();
        let (mode, active) = {
            let model = self.model.borrow();
            (model.mode(), model.panels().active())
        };
        match mode {
            Mode::Search => controls.search.row.input.hwnd(),
            Mode::Commander => self.panel(active).list.hwnd(),
        }
    }

    /// Closing hides the window while Gozler keeps running in the
    /// notification area, the first time with a notification that says so,
    /// and exits otherwise (DESIGN.md: Window lifetime).
    fn close(&self, hwnd: Hwnd) {
        if self.instance() == Instance::Another || !self.model.borrow().choices().keep_running {
            self.exit(hwnd);
            return;
        }
        hwnd.hide();
        self.follow_visibility(hwnd);
        // Gozler keeps running hidden, so this is when the window's
        // placement is saved, with the first hide's notice.
        let first_hide = self.model.borrow_mut().first_hide();
        self.save_settings(true);
        if !first_hide {
            return;
        }
        let (title, text) = {
            let model = self.model.borrow();
            let strings = model.strings();
            let text = match model.choices().show_shortcut {
                Some(shortcut) => {
                    strings.format("tray.notice_shortcut", &[&shortcut.text(strings)])
                }
                None => strings.text("tray.notice").to_owned(),
            };
            (strings.text("window.title").to_owned(), text)
        };
        if let Some(tray) = &mut self.created().work.borrow_mut().tray {
            tray.notify(&title, &text);
        }
    }

    /// Saves the settings and ends Gozler (DESIGN.md: C7, T14). While file
    /// operations run or wait, it asks first, and stops them
    /// (DESIGN.md: File operations), over the window, which comes forward from
    /// the notification area or another start's request. Returns whether Gozler
    /// ends; `false` when the person kept the operations.
    fn exit(&self, hwnd: Hwnd) -> bool {
        let counts = self
            .file_operations
            .get()
            .map_or_else(Default::default, FileOperations::exit_counts);
        let question = {
            let model = self.model.borrow();
            exit_question(model.strings(), &*self.numbers.borrow(), counts)
        };
        if let Some(question) = question {
            self.show_window(hwnd);
            // The question runs its own message loop, so nothing is
            // borrowed here. `WM_DESTROY` stops the operations.
            if !win::dialog::ask(hwnd, &question) {
                return false;
            }
        }
        // Each Viewer leaves its size, place and Wrap long lines as it
        // closes, so the save holds them.
        self.viewers.close_all();
        self.save_settings(true);
        hwnd.destroy();
        true
    }

    /// Asks GitHub for the latest release when
    /// Check for updates is on and the last check is a day old (DESIGN.md: The
    /// General page).
    fn check_for_updates(&self) {
        let now = jiff::Timestamp::now().as_second();
        if !self.model.borrow_mut().update_check_due(now) {
            return;
        }
        self.created()
            .work
            .borrow_mut()
            .check_release(ReleasePurpose::Update);
    }

    /// Asks GitHub whether its latest release is
    /// older than the window, which the fix of an older service says
    /// (DESIGN.md: Daemon failures). A reply that is not a release counts
    /// as not older.
    fn ask_latest_release(&self) {
        self.created()
            .work
            .borrow_mut()
            .check_release(ReleasePurpose::ServiceVersion);
    }

    /// A click on the status bar's right group updates Gozler while the
    /// group names a newer release (DESIGN.md: The General page).
    fn on_status_click(&self, hwnd: Hwnd) {
        let shows_update = captions::shows_update(&self.model.borrow());
        if shows_update {
            self.run_fix(hwnd, Fix::Update);
        }
    }

    /// Settings' Reset all, which the person confirmed: Gozler exits as
    /// File > Exit does, and [`super::run`] then deletes `ui.json` and
    /// opens the window again, as a new installation (DESIGN.md: Settings
    /// dialog).
    fn reset_all(&self, hwnd: Hwnd) {
        self.ending.set(Ending::ForReset);
        if !self.exit(hwnd) {
            self.ending.set(Ending::Running);
        }
    }

    /// Whether the window exited for Settings' Reset all.
    pub(super) fn resets_settings(&self) -> bool {
        self.ending.get() == Ending::ForReset
    }

    /// Saves what changed since the last save: from another window, only
    /// what its Settings and its Language menu apply. A failure is shown
    /// once per session, and not while Windows ends the session.
    fn save_settings(&self, report_failure: bool) {
        let scope = match self.instance() {
            Instance::First => SaveScope::Everything,
            Instance::Another => SaveScope::Choices,
        };
        let changes = {
            let mut model = self.model.borrow_mut();
            model.set_viewer(self.viewers.state());
            model.settings_changes(scope)
        };
        if let Some(changes) = changes {
            self.created().work.borrow_mut().settings.save(
                &self.settings_file,
                changes,
                report_failure,
            );
        }
    }

    fn on_timer(&self, hwnd: Hwnd, timer: Timer) {
        match timer {
            Timer::Search(list) => self.search(hwnd, list),
            Timer::Searching(list) => self.name_search(hwnd, list),
            Timer::Selection(list) => self.fetch_selection(hwnd, list),
            Timer::PanelSettle(side) => self.controls().panels.on_panel_settled(self, hwnd, side),
            Timer::PanelAgain(side) => {
                hwnd.kill_timer(timer.id());
                self.controls().panels.ask_panel_sizes(self, side);
            }
            Timer::Status => self.ask_status(),
            Timer::Age => {
                self.controls().inspector.show_age(self);
                self.controls().inspector.fit_inspector(self);
            }
            Timer::Dates => self.refresh_dates(),
            Timer::Clipboard => {
                hwnd.kill_timer(timer.id());
                let left = self.created().work.borrow_mut().clipboard.retry();
                if let Some(left) = left {
                    hwnd.set_timer(timer.id(), left.as_millis().max(1) as u32);
                }
            }
            Timer::DropScroll => self.drop_scroll(hwnd),
            Timer::Save => {
                hwnd.kill_timer(timer.id());
                self.save_settings(true);
            }
            Timer::Work => self.poll_work(hwnd),
            Timer::NativeSelection => self.continue_native_selections(hwnd),
            Timer::OperationDelivery => {
                hwnd.kill_timer(timer.id());
                self.operation_timer.set(false);
                if self.shutdown.borrow().is_none() && self.update_live() {
                    self.on_operations(hwnd);
                }
            }
            Timer::OperationsPoll => self.poll_operations(hwnd),
        }
    }

    fn min_window_size(&self, hwnd: Hwnd) -> Option<Size> {
        let created = self.created();
        let min_client = created.layout.borrow().as_ref()?.min_client;
        Some(win::window::window_size(
            hwnd,
            min_client,
            created.metrics.get().dpi,
        ))
    }
}

/// The width of `text` with its parts `bold` in the bold font, as
/// `CustomDraw::draw_cell` draws a matched cell.
fn matched_width(plain: &TextMeter, bold: &TextMeter, text: &str, parts: &[Range<usize>]) -> Px {
    let mut width = Px(0);
    let mut at = 0;
    for part in parts {
        width = width
            + plain.plain_width(&text[at..part.start])
            + bold.plain_width(&text[part.clone()]);
        at = part.end;
    }
    width + plain.plain_width(&text[at..])
}

/// The accelerator table of `mode`, from the command shortcuts `shortcuts`.
fn accelerator_table(
    shortcuts: &CommandShortcuts,
    mode: Mode,
) -> windows::core::Result<Accelerators> {
    let table: Vec<_> = commands::accelerators(shortcuts, mode)
        .into_iter()
        .map(|(shortcut, item)| (shortcut, item.id()))
        .collect();
    Accelerators::new(&table)
}

/// Windows' own words for why a file could not be written.
fn windows_reason(failure: WriteFailure) -> String {
    let error = match failure.os_code {
        Some(code) => std::io::Error::from_raw_os_error(code),
        None => std::io::Error::from(failure.kind),
    };
    win::error_text(&error)
}

impl WindowProc for MainWindow {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Create => Some(LRESULT(if self.on_create(hwnd) { 0 } else { -1 })),
            // Every other handler does nothing until WM_CREATE has made the
            // controls, because notifications arrive while it runs.
            _ if self.created.get().is_none() => None,
            // Before the window first shows, it has the placement it was
            // made with, not the saved one.
            Message::WindowPosChanged => {
                if hwnd.is_visible()
                    && let Some(placement) = hwnd.placement()
                {
                    self.model.borrow_mut().set_placement(placement);
                    self.note_saved_state(hwnd);
                }
                None
            }
            Message::Size => {
                // A minimized window has no room for its controls, and laying
                // them out would fill the columns into it, so restoring it
                // keeps the widths the person dragged.
                if !hwnd.is_minimized() {
                    self.layout(hwnd);
                }
                // Minimizing and restoring send it too.
                self.follow_visibility(hwnd);
                Some(LRESULT(0))
            }
            Message::EnterSizeMove => {
                self.controls()
                    .search
                    .column_sizing
                    .borrow_mut()
                    .drag_started();
                None
            }
            Message::ExitSizeMove => {
                self.controls().search.end_sizing(self);
                if self.model.borrow().mode() == Mode::Commander {
                    self.controls().panels.fill_panel_columns(self);
                }
                None
            }
            Message::DpiChanged { dpi, suggested } => {
                self.on_dpi_changed(hwnd, dpi, suggested);
                Some(LRESULT(0))
            }
            Message::GetMinMaxInfo(mut info) => {
                info.set_min_track_size(self.min_window_size(hwnd)?);
                Some(LRESULT(0))
            }
            // A window that is not a dialog keeps no focus of its own across
            // deactivation, so it gives it back to the control that had it.
            Message::Deactivate => {
                self.focus_on_activate.set(Hwnd::focused());
                None
            }
            // "Select the search text when the window is activated" is about
            // switching from another program; closing one of Gozler's own
            // dialogs gives the focus back to the control that opened it.
            Message::Activate {
                from_another_program,
            } => {
                let search = self.controls().search.row.input;
                let saved = self.focus_on_activate.take();
                let selects = {
                    let model = self.model.borrow();
                    model.mode() == Mode::Search && model.choices().select_on_activate
                };
                if from_another_program && selects {
                    search.hwnd().focus();
                    search.select_all();
                } else {
                    // A control that hid or turned off while the window was
                    // inactive, such as Cancel after its export, would keep
                    // the focus but take no key.
                    saved
                        .filter(|control| control.is_visible() && control.is_enabled())
                        .unwrap_or(self.home())
                        .focus();
                }
                if self.shortcuts.has_failures() {
                    hwnd.post(DEFERRED);
                }
                Some(LRESULT(0))
            }
            Message::Command(Command::Menu(id)) => {
                self.run(hwnd, MenuItem::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Command(Command::Control(event)) => {
                self.on_control(hwnd, event);
                None
            }
            Message::Notify(mut notification) => {
                let controls = self.controls();
                if let Some(request) = controls
                    .toolbars()
                    .find_map(|toolbar| toolbar.tip_request(&notification))
                {
                    let tip = request.id().and_then(|id| {
                        let tips = self.tool_tips.borrow();
                        tips.iter()
                            .find(|(tipped, _)| *tipped == id)
                            .map(|(_, tip)| tip.clone())
                    });
                    if let Some(tip) = tip {
                        request.set(&tip);
                    }
                    return Some(LRESULT(0));
                }
                if controls.filters.kind.selection_changed(&notification) {
                    self.controls().filters.on_kind_selected(self, hwnd);
                    return Some(LRESULT(0));
                }
                if controls.filters.location.selection_changed(&notification) {
                    self.controls().filters.on_location_selected(self, hwnd);
                    return Some(LRESULT(0));
                }
                if let Some(tip) = controls.filters.location.info_tip(&mut notification) {
                    return Some(self.controls().filters.location_tip(self, tip));
                }
                if controls.status.service_clicked(&notification) {
                    self.on_status_click(hwnd);
                    return Some(LRESULT(0));
                }
                if let Some(side) = Side::BOTH
                    .into_iter()
                    .find(|side| self.panel(*side).drive.dropped_down(&notification))
                {
                    self.controls().panels.drive_menu(self, hwnd, side);
                    return Some(LRESULT(0));
                }
                if let Some(side) = self.controls().panels.panel_list(notification.sender()) {
                    let list = self.panel(side).list.notification(notification)?;
                    return self
                        .controls()
                        .panels
                        .on_panel_notify(self, hwnd, side, list);
                }
                let list = controls.search.list.notification(notification)?;
                self.controls().search.on_list_notify(self, hwnd, list)
            }
            Message::Timer(id) => {
                self.on_timer(hwnd, Timer::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Wake => {
                self.on_wake(hwnd);
                Some(LRESULT(0))
            }
            Message::Deferred => {
                self.controls().search.read_list_selection(self, hwnd);
                self.controls().panels.read_panel_selection(self);
                self.resume_selection_actions(hwnd);
                if self.controls().search.column_dragged.replace(false) {
                    let list = self.controls().search.list;
                    let count = self.model.borrow().columns().len();
                    let order = list.column_order(count);
                    self.model.borrow_mut().set_column_order(&order);
                }
                if let Some(state) = self.viewers.take_state() {
                    self.model.borrow_mut().set_viewer(state);
                }
                self.note_saved_state(hwnd);
                self.apply_pending(hwnd);
                if self.controls().search.select_on_click.replace(false) {
                    self.controls().search.row.input.select_all();
                }
                if let RangeList::Closed(range) = self.controls().filters.range_list.get() {
                    self.controls().filters.range_list.set(RangeList::Idle);
                    self.controls().filters.choose_custom(self, hwnd, range);
                }
                self.shortcuts.show_failures(self, hwnd);
                self.run_modal_steps(hwnd);
                Some(LRESULT(0))
            }
            Message::VerticalScroll { from, request } if from == self.controls().filters.pane => {
                self.controls()
                    .filters
                    .scroll_filters(self, |filters, position| {
                        filters.scrolled(position, request)
                    });
                Some(LRESULT(0))
            }
            Message::VerticalScroll { .. } => None,
            Message::MouseWheel { delta, point }
                if self.model.borrow().panes().filters
                    && self.controls().filters.pane.screen_rect().contains(point) =>
            {
                self.controls()
                    .filters
                    .scroll_filters(self, |filters, position| {
                        filters.scrolled(position, ScrollRequest::Wheel(delta))
                    });
                Some(LRESULT(0))
            }
            Message::MouseWheel { .. } => None,
            Message::FocusMoved { to } => {
                self.controls().filters.reveal(self, to);
                Some(LRESULT(0))
            }
            Message::ContextMenu {
                point: Some(point), ..
            } if self.on_header(point) => {
                let column = self.controls().search.list.column_at(point);
                let column =
                    column.and_then(|index| self.model.borrow().columns().get(index).copied());
                self.context_menu(
                    hwnd,
                    &commands::header_menu(column),
                    point,
                    None,
                    self.command_context(),
                );
                Some(LRESULT(0))
            }
            Message::ContextMenu { from, point } if from == self.controls().search.list.hwnd() => {
                self.row_menu(hwnd, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { from, point }
                if self.controls().panels.panel_list(from).is_some() =>
            {
                let side = self
                    .controls()
                    .panels
                    .panel_list(from)
                    .expect("the guard found the panel");
                self.controls().panels.panel_menu(self, hwnd, side, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { .. } => None,
            Message::KeyDown {
                target,
                keystroke,
                repeat,
            } => Some(LRESULT(isize::from(
                (repeat && keystroke == Keystroke::plain(commands::Key::Escape))
                    || self.on_key(hwnd, target, keystroke),
            ))),
            Message::Char {
                target,
                character,
                alt,
            } => Some(LRESULT(isize::from(
                self.controls()
                    .panels
                    .on_panel_char(self, target, character, alt),
            ))),
            Message::CtlColorEdit { colors, control } => self
                .controls()
                .panels
                .path_box_colors(self, colors, control),
            Message::DeviceRemoval {
                removal,
                notification,
            } => {
                let allowed =
                    self.controls()
                        .panels
                        .on_device_removal(self, hwnd, removal, notification);
                Some(LRESULT(if allowed {
                    1
                } else {
                    BROADCAST_QUERY_DENY as isize
                }))
            }
            Message::InitMenuPopup => {
                self.update_menu(hwnd);
                Some(LRESULT(0))
            }
            Message::SetCursor { over, client: true }
                if over == hwnd
                    && hwnd
                        .cursor_point()
                        .is_some_and(|point| self.on_splitter(point)) =>
            {
                win::window::show_resize_cursor();
                Some(LRESULT(1))
            }
            Message::SetCursor { .. } => None,
            Message::LeftButtonDown { point } if self.on_splitter(point) => {
                let height = self
                    .created()
                    .layout
                    .borrow()
                    .as_ref()
                    .and_then(Layout::inspector)
                    .map(InspectorLayout::height);
                if let Some(height) = height {
                    self.splitter_drag.set(Some((point.1, height)));
                    hwnd.capture_mouse();
                }
                Some(LRESULT(0))
            }
            Message::LeftButtonDown { .. } => None,
            Message::MouseMove { point } => {
                let (start, height) = self.splitter_drag.get()?;
                let dragged = self
                    .created()
                    .layout
                    .borrow()
                    .as_ref()
                    .and_then(Layout::inspector)
                    .map(|inspector| inspector.dragged(height, Px(point.1 - start)));
                if let Some(dragged) = dragged {
                    let accepted = self.model.borrow_mut().set_inspector_height(dragged);
                    self.apply(hwnd, accepted);
                }
                Some(LRESULT(0))
            }
            // A drag ends when the button goes up, also when the window had
            // no capture to release.
            Message::LeftButtonUp if self.splitter_drag.take().is_some() => {
                win::window::release_mouse();
                Some(LRESULT(0))
            }
            Message::LeftButtonUp => None,
            Message::CaptureChanged => {
                self.splitter_drag.set(None);
                None
            }
            Message::CtlColorStatic { colors, control } => {
                let controls = self.controls();
                let inspector = &controls.inspector;
                Some(if control == inspector.note.hwnd() {
                    colors.gray_text()
                } else if control == inspector.age.hwnd() {
                    colors.muted_text()
                } else if controls.dividers.iter().any(|d| d.hwnd() == control) {
                    colors.divider()
                } else {
                    colors.window_background()
                })
            }
            Message::SettingChange {
                regional_format: true,
            } => {
                self.numbers.replace(WindowsNumbers::new());
                self.settings.include_summary_changed();
                self.settings.include_preview_changed();
                self.model.borrow_mut().operation_text_changed();
                self.start_list_passes();
                self.refresh_text(hwnd);
                None
            }
            Message::SettingChange { .. } => None,
            Message::SysColorChange => {
                self.apply_glyphs(hwnd.dpi());
                None
            }
            Message::EndSession { ending: true } => {
                if self.ending_session.replace(true) {
                    return Some(LRESULT(0));
                }
                self.viewers.close_all();
                hwnd.hide();
                self.save_settings(false);
                hwnd.destroy();
                let shutdown = self.shutdown.borrow().clone();
                if let Some(shutdown) = shutdown {
                    shutdown.finish_session();
                }
                Some(LRESULT(0))
            }
            Message::EndSession { .. } => {
                self.ending_session.set(false);
                None
            }
            Message::Close => {
                self.close(hwnd);
                Some(LRESULT(0))
            }
            // Close delivery before releasing native endpoints; persistence and
            // accepted-mutation cleanup retain their own off-UI lifetimes.
            Message::Destroy => {
                // A window gone before the loop ended leaves nothing for
                // close_for_setup to close.
                if self.ending.get() == Ending::ForSetup {
                    self.ending.set(Ending::Running);
                }
                // Exit can come inside a modal step's loop, and the steps
                // that wait for it would run against the destroyed window.
                let steps = std::mem::take(&mut *self.modal_steps.borrow_mut());
                let mut retired = Retirement::default();
                retired.push(steps, crate::core::work::INLINE_ITEMS + 1);
                retired.push(
                    self.operation_delivery.take(),
                    crate::core::work::INLINE_ITEMS + 1,
                );
                self.created().work.borrow_mut().retire(retired);
                self.retire_native_selections(hwnd);
                hwnd.kill_timer(Timer::OperationDelivery.id());
                self.operation_timer.set(false);
                self.drag_leave();
                self.created().waker.close();
                self.drop_target.replace(None);
                self.dropping.borrow_mut().clear();
                self.cancel_paste(None);
                self.model.borrow_mut().retire_following();
                self.retire_model();
                let recovery_finished = self.file_operations.get().map(|operations| {
                    let finished = operations.close();
                    self.created()
                        .work
                        .borrow_mut()
                        .retire(operations.take_retired());
                    finished
                });
                let (settings_finished, cleanup) = self.created().work.borrow_mut().close();
                let cleanup = win::work::spawn(cleanup);
                self.shortcuts.close();
                // No Viewer outlives the message loop.
                self.viewers.close_all();
                let error_log = self.settings_file.with_file_name("ui-error.log");
                let shutdown = win::window::quit_after(move |worker| {
                    super::work::finish_shutdown(
                        worker,
                        settings_finished,
                        recovery_finished,
                        cleanup,
                        &error_log,
                    );
                });
                self.shutdown.replace(Some(shutdown));
                Some(LRESULT(0))
            }
            Message::Hotkey(id) => {
                if let Some(mode) = self.shortcuts.mode(id) {
                    self.show_mode(hwnd, mode);
                }
                Some(LRESULT(0))
            }
            // A second start with `--location`, such as from Total Commander
            // or a script (DESIGN.md: Window lifetime), shows Search, whose
            // filter Location is (DESIGN.md: Commander mode's window).
            Message::CopyData { tag, text } if tag == self.show_message as usize => {
                let accepted = self.model.borrow_mut().show_mode(Mode::Search);
                self.apply(hwnd, accepted);
                let accepted = self.model.borrow_mut().choose_folder(text);
                self.apply(hwnd, accepted);
                self.show_window(hwnd);
                Some(LRESULT(1))
            }
            Message::CopyData { .. } => None,
            Message::Other(msg) if msg == self.show_message => {
                self.show_mode(hwnd, Mode::Search);
                Some(LRESULT(0))
            }
            // A start of another program (DESIGN.md: Window lifetime).
            // A window with a dialog open comes forward instead, because
            // the dialog's loop runs on the window.
            Message::Other(msg) if msg == self.exit_message => {
                let closes = if hwnd.is_enabled() {
                    self.exit(hwnd)
                } else {
                    self.show_window(hwnd);
                    false
                };
                let answer = if closes {
                    super::EXIT_CLOSES
                } else {
                    super::EXIT_STAYS
                };
                Some(LRESULT(answer))
            }
            Message::Other(clipboard::CLIPBOARD_CHANGED) => {
                self.model
                    .borrow_mut()
                    .set_clipboard_holds_files(clipboard::holds_files());
                Some(LRESULT(0))
            }
            Message::Other(QUEUE_CHANGED) => {
                self.created().work.borrow().queue.woken();
                self.schedule_operations(hwnd);
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.taskbar_button_created => {
                if let Some(operations) = self.file_operations.get() {
                    operations.forget_taskbar();
                }
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.taskbar_created => {
                if let Some(tray) = &mut self.created().work.borrow_mut().tray {
                    tray.restart();
                }
                Some(LRESULT(0))
            }
            Message::Other(_) => None,
        }
    }
}

/// The label of the mount prefix `prefix` in `labels`.
fn label_of(labels: &[(String, Option<String>)], prefix: &str) -> Option<String> {
    labels
        .iter()
        .find(|(p, _)| p == prefix)
        .and_then(|(_, label)| label.clone())
}
