//! Every caption of the main window, built from the model's facts and its
//! strings when it is shown (DESIGN.md: window rule 12).
//! [`WindowCaptions::new`] calls every caption function: `app` sets its result
//! at creation and after a change to the regional format, and
//! `layout::CaptionWidths` measures the same value. A handler that updates one
//! caption calls the same function.
//!
//! A dialog's fixed captions come from its [`CaptionTable`], which
//! [`DialogCaptions::new`] reads in the language of the moment.
//!
//! A catalogue key is written whole, as a literal, so the test below finds
//! every key the code uses in `en.json`.

use super::commands::{
    self, CommandButton, HIDE_DETAILS, INSPECTOR_BUTTONS, INSPECTOR_COPY_FOLDER,
    INSPECTOR_COPY_NAME, INSPECTOR_COPY_PATH, MenuNode, SHOW_DETAILS,
};
use super::fetch::{Beside, DaemonFailure, Local, LocalService, ProgramFile, Programs};
use super::fixer::{
    CommandFix, CommandResult, CommandStep, FixState, Fixer, SetupFix, SetupResult, SetupStep,
};
use super::glyph::Glyph;
use super::ids::{ControlId, DialogId};
use super::layout::WIDEST_TOTAL;
use super::model::{Column, Export, Job, Model};
use super::query::{
    ATTRIBUTES, AttributeChoice, DateModified, DateRange, Day, Kind, LocationItem, MatchMode,
    RangeItem, SizeFilter, SizeRange, SizeUnit, SizeWord, date_items, size_items,
};
use super::results::{Fix, LIST_LIMIT, ListButton, ListMessage, Shown};
use super::selection::{FolderContents, Selection, Sums};
use super::settings::SettingsPage;
use super::text::{self, Numbers, Strings};
use gozler_client::NotRunning;
use gozler_core::proto::ResultItem;
use gozler_core::query::{Malformed, QueryError};
use gozler_core::types::VolumePhase;
use std::path::PathBuf;

/// The main window's captions, in the language and regional format of the
/// moment they are built.
pub(crate) struct WindowCaptions {
    /// Whether icons show beside text (Settings > General), which makes
    /// the inspector's buttons and the Kind list's rows larger.
    pub(crate) icons: bool,
    pub(crate) title: String,
    pub(crate) menu_bar: Vec<MenuNode>,
    pub(crate) search_cue: String,
    pub(crate) clear_search: String,
    pub(crate) match_label: String,
    /// In [`MatchMode::ALL`]'s order.
    pub(crate) match_modes: [String; 5],
    pub(crate) match_case: String,
    pub(crate) match_path: String,
    pub(crate) filters: FilterCaptions,
    /// In [`Model::columns`]' order.
    pub(crate) headers: Vec<String>,
    /// The inspector's buttons, in [`INSPECTOR_BUTTONS`]' order.
    pub(crate) inspector_buttons: [ButtonCaption; INSPECTOR_BUTTONS.len()],
    pub(crate) copy_path: ButtonCaption,
    pub(crate) copy_name: ButtonCaption,
    pub(crate) copy_folder: ButtonCaption,
    pub(crate) hide_details: ButtonCaption,
    pub(crate) show_details: ButtonCaption,
    /// Every caption a button below the list's message can show, with
    /// whether the administrator shield shows beside it.
    pub(crate) list_buttons: Vec<(String, bool)>,
    /// Every label an inspector field can have, which the layout sizes the
    /// label column for, so the values keep their place in every state.
    pub(crate) inspector_labels: [String; FIELD_LABELS.len()],
    pub(crate) inspector: InspectorCaptions,
    pub(crate) status: StatusCaptions,
    /// The status bar's Cancel, which stops the running job.
    pub(crate) status_cancel: String,
    pub(crate) status_widest: StatusWidest,
    pub(crate) message: Option<ListCaption>,
}

/// What a [`CommandButton`] says: its caption, which for an icon is the name
/// screen readers say, and its tooltip, empty for none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ButtonCaption {
    pub(crate) caption: String,
    pub(crate) tip: String,
}

impl ButtonCaption {
    fn new(strings: &Strings, button: CommandButton) -> ButtonCaption {
        let tip = button.tip.map(|key| strings.text(key).to_owned());
        match button.caption {
            Some(key) => ButtonCaption {
                caption: strings.text(key).to_owned(),
                tip: tip.unwrap_or_default(),
            },
            None => {
                let name = tip.unwrap_or_else(|| {
                    text::without_access_key(strings.text(button.command.row().label))
                });
                ButtonCaption {
                    caption: name.clone(),
                    tip: name,
                }
            }
        }
    }
}

/// The status bar's three groups (DESIGN.md: Status bar).
pub(crate) struct StatusCaptions {
    /// What the view holds: the results and the selection.
    pub(crate) view: String,
    pub(crate) job: JobCaption,
    /// The service and the index, and a newer release, as the Settings
    /// dialog's right group shows them.
    pub(crate) service: String,
    /// The main window's right group: `service`, then the search's totals,
    /// each after its icon.
    pub(crate) right: Vec<ValuePart>,
}

/// The status bar's middle group: the running job, or the last job's
/// result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JobCaption {
    pub(crate) text: String,
    pub(crate) progress: JobProgress,
    /// Whether Cancel shows, because the job can stop now.
    pub(crate) cancel: bool,
}

/// The progress bar beside a job's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobProgress {
    /// No bar: the job waits for the person, or it ended.
    None,
    /// A bar that moves without measuring, because the job does not know
    /// how much is left.
    Unmeasured,
    /// A bar filled to `done` of `total`.
    Measured { done: u64, total: u64 },
}

/// The texts each fixed group of the status bar can show at its widest,
/// which the layout sizes the group for, so a group never moves with its
/// text (DESIGN.md: Look).
pub(crate) struct StatusWidest {
    pub(crate) view: Vec<String>,
    /// A running job's texts, which share the middle group with its
    /// progress bar and Cancel.
    pub(crate) running: Vec<String>,
    /// A job's results, which have the middle group to themselves.
    pub(crate) done: Vec<String>,
}

/// What the list's empty area says instead of rows: the message, and its
/// buttons, in [`Model::list_buttons`]' order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListCaption {
    pub(crate) text: String,
    pub(crate) buttons: Vec<MessageButton>,
}

/// The filter pane's captions and the items of its lists, in the lists'
/// orders.
pub(crate) struct FilterCaptions {
    pub(crate) title: String,
    pub(crate) clear: String,
    pub(crate) clear_tip: String,
    /// The names and tips of the icons that hide and show the pane.
    pub(crate) hide: String,
    pub(crate) show: String,
    pub(crate) kind: String,
    /// In [`Kind::ALL`]'s order.
    pub(crate) kinds: [String; 9],
    pub(crate) location: String,
    /// In [`Model::location_items`]' order.
    pub(crate) locations: Vec<String>,
    pub(crate) choose_folder: String,
    pub(crate) only_directly: String,
    pub(crate) date: String,
    /// In [`date_items`]' order.
    pub(crate) dates: Vec<String>,
    pub(crate) size: String,
    /// In [`size_items`]' order.
    pub(crate) sizes: Vec<String>,
    pub(crate) attributes: String,
    /// In [`ATTRIBUTES`]' order.
    pub(crate) attribute_names: [String; 6],
    /// In [`AttributeChoice::ALL`]'s order.
    pub(crate) attribute_choices: [String; 3],
    /// The Date modified and Size items the pane is sized for: each choice
    /// but a custom range, and a custom range of each as wide as a usual
    /// one, so the pane keeps its width when one is set.
    pub(crate) ranges_widest: Vec<String>,
}

impl FilterCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> FilterCaptions {
        let strings = model.strings();
        let filters = model.filters();
        let text = |key: &'static str| strings.text(key).to_owned();
        let dates = |chosen: DateModified| -> Vec<String> {
            date_items(chosen)
                .into_iter()
                .map(|item| match item {
                    RangeItem::Choice(choice) => date_text(strings, numbers, choice),
                    RangeItem::Custom => text("filter.custom"),
                })
                .collect()
        };
        let sizes = |chosen: SizeFilter| -> Vec<String> {
            size_items(chosen)
                .into_iter()
                .map(|item| match item {
                    RangeItem::Choice(choice) => size_text(strings, numbers, choice),
                    RangeItem::Custom => text("filter.custom"),
                })
                .collect()
        };
        let day = Day(jiff::civil::date(2026, 12, 30));
        let samples = [
            dates(DateModified::Custom(DateRange::Between(day, day))),
            sizes(SizeFilter::Custom(SizeRange {
                min: Some(150 * 1024),
                max: Some(1536 * 1024),
                unit: SizeUnit::Mb,
            })),
        ];
        FilterCaptions {
            title: text("filter.title"),
            clear: text("filter.clear"),
            clear_tip: text("filter.clear_tooltip"),
            hide: text("filter.hide"),
            show: text("filter.show"),
            kind: text("filter.kind"),
            kinds: Kind::ALL.map(|kind| text(kind.label())),
            location: text("filter.location"),
            locations: location_items(model),
            choose_folder: text("filter.choose_folder"),
            only_directly: text("filter.only_directly"),
            date: text("filter.date"),
            dates: dates(filters.date),
            size: text("filter.size"),
            sizes: sizes(filters.size),
            attributes: text("filter.attributes"),
            attribute_names: ATTRIBUTES.map(|(_, label)| text(label)),
            attribute_choices: AttributeChoice::ALL.map(|choice| text(choice.label())),
            ranges_widest: samples.into_iter().flatten().collect(),
        }
    }
}

/// The Location list's items: Everywhere, each volume by its mount prefix
/// with its state when it is not complete (DESIGN.md: Filter pane), and the
/// chosen folders.
pub(crate) fn location_items(model: &Model) -> Vec<String> {
    let strings = model.strings();
    model
        .location_items()
        .iter()
        .map(|item| match item {
            LocationItem::Everywhere => strings.text("filter.everywhere").to_owned(),
            LocationItem::Volume(volume) => {
                let prefix = volume.mounted().display_prefix();
                match volume.phase {
                    VolumePhase::Live | VolumePhase::Snapshot => prefix.to_owned(),
                    VolumePhase::Bootstrapping | VolumePhase::Rescanning => {
                        strings.format("filter.location_indexing", &[prefix])
                    }
                    VolumePhase::Offline => strings.format("filter.location_offline", &[prefix]),
                    VolumePhase::Failed { .. } => {
                        strings.format("filter.location_failed", &[prefix])
                    }
                }
            }
            LocationItem::Folder(folder) => (*folder).to_owned(),
        })
        .collect()
}

