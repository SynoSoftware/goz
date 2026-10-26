//! `ui.json`: the person's settings and the window's state, which only the
//! window writes (DESIGN.md: Where settings are saved). The keys are the file
//! format, so renaming one needs a migration. A value that is missing or
//! damaged falls back to its default, and every other value still loads.

use super::commands::{CommandId, MenuItem, row_menu};
use super::dates::DateFormat;
use super::glyph::Glyph;
use super::ids::DialogId;
use super::layout::{Dip, Panes};
use super::model::Column;
use super::open::total_commander_command;
use super::panel::SavedPanels;
use super::query::{Filters, Matching};
use super::shortcuts::{CommandShortcuts, ShowShortcut};
use super::speed::{OutOfRange, Speed, SpeedNumber};
use super::text::{LanguageChoice, SizeFormat};
use gozler_core::include::IndexSettings;
use gozler_core::types::SortSpec;
use gozler_fileops::{DeleteWorkers, Policy, TransferLimit};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// The settings that Settings and the Language menu change.
    #[serde(rename = "settings", default, deserialize_with = "or_default")]
    pub(crate) choices: Choices,
    /// What the window remembers about itself between runs.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) window: WindowState,
}

/// The person's settings of Settings > General, Search, Context menu,
/// Keyboard and File operations (DESIGN.md: Settings dialog). Start at sign-in is not here: the
/// registry holds it, because Task Manager and the uninstaller change it
/// there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Choices {
    /// A language tag, such as `de`; absent for Windows default.
    #[serde(
        default,
        deserialize_with = "language",
        serialize_with = "serialize_language",
        skip_serializing_if = "is_windows_default"
    )]
    pub(crate) language: LanguageChoice,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) check_for_updates: bool,
    /// Closing the window hides it, and Gozler keeps running in the
    /// notification area (DESIGN.md: Window lifetime).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) keep_running: bool,
    /// Starting Gozler again brings the open window forward; off, it opens
    /// another window (DESIGN.md: Window lifetime).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) one_window: bool,
    /// Icons before the text of menu items, buttons, list items, group
    /// captions and the status bar's totals (DESIGN.md: Look).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) show_icons: bool,
    /// The status bar's memory group (DESIGN.md: Status bar).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) show_memory: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) size_format: SizeFormat,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) date_format: DateFormat,
    /// The open commands (DESIGN.md: The Context menu page).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_folders: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_files: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_file_location: String,
    /// Which items a row's context menu shows.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) context_menu: ContextMenu,
    /// Off: only `Enter` runs the search.
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) search_as_you_type: bool,
    /// Off: an empty search shows "Type to search." (DESIGN.md: S6).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) show_all_when_empty: bool,
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) select_on_click: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) select_on_activate: bool,
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) show_shortcut: Option<ShowShortcut>,
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) commander_shortcut: Option<ShowShortcut>,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) shortcuts: CommandShortcuts,
    /// The speed settings that copies, moves and deletes start with
    /// (DESIGN.md: The File operations page).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) speed: Speed,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) transfers_per_device: TransferLimit,
    #[serde(default, deserialize_with = "copy_policy")]
    pub(crate) copy_policy: Policy,
}

fn copy_policy<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Policy, D::Error> {
    let policy = or_default(deserializer)?;
    Ok(if Policy::ALL.contains(&policy) {
        policy
    } else {
        Policy::default()
    })
}

impl Choices {
    pub(crate) fn show_shortcut(&self, mode: Mode) -> Option<ShowShortcut> {
        match mode {
            Mode::Search => self.show_shortcut,
            Mode::TwoPanels => self.commander_shortcut,
        }
    }
}

impl Default for Choices {
    fn default() -> Choices {
        Choices {
            language: LanguageChoice::Windows,
            check_for_updates: false,
            keep_running: on(),
            one_window: on(),
            show_icons: on(),
            show_memory: false,
            size_format: SizeFormat::Kb,
            date_format: DateFormat::Full,
            open_folders: String::new(),
            open_files: String::new(),
            open_file_location: String::new(),
            context_menu: ContextMenu::default(),
            search_as_you_type: on(),
            show_all_when_empty: on(),
            select_on_click: on(),
            select_on_activate: false,
            show_shortcut: None,
            commander_shortcut: None,
            shortcuts: CommandShortcuts::default(),
            speed: Speed::default(),
            transfers_per_device: TransferLimit::default(),
            copy_policy: Policy::default(),
        }
    }
}

/// What the Settings dialog edits: the choices, the settings whose one copy
/// is in the registry, read when the dialog opens, and the service's
/// settings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsValues {
    pub(crate) choices: Choices,
    pub(crate) start_at_sign_in: bool,
    /// The settings the service applies, with an entry for each connected
    /// drive; `None` while the service gives no status, and the Include page
    /// then changes nothing.
    pub(crate) include: Option<IndexSettings>,
    /// The users and groups who see every file; Windows' words for why the
    /// list could not be read, and the page then changes nothing, because
    /// saving a list it never read would replace the stored one.
    pub(crate) see_every_file: Result<SeeEveryFile, String>,
}

/// The SID string of Everyone.
const EVERYONE: &str = "S-1-1-0";

/// The users and groups who see every file (spec `V16`): the lines of the
/// stored list, SID strings, in its order. A line that is not a SID, which
/// only a hand edit writes, stays as it is, because the service skips it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeeEveryFile(Vec<String>);

impl SeeEveryFile {
    pub(crate) fn new(lines: Vec<String>) -> SeeEveryFile {
        SeeEveryFile(lines)
    }

