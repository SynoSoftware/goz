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

mod filters;
mod inspector;
mod panels;
mod search;
mod settings;
mod status;

pub(crate) use filters::{FilterCaptions, location_items};
pub(crate) use inspector::{Inspected, InspectorCaptions, MODIFIED_ROW, age};
pub(crate) use panels::{
    PanelCaptions, key_bar, look_in_item, panel_cell, panel_header, panel_headers, panel_message,
};
pub(crate) use search::{ListCaption, header, list_message};
pub(crate) use settings::{
    IndexCaptions, SETTINGS_GROUPS, service_report, settings_job, settings_page_hits,
    settings_pages_matching, settings_status_view, settings_status_widest,
};
pub(crate) use status::{
    IndexingCaption, JobCaption, JobProgress, MemoryCaptions, MemoryWidest, StatusCaptions,
    StatusWidest, shows_update,
};
#[cfg(test)]
pub(crate) use status::{export_status, index_status, results_status, selection_status};

use inspector::FIELD_LABELS;
use status::status_widest;

use super::commands::{
    self, CommandButton, HIDE_DETAILS, INSPECTOR_BUTTONS, INSPECTOR_COPY_FOLDER,
    INSPECTOR_COPY_NAME, INSPECTOR_COPY_PATH, MenuNode, SHOW_DETAILS,
};
use super::fetch::{DaemonFailure, LocalService};
use super::fixer::SetupFix;
use super::glyph::Glyph;
use super::ids::{ControlId, DialogId};
use super::model::Model;
use super::panel::LookIn;
use super::query::MatchMode;
use super::results::{Fix, ListButton};
use super::selection::SELECTION_LIMIT;
use super::settings::Mode;
use super::text::{self, Numbers, Strings};
use gozler_client::NotRunning;

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
    pub(crate) headers: Vec<text::Text>,
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
    pub(crate) status_progress: [String; 2],
    pub(crate) status_widest: StatusWidest,
    /// `None` while Settings > General hides the memory group.
    pub(crate) memory_widest: Option<MemoryWidest>,
    pub(crate) message: Option<ListCaption>,
    /// A search panel's Look in label, and its list's items in
    /// [`LookIn::ALL`]'s order.
    pub(crate) look_in_label: String,
    pub(crate) look_in: Vec<String>,
    /// The key bar's buttons: each key, its word and its tooltip.
    pub(crate) key_bar: [(String, String, Option<String>); 6],
    pub(crate) retry: String,
    /// The widest free space a status line shows, which the layout sizes
    /// the status line's right end for.
    pub(crate) free_widest: Vec<String>,
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