/// A Date modified choice as its list shows it.
fn date_text(strings: &Strings, numbers: &dyn Numbers, date: DateModified) -> String {
    let day = |day: Day| numbers.date(day.0);
    match date {
        DateModified::Any => strings.text("filter.any_time").to_owned(),
        DateModified::Today => strings.text("filter.today").to_owned(),
        DateModified::Yesterday => strings.text("filter.yesterday").to_owned(),
        DateModified::Last7Days => strings.text("filter.last_7_days").to_owned(),
        DateModified::Last30Days => strings.text("filter.last_30_days").to_owned(),
        DateModified::Last365Days => strings.text("filter.last_365_days").to_owned(),
        DateModified::Custom(DateRange::From(from)) => {
            strings.format("filter.date_from", &[&day(from)])
        }
        DateModified::Custom(DateRange::Until(to)) => {
            strings.format("filter.date_until", &[&day(to)])
        }
        DateModified::Custom(DateRange::Between(from, to)) => {
            strings.format("filter.date_between", &[&day(from), &day(to)])
        }
    }
}

/// A Size choice as its list shows it: a size word with its range, such as
/// "Tiny (up to 10 KB)", or a custom range in its unit.
fn size_text(strings: &Strings, numbers: &dyn Numbers, size: SizeFilter) -> String {
    match size {
        SizeFilter::Any => strings.text("filter.any_size").to_owned(),
        SizeFilter::Word(word) => {
            let range = word.word().range;
            let size = |bytes: u64| round_size(strings, numbers, bytes);
            let text = if range.min <= 1 {
                strings.format("filter.range_up_to", &[&size(range.max)])
            } else if range.max == u64::MAX {
                strings.format("filter.range_over", &[&size(range.min - 1)])
            } else {
                strings.format(
                    "filter.range_between",
                    &[&size(range.min - 1), &size(range.max)],
                )
            };
            // Empty has no range to show.
            match word {
                SizeWord::Empty => strings.text(word.label()).to_owned(),
                _ => strings.format(word.label(), &[&text]),
            }
        }
        SizeFilter::Custom(range) => {
            let size = |bytes: u64| unit_size(strings, numbers, bytes, range.unit);
            match (range.min, range.max) {
                (Some(min), Some(max)) => {
                    strings.format("filter.size_between", &[&size(min), &size(max)])
                }
                (Some(min), None) => strings.format("filter.size_at_least", &[&size(min)]),
                (None, Some(max)) => strings.format("filter.size_up_to", &[&size(max)]),
                (None, None) => strings.text("filter.any_size").to_owned(),
            }
        }
    }
}

const SIZE_UNITS: [(u64, &str); 3] = [
    (1 << 30, "size.gb"),
    (1 << 20, "size.mb"),
    (1 << 10, "size.kb"),
];

/// A size word's boundary, a whole number of a unit, such as "10 KB".
fn round_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64) -> String {
    SIZE_UNITS
        .iter()
        .find(|(unit, _)| bytes >= *unit && bytes.is_multiple_of(*unit))
        .map_or_else(
            || text::bytes(strings, numbers, bytes),
            |(unit, key)| strings.format(key, &[&numbers.integer(bytes / unit)]),
        )
}

/// `bytes` in `unit`, with the decimals the person can have typed, such as
/// "1.5 MB".
fn unit_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64, unit: SizeUnit) -> String {
    let value = bytes as f64 / unit.bytes() as f64;
    let digits = (0u8..2)
        .find(|&digits| {
            let scaled = value * 10f64.powi(i32::from(digits));
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(2);
    let key = match unit {
        SizeUnit::Kb => "size.kb",
        SizeUnit::Mb => "size.mb",
        SizeUnit::Gb => "size.gb",
    };
    strings.format(key, &[&numbers.decimal(value, digits)])
}

/// A control of a dialog that a caption goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogItem {
    Ok,
    Cancel,
    Control(ControlId),
}

impl DialogItem {
    /// Its control ID. OK and Cancel are `IDOK` and `IDCANCEL`, which a
    /// dialog sends for `Enter` and `Esc`.
    pub(crate) fn id(self) -> u16 {
        match self {
            DialogItem::Ok => 1,
            DialogItem::Cancel => 2,
            DialogItem::Control(control) => control.id(),
        }
    }
}

/// The group boxes of the Settings pages, each with the icon before its
/// caption.
pub(crate) const SETTINGS_GROUPS: [(ControlId, Glyph); 14] = [
    (ControlId::GeneralStarting, Glyph::PowerButton),
    (ControlId::SearchBoxGroup, Glyph::Search),
    (ControlId::ContextMenuItemGroup, Glyph::BulletedList),
    (ControlId::ContextMenuOpenGroup, Glyph::OpenFile),
    (ControlId::KeyboardShowGroup, Glyph::BackToWindow),
    (ControlId::KeyboardCommandsGroup, Glyph::KeyboardShortcut),
    (ControlId::IndexDrivesGroup, Glyph::HardDrive),
    (ControlId::IndexHoldsGroup, Glyph::Library),
    (ControlId::IndexServiceGroup, Glyph::Processing),
    (ControlId::IncludeFoldersGroup, Glyph::Folder),
    (ControlId::IncludeNamesGroup, Glyph::Rename),
    (ControlId::SpeedCopyingGroup, Glyph::Copy),
    (ControlId::SpeedRingGroup, Glyph::Sync),
    (ControlId::SpeedDeleteGroup, Glyph::Delete),
];

/// The icon of each push button's caption, by its catalogue key, so a
/// caption shows the same icon on every button that has it (DESIGN.md:
/// Look). A fix that needs administrator permission shows the
/// administrator shield instead, so its caption has no icon here.
const BUTTON_GLYPHS: [(&str, Glyph); 23] = [
    ("button.ok", Glyph::Accept),
    ("button.cancel", Glyph::Cancel),
    ("button.close", Glyph::ChromeClose),
    ("button.clear", Glyph::Clear),
    ("settings.apply", Glyph::Save),
    ("settings.reset", Glyph::Undo),
    ("settings.reset_all", Glyph::Undo),
    ("settings_context_menu.browse", Glyph::FolderOpen),
    ("settings_include.modify", Glyph::Edit),
    ("settings_index.report", Glyph::Document),
    ("settings_keyboard.add", Glyph::Add),
    ("settings_keyboard.edit", Glyph::Edit),
    ("settings_keyboard.remove", Glyph::Remove),
    ("service_report.copy", Glyph::Copy),
    ("file_operations.hide", Glyph::ChromeMinimize),
    ("filter.clear", Glyph::Clear),
    ("filter.choose_folder", Glyph::FolderOpen),
    ("command.clear_filters", Glyph::Clear),
    ("list.search_everywhere", Glyph::Search),
    ("fix.download", Glyph::Download),
    ("fix.update", Glyph::Download),
    ("fix.downgrade", Glyph::Download),
    ("fix.open_services", Glyph::Processing),
];

/// The icon of a push button whose caption is the catalogue text `key`.
pub(crate) fn button_glyph(key: &str) -> Option<Glyph> {
    BUTTON_GLYPHS
        .iter()
        .find(|(caption, _)| *caption == key)
        .map(|(_, glyph)| *glyph)
}

/// What a push button shows before its caption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonIcon {
    /// Shown only while icons show (DESIGN.md: Look).
    Glyph(Glyph),
    /// The administrator shield, of a button that asks for administrator
    /// permission, also while icons are hidden.
    Shield,
}

impl ButtonIcon {
    /// The icon of a button captioned with the catalogue text `key`, which
    /// asks for administrator permission when `shield`.
    pub(crate) fn of(key: &str, shield: bool) -> Option<ButtonIcon> {
        if shield {
            Some(ButtonIcon::Shield)
        } else {
            button_glyph(key).map(ButtonIcon::Glyph)
        }
    }
}

/// The catalogue keys of a dialog's title and fixed captions. A text that
/// depends on a fact, such as About's version, is the dialog's own.
pub(crate) struct CaptionTable {
    title: &'static str,
    /// Captions with an access key, which is unique in the dialog.
    keyed: &'static [(DialogItem, &'static str)],
    /// Captions without one: the buttons that `Enter` and `Esc` press, and
    /// texts that label no control.
    plain: &'static [(DialogItem, &'static str)],
}