    pub(crate) fn sids(&self) -> &[String] {
        &self.0
    }

    /// Whether the list holds Everyone, which turns the filter off; the
    /// "Every user sees every file" check box shows it.
    pub(crate) fn everyone(&self) -> bool {
        self.0.iter().any(|sid| sid.eq_ignore_ascii_case(EVERYONE))
    }

    /// The check box adds or removes Everyone and stores nothing of its own.
    pub(crate) fn set_everyone(&mut self, on: bool) {
        if on {
            self.add(EVERYONE.to_owned());
        } else {
            self.0.retain(|sid| !sid.eq_ignore_ascii_case(EVERYONE));
        }
    }

    /// Adds `sid` at the end, unless the list holds it.
    pub(crate) fn add(&mut self, sid: String) {
        if !self.0.iter().any(|each| each.eq_ignore_ascii_case(&sid)) {
            self.0.push(sid);
        }
    }

    pub(crate) fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }
}

/// The service's settings that differ from the applied ones, which OK and
/// Apply save through one copy of the window that runs as administrator;
/// `None` for each one that did not change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ServiceChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) include: Option<IndexSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) see_every_file: Option<SeeEveryFile>,
}

impl ServiceChanges {
    /// Whether the service must read its settings again: it reads the list
    /// for each search, but the Include settings only when told.
    pub(crate) fn tells_service(&self) -> bool {
        self.include.is_some()
    }
}

/// A page of the Settings dialog, in the page list's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    General,
    Search,
    ContextMenu,
    Keyboard,
    FileOperations,
    Performance,
    /// The service's index, which the page shows and does not change yet
    /// (DESIGN.md: Settings dialog).
    Index,
    /// What the service indexes (DESIGN.md: The Include page).
    Include,
    /// Who sees every file (spec `V14`).
    Visibility,
}

impl SettingsPage {
    pub(crate) const ALL: [SettingsPage; 9] = [
        SettingsPage::General,
        SettingsPage::Search,
        SettingsPage::ContextMenu,
        SettingsPage::Keyboard,
        SettingsPage::FileOperations,
        SettingsPage::Performance,
        SettingsPage::Index,
        SettingsPage::Include,
        SettingsPage::Visibility,
    ];

    /// Its child-dialog template.
    pub(crate) fn dialog(self) -> DialogId {
        match self {
            SettingsPage::General => DialogId::SettingsGeneral,
            SettingsPage::Search => DialogId::SettingsSearch,
            SettingsPage::ContextMenu => DialogId::SettingsContextMenu,
            SettingsPage::Keyboard => DialogId::SettingsKeyboard,
            SettingsPage::FileOperations => DialogId::SettingsFileOperations,
            SettingsPage::Performance => DialogId::SettingsPerformance,
            SettingsPage::Index => DialogId::SettingsIndex,
            SettingsPage::Include => DialogId::SettingsInclude,
            SettingsPage::Visibility => DialogId::SettingsVisibility,
        }
    }

    /// Its name in the page list and its heading.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "settings.general",
            SettingsPage::Search => "settings.search",
            SettingsPage::ContextMenu => "settings.context_menu",
            SettingsPage::Keyboard => "settings.keyboard",
            SettingsPage::FileOperations => "settings.file_operations",
            SettingsPage::Performance => "settings.performance",
            SettingsPage::Index => "settings.index",
            SettingsPage::Include => "settings.include",
            SettingsPage::Visibility => "settings.visibility",
        }
    }

    /// The icon before its name in the page list.
    pub(crate) fn glyph(self) -> Glyph {
        match self {
            SettingsPage::General => Glyph::Settings,
            SettingsPage::Search => Glyph::Search,
            SettingsPage::ContextMenu => Glyph::BulletedList,
            SettingsPage::Keyboard => Glyph::KeyboardClassic,
            SettingsPage::FileOperations => Glyph::Copy,
            SettingsPage::Performance => Glyph::SpeedHigh,
            SettingsPage::Index => Glyph::HardDrive,
            SettingsPage::Include => Glyph::CheckList,
            SettingsPage::Visibility => Glyph::People,
        }
    }
}

impl SettingsValues {
    /// The service's settings that these values change from `applied`, or
    /// `None` when they change none.
    pub(crate) fn service_changes(&self, applied: &SettingsValues) -> Option<ServiceChanges> {
        let include = (self.include != applied.include)
            .then(|| self.include.clone())
            .flatten();
        let see_every_file = match (&self.see_every_file, &applied.see_every_file) {
            (Ok(list), Ok(applied)) if list != applied => Some(list.clone()),
            _ => None,
        };
        (include.is_some() || see_every_file.is_some()).then_some(ServiceChanges {
            include,
            see_every_file,
        })
    }

