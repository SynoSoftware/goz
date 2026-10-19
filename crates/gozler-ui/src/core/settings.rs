//! `ui.json`: the person's settings and the window's state, which only the
//! window writes (DESIGN.md: Where settings are saved). The keys are the file
//! format, so renaming one needs a migration. A value that is missing or
//! damaged falls back to its default, and every other value still loads.

use super::layout::{Dip, Panes};
use super::model::Column;
use super::query::{Filters, Matching};
use super::shortcuts::{CommandShortcuts, ShowShortcut};
use super::text::LanguageChoice;
use gozler_core::types::SortSpec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io;
use std::path::Path;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// The settings that Settings and the Language menu change.
    #[serde(rename = "settings", default, deserialize_with = "or_default")]
    pub(crate) choices: Choices,
    /// What the window remembers about itself between runs.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) window: WindowState,
}

/// The person's settings of Settings > General, Search and Keyboard (DESIGN.md:
/// Settings dialog). Start at sign-in and Search with Gozler are not here:
/// the registry holds them, because Task Manager and the uninstaller change
/// them there.
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
    /// The open commands; empty does what File Explorer does (DESIGN.md: The
    /// General page).
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_folders: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_files: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) open_file_location: String,
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
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) shortcuts: CommandShortcuts,
}

impl Default for Choices {
    fn default() -> Choices {
        Choices {
            language: LanguageChoice::Windows,
            check_for_updates: false,
            keep_running: on(),
            open_folders: String::new(),
            open_files: String::new(),
            open_file_location: String::new(),
            search_as_you_type: on(),
            show_all_when_empty: on(),
            select_on_click: on(),
            select_on_activate: false,
            show_shortcut: None,
            shortcuts: CommandShortcuts::default(),
        }
    }
}

/// What the Settings dialog edits: the choices, and the two settings whose
/// one copy is in the registry, read when the dialog opens.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsValues {
    pub(crate) choices: Choices,
    pub(crate) start_at_sign_in: bool,
    pub(crate) search_with_gozler: bool,
}

/// A page of the Settings dialog, in the page list's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    General,
    Search,
    Keyboard,
    /// The service's index, which the page shows and does not change yet
    /// (DESIGN.md: Settings dialog).
    Index,
}

impl SettingsPage {
    pub(crate) const ALL: [SettingsPage; 4] = [
        SettingsPage::General,
        SettingsPage::Search,
        SettingsPage::Keyboard,
        SettingsPage::Index,
    ];

    /// Its name in the page list and its heading.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "settings.general",
            SettingsPage::Search => "settings.search",
            SettingsPage::Keyboard => "settings.keyboard",
            SettingsPage::Index => "settings.index",
        }
    }
}

impl SettingsValues {
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
                choices.open_folders = defaults.open_folders;
                choices.open_files = defaults.open_files;
                choices.open_file_location = defaults.open_file_location;
                self.start_at_sign_in = false;
                self.search_with_gozler = false;
            }
            SettingsPage::Search => {
                choices.search_as_you_type = defaults.search_as_you_type;
                choices.show_all_when_empty = defaults.show_all_when_empty;
                choices.select_on_click = defaults.select_on_click;
                choices.select_on_activate = defaults.select_on_activate;
            }
            SettingsPage::Keyboard => {
                choices.show_shortcut = defaults.show_shortcut;
                choices.shortcuts = defaults.shortcuts;
            }
            SettingsPage::Index => {}
        }
    }
}

/// The values the dialog shows and the values Apply last applied, so Apply
/// is enabled exactly while they differ (DESIGN.md: Dialogs).
pub(crate) struct SettingsDraft {
    applied: SettingsValues,
    pub(crate) edited: SettingsValues,
}

impl SettingsDraft {
    pub(crate) fn new(values: SettingsValues) -> SettingsDraft {
        SettingsDraft {
            applied: values.clone(),
            edited: values,
        }
    }

    pub(crate) fn can_apply(&self) -> bool {
        self.edited != self.applied
    }

