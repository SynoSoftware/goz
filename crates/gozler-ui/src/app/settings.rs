//! The Settings dialog and its command shortcut dialog (DESIGN.md:
//! Settings dialog). The dialog edits its own copy of the
//! settings, a [`SettingsDraft`], and OK or Apply gives it to the main
//! window, so no model borrow is held while its modal loop runs (DESIGN.md:
//! window rule 1). Each page is a child dialog of its own template, whose
//! controls this dialog's procedure handles.

use super::dialogs::ServiceReport;
use super::include::NotSaved;
use super::indexed_folders::IndexedFolders;
use super::main_window::MainWindow;
use super::status_bar::StatusBar;
use crate::core::captions::{
    self, DialogCaptions, DialogItem, IndexCaptions, SETTINGS_GROUPS, failure_text,
};
use crate::core::commands::{CommandId, Keystroke};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::include::SummaryRow;
use crate::core::layout::{self, Px, Rect, StatusLayout, StatusWidths};
use crate::core::model::Model;
use crate::core::open::{browse_command, total_commander_command};
use crate::core::results::Fix;
use crate::core::settings::{
    ContextItem, ItemVisibility, SettingsDraft, SettingsPage, SettingsValues,
};
use crate::core::shortcuts::{
    CommandRow, CommandShortcuts, ShowShortcut, command_place, command_rows, show_refusal_message,
};
use crate::core::speed::{ENGINES, Speed, SpeedNumber, SpeedPreset, ring_blocks};
use crate::core::text::{LANGUAGES, LanguageChoice, Strings, without_access_key};
use crate::win::controls::{
    Balloon, Button, CheckBox, DropDownList, Edit, HotkeyControl, HotkeyValue, ListBox, ReportList,
    Tooltip, show_group_glyph,
};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::gdi::{Font, TextMeter};
use crate::win::{self, Hwnd};
use gozler_core::include::IncludeSettings;
use gozler_fileops::{DeleteWorkers, Engine, RingBlock, SpeedSettings};
use std::cell::{Cell, OnceCell, RefCell};

/// The Index page's timer, which shows the service's latest state.
const INDEX_TIMER: usize = 1;
const INDEX_REFRESH_MS: u32 = 2_000;
/// The Include page's timer, which asks for the preview's count once the
/// person stops changing the settings for a moment.
const PREVIEW_TIMER: usize = 2;
const PREVIEW_DELAY_MS: u32 = 300;

/// The page's heading, which shows its name in semibold.
fn heading(page: SettingsPage) -> ControlId {
    match page {
        SettingsPage::General => ControlId::GeneralHeading,
        SettingsPage::Search => ControlId::SearchHeading,
        SettingsPage::ContextMenu => ControlId::ContextMenuHeading,
        SettingsPage::Keyboard => ControlId::KeyboardHeading,
        SettingsPage::FileOperations => ControlId::SpeedHeading,
        SettingsPage::Index => ControlId::IndexHeading,
        SettingsPage::Include => ControlId::IncludeHeading,
    }
}

/// The value of [`SettingsValues`] that a control shows.
type Field<T> = fn(&mut SettingsValues) -> &mut T;

/// Each check box, its page, and the value it shows.
const CHECK_BOXES: [(SettingsPage, ControlId, Field<bool>); 9] = [
    (SettingsPage::General, ControlId::GeneralSignIn, |v| {
        &mut v.start_at_sign_in
    }),
    (SettingsPage::General, ControlId::GeneralUpdates, |v| {
        &mut v.choices.check_for_updates
    }),
    (SettingsPage::General, ControlId::GeneralKeepRunning, |v| {
        &mut v.choices.keep_running
    }),
    (SettingsPage::General, ControlId::GeneralOneWindow, |v| {
        &mut v.choices.one_window
    }),
    (SettingsPage::General, ControlId::GeneralShowIcons, |v| {
        &mut v.choices.show_icons
    }),
    (SettingsPage::Search, ControlId::SearchAsYouType, |v| {
        &mut v.choices.search_as_you_type
    }),
    (SettingsPage::Search, ControlId::SearchShowAll, |v| {
        &mut v.choices.show_all_when_empty
    }),
    (SettingsPage::Search, ControlId::SearchSelectOnClick, |v| {
        &mut v.choices.select_on_click
    }),
    (
        SettingsPage::Search,
        ControlId::SearchSelectOnActivate,
        |v| &mut v.choices.select_on_activate,
    ),
];

/// Each open command's box, its Browse… button, its cue and the command it
/// shows.
const OPEN_COMMANDS: [(ControlId, ControlId, &str, Field<String>); 3] = [
    (
        ControlId::ContextMenuOpenFolders,
        ControlId::ContextMenuOpenFoldersBrowse,
        "settings_context_menu.cue_explorer",
        |v| &mut v.choices.open_folders,
    ),
    (
        ControlId::ContextMenuOpenFiles,
        ControlId::ContextMenuOpenFilesBrowse,
        "settings_context_menu.cue_default_program",
        |v| &mut v.choices.open_files,
    ),
    (
        ControlId::ContextMenuOpenLocation,
        ControlId::ContextMenuOpenLocationBrowse,
        "settings_context_menu.cue_explorer",
        |v| &mut v.choices.open_file_location,
    ),
];

/// The Context menu page's option buttons and the visibility each sets.
const VISIBILITY_BUTTONS: [(ControlId, ItemVisibility); 3] = [
    (ControlId::ContextMenuShow, ItemVisibility::Shown),
    (
        ControlId::ContextMenuShowWithShift,
        ItemVisibility::WithShift,
    ),
    (ControlId::ContextMenuHide, ItemVisibility::Hidden),
];

/// The Include setting that a check box shows.
type IncludeField = fn(&mut IncludeSettings) -> &mut bool;

/// Each check box of the Include page and the setting it shows.
const INCLUDE_CHECK_BOXES: [(ControlId, IncludeField); 3] = [
    (ControlId::IncludeNewDrives, |i| &mut i.include_new_drives),
    (ControlId::IncludeHidden, |i| &mut i.exclude_hidden),
    (ControlId::IncludeSystem, |i| &mut i.exclude_system),
];

/// The Include page's controls that change the settings, which are
/// disabled while the service gives none.
const INCLUDE_CONTROLS: [ControlId; 7] = [
    ControlId::IncludeFolders,
    ControlId::IncludeModify,
    ControlId::IncludeNewDrives,
    ControlId::IncludeNames,
    ControlId::IncludeHidden,
    ControlId::IncludeSystem,
    ControlId::IncludeRestore,
];

/// The speed setting that a control of the File operations page shows.
type SpeedField<T> = fn(&mut SpeedSettings) -> &mut T;

/// The File operations page's number boxes and the number each shows.
const SPEED_NUMBERS: [(ControlId, SpeedNumber); 9] = [
    (ControlId::SpeedLargeFileLimit, SpeedNumber::LargeFileLimit),
    (ControlId::SpeedWorkersSsd, SpeedNumber::WorkersSsd),
    (ControlId::SpeedWorkersUsb, SpeedNumber::WorkersUsb),
    (
        ControlId::SpeedWorkersHardDisk,
        SpeedNumber::WorkersHardDisk,
    ),
    (ControlId::SpeedWorkersShare, SpeedNumber::WorkersShare),
    (
        ControlId::SpeedRingSsdInFlight,
        SpeedNumber::RingSsdInFlight,
    ),
    (
        ControlId::SpeedRingHardDiskInFlight,
        SpeedNumber::RingHardDiskInFlight,
    ),
    (
        ControlId::SpeedRingHardDiskBuffer,
        SpeedNumber::RingHardDiskBuffer,
    ),
    (ControlId::SpeedDeleteWorkers, SpeedNumber::DeleteWorkers),
];

/// The File operations page's engine lists, the name each gives the
/// Windows copy function, and the setting each shows.
const SPEED_ENGINES: [(ControlId, &str, SpeedField<Engine>); 2] = [
    (
        ControlId::SpeedLargeBetweenSsds,
        "settings_file_operations.engine_windows_unbuffered",
        |s| &mut s.large_between_ssds,
    ),
    (
        ControlId::SpeedHardDisk,
        "settings_file_operations.engine_windows",
        |s| &mut s.hard_disk,
    ),
];