    /// Sets `page`'s settings to their defaults, which Apply or OK then
    /// applies (DESIGN.md: Settings dialog).
    pub(crate) fn restore_defaults(&mut self, page: SettingsPage) {
        let defaults = Choices::default();
        let choices = &mut self.choices;
        match page {
            SettingsPage::General => {
                choices.language = defaults.language;
                choices.check_for_updates = defaults.check_for_updates;
                choices.keep_running = defaults.keep_running;
                choices.one_window = defaults.one_window;
                choices.show_icons = defaults.show_icons;
                choices.show_memory = defaults.show_memory;
                choices.size_format = defaults.size_format;
                choices.date_format = defaults.date_format;
                self.start_at_sign_in = false;
            }
            SettingsPage::Search => {
                choices.search_as_you_type = defaults.search_as_you_type;
                choices.show_all_when_empty = defaults.show_all_when_empty;
                choices.select_on_click = defaults.select_on_click;
                choices.select_on_activate = defaults.select_on_activate;
            }
            SettingsPage::ContextMenu => {
                choices.open_folders = defaults.open_folders;
                choices.open_files = defaults.open_files;
                choices.open_file_location = defaults.open_file_location;
                choices.context_menu = defaults.context_menu;
            }
            SettingsPage::Keyboard => {
                choices.show_shortcut = defaults.show_shortcut;
                choices.commander_shortcut = defaults.commander_shortcut;
                choices.shortcuts = defaults.shortcuts;
            }
            SettingsPage::FileOperations => {
                choices.copy_policy = defaults.copy_policy;
                choices.transfers_per_device = defaults.transfers_per_device;
            }
            SettingsPage::Performance => {
                choices.speed = defaults.speed;
            }
            SettingsPage::Index | SettingsPage::Visibility => {}
            SettingsPage::Include => {
                if let Some(include) = &mut self.include {
                    // Every drive the page knows, checked, as the defaults
                    // index every drive.
                    let drives: Vec<(String, String)> = include
                        .drives
                        .iter()
                        .map(|d| (d.volume.clone(), d.name.clone()))
                        .collect();
                    // Dates created is the Drives and service page's.
                    *include = IndexSettings {
                        dates_created: include.dates_created,
                        ..IndexSettings::default().with_drives(&drives)
                    };
                }
            }
        }
    }
}

/// An item of a row's context menu, as Settings > Context menu lists it:
/// Open is two, for folders and for files (DESIGN.md: The Context menu
/// page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextItem {
    OpenFolders,
    OpenFiles,
    OpenFileLocation,
    ShowInCommander,
    Explore,
    Copy,
    CopyName,
    CopyFolder,
    CopyAsPath,
    SearchInThisFolder,
    Properties,
}

impl ContextItem {
    /// Every item, in the row menu's order.
    pub(crate) fn all() -> Vec<ContextItem> {
        row_menu()
            .into_iter()
            .flatten()
            .filter_map(|item| match item {
                MenuItem::Run(command) => Some(command),
                _ => None,
            })
            .flat_map(|command| match command {
                CommandId::Open => [Some(ContextItem::OpenFolders), Some(ContextItem::OpenFiles)],
                command => [ContextItem::of(command, false), None],
            })
            .flatten()
            .collect()
    }

    /// The item of the row menu that runs `command`, on a folder when
    /// `folder`; `None` for a command the row menu does not hold.
    pub(crate) fn of(command: CommandId, folder: bool) -> Option<ContextItem> {
        Some(match command {
            CommandId::Open if folder => ContextItem::OpenFolders,
            CommandId::Open => ContextItem::OpenFiles,
            CommandId::OpenFileLocation => ContextItem::OpenFileLocation,
            CommandId::ShowInCommander => ContextItem::ShowInCommander,
            CommandId::Explore => ContextItem::Explore,
            CommandId::Copy => ContextItem::Copy,
            CommandId::CopyAsPath => ContextItem::CopyAsPath,
            CommandId::CopyName => ContextItem::CopyName,
            CommandId::CopyFolder => ContextItem::CopyFolder,
            CommandId::SearchInThisFolder => ContextItem::SearchInThisFolder,
            CommandId::Properties => ContextItem::Properties,
            _ => return None,
        })
    }

    /// The command the item runs.
    pub(crate) fn command(self) -> CommandId {
        match self {
            ContextItem::OpenFolders | ContextItem::OpenFiles => CommandId::Open,
            ContextItem::OpenFileLocation => CommandId::OpenFileLocation,
            ContextItem::ShowInCommander => CommandId::ShowInCommander,
            ContextItem::Explore => CommandId::Explore,
            ContextItem::Copy => CommandId::Copy,
            ContextItem::CopyAsPath => CommandId::CopyAsPath,
            ContextItem::CopyName => CommandId::CopyName,
            ContextItem::CopyFolder => CommandId::CopyFolder,
            ContextItem::SearchInThisFolder => CommandId::SearchInThisFolder,
            ContextItem::Properties => CommandId::Properties,
        }
    }

    /// The catalogue key of its name on the page: its command's, and Open's
    /// own for folders and for files.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ContextItem::OpenFolders => "settings_context_menu.item_open_folders",
            ContextItem::OpenFiles => "settings_context_menu.item_open_files",
            item => item.command().row().label,
        }
    }
}

/// When a row's context menu shows an item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemVisibility {
    #[default]
    Shown,
    /// Shown when Shift is down as the menu opens.
    WithShift,
    Hidden,
}

impl ItemVisibility {
    /// Whether the menu shows the item, with Shift down when `shift`.
    pub(crate) fn shows(self, shift: bool) -> bool {
        match self {
            ItemVisibility::Shown => true,
            ItemVisibility::WithShift => shift,
            ItemVisibility::Hidden => false,
        }
    }
}

/// When a row's context menu shows each item. A hidden item still runs
/// from its shortcut, the menu bar and the inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextMenu {
    #[serde(default, deserialize_with = "or_default")]
    open_folders: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    open_files: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    open_file_location: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    show_in_commander: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    explore: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    copy: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    copy_path: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    copy_name: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    copy_folder: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    search_in_this_folder: ItemVisibility,
    #[serde(default, deserialize_with = "or_default")]
    properties: ItemVisibility,
}

impl ContextMenu {
    pub(crate) fn visibility(mut self, item: ContextItem) -> ItemVisibility {
        *self.visibility_mut(item)
    }

