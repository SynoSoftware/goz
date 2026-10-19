//! The main window and its message handlers.
//!
//! Window rule 1 (DESIGN.md): a handler changes the model in a statement of its
//! own, or binds a borrow with `let` only in a block that makes no Win32 call,
//! and calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.

use super::dialogs::{About, CustomDate, CustomSize, SearchSyntax};
use super::icons::Icons;
use super::requests::Requests;
use super::settings::SettingsDialog;
use crate::core::captions::{
    self, DialogCaptions, FilterCaptions, IndexCaptions, InspectorCaptions, ListCaption,
    StatusCaptions, WindowCaptions,
};
use crate::core::clipboard::{ClipboardData, ClipboardFormat};
use crate::core::commands::{
    self, CommandId, Focus, INSPECTOR_BUTTONS, KeyAction, Keystroke, Menu, MenuItem, State,
};
use crate::core::fetch::{DaemonFailure, Fetch, Purpose, WriteFailure};
use crate::core::ids::{ControlId, DialogId, IconId};
use crate::core::layout::{
    self, CaptionWidths, Dpi, FilterLayout, InspectorLayout, Layout, MessageArea, Px, Rect,
    ScrollRequest, Size,
};
use crate::core::model::{
    Accepted, Change, Column, ExportError, ExportFormat, Model, ScopeCheck, Then,
};
use crate::core::open::{self, CommandLine};
use crate::core::query::{
    AttributeChoice, DateModified, Filters, Kind, MatchMode, Matching, RangeItem, SizeFilter,
    date_items, size_items,
};
use crate::core::results::{Fix, ListButton};
use crate::core::selection::wide_path;
use crate::core::settings::{SavedColumn, SettingsValues};
use crate::core::shortcuts::{CommandShortcuts, ShowShortcut};
use crate::core::text::{LanguageChoice, Numbers, Strings, without_access_key};
use crate::core::update;
use crate::win::clipboard::ClipboardBlock;
use crate::win::controls::{
    Button, CheckBox, DropDownList, Edit, Label, ListBox, ListClosing, ListColumn, ListView,
    StatusBar,
};
use crate::win::dialog::{DialogEnd, DialogProc};
use crate::win::gdi::{Font, ImageList, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::menu::Accelerators;
use crate::win::notify::{
    Command, ControlEvent, CustomDraw, DispInfo, DrawAnswer, DrawStage, ListNotify,
};
use crate::win::shell::{self, FileType};
use crate::win::shortcut::{self, RegisterError, Registration};
use crate::win::tray::{self, TrayEvent, TrayIcon};
use crate::win::wake::Waker;
use crate::win::window::{DEFERRED, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_client::RELEASES;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortSpec};
use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::OsString;
use std::io;
use std::ops::Range;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use windows::Win32::Foundation::LRESULT;

mod timer {
    /// Search as you type waits this long after the last keystroke.
    pub(super) const SEARCH: usize = 1;
    pub(super) const STATUS: usize = 2;
    /// The selection is read this long after it last changed.
    pub(super) const SELECTION: usize = 3;
}

const SEARCH_DELAY_MS: u32 = 150;
const STATUS_POLL_MS: u32 = 2_000;
const SELECTION_DELAY_MS: u32 = 150;

/// Status part 4, which names a newer release that a click then opens.
const INDEX_PART: usize = 3;

/// What Settings' OK or Apply applied (DESIGN.md: Settings dialog).
pub(super) struct Applied {
    /// The values now in effect: the edited ones, except a registry
    /// setting whose write failed, which keeps its earlier value.
    pub(super) values: SettingsValues,
    /// The message of each registry write that failed.
    pub(super) failures: Vec<String>,
}

/// What the update check's thread found. The thread is not joined, because
/// a WinHTTP call cannot be ended at once and joining it would hold Exit for
/// as long as the network waits. Instead it wakes the window only while
/// `window_open`, which `WM_DESTROY` clears under the lock, so no wake-up
/// reaches a reused handle (DESIGN.md: window rule 5).
#[derive(Default)]
struct UpdateCheck {
    window_open: bool,
    newer: Option<String>,
}

/// The controls, created in `WM_CREATE` in Tab order; Windows destroys them
/// with the window.
struct Controls {
    search: Edit,
    match_label: Label,
    match_mode: DropDownList,
    match_case: CheckBox,
    match_path: CheckBox,
    filters: FilterPane,
    /// The list's message and its buttons, in the list's empty area. They
    /// are created before the list, so they lie above it and come before it
    /// in the Tab order.
    message: Label,
    /// In [`Model::list_buttons`]' order.
    message_buttons: [Button; 2],
    list: ListView,
    inspector: Inspector,
    status: StatusBar,
}

impl Controls {
    fn create(parent: Hwnd) -> windows::core::Result<Controls> {
        let controls = Controls {
            search: Edit::create(parent, ControlId::Search.id())?,
            // The label comes right before its list, so its access key
            // focuses the list (DESIGN.md: C19).
            match_label: Label::create(parent, ControlId::MatchLabel.id())?,
            match_mode: DropDownList::create(parent, ControlId::MatchMode.id())?,
            match_case: CheckBox::create(parent, ControlId::MatchCase.id())?,
            match_path: CheckBox::create(parent, ControlId::MatchPath.id())?,
            filters: FilterPane::create(parent)?,
            message: Label::create_message(parent, ControlId::Message.id())?,
            message_buttons: [
                Button::create(parent, ControlId::MessageButton1.id())?,
                Button::create(parent, ControlId::MessageButton2.id())?,
            ],
            list: ListView::create(parent, ControlId::List.id())?,
            inspector: Inspector::create(parent)?,
            status: StatusBar::create(parent, ControlId::Status.id())?,
        };
        controls.message.hwnd().set_visible(false);
        for button in controls.message_buttons {
            button.hwnd().set_visible(false);
        }
        Ok(controls)
    }

    /// Gives every control the window's font, at creation and on a monitor
    /// with another DPI.
    fn apply_fonts(&self, fonts: &Fonts) {
        let controls = [
            self.search.hwnd(),
            self.match_label.hwnd(),
            self.match_mode.hwnd(),
            self.match_case.hwnd(),
            self.match_path.hwnd(),
            self.message.hwnd(),
            self.message_buttons[0].hwnd(),
            self.message_buttons[1].hwnd(),
            self.list.hwnd(),
            self.status.hwnd(),
        ];
        let pane = self.filters.controls();
        let inspector = self.inspector.controls();
        for control in controls.into_iter().chain(pane).chain(inspector) {
            control.set_font(&fonts.ui);
        }
        self.inspector.heading.hwnd().set_font(&fonts.heading);
        // Rows as tall as the Kind list's, which Windows draws.
        let height = TextMeter::new(&fonts.ui).height();
        self.filters.location.set_item_height(height);
    }
}

/// The filter pane's window and its controls, in Tab order. Each label
/// comes right before its control, so its access key focuses the control
/// (DESIGN.md: Filter pane).
struct FilterPane {
    pane: Hwnd,
    clear: Button,
    kind_label: Label,
    kind: ListBox,
    location_label: Label,
    location: ListBox,
    choose_folder: Button,
    only_directly: CheckBox,
    date_label: Label,
    date: DropDownList,
    size_label: Label,
    size: DropDownList,
    attributes_label: Label,
    /// Each attribute's label and list, in the order of
    /// [`crate::core::query::ATTRIBUTES`].
    attributes: Vec<(Label, DropDownList)>,
}

/// The controls of each attribute's label and list.
const ATTRIBUTE_CONTROLS: [(ControlId, ControlId); 6] = [
    (ControlId::HiddenLabel, ControlId::Hidden),
    (ControlId::SystemLabel, ControlId::System),
    (ControlId::ReadOnlyLabel, ControlId::ReadOnly),
    (ControlId::LinkLabel, ControlId::Link),
    (ControlId::CompressedLabel, ControlId::Compressed),
    (ControlId::EncryptedLabel, ControlId::Encrypted),
];

/// The Date modified and Size lists, whose Custom… opens a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RangeFilter {
    Date,
    Size,
}

impl FilterPane {
    fn create(parent: Hwnd) -> windows::core::Result<FilterPane> {
        let pane = win::pane::create(parent, ControlId::FilterPane.id())?;
        let label = |id: ControlId| Label::create(pane, id.id());
        let drop_down = |id: ControlId| DropDownList::create(pane, id.id());
        let mut filters = FilterPane {
            pane,
            clear: Button::create(pane, ControlId::ClearFilters.id())?,
            kind_label: label(ControlId::KindLabel)?,
            kind: ListBox::create(pane, ControlId::Kind.id())?,
            location_label: label(ControlId::LocationLabel)?,
            location: ListBox::create_paths(pane, ControlId::Location.id())?,
            choose_folder: Button::create(pane, ControlId::ChooseFolder.id())?,
            only_directly: CheckBox::create(pane, ControlId::OnlyDirectly.id())?,
            date_label: label(ControlId::DateLabel)?,
            date: drop_down(ControlId::Date)?,
            size_label: label(ControlId::SizeLabel)?,
            size: drop_down(ControlId::Size)?,
            attributes_label: label(ControlId::AttributesLabel)?,
            attributes: Vec::new(),
        };
        for (name, choice) in ATTRIBUTE_CONTROLS {
            filters.attributes.push((label(name)?, drop_down(choice)?));
        }
        Ok(filters)
    }

    /// Every control in the pane.
    fn controls(&self) -> Vec<Hwnd> {
        self.placed_with(None)
            .into_iter()
            .map(|(control, ..)| control)
            .collect()
    }

    /// Each control, its rectangle in `layout` before the pane scrolls, and
    /// whether it is a drop-down list, whose rectangle also holds its open
    /// list.
    fn placed(&self, layout: &FilterLayout) -> Vec<(Hwnd, Rect, bool)> {
        self.placed_with(Some(layout))
            .into_iter()
            .map(|(control, rect, drop_down)| (control, rect.unwrap_or_default(), drop_down))
            .collect()
    }

    fn placed_with(&self, layout: Option<&FilterLayout>) -> Vec<(Hwnd, Option<Rect>, bool)> {
        let at = |rect: fn(&FilterLayout) -> Rect| layout.map(rect);
        let mut placed = vec![
            (self.clear.hwnd(), at(|l| l.clear), false),
            (self.kind_label.hwnd(), at(|l| l.kind_label), false),
            (self.kind.hwnd(), at(|l| l.kind), false),
            (self.location_label.hwnd(), at(|l| l.location_label), false),
            (self.location.hwnd(), at(|l| l.location), false),
            (self.choose_folder.hwnd(), at(|l| l.choose_folder), false),
            (self.only_directly.hwnd(), at(|l| l.only_directly), false),
            (self.date_label.hwnd(), at(|l| l.date_label), false),
            (self.date.hwnd(), at(|l| l.date), true),
            (self.size_label.hwnd(), at(|l| l.size_label), false),
            (self.size.hwnd(), at(|l| l.size), true),
            (
                self.attributes_label.hwnd(),
                at(|l| l.attributes_label),
                false,
            ),
        ];
        for (index, (name, choice)) in self.attributes.iter().enumerate() {
            let rects = layout.map(|l| l.attributes[index]);
            placed.push((name.hwnd(), rects.map(|r| r.0), false));
            placed.push((choice.hwnd(), rects.map(|r| r.1), true));
        }
        placed
    }