/// The File operations page's block size lists and the block each shows.
const SPEED_BLOCKS: [(ControlId, SpeedField<RingBlock>); 2] = [
    (ControlId::SpeedRingSsdBlock, |s| &mut s.ring_ssd.block),
    (ControlId::SpeedRingHardDiskBlock, |s| {
        &mut s.ring_hard_disk.block
    }),
];

/// Each page's Reset.
const RESTORE: [(SettingsPage, ControlId); 6] = [
    (SettingsPage::General, ControlId::GeneralRestore),
    (SettingsPage::Search, ControlId::SearchRestore),
    (SettingsPage::ContextMenu, ControlId::ContextMenuRestore),
    (SettingsPage::Keyboard, ControlId::KeyboardRestore),
    (SettingsPage::FileOperations, ControlId::SpeedRestore),
    (SettingsPage::Include, ControlId::IncludeRestore),
];

fn is(id: u16, control: ControlId) -> bool {
    id == control.id()
}

/// Moves the focus to the page list when `control` has it and is, or is
/// about to be, hidden or disabled, because such a control keeps the focus
/// but takes no key.
fn focus_off(dialog: Hwnd, control: Hwnd) {
    if Hwnd::focused() == Some(control)
        && let Some(pages) = dialog::item(dialog, ControlId::SettingsPages.id())
    {
        pages.focus();
    }
}

/// The Settings dialog (DESIGN.md: Settings dialog).
pub(super) struct SettingsDialog<'a> {
    main: &'a MainWindow,
    /// The main window, which applies the settings.
    window: Hwnd,
    strings: Strings,
    draft: RefCell<SettingsDraft>,
    /// The pages in [`SettingsPage::ALL`]'s order, once the dialog made
    /// them.
    pages: RefCell<Vec<Hwnd>>,
    /// The pages the page list shows, which the search box keeps; the
    /// shown page is one of them, or none when the list is empty.
    listed: RefCell<Vec<SettingsPage>>,
    shown: Cell<Option<SettingsPage>>,
    /// The commands the Keyboard page lists, after its filter.
    commands: RefCell<Vec<CommandRow>>,
    /// What the Index page shows, so a refresh that changes nothing sets
    /// no row.
    index: RefCell<Option<IndexCaptions>>,
    /// The Include settings the service was last asked to count, and its
    /// answer once it arrived; the preview shows the answer only while the
    /// page still holds those settings.
    counted: RefCell<Option<(IncludeSettings, Option<u64>)>>,
    /// The rows that Folders Gozler indexes shows, whose tooltips list the
    /// full paths of the folders left out.
    include_rows: RefCell<Vec<SummaryRow>>,
    /// Whether the dialog is setting its controls, whose notifications then
    /// change nothing.
    filling: Cell<bool>,
    balloon: OnceCell<Balloon>,
    /// The Drives and service page's tooltip, whose fix buttons' tips
    /// follow their fix.
    index_tips: OnceCell<Tooltip>,
    /// The pages' font and its bold form, for the captions that the search
    /// finds; they outlive the dialog's controls.
    highlight: OnceCell<(Font, Font)>,
    /// The status bar at the dialog's bottom (DESIGN.md: Status bar).
    status: OnceCell<StatusBar>,
    /// The status bar's group widths and its font's height, which place it
    /// once the dialog's buttons have their width.
    status_widths: OnceCell<(StatusWidths, Px)>,
}