    pub(crate) fn visibility_mut(&mut self, item: ContextItem) -> &mut ItemVisibility {
        match item {
            ContextItem::OpenFolders => &mut self.open_folders,
            ContextItem::OpenFiles => &mut self.open_files,
            ContextItem::OpenFileLocation => &mut self.open_file_location,
            ContextItem::ShowInCommander => &mut self.show_in_commander,
            ContextItem::Explore => &mut self.explore,
            ContextItem::Copy => &mut self.copy,
            ContextItem::CopyAsPath => &mut self.copy_path,
            ContextItem::CopyName => &mut self.copy_name,
            ContextItem::CopyFolder => &mut self.copy_folder,
            ContextItem::SearchInThisFolder => &mut self.search_in_this_folder,
            ContextItem::Properties => &mut self.properties,
        }
    }
}

/// The values the dialog shows and the values Apply last applied, so Apply
/// is enabled exactly while they differ (DESIGN.md: Dialogs).
pub(crate) struct SettingsDraft {
    applied: SettingsValues,
    pub(crate) edited: SettingsValues,
    /// The File operations page's number boxes whose text their range
    /// refuses, with that text, which the edited values cannot hold.
    refused: Vec<(SpeedNumber, String)>,
}

impl SettingsDraft {
    pub(crate) fn new(values: SettingsValues) -> SettingsDraft {
        SettingsDraft {
            applied: values.clone(),
            edited: values,
            refused: Vec::new(),
        }
    }

    /// Whether Apply is enabled: a value differs from what was applied, or
    /// a number box holds text that Apply then refuses.
    pub(crate) fn can_apply(&self) -> bool {
        self.edited != self.applied || self.refused().is_some()
    }

    /// Whether OK and Apply show the administrator shield: a service setting
    /// differs from the applied one, and saving it asks Windows for
    /// permission.
    pub(crate) fn shielded(&self) -> bool {
        self.edited.service_changes(&self.applied).is_some()
    }

    pub(crate) fn choices_applied(&mut self, choices: Choices) {
        self.applied.choices = choices;
    }

    /// A delayed acknowledgment may replace its submitted edit, but never a
    /// newer one. The answer says whether the check box needs to change.
    pub(crate) fn sign_in_saved(&mut self, submitted: bool, in_effect: bool) -> bool {
        self.applied.start_at_sign_in = in_effect;
        if self.edited.start_at_sign_in == submitted {
            let changed = self.edited.start_at_sign_in != in_effect;
            self.edited.start_at_sign_in = in_effect;
            changed
        } else {
            false
        }
    }

    pub(crate) fn last_applied(&self) -> &SettingsValues {
        &self.applied
    }

    /// Records each service setting of `saved` as applied.
    pub(crate) fn service_saved(&mut self, saved: ServiceChanges) {
        if let Some(include) = saved.include {
            self.applied.include = Some(include);
        }
        if let Some(list) = saved.see_every_file {
            self.applied.see_every_file = Ok(list);
        }
    }

    /// Records the text typed in `number`'s box, which makes the speed
    /// Custom: a number in its range becomes the value, and any other text
    /// is kept for Apply to refuse.
    pub(crate) fn type_number(&mut self, number: SpeedNumber, text: &str) {
        let mut taken = false;
        self.edited.choices.speed.change(|settings| {
            taken = number.set_text(settings, text).is_ok();
        });
        self.refused.retain(|(refused, _)| *refused != number);
        if !taken {
            self.refused.push((number, text.to_owned()));
        }
    }

    /// The first number box, in the page's order, whose text its range
    /// refuses, with that range. The delete workers' box is off while one
    /// per logical processor is on, so its text is not checked then.
    pub(crate) fn refused(&self) -> Option<(SpeedNumber, OutOfRange)> {
        let settings = self.edited.choices.speed.settings();
        let per_processor = settings.delete_workers == DeleteWorkers::PerProcessor;
        SpeedNumber::ALL
            .into_iter()
            .filter(|number| !(per_processor && *number == SpeedNumber::DeleteWorkers))
            .find_map(|number| {
                let (_, text) = self.refused.iter().find(|(r, _)| *r == number)?;
                let range = number.set_text(&mut settings.clone(), text).err()?;
                Some((number, range))
            })
    }

    /// The File operations page shows the edited values again, so no box
    /// holds refused text.
    pub(crate) fn speed_shown(&mut self) {
        self.refused.clear();
    }

    /// The Use Total Commander link: both open commands open Total Commander
    /// at `path`. Answers whether they changed; when they already did, the
    /// dialog says so, because the link changed nothing (DESIGN.md: The
    /// Context menu page).
    pub(crate) fn use_total_commander(&mut self, path: &str) -> bool {
        let command = total_commander_command(path);
        let choices = &mut self.edited.choices;
        if choices.open_folders == command && choices.open_files == command {
            return false;
        }
        choices.open_folders.clone_from(&command);
        choices.open_files = command;
        true
    }
}

/// What the window shows: Search's results list, or Norton Commander's two
/// panels (DESIGN.md: Two panels). The window opens in the mode it closed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    #[default]
    Search,
    TwoPanels,
}

