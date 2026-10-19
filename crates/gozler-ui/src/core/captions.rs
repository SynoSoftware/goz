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

use super::commands::{self, INSPECTOR_BUTTONS, MenuNode};
use super::fetch::DaemonFailure;
use super::ids::{ControlId, DialogId};
use super::layout::WIDEST_TOTAL;
use super::model::{Column, Export, Model};
use super::query::{
    ATTRIBUTES, AttributeChoice, DateModified, DateRange, Day, Kind, LocationItem, MatchMode,
    RangeItem, SizeFilter, SizeRange, SizeUnit, SizeWord, date_items, size_items,
};
use super::results::{LIST_LIMIT, ListMessage, Shown};
use super::selection::{FolderContents, Selection, Sums};
use super::text::{self, Numbers, Strings};
use gozler_client::NotRunning;
use gozler_core::proto::ResultItem;
use gozler_core::query::{Malformed, QueryError};
use gozler_core::types::VolumePhase;

/// The main window's captions, in the language and regional format of the
/// moment they are built.
pub(crate) struct WindowCaptions {
    pub(crate) title: String,
    pub(crate) menu_bar: Vec<MenuNode>,
    pub(crate) search_cue: String,
    pub(crate) match_label: String,
    /// In [`MatchMode::ALL`]'s order.
    pub(crate) match_modes: [String; 5],
    pub(crate) match_case: String,
    pub(crate) match_path: String,
    pub(crate) filters: FilterCaptions,
    /// In [`Model::columns`]' order.
    pub(crate) headers: Vec<String>,
    /// The inspector's Open, Open file location and Copy as path.
    pub(crate) inspector_buttons: [String; 3],
    /// Every label an inspector field can have, which the layout sizes the
    /// label column for, so the values keep their place in every state.
    pub(crate) inspector_labels: [String; 8],
    pub(crate) inspector: InspectorCaptions,
    pub(crate) status: StatusCaptions,
    pub(crate) message: Option<ListCaption>,
    /// The texts each fixed status part can show at its widest, which the
    /// layout sizes the part for, so a part never moves with its text
    /// (DESIGN.md: Look).
    pub(crate) status_widest: [Vec<String>; 3],
}

/// The status bar's four parts (DESIGN.md: Status bar).
pub(crate) struct StatusCaptions {
    pub(crate) results: String,
    pub(crate) selection: String,
    pub(crate) export: String,
    pub(crate) index: String,
}

/// What the list's empty area says instead of rows: the message, and the
/// labels of its buttons, in [`Model::list_buttons`]' order.
pub(crate) type ListCaption = (String, Vec<String>);

/// The filter pane's captions and the items of its lists, in the lists'
/// orders.
pub(crate) struct FilterCaptions {
    pub(crate) clear: String,
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
            clear: text("command.clear_filters"),
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
/// chosen folder.
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
                keyed: &[(Control(C::SettingsApply), "settings.apply")],
                plain: &[(Ok, "button.ok"), (Cancel, "button.cancel")],
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
                    (
                        Control(C::GeneralSearchWith),
                        "settings_general.search_with",
                    ),
                    (
                        Control(C::GeneralOpenFoldersLabel),
                        "settings_general.open_folders",
                    ),
                    (
                        Control(C::GeneralOpenFilesLabel),
                        "settings_general.open_files",
                    ),
                    (
                        Control(C::GeneralOpenLocationLabel),
                        "settings_general.open_file_location",
                    ),
                    (
                        Control(C::GeneralTotalCommander),
                        "settings_general.total_commander",
                    ),
                    (Control(C::GeneralRestore), "settings.restore_defaults"),
                ],
                plain: &[
                    (Control(C::GeneralHeading), "settings.general"),
                    (Control(C::GeneralStarting), "settings_general.starting"),
                    (
                        Control(C::GeneralFileExplorer),
                        "settings_general.file_explorer",
                    ),
                    // One letter cannot name three buttons (DESIGN.md: The
                    // General page).
                    (
                        Control(C::GeneralOpenFoldersBrowse),
                        "settings_general.browse",
                    ),
                    (
                        Control(C::GeneralOpenFilesBrowse),
                        "settings_general.browse",
                    ),
                    (
                        Control(C::GeneralOpenLocationBrowse),
                        "settings_general.browse",
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
                    (Control(C::SearchRestore), "settings.restore_defaults"),
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
                    (Control(C::KeyboardRestore), "settings.restore_defaults"),
                ],
                plain: &[
                    (Control(C::KeyboardHeading), "settings.keyboard"),
                    (
                        Control(C::KeyboardShowGroup),
                        "settings_keyboard.show_gozler",
                    ),
                    (
                        Control(C::KeyboardCommandsGroup),
                        "settings_keyboard.commands",
                    ),
                ],
            },
            DialogId::SettingsIndex => CaptionTable {
                title: "settings.index",
                keyed: &[],
                plain: &[
                    (Control(C::IndexHeading), "settings.index"),
                    (Control(C::IndexDrivesGroup), "settings_index.drives"),
                    (Control(C::IndexHoldsGroup), "settings_index.holds"),
                    (Control(C::IndexHolds), "settings_index.holds_text"),
                    (Control(C::IndexServiceGroup), "settings_index.service"),
                    (Control(C::IndexStatusHint), "settings_index.status_hint"),
                ],
            },
            DialogId::CommandShortcut => CaptionTable {
                title: "command_shortcut.title",
                keyed: &[(Control(C::CommandShortcutLabel), "command_shortcut.press")],
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
            | DialogId::SettingsKeyboard
            | DialogId::SettingsIndex => Some(DialogId::Settings),
            DialogId::About
            | DialogId::SearchSyntax
            | DialogId::CustomDate
            | DialogId::CustomSize
            | DialogId::Settings
            | DialogId::CommandShortcut => None,
        }
    }
}