impl<'a> SettingsDialog<'a> {
    pub(super) fn new(
        main: &'a MainWindow,
        window: Hwnd,
        strings: Strings,
        values: SettingsValues,
    ) -> SettingsDialog<'a> {
        SettingsDialog {
            main,
            window,
            strings,
            draft: RefCell::new(SettingsDraft::new(values)),
            pages: RefCell::new(Vec::new()),
            listed: RefCell::new(SettingsPage::ALL.to_vec()),
            shown: Cell::new(None),
            commands: RefCell::new(Vec::new()),
            index: RefCell::new(None),
            counted: RefCell::new(None),
            include_rows: RefCell::new(Vec::new()),
            filling: Cell::new(false),
            balloon: OnceCell::new(),
            index_tips: OnceCell::new(),
            highlight: OnceCell::new(),
            status: OnceCell::new(),
            status_widths: OnceCell::new(),
        }
    }

    /// Adds the status bar below OK, Cancel and Apply and makes the dialog
    /// that much taller; `buttons_sized` places its groups and controls.
    /// The bar's font is the dialog's, which `fonts` keeps.
    fn add_status_bar(&self, dialog: Hwnd, fonts: &mut Vec<Font>) -> Option<StatusBar> {
        let (Ok(status), Ok(font)) = (StatusBar::for_dialog(dialog), Font::of(dialog)) else {
            return None;
        };
        for control in status.text_controls() {
            control.set_font(&font);
        }
        let (widest, cancel) = self.main.settings_status_texts();
        status.set_cancel_caption(&cancel);
        let (widths, font_height) = {
            let meter = TextMeter::new(&font);
            let widths = StatusWidths::measure(&widest, &|text| meter.width(text));
            (widths, meter.height())
        };
        fonts.push(font);
        let dpi = dialog.dpi();
        let min_height = StatusLayout::min_height(dpi, font_height);
        let height = status.fit(min_height);
        let outer = dialog.screen_rect();
        dialog.set_rect(Rect {
            height: outer.height + height,
            ..outer
        });
        let _ = self.status_widths.set((widths, font_height));
        let _ = self.status.set(status);
        Some(status)
    }

    /// Widens the page list to show its longest page name in the dialog's
    /// language whole, after its icon, and the dialog with it, so the pages
    /// keep their width.
    fn fit_page_list(&self, dialog: Hwnd) {
        let Some(list) = dialog::item(dialog, ControlId::SettingsPages.id()) else {
            return;
        };
        let Ok(font) = Font::of(list) else {
            return;
        };
        let longest = {
            let meter = TextMeter::new(&font);
            SettingsPage::ALL
                .into_iter()
                .map(|page| meter.plain_width(self.strings.text(page.label())))
                .max()
                .unwrap_or_default()
        };
        let rect = list.rect_in(dialog);
        let frame = rect.width - list.client_size().width;
        let extra = layout::glyph_list_item_width(longest, dialog.dpi()) + frame - rect.width;
        if extra > Px(0) {
            dialog::widen(dialog, rect.right(), extra);
        }
    }

    fn page(&self, page: SettingsPage) -> Option<Hwnd> {
        let index = SettingsPage::ALL.iter().position(|p| *p == page)?;
        self.pages.borrow().get(index).copied()
    }

    fn control(&self, page: SettingsPage, control: ControlId) -> Option<Hwnd> {
        dialog::item(self.page(page)?, control.id())
    }

    fn check_box(&self, page: SettingsPage, control: ControlId) -> Option<CheckBox> {
        CheckBox::of_dialog(self.page(page)?, control.id())
    }

    fn hotkey(&self) -> Option<HotkeyControl> {
        HotkeyControl::of_dialog(
            self.page(SettingsPage::Keyboard)?,
            ControlId::KeyboardShortcut.id(),
        )
    }

    fn language_list(&self) -> Option<DropDownList> {
        DropDownList::of_dialog(
            self.page(SettingsPage::General)?,
            ControlId::GeneralLanguage.id(),
        )
    }

    fn command_list(&self) -> Option<ReportList> {
        ReportList::of_dialog(
            self.page(SettingsPage::Keyboard)?,
            ControlId::KeyboardCommands.id(),
        )
    }

    fn key_list(&self) -> Option<ListBox> {
        ListBox::of_dialog(
            self.page(SettingsPage::Keyboard)?,
            ControlId::KeyboardKeys.id(),
        )
    }

    /// Runs `fill`, whose control changes then change no setting.
    fn filling(&self, fill: impl FnOnce()) {
        let was = self.filling.replace(true);
        fill();
        self.filling.set(was);
    }

    /// Sets every control of `page` from the edited values.
    fn fill(&self, page: SettingsPage) {
        let mut values = self.draft.borrow().edited.clone();
        self.filling(|| match page {
            SettingsPage::General | SettingsPage::Search => {
                for (on, control, value) in CHECK_BOXES {
                    if on == page
                        && let Some(check_box) = self.check_box(page, control)
                    {
                        check_box.set_checked(*value(&mut values));
                    }
                }
                if page == SettingsPage::General
                    && let Some(list) = self.language_list()
                {
                    // A saved tag that no longer ships shows as Windows
                    // default; it stays saved unless the person chooses.
                    let place = values.choices.language.shipped();
                    list.select(place.map_or(0, |index| index + 1));
                }
            }
            SettingsPage::ContextMenu => self.fill_context_menu(&mut values),
            SettingsPage::Keyboard => {
                let shortcut = values.choices.show_shortcut;
                let windows = shortcut.is_some_and(ShowShortcut::windows);
                if let Some(check_box) = self.check_box(page, ControlId::KeyboardWindowsKey) {
                    check_box.set_checked(windows);
                }
                if let Some(hotkey) = self.hotkey() {
                    hotkey.require_two_modifiers(!windows);
                    hotkey.set(shortcut.map(ShowShortcut::keystroke));
                }
                self.update_clear_shortcut();
                self.fill_commands(self.selected_command());
            }
            SettingsPage::FileOperations => self.fill_speed(&values.choices.speed),
            SettingsPage::Index => {}
            SettingsPage::Include => self.fill_include(values.include.as_mut()),
        });
        self.show_closed_note();
    }

    /// Sets the File operations page's controls from `speed`.
    fn fill_speed(&self, speed: &Speed) {
        self.show_preset(speed.preset);
        let page = SettingsPage::FileOperations;
        let mut settings = speed.settings();
        for (control, number) in SPEED_NUMBERS {
            if let Some(edit) = self.control(page, control) {
                edit.set_text(&number.get(&settings).to_string());
            }
        }
        let per_processor = settings.delete_workers == DeleteWorkers::PerProcessor;
        if let Some(check_box) = self.check_box(page, ControlId::SpeedPerProcessor) {
            check_box.set_checked(per_processor);
        }
        if let Some(edit) = self.control(page, ControlId::SpeedDeleteWorkers) {
            edit.set_enabled(!per_processor);
        }
        for (control, _, engine) in SPEED_ENGINES {
            let shown = *engine(&mut settings);
            let place = ENGINES.iter().position(|each| *each == shown);
            if let (Some(list), Some(place)) = (self.speed_list(control), place) {
                list.select(place);
            }
        }
        for (control, block) in SPEED_BLOCKS {
            let shown = *block(&mut settings);
            let place = ring_blocks().position(|each| each == shown);
            if let (Some(list), Some(place)) = (self.speed_list(control), place) {
                list.select(place);
            }
        }
    }

    /// Selects `preset` in the Speed settings list, with the sentence under
    /// it that says what it does.
    fn show_preset(&self, preset: SpeedPreset) {
        let place = SpeedPreset::ALL.iter().position(|each| *each == preset);
        if let (Some(list), Some(place)) = (self.speed_list(ControlId::SpeedPreset), place) {
            list.select(place);
        }
        if let Some(label) = self.control(SettingsPage::FileOperations, ControlId::SpeedDescription)
        {
            label.set_text(self.strings.text(preset.texts().1));
        }
    }

    fn speed_list(&self, control: ControlId) -> Option<DropDownList> {
        DropDownList::of_dialog(self.page(SettingsPage::FileOperations)?, control.id())
    }

    /// Changes one of the File operations page's values, which makes its
    /// choice Custom.
    fn change_speed(&self, change: impl FnOnce(&mut SpeedSettings)) {
        let preset = {
            let speed = &mut self.draft.borrow_mut().edited.choices.speed;
            speed.change(change);
            speed.preset
        };
        self.show_preset(preset);
    }

    /// The first number box of the File operations page whose text its
    /// range refuses, with the message that says the range; such a box is
    /// refused when OK or Apply is pressed (DESIGN.md: Dialogs).
    fn read_speed(&self) -> Result<(), (ControlId, String)> {
        let mut settings = self.draft.borrow().edited.choices.speed.settings();
        for (control, number) in SPEED_NUMBERS {
            let Some(edit) = self
                .control(SettingsPage::FileOperations, control)
                .filter(|edit| edit.is_enabled())
            else {
                continue;
            };
            if let Err(range) = number.set_text(&mut settings, &edit.text()) {
                let message = self.strings.format(
                    "settings_file_operations.out_of_range",
                    &[&range.min.to_string(), &range.max.to_string()],
                );
                return Err((control, message));
            }
        }
        Ok(())
    }

    /// The item that the Context menu page's list selects.
    fn selected_context_item(&self) -> Option<ContextItem> {
        let list = ListBox::of_dialog(
            self.page(SettingsPage::ContextMenu)?,
            ControlId::ContextMenuItems.id(),
        )?;
        ContextItem::all().get(list.selected()?).copied()
    }

    /// Sets the Context menu page's controls from `values`: the open
    /// commands, and the group of the item that its list selects.
    fn fill_context_menu(&self, values: &mut SettingsValues) {
        for (control, _, _, command) in OPEN_COMMANDS {
            if let Some(edit) = self.control(SettingsPage::ContextMenu, control) {
                edit.set_text(command(values));
            }
        }
        let (Some(item), Some(page)) = (
            self.selected_context_item(),
            self.page(SettingsPage::ContextMenu),
        ) else {
            return;
        };
        if let Some(group) = dialog::item(page, ControlId::ContextMenuItemGroup.id()) {
            let name = without_access_key(self.strings.text(item.label()));
            group.set_text(
                &self
                    .strings
                    .format("settings_context_menu.settings_for", &[&name]),
            );
        }
        let visibility = values.choices.context_menu.visibility(item);
        for (control, shown) in VISIBILITY_BUTTONS {
            if let Some(button) = CheckBox::of_dialog(page, control.id()) {
                button.set_checked(shown == visibility);
            }
        }
    }

    /// Sets the Include page's controls from `include`; without it, the
    /// service gave no settings, so they are disabled.
    fn fill_include(&self, include: Option<&mut IncludeSettings>) {
        for control in INCLUDE_CONTROLS {
            if let Some(control) = self.control(SettingsPage::Include, control) {
                control.set_enabled(include.is_some());
            }
        }
        let Some(include) = include else {
            return;
        };
        for (control, value) in INCLUDE_CHECK_BOXES {
            if let Some(check_box) = self.check_box(SettingsPage::Include, control) {
                check_box.set_checked(*value(include));
            }
        }
        if let Some(edit) = self.control(SettingsPage::Include, ControlId::IncludeNames) {
            edit.set_text(&include.excluded_names);
        }
        let drives = self.main.include_drives(include);
        let rows = self.main.include_summary(&drives, include);
        let cells: Vec<[String; 3]> = rows.iter().map(|row| row.cells.clone()).collect();
        self.include_rows.replace(rows);
        if let Some(list) = self.include_folders() {
            list.set_rows(&cells, None);
        }
    }

    /// The Include page's list of the indexed folders.
    fn include_folders(&self) -> Option<ReportList> {
        self.page(SettingsPage::Include)
            .and_then(|page| ReportList::of_dialog(page, ControlId::IncludeFolders.id()))
    }

    /// Shows the preview line: the entries indexed now and, after OK, the
    /// service's count of the edited settings (DESIGN.md: The Include
    /// page).
    fn show_preview(&self) {
        let Some(label) = self.control(SettingsPage::Include, ControlId::IncludePreview) else {
            return;
        };
        let (edited, applied) = {
            let draft = self.draft.borrow();
            (
                draft.edited.include.clone(),
                draft.last_applied().include.clone(),
            )
        };
        let (Some(edited), Some(applied)) = (edited, applied) else {
            // The service's state or failure, as the Index page says it.
            label.set_text(&self.main.index_captions().service);
            return;
        };
        let now = self.main.indexed_entries();
        let after = if edited == applied {
            Some(now)
        } else {
            match &*self.counted.borrow() {
                Some((counted, entries)) if *counted == edited => *entries,
                _ => None,
            }
        };
        let drives = self.main.include_drives(&edited);
        let volumes: Vec<&str> = drives
            .iter()
            .filter(|d| d.root.is_some())
            .map(|d| d.volume.as_str())
            .collect();
        let adds = edited.adds_to(&applied, &volumes);
        label.set_text(&self.main.include_preview(now, after, adds));
    }

    /// The Include settings changed: the preview asks the service for a new
    /// count once the person stops changing them for a moment.
    fn include_changed(&self, dialog: Hwnd) {
        self.counted.replace(None);
        self.show_preview();
        dialog.set_timer(PREVIEW_TIMER, PREVIEW_DELAY_MS);
    }

    /// Asks the service to count the edited Include settings, unless they
    /// are the applied ones, whose count is the index's.
    fn count_preview(&self, dialog: Hwnd) {
        dialog.kill_timer(PREVIEW_TIMER);
        let (edited, applied) = {
            let draft = self.draft.borrow();
            (
                draft.edited.include.clone(),
                draft.last_applied().include.clone(),
            )
        };
        if let Some(edited) = edited
            && Some(&edited) != applied.as_ref()
        {
            self.counted.replace(Some((edited.clone(), None)));
            self.main.count_included(edited);
        }
    }

    /// The preview's count, which the main window holds.
    fn preview_counted(&self) {
        let Some(counted) = self.main.take_included_count() else {
            return;
        };
        match counted {
            Ok(entries) => {
                if let Some((_, answer)) = &mut *self.counted.borrow_mut() {
                    *answer = Some(entries);
                }
                self.show_preview();
            }
            Err(failure) => {
                if let Some(label) = self.control(SettingsPage::Include, ControlId::IncludePreview)
                {
                    label.set_text(&failure_text(&self.strings, &failure));
                }
            }
        }
    }

    /// Modify… opens Indexed folders, whose OK puts its choices on the page.
    fn modify_folders(&self, dialog: Hwnd) {
        let Some(include) = self.draft.borrow().edited.include.clone() else {
            return;
        };
        let drives = self.main.include_drives(&include);
        let state = IndexedFolders::new(self.main, &self.strings, &drives, include);
        let captions = DialogCaptions::new(DialogId::IndexedFolders, &self.strings);
        let ended = dialog::run_modal(dialog, DialogId::IndexedFolders, &captions, &state);
        if matches!(ended, Ok(DialogEnd::Ok)) {
            self.draft.borrow_mut().edited.include = Some(state.into_settings());
            self.fill(SettingsPage::Include);
            self.include_changed(dialog);
        }
    }

    /// Saves the edited Include settings, when they changed, as
    /// administrator, and answers whether they are saved.
    fn save_include(&self, dialog: Hwnd, edited: &SettingsValues, last: &SettingsValues) -> bool {
        let Some(include) = edited
            .include
            .as_ref()
            .filter(|_| edited.include != last.include)
        else {
            return true;
        };
        match self.main.save_include(dialog, include) {
            Ok(()) => true,
            Err(NotSaved::Declined) => false,
            Err(NotSaved::Failed(reason)) => {
                let text = self
                    .strings
                    .format("settings_include.not_saved", &[&reason]);
                win::dialog::error_box(Some(dialog), &text, self.strings.text("window.title"));
                false
            }
        }
    }

    /// The Keyboard page says under the shortcut whether it works after the
    /// window closes, which follows Keep Gozler running on the General page.
    fn show_closed_note(&self) {
        let keeps_running = self.draft.borrow().edited.choices.keep_running;
        let key = if keeps_running {
            "settings_keyboard.running_note"
        } else {
            "settings_keyboard.closed_note"
        };
        if let Some(note) = self.control(SettingsPage::Keyboard, ControlId::KeyboardClosedNote) {
            note.set_text(self.strings.text(key));
        }
    }

    /// Lists the commands that the filter keeps, with `selected` selected,
    /// or the first.
    fn fill_commands(&self, selected: Option<CommandId>) {
        let filter = self
            .control(SettingsPage::Keyboard, ControlId::KeyboardFilter)
            .map(Hwnd::text)
            .unwrap_or_default();
        let rows = {
            let draft = self.draft.borrow();
            command_rows(&self.strings, &draft.edited.choices.shortcuts, &filter)
        };
        let index = selected
            .and_then(|command| rows.iter().position(|row| row.command == command))
            .or((!rows.is_empty()).then_some(0));
        let cells: Vec<[String; 2]> = rows
            .iter()
            .map(|row| [row.place.clone(), row.keys.clone()])
            .collect();
        self.commands.replace(rows);
        if let Some(list) = self.command_list() {
            self.filling(|| list.set_rows(&cells, index));
        }
        self.fill_keys(0);
    }

    /// Lists the selected command's shortcuts, with the one at `selected`
    /// selected.
    fn fill_keys(&self, selected: usize) {
        let keys: Vec<String> = match self.selected_command() {
            Some(command) => {
                let draft = self.draft.borrow();
                draft
                    .edited
                    .choices
                    .shortcuts
                    .of(command)
                    .iter()
                    .map(|key| key.text(&self.strings))
                    .collect()
            }
            None => Vec::new(),
        };
        if let Some(list) = self.key_list() {
            self.filling(|| list.set_items(&keys, selected.min(keys.len().saturating_sub(1))));
        }
        self.update_key_buttons();
    }

    fn selected_command(&self) -> Option<CommandId> {
        let index = self.command_list()?.selected()?;
        self.commands.borrow().get(index).map(|row| row.command)
    }

    fn selected_key(&self) -> Option<Keystroke> {
        let command = self.selected_command()?;
        let index = self.key_list()?.selected()?;
        let draft = self.draft.borrow();
        draft
            .edited
            .choices
            .shortcuts
            .of(command)
            .get(index)
            .copied()
    }

    /// Add… needs a command; Edit… and Remove need one of its shortcuts.
    fn update_key_buttons(&self) {
        let command = self.selected_command().is_some();
        let key = self.selected_key().is_some();
        for (control, enabled) in [
            (ControlId::KeyboardAdd, command),
            (ControlId::KeyboardEdit, key),
            (ControlId::KeyboardRemove, key),
        ] {
            if let Some(button) = self.control(SettingsPage::Keyboard, control) {
                button.set_enabled(enabled);
            }
        }
    }

    /// The shortcut to show Gozler that the hotkey control and the Windows
    /// key check box hold, or the message that says why it cannot be one.
    fn read_shortcut(&self) -> Result<Option<ShowShortcut>, String> {
        let windows = self
            .check_box(SettingsPage::Keyboard, ControlId::KeyboardWindowsKey)
            .is_some_and(CheckBox::is_checked);
        match self
            .hotkey()
            .map_or(HotkeyValue::Empty, HotkeyControl::value)
        {
            HotkeyValue::Empty => Ok(None),
            HotkeyValue::Unsupported => Err(self
                .strings
                .text("settings_keyboard.unsupported")
                .to_owned()),
            HotkeyValue::Keys(keystroke) => ShowShortcut::new(windows, keystroke)
                .map(Some)
                .map_err(|refusal| {
                    show_refusal_message(&self.strings, refusal, windows, keystroke)
                }),
        }
    }

    /// Keeps the shortcut the controls hold, when it can be one; one that
    /// cannot is refused when OK or Apply is pressed (DESIGN.md: Dialogs).
    fn shortcut_changed(&self) {
        if let Ok(shortcut) = self.read_shortcut() {
            self.draft.borrow_mut().edited.choices.show_shortcut = shortcut;
        }
        self.update_clear_shortcut();
    }

    /// Clear is enabled while the shortcut box holds keys.
    fn update_clear_shortcut(&self) {
        if let (Some(hotkey), Some(clear)) = (
            self.hotkey(),
            self.control(SettingsPage::Keyboard, ControlId::KeyboardClear),
        ) {
            clear.set_enabled(hotkey.value() != HotkeyValue::Empty);
        }
    }

    /// With the Windows key off, a shortcut needs two of `Ctrl`, `Alt` and
    /// `Shift`, so the key shown gets `Ctrl+Alt` as the control would give
    /// it.
    fn windows_key_changed(&self) {
        let windows = self
            .check_box(SettingsPage::Keyboard, ControlId::KeyboardWindowsKey)
            .is_some_and(CheckBox::is_checked);
        if let Some(hotkey) = self.hotkey() {
            hotkey.require_two_modifiers(!windows);
            if !windows && let HotkeyValue::Keys(keystroke) = hotkey.value() {
                let fixed = ShowShortcut::two_modifiers(keystroke);
                if fixed != keystroke {
                    self.filling(|| hotkey.set(Some(fixed)));
                }
            }
        }
        self.shortcut_changed();
    }

    /// Apply is enabled while a value differs from what was applied
    /// (DESIGN.md: Dialogs), a refused shortcut included. OK and Apply show
    /// the administrator shield while the Include settings differ, because
    /// saving them asks Windows for permission.
    fn update_apply(&self, dialog: Hwnd) {
        let (changed, include_changed) = {
            let draft = self.draft.borrow();
            (
                draft.can_apply(),
                draft.edited.include != draft.last_applied().include,
            )
        };
        let changed = changed || self.read_shortcut().is_err() || self.read_speed().is_err();
        if let Some(apply) = dialog::item(dialog, ControlId::SettingsApply.id()) {
            apply.set_enabled(changed);
        }
        for button in [DialogItem::Ok.id(), ControlId::SettingsApply.id()] {
            if let Some(button) = Button::of_dialog(dialog, button) {
                button.set_shield(include_changed);
            }
        }
    }

    /// Shows `page`, or no page, and has the main window follow the service
    /// while the Index page shows (DESIGN.md: Settings dialog).
    fn show_page(&self, dialog: Hwnd, page: Option<SettingsPage>) {
        let previous = self.shown.replace(page);
        self.fill_page_list(dialog);
        if let Some(status) = self.status.get() {
            status.show_view(&captions::settings_status_view(&self.strings, page));
        }
        if previous == page {
            return;
        }
        let pages = self.pages.borrow().clone();
        for (each, hwnd) in SettingsPage::ALL.into_iter().zip(pages) {
            hwnd.set_visible(Some(each) == page);
        }
        if page == Some(SettingsPage::Include) {
            // The index changes while the dialog is open.
            self.show_preview();
        }
        if page == Some(SettingsPage::Index) {
            self.main.show_index_page(self.window, true);
            dialog.set_timer(INDEX_TIMER, INDEX_REFRESH_MS);
            self.refresh_index(dialog);
        } else if previous == Some(SettingsPage::Index) {
            dialog.kill_timer(INDEX_TIMER);
            self.main.show_index_page(self.window, false);
        }
    }

    /// Lists the pages that the search keeps, with the shown one selected.
    fn fill_page_list(&self, dialog: Hwnd) {
        let Some(list) = ListBox::of_dialog(dialog, ControlId::SettingsPages.id()) else {
            return;
        };
        let (labels, glyphs, index) = {
            let listed = self.listed.borrow();
            let shown = self.shown.get();
            let labels: Vec<&str> = listed
                .iter()
                .map(|page| self.strings.text(page.label()))
                .collect();
            let glyphs: Vec<Glyph> = listed.iter().map(|page| page.glyph()).collect();
            (
                labels,
                glyphs,
                listed.iter().position(|page| Some(*page) == shown),
            )
        };
        self.filling(|| list.set_items(&labels, index.unwrap_or(0)));
        list.set_glyphs(glyphs);
    }

    /// Keeps the pages that the search box's words find. The shown page
    /// stays while it is one of them; otherwise the first one shows, or
    /// none, with a line that says so.
    fn search_changed(&self, dialog: Hwnd) {
        let search = dialog::item(dialog, ControlId::SettingsSearch.id())
            .map(Hwnd::text)
            .unwrap_or_default();
        let listed = captions::settings_pages_matching(&self.strings, &search);
        let page = self
            .shown
            .get()
            .filter(|shown| listed.contains(shown))
            .or(listed.first().copied());
        self.listed.replace(listed);
        if let Some(clear) = dialog::item(dialog, ControlId::SettingsClearSearch.id()) {
            clear.set_enabled(!search.is_empty());
        }
        if let Some(no_match) = dialog::item(dialog, ControlId::SettingsNoMatch.id()) {
            no_match.set_visible(page.is_none());
        }
        self.show_page(dialog, page);
        self.highlight(&search);
    }

    /// Shows in bold each caption that holds a word of `search`; a page's
    /// heading keeps its own font.
    fn highlight(&self, search: &str) {
        let Some((normal, bold)) = self.highlight.get() else {
            return;
        };
        for page in SettingsPage::ALL {
            for (control, hit) in captions::settings_page_hits(&self.strings, page, search) {
                if control != heading(page)
                    && let Some(control) = self.control(page, control)
                {
                    control.set_font(if hit { bold } else { normal });
                }
            }
        }
    }

    /// Shows the service's latest state, and its failure's fix, on the
    /// Index page.
    fn refresh_index(&self, dialog: Hwnd) {
        let captions = self.main.index_captions();
        if self.index.borrow().as_ref() == Some(&captions) {
            return;
        }
        // Stored first, because the drive list asks for its empty text
        // while its rows change.
        self.index.replace(Some(captions.clone()));
        if let Some(list) = self
            .page(SettingsPage::Index)
            .and_then(|page| ReportList::of_dialog(page, ControlId::IndexDrives.id()))
        {
            list.set_rows(&captions.drives, None);
        }
        if let Some(service) = self.control(SettingsPage::Index, ControlId::IndexService) {
            service.set_text(&captions.service);
        }
        let fixes = [
            (ControlId::IndexUseLocal, &captions.use_local),
            (ControlId::IndexFix, &captions.fix),
        ];
        for (control, fix) in fixes {
            let Some(button) = self
                .page(SettingsPage::Index)
                .and_then(|page| Button::of_dialog(page, control.id()))
            else {
                continue;
            };
            let hwnd = button.hwnd();
            if !fix.as_ref().is_some_and(|fix| fix.enabled) {
                focus_off(dialog, hwnd);
            }
            if let Some(fix) = fix {
                hwnd.set_text(&fix.label);
                button.set_shield(fix.shield);
                hwnd.set_enabled(fix.enabled);
                if let Some(tips) = self.index_tips.get() {
                    tips.set(hwnd, fix.tooltip.as_deref().unwrap_or_default());
                }
            }
            hwnd.set_visible(fix.is_some());
        }
        if let Some(report) = self.control(SettingsPage::Index, ControlId::IndexReport) {
            if !captions.has_report {
                focus_off(dialog, report);
            }
            report.set_enabled(captions.has_report);
        }
    }

    /// Opens the service's report, as it was when the button was clicked.
    fn show_report(&self, dialog: Hwnd) {
        let Some(text) = self.main.service_report() else {
            return;
        };
        let captions = DialogCaptions::new(DialogId::ServiceReport, &self.strings);
        let _ = dialog::run_modal(
            dialog,
            DialogId::ServiceReport,
            &captions,
            &ServiceReport { text },
        );
        // The service may have stopped while the report was open, which
        // disabled Show report, and Windows gives the focus back to it.
        if let Some(report) = self.control(SettingsPage::Index, ControlId::IndexReport)
            && !report.is_enabled()
        {
            focus_off(dialog, report);
        }
    }

    /// Points the balloon `text` at `control` on `page`, which shows, and
    /// focuses it.
    fn refuse(&self, dialog: Hwnd, page: SettingsPage, control: ControlId, text: &str) {
        // The search may have left the page out of the list.
        let listed = self.listed.borrow().contains(&page);
        if !listed && let Some(search) = dialog::item(dialog, ControlId::SettingsSearch.id()) {
            self.filling(|| search.set_text(""));
            self.search_changed(dialog);
        }
        self.show_page(dialog, Some(page));
        if let Some(control) = self.control(page, control) {
            control.focus();
            if let Some(balloon) = self.balloon.get() {
                balloon.show(control, text);
            }
        }
    }

    /// Gives the edited values to the main window, and answers whether they
    /// were applied; a shortcut that cannot be one, or that another program
    /// holds, keeps the dialog open (DESIGN.md: Dialogs).
    fn apply(&self, dialog: Hwnd) -> bool {
        if let Err(message) = self.read_shortcut() {
            self.refuse(
                dialog,
                SettingsPage::Keyboard,
                ControlId::KeyboardShortcut,
                &message,
            );
            return false;
        }
        if let Err((control, message)) = self.read_speed() {
            self.refuse(dialog, SettingsPage::FileOperations, control, &message);
            return false;
        }
        let (edited, last) = {
            let draft = self.draft.borrow();
            (draft.edited.clone(), draft.last_applied().clone())
        };
        if edited == last {
            return true;
        }
        match self.main.apply_settings(self.window, &edited, &last) {
            Err(message) => {
                self.refuse(
                    dialog,
                    SettingsPage::Keyboard,
                    ControlId::KeyboardShortcut,
                    &message,
                );
                false
            }
            Ok(applied) => {
                let saved = self.save_include(dialog, &edited, &last);
                let mut values = applied.values;
                if saved {
                    values.include.clone_from(&edited.include);
                }
                let refill = SettingsValues {
                    include: edited.include.clone(),
                    ..values.clone()
                } != edited;
                {
                    let mut draft = self.draft.borrow_mut();
                    draft.applied(values);
                    // Settings that were not saved stay edited.
                    draft.edited.include.clone_from(&edited.include);
                }
                if refill {
                    for page in SettingsPage::ALL {
                        self.fill(page);
                    }
                }
                let title = self.strings.text("window.title");
                for failure in &applied.failures {
                    win::dialog::error_box(Some(dialog), failure, title);
                }
                if edited.choices.show_icons != last.choices.show_icons {
                    // The page list's rows and the group captions follow.
                    if let Some(list) = ListBox::of_dialog(dialog, ControlId::SettingsPages.id()) {
                        list.fit_glyph_rows();
                    }
                    dialog.draw_once_after(|| {});
                }
                self.show_preview();
                self.update_apply(dialog);
                saved
            }
        }
    }

    /// Writes the program that the person chooses into the open command
    /// box `edit`, as `"<program>" "%1"`.
    fn browse(&self, dialog: Hwnd, edit: ControlId) {
        let title = self.strings.text("settings_context_menu.browse_title");
        let programs = self.strings.text("settings_context_menu.programs");
        if let Some(program) = win::shell::pick_program(dialog, title, programs)
            && let Some(edit) = self.control(SettingsPage::ContextMenu, edit)
        {
            // EN_CHANGE then puts it in the draft.
            edit.set_text(&browse_command(&program));
        }
    }

    /// The Use Total Commander link's answer, which the main window holds.
    /// The balloon says when both boxes already hold the command, because
    /// the link changes nothing then.
    fn total_commander_found(&self, dialog: Hwnd) {
        let Some(found) = self.main.take_total_commander() else {
            return;
        };
        let link = self.control(
            SettingsPage::ContextMenu,
            ControlId::ContextMenuTotalCommander,
        );
        let note = match found {
            Ok(Some(path)) => {
                let command = total_commander_command(&path);
                let set = {
                    let choices = &mut self.draft.borrow_mut().edited.choices;
                    let set = choices.open_folders == command && choices.open_files == command;
                    choices.open_folders.clone_from(&command);
                    choices.open_files = command;
                    set
                };
                if !set {
                    self.fill(SettingsPage::ContextMenu);
                    self.update_apply(dialog);
                    return;
                }
                self.strings
                    .text("settings_context_menu.total_commander_set")
                    .to_owned()
            }
            Ok(None) => self
                .strings
                .text("settings_context_menu.no_total_commander")
                .to_owned(),
            Err(failure) => failure_text(&self.strings, &failure),
        };
        if let (Some(link), Some(balloon)) = (link, self.balloon.get()) {
            balloon.show(link, &note);
        }
    }

    /// Opens the command shortcut dialog for the selected command, to add a
    /// shortcut, or to change the selected one when `edit`.
    fn edit_shortcut(&self, dialog: Hwnd, edit: bool) {
        let Some(command) = self.selected_command() else {
            return;
        };
        let replaced = if edit {
            let Some(key) = self.selected_key() else {
                return;
            };
            Some(key)
        } else {
            None
        };
        let shortcuts = self.draft.borrow().edited.choices.shortcuts.clone();
        let state = CommandShortcut::new(&self.strings, command, replaced, shortcuts);
        let captions = DialogCaptions::command_shortcut(
            &self.strings,
            &command_place(&self.strings, command),
            edit,
        );
        let ended = dialog::run_modal(dialog, DialogId::CommandShortcut, &captions, &state);
        let Some(keystroke) = state
            .chosen()
            .filter(|_| matches!(ended, Ok(DialogEnd::Ok)))
        else {
            return;
        };
        let added = self
            .draft
            .borrow_mut()
            .edited
            .choices
            .shortcuts
            .add(command, keystroke, replaced);
        if added.is_ok() {
            self.fill_commands(Some(command));
            let position = {
                let draft = self.draft.borrow();
                draft
                    .edited
                    .choices
                    .shortcuts
                    .of(command)
                    .iter()
                    .position(|k| *k == keystroke)
            };
            self.fill_keys(position.unwrap_or(0));
        }
    }

    fn remove_shortcut(&self) {
        let (Some(command), Some(key)) = (self.selected_command(), self.selected_key()) else {
            return;
        };
        self.draft
            .borrow_mut()
            .edited
            .choices
            .shortcuts
            .remove(command, key);
        self.fill_commands(Some(command));
    }

    /// A control of a page holds another value.
    fn edited(&self, dialog: Hwnd, id: u16) {
        if is(id, ControlId::SettingsPages) {
            let selected = ListBox::of_dialog(dialog, id)
                .and_then(ListBox::selected)
                .and_then(|index| self.listed.borrow().get(index).copied());
            if selected.is_some() {
                self.show_page(dialog, selected);
            }
        } else if is(id, ControlId::SettingsSearch) {
            self.search_changed(dialog);
        } else if is(id, ControlId::ContextMenuItems) {
            self.fill(SettingsPage::ContextMenu);
        } else if let Some((control, _, _, command)) = OPEN_COMMANDS
            .into_iter()
            .find(|(control, ..)| is(id, *control))
        {
            let text = self
                .control(SettingsPage::ContextMenu, control)
                .map(Hwnd::text)
                .unwrap_or_default();
            *command(&mut self.draft.borrow_mut().edited) = text;
        } else if is(id, ControlId::GeneralLanguage) {
            // Windows default first, then the shipped languages.
            if let Some(index) = self.language_list().and_then(DropDownList::selected) {
                self.draft.borrow_mut().edited.choices.language =
                    LanguageChoice::at(index.checked_sub(1));
            }
        } else if is(id, ControlId::KeyboardShortcut) {
            self.shortcut_changed();
        } else if is(id, ControlId::KeyboardFilter) {
            self.fill_commands(self.selected_command());
        } else if is(id, ControlId::KeyboardCommands) {
            self.fill_keys(0);
        } else if is(id, ControlId::KeyboardKeys) {
            self.update_key_buttons();
        } else if is(id, ControlId::SpeedPreset) {
            let chosen = self
                .speed_list(ControlId::SpeedPreset)
                .and_then(DropDownList::selected)
                .and_then(|place| SpeedPreset::ALL.get(place).copied());
            if let Some(preset) = chosen {
                self.draft.borrow_mut().edited.choices.speed.preset = preset;
                self.fill(SettingsPage::FileOperations);
            }
        } else if let Some((control, number)) = SPEED_NUMBERS.into_iter().find(|(c, _)| is(id, *c))
        {
            let text = self
                .control(SettingsPage::FileOperations, control)
                .map(Hwnd::text)
                .unwrap_or_default();
            // A number out of its range is refused when OK or Apply is
            // pressed.
            self.change_speed(|settings| {
                let _ = number.set_text(settings, &text);
            });
        } else if let Some((control, _, engine)) =
            SPEED_ENGINES.into_iter().find(|(c, ..)| is(id, *c))
        {
            let chosen = self
                .speed_list(control)
                .and_then(DropDownList::selected)
                .and_then(|place| ENGINES.get(place).copied());
            if let Some(chosen) = chosen {
                self.change_speed(|settings| *engine(settings) = chosen);
            }
        } else if let Some((control, block)) = SPEED_BLOCKS.into_iter().find(|(c, _)| is(id, *c)) {
            let chosen = self
                .speed_list(control)
                .and_then(DropDownList::selected)
                .and_then(|place| ring_blocks().nth(place));
            if let Some(chosen) = chosen {
                self.change_speed(|settings| *block(settings) = chosen);
            }
        } else if is(id, ControlId::IncludeNames) {
            let text = self
                .control(SettingsPage::Include, ControlId::IncludeNames)
                .map(Hwnd::text)
                .unwrap_or_default();
            if let Some(include) = &mut self.draft.borrow_mut().edited.include {
                include.excluded_names = text;
            }
            self.include_changed(dialog);
        }
    }

    /// A click on a button or a check box of a page; answers whether it was
    /// one of the pages'.
    fn clicked(&self, dialog: Hwnd, id: u16) -> bool {
        if let Some((page, control, value)) = CHECK_BOXES.into_iter().find(|(_, c, _)| is(id, *c)) {
            let checked = self
                .check_box(page, control)
                .is_some_and(CheckBox::is_checked);
            *value(&mut self.draft.borrow_mut().edited) = checked;
            self.show_closed_note();
        } else if let Some((_, shown)) = VISIBILITY_BUTTONS.into_iter().find(|(c, _)| is(id, *c)) {
            if let Some(item) = self.selected_context_item() {
                *self
                    .draft
                    .borrow_mut()
                    .edited
                    .choices
                    .context_menu
                    .visibility_mut(item) = shown;
            }
        } else if let Some((edit, ..)) = OPEN_COMMANDS.into_iter().find(|(_, b, ..)| is(id, *b)) {
            self.browse(dialog, edit);
        } else if let Some((control, value)) =
            INCLUDE_CHECK_BOXES.into_iter().find(|(c, _)| is(id, *c))
        {
            let checked = self
                .check_box(SettingsPage::Include, control)
                .is_some_and(CheckBox::is_checked);
            if let Some(include) = &mut self.draft.borrow_mut().edited.include {
                *value(include) = checked;
            }
            self.include_changed(dialog);
        } else if let Some((page, _)) = RESTORE.into_iter().find(|(_, c)| is(id, *c)) {
            self.draft.borrow_mut().edited.restore_defaults(page);
            self.fill(page);
            if page == SettingsPage::Include {
                self.include_changed(dialog);
            }
        } else if is(id, ControlId::SpeedPerProcessor) {
            let per_processor = self
                .check_box(SettingsPage::FileOperations, ControlId::SpeedPerProcessor)
                .is_some_and(CheckBox::is_checked);
            self.change_speed(|settings| {
                if per_processor {
                    settings.delete_workers = DeleteWorkers::PerProcessor;
                } else {
                    // The count the box showed, at most the range's top on
                    // a computer with more logical processors.
                    let shown = SpeedNumber::DeleteWorkers.get(settings);
                    let _ = SpeedNumber::DeleteWorkers.set(settings, shown.min(64));
                }
            });
            self.fill(SettingsPage::FileOperations);
        } else if is(id, ControlId::IncludeModify) {
            self.modify_folders(dialog);
        } else if is(id, ControlId::KeyboardWindowsKey) {
            self.windows_key_changed();
        } else if is(id, ControlId::KeyboardClear) {
            // As Backspace in the box does.
            if let Some(hotkey) = self.hotkey() {
                hotkey.set(None);
                hotkey.hwnd().focus();
            }
            self.shortcut_changed();
        } else if is(id, ControlId::KeyboardAdd) {
            self.edit_shortcut(dialog, false);
        } else if is(id, ControlId::KeyboardEdit) {
            self.edit_shortcut(dialog, true);
        } else if is(id, ControlId::KeyboardRemove) {
            self.remove_shortcut();
        } else {
            return false;
        }
        true
    }
}