impl Mode {
    pub(crate) const ALL: [Mode; 2] = [Mode::Search, Mode::TwoPanels];
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WindowState {
    #[serde(
        default,
        deserialize_with = "placement",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) placement: Option<Placement>,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) mode: Mode,
    #[serde(default = "matching", deserialize_with = "matching_or_default")]
    pub(crate) matching: Matching,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) sort: SortSpec,
    /// The shown columns in the order the list shows them; empty for the
    /// default columns.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) columns: Vec<SavedColumn>,
    /// Whether the filter pane shows (DESIGN.md: C13).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) filter_pane: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) filters: Filters,
    /// Whether the inspector shows (DESIGN.md: C14).
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) inspector: bool,
    /// The height the person dragged the splitter to; the layout keeps it
    /// between the inspector's and the list's minimum heights.
    #[serde(
        default = "inspector_height",
        deserialize_with = "inspector_height_or_default"
    )]
    pub(crate) inspector_height: Dip,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) inspector_maximized: bool,
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) transfers_flipped: bool,
    /// Whether the notification of the first hide to the notification area
    /// has shown, which it does once (DESIGN.md: Window lifetime).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) tray_notice_shown: bool,
    /// When Gozler last asked for the latest release, in seconds since
    /// 1970, so it asks at most once a day.
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) last_update_check: Option<i64>,
    /// The folder last chosen for Copy to folder… or Move to folder…,
    /// where their folder picker opens next (DESIGN.md: File operations).
    #[serde(
        default,
        deserialize_with = "or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) destination_folder: Option<String>,
    /// Each panel's folder, sort and recent folders, and which panel is
    /// active (DESIGN.md: A panel).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) panels: SavedPanels,
    /// The Viewer's size and place, and Wrap long lines.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) viewer: ViewerState,
}

impl Default for WindowState {
    fn default() -> WindowState {
        WindowState {
            placement: None,
            mode: Mode::Search,
            matching: matching(),
            sort: SortSpec::default(),
            columns: Vec::new(),
            filter_pane: on(),
            filters: Filters::default(),
            inspector: on(),
            inspector_height: inspector_height(),
            inspector_maximized: false,
            transfers_flipped: true,
            tray_notice_shown: false,
            last_update_check: None,
            destination_folder: None,
            panels: SavedPanels::default(),
            viewer: ViewerState::default(),
        }
    }
}

/// A shown column. Its width is the live list's, which fills the list's
/// width, so it is not saved, and the width a file from an earlier version
/// holds is ignored (DESIGN.md: Results list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedColumn {
    pub(crate) column: Column,
}

/// The window's restored rectangle, in the workspace coordinates that
/// `GetWindowPlacement` gives, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Placement {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
    pub(crate) maximized: bool,
}

/// What the Viewer remembers (DESIGN.md: The Viewer): the size and place of the
/// Viewer that was moved or closed last, and Wrap long lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ViewerState {
    #[serde(
        default,
        deserialize_with = "placement",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) placement: Option<Placement>,
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) wrap: bool,
}

impl Default for ViewerState {
    fn default() -> ViewerState {
        ViewerState {
            placement: None,
            wrap: on(),
        }
    }
}

impl Settings {
    pub(crate) fn parse(text: &str) -> Settings {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub(crate) fn changes(&self, saved: &Settings) -> Changes {
        let now = serde_json::to_value(self).expect("Settings serializes");
        let before = serde_json::to_value(saved).expect("Settings serializes");
        let mut keys = BTreeMap::new();
        for (group, values) in now.as_object().expect("Settings is an object") {
            let before = before.get(group).and_then(Value::as_object);
            let values = values.as_object().expect("settings groups are objects");
            let changed: BTreeMap<_, _> = values
                .keys()
                .chain(before.into_iter().flat_map(Map::keys))
                .filter(|key| values.get(*key) != before.and_then(|values| values.get(*key)))
                .map(|key| (key.clone(), values.get(key).cloned()))
                .collect();
            if !changed.is_empty() {
                keys.insert(group.clone(), changed);
            }
        }
        Changes { full: now, keys }
    }
}

/// Pending changed-key intent, including removals. Coalescing retains keys even
/// if their newest value equals the value before the first pending change.
pub(crate) struct Changes {
    full: Value,
    keys: BTreeMap<String, BTreeMap<String, Option<Value>>>,
}

impl Changes {
    pub(crate) fn combine(mut self, newer: Changes) -> Changes {
        self.full = newer.full;
        for (group, keys) in newer.keys {
            self.keys.entry(group).or_default().extend(keys);
        }
        self
    }

    /// Applies only this window's changed keys to the latest file. An absent or
    /// invalid file gets the complete state, as at the first save of an install.
    pub(crate) fn apply(self, text: Option<&str>) -> String {
        let file = text
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(Value::is_object);
        let mut file = match file {
            Some(file) => file,
            None => {
                return serde_json::to_string_pretty(&self.full).expect("a JSON value serializes");
            }
        };
        let groups = file.as_object_mut().expect("the parsed file is an object");
        for (group, keys) in self.keys {
            let target = groups
                .entry(group)
                .or_insert_with(|| Value::Object(Map::new()));
            if !target.is_object() {
                *target = Value::Object(Map::new());
            }
            let target = target.as_object_mut().expect("the target is an object");
            for (key, value) in keys {
                match value {
                    Some(value) => target.insert(key, value),
                    None => target.remove(&key),
                };
            }
        }
        serde_json::to_string_pretty(&file).expect("a JSON value serializes")
    }
}

/// Reads a value, or its default when the value is damaged.
fn or_default<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// The default of a setting that is on until the person turns it off, such
/// as a pane that shows until it is hidden.
fn on() -> bool {
    true
}

fn on_or_default<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(or_default::<D, Option<bool>>(deserializer)?.unwrap_or_else(on))
}

/// The match controls a new install starts with: Match path on, because a
/// typed word then also finds what is in a folder of that name (DESIGN.md:
/// Search box).
fn matching() -> Matching {
    Matching {
        path: true,
        ..Matching::default()
    }
}

