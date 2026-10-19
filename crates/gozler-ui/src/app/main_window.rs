//! The main window and its message handlers.
//!
//! Design rule 1: a handler changes the model in a statement of its own, or
//! binds a borrow with `let` only in a block that makes no Win32 call, and
//! calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.

use super::dialogs;
use crate::core::commands::{self, CommandId, Focus, Menu, MenuItem, State};
use crate::core::layout::{Captions, Dpi, Layout, Px, Rect, Size};
use crate::core::model::Model;
use crate::core::query::MatchMode;
use crate::win::controls::{CheckBox, DropDownList, Edit, Label, ListColumn, ListView, StatusBar};
use crate::win::gdi::{Font, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::window::{Message, WindowProc};
use crate::win::{self, Hwnd};
use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use windows::Win32::Foundation::LRESULT;

/// The child control IDs, below every menu item ID.
mod id {
    pub(super) const SEARCH: u16 = 10;
    pub(super) const MATCH_LABEL: u16 = 11;
    pub(super) const MATCH_MODE: u16 = 12;
    pub(super) const MATCH_CASE: u16 = 13;
    pub(super) const MATCH_PATH: u16 = 14;
    pub(super) const LIST: u16 = 15;
    pub(super) const STATUS: u16 = 16;
}

/// The controls, created in `WM_CREATE` in Tab order; Windows destroys them
/// with the window.
struct Controls {
    search: Edit,
    match_label: Label,
    match_mode: DropDownList,
    match_case: CheckBox,
    match_path: CheckBox,
    list: ListView,
    status: StatusBar,
}

impl Controls {
    fn create(parent: Hwnd) -> windows::core::Result<Controls> {
        Ok(Controls {
            search: Edit::create(parent, id::SEARCH)?,
            // The label comes right before its list, so its access key
            // focuses the list (spec: C19).
            match_label: Label::create(parent, id::MATCH_LABEL)?,
            match_mode: DropDownList::create(parent, id::MATCH_MODE)?,
            match_case: CheckBox::create(parent, id::MATCH_CASE)?,
            match_path: CheckBox::create(parent, id::MATCH_PATH)?,
            list: ListView::create(parent, id::LIST)?,
            status: StatusBar::create(parent, id::STATUS)?,
        })
    }

    fn all(&self) -> [Hwnd; 7] {
        [
            self.search.0,
            self.match_label.0,
            self.match_mode.0,
            self.match_case.0,
            self.match_path.0,
            self.list.0,
            self.status.0,
        ]
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
    /// The controls' font. It is replaced only after every control has the
    /// new one, because `WM_SETFONT` does not take ownership.
    font: RefCell<Option<Font>>,
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
}

impl MainWindow {
    /// Creates and shows the window; the window owns its state.
    pub(crate) fn open(
        class: &str,
        model: Model,
        settings_file: PathBuf,
        show_message: u32,
    ) -> Hwnd {
        let placement = model.settings.window.placement;
        let window = Rc::new(MainWindow {
            controls: OnceCell::new(),
            font: RefCell::new(None),
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
        });
        let hwnd =
            win::window::create_window(class, window, placement).expect("the main window opens");
        // Showing the window activates it, and WM_ACTIVATE focuses the
        // search box.
        win::window::show(hwnd, placement);
        hwnd
    }

    fn on_create(&self, hwnd: Hwnd) -> bool {
        let dpi = hwnd.dpi();
        let (Ok(controls), Ok(font)) = (Controls::create(hwnd), Font::message(dpi)) else {
            return false;
        };
        for control in controls.all() {
            control.set_font(&font);
        }
        let columns: Vec<_> = self.model.borrow().columns.clone();
        for (index, column) in columns.iter().enumerate() {
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: column.default_width().to_px(dpi),
                    right_aligned: column.right_aligned(),
                },
            );
        }
        self.font.replace(Some(font));
        let _ = self.controls.set(controls);
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
                strings.text("window.title").to_owned(),
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

    /// Measures every caption that the layout places, in the current font.
    fn measure(&self, dpi: Dpi) {
        let font = self.font.borrow();
        let Some(font) = font.as_ref() else { return };
        let meter = TextMeter::new(font);
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
        hwnd.invalidate();
    }

    fn on_dpi_changed(&self, hwnd: Hwnd, dpi: Dpi, suggested: Rect) {
        let (Some(controls), Some(metrics)) = (self.controls.get(), self.metrics.get()) else {
            return;
        };
        let Ok(font) = Font::message(dpi) else { return };
        for control in controls.all() {
            control.set_font(&font);
        }
        drop(self.font.replace(Some(font)));
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

    /// Sets every menu item's enabled state and mark from the command table.
    fn update_menu(&self) {
        let Some(bar) = self.menu_bar.get() else {
            return;
        };
        let focus = self.focus();
        let states: Vec<(u16, bool, bool)> = {
            let model = self.model.borrow();
            let state = State {
                model: &model,
                focus,
            };
            commands::items(&self.menus)
                .into_iter()
                .map(|item| (item.id(), item.enabled(&state), item.checked(&state)))
                .collect()
        };
        for (id, enabled, checked) in states {
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
                | CommandId::Refresh
                | CommandId::Find
                | CommandId::Match
                | CommandId::SortBy
                | CommandId::ClearFilters
                | CommandId::SearchSyntax
                | CommandId::ShowWindow => {}
            },
            MenuItem::Column(_)
            | MenuItem::SortKey(_)
            | MenuItem::SortDirection(_)
            | MenuItem::Language(_) => {}
        }
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

    /// Saves the settings and ends gozler (spec: C7, T14).
    fn exit(&self, hwnd: Hwnd) {
        self.save_settings(hwnd, true);
        hwnd.destroy();
    }

    /// Saves the window's placement with the settings. A failure is shown
    /// once per session, and not while Windows ends the session.
    fn save_settings(&self, hwnd: Hwnd, report_failure: bool) {
        let placement = hwnd.placement();
        let saved = {
            let mut model = self.model.borrow_mut();
            if placement.is_some() {
                model.settings.window.placement = placement;
            }
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
            Message::Command {
                id,
                from_control: false,
                ..
            } => {
                self.run(hwnd, MenuItem::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Command { .. } => None,
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
            Message::Destroy => {
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