/// What Settings > Index shows (DESIGN.md: The Index page): each drive by its
/// mount prefix and label, its state and its entry count, and the
/// service's state and memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IndexCaptions {
    /// Each drive's name, state and entry count, as the list's columns.
    pub(crate) drives: Vec<[String; 3]>,
    pub(crate) service: String,
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
        if let Some(failure) = failure {
            return IndexCaptions {
                drives: Vec::new(),
                service: failure_text(strings, failure),
            };
        }
        let Some(status) = status else {
            return IndexCaptions {
                drives: Vec::new(),
                service: strings.text("settings_index.asking").to_owned(),
            };
        };
        let drives = status
            .volumes
            .iter()
            .map(|volume| {
                let prefix = volume.mounted().display_prefix();
                let name = match label(prefix).filter(|label| !label.is_empty()) {
                    Some(label) => format!("{prefix} {label}"),
                    None => prefix.to_owned(),
                };
                let state = match volume.phase {
                    VolumePhase::Live => "settings_index.live",
                    VolumePhase::Snapshot => "settings_index.snapshot",
                    VolumePhase::Bootstrapping | VolumePhase::Rescanning => {
                        "settings_index.indexing"
                    }
                    VolumePhase::Offline => "settings_index.offline",
                    VolumePhase::Failed { .. } => "settings_index.failed",
                };
                [
                    name,
                    strings.text(state).to_owned(),
                    numbers.integer(volume.entries),
                ]
            })
            .collect();
        // 0 is a service that could not read its own memory.
        let service = match status.process_private_bytes {
            0 => strings.text("settings_index.running").to_owned(),
            bytes => strings.format(
                "settings_index.running_memory",
                &[&text::bytes(strings, numbers, bytes)],
            ),
        };
        IndexCaptions { drives, service }
    }
}

/// A dialog's title and fixed captions in the language of the moment,
/// which the dialog module sets when the dialog opens.
pub(crate) struct DialogCaptions {
    pub(crate) title: String,
    /// Each control's ID and caption.
    pub(crate) items: Vec<(u16, String)>,
}

impl DialogCaptions {
    pub(crate) fn new(dialog: DialogId, strings: &Strings) -> DialogCaptions {
        let table = CaptionTable::of(dialog);
        DialogCaptions {
            title: strings.text(table.title).to_owned(),
            items: table
                .keyed
                .iter()
                .chain(table.plain)
                .map(|(item, key)| (item.id(), strings.text(key).to_owned()))
                .collect(),
        }
    }
}

