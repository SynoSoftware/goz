//! The Settings dialog and its command shortcut dialog (DESIGN.md:
//! Settings dialog). The dialog edits its own copy of the
//! settings, a [`SettingsDraft`], and OK or Apply gives it to the main
//! window, so no model borrow is held while its modal loop runs (DESIGN.md:
//! window rule 1). Each page is a child dialog of its own template, whose
//! controls this dialog's procedure handles.

use super::dialogs::ServiceReport;
use super::include::{Elevated, NotSaved};
use super::indexed_folders::IndexedFolders;
use super::main_window::{MainWindow, PreparedShortcuts};
use super::status_bar::{Entry as StatusEntry, Slot as StatusSlot, StatusBar};
use crate::core::captions::{
    self, ButtonIcon, DialogCaptions, DialogItem, IndexCaptions, JobCaption, JobProgress,
    SETTINGS_GROUPS, failure_text,
};
use crate::core::commands::{CommandId, Key, Keystroke, kept_by_commander};
use crate::core::dates::DateFormat;
use crate::core::fetch::Purpose;
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::include::{AfterOk, SummaryRow};
use crate::core::layout::{Px, Rect, StatusLayout, StatusWidths};
use crate::core::model::Model;
use crate::core::open::browse_command;
use crate::core::results::Fix;
use crate::core::settings::{
    ContextItem, ItemVisibility, Mode, ServiceChanges, SettingsDraft, SettingsPage, SettingsValues,
};
use crate::core::shortcuts::{
    CommandRow, CommandShortcuts, ShowShortcut, command_place, command_rows,
};
use crate::core::speed::{self, ENGINES, Speed, SpeedNumber, SpeedPreset, ring_blocks};
use crate::core::text::{LANGUAGES, LanguageChoice, SizeFormat, Strings, without_access_key};
use crate::core::work::{self, Id, Interest, Waiting};
use crate::win::controls::{
    Balloon, Button, CheckBox, DropDownList, Edit, GlyphLabel, HotkeyControl, HotkeyValue,
    IconList, ListBox, ReportList, Tooltip, show_heading_icon,
};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::gdi::{Font, TextMeter};
use crate::win::shortcut::{self, Capture};
use crate::win::{self, Hwnd};
use gozler_core::include::IndexSettings;
use gozler_core::proto::CallerSees;
use gozler_fileops::{DeleteWorkers, Engine, RingBlock, SpeedSettings};
use std::cell::{Cell, OnceCell, RefCell};
use std::time::Instant;

/// The Index page's timer, which shows the service's latest state.
const INDEX_TIMER: usize = 1;
const INDEX_REFRESH_MS: u32 = 2_000;
/// The Include page's timer, which asks for the preview's count once the
/// person stops changing the settings for a moment.
const PREVIEW_TIMER: usize = 2;
const PREVIEW_DELAY_MS: u32 = 300;
const WAIT_TIMER: usize = 3;
const WAIT_POLL_MS: u32 = 250;

/// What the dialog does once its accepted changes are saved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterSave {
    /// Apply was pressed: the dialog stays open.
    Stay,
    /// OK was pressed: the dialog applies anything changed since, and
    /// closes.
    Close,
}

enum Awaited {
    Shortcut(Box<Applying>),
    SignIn {
        id: Id,
        on: bool,
        service: Option<ServiceChanges>,
        after: AfterSave,
    },
    /// Saves these service settings, after Apply or OK.
    Service(ServiceChanges, AfterSave),
    /// Asks the service to rebuild the index, after Rebuild.
    Rebuild,
}

struct Applying {
    registration: PreparedShortcuts,
    edited: SettingsValues,
    last: SettingsValues,
    after: AfterSave,
    accepted: Instant,
}

/// The page's heading, which shows its name in semibold.
fn heading(page: SettingsPage) -> ControlId {
    match page {
        SettingsPage::General => ControlId::GeneralHeading,
        SettingsPage::Search => ControlId::SearchHeading,
        SettingsPage::ContextMenu => ControlId::ContextMenuHeading,
        SettingsPage::Keyboard => ControlId::KeyboardHeading,
        SettingsPage::FileOperations => ControlId::FileOperationsSettingsHeading,
        SettingsPage::Performance => ControlId::SpeedHeading,
        SettingsPage::Index => ControlId::IndexHeading,
        SettingsPage::Include => ControlId::IncludeHeading,
        SettingsPage::Visibility => ControlId::VisibilityHeading,
    }
}

/// The value of [`SettingsValues`] that a control shows.
type Field<T> = fn(&mut SettingsValues) -> &mut T;