impl DialogProc for SettingsDialog<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        self.fit_page_list(dialog);
        let area =
            dialog::item(dialog, ControlId::SettingsPageArea.id()).map(|a| a.rect_in(dialog));
        let pages_list = dialog::item(dialog, ControlId::SettingsPages.id());
        let mut pages = Vec::new();
        for page in SettingsPage::ALL {
            let id = page.dialog();
            let Ok(hwnd) = dialog::create_page(dialog, id, &DialogCaptions::new(id, &self.strings))
            else {
                dialog::end(dialog, DialogEnd::Cancel);
                return Opened {
                    focus: DialogItem::Cancel.id(),
                    fonts,
                };
            };
            // After the page list in the Tab order, before OK.
            if let (Some(area), Some(list)) = (area, pages_list) {
                hwnd.place_after(list, area);
            }
            if let Some(heading) = dialog::item(hwnd, heading(page).id())
                && let Ok(font) = Font::of(heading).and_then(|font| font.semibold())
            {
                heading.set_font(&font);
                fonts.push(font);
            }
            for (group, glyph) in SETTINGS_GROUPS {
                if let Some(group) = dialog::item(hwnd, group.id()) {
                    show_group_glyph(group, glyph);
                }
            }
            pages.push(hwnd);
        }
        self.pages.replace(pages);
        if let Some(list) = ListBox::of_dialog(dialog, ControlId::SettingsPages.id()) {
            list.fit_glyph_rows();
        }
        if let Some(label) = self
            .page(SettingsPage::General)
            .and_then(|page| dialog::item(page, ControlId::GeneralLanguageLabel.id()))
            && let Ok(normal) = Font::of(label)
            && let Ok(bold) = normal.bold()
        {
            let _ = self.highlight.set((normal, bold));
        }

        let text = |key: &'static str| self.strings.text(key);
        if let Some(list) = self.language_list() {
            // As the Language menu lists them (DESIGN.md: Language).
            let names = LANGUAGES
                .iter()
                .map(|language| without_access_key(&language.label(&self.strings)));
            let items: Vec<String> =
                std::iter::once(text("settings_general.windows_default").to_owned())
                    .chain(names)
                    .collect();
            list.set_items(&items, 0);
        }
        if let Some(list) = self.speed_list(ControlId::SpeedPreset) {
            let names: Vec<&str> = SpeedPreset::ALL
                .iter()
                .map(|preset| text(preset.texts().0))
                .collect();
            list.set_items(&names, 0);
        }
        for (control, windows, _) in SPEED_ENGINES {
            if let Some(list) = self.speed_list(control) {
                let names: Vec<&str> = ENGINES
                    .iter()
                    .map(|engine| match engine {
                        Engine::Windows => text(windows),
                        Engine::Ring => text("settings_file_operations.engine_ring"),
                    })
                    .collect();
                list.set_items(&names, 0);
            }
        }
        let blocks: Vec<String> = ring_blocks()
            .map(|block| {
                let kib = block.bytes() / 1024;
                if kib < 1024 {
                    self.strings.format("size.kb", &[&kib.to_string()])
                } else {
                    self.strings.format("size.mb", &[&(kib / 1024).to_string()])
                }
            })
            .collect();
        for (control, _) in SPEED_BLOCKS {
            if let Some(list) = self.speed_list(control) {
                list.set_items(&blocks, 0);
            }
        }
        if let Some(list) = self
            .page(SettingsPage::ContextMenu)
            .and_then(|page| ListBox::of_dialog(page, ControlId::ContextMenuItems.id()))
        {
            let names: Vec<String> = ContextItem::all()
                .into_iter()
                .map(|item| without_access_key(text(item.label())))
                .collect();
            list.set_items(&names, 0);
        }
        if let Some(page) = self.page(SettingsPage::ContextMenu) {
            for (control, _, cue, _) in OPEN_COMMANDS {
                if let Some(edit) = Edit::of_dialog(page, control.id()) {
                    edit.set_cue(text(cue));
                }
            }
            // The link ends the hint's line, at the Browse… buttons' right
            // edge.
            if let (Some(link), Some(hint)) = (
                dialog::item(page, ControlId::ContextMenuTotalCommander.id()),
                dialog::item(page, ControlId::ContextMenuCommandHint.id()),
            ) {
                dialog::fit_link(link, hint);
            }
        }
        if let Some(list) = self.command_list() {
            list.set_columns(&[
                (text("settings_keyboard.command_column"), 62, false),
                (text("settings_keyboard.keys_column"), 38, false),
            ]);
        }
        if let Some(list) = self
            .page(SettingsPage::Index)
            .and_then(|page| ReportList::of_dialog(page, ControlId::IndexDrives.id()))
        {
            list.set_columns(&[
                (text("settings_index.drive_column"), 38, false),
                (text("settings_index.state_column"), 42, false),
                (text("settings_index.entries_column"), 20, true),
            ]);
        }
        if let Some(list) = self.include_folders() {
            list.set_columns(&[
                (text("settings_include.folder_column"), 30, false),
                (text("settings_include.left_out_column"), 18, false),
                (text("settings_include.folders_column"), 52, false),
            ]);
            list.show_info_tips();
        }
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        // Reset all and every page's Reset say which settings they restore,
        // the Use Total Commander link says what it does, and the Service
        // group's fix buttons say what their fix does. Each page has one
        // tooltip, and Windows destroys the tooltips with the dialog.
        if let Some(button) = dialog::item(dialog, ControlId::SettingsResetAll.id())
            && let Ok(tip) = Tooltip::create(dialog, &[button])
        {
            tip.set(button, text("settings.reset_all_tooltip"));
        }
        for page in SettingsPage::ALL {
            let Some(hwnd) = self.page(page) else {
                continue;
            };
            let reset = RESTORE
                .into_iter()
                .find(|(restored, _)| *restored == page)
                .map(|(_, reset)| (reset, text("settings.reset_tooltip")));
            let others: &[(ControlId, &str)] = match page {
                SettingsPage::ContextMenu => &[(
                    ControlId::ContextMenuTotalCommander,
                    text("settings_context_menu.total_commander_tooltip"),
                )],
                // `refresh_index` gives these their tips.
                SettingsPage::Index => &[(ControlId::IndexUseLocal, ""), (ControlId::IndexFix, "")],
                _ => &[],
            };
            let tipped: Vec<(Hwnd, &str)> = reset
                .iter()
                .chain(others)
                .filter_map(|&(control, tip)| Some((dialog::item(hwnd, control.id())?, tip)))
                .collect();
            if tipped.is_empty() {
                continue;
            }
            let controls: Vec<Hwnd> = tipped.iter().map(|&(control, _)| control).collect();
            let Ok(tips) = Tooltip::create(hwnd, &controls) else {
                continue;
            };
            for (control, tip) in tipped {
                tips.set(control, tip);
            }
            if page == SettingsPage::Index {
                let _ = self.index_tips.set(tips);
            }
        }
        for page in SettingsPage::ALL {
            self.fill(page);
        }
        if let Some(search) = Edit::of_dialog(dialog, ControlId::SettingsSearch.id()) {
            search.set_cue(text("settings.search_cue"));
        }
        let status = self.add_status_bar(dialog, &mut fonts);
        self.main.settings_opened(dialog, status);
        self.show_page(dialog, Some(SettingsPage::General));
        self.update_apply(dialog);
        Opened {
            focus: ControlId::SettingsPages.id(),
            fonts,
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        let handled = match message {
            DialogMessage::Edited { .. } if self.filling.get() => return true,
            DialogMessage::Edited { id } => {
                self.edited(dialog, id);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                if self.apply(dialog) {
                    dialog::end(dialog, DialogEnd::Ok);
                }
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::SettingsResetAll) => {
                self.draft.borrow_mut().edited.restore_all_defaults();
                for page in SettingsPage::ALL {
                    self.fill(page);
                }
                self.include_changed(dialog);
                true
            }
            DialogMessage::Command { id } if is(id, ControlId::SettingsApply) => {
                self.apply(dialog);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::StatusCancel) => {
                self.main.cancel_job(self.window);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::IndexUseLocal) => {
                self.main.run_shown_fix(self.window, Model::use_local_fix);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::IndexFix) => {
                self.main.run_shown_fix(self.window, Model::service_fix);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::IndexReport) => {
                self.show_report(dialog);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::SettingsClearSearch) => {
                // The box's change shows every page again.
                if let Some(search) = dialog::item(dialog, ControlId::SettingsSearch.id()) {
                    search.set_text("");
                    search.focus();
                }
                return true;
            }
            DialogMessage::Command { id } => self.clicked(dialog, id),
            // A row's folders are the ones Indexed folders lists in full.
            DialogMessage::DoubleClicked { id } if is(id, ControlId::IncludeFolders) => {
                let on_row = self
                    .include_folders()
                    .and_then(ReportList::selected)
                    .is_some();
                if on_row {
                    self.modify_folders(dialog);
                }
                on_row
            }
            DialogMessage::Timer { id: INDEX_TIMER } => {
                self.refresh_index(dialog);
                return true;
            }
            DialogMessage::Timer { id: PREVIEW_TIMER } => {
                self.count_preview(dialog);
                return true;
            }
            DialogMessage::Reply => {
                self.total_commander_found(dialog);
                self.preview_counted();
                // A fix moved on, which the fix button follows.
                self.refresh_index(dialog);
                return true;
            }
            DialogMessage::LinkActivated { id } if is(id, ControlId::ContextMenuTotalCommander) => {
                self.main.find_total_commander();
                true
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Tree { .. }
            | DialogMessage::Other => return false,
        };
        if handled {
            if let Some(balloon) = self.balloon.get() {
                balloon.hide();
            }
            self.update_apply(dialog);
        }
        handled
    }

    /// Places the status bar, whose Cancel is as wide as OK and Apply.
    fn buttons_sized(&self, dialog: Hwnd, width: Px) {
        let (Some(status), Some((widths, font_height))) =
            (self.status.get(), self.status_widths.get())
        else {
            return;
        };
        let dpi = dialog.dpi();
        let height = status.fit(StatusLayout::min_height(dpi, *font_height));
        status.place(&StatusLayout::compute(
            widths,
            width,
            dpi,
            *font_height,
            height,
            None,
        ));
    }

    /// Every caption the Service group's fix buttons can show, and OK and
    /// Apply with the shield, which they show while Include has changes.
    fn later_captions(&self) -> Vec<(String, bool)> {
        Fix::label_groups()
            .into_iter()
            .flatten()
            .chain([("button.ok", true), ("settings.apply", true)])
            .map(|(key, shielded)| (self.strings.text(key).to_owned(), shielded))
            .collect()
    }

    fn empty_text(&self, id: u16) -> Option<String> {
        let index = self.index.borrow();
        let text = &index.as_ref()?.no_drives;
        (is(id, ControlId::IndexDrives) && !text.is_empty()).then(|| text.clone())
    }

    fn info_tip(&self, id: u16, row: usize) -> Option<String> {
        let rows = self.include_rows.borrow();
        let row = rows
            .get(row)
            .filter(|_| is(id, ControlId::IncludeFolders))?;
        row.tip()
    }
}