impl WindowCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> WindowCaptions {
        let strings = model.strings();
        WindowCaptions {
            title: title(model),
            menu_bar: commands::render(&commands::menu_bar(), strings, &model.choices().shortcuts),
            search_cue: strings.text("search.cue").to_owned(),
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
            inspector_buttons: INSPECTOR_BUTTONS.map(|(_, key)| strings.text(key).to_owned()),
            inspector_labels: FIELD_LABELS.map(|key| strings.text(key).to_owned()),
            inspector: InspectorCaptions::new(model, numbers),
            status: StatusCaptions::new(model, numbers),
            message: list_message(model),
            status_widest: status_widest(strings, numbers),
        }
    }
}

impl StatusCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> StatusCaptions {
        StatusCaptions {
            results: results_status(model, numbers),
            selection: selection_status(model, numbers),
            export: export_status(model, numbers),
            index: index_status(model),
        }
    }

    pub(crate) fn parts(&self) -> [&str; 4] {
        [&self.results, &self.selection, &self.export, &self.index]
    }
}

/// The labels of the inspector's fields: one entry's, in the pane's order,
/// then the several entries' Total size.
const FIELD_LABELS: [&str; 8] = [
    "inspector.folder",
    "inspector.full_path",
    "inspector.type",
    "inspector.size",
    "inspector.date_modified",
    "inspector.attributes",
    "inspector.volume",
    "inspector.total_size",
];

/// What the inspector shows for the selection (DESIGN.md: Inspector pane).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InspectorCaptions {
    pub(crate) heading: String,
    /// The one selected entry's name and whether it is a folder, whose icon
    /// the heading shows.
    pub(crate) icon: Option<(String, bool)>,
    /// Each field's label and value, in the pane's order.
    pub(crate) fields: Vec<(String, String)>,
}

