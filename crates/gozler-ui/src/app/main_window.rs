//! The main window and its message handlers.
//!
//! Design rule 1: a handler changes the model in a statement of its own, or
//! binds a borrow with `let` only in a block that makes no Win32 call, and
//! calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.

use super::dialogs;
use super::icons::Icons;
use super::requests::Requests;
use crate::core::captions::{self, StatusCaptions, WindowCaptions};
use crate::core::clipboard::{ClipboardData, ClipboardFormat};
use crate::core::commands::{self, CommandId, Focus, KeyAction, Keystroke, Menu, MenuItem, State};
use crate::core::fetch::{Fetch, WriteFailure};
use crate::core::ids::ControlId;
use crate::core::layout::{self, CaptionWidths, Dpi, Layout, MessageArea, Px, Rect, Size};
use crate::core::model::{Accepted, Change, Column, ExportError, ExportFormat, Model, Then};
use crate::core::query::{MatchMode, Matching};
use crate::core::results::{Fix, Shown};
use crate::core::selection::wide_path;
use crate::core::settings::SavedColumn;
use crate::win::clipboard::ClipboardBlock;
use crate::win::controls::{
    Button, CheckBox, DropDownList, Edit, Label, ListColumn, ListView, StatusBar,
};
use crate::win::gdi::{Font, ImageList, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::notify::{
    Command, ControlEvent, CustomDraw, DispInfo, DrawAnswer, DrawStage, ListNotify,
};
use crate::win::shell::{self, FileType};
use crate::win::wake::Waker;
use crate::win::window::{DEFERRED, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_client::RELEASES;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortSpec};
use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::OsString;
use std::ops::Range;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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
        controls.message.hwnd().set_visible(false);
        controls.fix.hwnd().set_visible(false);
        Ok(controls)
    }

    /// Gives every control the window's font, at creation and on a monitor
    /// with another DPI.
    fn apply_fonts(&self, fonts: &Fonts) {
        for control in [
            self.search.hwnd(),
            self.match_label.hwnd(),
            self.match_mode.hwnd(),
            self.match_case.hwnd(),
            self.match_path.hwnd(),
            self.message.hwnd(),
            self.fix.hwnd(),
            self.list.hwnd(),
            self.status.hwnd(),
        ] {
            control.set_font(&fonts.ui);
        }
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
    /// selection, so one user action reads it once (design: The selection).
    selection_pending: Cell<bool>,
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
        let placement = model.placement();
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
        });
        self.apply_text(hwnd, &captions);
        self.layout(hwnd);
        true
    }

    /// Every caption, from the model's facts and the current strings
    /// (design rule 12).
    fn captions(&self) -> WindowCaptions {
        WindowCaptions::new(&self.model.borrow(), &*self.numbers.borrow())
    }

    /// Sets every caption, then measures them and lays out again. Runs
    /// after a change to the regional format.
    fn refresh_text(&self, hwnd: Hwnd) {
        let captions = self.captions();
        self.apply_text(hwnd, &captions);
        let created = self.created();
        let metrics = Metrics::measure(&created.fonts.borrow(), &captions, hwnd.dpi());
        created.metrics.set(metrics);
        self.layout(hwnd);
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
        for (index, header) in captions.headers.iter().enumerate() {
            controls.list.set_header(index, header);
        }
        self.show_status(&captions.status);
        self.show_message(captions.message.as_ref());
        self.update_bound_controls();
    }

    fn match_mode_index(&self) -> usize {
        let mode = self.model.borrow().matching().mode;
        MatchMode::ALL
            .iter()
            .position(|m| *m == mode)
            .expect("ALL holds every mode")
    }

    /// Gives the controls bound to a command the command's state and
    /// choice, after every change (design rule 9): the match controls and
    /// the header's sort arrow.
    fn update_bound_controls(&self) {
        let controls = &self.created().controls;
        let (enabled, matching) = {
            let model = self.model.borrow();
            let state = State {
                model: &model,
                focus: Focus::Other,
            };
            ((CommandId::Match.row().enabled)(&state), model.matching())
        };
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

    /// Shows the list's message and its button, or hides them.
    fn show_message(&self, message: Option<&(String, Option<String>)>) {
        let controls = &self.created().controls;
        let (text, button) = match message {
            Some((text, button)) => (text.as_str(), button.as_deref()),
            None => ("", None),
        };
        controls.message.hwnd().set_text(text);
        controls.fix.hwnd().set_text(button.unwrap_or_default());
        self.place_message(text, button);
        controls.message.hwnd().set_visible(message.is_some());
        controls.fix.hwnd().set_visible(button.is_some());
    }

    /// Places the message below the list's header, and its button below it.
    fn place_message(&self, text: &str, button: Option<&str>) {
        let created = self.created();
        let Some(list) = created.layout.borrow().as_ref().map(|l| l.list) else {
            return;
        };
        let controls = &created.controls;
        let metrics = created.metrics.get();
        let header = controls.list.header().screen_rect().height;
        let area = MessageArea::new(list, header, metrics.dpi);
        let (text, button) = {
            let fonts = created.fonts.borrow();
            let meter = TextMeter::new(&fonts.ui);
            area.place(
                meter.wrapped_height(text, area.text_width()),
                metrics.font_height,
                button.map(|b| meter.width(b)),
            )
        };
        controls.message.hwnd().set_rect(text);
        if let Some(button) = button {
            controls.fix.hwnd().set_rect(button);
        }
    }

    /// Places every control where `Layout::compute` says (design rule 7).
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
        controls.list.hwnd().set_rect(layout.list);
        controls.status.set_parts(&layout.status_parts);
        created.layout.replace(Some(layout));
        self.update_message();
        hwnd.invalidate();
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
        match control {
            Some(control) if control == controls.search.hwnd() => Focus::SearchBox {
                text_selected: controls.search.has_selected_text(),
            },
            Some(control) if control == controls.list.hwnd() => Focus::List,
            _ => Focus::Other,
        }
    }

    fn focus(&self) -> Focus {
        self.focus_of(Hwnd::focused())
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
            (slot, commands::node(&slot, model.strings()))
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
        if !enabled {
            return;
        }
        let controls = &self.created().controls;
        let accepted = match item {
            MenuItem::Run(command) => match command {
                CommandId::Exit => return self.exit(hwnd),
                CommandId::About => return dialogs::about(hwnd, &self.model),
                CommandId::Open => return self.open_selected(hwnd),
                CommandId::OpenFileLocation => return self.show_selected(hwnd),
                CommandId::Properties => return self.show_properties(hwnd),
                CommandId::ExportResults => return self.export(hwnd),
                CommandId::CancelExport => self.model.borrow_mut().cancel_export(),
                CommandId::Copy => match focus {
                    Focus::SearchBox { .. } => return controls.search.copy(),
                    Focus::List | Focus::Other => return self.copy(hwnd, ClipboardFormat::Files),
                },
                CommandId::CopyAsPath => return self.copy(hwnd, ClipboardFormat::Paths),
                CommandId::CopyName => return self.copy(hwnd, ClipboardFormat::Names),
                CommandId::SelectAll => match focus {
                    Focus::SearchBox { .. } => return controls.search.select_all(),
                    Focus::List | Focus::Other => return controls.list.select_all(),
                },
                CommandId::Find => {
                    controls.search.hwnd().focus();
                    return controls.search.select_all();
                }
                CommandId::Refresh => return self.search(hwnd),
                CommandId::SearchSyntax => return dialogs::search_syntax(hwnd, &self.model),
                CommandId::Settings
                | CommandId::SearchInThisFolder
                | CommandId::Filters
                | CommandId::Inspector
                | CommandId::Columns
                | CommandId::Language
                | CommandId::Match
                | CommandId::SortBy
                | CommandId::ClearFilters
                | CommandId::ShowWindow => return,
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
            MenuItem::Language(_) => return,
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
        self.created().requests.start(fetch);
    }

    /// Asks for the chunk that the rows on screen still lack, so every
    /// chunk request comes from the rows visible now (design: Rows on
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
            Some(Then::ReadSelectionSoon) => hwnd.set_timer(timer::SELECTION, SELECTION_DELAY_MS),
            Some(Then::Cancel(purpose)) => created.requests.cancel(purpose),
            None => {}
        }
        let mut failed_export = None;
        match accepted.change {
            Change::Nothing | Change::Choice => {}
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
            Change::Searching | Change::Status | Change::Selection | Change::Export => {
                self.update_status();
            }
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
            Change::SelectionRead { rows } => {
                controls.list.redraw_rows(rows);
                self.update_status();
            }
            Change::ExportFailed { path, error } => {
                self.update_status();
                failed_export = Some((path, error));
            }
        }
        self.update_bound_controls();
        self.update_poll(hwnd);
        // Last, because the message box runs its own message loop.
        if let Some((path, error)) = failed_export {
            self.show_export_failure(hwnd, &path, &error);
        }
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
        for fetched in self.created().requests.drain() {
            let accepted = self.model.borrow_mut().accept(fetched);
            self.apply(hwnd, accepted);
        }
    }

    /// A control's notification in `WM_COMMAND`.
    fn on_control(&self, hwnd: Hwnd, event: ControlEvent) {
        let controls = &self.created().controls;
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
        } else if controls.fix.clicked(event) {
            let fix = match self.model.borrow().results().shown() {
                Shown::Message(message) => message.fix(),
                Shown::Nothing | Shown::Rows(_) => None,
            };
            if let Some(fix) = fix {
                run_fix(hwnd, &fix);
            }
            return;
        } else {
            return;
        };
        self.apply(hwnd, accepted);
    }

    /// The arrow on the sorted column's header (spec: Results list).
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
    /// the menu closes (design rule 9).
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
    /// (spec: Results list).
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
    /// (design: The selection).
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
    /// current language and regional format (design rule 12). This block
    /// borrows the model while it formats, which makes no Win32 call; an
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
    /// matched parts in bold (spec: Results list); the list draws every
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

    /// A key that the control `target` gives a meaning (design rule 8).
    /// Returns whether the window took it.
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
            // The focus moves to the first row (spec: Search box).
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
        }
        true
    }

    /// Whether `point`, in screen coordinates, is on the list's header.
    fn on_header(&self, point: (i32, i32)) -> bool {
        let header = self.created().controls.list.header().screen_rect();
        (header.x.0..header.right().0).contains(&point.0)
            && (header.y.0..header.bottom().0).contains(&point.1)
    }

    /// The selected entries' paths as Windows names them.
    fn selected_paths(&self) -> Vec<Vec<u16>> {
        let model = self.model.borrow();
        model
            .selected_files()
            .unwrap_or_default()
            .iter()
            .map(wide_path)
            .collect()
    }

    /// Opens each selected entry (spec: C1).
    fn open_selected(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_paths());
        for path in &present {
            shell::open_entry(hwnd, path);
        }
        self.report_missing(hwnd, missing);
    }

    /// Opens the folder of each selected entry with the entries selected
    /// (spec: C2): one File Explorer window for each folder.
    fn show_selected(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_paths());
        let mut folders: Vec<(Vec<u16>, Vec<Vec<u16>>)> = Vec::new();
        for path in present {
            let folder = folder_of(&path);
            match folders.iter_mut().find(|(f, _)| *f == folder) {
                Some((_, entries)) => entries.push(path),
                None => folders.push((folder, vec![path])),
            }
        }
        for (folder, entries) in &folders {
            shell::show_in_folder(folder, entries);
        }
        self.report_missing(hwnd, missing);
    }

    /// The shell's Properties of the selected entry (spec: C3).
    fn show_properties(&self, hwnd: Hwnd) {
        let (present, missing) = present(self.selected_paths());
        for path in &present {
            shell::properties(hwnd, path);
        }
        self.report_missing(hwnd, missing);
    }

    /// Says that `missing`, which the person chose, no longer exists (spec:
    /// Feedback).
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

    /// Puts the selected files on the clipboard in `format` (spec: C8, C9,
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

    /// Asks where to export the shown search, and starts the export (spec:
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

    /// The spec's message box for an export that failed (spec: T9).
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
    /// selected, so the person types the next search at once (spec: Window
    /// lifetime).
    fn show_window(&self, hwnd: Hwnd) {
        hwnd.bring_to_front();
        let search = self.created().controls.search;
        search.hwnd().focus();
        search.select_all();
    }

    /// Saves the settings and ends Gozler (spec: C7, T14).
    fn exit(&self, hwnd: Hwnd) {
        self.save_settings(hwnd, true);
        hwnd.destroy();
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
                    width: layout::saved_width(list.column_width(index), dpi),
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

/// `paths` that still exist, and the first that does not. An action on the
/// selection acts on every entry it can (spec: The selection).
fn present(paths: Vec<Vec<u16>>) -> (Vec<Vec<u16>>, Option<Vec<u16>>) {
    let mut missing = None;
    let present = paths
        .into_iter()
        .filter_map(|path| {
            if std::fs::symlink_metadata(OsString::from_wide(&path)).is_ok() {
                Some(path)
            } else {
                missing.get_or_insert(path);
                None
            }
        })
        .collect();
    (present, missing)
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
            Message::Activate { active: false } => {
                self.focus_on_activate.set(Hwnd::focused());
                None
            }
            Message::Activate { active: true } => {
                self.focus_on_activate
                    .take()
                    .unwrap_or(self.created().controls.search.hwnd())
                    .focus();
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
                let list = self.created().controls.list.notification(notification)?;
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
            Message::CtlColorStatic(colors) => Some(colors.window_background()),
            Message::Paint => {
                let dividers = self
                    .created()
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
                self.created().requests.shut_down();
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