impl CaptionTable {
    pub(crate) fn of(dialog: DialogId) -> CaptionTable {
        use ControlId as C;
        use DialogItem::{Cancel, Control, Ok};
        match dialog {
            DialogId::About => CaptionTable {
                title: "about.title",
                keyed: &[],
                plain: &[
                    (Control(C::AboutDescription), "about.description"),
                    (Control(C::AboutCopyright), "about.copyright"),
                    (Cancel, "button.close"),
                ],
            },
            DialogId::SearchSyntax => CaptionTable {
                title: "search_syntax.title",
                keyed: &[],
                plain: &[(Cancel, "button.close")],
            },
            DialogId::CustomDate => CaptionTable {
                title: "custom_date.title",
                keyed: &[
                    (Control(C::CustomDateFromLabel), "custom_date.from"),
                    (Control(C::CustomDateToLabel), "custom_date.to"),
                ],
                plain: &[(Ok, "button.ok"), (Cancel, "button.cancel")],
            },
            DialogId::CustomSize => CaptionTable {
                title: "custom_size.title",
                keyed: &[
                    (Control(C::CustomSizeMinimumLabel), "custom_size.minimum"),
                    (Control(C::CustomSizeMaximumLabel), "custom_size.maximum"),
                    (Control(C::CustomSizeUnitLabel), "custom_size.unit"),
                ],
                plain: &[(Ok, "button.ok"), (Cancel, "button.cancel")],
            },
            DialogId::Settings => CaptionTable {
                title: "settings.title",
                keyed: &[
                    (Control(C::SettingsResetAll), "settings.reset_all"),
                    (Control(C::SettingsApply), "settings.apply"),
                ],
                plain: &[
                    (Control(C::SettingsNoMatch), "settings.no_match"),
                    (Control(C::SettingsClearSearch), "button.clear"),
                    (Ok, "button.ok"),
                    (Cancel, "button.cancel"),
                ],
            },
            DialogId::SettingsGeneral => CaptionTable {
                title: "settings.general",
                keyed: &[
                    (
                        Control(C::GeneralLanguageLabel),
                        "settings_general.language",
                    ),
                    (Control(C::GeneralSignIn), "settings_general.sign_in"),
                    (Control(C::GeneralUpdates), "settings_general.updates"),
                    (
                        Control(C::GeneralKeepRunning),
                        "settings_general.keep_running",
                    ),
                    (Control(C::GeneralOneWindow), "settings_general.one_window"),
                    (Control(C::GeneralShowIcons), "settings_general.show_icons"),
                    (Control(C::GeneralRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::GeneralHeading), "settings.general"),
                    (Control(C::GeneralStarting), "settings_general.starting"),
                ],
            },
            // The item group's caption names the selected item, which the
            // page sets.
            DialogId::SettingsContextMenu => CaptionTable {
                title: "settings.context_menu",
                keyed: &[
                    (
                        Control(C::ContextMenuItemsLabel),
                        "settings_context_menu.items_label",
                    ),
                    (Control(C::ContextMenuShow), "settings_context_menu.show"),
                    (
                        Control(C::ContextMenuShowWithShift),
                        "settings_context_menu.show_with_shift",
                    ),
                    (Control(C::ContextMenuHide), "settings_context_menu.hide"),
                    (
                        Control(C::ContextMenuOpenFoldersLabel),
                        "settings_context_menu.open_folders",
                    ),
                    (
                        Control(C::ContextMenuOpenFilesLabel),
                        "settings_context_menu.open_files",
                    ),
                    (
                        Control(C::ContextMenuOpenLocationLabel),
                        "settings_context_menu.open_file_location",
                    ),
                    (Control(C::ContextMenuRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::ContextMenuHeading), "settings.context_menu"),
                    (
                        Control(C::ContextMenuOpenGroup),
                        "settings_context_menu.open_with",
                    ),
                    // One letter cannot name three buttons (DESIGN.md: The
                    // Context menu page).
                    (
                        Control(C::ContextMenuOpenFoldersBrowse),
                        "settings_context_menu.browse",
                    ),
                    (
                        Control(C::ContextMenuOpenFilesBrowse),
                        "settings_context_menu.browse",
                    ),
                    (
                        Control(C::ContextMenuOpenLocationBrowse),
                        "settings_context_menu.browse",
                    ),
                    (
                        Control(C::ContextMenuCommandHint),
                        "settings_context_menu.command_hint",
                    ),
                    (
                        Control(C::ContextMenuTotalCommander),
                        "settings_context_menu.total_commander",
                    ),
                ],
            },
            DialogId::SettingsSearch => CaptionTable {
                title: "settings.search",
                keyed: &[
                    (Control(C::SearchAsYouType), "settings_search.as_you_type"),
                    (Control(C::SearchShowAll), "settings_search.show_all"),
                    (
                        Control(C::SearchSelectOnClick),
                        "settings_search.select_on_click",
                    ),
                    (
                        Control(C::SearchSelectOnActivate),
                        "settings_search.select_on_activate",
                    ),
                    (Control(C::SearchRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::SearchHeading), "settings.search"),
                    (Control(C::SearchBoxGroup), "settings_search.search_box"),
                ],
            },
            DialogId::SettingsKeyboard => CaptionTable {
                title: "settings.keyboard",
                keyed: &[
                    (
                        Control(C::KeyboardShortcutLabel),
                        "settings_keyboard.shortcut",
                    ),
                    (
                        Control(C::KeyboardWindowsKey),
                        "settings_keyboard.windows_key",
                    ),
                    (Control(C::KeyboardFilterLabel), "settings_keyboard.filter"),
                    (Control(C::KeyboardKeysLabel), "settings_keyboard.keys"),
                    (Control(C::KeyboardAdd), "settings_keyboard.add"),
                    (Control(C::KeyboardEdit), "settings_keyboard.edit"),
                    (Control(C::KeyboardRemove), "settings_keyboard.remove"),
                    (Control(C::KeyboardRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::KeyboardHeading), "settings.keyboard"),
                    (Control(C::KeyboardClear), "button.clear"),
                    (
                        Control(C::KeyboardShowGroup),
                        "settings_keyboard.show_gozler",
                    ),
                    (
                        Control(C::KeyboardShortcutHint),
                        "settings_keyboard.shortcut_hint",
                    ),
                    (
                        Control(C::KeyboardCommandsGroup),
                        "settings_keyboard.commands",
                    ),
                ],
            },
            DialogId::SettingsIndex => CaptionTable {
                title: "settings.index",
                keyed: &[(Control(C::IndexReport), "settings_index.report")],
                plain: &[
                    (Control(C::IndexHeading), "settings.index"),
                    (Control(C::IndexDrivesGroup), "settings_index.drives"),
                    (Control(C::IndexHoldsGroup), "settings_index.holds"),
                    (Control(C::IndexHolds), "settings_index.holds_text"),
                    (Control(C::IndexServiceGroup), "settings_index.service"),
                    (
                        Control(C::IndexServiceProgramLabel),
                        "settings_index.service_program",
                    ),
                    (Control(C::IndexBesideLabel), "settings_index.beside"),
                ],
            },
            DialogId::SettingsInclude => CaptionTable {
                title: "settings.include",
                keyed: &[
                    (
                        Control(C::IncludeFoldersLabel),
                        "settings_include.folders_label",
                    ),
                    (Control(C::IncludeModify), "settings_include.modify"),
                    (Control(C::IncludeNewDrives), "settings_include.new_drives"),
                    (
                        Control(C::IncludeNamesLabel),
                        "settings_include.names_label",
                    ),
                    (Control(C::IncludeHidden), "settings_include.hidden"),
                    (Control(C::IncludeSystem), "settings_include.system"),
                    (Control(C::IncludeRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::IncludeHeading), "settings.include"),
                    (Control(C::IncludeAdminNote), "settings_include.admin_note"),
                    (Control(C::IncludeFoldersGroup), "settings_include.folders"),
                    (Control(C::IncludeNamesGroup), "settings_include.names"),
                    (Control(C::IncludeNamesHint), "settings_include.names_hint"),
                ],
            },
            DialogId::SettingsFileOperations => CaptionTable {
                title: "settings.file_operations",
                keyed: &[
                    (
                        Control(C::SpeedPresetLabel),
                        "settings_file_operations.speed",
                    ),
                    (
                        Control(C::SpeedLargeFileLimitLabel),
                        "settings_file_operations.large_file_limit",
                    ),
                    (
                        Control(C::SpeedLargeBetweenSsdsLabel),
                        "settings_file_operations.large_between_ssds",
                    ),
                    (
                        Control(C::SpeedHardDiskLabel),
                        "settings_file_operations.hard_disk",
                    ),
                    (
                        Control(C::SpeedWorkersSsdLabel),
                        "settings_file_operations.workers_ssd",
                    ),
                    (
                        Control(C::SpeedWorkersUsbLabel),
                        "settings_file_operations.workers_usb",
                    ),
                    (
                        Control(C::SpeedWorkersHardDiskLabel),
                        "settings_file_operations.workers_hard_disk",
                    ),
                    (
                        Control(C::SpeedWorkersShareLabel),
                        "settings_file_operations.workers_share",
                    ),
                    (
                        Control(C::SpeedRingSsdLabel),
                        "settings_file_operations.ring_ssd",
                    ),
                    (
                        Control(C::SpeedRingHardDiskLabel),
                        "settings_file_operations.ring_hard_disk",
                    ),
                    (
                        Control(C::SpeedDeleteWorkersLabel),
                        "settings_file_operations.delete_workers",
                    ),
                    (
                        Control(C::SpeedPerProcessor),
                        "settings_file_operations.per_processor",
                    ),
                    (Control(C::SpeedRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::SpeedHeading), "settings.file_operations"),
                    (
                        Control(C::SpeedCopyingGroup),
                        "settings_file_operations.copying",
                    ),
                    (
                        Control(C::SpeedLargeFileLimitUnit),
                        "settings_file_operations.mib",
                    ),
                    (
                        Control(C::SpeedWorkersLabel),
                        "settings_file_operations.workers",
                    ),
                    (Control(C::SpeedRingGroup), "settings_file_operations.ring"),
                    (
                        Control(C::SpeedDeleteGroup),
                        "settings_file_operations.deleting",
                    ),
                    (
                        Control(C::SpeedBlockSizeLabel),
                        "settings_file_operations.block_size",
                    ),
                    (
                        Control(C::SpeedInFlightLabel),
                        "settings_file_operations.in_flight",
                    ),
                    (
                        Control(C::SpeedBufferLabel),
                        "settings_file_operations.buffer",
                    ),
                ],
            },
            DialogId::IndexedFolders => CaptionTable {
                title: "indexed_folders.title",
                keyed: &[
                    (Control(C::FoldersTreeLabel), "indexed_folders.tree"),
                    (Control(C::FoldersSummaryLabel), "indexed_folders.summary"),
                ],
                plain: &[
                    (Control(C::FoldersHint), "indexed_folders.hint"),
                    (Ok, "button.ok"),
                    (Cancel, "button.cancel"),
                ],
            },
            DialogId::ServiceReport => CaptionTable {
                title: "service_report.title",
                keyed: &[(Control(C::ServiceReportCopy), "service_report.copy")],
                plain: &[(Cancel, "button.close")],
            },
            // The window sets the operation's name as the title, and the
            // texts and the list from what failed.
            DialogId::FileOperationsFailed => CaptionTable {
                title: "file_operations.title",
                keyed: &[],
                plain: &[(Cancel, "button.close")],
            },
            // DialogCaptions::command_shortcut fills in the command and the
            // title of Edit….
            DialogId::CommandShortcut => CaptionTable {
                title: "command_shortcut.add_title",
                keyed: &[(
                    Control(C::CommandShortcutLabel),
                    "command_shortcut.shortcut_for",
                )],
                plain: &[(Ok, "button.ok"), (Cancel, "button.cancel")],
            },
        }
    }