impl InspectorCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> InspectorCaptions {
        let strings = model.strings();
        let label = |key: &'static str| strings.text(key).to_owned();
        match model.selection() {
            // The number of matches.
            Selection::None => InspectorCaptions {
                heading: match model.results().shown() {
                    Shown::Rows(_) => {
                        strings.count("status.results", model.results().total(), numbers, &[])
                    }
                    Shown::Nothing | Shown::Message(_) => String::new(),
                },
                ..InspectorCaptions::default()
            },
            Selection::Reading { .. } => {
                let reading = label("status.reading");
                InspectorCaptions {
                    heading: reading.clone(),
                    icon: None,
                    fields: vec![(label("inspector.total_size"), reading)],
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
    let kind = if entry.is_dir {
        strings.text("inspector.folder_type").to_owned()
    } else {
        match gozler_core::query::extension(name.as_bytes(), false) {
            Some(extension) => strings.format(
                "inspector.file_type",
                &[&String::from_utf8_lossy(extension)],
            ),
            None => strings.text("inspector.file").to_owned(),
        }
    };
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
    let attributes = entry.attributes.map_or_else(unknown, |attributes| {
        let words: Vec<&str> = attributes
            .letters()
            .chars()
            .filter_map(attribute_word)
            .map(|key| strings.text(key))
            .collect();
        if words.is_empty() {
            strings.text("inspector.no_attributes").to_owned()
        } else {
            words.join(", ")
        }
    });
    let volume = model
        .volume_of(&entry.path)
        .map(|volume| {
            let prefix = volume.mounted().display_prefix();
            let key = match volume.phase {
                VolumePhase::Live => "inspector.volume_live",
                VolumePhase::Snapshot => "inspector.volume_snapshot",
                VolumePhase::Bootstrapping | VolumePhase::Rescanning => "filter.location_indexing",
                VolumePhase::Offline => "filter.location_offline",
                VolumePhase::Failed { .. } => "filter.location_failed",
            };
            strings.format(key, &[prefix])
        })
        .unwrap_or_default();
    let values = [
        gozler_core::winpath::parent_of(&entry.path),
        entry.path.clone(),
        kind,
        size,
        entry
            .mtime_ft
            .map_or_else(unknown, |ft| numbers.date_time_seconds(ft)),
        attributes,
        volume,
    ];
    InspectorCaptions {
        heading: name.to_owned(),
        icon: Some((name.to_owned(), entry.is_dir)),
        fields: FIELD_LABELS
            .iter()
            .zip(values)
            .map(|(key, value)| (strings.text(key).to_owned(), value))
            .collect(),
    }
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
        icon: None,
        fields: vec![(strings.text("inspector.total_size").to_owned(), total)],
    }
}

/// A size as the inspector shows it, with its exact bytes, as File
/// Explorer's Properties does: "1.20 KB (1,234 bytes)".
fn exact_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64) -> String {
    strings.format(
        "inspector.size_exact",
        &[
            &text::bytes(strings, numbers, bytes),
            &strings.count("size.bytes", bytes, numbers, &[]),
        ],
    )
}

/// The catalogue key of an attribute letter's word, as `attrib:` names the
/// letters. D has none, because the Type field already says Folder.
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

fn status_widest(strings: &Strings, numbers: &dyn Numbers) -> [Vec<String>; 3] {
    let limit = numbers.integer(LIST_LIMIT);
    let showing = strings.format("status.showing", &[&limit, &numbers.integer(WIDEST_TOTAL)]);
    // Which size is widest depends on the font and the language, so every
    // unit's widest forms are measured: 1,023 bytes, then a three-digit size
    // with two, one and no decimals in each unit.
    let sizes = std::iter::once(1023).chain(
        (1..=4)
            .flat_map(|unit| [9.99, 99.9, 999.0].map(|value| (value * 1024f64.powi(unit)) as u64)),
    );
    let mut selected: Vec<String> = sizes
        .map(|n| {
            strings.format(
                "status.selected",
                &[&limit, &text::bytes(strings, numbers, n)],
            )
        })
        .collect();
    selected.push(strings.text("status.reading").to_owned());
    [
        vec![
            strings.format("status.filtered", &[&showing]),
            strings.text("status.searching").to_owned(),
        ],
        selected,
        vec![
            strings.count("status.exporting", WIDEST_TOTAL, numbers, &[]),
            strings.count("status.exported", WIDEST_TOTAL, numbers, &["results.csv"]),
        ],
    ]
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

/// Status part 1: how many results, or that a search runs. A filter that
/// the hidden pane cannot show adds "· Filtered" (DESIGN.md: S10).
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

/// Status part 2: how many entries are selected and the size of the
/// selected files, or that the selection is read.
pub(crate) fn selection_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.selection() {
        Selection::None => String::new(),
        Selection::Reading { .. } => strings.text("status.reading").to_owned(),
        Selection::Files(_) | Selection::Folder { .. } => {
            let sums = Sums::of(model.selected_files().unwrap_or_default());
            strings.format(
                "status.selected",
                &[
                    &numbers.integer(sums.entries()),
                    &text::bytes(strings, numbers, sums.bytes),
                ],
            )
        }
    }
}

/// Status part 3: the export's progress, or the last export's result.
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

/// Status part 4: the service's failure while the list shows rows or
/// another message, so the window says that the service stopped also when
/// it keeps its rows (DESIGN.md: T10); else the index state, only when it
/// affects the answer (DESIGN.md: Status bar); else a newer release that Check
/// for updates found (DESIGN.md: The General page).
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

/// Whether status part 4 names a newer release, which a click on it then
/// opens (DESIGN.md: The General page).
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
        .map(|button| strings.text(button.label()).to_owned())
        .collect();
    Some((text, buttons))
}

/// What a failure of the service means for the person (DESIGN.md: Daemon
/// failures).
pub(crate) fn failure_text(strings: &Strings, failure: &DaemonFailure) -> String {
    match failure {
        DaemonFailure::NotRunning(why) => match why {
            NotRunning::Stopped => strings.text("failure.stopped").to_owned(),
            NotRunning::Disabled => strings.text("failure.disabled").to_owned(),
            NotRunning::NotInstalled {
                gozlerd_beside: true,
                ..
            } => strings.text("failure.not_installed").to_owned(),
            NotRunning::NotInstalled {
                gozlerd_beside: false,
                ..
            } => strings.text("failure.missing").to_owned(),
            NotRunning::Unknown(detail) => strings.format("failure.unknown", &[detail]),
        },
        DaemonFailure::NotReady => strings.text("failure.not_ready").to_owned(),
        DaemonFailure::NotGozler => strings.text("failure.not_gozler").to_owned(),
        DaemonFailure::TooOld => strings.text("failure.too_old").to_owned(),
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
            }
        }
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
