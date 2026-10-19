//! The main window and its message handlers.
//!
//! Design rule 1: a handler changes the model in a statement of its own, or
//! binds a borrow with `let` only in a block that makes no Win32 call, and
//! calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.

use super::dialogs;
use super::icons::Icons;
use super::requests::Requests;
use crate::core::commands::{self, CommandId, Focus, Menu, MenuItem, State};
use crate::core::fetch::Fetch;
use crate::core::ids::ControlId;
use crate::core::layout::{self, Captions, Dpi, Layout, MessageArea, Px, Rect, Size};
use crate::core::model::{Accepted, Change, Column, Model, Then};
use crate::core::query::MatchMode;
use crate::core::results::{Fix, Shown};
use crate::core::settings::SavedColumn;
use crate::core::text;
use crate::win::controls::{
    Button, CheckBox, DropDownList, Edit, Label, ListColumn, ListView, StatusBar,
};
use crate::win::gdi::{Font, ImageList, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::notify::{CustomDraw, DispInfo, DrawAnswer, DrawStage, ListNotify, Notify};
use crate::win::wake::Waker;
use crate::win::window::{Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_client::RELEASES;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortSpec};
use std::cell::{Cell, OnceCell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CBN_SELCHANGE, EN_CHANGE, IDCANCEL, IDOK,
};

mod timer {
    /// Search as you type waits this long after the last keystroke.
    pub(super) const SEARCH: usize = 1;
    pub(super) const STATUS: usize = 2;
}

const SEARCH_DELAY_MS: u32 = 150;
const STATUS_POLL_MS: u32 = 2_000;

/// The status bar's parts that stage 3 fills.
const RESULTS_PART: usize = 0;
const INDEX_PART: usize = 3;

/// The controls, created in `WM_CREATE` in Tab order; Windows destroys them
/// with the window.
struct Controls {
    search: Edit,
    match_label: Label,
    match_mode: DropDownList,
    match_case: CheckBox,
    match_path: CheckBox,
    /// The list's message and its button, in the list's empty area. They
    /// are created before the list, so they lie above it and come before it
    /// in the Tab order.
    message: Label,
    fix: Button,
    list: ListView,
    status: StatusBar,
}

impl Controls {
    fn create(parent: Hwnd) -> windows::core::Result<Controls> {
        let controls = Controls {
            search: Edit::create(parent, ControlId::Search.id())?,
            // The label comes right before its list, so its access key
            // focuses the list (spec: C19).
            match_label: Label::create(parent, ControlId::MatchLabel.id())?,
            match_mode: DropDownList::create(parent, ControlId::MatchMode.id())?,
            match_case: CheckBox::create(parent, ControlId::MatchCase.id())?,
            match_path: CheckBox::create(parent, ControlId::MatchPath.id())?,
            message: Label::create_message(parent, ControlId::Message.id())?,
            fix: Button::create(parent, ControlId::Fix.id())?,
            list: ListView::create(parent, ControlId::List.id())?,
            status: StatusBar::create(parent, ControlId::Status.id())?,
        };
        controls.message.0.set_visible(false);
        controls.fix.0.set_visible(false);
        Ok(controls)
    }

    fn all(&self) -> [Hwnd; 9] {
        [
            self.search.0,
            self.match_label.0,
            self.match_mode.0,
            self.match_case.0,
            self.match_path.0,
            self.message.0,
            self.fix.0,
            self.list.0,
            self.status.0,
        ]
    }
}

/// The controls' font, and its bold form for the matched part of a cell.
struct Fonts {
    ui: Font,
    bold: Font,
}

impl Fonts {
    fn new(dpi: Dpi) -> windows::core::Result<Fonts> {
        let ui = Font::message(dpi)?;
        let bold = ui.bold()?;
        Ok(Fonts { ui, bold })
    }
}

/// What the layout needs that only measuring the text gives.
#[derive(Clone, Copy)]
struct Metrics {
    dpi: Dpi,
    font_height: Px,
    captions: Captions,
}

pub(crate) struct MainWindow {
    controls: OnceCell<Controls>,
    /// Replaced only after every control has the new fonts, because
    /// `WM_SETFONT` does not take ownership.
    fonts: RefCell<Option<Fonts>>,
    metrics: Cell<Option<Metrics>>,
    layout: RefCell<Option<Layout>>,
    model: RefCell<Model>,
    numbers: RefCell<WindowsNumbers>,
    menus: Vec<Menu>,
    menu_bar: Cell<Option<win::menu::Menu>>,
    /// The control that had the focus when the window was deactivated.
    focus_on_activate: Cell<Option<Hwnd>>,
    settings_file: PathBuf,
    settings_failure_shown: Cell<bool>,
    show_message: u32,
    /// Made in `WM_CREATE`, which gives the window handle its waker needs.
    requests: OnceCell<Requests>,
    icons: Icons,
    images: Cell<Option<ImageList>>,
    /// Whether the status timer runs.
    polling: Cell<bool>,
}

impl MainWindow {
    /// Creates and shows the window, and starts the first search and the
    /// first status request; the window owns its state.
    pub(crate) fn open(
        class: &str,
        model: Model,
        settings_file: PathBuf,
        show_message: u32,
    ) -> Hwnd {
        let placement = model.settings.window.placement;
        let window = Rc::new(MainWindow {
            controls: OnceCell::new(),
            fonts: RefCell::new(None),
            metrics: Cell::new(None),
            layout: RefCell::new(None),
            model: RefCell::new(model),
            numbers: RefCell::new(WindowsNumbers::new()),
            menus: commands::menu_bar(),
            menu_bar: Cell::new(None),
            focus_on_activate: Cell::new(None),
            settings_file,
            settings_failure_shown: Cell::new(false),
            show_message,
            requests: OnceCell::new(),
            icons: Icons::default(),
            images: Cell::new(None),
            polling: Cell::new(false),
        });
        let hwnd = win::window::create_window(class, Rc::clone(&window), placement)
            .expect("the main window opens");
        // Only a status reply names the volumes, so it is asked for beside
        // the first search (design: Requests).
        window.search(hwnd);
        window.ask_status();
        // Showing the window activates it, and WM_ACTIVATE focuses the
        // search box.
        win::window::show(hwnd, placement);
        hwnd
    }

    fn on_create(&self, hwnd: Hwnd) -> bool {
        let dpi = hwnd.dpi();
        let (Ok(controls), Ok(fonts)) = (Controls::create(hwnd), Fonts::new(dpi)) else {
            return false;
        };
        for control in controls.all() {
            control.set_font(&fonts.ui);
        }
        let (columns, saved, order) = {
            let model = self.model.borrow();
            (
                model.columns.clone(),
                model.settings.window.columns.clone(),
                model.column_order(),
            )
        };
        for (index, column) in columns.iter().enumerate() {
            let width = saved.iter().find(|s| s.column == *column).map(|s| s.width);
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: layout::column_width(*column, width, dpi),
                    right_aligned: column.right_aligned(),
                },
            );
        }
        controls.list.set_column_order(&order);
        if let Some(images) = ImageList::system_small() {
            controls.list.set_small_images(&images);
            self.images.set(Some(images));
        }
        let _ = self.requests.set(Requests::new(Waker::new(hwnd)));
        self.fonts.replace(Some(fonts));
        let _ = self.controls.set(controls);
        self.show_sort();
        self.refresh_text(hwnd);
        true
    }

    /// Sets every caption from the model's facts and the current strings
    /// (design rule 12), then measures the captions and lays out again.
    /// Runs at creation and after a change to the regional format.
    fn refresh_text(&self, hwnd: Hwnd) {
        self.apply_text(hwnd);
        self.measure(hwnd.dpi());
        self.layout(hwnd);
    }

    fn apply_text(&self, hwnd: Hwnd) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let (title, menus, cue, label, modes, selected_mode, case, path, headers, matching) = {
            let model = self.model.borrow();
            let strings = &model.strings;
            (
                text::title(strings, &model.text),
                commands::render(&self.menus, strings),
                strings.text("search.cue").to_owned(),
                strings.text("match.label").to_owned(),
                MatchMode::ALL.map(|m| strings.text(m.label()).to_owned()),
                MatchMode::ALL
                    .iter()
                    .position(|m| *m == model.matching.mode)
                    .expect("ALL holds every mode"),
                strings.text("match.case").to_owned(),
                strings.text("match.path").to_owned(),
                model
                    .columns
                    .iter()
                    .map(|c| strings.text(c.header()).to_owned())
                    .collect::<Vec<_>>(),
                model.matching,
            )
        };
        hwnd.set_text(&title);
        if let Ok(bar) = win::menu::Menu::bar(&menus) {
            hwnd.set_menu(bar);
            if let Some(old) = self.menu_bar.replace(Some(bar)) {
                old.destroy();
            }
        }
        controls.search.set_cue(&cue);
        controls.match_label.0.set_text(&label);
        controls
            .match_mode
            .set_items(&modes.each_ref().map(String::as_str), selected_mode);
        controls.match_case.0.set_text(&case);
        controls.match_case.set_checked(matching.case);
        controls.match_path.0.set_text(&path);
        controls.match_path.set_checked(matching.path);
        for (index, header) in headers.iter().enumerate() {
            controls.list.set_header(index, header);
        }
        self.update_controls();
        self.update_status();
        self.update_message();
    }

    /// Gives the match controls the state of their command (design rule 9).
    fn update_controls(&self) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let enabled = {
            let model = self.model.borrow();
            (CommandId::Match.row().enabled)(&State {
                model: &model,
                focus: Focus::Other,
            })
        };
        for control in [
            controls.match_label.0,
            controls.match_mode.0,
            controls.match_case.0,
            controls.match_path.0,
        ] {
            control.set_enabled(enabled);
        }
    }

    /// Status parts 1 and 4: the results and the index state.
    fn update_status(&self) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let (results, index) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            (
                text::results_status(&model, &*numbers),
                text::index_status(&model),
            )
        };
        controls.status.set_text(RESULTS_PART, &results);
        controls.status.set_text(INDEX_PART, &index);
    }

    /// Shows the list's message and its button, or hides them.
    fn update_message(&self) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let message = text::list_message(&self.model.borrow());
        let (text, button) = match &message {
            Some((text, button)) => (text.as_str(), button.as_deref()),
            None => ("", None),
        };
        controls.message.0.set_text(text);
        controls.fix.0.set_text(button.unwrap_or_default());
        self.place_message(text, button);
        controls.message.0.set_visible(message.is_some());
        controls.fix.0.set_visible(button.is_some());
    }

    /// Places the message below the list's header, and its button below it.
    fn place_message(&self, text: &str, button: Option<&str>) {
        let (Some(controls), Some(metrics)) = (self.controls.get(), self.metrics.get()) else {
            return;
        };
        let Some(list) = self.layout.borrow().as_ref().map(|l| l.list) else {
            return;
        };
        let header = controls.list.header().screen_rect().height;
        let area = MessageArea::new(list, header, metrics.dpi);
        let placed = self.fonts.borrow().as_ref().map(|fonts| {
            let meter = TextMeter::new(&fonts.ui);
            area.place(
                meter.wrapped_height(text, area.text_width()),
                metrics.font_height,
                button.map(|b| meter.width(b)),
            )
        });
        let Some((text, button)) = placed else { return };
        controls.message.0.set_rect(text);
        if let Some(button) = button {
            controls.fix.0.set_rect(button);
        }
    }

    /// Measures every caption that the layout places, in the current font.
    fn measure(&self, dpi: Dpi) {
        let fonts = self.fonts.borrow();
        let Some(fonts) = fonts.as_ref() else { return };
        let meter = TextMeter::new(&fonts.ui);
        let captions = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            Captions::measure(&model.strings, &*numbers, &|text| meter.width(text))
        };
        self.metrics.set(Some(Metrics {
            dpi,
            font_height: meter.height(),
            captions,
        }));
    }

    /// Places every control where `Layout::compute` says (design rule 7).
    fn layout(&self, hwnd: Hwnd) {
        let (Some(controls), Some(metrics)) = (self.controls.get(), self.metrics.get()) else {
            return;
        };
        let status_height = controls.status.fit();
        let panes = self.model.borrow().panes;
        let layout = Layout::compute(
            hwnd.client_size(),
            metrics.dpi,
            metrics.font_height,
            status_height,
            &metrics.captions,
            &panes,
        );
        controls.search.0.set_rect(layout.search);
        controls.match_label.0.set_rect(layout.match_label);
        // A drop-down list's height includes its open list.
        controls.match_mode.0.set_rect(Rect {
            height: Px(layout.match_mode.height.0 * 10),
            ..layout.match_mode
        });
        controls.match_case.0.set_rect(layout.match_case);
        controls.match_path.0.set_rect(layout.match_path);
        controls.list.0.set_rect(layout.list);
        controls.status.set_parts(&layout.status_parts);
        self.layout.replace(Some(layout));
        self.update_message();
        hwnd.invalidate();
    }

    fn on_dpi_changed(&self, hwnd: Hwnd, dpi: Dpi, suggested: Rect) {
        let (Some(controls), Some(metrics)) = (self.controls.get(), self.metrics.get()) else {
            return;
        };
        let Ok(fonts) = Fonts::new(dpi) else { return };
        for control in controls.all() {
            control.set_font(&fonts.ui);
        }
        drop(self.fonts.replace(Some(fonts)));
        let columns = self.model.borrow().columns.len();
        for index in 0..columns {
            let width = controls.list.column_width(index);
            controls
                .list
                .set_column_width(index, width.rescale(metrics.dpi, dpi));
        }
        self.measure(dpi);
        // The minimum size is in the old DPI's pixels until the layout
        // runs again, so it does not hold the window to the old size.
        self.layout.replace(None);
        hwnd.set_rect(suggested);
        self.layout(hwnd);
    }

    /// Which control has the focus, as the command table asks.
    fn focus(&self) -> Focus {
        match (self.controls.get(), Hwnd::focused()) {
            (Some(controls), Some(focused)) if focused == controls.search.0 => Focus::SearchBox {
                text_selected: controls.search.has_selected_text(),
            },
            _ => Focus::Other,
        }
    }

    fn search_box_focused(&self) -> bool {
        self.controls
            .get()
            .is_some_and(|controls| Hwnd::focused() == Some(controls.search.0))
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

    /// Sets every menu item's enabled state and mark from the command table.
    fn update_menu(&self) {
        let Some(bar) = self.menu_bar.get() else {
            return;
        };
        for (id, enabled, checked) in self.item_states(&commands::items(&self.menus)) {
            bar.set_state(id, enabled, checked);
        }
    }

    /// Runs one menu item or shortcut; every surface calls this (design
    /// rule 9). It asks the command table again whether the item is enabled,
    /// because the state can change while a menu is open (spec: T11).
    fn run(&self, hwnd: Hwnd, item: MenuItem) {
        let focus = self.focus();
        let enabled = {
            let model = self.model.borrow();
            item.enabled(&State {
                model: &model,
                focus,
            })
        };
        let Some(controls) = self.controls.get() else {
            return;
        };
        if !enabled {
            return;
        }
        match item {
            MenuItem::Run(command) => match command {
                CommandId::Exit => self.exit(hwnd),
                CommandId::About => dialogs::about(hwnd, &self.model),
                CommandId::Copy => controls.search.copy(),
                CommandId::SelectAll => controls.search.select_all(),
                CommandId::Find => {
                    controls.search.0.focus();
                    controls.search.select_all();
                }
                CommandId::Refresh => self.search(hwnd),
                CommandId::SearchSyntax => dialogs::search_syntax(hwnd, &self.model),
                CommandId::Open
                | CommandId::OpenFileLocation
                | CommandId::Properties
                | CommandId::ExportResults
                | CommandId::CancelExport
                | CommandId::Settings
                | CommandId::CopyAsPath
                | CommandId::CopyName
                | CommandId::SearchInThisFolder
                | CommandId::Filters
                | CommandId::Inspector
                | CommandId::Columns
                | CommandId::Language
                | CommandId::Match
                | CommandId::SortBy
                | CommandId::ClearFilters
                | CommandId::ShowWindow => {}
            },
            MenuItem::Column(column) => self.toggle_column(hwnd, column),
            MenuItem::SortKey(key) => {
                let sort = {
                    let model = self.model.borrow();
                    if model.sort.key == key {
                        model.sort
                    } else {
                        SortSpec::default_for(key)
                    }
                };
                self.sort_by(hwnd, sort);
            }
            MenuItem::SortDirection(dir) => {
                let key = self.model.borrow().sort.key;
                self.sort_by(hwnd, SortSpec { key, dir });
            }
            MenuItem::Language(_) => {}
        }
    }

    /// Runs the search that the text and the controls ask for now.
    fn search(&self, hwnd: Hwnd) {
        hwnd.kill_timer(timer::SEARCH);
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: jiff::tz::TimeZone::system(),
        };
        let accepted = self.model.borrow_mut().start_search(&start);
        self.apply(hwnd, accepted);
    }

    fn ask_status(&self) {
        let fetch = self.model.borrow_mut().status_needed();
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    fn start(&self, fetch: Fetch) {
        if let Some(requests) = self.requests.get() {
            requests.start(fetch);
        }
    }

    /// Asks for the chunk that the rows on screen still lack, so every
    /// chunk request comes from the rows visible now (design: Rows on
    /// demand).
    fn fetch_visible(&self) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let visible = controls.list.visible_rows();
        let fetch = self.model.borrow_mut().rows_needed(visible);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// Updates the controls for what a reply or an input changed, and starts
    /// the request it makes necessary.
    fn apply(&self, hwnd: Hwnd, accepted: Accepted) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        match accepted.then {
            Some(Then::Fetch(fetch)) => self.start(fetch),
            Some(Then::SearchAgain) => self.search(hwnd),
            None => {}
        }
        match accepted.change {
            Change::Stale => {}
            Change::Searching | Change::Status => self.update_status(),
            Change::NewSearch { count } => {
                // A new search shows its results from the top, with nothing
                // selected (spec: T2).
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
        }
        // Rule 9: a reply can change what a command allows.
        self.update_controls();
        self.update_poll(hwnd);
    }

    /// Starts or stops the one status timer as the model says (design:
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

    /// Accepts every reply that has arrived.
    fn on_wake(&self, hwnd: Hwnd) {
        let Some(requests) = self.requests.get() else {
            return;
        };
        for fetched in requests.drain() {
            let accepted = self.model.borrow_mut().accept(fetched);
            self.apply(hwnd, accepted);
        }
    }

    /// A control's notification in `WM_COMMAND`.
    fn on_control(&self, hwnd: Hwnd, id: u16, notification: u16) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let Some(control) = ControlId::from_id(id) else {
            return;
        };
        match (control, u32::from(notification)) {
            (ControlId::Search, EN_CHANGE) => {
                let text = controls.search.text();
                let title = {
                    let mut model = self.model.borrow_mut();
                    model.text = text;
                    text::title(&model.strings, &model.text)
                };
                hwnd.set_text(&title);
                hwnd.set_timer(timer::SEARCH, SEARCH_DELAY_MS);
            }
            (ControlId::MatchMode, CBN_SELCHANGE) => {
                let Some(mode) = controls
                    .match_mode
                    .selected()
                    .and_then(|i| MatchMode::ALL.get(i).copied())
                else {
                    return;
                };
                self.model.borrow_mut().matching.mode = mode;
                self.search(hwnd);
            }
            (ControlId::MatchCase, BN_CLICKED) => {
                let checked = controls.match_case.is_checked();
                self.model.borrow_mut().matching.case = checked;
                self.search(hwnd);
            }
            (ControlId::MatchPath, BN_CLICKED) => {
                let checked = controls.match_path.is_checked();
                self.model.borrow_mut().matching.path = checked;
                self.search(hwnd);
            }
            (ControlId::Fix, BN_CLICKED) => {
                let fix = match self.model.borrow().results.shown() {
                    Shown::Message(message) => message.fix(),
                    Shown::Nothing | Shown::Rows(_) => None,
                };
                if let Some(fix) = fix {
                    run_fix(hwnd, &fix);
                }
            }
            _ => {}
        }
    }

    /// Sorts by `sort` and runs the search again (spec: C20).
    fn sort_by(&self, hwnd: Hwnd, sort: SortSpec) {
        self.model.borrow_mut().sort = sort;
        self.show_sort();
        self.search(hwnd);
    }

    /// The arrow on the sorted column's header (spec: Results list).
    fn show_sort(&self) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let (count, sorted) = {
            let model = self.model.borrow();
            (
                model.columns.len(),
                model
                    .columns
                    .iter()
                    .position(|c| c.sort_key() == Some(model.sort.key))
                    .map(|index| (index, model.sort.dir == SortDir::Asc)),
            )
        };
        controls.list.show_sort(count, sorted);
    }

    /// Shows or hides a column (spec: C15). A shown column joins at the end.
    fn toggle_column(&self, hwnd: Hwnd, column: Column) {
        let Some(controls) = self.controls.get() else {
            return;
        };
        let removed = {
            let mut model = self.model.borrow_mut();
            match model.columns.iter().position(|c| *c == column) {
                Some(index) => {
                    model.columns.remove(index);
                    Some(index)
                }
                None => {
                    model.columns.push(column);
                    None
                }
            }
        };
        match removed {
            Some(index) => controls.list.delete_column(index),
            None => {
                let (index, header) = {
                    let model = self.model.borrow();
                    (
                        model.columns.len() - 1,
                        model.strings.text(column.header()).to_owned(),
                    )
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
        }
        self.show_sort();
    }

    /// The header's context menu, View > Columns' items (spec: C15).
    fn header_menu(&self, hwnd: Hwnd, point: (i32, i32)) {
        let items = commands::header_menu();
        let states = self.item_states(&items);
        let nodes = commands::render_popup(&items, &self.model.borrow().strings);
        let chosen = win::menu::Menu::track_popup(hwnd, &nodes, point, &states);
        if let Some(item) = chosen.and_then(MenuItem::from_id) {
            self.run(hwnd, item);
        }
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
                let sort = {
                    let model = self.model.borrow();
                    model
                        .columns
                        .get(index)
                        .and_then(|column| model.header_sort(*column))
                };
                if let Some(sort) = sort {
                    self.sort_by(hwnd, sort);
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(draw) => Some(CustomDraw::answer(match draw.stage() {
                DrawStage::Paint => DrawAnswer::EachRow,
                DrawStage::Row => DrawAnswer::EachCell,
                DrawStage::Cell { row, column } => self.draw_cell(&draw, hwnd.dpi(), row, column),
                DrawStage::Other => DrawAnswer::Default,
            })),
            ListNotify::Other => None,
        }
    }

    /// Formats a cell when the list asks for it, so a redraw shows the
    /// current language and regional format (design rule 12). This block
    /// borrows the model while it formats, which makes no Win32 call; an
    /// icon not asked for yet is looked up after the borrow ends.
    fn fill_cell(&self, info: &mut DispInfo) {
        let (text, icon) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let column = model.columns.get(info.column()).copied();
            match (column, model.results.row(info.row())) {
                (Some(column), Some(row)) => (
                    info.wants_text()
                        .then(|| text::cell(&model.strings, &*numbers, column, row)),
                    text::path_part(column, &row.path)
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
    /// matched parts in bold (spec: Results list); the list draws every
    /// other cell.
    fn draw_cell(&self, draw: &CustomDraw, dpi: Dpi, row: u64, column: usize) -> DrawAnswer {
        let cell = {
            let model = self.model.borrow();
            let column = model.columns.get(column).copied();
            let Some(item) = model.results.row(row) else {
                return DrawAnswer::Default;
            };
            let Some(span) = column.and_then(|c| text::path_part(c, &item.path)) else {
                return DrawAnswer::Default;
            };
            let bold = within(&model.results.matched_parts(item), &span);
            if bold.is_empty() {
                return DrawAnswer::Default;
            }
            let icon = (column == Some(Column::Name)).then_some(item.is_dir);
            (item.path[span].to_owned(), bold, icon)
        };
        let (text, bold, icon) = cell;
        let icon = icon.map(|is_folder| self.icons.index(&text, is_folder));
        let fonts = self.fonts.borrow();
        let Some(fonts) = fonts.as_ref() else {
            return DrawAnswer::Default;
        };
        draw.draw_cell(
            &text,
            &bold,
            (&fonts.ui, &fonts.bold),
            self.images.get().zip(icon),
            layout::cell_text_inset(column == 0, dpi),
        );
        DrawAnswer::Drawn
    }

    /// `Down` in the search box moves the focus to the first row (spec:
    /// Search box). Returns whether it took the key.
    fn on_key(&self, target: Hwnd, key: u16) -> bool {
        let Some(controls) = self.controls.get() else {
            return false;
        };
        if target != controls.search.0 || key != VK_DOWN.0 {
            return false;
        }
        let count = self.model.borrow().results.count();
        controls.list.0.focus();
        if count > 0 {
            controls.list.select_only(Some(0));
            controls.list.ensure_visible(0);
        }
        true
    }

    /// Whether `point`, in screen coordinates, is on the list's header.
    fn on_header(&self, point: (i32, i32)) -> bool {
        self.controls.get().is_some_and(|controls| {
            let header = controls.list.header().screen_rect();
            (header.x.0..header.right().0).contains(&point.0)
                && (header.y.0..header.bottom().0).contains(&point.1)
        })
    }

    /// Brings the window forward with the search box focused and its text
    /// selected, so the person types the next search at once (spec: Window
    /// lifetime).
    fn show_window(&self, hwnd: Hwnd) {
        hwnd.bring_to_front();
        if let Some(controls) = self.controls.get() {
            controls.search.0.focus();
            controls.search.select_all();
        }
    }

    /// Saves the settings and ends Gozler (spec: C7, T14).
    fn exit(&self, hwnd: Hwnd) {
        self.save_settings(hwnd, true);
        hwnd.destroy();
    }

    /// The shown columns in the header's order, with their widths in DIP.
    fn saved_columns(&self) -> Option<Vec<SavedColumn>> {
        let (controls, metrics) = (self.controls.get()?, self.metrics.get()?);
        let columns = self.model.borrow().columns.clone();
        Some(
            controls
                .list
                .column_order(columns.len())
                .into_iter()
                .filter_map(|index| {
                    Some(SavedColumn {
                        column: *columns.get(index)?,
                        width: layout::saved_width(controls.list.column_width(index), metrics.dpi),
                    })
                })
                .collect(),
        )
    }

    /// Saves the window's placement, match controls, sort and columns with
    /// the settings. A failure is shown once per session, and not while
    /// Windows ends the session.
    fn save_settings(&self, hwnd: Hwnd, report_failure: bool) {
        let placement = hwnd.placement();
        let columns = self.saved_columns();
        let saved = {
            let mut model = self.model.borrow_mut();
            let model = &mut *model;
            let window = &mut model.settings.window;
            if placement.is_some() {
                window.placement = placement;
            }
            if let Some(columns) = columns {
                window.columns = columns;
            }
            window.matching = model.matching;
            window.sort = model.sort;
            model.settings.save(&self.settings_file)
        };
        let Err(error) = saved else { return };
        if !report_failure || self.settings_failure_shown.replace(true) {
            return;
        }
        let (text, title) = {
            let model = self.model.borrow();
            let strings = &model.strings;
            (
                strings.format(
                    "message.settings_not_saved",
                    &[
                        &self.settings_file.display().to_string(),
                        &win::error_text(&error),
                    ],
                ),
                strings.text("window.title").to_owned(),
            )
        };
        win::dialog::error_box(Some(hwnd), &text, &title);
    }

    fn min_window_size(&self, hwnd: Hwnd) -> Option<Size> {
        let min_client = self.layout.borrow().as_ref()?.min_client;
        Some(win::window::window_size(
            hwnd,
            min_client,
            self.metrics.get()?.dpi,
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

/// Runs a failure's fix: the commands that `gozler` prints for it, in one
/// `cmd /c` as administrator, so Windows asks for permission once (design:
/// Feedback and fixes).
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
            _ if self.controls.get().is_none() => None,
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
            Message::Activate { active: false } => {
                self.focus_on_activate.set(Hwnd::focused());
                None
            }
            Message::Activate { active: true } => {
                let controls = self.controls.get()?;
                self.focus_on_activate
                    .take()
                    .unwrap_or(controls.search.0)
                    .focus();
                Some(LRESULT(0))
            }
            // IsDialogMessageW turns Enter into IDOK and Esc into IDCANCEL,
            // which the focused control decides (design rule 8): in the
            // search box, Enter runs the search and Esc clears the box.
            Message::Command {
                id,
                from_control: false,
                ..
            } if id == IDOK.0 as u16 || id == IDCANCEL.0 as u16 => {
                if self.search_box_focused() {
                    if id == IDOK.0 as u16 {
                        self.search(hwnd);
                    } else if let Some(controls) = self.controls.get() {
                        controls.search.0.set_text("");
                    }
                }
                Some(LRESULT(0))
            }
            Message::Command {
                id,
                from_control: false,
                ..
            } => {
                self.run(hwnd, MenuItem::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Command {
                id,
                notification,
                from_control: true,
            } => {
                self.on_control(hwnd, id, notification);
                None
            }
            Message::Notify(Notify { from, list }) if from == ControlId::List.id() => {
                self.on_list_notify(hwnd, list)
            }
            Message::Notify(_) => None,
            Message::Timer(timer::SEARCH) => {
                self.search(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(timer::STATUS) => {
                self.ask_status();
                Some(LRESULT(0))
            }
            Message::Timer(_) => None,
            Message::Wake => {
                self.on_wake(hwnd);
                Some(LRESULT(0))
            }
            Message::ContextMenu {
                point: Some(point), ..
            } if self.on_header(point) => {
                self.header_menu(hwnd, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { .. } => None,
            Message::KeyDown { target, key } => {
                Some(LRESULT(isize::from(self.on_key(target, key))))
            }
            Message::InitMenuPopup => {
                self.update_menu();
                Some(LRESULT(0))
            }
            Message::CtlColorStatic(colors) => Some(colors.window_background()),
            Message::Paint => {
                let dividers = self
                    .layout
                    .borrow()
                    .as_ref()
                    .map(|l| l.dividers.clone())
                    .unwrap_or_default();
                hwnd.paint_dividers(&dividers);
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
                self.exit(hwnd);
                Some(LRESULT(0))
            }
            // Every request thread is joined while the window's handle is
            // still valid, so no wake-up reaches a reused handle (design
            // rule 5).
            Message::Destroy => {
                if let Some(requests) = self.requests.get() {
                    requests.shut_down();
                }
                win::window::quit();
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.show_message => {
                self.show_window(hwnd);
                Some(LRESULT(0))
            }
            Message::Other(_) => None,
        }
    }
}