    /// The dialog whose keyboard a page's controls share, because Windows
    /// looks for an access key in the whole dialog.
    #[cfg(test)]
    fn shares_keys_with(dialog: DialogId) -> Option<DialogId> {
        match dialog {
            DialogId::SettingsGeneral
            | DialogId::SettingsSearch
            | DialogId::SettingsContextMenu
            | DialogId::SettingsKeyboard
            | DialogId::SettingsIndex
            | DialogId::SettingsInclude
            | DialogId::SettingsFileOperations => Some(DialogId::Settings),
            DialogId::About
            | DialogId::SearchSyntax
            | DialogId::CustomDate
            | DialogId::CustomSize
            | DialogId::Settings
            | DialogId::IndexedFolders
            | DialogId::ServiceReport
            | DialogId::FileOperationsFailed
            | DialogId::CommandShortcut => None,
        }
    }
}

/// What Settings > Index shows (DESIGN.md: The Drives and service page):
/// each drive by its mount prefix and label, its state and its entry count,
/// the service's state and memory, the gozlerd.exe files, the failure's fix,
/// and whether the service has a report to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IndexCaptions {
    /// Each drive's name, state and entry count, as the list's columns.
    pub(crate) drives: Vec<[String; 3]>,
    /// What the drive list's empty area says, or empty.
    pub(crate) no_drives: String,
    pub(crate) service: String,
    pub(crate) service_program: ProgramLines,
    /// The gozlerd.exe beside the window.
    pub(crate) beside: ProgramLines,
    /// Why Use local is not offered, or empty.
    pub(crate) note: String,
    /// The Use local button, which shows before the fix's.
    pub(crate) use_local: Option<MessageButton>,
    pub(crate) fix: Option<MessageButton>,
    pub(crate) has_report: bool,
}

/// A gozlerd.exe file's path and build, each empty while unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProgramLines {
    pub(crate) path: String,
    pub(crate) build: String,
}

impl ProgramLines {
    fn of(strings: &Strings, numbers: &dyn Numbers, file: &ProgramFile) -> ProgramLines {
        let modified = numbers.date_time_seconds(file.modified);
        // Without the time in parentheses, which `ProgramFile::modified`
        // replaces.
        let number = file
            .version
            .as_deref()
            .and_then(|version| version.split_whitespace().next());
        let build = match number {
            Some(number) => strings.format("settings_index.version_modified", &[number, &modified]),
            None => strings.format("settings_index.modified", &[&modified]),
        };
        ProgramLines {
            path: file.path.display().to_string(),
            build,
        }
    }

    /// The service's program and the gozlerd.exe beside the window.
    fn both(strings: &Strings, numbers: &dyn Numbers, programs: &Programs) -> [ProgramLines; 2] {
        let service = programs
            .service
            .as_ref()
            .map(|file| ProgramLines::of(strings, numbers, file))
            .unwrap_or_default();
        let beside = match &programs.beside {
            Beside::Unknown => ProgramLines::default(),
            Beside::SameAsService(path) => ProgramLines {
                path: path.display().to_string(),
                build: strings.text("settings_index.same_as_service").to_owned(),
            },
            Beside::Other(file) => ProgramLines::of(strings, numbers, file),
        };
        [service, beside]
    }
}

/// A button of the list's message or of the Service group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessageButton {
    pub(crate) label: String,
    pub(crate) icon: Option<ButtonIcon>,
    pub(crate) enabled: bool,
    pub(crate) tooltip: Option<String>,
}

impl MessageButton {
    fn new(model: &Model, button: &ListButton) -> MessageButton {
        let strings = model.strings();
        let key = model.button_label(button);
        MessageButton {
            label: strings.text(key).to_owned(),
            icon: ButtonIcon::of(key, button.needs_administrator()),
            enabled: model.list_button_enabled(button),
            tooltip: match button {
                ListButton::Fix(fix) => fix_tooltip(strings, fix),
                ListButton::ClearFilters
                | ListButton::SearchEverywhere
                | ListButton::ChooseFolder => None,
            },
        }
    }
}

impl IndexCaptions {
    /// `label` gives the volume label of a mount prefix, such as "Windows"
    /// for `C:\`, which only Windows knows.
    pub(crate) fn new(
        model: &Model,
        numbers: &dyn Numbers,
        label: &dyn Fn(&str) -> Option<String>,
    ) -> IndexCaptions {
        let strings = model.strings();
        let (status, failure) = model.service();
        let programs = model.programs();
        let [service_program, beside] = programs
            .map(|programs| ProgramLines::both(strings, numbers, programs))
            .unwrap_or_default();
        let note = match programs.map(|programs| &programs.local) {
            Some(Local::Mismatched) => strings.text("fixing.use_local_not_paired").to_owned(),
            Some(Local::NotOffered | Local::Offered(_)) | None => String::new(),
        };
        let files = IndexCaptions {
            service_program,
            beside,
            note,
            ..IndexCaptions::default()
        };
        if let Some(failure) = failure {
            let fix_button = |fix: Fix| MessageButton::new(model, &ListButton::Fix(fix));
            return IndexCaptions {
                no_drives: strings.text("settings_index.no_drives").to_owned(),
                service: failure_text(strings, failure),
                use_local: model.use_local_fix().map(fix_button),
                fix: model.service_fix().map(fix_button),
                ..files
            };
        }
        let Some(status) = status else {
            return IndexCaptions {
                service: strings.text("settings_index.asking").to_owned(),
                ..files
            };
        };
        let name = |prefix: &str| match label(prefix).filter(|label| !label.is_empty()) {
            Some(label) => format!("{prefix} {label}"),
            None => prefix.to_owned(),
        };
        let indexed = status.volumes.iter().map(|volume| {
            let state = match volume.phase {
                VolumePhase::Live if volume.metadata_pending => "settings_index.live_pending",
                VolumePhase::Live => "settings_index.live",
                VolumePhase::Snapshot => "settings_index.snapshot",
                VolumePhase::Bootstrapping | VolumePhase::Rescanning => "settings_index.indexing",
                VolumePhase::Offline => "settings_index.offline",
                VolumePhase::Failed { .. } => "settings_index.failed",
            };
            [
                name(volume.mounted().display_prefix()),
                strings.text(state).to_owned(),
                numbers.integer(volume.entries),
            ]
        });
        // A drive the Include settings leave out has no entries.
        let left_out = status.left_out.iter().map(|volume| {
            [
                name(volume.mounted().display_prefix()),
                strings.text("settings_index.left_out").to_owned(),
                String::new(),
            ]
        });
        let drives = indexed.chain(left_out).collect();
        // 0 is a service that could not read its own memory.
        let service = match status.process_private_bytes {
            0 => strings.text("settings_index.running").to_owned(),
            bytes => strings.format(
                "settings_index.running_memory",
                &[&text::bytes(strings, numbers, bytes)],
            ),
        };
        IndexCaptions {
            drives,
            service,
            use_local: model
                .use_local_fix()
                .map(|fix| MessageButton::new(model, &ListButton::Fix(fix))),
            has_report: true,
            ..files
        }
    }
}

/// The text that `gozler --status` prints for the service's latest status,
/// which Show report shows; `None` while the service does not answer.
pub(crate) fn service_report(model: &Model) -> Option<String> {
    let (Some(status), None) = model.service() else {
        return None;
    };
    let mut report = Vec::new();
    gozler_core::output::write_status(status, &mut report).expect("writing to memory cannot fail");
    Some(String::from_utf8_lossy(&report).into_owned())
}

/// A dialog's title and fixed captions in the language of the moment,
/// which the dialog module sets when the dialog opens.
pub(crate) struct DialogCaptions {
    pub(crate) title: String,
    /// Each control's ID and caption.
    pub(crate) items: Vec<(u16, String)>,
    /// Each push button's ID and the icon before its caption.
    pub(crate) glyphs: Vec<(u16, Glyph)>,
}

/// The Settings pages, in their order, whose name and captions hold every
/// word of `search` at the start of one of their words, ignoring case, so
/// "upd" finds "Check for updates" and "dates" does not (DESIGN.md:
/// Settings dialog). An empty search keeps every page.
pub(crate) fn settings_pages_matching(strings: &Strings, search: &str) -> Vec<SettingsPage> {
    let terms = words(search);
    SettingsPage::ALL
        .into_iter()
        .filter(|page| {
            let table = CaptionTable::of(page.dialog());
            let page_words: Vec<String> = std::iter::once(page.label())
                .chain(table.keyed.iter().chain(table.plain).map(|(_, key)| *key))
                .flat_map(|key| words(strings.text(key)))
                .collect();
            terms.iter().all(|term| starts_a_word(&page_words, term))
        })
        .collect()
}

/// Each control of `page` with a caption, and whether that caption holds a
/// word of `search` at the start of one of its words, so the dialog shows
/// what the search found in bold (DESIGN.md: Settings dialog).
pub(crate) fn settings_page_hits(
    strings: &Strings,
    page: SettingsPage,
    search: &str,
) -> Vec<(ControlId, bool)> {
    let terms = words(search);
    let table = CaptionTable::of(page.dialog());
    table
        .keyed
        .iter()
        .chain(table.plain)
        .filter_map(|(item, key)| match item {
            DialogItem::Control(control) => {
                let caption = words(strings.text(key));
                let hit = terms.iter().any(|term| starts_a_word(&caption, term));
                Some((*control, hit))
            }
            DialogItem::Ok | DialogItem::Cancel => None,
        })
        .collect()
}

/// Whether one of `words` starts with `term`.
fn starts_a_word(words: &[String], term: &str) -> bool {
    words.iter().any(|word| word.starts_with(term))
}