/// Each check box, its page, and the value it shows.
const CHECK_BOXES: [(SettingsPage, ControlId, Field<bool>); 10] = [
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
    (SettingsPage::General, ControlId::GeneralShowMemory, |v| {
        &mut v.choices.show_memory
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
        "settings_context_menu.cue_commander",
        |v| &mut v.choices.open_folders,
    ),
    (
        ControlId::ContextMenuOpenFiles,
        ControlId::ContextMenuOpenFilesBrowse,
        "settings_context_menu.cue_commander",
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

const SHOW_SHORTCUTS: [(Mode, ControlId, ControlId); 2] = [
    (
        Mode::Search,
        ControlId::KeyboardShortcut,
        ControlId::KeyboardClear,
    ),
    (
        Mode::Commander,
        ControlId::KeyboardCommanderShortcut,
        ControlId::KeyboardCommanderClear,
    ),
];

fn shortcut_control(mode: Mode) -> ControlId {
    match mode {
        Mode::Search => ControlId::KeyboardShortcut,
        Mode::Commander => ControlId::KeyboardCommanderShortcut,
    }
}

/// The Include setting that a check box shows.
type IncludeField = fn(&mut IndexSettings) -> &mut bool;

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
const RESTORE: [(SettingsPage, ControlId); 7] = [
    (SettingsPage::General, ControlId::GeneralRestore),
    (SettingsPage::Search, ControlId::SearchRestore),
    (SettingsPage::ContextMenu, ControlId::ContextMenuRestore),
    (SettingsPage::Keyboard, ControlId::KeyboardRestore),
    (
        SettingsPage::FileOperations,
        ControlId::FileOperationsRestore,
    ),
    (SettingsPage::Performance, ControlId::SpeedRestore),
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
    /// The pages in [`SettingsPage::ALL`]'s order, each once it first
    /// showed. A page is made then, not when the dialog opens, so the
    /// dialog shows without waiting for every page's controls; the draft
    /// holds the values of a page not made yet.
    pages: [Cell<Option<Hwnd>>; SettingsPage::ALL.len()],
    /// The pages' heading fonts, which live until the dialog ends, because
    /// `WM_SETFONT` does not take ownership.
    fonts: RefCell<Vec<Font>>,
    /// The one width of the push buttons, which a page made later gets.
    button_width: Cell<Option<Px>>,
    /// The pages the page list shows, which the search box keeps; the
    /// shown page is one of them, or none when the list is empty.
    listed: RefCell<Vec<SettingsPage>>,
    shown: Cell<Option<SettingsPage>>,
    /// The commands the Keyboard page lists, after its filter.
    commands: RefCell<Vec<CommandRow>>,
    /// What the Index page shows, so a refresh sets only what changed.
    index: RefCell<Option<IndexCaptions>>,
    /// The rows that Folders Gozler indexes shows, whose tooltips list the
    /// full paths of the folders left out.
    include_rows: RefCell<Vec<SummaryRow>>,
    /// What the Visibility page's line says, so a refresh sets it only when
    /// it changed.
    seen: RefCell<String>,
    /// Whether the dialog is setting its controls, whose notifications then
    /// change nothing.
    filling: Cell<bool>,
    balloon: OnceCell<Balloon>,
    /// The balloon a change asked for, beside its control, which shows after
    /// the message that made the change hides the previous one.
    note: RefCell<Option<(Hwnd, String)>>,
    /// The keys pressed while the shortcut box has the focus, which reach
    /// the dialog instead of the box (DESIGN.md: The Keyboard page).
    capture: Cell<Option<(Mode, Capture)>>,
    /// Whether the person confirmed Reset all, which the main window carries
    /// out once the dialog has closed.
    resets: Cell<bool>,
    /// Accepted settings work keeps its captured edits and continuation while
    /// the dialog remains usable; its command buttons wait for acknowledgment.
    saving: RefCell<Option<Awaited>>,
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
    status_layout: Cell<Option<StatusLayout>>,
    apply_shield: Cell<Option<bool>>,
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
            pages: Default::default(),
            fonts: RefCell::new(Vec::new()),
            button_width: Cell::new(None),
            listed: RefCell::new(SettingsPage::ALL.to_vec()),
            shown: Cell::new(None),
            commands: RefCell::new(Vec::new()),
            index: RefCell::new(None),
            include_rows: RefCell::new(Vec::new()),
            seen: RefCell::new(String::new()),
            filling: Cell::new(false),
            balloon: OnceCell::new(),
            note: RefCell::new(None),
            capture: Cell::new(None),
            resets: Cell::new(false),
            saving: RefCell::new(None),
            index_tips: OnceCell::new(),
            highlight: OnceCell::new(),
            status: OnceCell::new(),
            status_widths: OnceCell::new(),
            status_layout: Cell::new(None),
            apply_shield: Cell::new(None),
        }
    }

    /// Whether the person confirmed Reset all before the dialog closed.
    pub(super) fn resets(&self) -> bool {
        self.resets.get()
    }

    /// Adds the status bar below OK, Cancel and Apply and makes the dialog
    /// that much taller; `buttons_sized` places its groups and controls.
    /// The bar's font is the dialog's, which `fonts` keeps.
    fn add_status_bar(&self, dialog: Hwnd, fonts: &mut Vec<Font>) -> Option<&StatusBar> {
        let (Ok(status), Ok(font)) = (StatusBar::for_dialog(dialog), Font::of(dialog)) else {
            return None;
        };
        for control in status.text_controls() {
            control.set_font(&font);
        }
        let (widest, cancel) = self.main.settings.settings_status_texts(self.main);
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
        self.status.get()
    }

    /// Widens the page list to show its longest page name in the dialog's
    /// language whole, after its icon, and the dialog with it, so the pages
    /// keep their width; while the list holds every page, before the pages
    /// are made.
    fn fit_page_list(&self, dialog: Hwnd) {
        let Some(list) = self.page_list(dialog) else {
            return;
        };
        let rect = list.hwnd().rect_in(dialog);
        let frame = rect.width - list.hwnd().client_size().width;
        let extra = list.row_size().width + frame - rect.width;
        if extra > Px(0) {
            dialog::widen(dialog, rect.right(), extra);
        }
        list.fill_column();
    }

    fn page_list(&self, dialog: Hwnd) -> Option<IconList> {
        IconList::of_dialog(dialog, ControlId::SettingsPages.id())
    }

    fn page(&self, page: SettingsPage) -> Option<Hwnd> {
        let index = SettingsPage::ALL.iter().position(|p| *p == page)?;
        self.pages[index].get()
    }

    fn control(&self, page: SettingsPage, control: ControlId) -> Option<Hwnd> {
        dialog::item(self.page(page)?, control.id())
    }

    fn check_box(&self, page: SettingsPage, control: ControlId) -> Option<CheckBox> {
        CheckBox::of_dialog(self.page(page)?, control.id())
    }

    fn shortcut_box(&self, mode: Mode) -> Option<Edit> {
        Edit::of_dialog(
            self.page(SettingsPage::Keyboard)?,
            shortcut_control(mode).id(),
        )
    }

    fn language_list(&self) -> Option<DropDownList> {
        DropDownList::of_dialog(
            self.page(SettingsPage::General)?,
            ControlId::GeneralLanguage.id(),
        )
    }

    fn size_list(&self) -> Option<DropDownList> {
        DropDownList::of_dialog(
            self.page(SettingsPage::General)?,
            ControlId::GeneralSizes.id(),
        )
    }

    fn date_list(&self) -> Option<DropDownList> {
        DropDownList::of_dialog(
            self.page(SettingsPage::General)?,
            ControlId::GeneralDates.id(),
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

    /// Sets every control of `page` from the edited values; a page not made
    /// yet is set when it is made.
    fn fill(&self, page: SettingsPage) {
        if self.page(page).is_none() {
            return;
        }
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
                if page == SettingsPage::General {
                    for (control, checked) in [
                        (
                            ControlId::GeneralSelectFiles,
                            !values.choices.select_folders,
                        ),
                        (
                            ControlId::GeneralSelectFolders,
                            values.choices.select_folders,
                        ),
                    ] {
                        if let Some(button) = self.check_box(page, control) {
                            button.set_checked(checked);
                        }
                    }
                    if let Some(list) = self.language_list() {
                        // A saved tag that no longer ships shows as Windows
                        // default; it stays saved unless the person chooses.
                        let place = values.choices.language.shipped();
                        list.select(place.map_or(0, |index| index + 1));
                    }
                    let format = values.choices.size_format;
                    let place = SizeFormat::ALL.iter().position(|each| *each == format);
                    if let (Some(list), Some(place)) = (self.size_list(), place) {
                        list.select(place);
                    }
                    let format = values.choices.date_format;
                    let place = DateFormat::ALL.iter().position(|each| *each == format);
                    if let (Some(list), Some(place)) = (self.date_list(), place) {
                        list.select(place);
                    }
                }
            }
            SettingsPage::ContextMenu => self.fill_context_menu(&mut values),
            SettingsPage::Keyboard => {
                self.show_shortcut();
                self.fill_commands(self.selected_command());
            }
            SettingsPage::FileOperations => {
                if let Some(list) = self.file_operations_list(ControlId::SpeedTransfers) {
                    list.select((values.choices.transfers_per_device.get() - 1) as usize);
                }
                if let Some(list) = self.file_operations_list(ControlId::FileOperationsPolicy) {
                    let selected = gozler_fileops::Policy::ALL
                        .iter()
                        .position(|policy| *policy == values.choices.copy_policy)
                        .unwrap_or(0);
                    list.select(selected);
                }
            }
            SettingsPage::Performance => self.fill_speed(&values.choices.speed),
            SettingsPage::Index => self.fill_dates_created(values.include.as_ref()),
            SettingsPage::Include => self.fill_include(values.include.as_mut()),
            SettingsPage::Visibility => self.fill_visibility(None),
        });
    }

    /// Sets the Performance page's controls from `speed`.
    fn fill_speed(&self, speed: &Speed) {
        self.draft.borrow_mut().speed_shown();
        self.show_preset(speed.preset);
        let page = SettingsPage::Performance;
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
        if let Some(label) = self.control(SettingsPage::Performance, ControlId::SpeedDescription) {
            label.set_text(self.strings.text(preset.texts().1));
        }
    }

    fn file_operations_list(&self, control: ControlId) -> Option<DropDownList> {
        DropDownList::of_dialog(self.page(SettingsPage::FileOperations)?, control.id())
    }

    fn speed_list(&self, control: ControlId) -> Option<DropDownList> {
        DropDownList::of_dialog(self.page(SettingsPage::Performance)?, control.id())
    }

    /// Changes one of the Performance page's values, which makes its
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
        let Some((number, range)) = self.draft.borrow().refused() else {
            return Ok(());
        };
        let control = SPEED_NUMBERS
            .into_iter()
            .find_map(|(control, each)| (each == number).then_some(control))
            .expect("every speed number has its box");
        let message = self.strings.format(
            "settings_file_operations.out_of_range",
            &[&range.min.to_string(), &range.max.to_string()],
        );
        Err((control, message))
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
            group.set_text(&without_access_key(self.strings.text(item.label())));
        }
        let visibility = values.choices.context_menu.visibility(item);
        for (control, shown) in VISIBILITY_BUTTONS {
            if let Some(button) = CheckBox::of_dialog(page, control.id()) {
                button.set_checked(shown == visibility);
            }
        }
    }

    /// Sets Dates created from `include`; without it, the service gave no
    /// settings, so it is disabled.
    fn fill_dates_created(&self, include: Option<&IndexSettings>) {
        if let Some(control) = self.control(SettingsPage::Index, ControlId::IndexDatesCreated) {
            control.set_enabled(include.is_some());
        }
        if let Some(check_box) = self.check_box(SettingsPage::Index, ControlId::IndexDatesCreated) {
            check_box.set_checked(include.is_some_and(|i| i.dates_created));
        }
    }

    /// Sets the Include page's controls from `include`; without it, the
    /// service gave no settings, so they are disabled.
    fn fill_include(&self, include: Option<&mut IndexSettings>) {
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
        self.refresh_include_rows(include);
    }

    fn refresh_include_rows(&self, include: &IndexSettings) {
        self.main.settings.take_include_summary_changed();
        let Some(list) = self.include_folders() else {
            return;
        };
        let drives = self.main.settings.include_drives(self.main, include);
        let rows = self
            .main
            .settings
            .include_summary(self.main, &self.strings, &drives, include);
        let cells: Vec<[String; 3]> = rows.iter().map(|row| row.cells.clone()).collect();
        let shown = self.include_rows.replace(rows);
        let shown: Vec<[String; 3]> = shown.into_iter().map(|row| row.cells).collect();
        list.update_rows(&shown, &cells);
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
        self.main.settings.take_include_preview_changed();
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
            label.set_text(&self.main.settings.index_captions(self.main).service);
            return;
        };
        let now = self.main.settings.indexed_entries(self.main);
        let after = if edited == applied {
            AfterOk::Counted(now)
        } else if !self.counts_included() {
            AfterOk::NotCounted
        } else {
            match self.main.settings.included_count(self.main) {
                Some(Ok(entries)) => AfterOk::Counted(entries),
                Some(Err(failure)) => {
                    label.set_text(&failure_text(&self.strings, &failure));
                    return;
                }
                None => AfterOk::Counting,
            }
        };
        let volumes = self.main.settings.connected_volumes(self.main);
        let volumes: Vec<&str> = volumes.iter().map(String::as_str).collect();
        let adds = edited.adds_to(&applied, &volumes);
        let text = self
            .main
            .settings
            .include_preview(self.main, &self.strings, now, after, adds);
        label.set_text(&text);
    }

    /// The Include settings changed: the preview asks the service for a new
    /// count once the person stops changing them for a moment.
    fn include_changed(&self, dialog: Hwnd) {
        self.main
            .settings
            .forget_included_count(self.main, self.window);
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
            && self.counts_included()
        {
            self.main.settings.count_included(self.main, edited);
        }
    }

    /// Whether to ask the service to count the entries that edited Include
    /// settings keep: not for a filtered user, whom it refuses. Any other
    /// refusal or failure shows as the count's failure.
    fn counts_included(&self) -> bool {
        self.main.settings.caller_sees(self.main) != Some(CallerSees::Filtered)
    }

    /// Modify… opens Indexed folders, whose OK puts its choices on the page.
    fn modify_folders(&self, dialog: Hwnd) {
        let Some(include) = self.draft.borrow().edited.include.clone() else {
            return;
        };
        let drives = self.main.settings.include_drives(self.main, &include);
        let state = IndexedFolders::new(self.main, &self.strings, &drives, include);
        let captions = DialogCaptions::new(DialogId::IndexedFolders, &self.strings);
        let ended = dialog::run_modal(dialog, DialogId::IndexedFolders, &captions, &state);
        if matches!(ended, Ok(DialogEnd::Ok)) {
            self.draft.borrow_mut().edited.include = Some(state.into_settings());
            self.fill(SettingsPage::Include);
            self.include_changed(dialog);
        }
    }

    /// Saves the captured service settings; later edits remain in the
    /// draft.
    fn save_service(&self, service: Option<ServiceChanges>, after: AfterSave) -> bool {
        let Some(service) = service else {
            return false;
        };
        self.saving
            .replace(Some(Awaited::Service(service.clone(), after)));
        self.main
            .settings
            .run_as_administrator(self.main, self.window, Elevated::Save(service));
        true
    }

    fn visibility_list(&self) -> Option<ReportList> {
        ReportList::of_dialog(
            self.page(SettingsPage::Visibility)?,
            ControlId::VisibilityList.id(),
        )
    }

    /// Sets the Visibility page's controls from the edited list, with the
    /// row at `selected` selected. While the list could not be read, they
    /// are disabled and the list says why.
    fn fill_visibility(&self, selected: Option<usize>) {
        let (sids, everyone) = match &self.draft.borrow().edited.see_every_file {
            Ok(list) => (Some(list.lines().to_vec()), list.everyone()),
            Err(_) => (None, false),
        };
        for control in [
            ControlId::VisibilityEveryone,
            ControlId::VisibilityList,
            ControlId::VisibilityAdd,
        ] {
            if let Some(control) = self.control(SettingsPage::Visibility, control) {
                control.set_enabled(sids.is_some());
            }
        }
        if let Some(check) = self.check_box(SettingsPage::Visibility, ControlId::VisibilityEveryone)
        {
            self.filling(|| check.set_checked(everyone));
        }
        let sids = sids.unwrap_or_default();
        let rows: Vec<[String; 1]> = (self.main.settings)
            .account_rows(self.main, &self.strings, &sids)
            .into_iter()
            .map(|row| [row])
            .collect();
        if let Some(list) = self.visibility_list() {
            self.filling(|| list.set_rows(&rows, selected));
        }
        self.update_visibility_buttons();
    }

    /// Remove needs a selected row of a list that was read.
    fn update_visibility_buttons(&self) {
        let selected = self
            .visibility_list()
            .and_then(ReportList::selected)
            .is_some();
        let readable = self.draft.borrow().edited.see_every_file.is_ok();
        if let Some(remove) = self.control(SettingsPage::Visibility, ControlId::VisibilityRemove) {
            let enabled = selected && readable;
            // A disabled button keeps the focus but takes no key. Add… is
            // what the person does next once the last entry is removed.
            if !enabled
                && Hwnd::focused() == Some(remove)
                && let Some(add) = self.control(SettingsPage::Visibility, ControlId::VisibilityAdd)
            {
                add.focus();
            }
            remove.set_enabled(enabled);
        }
    }

    /// Says what the person sees in searches, as the service last said, or
    /// the service's state while it has not answered or a later request
    /// failed (DESIGN.md: The Visibility page). Each reply runs it, because
    /// a failed request changes it too.
    fn show_seen(&self) {
        let Some(label) = self.control(SettingsPage::Visibility, ControlId::VisibilitySeen) else {
            return;
        };
        let key = match self.main.settings.caller_sees(self.main) {
            Some(CallerSees::EveryFile) => Some("settings_visibility.seen_every_file"),
            Some(CallerSees::Filtered) => Some("settings_visibility.seen_filtered"),
            Some(CallerSees::Unidentified) => Some("settings_visibility.seen_unidentified"),
            None => None,
        };
        let text = match key {
            Some(key) => self.strings.text(key).to_owned(),
            None => self.main.settings.index_captions(self.main).service,
        };
        if *self.seen.borrow() != text {
            label.set_text(&text);
            self.seen.replace(text);
        }
    }

    /// Add… opens Windows' Select Users or Groups dialog, and adds those
    /// the person chose that the list does not hold.
    fn add_accounts(&self, dialog: Hwnd) {
        let chosen = match win::accounts::pick_accounts(dialog) {
            Ok(chosen) => chosen,
            Err(error) => {
                let text = self.strings.format(
                    "settings_visibility.picker_failed",
                    &[&win::error_text(&error)],
                );
                win::dialog::error_box(Some(dialog), &text, self.strings.text("settings.title"));
                return;
            }
        };
        let Some(first) = chosen.first() else {
            return;
        };
        let selected = {
            let mut draft = self.draft.borrow_mut();
            let Ok(list) = &mut draft.edited.see_every_file else {
                return;
            };
            for sid in &chosen {
                list.add(sid.clone());
            }
            (list.lines().iter()).position(|sid| sid.eq_ignore_ascii_case(first))
        };
        self.fill_visibility(selected);
    }

    /// Remove takes the selected row off the list and selects the row that
    /// takes its place.
    fn remove_account(&self) {
        let Some(index) = self.visibility_list().and_then(ReportList::selected) else {
            return;
        };
        let left = {
            let mut draft = self.draft.borrow_mut();
            let Ok(list) = &mut draft.edited.see_every_file else {
                return;
            };
            list.remove(index);
            list.lines().len()
        };
        self.fill_visibility((left > 0).then(|| index.min(left - 1)));
    }

    /// Asks the service to rebuild the index, as administrator (DESIGN.md:
    /// The Drives and service page).
    fn rebuild(&self, dialog: Hwnd) {
        if self.saving.borrow().is_some() {
            return;
        }
        self.saving.replace(Some(Awaited::Rebuild));
        self.main
            .settings
            .run_as_administrator(self.main, self.window, Elevated::Rebuild);
        self.update_apply(dialog);
    }

    /// The answer of the copy that runs as administrator, which the main
    /// window holds: saved settings become applied, and after OK the dialog
    /// closes; refused ones stay edited, and a failure says why. The
    /// service shows a rebuild in its drives' state.
    fn save_answered(&self, dialog: Hwnd) {
        if matches!(self.saving.borrow().as_ref(), Some(Awaited::Shortcut(..))) {
            self.shortcut_answered(dialog);
            return;
        }
        if matches!(self.saving.borrow().as_ref(), Some(Awaited::SignIn { .. })) {
            self.sign_in_answered(dialog);
            return;
        }
        let Some(answer) = self.main.settings.take_elevated_answer() else {
            return;
        };
        let Some(awaited) = self.saving.take() else {
            return;
        };
        match (awaited, answer) {
            (Awaited::Service(service, after), Ok(())) => {
                self.draft.borrow_mut().service_saved(service);
                if after == AfterSave::Close && self.apply(dialog, AfterSave::Close) {
                    dialog::end(dialog, DialogEnd::Ok);
                    return;
                }
            }
            (Awaited::Rebuild, Ok(())) | (_, Err(NotSaved::Declined)) => {}
            (Awaited::SignIn { .. } | Awaited::Shortcut(..), Ok(())) => {
                unreachable!("shortcut and registry replies are handled separately")
            }
            (awaited, Err(NotSaved::Failed(reason))) => {
                let key = match awaited {
                    Awaited::Service(..) => "settings.not_saved",
                    Awaited::Rebuild => "settings_index.not_rebuilt",
                    Awaited::SignIn { .. } | Awaited::Shortcut(..) => {
                        unreachable!("shortcut and registry replies are handled separately")
                    }
                };
                let text = self.strings.format(key, &[&reason]);
                win::dialog::error_box(Some(dialog), &text, self.strings.text("settings.title"));
            }
        }
        self.show_preview();
        self.update_apply(dialog);
    }

    fn shortcut_answered(&self, dialog: Hwnd) {
        let answer = {
            let mut saving = self.saving.borrow_mut();
            let Some(Awaited::Shortcut(applying)) = saving.as_mut() else {
                return;
            };
            applying.registration.poll()
        };
        let Some(answer) = answer else { return };
        let Some(Awaited::Shortcut(applying)) = self.saving.take() else {
            return;
        };
        let Applying {
            registration,
            edited,
            last,
            after,
            ..
        } = *applying;
        match answer {
            Ok(()) => {
                if self.apply_ready(dialog, edited, last, after, Some(registration))
                    && after == AfterSave::Close
                    && self.apply(dialog, AfterSave::Close)
                {
                    dialog::end(dialog, DialogEnd::Ok);
                }
            }
            Err(error) => {
                let message = error.message(&self.strings, false);
                self.refuse(
                    dialog,
                    SettingsPage::Keyboard,
                    shortcut_control(error.mode()),
                    &message,
                );
                self.update_apply(dialog);
            }
        }
    }

    fn sign_in_answered(&self, dialog: Hwnd) {
        let (id, on, answer) = {
            let saving = self.saving.borrow();
            let Some(Awaited::SignIn { id, on, .. }) = saving.as_ref() else {
                return;
            };
            (*id, *on, self.main.settings.take_sign_in_saved(*id))
        };
        let Some(answer) = answer else { return };
        let changed = {
            let mut draft = self.draft.borrow_mut();
            let in_effect = if answer.is_ok() {
                on
            } else {
                draft.last_applied().start_at_sign_in
            };
            draft.sign_in_saved(on, in_effect)
        };
        if changed
            && let Some(check) = self.check_box(SettingsPage::General, ControlId::GeneralSignIn)
        {
            let on = self.draft.borrow().edited.start_at_sign_in;
            self.filling(|| check.set_checked(on));
        }
        if let Err(error) = answer {
            let text = self.strings.format(
                "message.registry_failed",
                &[
                    &without_access_key(self.strings.text("settings_general.sign_in")),
                    &win::error_text(&error),
                ],
            );
            win::dialog::error_box(Some(dialog), &text, self.strings.text("settings.title"));
        }
        if !self.main.settings.sign_in_is_current(self.main, id) || win::window::quit_pending() {
            return;
        }
        let Some(Awaited::SignIn { service, after, .. }) = self.saving.take() else {
            return;
        };
        let saving = self.save_service(service, after);
        if !saving && after == AfterSave::Close && self.apply(dialog, AfterSave::Close) {
            dialog::end(dialog, DialogEnd::Ok);
            return;
        }
        self.show_preview();
        self.update_apply(dialog);
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
            command_rows(
                &self.strings,
                &draft.edited.choices.shortcuts,
                Mode::ALL.map(|mode| self.main.shortcuts.registered(mode)),
                &filter,
            )
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
            Some(command @ (CommandId::ShowSearch | CommandId::ShowCommander)) => self
                .main
                .shortcuts
                .registered(if command == CommandId::ShowSearch {
                    Mode::Search
                } else {
                    Mode::Commander
                })
                .map(|show| show.text(&self.strings))
                .into_iter()
                .collect(),
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
    /// Global shortcuts are set in their boxes above the list.
    fn update_key_buttons(&self) {
        let command = self.selected_command().is_some_and(|command| {
            !matches!(command, CommandId::ShowSearch | CommandId::ShowCommander)
        });
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

    /// Only captured keystrokes set these boxes; pasted text is discarded.
    fn show_shortcut(&self) {
        for (mode, _, clear) in SHOW_SHORTCUTS {
            let shortcut = self.draft.borrow().edited.choices.show_shortcut(mode);
            if let Some(shortcut_box) = self.shortcut_box(mode) {
                let text =
                    shortcut.map_or_else(String::new, |shortcut| shortcut.text(&self.strings));
                self.filling(|| shortcut_box.set_text_caret_at_end(&text));
            }
            if let Some(clear) = self.control(SettingsPage::Keyboard, clear) {
                clear.set_enabled(shortcut.is_some());
            }
        }
        self.show_shortcut_hint();
    }

    /// The line under the shortcut boxes, which says what taking a Windows
    /// key over does for the shortcut being edited and is empty otherwise.
    /// Its height is fixed, so the page never moves while the person types.
    fn show_shortcut_hint(&self) {
        let capture = self.capture.take();
        let mode = capture.as_ref().map_or(Mode::Search, |(mode, _)| *mode);
        self.capture.set(capture);
        let shortcut = self.draft.borrow().edited.choices.show_shortcut(mode);
        if let Some(hint) = self.control(SettingsPage::Keyboard, ControlId::KeyboardShortcutHint) {
            let text = if let Some(shortcut) = shortcut.filter(|shortcut| shortcut.windows()) {
                let keys = shortcut.text(&self.strings);
                let warning = match shortcut.windows_action() {
                    Some(action) => self.strings.format(
                        "settings_keyboard.windows_use",
                        &[&keys, self.strings.text(action)],
                    ),
                    None => self
                        .strings
                        .format("settings_keyboard.windows_unknown", &[&keys]),
                };
                let detail = match shortcut.explorer_key() {
                    Some(key) => self.strings.format(
                        "settings_keyboard.windows_registry_hint",
                        &[&char::from(key).to_string()],
                    ),
                    None => self
                        .strings
                        .text("settings_keyboard.windows_hook_hint")
                        .to_owned(),
                };
                format!("{warning} {detail}")
            } else {
                String::new()
            };
            hint.set_text(&text);
        }
    }

    fn set_shortcut(&self, mode: Mode, shortcut: Option<ShowShortcut>) {
        let mut draft = self.draft.borrow_mut();
        match mode {
            Mode::Search => draft.edited.choices.show_shortcut = shortcut,
            Mode::Commander => draft.edited.choices.commander_shortcut = shortcut,
        }
        drop(draft);
        self.show_shortcut();
    }

    /// Takes the keys pressed in the shortcut box: `Backspace` alone
    /// removes the shortcut, and keys that cannot show Gozler, `Win+L`,
    /// leave it as it is. Keys that the other box holds move here, because
    /// one shortcut cannot show both modes, and a balloon on the other box
    /// says where they went.
    fn keys_captured(&self, mode: Mode, windows: bool, keystroke: Keystroke) {
        if !windows && keystroke == Keystroke::plain(Key::Backspace) {
            self.set_shortcut(mode, None);
        } else if let Some(shortcut) = ShowShortcut::pressed(windows, keystroke) {
            let (other, command) = match mode {
                Mode::Search => (Mode::Commander, "command.show_search"),
                Mode::Commander => (Mode::Search, "command.show_commander"),
            };
            let moved = self.draft.borrow().edited.choices.show_shortcut(other) == Some(shortcut);
            if moved {
                self.set_shortcut(other, None);
            }
            self.set_shortcut(mode, Some(shortcut));
            if moved && let Some(cleared) = self.shortcut_box(other) {
                let text = self.strings.format(
                    "settings_keyboard.shortcut_moved",
                    &[
                        &shortcut.text(&self.strings),
                        &without_access_key(self.strings.text(command)),
                    ],
                );
                self.note.replace(Some((cleared.hwnd(), text)));
            }
        }
    }

    /// Apply is enabled while a value differs from what was applied
    /// (DESIGN.md: Dialogs), a refused speed included. OK and Apply show
    /// the administrator shield while a service setting differs, because
    /// saving it asks Windows for permission. Rebuild is enabled while the
    /// service answers.
    fn update_apply(&self, dialog: Hwnd) {
        let (changed, service_changed) = {
            let draft = self.draft.borrow();
            (draft.can_apply(), draft.shielded())
        };
        let saving = self.saving.borrow().is_some();
        if let Some(apply) = dialog::item(dialog, ControlId::SettingsApply.id()) {
            if saving {
                focus_off(dialog, apply);
            }
            apply.set_enabled(changed && !saving);
        }
        for button in [
            DialogItem::Ok.id(),
            DialogItem::Cancel.id(),
            ControlId::SettingsResetAll.id(),
        ] {
            if let Some(button) = dialog::item(dialog, button) {
                if saving {
                    focus_off(dialog, button);
                }
                button.set_enabled(!saving);
            }
        }
        if self.apply_shield.replace(Some(service_changed)) != Some(service_changed) {
            for (button, key) in [
                (DialogItem::Ok.id(), "button.ok"),
                (ControlId::SettingsApply.id(), "settings.apply"),
            ] {
                if let Some(button) = Button::of_dialog(dialog, button) {
                    button.set_icon(ButtonIcon::of(key, service_changed));
                }
            }
        }
        self.update_rebuild(dialog);
        self.show_status(dialog);
    }

    fn update_rebuild(&self, dialog: Hwnd) {
        let saving = self.saving.borrow().is_some();
        let answers = self.index.borrow().as_ref().is_some_and(|c| c.has_report);
        if let Some(rebuild) = self.control(SettingsPage::Index, ControlId::IndexRebuild) {
            if saving || !answers {
                focus_off(dialog, rebuild);
            }
            rebuild.set_enabled(answers && !saving);
        }
    }

    fn show_status(&self, dialog: Hwnd) {
        let Some(status) = self.status.get() else {
            return;
        };
        let now = Instant::now();
        let request = self.request_waiting(now);
        let fixing = self.main.fix_waiting(now);
        let (job, observed) = {
            let saving = self.saving.borrow();
            match saving.as_ref() {
                Some(awaited) => {
                    let waiting = match awaited {
                        Awaited::Shortcut(applying) => Some(
                            Interest::Awaited
                                .after(now.saturating_duration_since(applying.accepted)),
                        ),
                        Awaited::SignIn { id, .. } => self.main.sign_in_waiting(*id, now),
                        Awaited::Service(..) | Awaited::Rebuild => self.main.elevated_waiting(now),
                    };
                    let key = match awaited {
                        Awaited::Shortcut(..) | Awaited::SignIn { .. } | Awaited::Service(..) => {
                            "settings.saving"
                        }
                        Awaited::Rebuild => "settings.rebuilding",
                    };
                    (
                        JobCaption {
                            text: if waiting == Some(Waiting::Slow) {
                                work::waiting(&self.strings, None)
                            } else {
                                self.strings.text(key).to_owned()
                            },
                            progress: JobProgress::Unmeasured,
                            ..JobCaption::default()
                        },
                        true,
                    )
                }
                None => {
                    let mut job = self.main.settings.settings_job(self.main, &self.strings);
                    if self.status_request(now).is_some() {
                        job = JobCaption {
                            text: work::waiting(&self.strings, None),
                            progress: JobProgress::Unmeasured,
                            cancel: true,
                        };
                    }
                    (
                        job,
                        fixing.is_some()
                            || request.is_some()
                            || self.main.settings.settings_waiting(self.main, now),
                    )
                }
            }
        };
        if observed {
            dialog.set_timer(WAIT_TIMER, WAIT_POLL_MS);
        } else {
            dialog.kill_timer(WAIT_TIMER);
        }
        let changed =
            dialog::item(dialog, ControlId::SettingsApply.id()).is_some_and(Hwnd::is_enabled);
        if status.render(
            &[
                (
                    StatusSlot::Left,
                    StatusEntry::Text(captions::settings_status_view(&self.strings, changed)),
                ),
                (
                    StatusSlot::Center,
                    StatusEntry::Work(super::status_bar::WorkId::Waiting, &job),
                ),
            ],
            self.status_layout.get().as_ref(),
        ) && let Some(pages) = dialog::item(dialog, ControlId::SettingsPages.id())
        {
            pages.focus();
        }
    }

    fn request_waiting(&self, now: Instant) -> Option<(Purpose, Waiting)> {
        let purpose = match self.shown.get()? {
            SettingsPage::ContextMenu => Purpose::Program,
            SettingsPage::Include => Purpose::IncludeCount,
            _ => return None,
        };
        self.main
            .request_waiting(purpose, now)
            .map(|waiting| (purpose, waiting))
    }

    fn cancel_status(&self, dialog: Hwnd) {
        if self.saving.borrow().is_some() {
            return;
        }
        let now = Instant::now();
        if let Some(purpose) = self.status_request(now) {
            self.main.cancel_settings_request(self.window, purpose);
        } else if self.main.fix_waiting(now).is_some() {
            if self
                .main
                .settings
                .settings_job(self.main, &self.strings)
                .cancel
            {
                self.main.cancel_settings_fix(self.window);
            }
        } else {
            self.main.settings.cancel_settings_waiting(self.main);
        }
        self.show_status(dialog);
    }

    fn status_request(&self, now: Instant) -> Option<Purpose> {
        if self.main.fix_waiting(now).is_some() {
            return None;
        }
        self.request_waiting(now)
            .and_then(|(purpose, waiting)| (waiting == Waiting::Slow).then_some(purpose))
    }

    /// Makes `page`, unless it was made: its controls, their lists and
    /// tooltips, and their values from the draft.
    fn make_page(&self, dialog: Hwnd, page: SettingsPage) {
        let Some(index) = SettingsPage::ALL.iter().position(|p| *p == page) else {
            return;
        };
        if self.pages[index].get().is_some() {
            return;
        }
        let id = page.dialog();
        let Ok(hwnd) = dialog::create_page(dialog, id, &DialogCaptions::new(id, &self.strings))
        else {
            dialog::end(dialog, DialogEnd::Cancel);
            return;
        };
        // After the page list in the Tab order, before OK.
        if let (Some(area), Some(list)) = (
            dialog::item(dialog, ControlId::SettingsPageArea.id()),
            dialog::item(dialog, ControlId::SettingsPages.id()),
        ) {
            hwnd.place_after(list, area.rect_in(dialog));
        }
        if let Some(heading) = dialog::item(hwnd, heading(page).id())
            && let Ok(font) = Font::of(heading).and_then(|font| font.semibold())
        {
            heading.set_font(&font);
            for (title, _, _) in SETTINGS_GROUPS {
                if let Some(title) = dialog::item(hwnd, title.id()) {
                    title.set_font(&font);
                }
            }
            self.fonts.borrow_mut().push(font);
        }
        self.show_group_icons(hwnd);
        self.pages[index].set(Some(hwnd));
        // The first page is made before the dialog sizes its buttons.
        if let Some(width) = self.button_width.get() {
            dialog::size_page_buttons(hwnd, self, width);
        }
        self.set_up_lists(page);
        self.add_tooltips(page, hwnd);
        if page == SettingsPage::Index
            && let Some(rebuild) = Button::of_dialog(hwnd, ControlId::IndexRebuild.id())
        {
            rebuild.set_icon(Some(ButtonIcon::Shield));
        }
        self.fill(page);
        let search = dialog::item(dialog, ControlId::SettingsSearch.id())
            .map(Hwnd::text)
            .unwrap_or_default();
        if !search.is_empty() {
            self.highlight_page(page, &search);
        }
    }

    /// Gives the lists of `page` their items or columns, and the boxes
    /// their cues.
    fn set_up_lists(&self, page: SettingsPage) {
        let text = |key: &'static str| self.strings.text(key);
        match page {
            SettingsPage::General => {
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
                if let Some(list) = self.size_list() {
                    let names: Vec<&str> = SizeFormat::ALL
                        .iter()
                        .map(|format| text(format.label()))
                        .collect();
                    list.set_items(&names, 0);
                }
                if let Some(list) = self.date_list() {
                    let names: Vec<&str> = DateFormat::ALL
                        .iter()
                        .map(|format| text(format.label()))
                        .collect();
                    list.set_items(&names, 0);
                }
            }
            SettingsPage::Search => {}
            SettingsPage::ContextMenu => {
                let Some(page) = self.page(SettingsPage::ContextMenu) else {
                    return;
                };
                if let Some(list) = ListBox::of_dialog(page, ControlId::ContextMenuItems.id()) {
                    let names: Vec<String> = ContextItem::all()
                        .into_iter()
                        .map(|item| without_access_key(text(item.label())))
                        .collect();
                    list.set_items(&names, 0);
                }
                for (control, _, cue, _) in OPEN_COMMANDS {
                    if let Some(edit) = Edit::of_dialog(page, control.id()) {
                        edit.set_cue(text(cue));
                    }
                }
                // The link ends the hint's line, at the Browse… buttons'
                // right edge.
                if let (Some(link), Some(hint)) = (
                    dialog::item(page, ControlId::ContextMenuTotalCommander.id()),
                    dialog::item(page, ControlId::ContextMenuCommandHint.id()),
                ) {
                    dialog::fit_link(link, hint);
                }
            }
            SettingsPage::Keyboard => {
                if let Some(page) = self.page(SettingsPage::Keyboard) {
                    for control in [
                        ControlId::KeyboardShortcut,
                        ControlId::KeyboardCommanderShortcut,
                    ] {
                        if let Some(edit) = Edit::of_dialog(page, control.id()) {
                            edit.set_cue(text("settings_keyboard.shortcut_hint"));
                        }
                    }
                }
                if let Some(list) = self.command_list() {
                    list.set_columns(&[
                        (text("settings_keyboard.command_column"), 62, false),
                        (text("settings_keyboard.keys_column"), 38, false),
                    ]);
                }
            }
            SettingsPage::FileOperations => {
                if let Some(list) = self.file_operations_list(ControlId::SpeedTransfers) {
                    list.set_items(&["1", "2", "3"], 0);
                }
                if let Some(list) = self.file_operations_list(ControlId::FileOperationsPolicy) {
                    list.set_items(
                        &crate::core::file_operations::policy_names(&self.strings),
                        0,
                    );
                }
            }
            SettingsPage::Performance => {
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
                            self.strings
                                .format("settings_file_operations.block_kib", &[&kib.to_string()])
                        } else {
                            self.strings.format(
                                "settings_file_operations.block_mib",
                                &[&(kib / 1024).to_string()],
                            )
                        }
                    })
                    .collect();
                for (control, _) in SPEED_BLOCKS {
                    if let Some(list) = self.speed_list(control) {
                        list.set_items(&blocks, 0);
                    }
                }
            }
            SettingsPage::Index => {
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
            }
            SettingsPage::Include => {
                if let Some(list) = self.include_folders() {
                    list.set_columns(&[
                        (text("settings_include.folder_column"), 30, false),
                        (text("settings_include.left_out_column"), 18, false),
                        (text("settings_include.folders_column"), 52, false),
                    ]);
                    list.show_info_tips();
                }
            }
            SettingsPage::Visibility => {
                if let Some(list) = self.visibility_list() {
                    list.set_columns(&[("", 100, false)]);
                }
            }
        }
    }

    /// Gives `page`, whose window is `hwnd`, its tooltip: its Reset says
    /// which settings it restores, the Use Total Commander link says what it
    /// does, the Service group's fix buttons say what their fix does, Clear
    /// says its name, because it can show only its icon, and the shortcut
    /// boxes say when their shortcut works.
    fn add_tooltips(&self, page: SettingsPage, hwnd: Hwnd) {
        let text = |key: &'static str| self.strings.text(key);
        let clear = without_access_key(text("button.clear"));
        let reset = RESTORE
            .into_iter()
            .find(|(restored, _)| *restored == page)
            .map(|(_, reset)| (reset, text("settings.reset_tooltip")));
        let others: &[(ControlId, &str)] = match page {
            SettingsPage::ContextMenu => &[(
                ControlId::ContextMenuTotalCommander,
                text("settings_context_menu.total_commander_tooltip"),
            )],
            SettingsPage::Keyboard => &[
                (ControlId::KeyboardClear, clear.as_str()),
                (ControlId::KeyboardCommanderClear, clear.as_str()),
                (
                    ControlId::KeyboardShortcut,
                    text("settings_keyboard.shortcut_tip"),
                ),
                (
                    ControlId::KeyboardCommanderShortcut,
                    text("settings_keyboard.shortcut_tip"),
                ),
            ],
            // `refresh_index` gives these their tips, but Rebuild's.
            SettingsPage::Index => &[
                (
                    ControlId::IndexRebuild,
                    text("settings_index.rebuild_tooltip"),
                ),
                (ControlId::IndexUseLocal, ""),
                (ControlId::IndexFix, ""),
                (ControlId::IndexServiceProgramPath, ""),
                (ControlId::IndexBesidePath, ""),
            ],
            _ => &[],
        };
        let tipped: Vec<(Hwnd, &str)> = reset
            .iter()
            .chain(others)
            .filter_map(|&(control, tip)| Some((dialog::item(hwnd, control.id())?, tip)))
            .collect();
        if tipped.is_empty() {
            return;
        }
        let controls: Vec<Hwnd> = tipped.iter().map(|&(control, _)| control).collect();
        let Ok(tips) = Tooltip::create(hwnd, &controls) else {
            return;
        };
        for (control, tip) in tipped {
            tips.set(control, tip);
        }
        if page == SettingsPage::Index {
            let _ = self.index_tips.set(tips);
        }
    }

    /// Shows `page`, or no page, and has the main window follow the service
    /// while the Index page shows (DESIGN.md: Settings dialog).
    fn show_page(&self, dialog: Hwnd, page: Option<SettingsPage>) {
        if let Some(page) = page {
            self.make_page(dialog, page);
        }
        let previous = self.shown.replace(page);
        self.fill_page_list(dialog);
        self.show_status(dialog);
        if previous == page {
            return;
        }
        for (each, hwnd) in SettingsPage::ALL.into_iter().zip(&self.pages) {
            if let Some(hwnd) = hwnd.get() {
                hwnd.set_visible(Some(each) == page);
            }
        }
        if page == Some(SettingsPage::Include) {
            // The index changes while the dialog is open.
            self.show_preview();
        }
        if page == Some(SettingsPage::Visibility) {
            self.show_seen();
        }
        self.main
            .settings
            .show_settings_page(self.main, self.window, page);
        if page == Some(SettingsPage::Index) {
            dialog.set_timer(INDEX_TIMER, INDEX_REFRESH_MS);
            self.refresh_index(dialog);
        } else if previous == Some(SettingsPage::Index) {
            dialog.kill_timer(INDEX_TIMER);
        }
    }

    /// Lists the pages that the search keeps, with the shown one selected.
    fn fill_page_list(&self, dialog: Hwnd) {
        let Some(list) = self.page_list(dialog) else {
            return;
        };
        let (items, index) = {
            let listed = self.listed.borrow();
            let shown = self.shown.get();
            // Each page shows its icon, at its place in `SettingsPage::ALL`.
            let items: Vec<(&str, usize)> = listed
                .iter()
                .filter_map(|page| {
                    let icon = SettingsPage::ALL.iter().position(|p| p == page)?;
                    Some((self.strings.text(page.label()), icon))
                })
                .collect();
            (items, listed.iter().position(|page| Some(*page) == shown))
        };
        self.filling(|| list.set_items(&items, index.unwrap_or(0)));
    }

    /// Gives the page list its pages' icons, again when icons are shown or
    /// hidden.
    fn show_page_icons(&self, dialog: Hwnd) {
        if let Some(list) = self.page_list(dialog) {
            list.set_icons(&SettingsPage::ALL.map(SettingsPage::glyph));
        }
    }

    /// Gives each group's title on `page` its icon, again when icons are
    /// shown or hidden.
    fn show_group_icons(&self, page: Hwnd) {
        for (title, icon, glyph) in SETTINGS_GROUPS {
            if let (Some(title), Some(icon)) = (
                dialog::item(page, title.id()),
                GlyphLabel::of_dialog(page, icon.id()),
            ) {
                show_heading_icon(icon, title, glyph);
            }
        }
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
    /// heading and its groups' titles keep their own font.
    fn highlight(&self, search: &str) {
        for page in SettingsPage::ALL {
            self.highlight_page(page, search);
        }
    }

    fn highlight_page(&self, page: SettingsPage, search: &str) {
        let Some((normal, bold)) = self.highlight.get() else {
            return;
        };
        if self.page(page).is_none() {
            return;
        }
        for (control, hit) in captions::settings_page_hits(&self.strings, page, search) {
            if control != heading(page)
                && !SETTINGS_GROUPS
                    .iter()
                    .any(|(title, _, _)| *title == control)
                && let Some(control) = self.control(page, control)
            {
                control.set_font(if hit { bold } else { normal });
            }
        }
    }

    /// Shows the service's latest state, its programs and its failure's
    /// fix on the Index page. It sets only what changed since the last
    /// refresh, because setting a control redraws it, and the memory and
    /// the entry counts change at nearly every 2-second refresh.
    fn refresh_index(&self, dialog: Hwnd) {
        // The page refreshes when it is made, so it shows the latest state.
        if self.page(SettingsPage::Index).is_none() {
            return;
        }
        let captions = self.main.settings.index_captions(self.main);
        // Stored first, because the drive list asks for its empty text
        // while its rows change.
        let shown = self.index.replace(Some(captions.clone()));
        let shown = shown.as_ref();
        if shown == Some(&captions) {
            return;
        }
        let changed = |part: fn(&IndexCaptions) -> &str| {
            shown.is_none_or(|shown| part(shown) != part(&captions))
        };
        if let Some(list) = self
            .page(SettingsPage::Index)
            .and_then(|page| ReportList::of_dialog(page, ControlId::IndexDrives.id()))
        {
            // Replacing the rows makes the list ask for its empty text again.
            match shown.filter(|shown| shown.no_drives == captions.no_drives) {
                Some(shown) => list.update_rows(&shown.drives, &captions.drives),
                None => list.set_rows(&captions.drives, None),
            }
        }
        // The paths end in an ellipsis, so their tooltips show them whole.
        type Text = fn(&IndexCaptions) -> &str;
        let texts: [(ControlId, Text, bool); 6] = [
            (ControlId::IndexService, |c| c.service.as_str(), false),
            (
                ControlId::IndexServiceProgramPath,
                |c| c.service_program.path.as_str(),
                true,
            ),
            (
                ControlId::IndexServiceProgramBuild,
                |c| c.service_program.build.as_str(),
                false,
            ),
            (ControlId::IndexBesidePath, |c| c.beside.path.as_str(), true),
            (
                ControlId::IndexBesideBuild,
                |c| c.beside.build.as_str(),
                false,
            ),
            (ControlId::IndexNote, |c| c.note.as_str(), false),
        ];
        for (control, text, tipped) in texts {
            let Some(hwnd) = self
                .control(SettingsPage::Index, control)
                .filter(|_| changed(text))
            else {
                continue;
            };
            hwnd.set_text(text(&captions));
            if tipped && let Some(tips) = self.index_tips.get() {
                tips.set(hwnd, text(&captions));
            }
        }
        // A button with nothing to offer stays in its place, disabled, with
        // the caption paired with it here (DESIGN.md: The Drives and service
        // page).
        let fixes = [
            (
                ControlId::IndexUseLocal,
                &captions.use_local,
                shown.map(|shown| &shown.use_local),
                "fix.use_local",
            ),
            (
                ControlId::IndexFix,
                &captions.fix,
                shown.map(|shown| &shown.fix),
                "fix.start_service",
            ),
        ];
        for (control, fix, shown_fix, idle) in fixes {
            if shown_fix == Some(fix) {
                continue;
            }
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
            let (label, icon, enabled, tooltip) = match fix {
                Some(fix) => (
                    fix.label.as_str(),
                    fix.icon,
                    fix.enabled,
                    fix.tooltip.as_deref().unwrap_or_default(),
                ),
                None => (self.strings.text(idle), None, false, ""),
            };
            hwnd.set_text(label);
            button.set_icon(icon);
            hwnd.set_enabled(enabled);
            if let Some(tips) = self.index_tips.get() {
                tips.set(hwnd, tooltip);
            }
        }
        if shown.is_none_or(|shown| shown.has_report != captions.has_report)
            && let Some(report) = self.control(SettingsPage::Index, ControlId::IndexReport)
        {
            if !captions.has_report {
                focus_off(dialog, report);
            }
            report.set_enabled(captions.has_report);
        }
        if shown.is_none_or(|shown| shown.has_report != captions.has_report) {
            self.update_rebuild(dialog);
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
    /// were applied; a speed that cannot be one, or a shortcut that another
    /// program holds, keeps the dialog open (DESIGN.md: Dialogs). Changed
    /// service settings are applied once they are saved, and `after` says
    /// what the dialog does then.
    fn apply(&self, dialog: Hwnd, after: AfterSave) -> bool {
        if let Err((control, message)) = self.read_speed() {
            self.refuse(dialog, SettingsPage::Performance, control, &message);
            return false;
        }
        let (edited, last) = {
            let draft = self.draft.borrow();
            (draft.edited.clone(), draft.last_applied().clone())
        };
        if edited == last {
            return true;
        }
        let registration = match self.main.shortcuts.prepare(
            self.main,
            self.window,
            &edited.choices,
            &last.choices,
        ) {
            Err(error) => {
                let message = error.message(&self.strings, false);
                self.refuse(
                    dialog,
                    SettingsPage::Keyboard,
                    shortcut_control(error.mode()),
                    &message,
                );
                return false;
            }
            Ok(registration) => registration,
        };
        let registration = if let Some(mut prepared) = registration {
            match prepared.poll() {
                None => {
                    self.saving
                        .replace(Some(Awaited::Shortcut(Box::new(Applying {
                            registration: prepared,
                            edited,
                            last,
                            after,
                            accepted: Instant::now(),
                        }))));
                    self.update_apply(dialog);
                    return false;
                }
                Some(Err(error)) => {
                    let message = error.message(&self.strings, false);
                    self.refuse(
                        dialog,
                        SettingsPage::Keyboard,
                        shortcut_control(error.mode()),
                        &message,
                    );
                    return false;
                }
                Some(Ok(())) => {}
            }
            Some(prepared)
        } else {
            None
        };
        self.apply_ready(dialog, edited, last, after, registration)
    }

    fn apply_ready(
        &self,
        dialog: Hwnd,
        edited: SettingsValues,
        last: SettingsValues,
        after: AfterSave,
        registration: Option<PreparedShortcuts>,
    ) -> bool {
        self.main.settings.apply_choices(
            self.main,
            self.window,
            &edited.choices,
            &last.choices,
            registration,
        );
        self.draft
            .borrow_mut()
            .choices_applied(edited.choices.clone());
        let service = edited.service_changes(&last);
        let saving = if edited.start_at_sign_in != last.start_at_sign_in {
            let id = self
                .main
                .settings
                .set_sign_in(self.main, edited.start_at_sign_in);
            self.saving.replace(Some(Awaited::SignIn {
                id,
                on: edited.start_at_sign_in,
                service,
                after,
            }));
            true
        } else {
            self.save_service(service, after)
        };
        if Mode::ALL
            .into_iter()
            .any(|mode| edited.choices.show_shortcut(mode) != last.choices.show_shortcut(mode))
        {
            // These rows show what Windows registered.
            self.fill_commands(self.selected_command());
        }
        let icons_changed = edited.choices.show_icons != last.choices.show_icons;
        if icons_changed {
            // The page list's rows, the group titles and the
            // buttons follow.
            self.show_page_icons(dialog);
            for page in self.pages.iter().filter_map(Cell::get) {
                self.show_group_icons(page);
            }
            self.show_button_glyphs(dialog);
            self.refresh_index(dialog);
        }
        self.show_preview();
        self.update_apply(dialog);
        if icons_changed {
            let width = dialog::size_buttons(dialog, self);
            self.buttons_sized(dialog, width);
        }
        !saving
    }

    /// Gives the push buttons of the dialog and its pages their icons, as
    /// they show while icons show or are hidden.
    fn show_button_glyphs(&self, dialog: Hwnd) {
        self.apply_shield.set(None);
        dialog::show_glyphs(
            dialog,
            &DialogCaptions::new(DialogId::Settings, &self.strings),
        );
        for page in SettingsPage::ALL {
            if let Some(hwnd) = self.page(page) {
                dialog::show_glyphs(hwnd, &DialogCaptions::new(page.dialog(), &self.strings));
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
        let Some(found) = self.main.settings.take_total_commander() else {
            return;
        };
        let link = self.control(
            SettingsPage::ContextMenu,
            ControlId::ContextMenuTotalCommander,
        );
        let note = match found {
            Ok(Some(path)) => {
                let changed = self.draft.borrow_mut().use_total_commander(&path);
                if changed {
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
        let state = CommandShortcut::new(&self.strings, replaced);
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
        if let Ok(moved) = added {
            self.fill_commands(Some(command));
            if let (Some(from), Some(keys)) = (moved, self.key_list()) {
                let text = self.strings.format(
                    "settings_keyboard.shortcut_moved_from",
                    &[
                        &keystroke.text(&self.strings),
                        &command_place(&self.strings, from),
                    ],
                );
                self.note.replace(Some((keys.hwnd(), text)));
            }
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
            // A list view also tells of the item that lost the selection,
            // while none is selected.
            let selected = self
                .page_list(dialog)
                .and_then(IconList::selected)
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
        } else if is(id, ControlId::GeneralSizes) {
            let chosen = self
                .size_list()
                .and_then(DropDownList::selected)
                .and_then(|place| SizeFormat::ALL.get(place).copied());
            if let Some(format) = chosen {
                self.draft.borrow_mut().edited.choices.size_format = format;
            }
        } else if is(id, ControlId::GeneralDates) {
            let chosen = self
                .date_list()
                .and_then(DropDownList::selected)
                .and_then(|place| DateFormat::ALL.get(place).copied());
            if let Some(format) = chosen {
                self.draft.borrow_mut().edited.choices.date_format = format;
            }
        } else if SHOW_SHORTCUTS
            .into_iter()
            .any(|(_, control, _)| is(id, control))
        {
            // Undoes a paste from the box's context menu: the keys pressed
            // in it arrive as DialogMessage::KeysCaptured.
            self.show_shortcut();
        } else if is(id, ControlId::KeyboardFilter) {
            self.fill_commands(self.selected_command());
        } else if is(id, ControlId::KeyboardCommands) {
            self.fill_keys(0);
        } else if is(id, ControlId::KeyboardKeys) {
            self.update_key_buttons();
        } else if is(id, ControlId::FileOperationsPolicy) {
            if let Some(policy) = self
                .file_operations_list(ControlId::FileOperationsPolicy)
                .and_then(DropDownList::selected)
                .and_then(|i| gozler_fileops::Policy::ALL.get(i).copied())
            {
                self.draft.borrow_mut().edited.choices.copy_policy = policy;
            }
        } else if is(id, ControlId::SpeedTransfers) {
            if let Some(place) = self
                .file_operations_list(ControlId::SpeedTransfers)
                .and_then(DropDownList::selected)
            {
                self.draft.borrow_mut().edited.choices.transfers_per_device =
                    gozler_fileops::TransferLimit::try_from(place as u32 + 1)
                        .expect("the list contains transfer limits 1 through 3");
            }
        } else if is(id, ControlId::SpeedPreset) {
            let chosen = self
                .speed_list(ControlId::SpeedPreset)
                .and_then(DropDownList::selected)
                .and_then(|place| SpeedPreset::ALL.get(place).copied());
            if let Some(preset) = chosen {
                self.draft.borrow_mut().edited.choices.speed.preset = preset;
                self.fill(SettingsPage::Performance);
            }
        } else if let Some((control, number)) = SPEED_NUMBERS.into_iter().find(|(c, _)| is(id, *c))
        {
            let text = self
                .control(SettingsPage::Performance, control)
                .map(Hwnd::text)
                .unwrap_or_default();
            // A number out of its range is refused when OK or Apply is
            // pressed.
            let preset = {
                let mut draft = self.draft.borrow_mut();
                draft.type_number(number, &text);
                draft.edited.choices.speed.preset
            };
            self.show_preset(preset);
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
        } else if is(id, ControlId::VisibilityList) {
            self.update_visibility_buttons();
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
        if is(id, ControlId::GeneralSelectFiles) || is(id, ControlId::GeneralSelectFolders) {
            self.draft.borrow_mut().edited.choices.select_folders =
                is(id, ControlId::GeneralSelectFolders);
        } else if let Some((page, control, value)) =
            CHECK_BOXES.into_iter().find(|(_, c, _)| is(id, *c))
        {
            let checked = self
                .check_box(page, control)
                .is_some_and(CheckBox::is_checked);
            *value(&mut self.draft.borrow_mut().edited) = checked;
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
            if control == ControlId::IncludeNewDrives {
                self.main.settings.include_summary_changed();
            }
            self.include_changed(dialog);
        } else if is(id, ControlId::IndexDatesCreated) {
            let checked = self
                .check_box(SettingsPage::Index, ControlId::IndexDatesCreated)
                .is_some_and(CheckBox::is_checked);
            if let Some(include) = &mut self.draft.borrow_mut().edited.include {
                include.dates_created = checked;
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
                .check_box(SettingsPage::Performance, ControlId::SpeedPerProcessor)
                .is_some_and(CheckBox::is_checked);
            self.change_speed(|settings| speed::set_per_processor(settings, per_processor));
            self.fill(SettingsPage::Performance);
        } else if is(id, ControlId::IncludeModify) {
            self.modify_folders(dialog);
        } else if is(id, ControlId::VisibilityEveryone) {
            let checked = self
                .check_box(SettingsPage::Visibility, ControlId::VisibilityEveryone)
                .is_some_and(CheckBox::is_checked);
            let selected = self.visibility_list().and_then(ReportList::selected);
            let selected = match &mut self.draft.borrow_mut().edited.see_every_file {
                // Everyone may sit above the selected row, so the selection
                // follows its account, not its row, or Remove would act on
                // another account.
                Ok(list) => {
                    let line = selected.and_then(|row| list.lines().get(row).cloned());
                    list.set_everyone(checked);
                    line.and_then(|line| list.lines().iter().position(|each| *each == line))
                }
                Err(_) => selected,
            };
            self.fill_visibility(selected);
        } else if is(id, ControlId::VisibilityAdd) {
            self.add_accounts(dialog);
        } else if is(id, ControlId::VisibilityRemove) {
            self.remove_account();
        } else if let Some((mode, _, _)) = SHOW_SHORTCUTS
            .into_iter()
            .find(|(_, _, clear)| is(id, *clear))
        {
            // As Backspace in the box does.
            self.set_shortcut(mode, None);
            if let Some(shortcut_box) = self.shortcut_box(mode) {
                shortcut_box.hwnd().focus();
            }
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
    fn closed(&self, dialog: Hwnd) {
        dialog.kill_timer(WAIT_TIMER);
        self.capture.take();
        self.saving.take();
        self.main.settings.settings_closed(dialog);
    }

    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(list) = self.page_list(dialog) {
            list.set_up();
        }
        self.show_page_icons(dialog);
        self.fill_page_list(dialog);
        self.fit_page_list(dialog);
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        // Reset all says which settings it restores, and Clear says its
        // name, because it can show only its icon. Each page has a tooltip
        // of its own, and Windows destroys the tooltips with the dialog.
        let clear = without_access_key(self.strings.text("button.clear"));
        let tipped: Vec<(Hwnd, &str)> = [
            (
                ControlId::SettingsResetAll,
                self.strings.text("settings.reset_all_tooltip"),
            ),
            (ControlId::SettingsClearSearch, clear.as_str()),
        ]
        .into_iter()
        .filter_map(|(control, tip)| Some((dialog::item(dialog, control.id())?, tip)))
        .collect();
        let controls: Vec<Hwnd> = tipped.iter().map(|&(control, _)| control).collect();
        if let Ok(tips) = Tooltip::create(dialog, &controls) {
            for (control, tip) in tipped {
                tips.set(control, tip);
            }
        }
        if let Some(search) = Edit::of_dialog(dialog, ControlId::SettingsSearch.id()) {
            search.set_cue(self.strings.text("settings.search_cue"));
        }
        self.add_status_bar(dialog, &mut fonts);
        self.main.settings.settings_opened(self.main, dialog);
        self.show_page(dialog, Some(SettingsPage::General));
        if let Some(label) = self
            .page(SettingsPage::General)
            .and_then(|page| dialog::item(page, ControlId::GeneralLanguageLabel.id()))
            && let Ok(normal) = Font::of(label)
            && let Ok(bold) = normal.bold()
        {
            let _ = self.highlight.set((normal, bold));
        }
        self.update_apply(dialog);
        Opened {
            focus: ControlId::SettingsPages.id(),
            fonts,
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        if self.saving.borrow().is_some()
            && let DialogMessage::Command { id } = message
            && [
                DialogItem::Ok.id(),
                DialogItem::Cancel.id(),
                ControlId::SettingsApply.id(),
                ControlId::SettingsResetAll.id(),
            ]
            .contains(&id)
        {
            // Esc and the close button also preserve an accepted write.
            return true;
        }
        let handled = match message {
            DialogMessage::Edited { .. } if self.filling.get() => return true,
            DialogMessage::Edited { id } => {
                self.edited(dialog, id);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                if self.apply(dialog, AfterSave::Close) {
                    dialog::end(dialog, DialogEnd::Ok);
                }
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::SettingsResetAll) => {
                if win::dialog::ask(dialog, &captions::reset_all_question(&self.strings)) {
                    self.resets.set(true);
                    dialog::end(dialog, DialogEnd::Cancel);
                }
                true
            }
            DialogMessage::Command { id } if is(id, ControlId::SettingsApply) => {
                self.apply(dialog, AfterSave::Stay);
                return true;
            }
            DialogMessage::Command { id } if is(id, ControlId::StatusCancel) => {
                self.cancel_status(dialog);
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
            DialogMessage::Command { id } if is(id, ControlId::IndexRebuild) => {
                self.rebuild(dialog);
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
            DialogMessage::Focus { id, focused }
                if SHOW_SHORTCUTS
                    .into_iter()
                    .any(|(_, control, _)| is(id, control)) =>
            {
                self.capture.take();
                if focused {
                    let mode = SHOW_SHORTCUTS
                        .into_iter()
                        .find(|(_, control, _)| is(id, *control))
                        .expect("the focused control is a shortcut box")
                        .0;
                    self.capture.set(
                        shortcut::capture(dialog)
                            .ok()
                            .map(|capture| (mode, capture)),
                    );
                    self.show_shortcut();
                }
                return true;
            }
            DialogMessage::KeysCaptured {
                capture: token,
                windows,
                keystroke,
            } => {
                let capture = self.capture.take();
                let mode = capture
                    .as_ref()
                    .filter(|(_, capture)| capture.accepts(token))
                    .map(|(mode, _)| *mode);
                self.capture.set(capture);
                if let Some(mode) = mode {
                    self.keys_captured(mode, windows, keystroke);
                }
                true
            }
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
                self.main.settings.refresh_volume_labels(self.main);
                self.refresh_index(dialog);
                self.show_status(dialog);
                return true;
            }
            DialogMessage::Timer { id: PREVIEW_TIMER } => {
                self.count_preview(dialog);
                self.show_status(dialog);
                return true;
            }
            DialogMessage::Timer { id: WAIT_TIMER } => {
                self.show_status(dialog);
                return true;
            }
            DialogMessage::Reply => {
                self.total_commander_found(dialog);
                if self.main.settings.take_account_names_changed()
                    && self.page(SettingsPage::Visibility).is_some()
                {
                    let selected = self.visibility_list().and_then(ReportList::selected);
                    self.fill_visibility(selected);
                }
                self.show_seen();
                if self.main.settings.take_include_summary_changed()
                    && self.include_folders().is_some()
                {
                    let include = self.draft.borrow().edited.include.clone();
                    if let Some(include) = include {
                        self.refresh_include_rows(&include);
                    }
                }
                if self.main.settings.take_include_preview_changed()
                    || self.draft.borrow().edited.include.is_none()
                {
                    self.show_preview();
                }
                self.save_answered(dialog);
                // A fix moved on, which the fix button follows.
                self.refresh_index(dialog);
                self.show_status(dialog);
                return true;
            }
            DialogMessage::LinkActivated { id } if is(id, ControlId::ContextMenuTotalCommander) => {
                self.main.settings.find_total_commander(self.main);
                true
            }
            DialogMessage::Focus { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Tree { .. }
            | DialogMessage::Other => return false,
        };
        if handled {
            if let Some(balloon) = self.balloon.get() {
                balloon.hide();
                if let Some((control, text)) = self.note.take() {
                    balloon.show(control, &text);
                }
            }
            self.update_apply(dialog);
        }
        handled
    }

    /// Places the status bar, whose Cancel is as wide as OK and Apply.
    fn buttons_sized(&self, dialog: Hwnd, width: Px) {
        self.button_width.set(Some(width));
        let (Some(status), Some((widths, font_height))) =
            (self.status.get(), self.status_widths.get())
        else {
            return;
        };
        let dpi = dialog.dpi();
        let height = status.fit(StatusLayout::min_height(dpi, *font_height));
        let layout = StatusLayout::for_dialog(
            widths,
            width,
            dpi,
            *font_height,
            height,
            dialog.client_size().width,
        );
        self.status_layout.set(Some(layout));
        status.place(&layout);
        self.show_status(dialog);
    }

    /// Every caption the Service group's fix buttons can show, OK and
    /// Apply with the shield, which they show while Include has changes,
    /// Rebuild with its shield, and every page's push buttons but Clear,
    /// because a page is made when it first shows. Each of them shows an
    /// icon also without the shield while icons show.
    fn later_captions(&self) -> Vec<(String, bool)> {
        let icons = self.draft.borrow().last_applied().choices.show_icons;
        let mut captions: Vec<(String, bool)> = Fix::label_groups()
            .into_iter()
            .flatten()
            .chain([
                ("button.ok", true),
                ("settings.apply", true),
                ("settings_index.rebuild", true),
            ])
            .map(|(key, shielded)| (self.strings.text(key).to_owned(), shielded || icons))
            .collect();
        for page in SettingsPage::ALL {
            // A page's push buttons are its captions with an icon, but for
            // the fix buttons above.
            let page = DialogCaptions::new(page.dialog(), &self.strings);
            for (id, _) in &page.glyphs {
                if self.clear_glyph(*id).is_none()
                    && let Some((_, caption)) = page.items.iter().find(|(item, _)| item == id)
                {
                    captions.push((caption.clone(), icons));
                }
            }
        }
        captions
    }

    fn clear_glyph(&self, id: u16) -> Option<Glyph> {
        (is(id, ControlId::SettingsClearSearch)
            || SHOW_SHORTCUTS
                .into_iter()
                .any(|(_, _, clear)| is(id, clear)))
        .then(|| captions::button_glyph("button.clear"))
        .flatten()
    }

    fn empty_text(&self, id: u16) -> Option<String> {
        if is(id, ControlId::VisibilityList) {
            return Some(match &self.draft.borrow().edited.see_every_file {
                Ok(_) => self.strings.text("settings_visibility.empty").to_owned(),
                Err(reason) => self
                    .strings
                    .format("settings_visibility.unreadable", &[reason.as_str()]),
            });
        }
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
    /// The shortcut Edit… changes; `None` for Add….
    replaced: Option<Keystroke>,
    balloon: OnceCell<Balloon>,
    chosen: Cell<Option<Keystroke>>,
}

impl<'a> CommandShortcut<'a> {
    fn new(strings: &'a Strings, replaced: Option<Keystroke>) -> CommandShortcut<'a> {
        CommandShortcut {
            strings,
            replaced,
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
            hotkey.take_any_key();
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
            // A key that Commander mode keeps is allowed, and the balloon
            // says so only when the person picks one.
            DialogMessage::Edited { .. } => {
                if let Some(balloon) = self.balloon.get() {
                    balloon.hide();
                }
                if let Some(hotkey) = Self::hotkey(dialog)
                    && let HotkeyValue::Keys(keystroke) = hotkey.value()
                    && kept_by_commander(keystroke)
                    && let Some(balloon) = self.balloon.get()
                {
                    balloon.show(
                        hotkey.hwnd(),
                        self.strings.text("settings_keyboard.commander_keeps"),
                    );
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
                    HotkeyValue::Keys(keystroke) => match CommandShortcuts::check(keystroke) {
                        Ok(()) => {
                            self.chosen.set(Some(keystroke));
                            dialog::end(dialog, DialogEnd::Ok);
                        }
                        Err(refusal) => {
                            self.refuse(dialog, &refusal.message(self.strings, keystroke));
                        }
                    },
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}
