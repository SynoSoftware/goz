//! `ui.json`: the person's settings and the window's state, which only the
//! window writes (spec: Where settings are saved). The keys are the file
//! format, so renaming one needs a migration. A value that is missing or
//! damaged falls back to its default, and every other value still loads.

use super::layout::Dip;
use super::model::Column;
use super::query::Matching;
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

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Choices {
    /// A language tag, such as `de`; absent for Windows default.
    #[serde(
        default,
        deserialize_with = "language",
        serialize_with = "serialize_language",
        skip_serializing_if = "is_windows_default"
    )]
    pub(crate) language: LanguageChoice,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
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
    }

    /// The keys written must be the keys read, and the second save must
    /// replace the first file.
    #[test]
    fn saved_settings_load_again() {
        let folder =
            std::env::temp_dir().join(format!("gozler-ui-settings-{}", std::process::id()));
        let path = folder.join("ui.json");
        let settings = Settings {
            choices: Choices {
                language: LanguageChoice::Tag("qps-ploc".into()),
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