/// The words of `text` in lower case: its runs of letters and digits. An
/// access key's `&` joins the letters around it, as in "f&iles", and a
/// link's `<a>` and `</a>` are not words.
fn words(text: &str) -> Vec<String> {
    text.replace('&', "")
        .replace("<a>", "")
        .replace("</a>", "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

impl DialogCaptions {
    pub(crate) fn new(dialog: DialogId, strings: &Strings) -> DialogCaptions {
        let table = CaptionTable::of(dialog);
        let captions = || table.keyed.iter().chain(table.plain);
        DialogCaptions {
            title: strings.text(table.title).to_owned(),
            items: captions()
                .map(|(item, key)| (item.id(), strings.text(key).to_owned()))
                .collect(),
            glyphs: captions()
                .filter_map(|(item, key)| Some((item.id(), button_glyph(key)?)))
                .collect(),
        }
    }

    /// The command shortcut dialog's: "Add shortcut" or, to change one of
    /// `command`'s shortcuts, "Edit shortcut", with `command` in the label.
    pub(crate) fn command_shortcut(strings: &Strings, command: &str, edit: bool) -> DialogCaptions {
        let mut captions = DialogCaptions::new(DialogId::CommandShortcut, strings);
        if edit {
            strings
                .text("command_shortcut.edit_title")
                .clone_into(&mut captions.title);
        }
        let label = ControlId::CommandShortcutLabel.id();
        for (id, text) in &mut captions.items {
            if *id == label {
                *text = strings.format("command_shortcut.shortcut_for", &[command]);
            }
        }
        captions
    }
}

impl WindowCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> WindowCaptions {
        let strings = model.strings();
        WindowCaptions {
            icons: model.choices().show_icons,
            title: title(model),
            menu_bar: commands::render(&commands::menu_bar(), strings, &model.choices().shortcuts),
            search_cue: strings.text("search.cue").to_owned(),
            clear_search: strings.text("button.clear").to_owned(),
            match_label: strings.text("match.label").to_owned(),
            match_modes: MatchMode::ALL.map(|m| strings.text(m.label()).to_owned()),
            match_case: strings.text("match.case").to_owned(),
            match_path: strings.text("match.path").to_owned(),
            filters: FilterCaptions::new(model, numbers),
            headers: model
                .columns()
                .iter()
                .map(|c| header(strings, *c))
                .collect(),
            inspector_buttons: INSPECTOR_BUTTONS.map(|button| ButtonCaption::new(strings, button)),
            copy_path: ButtonCaption::new(strings, INSPECTOR_COPY_PATH),
            copy_name: ButtonCaption::new(strings, INSPECTOR_COPY_NAME),
            copy_folder: ButtonCaption::new(strings, INSPECTOR_COPY_FOLDER),
            hide_details: ButtonCaption::new(strings, HIDE_DETAILS),
            show_details: ButtonCaption::new(strings, SHOW_DETAILS),
            list_buttons: [
                ListButton::ClearFilters,
                ListButton::SearchEverywhere,
                ListButton::ChooseFolder,
            ]
            .iter()
            .map(|button| (button.label(), false))
            .chain(Fix::label_groups().into_iter().flatten())
            .map(|(key, shielded)| (strings.text(key).to_owned(), shielded))
            .collect(),
            inspector_labels: FIELD_LABELS.map(|key| strings.text(key).to_owned()),
            inspector: InspectorCaptions::new(model, numbers),
            status: StatusCaptions::new(model, numbers),
            status_cancel: strings.text("button.cancel").to_owned(),
            status_widest: status_widest(strings, numbers),
            message: list_message(model),
        }
    }
}

impl StatusCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> StatusCaptions {
        let service = index_status(model);
        StatusCaptions {
            view: joined(
                model.strings(),
                results_status(model, numbers),
                selection_status(model, numbers),
            ),
            job: match model.job() {
                Job::Export => JobCaption {
                    text: export_status(model, numbers),
                    progress: if model.exporting() {
                        JobProgress::Unmeasured
                    } else {
                        JobProgress::None
                    },
                    cancel: model.exporting(),
                },
                Job::Fix => fix_status(model.strings(), numbers, model.fix()),
            },
            right: joined_parts(
                model.strings(),
                std::iter::once(ValuePart::plain(service.clone()))
                    .chain(totals_status(model, numbers)),
            ),
            service,
        }
    }
}

/// The labels of the inspector's fields: one entry's, in the pane's order,
/// then the several entries' Total size.
const FIELD_LABELS: [&str; 5] = [
    "inspector.folder",
    "inspector.size",
    "inspector.date_modified",
    "inspector.attributes",
    "inspector.total_size",
];

/// The rows of [`FIELD_LABELS`] that show more than a text: the age after
/// Date modified, and Attributes in two colours.
pub(crate) const MODIFIED_ROW: usize = 2;
pub(crate) const ATTRIBUTES_ROW: usize = 3;

/// What the inspector shows for the selection (DESIGN.md: Inspector pane).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InspectorCaptions {
    pub(crate) heading: String,
    /// The one selected entry's name and whether it is a folder, whose icon
    /// the heading shows.
    pub(crate) icon: Option<(String, bool)>,
    /// Each field's label and value, in the pane's order.
    pub(crate) fields: Vec<(String, String)>,
    /// The one selected entry's modification time, a FILETIME, whose
    /// [`age`] the window shows after it and keeps current.
    pub(crate) modified: Option<i64>,
    /// The one selected entry's Attributes value in parts, separators
    /// included; its field's value is their text. Empty for any other
    /// selection.
    pub(crate) attributes: Vec<ValuePart>,
    /// The gray hint under the heading, or empty.
    pub(crate) note: String,
}

/// A part of a value that the window draws in the normal or the muted text
/// colour, after its icon if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValuePart {
    pub(crate) glyph: Option<Glyph>,
    pub(crate) text: String,
    pub(crate) muted: bool,
}

impl ValuePart {
    pub(crate) fn plain(text: String) -> ValuePart {
        ValuePart {
            glyph: None,
            text,
            muted: false,
        }
    }
}

impl InspectorCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> InspectorCaptions {
        let strings = model.strings();
        let label = |key: &'static str| strings.text(key).to_owned();
        match model.selection() {
            // The number of matches, and how to see an entry's details.
            Selection::None => match model.results().shown() {
                Shown::Rows(_) => InspectorCaptions {
                    heading: strings.count("status.results", model.results().total(), numbers, &[]),
                    note: label("inspector.nothing_selected"),
                    ..InspectorCaptions::default()
                },
                Shown::Nothing | Shown::Message(_) => InspectorCaptions::default(),
            },
            Selection::Reading { .. } => {
                let reading = label("status.reading");
                InspectorCaptions {
                    heading: reading.clone(),
                    fields: vec![(label("inspector.total_size"), reading)],
                    ..InspectorCaptions::default()
                }
            }
            Selection::Files(files) => match files.as_slice() {
                [file] => entry(model, numbers, file, None),
                files => several(strings, numbers, Sums::of(files)),
            },
            Selection::Folder { folder, contents } => entry(model, numbers, folder, Some(contents)),
        }
    }
}

/// One selected entry; `contents` is a folder's.
fn entry(
    model: &Model,
    numbers: &dyn Numbers,
    entry: &ResultItem,
    contents: Option<&FolderContents>,
) -> InspectorCaptions {
    let strings = model.strings();
    let unknown = || strings.text("cell.unknown").to_owned();
    let exact = |bytes: u64| exact_size(strings, numbers, bytes);
    let name = text::name_of(&entry.path);
    let size = match contents {
        Some(FolderContents::Calculating(_)) => strings.text("inspector.calculating").to_owned(),
        Some(FolderContents::Counted(totals)) => {
            let key = if totals.volumes_incomplete || totals.metadata_pending {
                "inspector.folder_size_at_least"
            } else {
                "inspector.folder_size"
            };
            strings.count(key, totals.count, numbers, &[&exact(totals.size)])
        }
        None => entry.size.map_or_else(unknown, exact),
    };
    let plain = |text: String| vec![ValuePart::plain(text)];
    let attributes = entry.attributes.map_or_else(
        || plain(unknown()),
        |attributes| {
            let mut parts = Vec::new();
            for letter in attributes.letters().chars() {
                let Some(key) = attribute_word(letter) else {
                    continue;
                };
                if !parts.is_empty() {
                    parts.extend(plain(", ".to_owned()));
                }
                // Gray for the entries that File Explorer hides by default.
                parts.push(ValuePart {
                    glyph: None,
                    text: strings.text(key).to_owned(),
                    muted: matches!(letter, 'H' | 'S'),
                });
            }
            if parts.is_empty() {
                plain(strings.text("inspector.no_attributes").to_owned())
            } else {
                parts
            }
        },
    );
    let values = [
        gozler_core::winpath::parent_of(&entry.path),
        size,
        entry
            .mtime_ft
            .map_or_else(unknown, |ft| numbers.date_time_seconds(ft)),
        attributes.iter().map(|part| part.text.as_str()).collect(),
    ];
    InspectorCaptions {
        heading: name.to_owned(),
        icon: Some((name.to_owned(), entry.is_dir)),
        fields: FIELD_LABELS
            .iter()
            .zip(values)
            .map(|(key, value)| (strings.text(key).to_owned(), value))
            .collect(),
        modified: entry.mtime_ft,
        attributes,
        note: String::new(),
    }
}

/// How long ago the FILETIME `modified` was at `now`, or how long until it
/// comes, after a "·": in the largest whole unit, rounded down, or "just
/// now" under a minute (DESIGN.md: Inspector pane). A month is 30 days, so
/// 360 to 364 days read 11 months, under a year.
pub(crate) fn age(strings: &Strings, numbers: &dyn Numbers, modified: i64, now: i64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    let seconds = now.abs_diff(modified) / 10_000_000;
    let (ago, ahead, count) = match seconds {
        ..MINUTE => return strings.format("inspector.age", &[strings.text("age.just_now")]),
        MINUTE..HOUR => ("age.minutes_ago", "age.in_minutes", seconds / MINUTE),
        HOUR..DAY => ("age.hours_ago", "age.in_hours", seconds / HOUR),
        DAY..MONTH => ("age.days_ago", "age.in_days", seconds / DAY),
        MONTH..YEAR => ("age.months_ago", "age.in_months", (seconds / MONTH).min(11)),
        YEAR.. => ("age.years_ago", "age.in_years", seconds / YEAR),
    };
    let key = if modified > now { ahead } else { ago };
    strings.format("inspector.age", &[&strings.count(key, count, numbers, &[])])
}