    /// Sets the labels and the buttons' captions.
    fn set_captions(&self, captions: &FilterCaptions) {
        self.clear.hwnd().set_text(&captions.clear);
        self.kind_label.hwnd().set_text(&captions.kind);
        self.location_label.hwnd().set_text(&captions.location);
        self.choose_folder.hwnd().set_text(&captions.choose_folder);
        self.only_directly.hwnd().set_text(&captions.only_directly);
        self.date_label.hwnd().set_text(&captions.date);
        self.size_label.hwnd().set_text(&captions.size);
        self.attributes_label.hwnd().set_text(&captions.attributes);
        for ((name, _), text) in self.attributes.iter().zip(&captions.attribute_names) {
            name.hwnd().set_text(text);
        }
    }

    /// The list `range` stands for.
    fn range(&self, range: RangeFilter) -> DropDownList {
        match range {
            RangeFilter::Date => self.date,
            RangeFilter::Size => self.size,
        }
    }

    /// The range list that `control` is, if it is one.
    fn range_of(&self, control: Hwnd) -> Option<RangeFilter> {
        [RangeFilter::Date, RangeFilter::Size]
            .into_iter()
            .find(|range| self.range(*range).hwnd() == control)
    }
}

/// The inspector's controls, in Tab order (DESIGN.md: Inspector pane). Its icon
/// is painted by the window.
struct Inspector {
    heading: Label,
    /// In [`INSPECTOR_BUTTONS`]' order.
    buttons: [Button; 3],
    /// Each field row's label and value.
    fields: Vec<(Label, Edit)>,
}

/// The controls of each inspector field row's label and value.
const FIELD_CONTROLS: [(ControlId, ControlId); 7] = [
    (ControlId::InspectorLabel1, ControlId::InspectorValue1),
    (ControlId::InspectorLabel2, ControlId::InspectorValue2),
    (ControlId::InspectorLabel3, ControlId::InspectorValue3),
    (ControlId::InspectorLabel4, ControlId::InspectorValue4),
    (ControlId::InspectorLabel5, ControlId::InspectorValue5),
    (ControlId::InspectorLabel6, ControlId::InspectorValue6),
    (ControlId::InspectorLabel7, ControlId::InspectorValue7),
];

impl Inspector {
    fn create(parent: Hwnd) -> windows::core::Result<Inspector> {
        let button = |id: ControlId| Button::create(parent, id.id());
        let mut inspector = Inspector {
            heading: Label::create_heading(parent, ControlId::InspectorHeading.id())?,
            buttons: [
                button(ControlId::InspectorOpen)?,
                button(ControlId::InspectorOpenFileLocation)?,
                button(ControlId::InspectorCopyAsPath)?,
            ],
            fields: Vec::new(),
        };
        for (label, value) in FIELD_CONTROLS {
            inspector.fields.push((
                Label::create(parent, label.id())?,
                Edit::create_read_only(parent, value.id())?,
            ));
        }
        Ok(inspector)
    }

    /// Every control but the heading, which has a font of its own.
    fn controls(&self) -> Vec<Hwnd> {
        let fields = self
            .fields
            .iter()
            .flat_map(|(label, value)| [label.hwnd(), value.hwnd()]);
        self.buttons
            .iter()
            .map(|b| b.hwnd())
            .chain(fields)
            .collect()
    }

    fn contains(&self, control: Hwnd) -> bool {
        control == self.heading.hwnd() || self.controls().contains(&control)
    }

    /// The value that `control` is, if it is one.
    fn value(&self, control: Hwnd) -> Option<Edit> {
        self.fields
            .iter()
            .map(|(_, value)| *value)
            .find(|value| value.hwnd() == control)
    }
}

/// The controls' font, its bold form for the matched part of a cell, and
/// the inspector's heading font.
struct Fonts {
    ui: Font,
    bold: Font,
    heading: Font,
}

impl Fonts {
    fn new(dpi: Dpi) -> windows::core::Result<Fonts> {
        let ui = Font::message(dpi)?;
        let bold = ui.bold()?;
        let heading = ui.heading()?;
        Ok(Fonts { ui, bold, heading })
    }
}

/// What the layout needs that only measuring the text gives.
#[derive(Clone, Copy)]
struct Metrics {
    dpi: Dpi,
    font_height: Px,
    captions: CaptionWidths,
}