fn matching_or_default<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Matching, D::Error> {
    Ok(or_default::<D, Option<Matching>>(deserializer)?.unwrap_or_else(matching))
}

fn inspector_height() -> Dip {
    Panes::default().inspector_height
}

/// A height that is not positive, or taller than any screen, is damaged,
/// because the layout converts it to pixels, which a huge height overflows.
fn inspector_height_or_default<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Dip, D::Error> {
    Ok(or_default::<D, Option<Dip>>(deserializer)?
        .filter(|height| (1..=16_384).contains(&height.0))
        .unwrap_or_else(inspector_height))
}

/// A placement whose width or height is not a positive `i32` is damaged,
/// because the window computes its size from it; `GetWindowPlacement`
/// never gives one.
fn placement<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Placement>, D::Error> {
    Ok(
        or_default::<D, Option<Placement>>(deserializer)?.filter(|p| {
            let positive = |from: i32, to: i32| to.checked_sub(from).is_some_and(|n| n > 0);
            positive(p.left, p.right) && positive(p.top, p.bottom)
        }),
    )
}

fn language<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LanguageChoice, D::Error> {
    Ok(match or_default::<D, Option<String>>(deserializer)? {
        Some(tag) => LanguageChoice::Tag(tag),
        None => LanguageChoice::Windows,
    })
}

fn serialize_language<S: Serializer>(
    choice: &LanguageChoice,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match choice {
        LanguageChoice::Windows => serializer.serialize_none(),
        LanguageChoice::Tag(tag) => serializer.serialize_str(tag),
    }
}