/// Several selected entries: how many files and folders, and the size of
/// the files.
fn several(strings: &Strings, numbers: &dyn Numbers, sums: Sums) -> InspectorCaptions {
    let files = strings.count("inspector.files", sums.files, numbers, &[]);
    let folders = strings.count("inspector.folders", sums.folders, numbers, &[]);
    let heading = match (sums.files, sums.folders) {
        (_, 0) => files,
        (0, _) => folders,
        _ => strings.format("inspector.files_and_folders", &[&files, &folders]),
    };
    let total = if sums.files == 0 {
        strings.text("inspector.no_files").to_owned()
    } else {
        let size = exact_size(strings, numbers, sums.bytes);
        if sums.unknown == 0 {
            size
        } else {
            strings.count("inspector.unknown", sums.unknown, numbers, &[&size])
        }
    };
    InspectorCaptions {
        heading,
        fields: vec![(strings.text("inspector.total_size").to_owned(), total)],
        ..InspectorCaptions::default()
    }
}

/// A size as the inspector shows it, with its exact bytes, as File
/// Explorer's Properties does: "1.20 KB (1,234 bytes)", or "16 bytes" once
/// when the two read the same.
fn exact_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64) -> String {
    let size = text::bytes(strings, numbers, bytes);
    let exact = strings.count("size.bytes", bytes, numbers, &[]);
    if size == exact {
        size
    } else {
        strings.format("inspector.size_exact", &[&size, &exact])
    }
}

/// The catalogue key of an attribute letter's word, as `attrib:` names the
/// letters. D has none, because the icon already shows a folder.
fn attribute_word(letter: char) -> Option<&'static str> {
    Some(match letter {
        'R' => "attribute.r",
        'H' => "attribute.h",
        'S' => "attribute.s",
        'A' => "attribute.a",
        'V' => "attribute.v",
        'X' => "attribute.x",
        'N' => "attribute.n",
        'T' => "attribute.t",
        'L' => "attribute.l",
        'C' => "attribute.c",
        'O' => "attribute.o",
        'I' => "attribute.i",
        'E' => "attribute.e",
        'U' => "attribute.u",
        'P' => "attribute.p",
        'M' => "attribute.m",
        _ => return None,
    })
}

/// The Settings dialog's status bar's left group: the dialog and its shown
/// page, or the dialog alone while the search shows no page.
pub(crate) fn settings_status_view(strings: &Strings, page: Option<SettingsPage>) -> String {
    match page {
        Some(page) => strings.format("settings.status_view", &[strings.text(page.label())]),
        None => strings.text("settings.title").to_owned(),
    }
}

/// The Settings dialog's status bar's widest texts: the main window's, with
/// the dialog's own left group.
pub(crate) fn settings_status_widest(strings: &Strings, numbers: &dyn Numbers) -> StatusWidest {
    let view = std::iter::once(None)
        .chain(SettingsPage::ALL.map(Some))
        .map(|page| settings_status_view(strings, page))
        .collect();
    StatusWidest {
        view,
        ..status_widest(strings, numbers)
    }
}

/// A release's version at its widest, which the status bar's middle group
/// is sized for.
const WIDEST_VERSION: &str = "10.10.10";

/// The texts a fix shows in the status bar's middle group while it runs,
/// and once it ended, which the group is sized for. Windows' words for a
/// failure have no widest form, and a development build's instructions
/// would widen the window for a case only developers meet, so a long one
/// is cut off, as an exported file's long name is, and its tooltip shows
/// it whole.
fn fix_widest(strings: &Strings, numbers: &dyn Numbers, sizes: &[u64]) -> [Vec<String>; 2] {
    let version = || WIDEST_VERSION.to_owned();
    let commands = |fix, state| Fixer::Commands { fix, state };
    let setup = |state| Fixer::Setup {
        fix: SetupFix::Update,
        state,
    };
    let fixes = [CommandFix::Start, CommandFix::Install, CommandFix::UseLocal];
    let mut running: Vec<Fixer> = fixes
        .map(|fix| commands(fix, FixState::Running(CommandStep::RunningCommands)))
        .into_iter()
        .chain([
            commands(
                CommandFix::Start,
                FixState::Running(CommandStep::AskingWindows),
            ),
            commands(
                CommandFix::Start,
                FixState::Running(CommandStep::WaitingForService),
            ),
            setup(FixState::Running(SetupStep::FindingRelease)),
            setup(FixState::Running(SetupStep::CheckingFile {
                version: version(),
            })),
            setup(FixState::Running(SetupStep::StartingSetup {
                version: version(),
                path: PathBuf::new(),
            })),
        ])
        .collect();
    running.extend(sizes.iter().map(|&size| {
        setup(FixState::Running(SetupStep::Downloading {
            version: version(),
            bytes: size,
            size,
        }))
    }));
    let failed = CommandResult::CommandsFailed {
        code: u32::MAX,
        said: None,
    };
    let ended: Vec<Fixer> = fixes
        .map(|fix| commands(fix, FixState::Ended(failed.clone())))
        .into_iter()
        .chain(
            [
                CommandResult::Done,
                CommandResult::Cancelled,
                CommandResult::Refused,
                CommandResult::NoAnswer,
            ]
            .map(|result| commands(CommandFix::Start, FixState::Ended(result))),
        )
        .chain(
            [
                SetupResult::Cancelled,
                SetupResult::DownloadFailed,
                SetupResult::Damaged,
            ]
            .map(|result| setup(FixState::Ended(result))),
        )
        .collect();
    [running, ended].map(|fixes| {
        fixes
            .iter()
            .map(|fix| fix_status(strings, numbers, fix).text)
            .collect()
    })
}

fn status_widest(strings: &Strings, numbers: &dyn Numbers) -> StatusWidest {
    let limit = numbers.integer(LIST_LIMIT);
    let showing = strings.format("status.showing", &[&limit, &numbers.integer(WIDEST_TOTAL)]);
    // Which size is widest depends on the font and the language, so every
    // unit's widest forms are measured: 1,023 bytes, then a three-digit size
    // with two, one and no decimals in each unit.
    let sizes: Vec<u64> =
        std::iter::once(1023)
            .chain((1..=4).flat_map(|unit| {
                [9.99, 99.9, 999.0].map(|value| (value * 1024f64.powi(unit)) as u64)
            }))
            .collect();
    let mut selected: Vec<String> = sizes
        .iter()
        .map(|&n| {
            strings.format(
                "status.selected",
                &[&limit, &text::bytes(strings, numbers, n)],
            )
        })
        .collect();
    selected.push(strings.format("status.selected_only", &[&limit]));
    selected.push(strings.text("status.reading").to_owned());
    let results = [
        strings.format("status.filtered", &[&showing]),
        strings.text("status.searching").to_owned(),
    ];
    let [fix_running, fix_ended] = fix_widest(strings, numbers, &sizes);
    StatusWidest {
        view: results
            .iter()
            .flat_map(|results| {
                selected
                    .iter()
                    .map(|selection| joined(strings, results.clone(), selection.clone()))
            })
            .collect(),
        running: std::iter::once(strings.count("status.exporting", WIDEST_TOTAL, numbers, &[]))
            .chain(fix_running)
            .collect(),
        done: std::iter::once(strings.count(
            "status.exported",
            WIDEST_TOTAL,
            numbers,
            &["results.csv"],
        ))
        .chain(fix_ended)
        .collect(),
    }
}

/// The window's title: the typed search and the product's name, as Windows
/// titles a document window, or the name alone while the search is empty.
pub(crate) fn title(model: &Model) -> String {
    let strings = model.strings();
    let text = model.text.trim();
    if text.is_empty() {
        strings.text("window.title").to_owned()
    } else {
        strings.format("window.title_search", &[text])
    }
}

/// A column's header.
pub(crate) fn header(strings: &Strings, column: Column) -> String {
    strings.text(column.header()).to_owned()
}

/// Two texts of a status bar group with a dot between them, or the one
/// that is not empty, such as the results and the selection.
fn joined(strings: &Strings, first: String, second: String) -> String {
    if second.is_empty() {
        first
    } else if first.is_empty() {
        second
    } else {
        strings.format("status.view", &[&first, &second])
    }
}

/// `parts` with the dot of [`joined`] between them, and the empty ones left
/// out.
fn joined_parts(strings: &Strings, parts: impl Iterator<Item = ValuePart>) -> Vec<ValuePart> {
    let dot = strings.format("status.view", &["", ""]);
    let mut joined = Vec::new();
    for part in parts.filter(|part| !part.text.is_empty()) {
        if !joined.is_empty() {
            joined.push(ValuePart::plain(dot.clone()));
        }
        joined.push(part);
    }
    joined
}

/// The shown search's files and folders, each kind after its icon and with
/// the space it takes, and a kind no match is left out; none while a search
/// runs, or when the service sent no sums (DESIGN.md: Status bar).
fn totals_status(model: &Model, numbers: &dyn Numbers) -> Vec<ValuePart> {
    let strings = model.strings();
    let results = model.results();
    let Some(sums) = results.sums().filter(|_| !results.searching()) else {
        return Vec::new();
    };
    let kind = |glyph: Glyph, key: &str, count: u64, bytes: Option<u64>| {
        let size = bytes.map_or_else(
            || strings.text("status.size_unknown").to_owned(),
            |bytes| text::bytes(strings, numbers, bytes),
        );
        ValuePart {
            glyph: Some(glyph),
            text: strings.count(key, count, numbers, &[&size]),
            muted: false,
        }
    };
    [
        (Glyph::Page, "status.files", sums.files, sums.file_bytes),
        (
            Glyph::Folder,
            "status.folders",
            sums.folders,
            sums.folder_bytes,
        ),
    ]
    .into_iter()
    .filter(|(_, _, count, _)| *count > 0)
    .map(|(glyph, key, count, bytes)| kind(glyph, key, count, bytes))
    .collect()
}

/// How many results, or that a search runs. A filter that the hidden pane
/// cannot show adds "· Filtered" (DESIGN.md: S10).
pub(crate) fn results_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    let results = model.results();
    if results.searching() {
        return strings.text("status.searching").to_owned();
    }
    match results.shown() {
        Shown::Rows(_) => {
            let total = results.total();
            let count = if total > LIST_LIMIT {
                strings.format(
                    "status.showing",
                    &[&numbers.integer(LIST_LIMIT), &numbers.integer(total)],
                )
            } else {
                strings.count("status.results", total, numbers, &[])
            };
            if model.filters().is_set() && !model.panes().filters {
                strings.format("status.filtered", &[&count])
            } else {
                count
            }
        }
        Shown::Nothing | Shown::Message(_) => String::new(),
    }
}