    /// Records `values`, what Apply or OK applied, which can differ from
    /// the edited values when a registry write failed.
    pub(crate) fn applied(&mut self, values: SettingsValues) {
        self.edited = values.clone();
        self.applied = values;
    }

    pub(crate) fn last_applied(&self) -> &SettingsValues {
        &self.applied
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WindowState {
    #[serde(
        default,
        deserialize_with = "placement",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) placement: Option<Placement>,
    #[serde(default, deserialize_with = "or_default")]
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
}

impl Default for WindowState {
    fn default() -> WindowState {
        WindowState {
            placement: None,
            matching: Matching::default(),
            sort: SortSpec::default(),
            columns: Vec::new(),
            filter_pane: on(),
            filters: Filters::default(),
            inspector: on(),
            inspector_height: inspector_height(),
            tray_notice_shown: false,
            last_update_check: None,
        }
    }
}

/// A shown column and its width in device-independent pixels, so the width
/// is the same on a monitor with another DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedColumn {
    pub(crate) column: Column,
    pub(crate) width: Dip,
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

impl Settings {
    /// The settings in `path`; the defaults when the file is missing or is
    /// not a JSON object.
    pub(crate) fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes `path` through a temporary file renamed over it, so a failed
    /// write never leaves half a file.
    pub(crate) fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).expect("Settings serializes");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
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
    use crate::core::query::{DateModified, DateRange, Day, Kind, SizeFilter, SizeWord};

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
        assert_eq!(
            Settings::load(Path::new("no such folder/ui.json")),
            Settings::default()
        );
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
            search_with_gozler: false,
        };
        let mut draft = SettingsDraft::new(values.clone());
        assert!(!draft.can_apply());
        draft.edited.restore_defaults(SettingsPage::Search);
        assert!(draft.edited.choices.search_as_you_type);
        assert_eq!(
            draft.edited.choices.open_files, "notepad %1",
            "General keeps"
        );
        assert!(draft.edited.start_at_sign_in);
        assert!(draft.can_apply());
        let applied = draft.edited.clone();
        draft.applied(applied);
        assert!(!draft.can_apply());
        draft.edited.choices.search_as_you_type = false;
        draft.edited.choices.search_as_you_type = true;
        assert!(!draft.can_apply(), "a value changed back is no change");
        draft.edited.restore_defaults(SettingsPage::General);
        assert_eq!(draft.edited.choices.open_files, "");
        assert!(!draft.edited.start_at_sign_in);
    }

    /// A file another version or a person edited must not lose every
    /// setting for one bad value: each damaged value falls back alone.
    #[test]
    fn a_damaged_value_falls_back_to_its_default() {
        let text = r#"{
            "settings": { "language": 5 },
            "window": { "placement": { "left": 10, "top": 20, "right": 900, "bottom": 700, "maximized": true } }
        }"#;
        let settings: Settings = serde_json::from_str(text).expect("the file is an object");
        assert_eq!(settings.choices.language, LanguageChoice::Windows);
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

    /// The keys written must be the keys read, and the second save must
    /// replace the first file.
    #[test]
    fn saved_settings_load_again() {
        let folder =
            std::env::temp_dir().join(format!("gozler-ui-settings-{}", std::process::id()));
        let path = folder.join("ui.json");
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
                matching: Matching {
                    mode: crate::core::query::MatchMode::WholeWords,
                    case: true,
                    path: false,
                },
                sort: SortSpec::default_for(gozler_core::types::SortKey::Size),
                columns: vec![
                    SavedColumn {
                        column: Column::Size,
                        width: Dip(90),
                    },
                    SavedColumn {
                        column: Column::Name,
                        width: Dip(300),
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
                tray_notice_shown: true,
                last_update_check: Some(1_790_000_000),
            },
        };
        Settings::default()
            .save(&path)
            .expect("the temporary folder is writable");
        settings
            .save(&path)
            .expect("the temporary folder is writable");
        assert_eq!(Settings::load(&path), settings);
        std::fs::remove_dir_all(&folder).expect("the test's folder is removable");
    }
}