fn is_windows_default(choice: &LanguageChoice) -> bool {
    *choice == LanguageChoice::Windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::commands::{CommandId, Key, Keystroke};
    use crate::core::panel::{SavedPanel, Side};
    use crate::core::query::{DateModified, DateRange, Day, Kind, SizeFilter, SizeWord};
    use crate::core::speed;
    use gozler_fileops::Count;

    fn values() -> SettingsValues {
        SettingsValues {
            choices: Choices::default(),
            start_at_sign_in: false,
            include: Some(IndexSettings::default()),
            see_every_file: Ok(SeeEveryFile::default()),
        }
    }

    /// A sign-in setting whose registry write failed keeps its earlier
    /// value, which the page shows again, while the Include settings,
    /// whose save as administrator the person declined, stay edited with
    /// the shield on Apply.
    #[test]
    fn a_failed_write_reverts_and_unsaved_include_keeps_the_shield() {
        let mut draft = SettingsDraft::new(values());
        draft.edited.start_at_sign_in = true;
        let include = IndexSettings {
            exclude_hidden: true,
            ..IndexSettings::default()
        };
        draft.edited.include = Some(include.clone());
        draft.choices_applied(draft.edited.choices.clone());
        assert!(draft.sign_in_saved(true, false));
        assert!(!draft.edited.start_at_sign_in);
        assert_eq!(draft.edited.include, Some(include));
        assert!(draft.shielded() && draft.can_apply());
    }

    #[test]
    fn a_delayed_sign_in_acknowledgment_preserves_newer_edits() {
        let mut draft = SettingsDraft::new(values());
        draft.edited.start_at_sign_in = true;
        draft.choices_applied(draft.edited.choices.clone());
        draft.edited.start_at_sign_in = false;
        draft.edited.choices.show_memory = true;
        draft.edited.choices.speed.change(|settings| {
            settings.delete_workers = DeleteWorkers::Fixed(Count::new(4).expect("in range"));
        });
        draft.type_number(SpeedNumber::DeleteWorkers, "0");
        assert!(!draft.sign_in_saved(true, true));
        assert!(draft.last_applied().start_at_sign_in);
        assert!(!draft.edited.start_at_sign_in);
        assert!(draft.edited.choices.show_memory);
        assert!(!draft.last_applied().choices.show_memory);
        assert_eq!(
            draft.refused().map(|(number, _)| number),
            Some(SpeedNumber::DeleteWorkers)
        );
        assert!(draft.can_apply());
    }

    /// A number box's text out of its range enables Apply, which refuses
    /// it, until the box is turned off: delete workers' while one per
    /// logical processor is on.
    #[test]
    fn a_turned_off_box_is_not_checked() {
        let mut draft = SettingsDraft::new(values());
        draft.edited.choices.speed.change(|settings| {
            settings.delete_workers = DeleteWorkers::Fixed(Count::new(4).expect("in range"));
        });
        draft.choices_applied(draft.edited.choices.clone());
        draft.type_number(SpeedNumber::DeleteWorkers, "0");
        assert_eq!(
            draft.refused().map(|(number, _)| number),
            Some(SpeedNumber::DeleteWorkers)
        );
        assert!(draft.can_apply());
        draft.edited.choices.speed.change(|settings| {
            speed::set_per_processor(settings, true);
        });
        assert_eq!(draft.refused(), None);
    }

    /// The Use Total Commander link sets both open commands, and says when
    /// they already held it, because it then changes nothing.
    #[test]
    fn the_total_commander_link_sets_both_open_commands() {
        let mut draft = SettingsDraft::new(values());
        let path = r"C:\totalcmd\TOTALCMD64.EXE";
        assert!(draft.use_total_commander(path));
        let command = total_commander_command(path);
        assert_eq!(draft.edited.choices.open_folders, command);
        assert_eq!(draft.edited.choices.open_files, command);
        assert!(!draft.use_total_commander(path));
    }

    /// A settings file that breaks a rule of Settings > Keyboard would give
    /// two commands one key, or a key the search box needs; the window
    /// opens with the defaults instead, and the other settings still load.
    #[test]
    fn a_damaged_settings_file_opens_with_the_defaults() {
        let text = r#"{ "settings": {
            "keep_running": "yes",
            "search_as_you_type": false,
            "show_shortcut": "Win+L",
            "shortcuts": { "copy": ["Ctrl+E"] }
        } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(
            settings.choices,
            Choices {
                search_as_you_type: false,
                ..Choices::default()
            }
        );
        // A key moved from one command to another is no conflict.
        let text = r#"{ "settings": { "shortcuts": {
            "find": ["Ctrl+F"], "copy": ["Ctrl+C", "Ctrl+E"]
        } } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(
            settings.choices.shortcuts.of(CommandId::Copy),
            [
                Keystroke::ctrl(Key::Letter(b'C')),
                Keystroke::ctrl(Key::Letter(b'E'))
            ]
        );
        assert_eq!(Settings::parse(""), Settings::default());
    }

    /// A new install starts with Match path on, while a person who turned it
    /// off keeps it off: a default read over the saved value would turn it
    /// on again at each start.
    #[test]
    fn match_path_starts_on_and_stays_off_once_turned_off() {
        let new_install: Settings = serde_json::from_str("{}").expect("an object");
        assert!(new_install.window.matching.path);
        let text = r#"{ "window": { "matching":
            { "mode": "start_of_words", "case": false, "path": false } } }"#;
        let turned_off: Settings = serde_json::from_str(text).expect("an object");
        assert!(!turned_off.window.matching.path);
    }

    /// Apply that stays enabled after Apply, or is disabled while a value
    /// differs, misleads about what OK and Cancel do (DESIGN.md: Dialogs).
    #[test]
    fn apply_follows_the_edited_values_and_restore_defaults_resets_one_page() {
        let values = SettingsValues {
            choices: Choices {
                search_as_you_type: false,
                open_files: "notepad %1".into(),
                ..Choices::default()
            },
            start_at_sign_in: true,
            include: None,
            see_every_file: Ok(SeeEveryFile::default()),
        };
        let mut draft = SettingsDraft::new(values.clone());
        assert!(!draft.can_apply());
        draft.edited.restore_defaults(SettingsPage::Search);
        assert!(draft.edited.choices.search_as_you_type);
        assert_eq!(
            draft.edited.choices.open_files, "notepad %1",
            "File Explorer keeps"
        );
        assert!(draft.edited.start_at_sign_in);
        assert!(draft.can_apply());
        draft.choices_applied(draft.edited.choices.clone());
        assert!(!draft.can_apply());
        draft.edited.choices.search_as_you_type = false;
        draft.edited.choices.search_as_you_type = true;
        assert!(!draft.can_apply(), "a value changed back is no change");
        draft.edited.restore_defaults(SettingsPage::General);
        assert!(!draft.edited.start_at_sign_in);
        assert_eq!(draft.edited.choices.open_files, "notepad %1");
        draft.edited.restore_defaults(SettingsPage::ContextMenu);
        assert_eq!(draft.edited.choices.open_files, "");
    }

    /// Saving the list asks for administrator permission and replaces the
    /// stored one, so only a real change may be saved: never an account
    /// picked again, Everyone turned on and off, or a list never read.
    #[test]
    fn only_a_changed_list_is_saved() {
        const ADMINISTRATORS: &str = "S-1-5-32-544";
        fn list(draft: &mut SettingsDraft) -> &mut SeeEveryFile {
            draft
                .edited
                .see_every_file
                .as_mut()
                .expect("the list was read")
        }
        let mut draft = SettingsDraft::new(SettingsValues {
            see_every_file: Ok(SeeEveryFile::new(vec![ADMINISTRATORS.to_owned()])),
            ..values()
        });
        list(&mut draft).add("s-1-5-32-544".to_owned());
        list(&mut draft).set_everyone(true);
        list(&mut draft).set_everyone(false);
        assert!(!draft.shielded());

        list(&mut draft).set_everyone(true);
        let changes = draft.edited.service_changes(draft.last_applied());
        assert_eq!(
            changes,
            Some(ServiceChanges {
                include: None,
                see_every_file: Some(SeeEveryFile::new(vec![
                    ADMINISTRATORS.to_owned(),
                    EVERYONE.to_owned(),
                ])),
            })
        );

        let unread = SettingsDraft::new(SettingsValues {
            see_every_file: Err("Access is denied.".to_owned()),
            ..values()
        });
        assert_eq!(unread.edited.service_changes(unread.last_applied()), None);
    }

    /// A file another version or a person edited must not lose every
    /// setting for one bad value: each damaged value falls back alone.
    #[test]
    fn a_damaged_value_falls_back_to_its_default() {
        let text = r#"{
            "settings": { "language": 5, "copy_policy": "replace" },
            "window": { "placement": { "left": 10, "top": 20, "right": 900, "bottom": 700, "maximized": true } }
        }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(settings.choices.language, LanguageChoice::Windows);
        assert_eq!(settings.choices.copy_policy, Policy::UpdateChanged);
        assert_eq!(
            settings.window.placement,
            Some(Placement {
                left: 10,
                top: 20,
                right: 900,
                bottom: 700,
                maximized: true
            })
        );

        let text =
            r#"{ "settings": { "language": "de" }, "window": { "placement": { "left": "x" } } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(settings.choices.language, LanguageChoice::Tag("de".into()));
        assert_eq!(settings.window.placement, None);

        // A width that overflows `i32` would panic where the window
        // computes it, on every start.
        let text = r#"{ "window": { "placement": { "left": -2147483648, "top": 0, "right": 2147483647, "bottom": 10, "maximized": false } } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(settings.window.placement, None);

        // So would an inspector taller than `i32` pixels at 200 %.
        let text = r#"{ "window": { "inspector_height": 2147483647 } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(settings.window.inspector_height, inspector_height());
    }

    /// An earlier version saved each column's width, and its file keeps its
    /// columns and their order.
    #[test]
    fn saved_column_widths_are_ignored() {
        let text = r#"{ "window": { "columns": [
            { "column": "size", "width": 90 },
            { "column": "name", "width": 300 }
        ] } }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(
            settings.window.columns,
            [
                SavedColumn {
                    column: Column::Size
                },
                SavedColumn {
                    column: Column::Name
                },
            ]
        );
    }

    /// The keys written must be the keys read.
    #[test]
    fn saved_settings_load_again() {
        let mut shortcuts = CommandShortcuts::default();
        let ctrl_k = Keystroke::ctrl(Key::Letter(b'K'));
        shortcuts
            .add(CommandId::Find, ctrl_k, None)
            .expect("Ctrl+K is free");
        let settings = Settings {
            choices: Choices {
                language: LanguageChoice::Tag("qps-ploc".into()),
                keep_running: false,
                open_files: r#""C:\Windows\notepad.exe" "%1""#.into(),
                show_shortcut: Some(
                    ShowShortcut::new(true, Keystroke::plain(Key::Letter(b'S')))
                        .expect("Win+S is a shortcut"),
                ),
                shortcuts,
                ..Choices::default()
            },
            window: WindowState {
                placement: Some(Placement {
                    left: -5,
                    top: 0,
                    right: 800,
                    bottom: 600,
                    maximized: false,
                }),
                mode: Mode::TwoPanels,
                matching: Matching {
                    mode: crate::core::query::MatchMode::WholeWords,
                    case: true,
                    path: false,
                },
                sort: SortSpec::default_for(gozler_core::types::SortKey::Size),
                columns: vec![
                    SavedColumn {
                        column: Column::Size,
                    },
                    SavedColumn {
                        column: Column::Name,
                    },
                ],
                filter_pane: false,
                filters: Filters {
                    kind: Kind::Documents,
                    location: Some(r"D:\work".into()),
                    date: DateModified::Custom(DateRange::Until(Day("2026-10-11"
                        .parse()
                        .expect("a date")))),
                    size: SizeFilter::Word(SizeWord::Large),
                    ..Filters::default()
                },
                inspector: false,
                inspector_height: Dip(250),
                inspector_maximized: false,
                transfers_flipped: true,
                tray_notice_shown: true,
                last_update_check: Some(1_790_000_000),
                destination_folder: Some(r"E:\Backup".into()),
                panels: SavedPanels {
                    left: SavedPanel {
                        folder: Some(r"D:\work".into()),
                        sort: SortSpec::default_for(gozler_core::types::SortKey::DateModified),
                        history: vec![r"D:\work".into(), r"C:\".into()],
                    },
                    right: SavedPanel::default(),
                    active: Side::Right,
                },
                viewer: ViewerState {
                    placement: Some(Placement {
                        left: 40,
                        top: 30,
                        right: 940,
                        bottom: 730,
                        maximized: true,
                    }),
                    wrap: false,
                },
            },
        };
        let text = settings.changes(&Settings::default()).apply(Some("{}"));
        assert_eq!(Settings::parse(&text), settings);
    }

    /// Each window saves its whole state, so a window that wrote every
    /// setting would undo a language that another window applied since
    /// (DESIGN.md: Window lifetime); a setting this window turned back to
    /// its default leaves the file.
    #[test]
    fn a_save_keeps_what_another_window_saved() {
        let mut loaded = Settings::default();
        loaded.window.destination_folder = Some(r"E:\Backup".into());
        let text = loaded.changes(&Settings::default()).apply(None);
        let mut other = loaded.clone();
        other.choices.language = LanguageChoice::Tag("de".into());
        let text = other.changes(&loaded).apply(Some(&text));
        let mut this = loaded.clone();
        this.window.mode = Mode::TwoPanels;
        this.window.destination_folder = None;
        let text = this.changes(&loaded).apply(Some(&text));
        let saved = Settings::parse(&text);
        assert_eq!(saved.choices.language, LanguageChoice::Tag("de".into()));
        assert_eq!(saved.window.mode, Mode::TwoPanels);
        assert_eq!(saved.window.destination_folder, None);
    }

    #[test]
    fn coalesced_saves_keep_changed_keys_removals_and_other_windows_choices() {
        let mut before = Settings::default();
        before.choices.show_memory = true;
        before.window.destination_folder = Some(r"E:\Backup".into());
        let mut first = before.clone();
        first.choices.show_memory = false;
        first.window.destination_folder = None;
        let mut last = first.clone();
        last.choices.show_memory = true;
        last.window.mode = Mode::TwoPanels;
        let changes = first.changes(&before).combine(last.changes(&first));
        let latest_file = r#"{
            "settings": {"language": "de", "show_memory": false},
            "window": {"destination_folder": "E:\\Backup"}
        }"#;
        let saved: Value =
            serde_json::from_str(&changes.apply(Some(latest_file))).expect("changes produce JSON");
        assert_eq!(saved["settings"]["language"], "de");
        assert_eq!(saved["settings"]["show_memory"], true);
        assert_eq!(saved["window"]["mode"], "two_panels");
        assert!(saved["window"].get("destination_folder").is_none());
    }
}