/// The command shortcut dialog: one shortcut of one command, which
/// Settings > Keyboard's Add… and Edit… open (DESIGN.md: The Keyboard page).
struct CommandShortcut<'a> {
    strings: &'a Strings,
    command: CommandId,
    /// The shortcut Edit… changes; `None` for Add….
    replaced: Option<Keystroke>,
    /// The page's shortcuts, which the new one must not repeat.
    shortcuts: CommandShortcuts,
    balloon: OnceCell<Balloon>,
    chosen: Cell<Option<Keystroke>>,
}

impl<'a> CommandShortcut<'a> {
    fn new(
        strings: &'a Strings,
        command: CommandId,
        replaced: Option<Keystroke>,
        shortcuts: CommandShortcuts,
    ) -> CommandShortcut<'a> {
        CommandShortcut {
            strings,
            command,
            replaced,
            shortcuts,
            balloon: OnceCell::new(),
            chosen: Cell::new(None),
        }
    }

    /// The shortcut OK chose.
    fn chosen(&self) -> Option<Keystroke> {
        self.chosen.get()
    }

    fn hotkey(dialog: Hwnd) -> Option<HotkeyControl> {
        HotkeyControl::of_dialog(dialog, ControlId::CommandShortcutKey.id())
    }

    /// OK stays disabled while no key is pressed.
    fn update_ok(dialog: Hwnd) {
        let empty = Self::hotkey(dialog).is_none_or(|h| h.value() == HotkeyValue::Empty);
        if let Some(ok) = dialog::item(dialog, DialogItem::Ok.id()) {
            ok.set_enabled(!empty);
        }
    }

    fn refuse(&self, dialog: Hwnd, text: &str) {
        if let Some(hotkey) = Self::hotkey(dialog) {
            hotkey.hwnd().focus();
            if let Some(balloon) = self.balloon.get() {
                balloon.show(hotkey.hwnd(), text);
            }
        }
    }
}