/// How many entries are selected and the size of the selected files, or
/// that the selection is read.
pub(crate) fn selection_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.selection() {
        Selection::None => String::new(),
        Selection::Reading { .. } => strings.text("status.reading").to_owned(),
        Selection::Files(_) | Selection::Folder { .. } => {
            let sums = Sums::of(model.selected_files().unwrap_or_default());
            let count = numbers.integer(sums.entries());
            // The size counts files only, so folders alone have none.
            if sums.files == 0 {
                strings.format("status.selected_only", &[&count])
            } else {
                strings.format(
                    "status.selected",
                    &[&count, &text::bytes(strings, numbers, sums.bytes)],
                )
            }
        }
    }
}

/// The export's progress, or the last export's result.
pub(crate) fn export_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.export() {
        Export::Idle => String::new(),
        Export::Running { rows, .. } => strings.count("status.exporting", *rows, numbers, &[]),
        Export::Done { path, rows } => {
            let path = path.to_string_lossy();
            strings.count("status.exported", *rows, numbers, &[text::name_of(&path)])
        }
    }
}

/// A fix's step, with its progress and whether Cancel stops it, or its
/// result (DESIGN.md: Service fixes).
fn fix_status(strings: &Strings, numbers: &dyn Numbers, fix: &Fixer) -> JobCaption {
    let ended = |text: String| JobCaption {
        text,
        progress: JobProgress::None,
        cancel: false,
    };
    let running = |(text, progress), cancel| JobCaption {
        text,
        progress,
        cancel,
    };
    match fix {
        Fixer::Idle => ended(String::new()),
        Fixer::Commands {
            fix,
            state: FixState::Running(step),
        } => running(command_step(strings, *fix, step), step.can_cancel()),
        Fixer::Commands {
            fix,
            state: FixState::Ended(result),
        } => ended(command_result(strings, *fix, result)),
        Fixer::Setup {
            state: FixState::Running(step),
            ..
        } => running(setup_step(strings, numbers, step), step.can_cancel()),
        Fixer::Setup {
            state: FixState::Ended(result),
            ..
        } => ended(setup_result(strings, result)),
    }
}

fn command_step(strings: &Strings, fix: CommandFix, step: &CommandStep) -> (String, JobProgress) {
    let text = |key: &'static str| strings.text(key).to_owned();
    match step {
        CommandStep::AskingWindows => (text("fixing.asking"), JobProgress::None),
        CommandStep::RunningCommands => {
            let key = match fix {
                CommandFix::Start => "fixing.starting",
                CommandFix::Install => "fixing.installing",
                CommandFix::UseLocal => "fixing.updating",
            };
            (text(key), JobProgress::Unmeasured)
        }
        CommandStep::WaitingForService => (text("fixing.waiting"), JobProgress::Unmeasured),
    }
}

fn setup_step(strings: &Strings, numbers: &dyn Numbers, step: &SetupStep) -> (String, JobProgress) {
    let text = |key: &'static str| strings.text(key).to_owned();
    match step {
        SetupStep::FindingRelease => (text("fixing.finding"), JobProgress::Unmeasured),
        SetupStep::Confirming { .. } => (text("fixing.finding"), JobProgress::None),
        SetupStep::Downloading {
            version,
            bytes,
            size,
        } => (
            strings.format(
                "fixing.downloading",
                &[
                    version,
                    &text::bytes(strings, numbers, *bytes),
                    &text::bytes(strings, numbers, *size),
                ],
            ),
            JobProgress::Measured {
                done: *bytes,
                total: *size,
            },
        ),
        SetupStep::CheckingFile { .. } => (text("fixing.checking"), JobProgress::Unmeasured),
        SetupStep::StartingSetup { version, .. } => (
            strings.format("fixing.closing", &[version]),
            JobProgress::None,
        ),
    }
}

fn command_result(strings: &Strings, fix: CommandFix, result: &CommandResult) -> String {
    let text = |key: &'static str| strings.text(key).to_owned();
    match result {
        CommandResult::Done => text("fixing.done"),
        CommandResult::Cancelled => text("fixing.cancelled"),
        CommandResult::Refused => text("fixing.refused"),
        CommandResult::Failed(why) => strings.format("fixing.failed", &[why]),
        CommandResult::CommandsFailed { code, said } => {
            let reason = said
                .clone()
                .unwrap_or_else(|| strings.format("fixing.exit_code", &[&code.to_string()]));
            let key = match fix {
                CommandFix::Start => "fixing.start_failed",
                CommandFix::Install => "fixing.install_failed",
                CommandFix::UseLocal => "fixing.update_failed",
            };
            strings.format(key, &[&reason])
        }
        CommandResult::NoAnswer => text("fixing.no_answer"),
    }
}

fn setup_result(strings: &Strings, result: &SetupResult) -> String {
    let text = |key: &'static str| strings.text(key).to_owned();
    match result {
        SetupResult::Cancelled => text("fixing.download_cancelled"),
        // The person said no, or a message box said why there is no
        // setup, so the status bar adds nothing.
        SetupResult::Declined | SetupResult::NoSetup => String::new(),
        SetupResult::DownloadFailed => text("fixing.download_failed"),
        SetupResult::NotSaved { folder, why } => {
            strings.format("fixing.not_saved", &[&folder.display().to_string(), why])
        }
        SetupResult::Damaged => text("fixing.damaged"),
        SetupResult::Failed(why) => strings.format("fixing.setup_failed", &[why]),
    }
}

/// A task dialog's question (DESIGN.md: Service fixes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Question {
    /// The dialog's title, which names the command, as Windows' own
    /// dialogs do.
    pub(crate) title: String,
    pub(crate) instruction: String,
    pub(crate) body: QuestionBody,
    /// A line below the buttons, beside an information icon.
    pub(crate) footer: Option<String>,
    pub(crate) confirm: String,
    pub(crate) cancel: String,
    pub(crate) guard: Guard,
}

/// How a question guards its confirm button against an `Enter` pressed
/// without reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Guard {
    /// The confirm button is the default.
    None,
    /// Cancel is the default, because the confirm button cannot be undone,
    /// such as a permanent delete. The words carry the weight, as Fluent 2
    /// asks, so the dialog shows no icon.
    CancelFirst,
    /// Cancel is the default and a warning icon shows, because the confirm
    /// button may be regretted, such as a downgrade.
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuestionBody {
    Text(String),
    /// Lines of a label and a value, whose values the dialog lines up.
    Lines(Vec<(String, String)>),
}

/// The question for the setup the fix asks about, while it asks.
pub(crate) fn setup_question(model: &Model, numbers: &dyn Numbers) -> Option<Question> {
    let strings = model.strings();
    let (fix, setup) = model.fix().confirming()?;
    let size = text::bytes(strings, numbers, setup.size);
    let cancel = strings.text("button.cancel").to_owned();
    if fix == SetupFix::Download {
        return Some(Question {
            title: strings.text("window.title").to_owned(),
            instruction: strings.format("fixing.install_question", &[&setup.version]),
            body: QuestionBody::Text(strings.format("fixing.install_details", &[&size])),
            footer: None,
            confirm: strings.text("fixing.install").to_owned(),
            cancel,
            guard: Guard::None,
        });
    }
    let [title, question, confirm] = if setup.older {
        [
            "fixing.downgrade_caption",
            "fixing.downgrade_question",
            "fixing.downgrade",
        ]
    } else {
        [
            "fixing.update_caption",
            "fixing.update_question",
            "fixing.update",
        ]
    };
    Some(Question {
        title: strings.text(title).to_owned(),
        instruction: strings.format(question, &[&setup.version]),
        body: QuestionBody::Lines(vec![label_and_value(
            strings,
            "fixing.installed_version",
            gozler_core::escompat::version(),
        )]),
        footer: Some(strings.format("fixing.download_size", &[&size])),
        confirm: strings.text(confirm).to_owned(),
        cancel,
        guard: if setup.older {
            Guard::Warning
        } else {
            Guard::None
        },
    })
}

/// The question before Use local switches the service to the gozlerd.exe
/// `local` beside the window (DESIGN.md: Service fixes).
pub(crate) fn use_local_question(strings: &Strings, local: &LocalService) -> Question {
    let folder = local.program.parent().unwrap_or(&local.program);
    let installed = local
        .installed
        .as_ref()
        .map(|installed| label_and_value(strings, "fixing.installed_version", installed));
    let compatible = label_and_value(strings, "fixing.compatible_version", &local.version);
    Question {
        title: strings.text("fixing.use_local_caption").to_owned(),
        instruction: strings.text("fixing.use_local_question").to_owned(),
        body: QuestionBody::Lines(installed.into_iter().chain([compatible]).collect()),
        footer: Some(strings.format("fixing.found_in", &[&folder.display().to_string()])),
        confirm: strings.text("fixing.switch").to_owned(),
        cancel: strings.text("button.cancel").to_owned(),
        guard: Guard::None,
    }
}

/// The status bar's right group: the service's failure while the list
/// shows rows or another message, so the window says that the service
/// stopped also when it keeps its rows (DESIGN.md: T10); else the index
/// state, only when it affects the answer (DESIGN.md: Status bar); else a
/// newer release that Check for updates found (DESIGN.md: The General
/// page).
pub(crate) fn index_status(model: &Model) -> String {
    let strings = model.strings();
    if let (_, Some(failure)) = model.service()
        && !matches!(
            model.results().shown(),
            Shown::Message(ListMessage::Failure(_))
        )
    {
        return failure_text(strings, failure);
    }
    let coverage = model.coverage();
    if let Some(volume) = coverage.incomplete() {
        let key = match volume.phase {
            VolumePhase::Live | VolumePhase::Snapshot => {
                unreachable!("an incomplete volume is neither live nor a snapshot")
            }
            VolumePhase::Bootstrapping | VolumePhase::Rescanning => "status.volume_indexing",
            VolumePhase::Offline => "status.volume_offline",
            VolumePhase::Failed { .. } => "status.volume_failed",
        };
        return strings.format(key, &[volume.mounted().display_prefix()]);
    }
    coverage
        .sizes_or_dates_unknown()
        .map(|volume| strings.format("status.metadata", &[volume.mounted().display_prefix()]))
        .or_else(|| {
            model
                .newer_release()
                .map(|version| update_text(strings, version))
        })
        .unwrap_or_default()
}