/// The icon of each push button's caption, by its catalogue key, so a
/// caption shows the same icon on every button that has it (DESIGN.md:
/// Look). A fix that needs administrator permission shows the
/// administrator shield instead, so its caption has no icon here.
const BUTTON_GLYPHS: [(&str, Glyph); 24] = [
    ("button.ok", Glyph::Accept),
    ("button.cancel", Glyph::Cancel),
    ("button.close", Glyph::Cancel),
    ("button.clear", Glyph::EraseTool),
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
    ("file_operations.hide", Glyph::Remove),
    ("filter.clear", Glyph::EraseTool),
    ("filter.choose_folder", Glyph::FolderOpen),
    ("command.clear_filters", Glyph::EraseTool),
    ("list.search_everywhere", Glyph::Search),
    ("fix.download", Glyph::Download),
    ("fix.update", Glyph::Download),
    ("fix.downgrade", Glyph::Download),
    ("fix.open_services", Glyph::Processing),
    ("panel.retry", Glyph::Refresh),
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
            // Move to folder's title and button are the dialog's own.
            DialogId::Transfer => CaptionTable {
                title: "file_operations.copy_to_title",
                keyed: &[(Control(C::TransferPolicyLabel), "copy_policy.label")],
                plain: &[
                    (Control(C::TransferPolicyHint), "copy_policy.hint"),
                    (Control(C::TransferBrowse), "settings_context_menu.browse"),
                    (Control(C::TransferMirror), "transfer.mirror"),
                    (Control(C::TransferSync), "transfer.sync"),
                    (Ok, "file_operations.copy"),
                    (Cancel, "button.cancel"),
                ],
            },
            DialogId::NewFolder => CaptionTable {
                title: "file_operations.new_folder",
                keyed: &[(Control(C::NewFolderLabel), "new_folder.name")],
                plain: &[(Ok, "new_folder.create"), (Cancel, "button.cancel")],
            },
            DialogId::Synchronize => CaptionTable {
                title: "sync.title",
                keyed: &[
                    (Control(C::SyncSourceLabel), "sync.source"),
                    (Control(C::SyncDestinationLabel), "sync.destination"),
                    (Control(C::SyncMirror), "sync.mirror"),
                    (Control(C::SyncCompare), "sync.compare"),
                ],
                plain: &[
                    (Control(C::SyncSourceBrowse), "settings_context_menu.browse"),
                    (
                        Control(C::SyncDestinationBrowse),
                        "settings_context_menu.browse",
                    ),
                    (Control(C::SyncDirection), "sync.direction"),
                    (Control(C::SyncHint), "sync.initial"),
                    (Ok, "sync.synchronize"),
                    (Cancel, "button.cancel"),
                ],
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
                    (Control(C::GeneralSizesLabel), "settings_general.sizes"),
                    (Control(C::GeneralDatesLabel), "settings_general.dates"),
                    (Control(C::GeneralSignIn), "settings_general.sign_in"),
                    (Control(C::GeneralUpdates), "settings_general.updates"),
                    (
                        Control(C::GeneralKeepRunning),
                        "settings_general.keep_running",
                    ),
                    (Control(C::GeneralOneWindow), "settings_general.one_window"),
                    (Control(C::GeneralShowIcons), "settings_general.show_icons"),
                    (
                        Control(C::GeneralShowMemory),
                        "settings_general.show_memory",
                    ),
                    (Control(C::GeneralRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::GeneralHeading), "settings.general"),
                    (Control(C::GeneralDisplay), "settings_general.display"),
                    (Control(C::GeneralFormats), "settings_general.formats"),
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
                    (
                        Control(C::SearchEnterPanels),
                        "settings_search.enter_panels",
                    ),
                    (Control(C::SearchEnterOpen), "settings_search.enter_open"),
                    (Control(C::SearchRestore), "settings.reset"),
                ],
                plain: &[
                    (Control(C::SearchHeading), "settings.search"),
                    (Control(C::SearchBoxGroup), "settings_search.search_box"),
                    (Control(C::SearchResultsGroup), "settings_search.results"),
                    (Control(C::SearchEnterLabel), "settings_search.enter_label"),
                ],
            },
            DialogId::SettingsKeyboard => CaptionTable {
                title: "settings.keyboard",
                keyed: &[
                    (
                        Control(C::KeyboardShortcutLabel),
                        "settings_keyboard.shortcut",
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
                    (
                        Control(C::KeyboardTwoPanelsNote),
                        "settings_keyboard.two_panels",
                    ),
                ],
            },
            DialogId::SettingsIndex => CaptionTable {
                title: "settings.index",
                keyed: &[
                    (Control(C::IndexRebuild), "settings_index.rebuild"),
                    (Control(C::IndexReport), "settings_index.report"),
                    (
                        Control(C::IndexDatesCreated),
                        "settings_index.dates_created",
                    ),
                ],
                plain: &[
                    (Control(C::IndexHeading), "settings.index"),
                    (Control(C::IndexDrivesGroup), "settings_index.drives"),
                    (Control(C::IndexHoldsGroup), "settings_index.holds"),
                    (Control(C::IndexHolds), "settings_index.holds_text"),
                    (
                        Control(C::IndexDatesCreatedNote),
                        "settings_index.dates_created_note",
                    ),
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
                    (Control(C::FileOperationsPolicyLabel), "copy_policy.label"),
                    (
                        Control(C::SpeedTransfersLabel),
                        "settings_file_operations.transfers_per_device",
                    ),
                    (Control(C::FileOperationsRestore), "settings.reset"),
                ],
                plain: &[
                    (
                        Control(C::FileOperationsSettingsHeading),
                        "settings.file_operations",
                    ),
                    (Control(C::FileOperationsPolicyHint), "copy_policy.hint"),
                ],
            },
            DialogId::SettingsPerformance => CaptionTable {
                title: "settings.performance",
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
                    (Control(C::SpeedHeading), "settings.performance"),
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
            | DialogId::SettingsFileOperations
            | DialogId::SettingsPerformance => Some(DialogId::Settings),
            DialogId::About
            | DialogId::SearchSyntax
            | DialogId::CustomDate
            | DialogId::CustomSize
            | DialogId::Transfer
            | DialogId::Synchronize
            | DialogId::NewFolder
            | DialogId::Settings
            | DialogId::IndexedFolders
            | DialogId::ServiceReport
            | DialogId::FileOperationsFailed
            | DialogId::CommandShortcut => None,
        }
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

/// A dialog's title and fixed captions in the language of the moment,
/// which the dialog module sets when the dialog opens.
pub(crate) struct DialogCaptions {
    pub(crate) title: String,
    /// Each control's ID and caption.
    pub(crate) items: Vec<(u16, String)>,
    /// Each push button's ID and the icon before its caption.
    pub(crate) glyphs: Vec<(u16, Glyph)>,
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
            menu_bar: commands::render(
                &commands::menu_bar(model.mode()),
                strings,
                &model.choices().shortcuts,
                model.mode(),
            ),
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
            status_progress: [
                strings.text("file_operations.progress_current").to_owned(),
                strings.text("file_operations.progress_total").to_owned(),
            ],
            status_widest: status_widest(strings, numbers),
            memory_widest: model
                .choices()
                .show_memory
                .then(|| MemoryWidest::new(strings, numbers)),
            message: list_message(model),
            look_in_label: strings.text("panel.look_in").to_owned(),
            look_in: LookIn::ALL
                .map(|look_in| look_in_item(strings, look_in))
                .to_vec(),
            key_bar: key_bar(strings),
            retry: strings.text("panel.retry").to_owned(),
            free_widest: (2..=5)
                .flat_map(|unit| {
                    [9.99, 99.9, 999.0].map(move |value| (value * 1024f64.powi(unit)) as u64)
                })
                .map(|bytes| strings.format("panel.free", &[&text::bytes(strings, numbers, bytes)]))
                .collect(),
        }
    }
}

