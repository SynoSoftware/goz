//! The drives and rows of Settings > Include and its Indexed folders dialog,
//! from the service's status and the Include settings (DESIGN.md: The
//! Include page). `gozler_core::include` owns the rules; this module only
//! names the drives and folders as the person sees them.

use super::text::{Numbers, Strings};
use gozler_core::include::IndexSettings;
use gozler_core::proto::DaemonStatus;

/// A drive that the Include settings can name: one the service indexes or
/// leaves out, or one a saved choice names that is not connected.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IncludeDrive {
    /// Its GUID path, which the settings key it by.
    pub(crate) volume: String,
    /// As the tree shows it, such as `C:\ Windows`.
    pub(crate) name: String,
    /// What the paths of its folders start with, such as `C:\`.
    pub(crate) prefix: String,
    /// The folder to list its folders from; `None` when it is not
    /// connected.
    pub(crate) root: Option<String>,
}

/// The connected drives of `status` in its order, then the drives that
/// `settings` names and that are not connected, which keep their rows so a
/// choice for a docking drive is not lost. `label` gives the volume label of
/// a mount prefix, which only Windows knows.
pub(crate) fn include_drives(
    status: &DaemonStatus,
    settings: &IndexSettings,
    label: &dyn Fn(&str) -> Option<String>,
) -> Vec<IncludeDrive> {
    let connected = status
        .volumes
        .iter()
        .map(|v| (&v.guid, v.mounted()))
        .chain(status.left_out.iter().map(|v| (&v.guid, v.mounted())));
    let mut drives: Vec<IncludeDrive> = connected
        .map(|(guid, mounted)| {
            let prefix = mounted.display_prefix();
            let name = match label(prefix).filter(|label| !label.is_empty()) {
                Some(label) => format!("{prefix} {label}"),
                None => prefix.to_owned(),
            };
            IncludeDrive {
                volume: guid.clone(),
                name,
                prefix: prefix.to_owned(),
                root: Some(prefix.to_owned()),
            }
        })
        .collect();
    for saved in &settings.drives {
        if !drives
            .iter()
            .any(|d| d.volume.eq_ignore_ascii_case(&saved.volume))
        {
            // The name is the drive's prefix, which ends in `\`, and then
            // its label after a space, as above, such as `C:\Mount\Data\ Data`.
            // A label holds no `\`, so the last `\ ` ends the prefix.
            let prefix = if saved.name.ends_with('\\') {
                saved.name.as_str()
            } else {
                saved
                    .name
                    .rfind("\\ ")
                    .map_or(saved.volume.as_str(), |end| &saved.name[..=end])
            };
            drives.push(IncludeDrive {
                volume: saved.volume.clone(),
                name: saved.name.clone(),
                prefix: prefix.to_owned(),
                root: None,
            });
        }
    }
    drives
}

/// The GUID paths of the drives that `status` lists, indexed or left out:
/// the connected drives of [`include_drives`], read without their labels.
pub(crate) fn connected_volumes(status: &DaemonStatus) -> Vec<String> {
    let indexed = status.volumes.iter().map(|v| v.guid.clone());
    indexed
        .chain(status.left_out.iter().map(|v| v.guid.clone()))
        .collect()
}

/// One row of the summary of the indexed folders.
pub(crate) struct SummaryRow {
    /// The index of its drive in the drive list.
    pub(crate) drive: usize,
    /// The folder below the drive's root; empty for the root.
    pub(crate) path: String,
    /// Folder; Left out, the count of the folders left out inside it; and
    /// Folders, those folders below the drive's root.
    pub(crate) cells: [String; 3],
    /// The full paths of the folders left out inside it, which the row's
    /// tooltip shows.
    pub(crate) left_out: Vec<String>,
}

impl SummaryRow {
    /// The row's tooltip: each folder left out inside it, by its full path,
    /// one to a line; `None` when it has none.
    pub(crate) fn tip(&self) -> Option<String> {
        (!self.left_out.is_empty()).then(|| self.left_out.join("\r\n"))
    }
}

/// Windows' "Index these locations": each indexed folder of each drive,
/// with the folders left out inside it.
pub(crate) fn summary_rows(
    drives: &[IncludeDrive],
    settings: &IndexSettings,
    strings: &Strings,
    numbers: &dyn Numbers,
) -> Vec<SummaryRow> {
    let mut rows = Vec::new();
    for (index, drive) in drives.iter().enumerate() {
        let full = |path: &str| format!("{}{path}", drive.prefix);
        for folder in settings.summary(&drive.volume) {
            let count = match folder.left_out.len() {
                0 => strings.text("settings_include.left_out_none").to_owned(),
                count => strings.count(
                    "settings_include.left_out_folders",
                    count as u64,
                    numbers,
                    &[],
                ),
            };
            rows.push(SummaryRow {
                drive: index,
                cells: [full(&folder.path), count, folder.left_out.join("; ")],
                left_out: folder.left_out.iter().map(|p| full(p)).collect(),
                path: folder.path,
            });
        }
    }
    rows
}

/// The pairs of GUID path and name that `IndexSettings::with_drives`
/// takes, for the connected drives.
pub(crate) fn connected_drives(drives: &[IncludeDrive]) -> Vec<(String, String)> {
    drives
        .iter()
        .filter(|d| d.root.is_some())
        .map(|d| (d.volume.clone(), d.name.clone()))
        .collect()
}

/// The preview line: the entries the index holds now, and about how many
/// it holds after OK, `None` while the service counts them. `adds` is
/// whether the edited settings check again what the index does not hold,
/// which the count cannot include.
pub(crate) fn preview(
    strings: &Strings,
    numbers: &dyn Numbers,
    now: u64,
    after: Option<u64>,
    adds: bool,
) -> String {
    let after = after.map_or_else(|| "…".to_owned(), |count| numbers.integer(count));
    let added = if adds {
        strings.text("settings_include.preview_added")
    } else {
        ""
    };
    strings.format(
        "settings_include.preview",
        &[&numbers.integer(now), &after, added],
    )
}