fn update_text(strings: &Strings, version: &str) -> String {
    strings.format("status.update_available", &[version])
}

/// Whether the status bar's right group names a newer release, so a click
/// on it runs Update (DESIGN.md: The General page).
pub(crate) fn shows_update(model: &Model) -> bool {
    model
        .newer_release()
        .is_some_and(|version| index_status(model) == update_text(model.strings(), version))
}

/// What the list's empty area says instead of rows, if anything.
pub(crate) fn list_message(model: &Model) -> Option<ListCaption> {
    let strings = model.strings();
    let text = match model.results().shown() {
        Shown::Rows(_) if model.results().total() == 0 => {
            strings.text("list.no_results").to_owned()
        }
        Shown::Nothing | Shown::Rows(_) => return None,
        Shown::Message(ListMessage::Parse(error)) => query_error(strings, error),
        Shown::Message(ListMessage::MissingFolder(folder)) => {
            strings.format("list.missing_folder", &[folder])
        }
        Shown::Message(ListMessage::Failure(failure)) => failure_text(strings, failure),
        Shown::Message(ListMessage::TypeToSearch) => strings.text("list.type_to_search").to_owned(),
    };
    let buttons = model
        .list_buttons()
        .iter()
        .map(|button| MessageButton::new(model, button))
        .collect();
    Some(ListCaption { text, buttons })
}

/// The label and the value of the line `key`, such as "Installed version:
/// {0}" with `value` in place of `{0}`, for a dialog that lines up its
/// values. The label leaves out the space before the value.
fn label_and_value(strings: &Strings, key: &str, value: &str) -> (String, String) {
    let line = strings.text(key);
    let (label, after) = line.split_once("{0}").unwrap_or((line, ""));
    (label.trim_end().to_owned(), format!("{value}{after}"))
}

/// The tooltip of `fix`'s button, if it has one: Use local's names the
/// version it puts in place and, on its own line, the program it copies.
fn fix_tooltip(strings: &Strings, fix: &Fix) -> Option<String> {
    match fix {
        Fix::UseLocal(local) => Some(format!(
            "{}\r\n{}",
            strings.format("fix.use_local_tooltip", &[&local.version]),
            local.program.display()
        )),
        Fix::Repair(_) | Fix::Update | Fix::OpenServices => None,
    }
}

/// What a failure of the service means for the person (DESIGN.md: Daemon
/// failures).
pub(crate) fn failure_text(strings: &Strings, failure: &DaemonFailure) -> String {
    match failure {
        DaemonFailure::NotRunning(why) => match why {
            NotRunning::Stopped => strings.text("failure.stopped").to_owned(),
            NotRunning::Disabled => strings.text("failure.disabled").to_owned(),
            NotRunning::NotInstalled { .. } => strings.text("failure.not_installed").to_owned(),
            NotRunning::Unknown(detail) => strings.format("failure.unknown", &[detail]),
        },
        DaemonFailure::NotReady => strings.text("failure.not_ready").to_owned(),
        DaemonFailure::NotGozler => strings.text("failure.not_gozler").to_owned(),
        DaemonFailure::TooOld { .. } => strings.text("failure.too_old").to_owned(),
        DaemonFailure::Refused { detail } => strings.format("failure.refused", &[detail]),
        DaemonFailure::Lost => strings.text("failure.lost").to_owned(),
    }
}

/// What is wrong with a search that does not parse, and how to fix it, in
/// the person's language; text from a library stays English after it.
fn query_error(strings: &Strings, error: &QueryError) -> String {
    let as_typed = |function: &str, value: &str| format!("{function}{value}");
    match error {
        QueryError::UnsupportedOperator(c) => {
            strings.format("query.unsupported_operator", &[&c.to_string()])
        }
        QueryError::UnknownFunction(name) => {
            strings.format("query.unknown_function", &[&format!("{name}:")])
        }
        QueryError::RelativePath(path) => strings.format("query.relative_path", &[path]),
        QueryError::NoQueryStart => strings.text("query.no_query_start").to_owned(),
        QueryError::TooManyTerms(n) => strings.format("query.too_many_terms", &[&n.to_string()]),
        QueryError::TooManyRegexes(n) => {
            strings.format("query.too_many_regexes", &[&n.to_string()])
        }
        QueryError::Malformed(reason) => match reason {
            Malformed::MisplacedNot => strings.text("query.misplaced_not").to_owned(),
            Malformed::NotWithoutTerm => strings.text("query.not_without_term").to_owned(),
            Malformed::OrWithoutTerms => strings.text("query.or_without_terms").to_owned(),
            Malformed::OperatorOnNothing(term) => {
                strings.format("query.operator_on_nothing", &[term])
            }
            Malformed::Size(value) => strings.format("query.size", &[&as_typed("size:", value)]),
            Malformed::ReversedDates(value) => {
                strings.format("query.reversed_dates", &[&as_typed("dm:", value)])
            }
            Malformed::DateOutOfRange(value) => {
                strings.format("query.date_out_of_range", &[&as_typed("dm:", value)])
            }
            Malformed::Days(value) => strings.format("query.days", &[&as_typed("dm:", value)]),
            Malformed::Date(value) => strings.format("query.date", &[&as_typed("dm:", value)]),
            Malformed::NoParentFolder => strings.text("query.no_parent_folder").to_owned(),
            Malformed::AttributeLetter(c) => {
                strings.format("query.attribute_letter", &[&c.to_string()])
            }
            Malformed::Regex(detail) => strings.format("query.regex", &[detail]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::text::{LANGUAGES, Strings, access_key, parse_catalogue};
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    /// An access key that two controls of a dialog share focuses the first
    /// only, and a label without one leaves its control out of reach of the
    /// keyboard (DESIGN.md: Words). Every shipped language chooses its own
    /// letters.
    #[test]
    fn every_dialog_label_has_its_own_access_key() {
        let keyed = |dialog: DialogId| CaptionTable::of(dialog).keyed;
        for language in LANGUAGES {
            let strings = Strings::load(language);
            for dialog in DialogId::ALL {
                let mut keys = HashSet::new();
                let shared = CaptionTable::shares_keys_with(dialog).map_or(&[][..], keyed);
                for (_, key) in shared.iter().chain(keyed(dialog)) {
                    let caption = strings.text(key);
                    assert!(
                        access_key(caption).is_some_and(|k| keys.insert(k)),
                        "{}: {caption} in {dialog:?} has no access key, or one the dialog \
                         already uses",
                        language.tag
                    );
                }
                // The Service group shows the fix buttons that show
                // together.
                if dialog == DialogId::SettingsIndex {
                    for shown in Fix::label_groups() {
                        let mut keys = keys.clone();
                        for (key, _) in shown {
                            let caption = strings.text(key);
                            assert!(
                                access_key(caption).is_some_and(|k| keys.insert(k)),
                                "{}: {caption} on the Index page has no access key, or one \
                                 the dialog already uses",
                                language.tag
                            );
                        }
                    }
                }
            }
        }
    }

    /// A search that matched inside a word would keep pages the person did
    /// not mean, and one that needed the words in order would drop pages
    /// they did.
    #[test]
    fn a_settings_search_matches_the_start_of_words() {
        let strings = Strings::load(&LANGUAGES[0]);
        let pages = |search| settings_pages_matching(&strings, search);
        assert_eq!(pages("UPD"), [SettingsPage::General]);
        assert!(pages("pdates").is_empty());
        assert_eq!(pages("cont ite"), [SettingsPage::ContextMenu]);
        assert_eq!(pages(" "), SettingsPage::ALL);
        let bold: Vec<ControlId> = settings_page_hits(&strings, SettingsPage::General, "upd start")
            .into_iter()
            .filter_map(|(control, hit)| hit.then_some(control))
            .collect();
        assert_eq!(
            bold,
            [
                ControlId::GeneralSignIn,
                ControlId::GeneralUpdates,
                ControlId::GeneralStarting
            ]
        );
    }

    /// An attribute letter without a word is left out of the inspector, and
    /// no compiler sees a letter that `gozler-core` adds to its table.
    #[test]
    fn every_attribute_letter_but_d_has_its_word() {
        let strings = Strings::load(&LANGUAGES[0]);
        for letter in gozler_core::attributes::Attributes::all_letters().chars() {
            let key = attribute_word(letter);
            assert!(
                letter == 'D' || key.is_some_and(|key| strings.text(key) != key),
                "{letter} has no word in en.json"
            );
        }
    }

    /// A key that `en.json` lacks shows its dotted name on screen, and only
    /// the code that builds that caption shows it, so a typo in a rare
    /// message reaches a person before anyone sees it.
    #[test]
    fn every_catalogue_key_in_the_code_is_in_english() {
        let entries = parse_catalogue(LANGUAGES[0].catalogue).expect("en.json parses");
        let keys: HashSet<String> = entries
            .iter()
            .flat_map(|(key, _)| {
                let base = key
                    .strip_suffix("_one")
                    .or_else(|| key.strip_suffix("_other"));
                [Some(key.clone()), base.map(str::to_owned)]
            })
            .flatten()
            .collect();
        let groups: HashSet<&str> = keys
            .iter()
            .filter_map(|key| key.split_once('.').map(|(group, _)| group))
            .collect();
        let mut missing = Vec::new();
        let mut folders = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("src is readable") {
                let path = entry.expect("src is readable").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let code = std::fs::read_to_string(&path).expect("the source is UTF-8");
                // Every second piece between quotes is a string literal, or
                // close enough: a key has no quote, space or backslash.
                for literal in code.split('"').skip(1).step_by(2) {
                    let Some((group, name)) = literal.split_once('.') else {
                        continue;
                    };
                    let is_key = |part: &str| {
                        !part.is_empty()
                            && part.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                    };
                    if groups.contains(group)
                        && is_key(group)
                        && is_key(name)
                        && !keys.contains(literal)
                    {
                        missing.push(format!("{}: {literal}", path.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "en.json lacks: {missing:#?}");
    }
}
