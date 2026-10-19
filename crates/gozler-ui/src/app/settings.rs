//! The Settings dialog and its command shortcut dialog (DESIGN.md:
//! Settings dialog). The dialog edits its own copy of the
//! settings, a [`SettingsDraft`], and OK or Apply gives it to the main
//! window, so no model borrow is held while its modal loop runs (DESIGN.md:
//! window rule 1). Each page is a child dialog of its own template, whose
//! controls this dialog's procedure handles.

use super::main_window::MainWindow;
use crate::core::captions::{self, DialogCaptions, DialogItem, IndexCaptions, failure_text};
use crate::core::commands::{CommandId, Keystroke};
use crate::core::ids::{ControlId, DialogId};
use crate::core::open::{browse_command, total_commander_command};
use crate::core::settings::{SettingsDraft, SettingsPage, SettingsValues};
use crate::core::shortcuts::{
    CommandRow, CommandShortcuts, ShowShortcut, command_place, command_rows, show_refusal_message,
};
use crate::core::text::{LANGUAGES, LanguageChoice, Strings, without_access_key};
use crate::win::controls::{
    Balloon, CheckBox, DropDownList, Edit, HotkeyControl, HotkeyValue, ListBox, ReportList,
};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::gdi::Font;
use crate::win::{self, Hwnd};
use std::cell::{Cell, OnceCell, RefCell};

/// The Index page's timer, which shows the service's latest state.
const INDEX_TIMER: usize = 1;
const INDEX_REFRESH_MS: u32 = 2_000;

/// The page's heading, which shows its name in semibold.
fn heading(page: SettingsPage) -> ControlId {
    match page {
        SettingsPage::General => ControlId::GeneralHeading,
        SettingsPage::Search => ControlId::SearchHeading,
        SettingsPage::FileExplorer => ControlId::FileExplorerHeading,
        SettingsPage::Keyboard => ControlId::KeyboardHeading,
        SettingsPage::Index => ControlId::IndexHeading,
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
    (
        SettingsPage::FileExplorer,
        ControlId::FileExplorerSearchWith,
        |v| &mut v.search_with_gozler,
    ),
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
        ControlId::FileExplorerOpenFolders,
        ControlId::FileExplorerOpenFoldersBrowse,
        "settings_file_explorer.cue_explorer",
        |v| &mut v.choices.open_folders,
    ),
    (
        ControlId::FileExplorerOpenFiles,
        ControlId::FileExplorerOpenFilesBrowse,
        "settings_file_explorer.cue_default_program",
        |v| &mut v.choices.open_files,
    ),
    (
        ControlId::FileExplorerOpenLocation,
        ControlId::FileExplorerOpenLocationBrowse,
        "settings_file_explorer.cue_explorer",
        |v| &mut v.choices.open_file_location,
    ),
];

/// Each page's Restore defaults.
const RESTORE: [(SettingsPage, ControlId); 4] = [
    (SettingsPage::General, ControlId::GeneralRestore),
    (SettingsPage::Search, ControlId::SearchRestore),
    (SettingsPage::FileExplorer, ControlId::FileExplorerRestore),
    (SettingsPage::Keyboard, ControlId::KeyboardRestore),
];