/// A part of a value, after its icon if it has one, such as a status bar
/// figure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValuePart {
    pub(crate) connector: Option<String>,
    pub(crate) glyph: Option<Glyph>,
    pub(crate) text: String,
    pub(crate) narrow: Option<Narrow>,
}

impl ValuePart {
    pub(crate) fn plain(text: String) -> ValuePart {
        ValuePart {
            connector: None,
            glyph: None,
            text,
            narrow: None,
        }
    }
}

/// What the status bar's files total shows where its `text` does not fit
/// (DESIGN.md: Status bar).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Narrow {
    /// The files total without its size, so the number of files stays
    /// whole.
    pub(crate) text: String,
    /// Both totals whole, one on each line, for the tooltip that shows
    /// while a total is shortened or left out.
    pub(crate) tip: String,
}

/// The window's title: the typed search and the product's name, as Windows
/// titles a document window, or the name alone while the search is empty or
/// Two panels shows, because a title that named one panel's folder would be
/// wrong for the other (DESIGN.md: Two panels' window).
pub(crate) fn title(model: &Model) -> String {
    let strings = model.strings();
    let text = model.search_list().text().trim();
    if text.is_empty() || model.mode() == Mode::TwoPanels {
        strings.text("window.title").to_owned()
    } else {
        strings.format("window.title_search", &[text])
    }
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

/// The sum of the known sizes, `None` when no size is known, and how many
/// items have no known size: "3.4 GB", "3.4 GB, 12 unknown", or "12
/// unknown"; the noun beside it says what is counted. It never says only
/// "unknown" (**Owner ruling**: give the part that is known and say what
/// is missing).
fn size_with_unknown(
    strings: &Strings,
    numbers: &dyn Numbers,
    known: Option<u64>,
    without_size: u64,
) -> String {
    let unknown = || strings.count("status.sizes_unknown", without_size, numbers, &[]);
    match known {
        Some(bytes) if without_size == 0 => text::bytes(strings, numbers, bytes),
        Some(bytes) => strings.format(
            "status.size_with_unknown",
            &[&text::bytes(strings, numbers, bytes), &unknown()],
        ),
        None => unknown(),
    }
}

/// The status of a selection of `count` rows, more than a command acts on.
fn too_many(strings: &Strings, numbers: &dyn Numbers, count: u64) -> String {
    strings.format(
        "status.too_many",
        &[&numbers.integer(count), &numbers.integer(SELECTION_LIMIT)],
    )
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
            title: strings.text("fixing.install").to_owned(),
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

/// The question before Settings' Reset all deletes `ui.json` and opens the
/// window again (DESIGN.md: Settings dialog).
pub(crate) fn reset_all_question(strings: &Strings) -> Question {
    Question {
        title: text::without_access_key(strings.text("settings.reset_all")),
        instruction: strings.text("settings.reset_all_question").to_owned(),
        body: QuestionBody::Text(strings.text("settings.reset_all_body").to_owned()),
        footer: None,
        confirm: text::without_access_key(strings.text("settings.reset")),
        cancel: strings.text("button.cancel").to_owned(),
        guard: Guard::CancelFirst,
    }
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

#[cfg(test)]
mod tests {
    use super::CaptionTable;
    use crate::core::ids::DialogId;
    use crate::core::results::Fix;
    use crate::core::text::{LANGUAGES, Strings, access_key, parse_catalogue};
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