impl Metrics {
    fn measure(fonts: &Fonts, captions: &WindowCaptions, dpi: Dpi) -> Metrics {
        let meter = TextMeter::new(&fonts.ui);
        Metrics {
            dpi,
            font_height: meter.height(),
            captions: CaptionWidths::measure(captions, &|text| meter.width(text)),
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
    /// `None` while the window moves to a monitor with another DPI.
    layout: RefCell<Option<Layout>>,
    requests: Requests,
    images: Option<ImageList>,
    /// The inspector's icons; their indexes are those of `images`.
    large_images: Option<ImageList>,
}

pub(crate) struct MainWindow {
    created: OnceCell<Created>,
    model: RefCell<Model>,
    numbers: RefCell<WindowsNumbers>,
    menus: Vec<Menu>,
    /// The control that had the focus when the window was deactivated.
    focus_on_activate: Cell<Option<Hwnd>>,
    settings_file: PathBuf,
    settings_failure_shown: Cell<bool>,
    show_message: u32,
    icons: Icons,
    /// Whether the status timer runs.
    polling: Cell<bool>,
    /// Whether a [`Message::Deferred`] is posted to read the list's
    /// selection, so one user action reads it once (DESIGN.md: The selection).
    selection_pending: Cell<bool>,
    /// The filter pane's scroll position.
    filter_scroll: Cell<Px>,
    /// How the open Date modified or Size list is closing, between its
    /// `CBN_SELENDOK` or `CBN_SELENDCANCEL` and its `CBN_CLOSEUP`.
    range_closing: Cell<Option<(RangeFilter, ListClosing)>>,
    /// The range list that closed on a choice, whose Custom… opens its
    /// dialog in [`Message::Deferred`], after the list has said which item
    /// it closed on, in whichever order it sends that.
    range_closed: Cell<Option<RangeFilter>>,
    /// What the inspector shows, so a reply that changes none of it sets no
    /// text, and the index of its icon in the system image lists.
    inspector_shown: RefCell<(InspectorCaptions, Option<i32>)>,
    /// While the person drags the splitter: the pointer's client `y` and the
    /// inspector's height when the drag started.
    splitter_drag: Cell<Option<(i32, Px)>>,
    /// The person's command shortcuts, which Settings replaces; the message
    /// loop translates with the table of the moment.
    accelerators: RefCell<Rc<Accelerators>>,
    /// The tray icon, while Gozler keeps running in the notification area.
    tray: RefCell<Option<TrayIcon>>,
    /// `TaskbarCreated`, after which the tray icon is added again.
    taskbar_created: u32,
    /// The shortcut to show Gozler, while Windows lets Gozler use it.
    shortcut: RefCell<Option<Registration>>,
    /// Why the saved shortcut could not be used at start, which the window
    /// says the first time it shows.
    shortcut_failure: RefCell<Option<RegisterError>>,
    /// Whether the window has not shown yet, because Gozler started in the
    /// notification area; its first show restores the saved placement.
    unshown: Cell<bool>,
    /// The open Settings dialog, which Use Total Commander's reply goes to,
    /// and that reply until the dialog reads it.
    settings_dialog: Cell<Option<Hwnd>>,
    total_commander: RefCell<Option<Result<Option<String>, DaemonFailure>>>,
    /// Whether a click gave the search box the focus, so its text is
    /// selected once the click is handled (Settings > Search).
    select_search: Cell<bool>,
    update: Arc<Mutex<UpdateCheck>>,
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
        let accelerators =
            accelerator_table(&model.choices().shortcuts).expect("the accelerator table builds");
        let window = Rc::new(MainWindow {
            created: OnceCell::new(),
            model: RefCell::new(model),
            numbers: RefCell::new(WindowsNumbers::new()),
            menus: commands::menu_bar(),
            focus_on_activate: Cell::new(None),
            settings_file,
            settings_failure_shown: Cell::new(false),
            show_message,
            icons: Icons::default(),
            polling: Cell::new(false),
            selection_pending: Cell::new(false),
            filter_scroll: Cell::new(Px(0)),
            range_closing: Cell::new(None),
            range_closed: Cell::new(None),
            inspector_shown: RefCell::new((InspectorCaptions::default(), None)),
            splitter_drag: Cell::new(None),
            accelerators: RefCell::new(Rc::new(accelerators)),
            tray: RefCell::new(None),
            taskbar_created: win::instance::register_message("TaskbarCreated"),
            shortcut: RefCell::new(None),
            shortcut_failure: RefCell::new(None),
            unshown: Cell::new(false),
            settings_dialog: Cell::new(None),
            total_commander: RefCell::new(None),
            select_search: Cell::new(false),
            update: Arc::new(Mutex::new(UpdateCheck {
                window_open: true,
                newer: None,
            })),
        });
        let hwnd = win::window::create_window(class, Rc::clone(&window), placement)
            .expect("the main window opens");
        // Only a status reply names the volumes, so it is asked for beside
        // the first search (DESIGN.md: Requests).
        window.search(hwnd);
        window.ask_status();
        window.register_saved_shortcut(hwnd);
        window.update_tray(hwnd);
        window.check_for_updates(hwnd);
        let keeps_running = window.model.borrow().choices().keep_running;
        // Showing the window activates it, and WM_ACTIVATE focuses the
        // search box.
        match (sign_in, keeps_running) {
            (true, true) => window.unshown.set(true),
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

    fn on_create(&self, hwnd: Hwnd) -> bool {
        let dpi = hwnd.dpi();
        let (Ok(controls), Ok(fonts)) = (Controls::create(hwnd), Fonts::new(dpi)) else {
            return false;
        };
        controls.apply_fonts(&fonts);
        let (columns, order) = {
            let model = self.model.borrow();
            let columns: Vec<(Column, Px)> = model
                .columns()
                .iter()
                .map(|c| (*c, layout::column_width(*c, model.saved_width(*c), dpi)))
                .collect();
            (columns, model.column_order())
        };
        for (index, (column, width)) in columns.into_iter().enumerate() {
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width,
                    right_aligned: column.right_aligned(),
                },
            );
        }
        controls.list.set_column_order(&order);
        let images = ImageList::system_small();
        if let Some(images) = &images {
            controls.list.set_small_images(images);
        }
        let captions = self.captions();
        let metrics = Metrics::measure(&fonts, &captions, dpi);
        let _ = self.created.set(Created {
            controls,
            fonts: RefCell::new(fonts),
            metrics: Cell::new(metrics),
            layout: RefCell::new(None),
            requests: Requests::new(Waker::new(hwnd)),
            images,
            large_images: ImageList::system_large(),
        });
        self.apply_text(hwnd, &captions);
        self.layout(hwnd);
        true
    }

    /// Every caption, from the model's facts and the current strings
    /// (DESIGN.md: window rule 12).
    fn captions(&self) -> WindowCaptions {
        WindowCaptions::new(&self.model.borrow(), &*self.numbers.borrow())
    }

    /// Sets every caption, then measures them and lays out again, and draws
    /// the window once at the end. Runs after a change to the language or
    /// to the regional format.
    fn refresh_text(&self, hwnd: Hwnd) {
        hwnd.draw_once_after(|| {
            let captions = self.captions();
            self.apply_text(hwnd, &captions);
            let created = self.created();
            let metrics = Metrics::measure(&created.fonts.borrow(), &captions, hwnd.dpi());
            created.metrics.set(metrics);
            self.layout(hwnd);
        });
    }

    /// Loads the language of `choice`, gives it to the model and sets every
    /// caption in it (DESIGN.md: Language). The Language menu
    /// and Settings' OK and Apply call it; the typed text, the results, the
    /// selection and the focus stay as they are.
    fn set_language(&self, hwnd: Hwnd, choice: LanguageChoice) {
        let language = choice.resolve(&win::locale::display_languages());
        self.model
            .borrow_mut()
            .set_language(choice, Strings::load(language));
        super::set_panic_report(|report| report.language = Some(language));
        self.refresh_text(hwnd);
    }

    fn apply_text(&self, hwnd: Hwnd, captions: &WindowCaptions) {
        let controls = &self.created().controls;
        hwnd.set_text(&captions.title);
        let _ = win::menu::replace_menu_bar(hwnd, &captions.menu_bar);
        controls.search.set_cue(&captions.search_cue);
        controls.match_label.hwnd().set_text(&captions.match_label);
        let mode = self.match_mode_index();
        let modes = captions.match_modes.each_ref().map(String::as_str);
        controls.match_mode.set_items(&modes, mode);
        controls.match_case.hwnd().set_text(&captions.match_case);
        controls.match_path.hwnd().set_text(&captions.match_path);
        controls.filters.set_captions(&captions.filters);
        self.show_filters(&captions.filters);
        for (index, header) in captions.headers.iter().enumerate() {
            controls.list.set_header(index, header);
        }
        for (button, caption) in controls
            .inspector
            .buttons
            .iter()
            .zip(&captions.inspector_buttons)
        {
            button.hwnd().set_text(caption);
        }
        self.show_inspector(hwnd, captions.inspector.clone());
        self.show_status(&captions.status);
        self.show_message(captions.message.as_ref());
        self.update_bound_controls();
    }

    /// Gives the filter pane's lists their items and selects the model's
    /// choices.
    fn show_filters(&self, captions: &FilterCaptions) {
        let pane = &self.created().controls.filters;
        let (positions, located, only_directly) = {
            let model = self.model.borrow();
            let filters = model.filters();
            (
                filters.positions(&model.location_items()),
                filters.location.is_some(),
                filters.only_directly,
            )
        };
        pane.kind.set_items(&captions.kinds, positions.kind);
        pane.location
            .set_items(&captions.locations, positions.location);
        // Only directly in a folder needs a folder.
        pane.only_directly.hwnd().set_enabled(located);
        pane.only_directly.set_checked(only_directly);
        pane.date.set_items(&captions.dates, positions.date);
        pane.size.set_items(&captions.sizes, positions.size);
        for ((_, list), position) in pane.attributes.iter().zip(positions.attributes) {
            list.set_items(&captions.attribute_choices, position);
        }
    }

    fn update_filters(&self) {
        let captions = FilterCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        self.show_filters(&captions);
    }

    /// The Location list after a status reply, which can name other
    /// volumes; the other lists keep what the person moved to, such as
    /// Custom… with the arrow keys.
    fn update_locations(&self) {
        let (locations, position) = {
            let model = self.model.borrow();
            let items = model.location_items();
            (
                captions::location_items(&model),
                model.filters().positions(&items).location,
            )
        };
        self.created()
            .controls
            .filters
            .location
            .set_items(&locations, position);
    }

    fn match_mode_index(&self) -> usize {
        let mode = self.model.borrow().matching().mode;
        MatchMode::ALL
            .iter()
            .position(|m| *m == mode)
            .expect("ALL holds every mode")
    }

    /// Gives the controls bound to a command the command's state and
    /// choice, after every change (DESIGN.md: window rule 9): the match
    /// controls, the pane's Clear filters, the inspector's buttons and the
    /// header's sort arrow.
    fn update_bound_controls(&self) {
        let controls = &self.created().controls;
        let (enabled, can_clear, buttons, matching) = {
            let model = self.model.borrow();
            let state = State {
                model: &model,
                focus: Focus::Other,
            };
            (
                (CommandId::Match.row().enabled)(&state),
                (CommandId::ClearFilters.row().enabled)(&state),
                INSPECTOR_BUTTONS.map(|(command, _)| (command.row().enabled)(&state)),
                model.matching(),
            )
        };
        controls.filters.clear.hwnd().set_enabled(can_clear);
        for (button, enabled) in controls.inspector.buttons.iter().zip(buttons) {
            button.hwnd().set_enabled(enabled);
        }
        for control in [
            controls.match_label.hwnd(),
            controls.match_mode.hwnd(),
            controls.match_case.hwnd(),
            controls.match_path.hwnd(),
        ] {
            control.set_enabled(enabled);
        }
        controls.match_mode.select(self.match_mode_index());
        controls.match_case.set_checked(matching.case);
        controls.match_path.set_checked(matching.path);
        self.show_sort();
    }

    fn update_status(&self) {
        let status = StatusCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        self.show_status(&status);
    }

    /// Shows the selection in the inspector, if anything it shows changed.
    fn update_inspector(&self, hwnd: Hwnd) {
        let captions = InspectorCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        if self.inspector_shown.borrow().0 != captions {
            self.show_inspector(hwnd, captions);
        }
    }

    fn show_inspector(&self, hwnd: Hwnd, captions: InspectorCaptions) {
        let inspector = &self.created().controls.inspector;
        inspector.heading.hwnd().set_text(&captions.heading);
        for (index, (label, value)) in inspector.fields.iter().enumerate() {
            let (label_text, value_text) = captions
                .fields
                .get(index)
                .map_or(("", ""), |(l, v)| (l.as_str(), v.as_str()));
            label.hwnd().set_text(label_text);
            value.hwnd().set_text(value_text);
        }
        // The shell call can dispatch messages, so no borrow is held
        // (DESIGN.md: window rule 1).
        let icon = captions
            .icon
            .as_ref()
            .map(|(name, is_folder)| self.icons.index(name, *is_folder));
        let icon_changed = self.inspector_shown.borrow().1 != icon;
        self.inspector_shown.replace((captions, icon));
        self.show_fields();
        if icon_changed {
            hwnd.invalidate();
        }
    }

    /// Shows each field row that has a field and room in the pane, and
    /// hides the others, so Tab skips them.
    fn show_fields(&self) {
        let rows = {
            let layout = self.created().layout.borrow();
            let room = layout
                .as_ref()
                .and_then(|l| l.inspector.as_ref())
                .map_or(0, |i| i.fields.len());
            room.min(self.inspector_shown.borrow().0.fields.len())
        };
        for (index, (label, value)) in self.created().controls.inspector.fields.iter().enumerate() {
            label.hwnd().set_visible(index < rows);
            value.hwnd().set_visible(index < rows);
        }
    }

    fn show_status(&self, status: &StatusCaptions) {
        let bar = self.created().controls.status;
        for (part, text) in status.parts().into_iter().enumerate() {
            bar.set_text(part, text);
        }
    }

    fn update_message(&self) {
        let message = captions::list_message(&self.model.borrow());
        self.show_message(message.as_ref());
    }

    /// Shows the list's message and its buttons, or hides them.
    fn show_message(&self, message: Option<&ListCaption>) {
        let controls = &self.created().controls;
        let (text, buttons) = match message {
            Some((text, buttons)) => (text.as_str(), buttons.as_slice()),
            None => ("", &[][..]),
        };
        controls.message.hwnd().set_text(text);
        for (index, button) in controls.message_buttons.iter().enumerate() {
            button
                .hwnd()
                .set_text(buttons.get(index).map_or("", String::as_str));
        }
        self.place_message(text, buttons);
        controls.message.hwnd().set_visible(message.is_some());
        for (index, button) in controls.message_buttons.iter().enumerate() {
            button.hwnd().set_visible(index < buttons.len());
        }
    }

    /// Places the message below the list's header, and its buttons below
    /// it.
    fn place_message(&self, text: &str, buttons: &[String]) {
        let created = self.created();
        let Some(list) = created.layout.borrow().as_ref().map(|l| l.list) else {
            return;
        };
        let controls = &created.controls;
        let metrics = created.metrics.get();
        let header = controls.list.header().screen_rect().height;
        let area = MessageArea::new(list, header, metrics.dpi);
        let (text, rects) = {
            let fonts = created.fonts.borrow();
            let meter = TextMeter::new(&fonts.ui);
            let widths: Vec<Px> = buttons.iter().map(|b| meter.width(b)).collect();
            area.place(
                meter.wrapped_height(text, area.text_width()),
                metrics.font_height,
                &widths,
            )
        };
        controls.message.hwnd().set_rect(text);
        for (button, rect) in controls.message_buttons.iter().zip(rects) {
            button.hwnd().set_rect(rect);
        }
    }

    /// Places every control where `Layout::compute` says (DESIGN.md:
    /// window rule 7).
    fn layout(&self, hwnd: Hwnd) {
        let created = self.created();
        let controls = &created.controls;
        let metrics = created.metrics.get();
        let status_height = controls.status.fit();
        let panes = self.model.borrow().panes();
        let layout = Layout::compute(
            hwnd.client_size(),
            metrics.dpi,
            metrics.font_height,
            status_height,
            &metrics.captions,
            &panes,
        );
        controls.search.hwnd().set_rect(layout.search);
        controls.match_label.hwnd().set_rect(layout.match_label);
        // A drop-down list's height includes its open list.
        controls.match_mode.hwnd().set_rect(Rect {
            height: Px(layout.match_mode.height.0 * 10),
            ..layout.match_mode
        });
        controls.match_case.hwnd().set_rect(layout.match_case);
        controls.match_path.hwnd().set_rect(layout.match_path);
        if let Some(filters) = &layout.filters {
            self.place_filters(filters, self.filter_scroll.get());
        }
        controls.filters.pane.set_visible(layout.filters.is_some());
        controls.list.hwnd().set_rect(layout.list);
        let inspector = &controls.inspector;
        if let Some(placed) = &layout.inspector {
            inspector.heading.hwnd().set_rect(placed.heading);
            for (button, rect) in inspector.buttons.iter().zip(placed.buttons) {
                button.hwnd().set_rect(rect);
            }
            for ((label, value), (label_rect, value_rect)) in
                inspector.fields.iter().zip(&placed.fields)
            {
                label.hwnd().set_rect(*label_rect);
                value.hwnd().set_rect(*value_rect);
            }
        }
        let shown = layout.inspector.is_some();
        inspector.heading.hwnd().set_visible(shown);
        for button in inspector.buttons {
            button.hwnd().set_visible(shown);
        }
        controls.status.set_parts(&layout.status_parts);
        created.layout.replace(Some(layout));
        self.show_fields();
        self.update_message();
        hwnd.invalidate();
    }

    /// Places the pane and its controls for the scroll position nearest
    /// `position` that `filters` allows.
    fn place_filters(&self, filters: &FilterLayout, position: Px) {
        let pane = &self.created().controls.filters;
        let position = filters.scrolled(position, ScrollRequest::To(position));
        self.filter_scroll.set(position);
        pane.pane.set_rect(filters.pane);
        for (control, rect, drop_down) in pane.placed(filters) {
            // A drop-down list's height includes its open list.
            let height = if drop_down {
                Px(rect.height.0 * 12)
            } else {
                rect.height
            };
            control.set_rect(Rect {
                y: rect.y - position,
                height,
                ..rect
            });
        }
        pane.pane
            .set_vertical_scroll(filters.content_height, filters.pane.height, position);
    }

    /// Scrolls the pane to the position `to` gives for its layout and its
    /// current position.
    fn scroll_filters(&self, to: impl FnOnce(&FilterLayout, Px) -> Px) {
        let filters = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(|l| l.filters.clone());
        let Some(filters) = filters else { return };
        let position = to(&filters, self.filter_scroll.get());
        if position != self.filter_scroll.get() {
            self.place_filters(&filters, position);
        }
    }

    /// Scrolls the pane so that `control`, which a key focused, shows.
    fn reveal(&self, control: Hwnd) {
        let filters = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(|l| l.filters.clone());
        let Some(filters) = filters else { return };
        let placed = self.created().controls.filters.placed(&filters);
        if let Some((_, rect, _)) = placed.into_iter().find(|(c, ..)| *c == control) {
            self.scroll_filters(|filters, position| filters.revealing(position, rect));
        }
    }

    fn on_dpi_changed(&self, hwnd: Hwnd, dpi: Dpi, suggested: Rect) {
        let created = self.created();
        let controls = &created.controls;
        let Ok(fonts) = Fonts::new(dpi) else { return };
        controls.apply_fonts(&fonts);
        drop(created.fonts.replace(fonts));
        let old = created.metrics.get().dpi;
        let columns = self.model.borrow().columns().len();
        for index in 0..columns {
            let width = controls.list.column_width(index);
            controls
                .list
                .set_column_width(index, width.rescale(old, dpi));
        }
        let captions = self.captions();
        created
            .metrics
            .set(Metrics::measure(&created.fonts.borrow(), &captions, dpi));
        // The minimum size is in the old DPI's pixels until the layout
        // runs again, so it does not hold the window to the old size.
        created.layout.replace(None);
        hwnd.set_rect(suggested);
        self.layout(hwnd);
    }

    /// Which control of the window `control` is, as the command table asks.
    fn focus_of(&self, control: Option<Hwnd>) -> Focus {
        let controls = &self.created().controls;
        if let Some(value) = control.and_then(|c| controls.inspector.value(c)) {
            return Focus::Value {
                text_selected: value.has_selected_text(),
            };
        }
        match control {
            Some(control) if control == controls.search.hwnd() => Focus::SearchBox {
                text_selected: controls.search.has_selected_text(),
            },
            Some(control) if control == controls.list.hwnd() => Focus::List,
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
        Hwnd::focused().and_then(|focused| self.created().controls.inspector.value(focused))
    }

    /// Each item's ID, whether it is enabled and whether it is checked, from
    /// the command table.
    fn item_states(&self, items: &[MenuItem]) -> Vec<(u16, bool, bool)> {
        let focus = self.focus();
        let model = self.model.borrow();
        let state = State {
            model: &model,
            focus,
        };
        items
            .iter()
            .map(|item| (item.id(), item.enabled(&state), item.checked(&state)))
            .collect()
    }

    /// Sets every menu item's enabled state and mark from the command
    /// table, with Export results… or Cancel export in its place, after the
    /// selection of the action that opened the menu is read.
    fn update_menu(&self, hwnd: Hwnd) {
        self.read_list_selection(hwnd);
        let Some(bar) = win::menu::Menu::of(hwnd) else {
            return;
        };
        let (slot, node) = {
            let model = self.model.borrow();
            let slot = commands::export_slot(&model);
            let node = commands::node(&slot, model.strings(), &model.choices().shortcuts);
            (slot, node)
        };
        let export = [CommandId::ExportResults, CommandId::CancelExport]
            .map(|command| MenuItem::Run(command).id());
        bar.replace_item(&export, &node);
        let items: Vec<MenuItem> = commands::items(&self.menus)
            .into_iter()
            .map(|item| match item {
                MenuItem::Run(CommandId::ExportResults) => slot,
                item => item,
            })
            .collect();
        for (id, enabled, checked) in self.item_states(&items) {
            bar.set_state(id, enabled, checked);
        }
    }

    /// Runs one menu item or shortcut; every surface calls this (DESIGN.md:
    /// window rule 9). It asks the command table again whether the item is
    /// enabled, because the state can change while a menu is open (DESIGN.md:
    /// T11).
    fn run(&self, hwnd: Hwnd, item: MenuItem) {
        let focus = self.focus();
        let enabled = {
            let model = self.model.borrow();
            item.enabled(&State {
                model: &model,
                focus,
            })
        };
        if !enabled {
            return;
        }
        let controls = &self.created().controls;
        let accepted = match item {
            MenuItem::Run(command) => match command {
                CommandId::Exit => return self.exit(hwnd),
                CommandId::About => {
                    let about = About::new(self.model.borrow().strings());
                    self.open_dialog(hwnd, DialogId::About, &about);
                    return;
                }
                CommandId::Open => return self.open_selected(hwnd),
                CommandId::OpenFileLocation => return self.show_selected(hwnd),
                CommandId::Properties => return self.show_properties(hwnd),
                CommandId::ExportResults => return self.export(hwnd),
                CommandId::CancelExport => self.model.borrow_mut().cancel_export(),
                CommandId::Copy => match focus {
                    Focus::SearchBox { .. } => return controls.search.copy(),
                    Focus::Value { .. } => {
                        if let Some(value) = self.focused_value() {
                            value.copy();
                        }
                        return;
                    }
                    Focus::List | Focus::RangeFilter | Focus::Other => {
                        return self.copy(hwnd, ClipboardFormat::Files);
                    }
                },
                CommandId::CopyAsPath => return self.copy(hwnd, ClipboardFormat::Paths),
                CommandId::CopyName => return self.copy(hwnd, ClipboardFormat::Names),
                CommandId::SelectAll => match focus {
                    Focus::SearchBox { .. } => return controls.search.select_all(),
                    Focus::Value { .. } => {
                        if let Some(value) = self.focused_value() {
                            value.select_all();
                        }
                        return;
                    }
                    Focus::List | Focus::RangeFilter | Focus::Other => {
                        return controls.list.select_all();
                    }
                },
                CommandId::Find => {
                    controls.search.hwnd().focus();
                    return controls.search.select_all();
                }
                CommandId::Refresh => return self.search(hwnd),
                CommandId::SearchSyntax => {
                    self.open_dialog(hwnd, DialogId::SearchSyntax, &SearchSyntax);
                    return;
                }
                CommandId::SearchInThisFolder => self.model.borrow_mut().search_in_this_folder(),
                CommandId::Filters => self.model.borrow_mut().toggle_filter_pane(),
                CommandId::Inspector => self.model.borrow_mut().toggle_inspector(),
                CommandId::ClearFilters => self.model.borrow_mut().clear_filters(),
                CommandId::Settings => return self.open_settings(hwnd),
                CommandId::ShowWindow => return self.show_window(hwnd),
                CommandId::Columns | CommandId::Language | CommandId::Match | CommandId::SortBy => {
                    return;
                }
            },
            MenuItem::Column(column) => self.model.borrow_mut().toggle_column(column),
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
            MenuItem::Language(place) => {
                self.set_language(hwnd, LanguageChoice::at(place));
                self.save_settings(hwnd, true);
                return;
            }
        };
        self.apply(hwnd, accepted);
    }

    /// Runs the search that the text and the controls ask for now.
    fn search(&self, hwnd: Hwnd) {
        hwnd.kill_timer(timer::SEARCH);
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: jiff::tz::TimeZone::system(),
        };
        let location = self.model.borrow().location().map(str::to_owned);
        let scope = match location {
            Some(folder) if gozler_client::folder_is_missing(&folder) => ScopeCheck::Missing,
            Some(_) | None => ScopeCheck::Exists,
        };
        let accepted = self.model.borrow_mut().start_search(&start, scope);
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
                super::write_log(
                    &self.settings_file.with_file_name("ui-error.log"),
                    &format!("The {dialog:?} dialog did not open: {error}"),
                );
                None
            }
        }
    }

    /// Changes the filters to what `change` makes of them.
    fn change_filters(&self, hwnd: Hwnd, change: impl FnOnce(&mut Filters)) {
        let mut filters = self.model.borrow().filters().clone();
        change(&mut filters);
        let accepted = self.model.borrow_mut().set_filters(filters);
        self.apply(hwnd, accepted);
    }

    /// Asks for a folder with the Windows folder picker and sets Location
    /// to it; Cancel keeps the Location (DESIGN.md: Filter pane).
    fn choose_folder(&self, hwnd: Hwnd) {
        let start = self.model.borrow().location().map(str::to_owned);
        let Some(folder) = win::shell::pick_folder(hwnd, start.as_deref()) else {
            return;
        };
        let accepted = self.model.borrow_mut().choose_folder(folder);
        self.apply(hwnd, accepted);
    }

    /// Opens the custom range dialog of `range` when its list shows
    /// Custom…; OK sets the range, and Cancel shows the previous choice
    /// again (DESIGN.md: Filter pane).
    fn choose_custom(&self, hwnd: Hwnd, range: RangeFilter) {
        let custom = {
            let model = self.model.borrow();
            let filters = model.filters();
            match range {
                RangeFilter::Date => date_items(filters.date).len(),
                RangeFilter::Size => size_items(filters.size).len(),
            }
        };
        let list = self.created().controls.filters.range(range);
        if list.selected() != Some(custom - 1) {
            return;
        }
        match range {
            RangeFilter::Date => {
                let dialog = {
                    let model = self.model.borrow();
                    let shown = match model.filters().date {
                        DateModified::Custom(range) => Some(range),
                        _ => None,
                    };
                    CustomDate::new(model.strings(), shown)
                };
                if self.open_dialog(hwnd, DialogId::CustomDate, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(hwnd, |f| f.date = DateModified::Custom(chosen));
                }
            }
            RangeFilter::Size => {
                let dialog = {
                    let model = self.model.borrow();
                    let shown = match model.filters().size {
                        SizeFilter::Custom(range) => Some(range),
                        _ => None,
                    };
                    let numbers = self.numbers.borrow();
                    CustomSize::new(model.strings(), numbers.decimal_separator(), shown)
                };
                if self.open_dialog(hwnd, DialogId::CustomSize, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(hwnd, |f| f.size = SizeFilter::Custom(chosen));
                }
            }
        }
        // After Cancel, the list shows Custom… and the filter is the one
        // before it.
        self.update_filters();
    }

    fn ask_status(&self) {
        let fetch = self.model.borrow_mut().status_needed();
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    fn start(&self, fetch: Fetch) {
        self.created().requests.start(fetch);
    }

    /// Asks for the chunk that the rows on screen still lack, so every
    /// chunk request comes from the rows visible now (DESIGN.md: Rows on
    /// demand).
    fn fetch_visible(&self) {
        let visible = self.created().controls.list.visible_rows();
        let fetch = self.model.borrow_mut().rows_needed(visible);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// Updates the controls for what a reply or an input changed, and starts
    /// the request it makes necessary.
    fn apply(&self, hwnd: Hwnd, accepted: Accepted) {
        let created = self.created();
        let controls = &created.controls;
        match accepted.then {
            Some(Then::Fetch(fetch)) => self.start(fetch),
            Some(Then::SearchAgain) => self.search(hwnd),
            Some(Then::SearchSoon) => hwnd.set_timer(timer::SEARCH, SEARCH_DELAY_MS),
            Some(Then::ReadSelectionSoon) => {
                created.requests.cancel(Purpose::Selection);
                hwnd.set_timer(timer::SELECTION, SELECTION_DELAY_MS);
            }
            Some(Then::Cancel(purpose)) => created.requests.cancel(purpose),
            None => {}
        }
        let mut failed_export = None;
        match accepted.change {
            Change::Nothing | Change::Choice | Change::FolderCounted => {}
            Change::Filters => self.update_filters(),
            Change::Panes => {
                let panes = self.model.borrow().panes();
                // A hidden control keeps the focus but takes no key.
                let hidden = |focused: Hwnd| {
                    !panes.filters && controls.filters.pane.is_parent_of(focused)
                        || !panes.inspector && controls.inspector.contains(focused)
                };
                if Hwnd::focused().is_some_and(hidden) {
                    controls.search.hwnd().focus();
                }
                self.layout(hwnd);
                self.update_status();
            }
            Change::Typed => hwnd.set_text(&captions::title(&self.model.borrow())),
            Change::ColumnShown { index } => {
                let (column, header) = {
                    let model = self.model.borrow();
                    let column = model.columns()[index];
                    (column, captions::header(model.strings(), column))
                };
                controls.list.insert_column(
                    index,
                    &ListColumn {
                        header: &header,
                        width: layout::column_width(column, None, hwnd.dpi()),
                        right_aligned: column.right_aligned(),
                    },
                );
            }
            Change::ColumnHidden { index } => controls.list.delete_column(index),
            Change::Status => {
                self.update_status();
                self.update_locations();
            }
            Change::Searching | Change::Selection | Change::Export => {
                self.update_status();
            }
            Change::NewSearch { count } => {
                // A new search shows its results from the top, with nothing
                // selected (DESIGN.md: T2).
                controls.list.select_only(None);
                controls.list.set_count(count);
                if count > 0 {
                    controls.list.ensure_visible(0);
                }
                self.update_status();
                self.update_message();
                self.fetch_visible();
            }
            Change::Rows { rows, count } => {
                if controls.list.count() == count {
                    controls.list.redraw_rows(rows);
                } else {
                    controls.list.set_count(count);
                    self.update_message();
                }
                self.update_status();
                self.fetch_visible();
            }
            Change::Message => {
                controls.list.select_only(None);
                controls.list.set_count(0);
                self.update_status();
                self.update_message();
            }
            Change::SelectionRead { rows } => {
                controls.list.redraw_rows(rows);
                self.update_status();
            }
            Change::ExportFailed { path, error } => {
                self.update_status();
                failed_export = Some((path, error));
            }
            Change::Settings => {
                let (menu_bar, shortcuts) = {
                    let model = self.model.borrow();
                    let shortcuts = &model.choices().shortcuts;
                    let menu_bar =
                        commands::render(&commands::menu_bar(), model.strings(), shortcuts);
                    (menu_bar, shortcuts.clone())
                };
                let _ = win::menu::replace_menu_bar(hwnd, &menu_bar);
                if let Ok(table) = accelerator_table(&shortcuts) {
                    self.accelerators.replace(Rc::new(table));
                }
                self.update_tray(hwnd);
            }
            Change::TotalCommander(found) => {
                self.total_commander.replace(Some(found));
                if let Some(dialog) = self.settings_dialog.get() {
                    win::dialog::post_reply(dialog);
                }
            }
        }
        self.update_bound_controls();
        self.update_inspector(hwnd);
        self.update_poll(hwnd);
        // Last, because the message box runs its own message loop.
        if let Some((path, error)) = failed_export {
            self.show_export_failure(hwnd, &path, &error);
        }
    }

    /// Starts or stops the one status timer as the model says (DESIGN.md:
    /// Requests).
    fn update_poll(&self, hwnd: Hwnd) {
        let polls = self.model.borrow().polls_status();
        if polls != self.polling.replace(polls) {
            if polls {
                hwnd.set_timer(timer::STATUS, STATUS_POLL_MS);
            } else {
                hwnd.kill_timer(timer::STATUS);
            }
        }
    }

    /// Accepts every reply that has arrived, and the newer release that the
    /// update check found.
    fn on_wake(&self, hwnd: Hwnd) {
        for fetched in self.created().requests.drain() {
            let accepted = self.model.borrow_mut().accept(fetched);
            self.apply(hwnd, accepted);
        }
        let newer = self
            .update
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .newer
            .take();
        if let Some(version) = newer {
            let accepted = self.model.borrow_mut().set_newer_release(version);
            self.apply(hwnd, accepted);
        }
    }

    /// A control's notification in `WM_COMMAND`.
    fn on_control(&self, hwnd: Hwnd, event: ControlEvent) {
        let controls = &self.created().controls;
        if controls.search.focused(event) {
            // The click places the caret after the focus arrives, so the
            // text is selected once the click is handled.
            let select = self.model.borrow().choices().select_on_click;
            if select && win::window::left_button_down() && !self.select_search.replace(true) {
                hwnd.post(DEFERRED);
            }
            return;
        }
        let matching = || self.model.borrow().matching();
        let accepted = if controls.search.changed(event) {
            let text = controls.search.text();
            self.model.borrow_mut().set_typed_search(text)
        } else if controls.match_mode.selection_changed(event) {
            let Some(mode) = controls
                .match_mode
                .selected()
                .and_then(|i| MatchMode::ALL.get(i).copied())
            else {
                return;
            };
            let choice = Matching { mode, ..matching() };
            self.model.borrow_mut().set_matching(choice)
        } else if controls.match_case.clicked(event) {
            let choice = Matching {
                case: controls.match_case.is_checked(),
                ..matching()
            };
            self.model.borrow_mut().set_matching(choice)
        } else if controls.match_path.clicked(event) {
            let choice = Matching {
                path: controls.match_path.is_checked(),
                ..matching()
            };
            self.model.borrow_mut().set_matching(choice)
        } else if let Some(index) = controls
            .message_buttons
            .iter()
            .position(|button| button.clicked(event))
        {
            let button = self.model.borrow().list_buttons().get(index).cloned();
            match button {
                Some(ListButton::Fix(fix)) => run_fix(hwnd, &fix),
                Some(ListButton::ClearFilters) => {
                    self.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
                }
                Some(ListButton::SearchEverywhere) => {
                    let accepted = self.model.borrow_mut().search_everywhere();
                    self.apply(hwnd, accepted);
                }
                Some(ListButton::ChooseFolder) => self.choose_folder(hwnd),
                None => {}
            }
            return;
        } else if let Some(index) = controls
            .inspector
            .buttons
            .iter()
            .position(|button| button.clicked(event))
        {
            // The button runs its menu item's command (DESIGN.md:
            // window rule 9).
            self.read_list_selection(hwnd);
            return self.run(hwnd, MenuItem::Run(INSPECTOR_BUTTONS[index].0));
        } else {
            return self.on_filter(hwnd, event);
        };
        self.apply(hwnd, accepted);
    }

    /// A notification of a control in the filter pane.
    fn on_filter(&self, hwnd: Hwnd, event: ControlEvent) {
        let pane = &self.created().controls.filters;
        if pane.clear.clicked(event) {
            self.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
        } else if pane.kind.selection_changed(event) {
            if let Some(kind) = pane.kind.selected().and_then(|i| Kind::ALL.get(i)) {
                self.change_filters(hwnd, |f| f.kind = *kind);
            }
        } else if pane.location.selection_changed(event) {
            let location = {
                let model = self.model.borrow();
                let items = model.location_items();
                pane.location
                    .selected()
                    .and_then(|i| items.get(i))
                    .map(|item| item.folder().map(str::to_owned))
            };
            if let Some(location) = location {
                self.change_filters(hwnd, |f| f.location = location);
            }
        } else if pane.choose_folder.clicked(event) {
            self.choose_folder(hwnd);
        } else if pane.only_directly.clicked(event) {
            let checked = pane.only_directly.is_checked();
            self.change_filters(hwnd, |f| f.only_directly = checked);
        } else if pane.date.selection_changed(event) {
            let date = self.model.borrow().filters().date;
            // Custom… opens its dialog only when the list closes on it, so
            // the arrow keys pass it by and keep the previous filter.
            if let Some(RangeItem::Choice(date)) = pane
                .date
                .selected()
                .and_then(|i| date_items(date).get(i).copied())
            {
                self.change_filters(hwnd, |f| f.date = date);
            }
        } else if pane.size.selection_changed(event) {
            let size = self.model.borrow().filters().size;
            if let Some(RangeItem::Choice(size)) = pane
                .size
                .selected()
                .and_then(|i| size_items(size).get(i).copied())
            {
                self.change_filters(hwnd, |f| f.size = size);
            }
        } else if let Some(range) = pane.range_of(event.sender()) {
            let list = pane.range(range);
            if let Some(closing) = list.closing(event) {
                self.range_closing.set(Some((range, closing)));
            } else if list.closed(event)
                && self.range_closing.take() == Some((range, ListClosing::Chosen))
            {
                self.range_closed.set(Some(range));
                hwnd.post(DEFERRED);
            }
        } else if let Some(index) = pane
            .attributes
            .iter()
            .position(|(_, list)| list.selection_changed(event))
        {
            let choice = pane.attributes[index]
                .1
                .selected()
                .and_then(|i| AttributeChoice::ALL.get(i).copied());
            if let Some(choice) = choice {
                self.change_filters(hwnd, |f| f.attributes[index] = choice);
            }
        }
    }

    /// The arrow on the sorted column's header (DESIGN.md: Results list).
    fn show_sort(&self) {
        let (count, sorted) = {
            let model = self.model.borrow();
            let sort = model.sort();
            (
                model.columns().len(),
                model
                    .columns()
                    .iter()
                    .position(|c| c.sort_key() == Some(sort.key))
                    .map(|index| (index, sort.dir == SortDir::Asc)),
            )
        };
        self.created().controls.list.show_sort(count, sorted);
    }

    /// A context menu of `groups` at `point`; the chosen item runs after
    /// the menu closes (DESIGN.md: window rule 9).
    fn context_menu(
        &self,
        hwnd: Hwnd,
        groups: &[Vec<MenuItem>],
        point: (i32, i32),
        default: Option<MenuItem>,
    ) {
        let items: Vec<MenuItem> = groups.iter().flatten().copied().collect();
        let states = self.item_states(&items);
        let nodes = commands::render_popup(groups, self.model.borrow().strings());
        let default = default.map(MenuItem::id);
        let chosen = win::menu::Menu::track_popup(hwnd, &nodes, point, &states, default);
        if let Some(item) = chosen.and_then(MenuItem::from_id) {
            self.run(hwnd, item);
        }
    }

    /// The context menu of a row, at `point`, or at the focused row for the
    /// Menu key and Shift+F10. A right-click below the last row shows none
    /// (DESIGN.md: Results list).
    fn row_menu(&self, hwnd: Hwnd, point: Option<(i32, i32)>) {
        let list = self.created().controls.list;
        let point = match point {
            Some(point) => list.row_at(point).map(|_| point),
            None => list.focused_row_point(),
        };
        let Some(point) = point else { return };
        // The right-click selected its row a moment ago.
        self.read_list_selection(hwnd);
        let open = MenuItem::Run(CommandId::Open);
        self.context_menu(hwnd, &commands::row_menu(), point, Some(open));
    }

    /// Reads the list's selection, if it changed since it was last read
    /// (DESIGN.md: The selection).
    fn read_list_selection(&self, hwnd: Hwnd) {
        if !self.selection_pending.replace(false) {
            return;
        }
        let positions = self.created().controls.list.selected();
        let accepted = self.model.borrow_mut().set_selection(positions);
        self.apply(hwnd, accepted);
    }

    fn on_list_notify(&self, hwnd: Hwnd, notify: ListNotify) -> Option<LRESULT> {
        match notify {
            ListNotify::GetDispInfo(mut info) => {
                self.fill_cell(&mut info);
                Some(LRESULT(0))
            }
            ListNotify::CacheHint(rows) => {
                let fetch = self.model.borrow_mut().rows_needed(rows);
                if let Some(fetch) = fetch {
                    self.start(fetch);
                }
                Some(LRESULT(0))
            }
            ListNotify::ColumnClick(index) => {
                let accepted = {
                    let mut model = self.model.borrow_mut();
                    let sort = model
                        .columns()
                        .get(index)
                        .and_then(|column| model.header_sort(*column));
                    sort.map(|sort| model.sort_by(sort))
                };
                if let Some(accepted) = accepted {
                    self.apply(hwnd, accepted);
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(draw) => Some(CustomDraw::answer(match draw.stage() {
                DrawStage::Paint => DrawAnswer::EachRow,
                DrawStage::Row => DrawAnswer::EachCell,
                DrawStage::Cell { row, column } => self.draw_cell(&draw, hwnd.dpi(), row, column),
                DrawStage::Other => DrawAnswer::Default,
            })),
            ListNotify::SelectionChanged => {
                if !self.selection_pending.replace(true) {
                    hwnd.post(DEFERRED);
                }
                None
            }
            ListNotify::Activated => {
                self.read_list_selection(hwnd);
                self.run(hwnd, MenuItem::Run(CommandId::Open));
                None
            }
            ListNotify::Other => None,
        }
    }

    /// Formats a cell when the list asks for it, so a redraw shows the
    /// current language and regional format (DESIGN.md: window rule 12). This
    /// block borrows the model while it formats, which makes no Win32 call; an
    /// icon not asked for yet is looked up after the borrow ends.
    fn fill_cell(&self, info: &mut DispInfo) {
        let (text, icon) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let column = model.columns().get(info.column()).copied();
            match (column, model.results().row(info.row())) {
                (Some(column), Some(row)) => (
                    info.wants_text()
                        .then(|| crate::core::text::cell(model.strings(), &*numbers, column, row)),
                    crate::core::text::path_part(column, &row.path)
                        .filter(|_| column == Column::Name)
                        .map(|name| (row.path[name].to_owned(), row.is_dir)),
                ),
                _ => (None, None),
            }
        };
        if info.wants_text() {
            info.set_text(text.as_deref().unwrap_or_default());
        }
        if info.wants_image() {
            info.set_image(icon.map(|(name, is_folder)| self.icons.index(&name, is_folder)));
        }
    }

    /// Draws a Name or Path cell whose text the search matched, with the
    /// matched parts in bold (DESIGN.md: Results list); the list draws every
    /// other cell.
    fn draw_cell(&self, draw: &CustomDraw, dpi: Dpi, row: u64, column: usize) -> DrawAnswer {
        let cell = {
            let model = self.model.borrow();
            let column = model.columns().get(column).copied();
            let Some(item) = model.results().row(row) else {
                return DrawAnswer::Default;
            };
            let Some(span) = column.and_then(|c| crate::core::text::path_part(c, &item.path))
            else {
                return DrawAnswer::Default;
            };
            let bold = within(&model.results().matched_parts(item), &span);
            if bold.is_empty() {
                return DrawAnswer::Default;
            }
            let icon = (column == Some(Column::Name)).then_some(item.is_dir);
            (item.path[span].to_owned(), bold, icon)
        };
        let (text, bold, icon) = cell;
        let icon = icon.map(|is_folder| self.icons.index(&text, is_folder));
        let created = self.created();
        let fonts = created.fonts.borrow();
        draw.draw_cell(
            &text,
            &bold,
            (&fonts.ui, &fonts.bold),
            created.images.zip(icon),
            layout::cell_text_inset(column == 0, dpi),
        );
        DrawAnswer::Drawn
    }

    /// A key that the control `target` gives a meaning (DESIGN.md:
    /// window rule 8). Returns whether the window took it.
    fn on_key(&self, hwnd: Hwnd, target: Hwnd, keystroke: Keystroke) -> bool {
        let Some(action) = commands::control_key(self.focus_of(Some(target)), keystroke) else {
            return false;
        };
        let controls = &self.created().controls;
        match action {
            KeyAction::Run(command) => {
                self.read_list_selection(hwnd);
                self.run(hwnd, MenuItem::Run(command));
            }
            // The focus moves to the first row (DESIGN.md: Search box).
            KeyAction::FocusList => {
                let count = self.model.borrow().results().count();
                controls.list.hwnd().focus();
                if count > 0 {
                    controls.list.select_only(Some(0));
                    controls.list.ensure_visible(0);
                }
            }
            KeyAction::SearchNow => self.search(hwnd),
            KeyAction::ClearSearch => controls.search.hwnd().set_text(""),
            KeyAction::ChooseCustom => {
                if let Some(range) = controls.filters.range_of(target) {
                    self.choose_custom(hwnd, range);
                }
            }
        }
        true
    }

    /// Whether `point`, in client coordinates, is on the splitter above the
    /// inspector.
    fn on_splitter(&self, point: (i32, i32)) -> bool {
        self.created()
            .layout
            .borrow()
            .as_ref()
            .and_then(|l| l.inspector.as_ref())
            .is_some_and(|inspector| inspector.splitter.contains(point))
    }

    /// Whether `point`, in screen coordinates, is on the list's header.
    fn on_header(&self, point: (i32, i32)) -> bool {
        self.created()
            .controls
            .list
            .header()
            .screen_rect()
            .contains(point)
    }

    /// The selected entries.
    fn selected_entries(&self) -> Vec<Selected> {
        let model = self.model.borrow();
        model
            .selected_files()
            .unwrap_or_default()
            .iter()
            .map(|file| Selected {
                path: wide_path(file),
                is_folder: file.is_dir,
            })
            .collect()
    }

    /// Opens each selected entry with the Open folders or Open files
    /// command, or as File Explorer does when it is empty (DESIGN.md: C1).
    fn open_selected(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_entries());
        let (folders, files) = {
            let model = self.model.borrow();
            let choices = model.choices();
            (choices.open_folders.clone(), choices.open_files.clone())
        };
        for entry in &present {
            let command = if entry.is_folder { &folders } else { &files };
            match open::expand(command, &String::from_utf16_lossy(&entry.path)) {
                Some(line) => {
                    if !self.run_open_command(hwnd, &line) {
                        return;
                    }
                }
                None => shell::open_entry(hwnd, &entry.path),
            }
        }
        self.report_missing(hwnd, missing);
    }

    /// Opens the folder of each selected entry with the entries selected,
    /// one File Explorer window for each folder, or runs the Open file
    /// location command for each entry (DESIGN.md: C2).
    fn show_selected(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_entries());
        let command = self.model.borrow().choices().open_file_location.clone();
        if command.trim().is_empty() {
            let mut folders: Vec<(Vec<u16>, Vec<Vec<u16>>)> = Vec::new();
            for Selected { path, .. } in present {
                let folder = folder_of(&path);
                match folders.iter_mut().find(|(f, _)| *f == folder) {
                    Some((_, entries)) => entries.push(path),
                    None => folders.push((folder, vec![path])),
                }
            }
            for (folder, entries) in &folders {
                shell::show_in_folder(folder, entries);
            }
        } else {
            for entry in &present {
                if let Some(line) = open::expand(&command, &String::from_utf16_lossy(&entry.path))
                    && !self.run_open_command(hwnd, &line)
                {
                    return;
                }
            }
        }
        self.report_missing(hwnd, missing);
    }

    /// Runs an open command of Settings > General, and answers whether it
    /// started; one that cannot start says why, and the rest of the
    /// selection is not tried, because it would fail the same way.
    fn run_open_command(&self, hwnd: Hwnd, line: &CommandLine) -> bool {
        let Err(error) = shell::run_command(hwnd, &line.program, &line.parameters) else {
            return true;
        };
        let text = self.model.borrow().strings().format(
            "message.open_failed",
            &[&line.program, &win::error_text(&error)],
        );
        self.error_box(hwnd, &text);
        false
    }

    /// The shell's Properties of the selected entry (DESIGN.md: C3).
    fn show_properties(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_entries());
        for entry in &present {
            shell::properties(hwnd, &entry.path);
        }
        self.report_missing(hwnd, missing);
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
    /// C10). The block is filled while the model is borrowed, which no call
    /// that sends a message interrupts, and given to the clipboard after.
    fn copy(&self, hwnd: Hwnd, format: ClipboardFormat) {
        let block = {
            let model = self.model.borrow();
            let Some(files) = model.selected_files() else {
                return;
            };
            ClipboardBlock::new(&ClipboardData::new(format, files))
        };
        let Some(block) = block else { return };
        if block.set(hwnd).is_err() {
            let text = self
                .model
                .borrow()
                .strings()
                .text("message.clipboard_busy")
                .to_owned();
            self.error_box(hwnd, &text);
        }
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
        let order = self.created().controls.list.column_order(columns);
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

    /// Brings the window forward with the search box focused and its text
    /// selected, so the person types the next search at once (DESIGN.md: Window
    /// lifetime, C24). While a dialog is open, the dialog keeps the focus.
    fn show_window(&self, hwnd: Hwnd) {
        if self.unshown.replace(false) {
            let placement = self.model.borrow().placement();
            win::window::show(hwnd, placement);
        }
        hwnd.bring_to_front();
        if !hwnd.is_enabled() {
            return;
        }
        let search = self.created().controls.search;
        search.hwnd().focus();
        search.select_all();
    }

    /// Closing hides the window while Gozler keeps running in the
    /// notification area, the first time with a notification that says so,
    /// and exits otherwise (DESIGN.md: Window lifetime).
    fn close(&self, hwnd: Hwnd) {
        if !self.model.borrow().choices().keep_running {
            return self.exit(hwnd);
        }
        hwnd.hide();
        if !self.model.borrow_mut().first_hide() {
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
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.notify(&title, &text);
        }
        self.save_settings(hwnd, true);
    }

    /// Saves the settings and ends Gozler (DESIGN.md: C7, T14).
    fn exit(&self, hwnd: Hwnd) {
        self.save_settings(hwnd, true);
        hwnd.destroy();
    }

    /// Adds the tray icon while Gozler keeps running in the notification
    /// area, and removes it otherwise (DESIGN.md: Window lifetime).
    fn update_tray(&self, hwnd: Hwnd) {
        let keeps_running = self.model.borrow().choices().keep_running;
        if keeps_running == self.tray.borrow().is_some() {
            return;
        }
        let tray = if keeps_running {
            let tip = self
                .model
                .borrow()
                .strings()
                .text("window.title")
                .to_owned();
            TrayIcon::add(hwnd, IconId::Gozler.id(), &tip).ok()
        } else {
            None
        };
        self.tray.replace(tray);
    }

    /// A click on the tray icon, or its menu. While a dialog is open, the
    /// menu shows the window and its dialog instead, because Exit would end
    /// the window under the dialog.
    fn on_tray(&self, hwnd: Hwnd, event: TrayEvent) {
        match event {
            TrayEvent::Menu { point } if hwnd.is_enabled() => {
                let nodes =
                    commands::render_popup(&commands::tray_menu(), self.model.borrow().strings());
                let show = MenuItem::Run(CommandId::ShowWindow);
                let chosen = tray::track_menu(hwnd, &nodes, point, Some(show.id()));
                if let Some(item) = chosen.and_then(MenuItem::from_id) {
                    self.run(hwnd, item);
                }
            }
            TrayEvent::Show | TrayEvent::Menu { .. } => self.show_window(hwnd),
        }
    }

    /// Registers the saved shortcut to show Gozler. One that Windows
    /// refuses is said the first time the window shows, because a message
    /// at sign-in would come before the person looks.
    fn register_saved_shortcut(&self, hwnd: Hwnd) {
        let Some(saved) = self.model.borrow().choices().show_shortcut else {
            return;
        };
        match shortcut::register(hwnd, saved, self.show_message) {
            Ok(registration) => {
                self.shortcut.replace(Some(registration));
            }
            Err(error) => {
                self.shortcut_failure.replace(Some(error));
            }
        }
    }

    /// What the window says when Windows refused `shortcut`: in Settings,
    /// or at start, when the saved shortcut no longer works.
    fn shortcut_message(
        &self,
        shortcut: ShowShortcut,
        error: &RegisterError,
        at_start: bool,
    ) -> String {
        let model = self.model.borrow();
        let strings = model.strings();
        let keys = shortcut.text(strings);
        match error {
            RegisterError::Taken if at_start => strings.format("message.shortcut_taken", &[&keys]),
            RegisterError::Taken => strings.format("settings_keyboard.shortcut_taken", &[&keys]),
            RegisterError::Failed(error) => strings.format(
                "settings_keyboard.shortcut_failed",
                &[&keys, error.message().trim()],
            ),
        }
    }

    /// Says why the saved shortcut does not work, once the window shows.
    fn show_shortcut_failure(&self, hwnd: Hwnd) {
        if !hwnd.is_visible() {
            return;
        }
        let Some(error) = self.shortcut_failure.take() else {
            return;
        };
        let saved = self.model.borrow().choices().show_shortcut;
        if let Some(saved) = saved {
            let text = self.shortcut_message(saved, &error, true);
            self.error_box(hwnd, &text);
        }
    }

    /// Asks GitHub for the latest release on a thread of its own, when
    /// Check for updates is on and the last check is a day old (DESIGN.md: The
    /// General page).
    fn check_for_updates(&self, hwnd: Hwnd) {
        let now = jiff::Timestamp::now().as_second();
        if !self.model.borrow_mut().update_check_due(now) {
            return;
        }
        let found = Arc::clone(&self.update);
        let waker = Waker::new(hwnd);
        std::thread::spawn(move || {
            let version = gozler_core::escompat::version();
            let newer = win::update::get(
                update::LATEST_RELEASE_HOST,
                update::LATEST_RELEASE_PATH,
                &format!("Gozler/{version}"),
            )
            .and_then(|reply| update::newer_release(&reply, version));
            let Some(newer) = newer else { return };
            let mut found = found.lock().unwrap_or_else(PoisonError::into_inner);
            if found.window_open {
                found.newer = Some(newer);
                waker.wake();
            }
        });
    }

    /// A click on status part 4 opens the release page while the part
    /// names a newer release (DESIGN.md: The General page).
    fn on_status_click(&self) {
        let shows_update = captions::shows_update(&self.model.borrow());
        if shows_update {
            shell::open(update::RELEASES_PAGE);
        }
    }

    /// File > Settings…: the dialog edits a copy of the settings, which OK
    /// and Apply give back through [`MainWindow::apply_settings`] (DESIGN.md:
    /// Settings dialog).
    fn open_settings(&self, hwnd: Hwnd) {
        let (choices, strings) = {
            let model = self.model.borrow();
            (model.choices().clone(), model.strings().clone())
        };
        let values = SettingsValues {
            choices,
            start_at_sign_in: shell::starts_at_sign_in(),
            search_with_gozler: shell::shows_search_with(),
        };
        let dialog = SettingsDialog::new(self, hwnd, strings, values);
        self.open_dialog(hwnd, DialogId::Settings, &dialog);
        self.settings_dialog.set(None);
        self.total_commander.replace(None);
        let accepted = self.model.borrow_mut().forget_total_commander();
        self.apply(hwnd, accepted);
        self.show_index_page(hwnd, false);
    }

    /// Applies what Settings edited, `edited`, which was `last` before; OK
    /// and Apply call it (DESIGN.md: Settings dialog). A shortcut that Windows
    /// refuses applies nothing and returns its message, so the dialog stays
    /// open. The registry is written only for a value that changed, so a page
    /// that was not touched never changes what Task Manager or File Explorer
    /// show.
    pub(super) fn apply_settings(
        &self,
        hwnd: Hwnd,
        edited: &SettingsValues,
        last: &SettingsValues,
    ) -> Result<Applied, String> {
        let shortcut = edited.choices.show_shortcut;
        if shortcut != last.choices.show_shortcut {
            // The new shortcut is registered before the old one goes, so a
            // refusal leaves the old one working.
            let registration = match shortcut {
                Some(shortcut) => Some(
                    shortcut::register(hwnd, shortcut, self.show_message)
                        .map_err(|error| self.shortcut_message(shortcut, &error, false))?,
                ),
                None => None,
            };
            self.shortcut.replace(registration);
            self.shortcut_failure.replace(None);
        }
        let mut failures = Vec::new();
        let verb = self
            .model
            .borrow()
            .strings()
            .text("settings_general.verb")
            .to_owned();
        let values = SettingsValues {
            choices: edited.choices.clone(),
            start_at_sign_in: self.write_registry(
                (edited.start_at_sign_in, last.start_at_sign_in),
                "settings_general.sign_in",
                &mut failures,
                |program| shell::set_start_at_sign_in(edited.start_at_sign_in, program),
            ),
            search_with_gozler: self.write_registry(
                (edited.search_with_gozler, last.search_with_gozler),
                "settings_general.search_with",
                &mut failures,
                |program| shell::set_search_with(edited.search_with_gozler, program, &verb),
            ),
        };
        let accepted = self.model.borrow_mut().set_choices(values.choices.clone());
        self.apply(hwnd, accepted);
        if edited.choices.language != last.choices.language {
            self.set_language(hwnd, edited.choices.language.clone());
        }
        self.save_settings(hwnd, true);
        Ok(Applied { values, failures })
    }

    /// Writes a setting whose one copy is in the registry with `write`, if
    /// it changed from `was` to `on`, and returns the value now in effect.
    /// A failed write keeps `was` and adds its message, which names the
    /// setting by the check box `label`, to `failures`.
    fn write_registry(
        &self,
        (on, was): (bool, bool),
        label: &str,
        failures: &mut Vec<String>,
        write: impl FnOnce(&Path) -> io::Result<()>,
    ) -> bool {
        if on == was {
            return on;
        }
        let Err(error) = std::env::current_exe().and_then(|program| write(&program)) else {
            return on;
        };
        let model = self.model.borrow();
        let strings = model.strings();
        failures.push(strings.format(
            "message.registry_failed",
            &[
                &without_access_key(strings.text(label)),
                &win::error_text(&error),
            ],
        ));
        was
    }

    /// Settings shows or hides its Index page, which follows the service's
    /// state while it shows (DESIGN.md: The Index page).
    pub(super) fn show_index_page(&self, hwnd: Hwnd, shown: bool) {
        self.model.borrow_mut().show_index_page(shown);
        self.update_poll(hwnd);
        if shown {
            self.ask_status();
        }
    }

    /// What the Index page shows of the service's latest state.
    pub(super) fn index_captions(&self) -> IndexCaptions {
        let prefixes: Vec<String> = {
            let model = self.model.borrow();
            let (status, _) = model.service();
            status
                .map(|status| {
                    let volumes = status.volumes.iter();
                    volumes
                        .map(|volume| volume.mounted().display_prefix().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        // Windows reads the labels with no borrow held (DESIGN.md:
        // window rule 1).
        let labels: Vec<(String, Option<String>)> = prefixes
            .into_iter()
            .map(|prefix| {
                let label = shell::volume_label(&prefix);
                (prefix, label)
            })
            .collect();
        let label = |prefix: &str| {
            labels
                .iter()
                .find(|(p, _)| p == prefix)
                .and_then(|(_, label)| label.clone())
        };
        IndexCaptions::new(&self.model.borrow(), &*self.numbers.borrow(), &label)
    }

    /// Use Total Commander: searches the index for Total Commander, whose
    /// answer goes to the Settings dialog `dialog` (DESIGN.md: The General
    /// page).
    pub(super) fn find_total_commander(&self, dialog: Hwnd) {
        self.settings_dialog.set(Some(dialog));
        let fetch = self.model.borrow_mut().find_total_commander();
        self.start(fetch);
    }

    /// Use Total Commander's answer, once it arrived.
    pub(super) fn take_total_commander(&self) -> Option<Result<Option<String>, DaemonFailure>> {
        self.total_commander.take()
    }

    /// The shown columns in the header's order, with their widths in DIP.
    fn saved_columns(&self) -> Vec<SavedColumn> {
        let created = self.created();
        let list = created.controls.list;
        let dpi = created.metrics.get().dpi;
        let columns = self.model.borrow().columns().to_vec();
        list.column_order(columns.len())
            .into_iter()
            .filter_map(|index| {
                Some(SavedColumn {
                    column: *columns.get(index)?,
                    width: list.column_width(index).to_dip(dpi),
                })
            })
            .collect()
    }

    /// Saves the window's placement, match controls, sort and columns with
    /// the settings. A failure is shown once per session, and not while
    /// Windows ends the session.
    fn save_settings(&self, hwnd: Hwnd, report_failure: bool) {
        let placement = hwnd.placement();
        let columns = self.saved_columns();
        let saved =
            self.model
                .borrow_mut()
                .save_settings(&self.settings_file, placement, Some(columns));
        let Err(error) = saved else { return };
        if !report_failure || self.settings_failure_shown.replace(true) {
            return;
        }
        let text = self.model.borrow().strings().format(
            "message.settings_not_saved",
            &[
                &self.settings_file.display().to_string(),
                &win::error_text(&error),
            ],
        );
        self.error_box(hwnd, &text);
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

/// The parts of `ranges` inside `span`, from `span`'s start.
fn within(ranges: &[Range<usize>], span: &Range<usize>) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(span.start);
            let end = range.end.min(span.end);
            (start < end).then(|| start - span.start..end - span.start)
        })
        .collect()
}

/// A selected entry, by its path as Windows names it.
struct Selected {
    path: Vec<u16>,
    is_folder: bool,
}

/// The `entries` that still exist, and the first path that does not. An
/// action on the selection acts on every entry it can (DESIGN.md: The
/// selection).
fn present(entries: Vec<Selected>) -> (Vec<Selected>, Option<Vec<u16>>) {
    let mut missing = None;
    let present = entries
        .into_iter()
        .filter(|entry| {
            let exists = std::fs::symlink_metadata(OsString::from_wide(&entry.path)).is_ok();
            if !exists {
                missing.get_or_insert_with(|| entry.path.clone());
            }
            exists
        })
        .collect();
    (present, missing)
}

/// The accelerator table of the command shortcuts `shortcuts`.
fn accelerator_table(shortcuts: &CommandShortcuts) -> windows::core::Result<Accelerators> {
    let table: Vec<_> = commands::accelerators(shortcuts)
        .into_iter()
        .map(|(shortcut, item)| (shortcut, item.id()))
        .collect();
    Accelerators::new(&table)
}

/// The folder that holds `path`, a drive's root as `C:\`.
fn folder_of(path: &[u16]) -> Vec<u16> {
    let backslash = u16::from(b'\\');
    let end = path.iter().rposition(|&u| u == backslash).unwrap_or(0);
    let mut folder = path[..end].to_vec();
    if folder.last() == Some(&u16::from(b':')) {
        folder.push(backslash);
    }
    folder
}

/// Windows' own words for why a file could not be written.
fn windows_reason(failure: WriteFailure) -> String {
    let error = match failure.os_code {
        Some(code) => std::io::Error::from_raw_os_error(code),
        None => std::io::Error::from(failure.kind),
    };
    win::error_text(&error)
}

/// Runs a failure's fix: the commands that `gozler` prints for it, in one
/// `cmd /c` as administrator, so Windows asks for permission once (DESIGN.md:
/// Daemon failures).
fn run_fix(hwnd: Hwnd, fix: &Fix) {
    match fix {
        Fix::Repair(repair) => match repair.commands() {
            Some(commands) => win::shell::run_as_administrator(
                hwnd,
                "cmd.exe",
                &format!("/c {}", commands.join(" && ")),
            ),
            None => win::shell::open(RELEASES),
        },
        Fix::OpenServices => win::shell::open("services.msc"),
    }
}

impl WindowProc for MainWindow {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Create => Some(LRESULT(if self.on_create(hwnd) { 0 } else { -1 })),
            // Every other handler does nothing until WM_CREATE has made the
            // controls, because notifications arrive while it runs.
            _ if self.created.get().is_none() => None,
            Message::Size => {
                self.layout(hwnd);
                Some(LRESULT(0))
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
            // "Select the search text when Gozler is activated" is about
            // switching from another program; closing one of Gozler's own
            // dialogs gives the focus back to the control that opened it.
            Message::Activate {
                from_another_program,
            } => {
                let search = self.created().controls.search;
                let saved = self.focus_on_activate.take();
                if from_another_program && self.model.borrow().choices().select_on_activate {
                    search.hwnd().focus();
                    search.select_all();
                } else {
                    saved.unwrap_or(search.hwnd()).focus();
                }
                if self.shortcut_failure.borrow().is_some() {
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
            Message::Notify(notification) => {
                let controls = &self.created().controls;
                if controls.status.clicked_part(&notification) == Some(INDEX_PART) {
                    self.on_status_click();
                    return Some(LRESULT(0));
                }
                let list = controls.list.notification(notification)?;
                self.on_list_notify(hwnd, list)
            }
            Message::Timer(timer::SEARCH) => {
                self.search(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(timer::STATUS) => {
                self.ask_status();
                Some(LRESULT(0))
            }
            Message::Timer(timer::SELECTION) => {
                hwnd.kill_timer(timer::SELECTION);
                let fetch = self.model.borrow_mut().read_selection();
                if let Some(fetch) = fetch {
                    self.start(fetch);
                }
                Some(LRESULT(0))
            }
            Message::Timer(_) => None,
            Message::Wake => {
                self.on_wake(hwnd);
                Some(LRESULT(0))
            }
            Message::Deferred => {
                self.read_list_selection(hwnd);
                if self.select_search.replace(false) {
                    self.created().controls.search.select_all();
                }
                if let Some(range) = self.range_closed.take() {
                    self.choose_custom(hwnd, range);
                }
                self.show_shortcut_failure(hwnd);
                Some(LRESULT(0))
            }
            Message::VerticalScroll { from, request }
                if from == self.created().controls.filters.pane =>
            {
                self.scroll_filters(|filters, position| filters.scrolled(position, request));
                Some(LRESULT(0))
            }
            Message::VerticalScroll { .. } => None,
            Message::MouseWheel { delta, point }
                if self.model.borrow().panes().filters
                    && self
                        .created()
                        .controls
                        .filters
                        .pane
                        .screen_rect()
                        .contains(point) =>
            {
                self.scroll_filters(|filters, position| {
                    filters.scrolled(position, ScrollRequest::Wheel(delta))
                });
                Some(LRESULT(0))
            }
            Message::MouseWheel { .. } => None,
            Message::FocusMoved { to } => {
                self.reveal(to);
                Some(LRESULT(0))
            }
            Message::ContextMenu {
                point: Some(point), ..
            } if self.on_header(point) => {
                self.context_menu(hwnd, &commands::header_menu(), point, None);
                Some(LRESULT(0))
            }
            Message::ContextMenu { from, point } if from == self.created().controls.list.hwnd() => {
                self.row_menu(hwnd, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { .. } => None,
            Message::KeyDown { target, keystroke } => {
                Some(LRESULT(isize::from(self.on_key(hwnd, target, keystroke))))
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
                    .and_then(|l| l.inspector.as_ref())
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
                    .and_then(|l| l.inspector.as_ref())
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
            Message::CtlColorStatic(colors) => Some(colors.window_background()),
            Message::Paint => {
                let created = self.created();
                let (dividers, icon) = {
                    let layout = created.layout.borrow();
                    let layout = layout.as_ref();
                    let icon = self.inspector_shown.borrow().1;
                    (
                        layout.map(|l| l.dividers.clone()).unwrap_or_default(),
                        created
                            .large_images
                            .zip(icon)
                            .zip(layout.and_then(|l| l.inspector.as_ref()))
                            .map(|((images, index), inspector)| (images, index, inspector.icon)),
                    )
                };
                hwnd.paint_window(&dividers, icon);
                Some(LRESULT(0))
            }
            Message::SettingChange {
                regional_format: true,
            } => {
                self.numbers.replace(WindowsNumbers::new());
                self.refresh_text(hwnd);
                None
            }
            Message::SettingChange { .. } => None,
            Message::EndSession { ending: true } => {
                self.save_settings(hwnd, false);
                Some(LRESULT(0))
            }
            Message::EndSession { .. } => None,
            Message::Close => {
                self.close(hwnd);
                Some(LRESULT(0))
            }
            // Every request thread is joined, and the update check stops
            // waking the window, while the window's handle is still valid,
            // so no wake-up reaches a reused handle (DESIGN.md: window rule 5).
            // The shortcut and the tray icon go with the window, so Windows'
            // own use of the shortcut's keys returns at once.
            Message::Destroy => {
                self.update
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .window_open = false;
                self.shortcut.replace(None);
                self.tray.replace(None);
                self.created().requests.shut_down();
                win::window::quit();
                Some(LRESULT(0))
            }
            Message::Hotkey => {
                self.show_window(hwnd);
                Some(LRESULT(0))
            }
            Message::Tray(event) => {
                self.on_tray(hwnd, event);
                Some(LRESULT(0))
            }
            // A second start with `--location`, from File Explorer's
            // "Search with Gozler" (DESIGN.md: Window lifetime).
            Message::CopyData { tag, text } if tag == self.show_message as usize => {
                let accepted = self.model.borrow_mut().choose_folder(text);
                self.apply(hwnd, accepted);
                self.show_window(hwnd);
                Some(LRESULT(1))
            }
            Message::CopyData { .. } => None,
            Message::Other(msg) if msg == self.show_message => {
                self.show_window(hwnd);
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.taskbar_created => {
                if let Some(tray) = self.tray.borrow().as_ref() {
                    tray.add_again();
                }
                Some(LRESULT(0))
            }
            Message::Other(_) => None,
        }
    }
}