fn is(id: u16, control: ControlId) -> bool {
    id == control.id()
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
    /// Whether the dialog is setting its controls, whose notifications then
    /// change nothing.
    filling: Cell<bool>,
    balloon: OnceCell<Balloon>,
    /// The pages' font and its bold form, for the captions that the search
    /// finds; they outlive the dialog's controls.
    highlight: OnceCell<(Font, Font)>,
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
            filling: Cell::new(false),
            balloon: OnceCell::new(),
            highlight: OnceCell::new(),
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
            SettingsPage::General | SettingsPage::Search | SettingsPage::FileExplorer => {
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
                if page == SettingsPage::FileExplorer {
                    for (control, _, _, command) in OPEN_COMMANDS {
                        if let Some(edit) = self.control(page, control) {
                            edit.set_text(command(&mut values));
                        }
                    }
                }
            }
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
            SettingsPage::Index => {}
        });
        self.show_closed_note();
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
    /// (DESIGN.md: Dialogs), a refused shortcut included.
    fn update_apply(&self, dialog: Hwnd) {
        let changed = self.draft.borrow().can_apply() || self.read_shortcut().is_err();
        if let Some(apply) = dialog::item(dialog, ControlId::SettingsApply.id()) {
            apply.set_enabled(changed);
        }
    }

    /// Shows `page`, or no page, and has the main window follow the service
    /// while the Index page shows (DESIGN.md: Settings dialog).
    fn show_page(&self, dialog: Hwnd, page: Option<SettingsPage>) {
        let previous = self.shown.replace(page);
        self.fill_page_list(dialog);
        if previous == page {
            return;
        }
        let pages = self.pages.borrow().clone();
        for (each, hwnd) in SettingsPage::ALL.into_iter().zip(pages) {
            hwnd.set_visible(Some(each) == page);
        }
        if page == Some(SettingsPage::Index) {
            self.main.show_index_page(self.window, true);
            dialog.set_timer(INDEX_TIMER, INDEX_REFRESH_MS);
            self.refresh_index();
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
        let (labels, index) = {
            let listed = self.listed.borrow();
            let shown = self.shown.get();
            let labels: Vec<&str> = listed
                .iter()
                .map(|page| self.strings.text(page.label()))
                .collect();
            (labels, listed.iter().position(|page| Some(*page) == shown))
        };
        self.filling(|| list.set_items(&labels, index.unwrap_or(0)));
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

    /// Shows the service's latest state on the Index page.
    fn refresh_index(&self) {
        let captions = self.main.index_captions();
        if self.index.borrow().as_ref() == Some(&captions) {
            return;
        }
        if let Some(list) = self
            .page(SettingsPage::Index)
            .and_then(|page| ReportList::of_dialog(page, ControlId::IndexDrives.id()))
        {
            list.set_rows(&captions.drives, None);
        }
        if let Some(service) = self.control(SettingsPage::Index, ControlId::IndexService) {
            service.set_text(&captions.service);
        }
        self.index.replace(Some(captions));
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
                let refill = applied.values != edited;
                self.draft.borrow_mut().applied(applied.values);
                if refill {
                    for page in SettingsPage::ALL {
                        self.fill(page);
                    }
                }
                let title = self.strings.text("window.title");
                for failure in &applied.failures {
                    win::dialog::error_box(Some(dialog), failure, title);
                }
                self.update_apply(dialog);
                true
            }
        }
    }

    /// Writes the program that the person chooses into the open command
    /// box `edit`, as `"<program>" "%1"`.
    fn browse(&self, dialog: Hwnd, edit: ControlId) {
        let title = self.strings.text("settings_file_explorer.browse_title");
        let programs = self.strings.text("settings_file_explorer.programs");
        if let Some(program) = win::shell::pick_program(dialog, title, programs)
            && let Some(edit) = self.control(SettingsPage::FileExplorer, edit)
        {
            // EN_CHANGE then puts it in the draft.
            edit.set_text(&browse_command(&program));
        }
    }

    /// Use Total Commander's answer, which the main window holds.
    fn total_commander_found(&self) {
        let Some(found) = self.main.take_total_commander() else {
            return;
        };
        let button = self.control(
            SettingsPage::FileExplorer,
            ControlId::FileExplorerTotalCommander,
        );
        let refusal = match found {
            Ok(Some(path)) => {
                let command = total_commander_command(&path);
                for control in [
                    ControlId::FileExplorerOpenFolders,
                    ControlId::FileExplorerOpenFiles,
                ] {
                    if let Some(edit) = self.control(SettingsPage::FileExplorer, control) {
                        edit.set_text(&command);
                    }
                }
                return;
            }
            Ok(None) => self
                .strings
                .text("settings_file_explorer.no_total_commander")
                .to_owned(),
            Err(failure) => failure_text(&self.strings, &failure),
        };
        if let (Some(button), Some(balloon)) = (button, self.balloon.get()) {
            balloon.show(button, &refusal);
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
        } else if let Some((control, _, _, command)) = OPEN_COMMANDS
            .into_iter()
            .find(|(control, ..)| is(id, *control))
        {
            let text = self
                .control(SettingsPage::FileExplorer, control)
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
        } else if let Some((edit, ..)) = OPEN_COMMANDS.into_iter().find(|(_, b, ..)| is(id, *b)) {
            self.browse(dialog, edit);
        } else if let Some((page, _)) = RESTORE.into_iter().find(|(_, c)| is(id, *c)) {
            self.draft.borrow_mut().edited.restore_defaults(page);
            self.fill(page);
        } else if is(id, ControlId::FileExplorerTotalCommander) {
            self.main.find_total_commander(dialog);
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
                && let Ok(font) = Font::semibold_of(heading)
            {
                heading.set_font(&font);
                fonts.push(font);
            }
            pages.push(hwnd);
        }
        self.pages.replace(pages);
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
            // As the Language menu lists them, each language by its own
            // name (DESIGN.md: Language).
            let names = LANGUAGES
                .iter()
                .map(|language| without_access_key(language.name));
            let items: Vec<String> =
                std::iter::once(text("settings_general.windows_default").to_owned())
                    .chain(names)
                    .collect();
            list.set_items(&items, 0);
        }
        if let Some(page) = self.page(SettingsPage::FileExplorer) {
            for (control, _, cue, _) in OPEN_COMMANDS {
                if let Some(edit) = Edit::of_dialog(page, control.id()) {
                    edit.set_cue(text(cue));
                }
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
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        for page in SettingsPage::ALL {
            self.fill(page);
        }
        if let Some(search) = Edit::of_dialog(dialog, ControlId::SettingsSearch.id()) {
            search.set_cue(text("settings.search_cue"));
        }
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
            DialogMessage::Command { id } if is(id, ControlId::SettingsApply) => {
                self.apply(dialog);
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
            DialogMessage::Timer { id: INDEX_TIMER } => {
                self.refresh_index();
                return true;
            }
            DialogMessage::Reply => {
                self.total_commander_found();
                return true;
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::Timer { .. }
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
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Other => false,
        }
    }
}