impl DialogProc for CommandShortcut<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        if let Some(hotkey) = Self::hotkey(dialog) {
            // Any key: the command shortcut rules refuse a key for typing
            // with their message, which the control's own rules would not
            // give.
            hotkey.require_two_modifiers(false);
            hotkey.set(self.replaced);
        }
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        Self::update_ok(dialog);
        Opened {
            focus: ControlId::CommandShortcutKey.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { .. } => {
                if let Some(balloon) = self.balloon.get() {
                    balloon.hide();
                }
                Self::update_ok(dialog);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                match Self::hotkey(dialog).map_or(HotkeyValue::Empty, HotkeyControl::value) {
                    // OK is disabled then.
                    HotkeyValue::Empty => {}
                    HotkeyValue::Unsupported => {
                        self.refuse(dialog, self.strings.text("settings_keyboard.unsupported"));
                    }
                    HotkeyValue::Keys(keystroke) => {
                        match self.shortcuts.check(self.command, keystroke) {
                            Ok(()) => {
                                self.chosen.set(Some(keystroke));
                                dialog::end(dialog, DialogEnd::Ok);
                            }
                            Err(refusal) => {
                                self.refuse(dialog, &refusal.message(self.strings, keystroke));
                            }
                        }
                    }
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Other => false,
        }
    }
}
