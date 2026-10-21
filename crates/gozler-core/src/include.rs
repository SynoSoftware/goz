//! The Include settings: which drives, folders and names gozlerd indexes.
//!
//! The window edits them, an administrator saves them in the registry, and
//! gozlerd applies them. [`IndexSettings::rules_for`] gives one volume's
//! [`VolumeRules`], which [`VolumeIndex::apply_rules`] applies and
//! [`VolumeIndex::count_kept`] counts, so the service, its preview and the
//! window read one set of rules.
//!
//! The nearest choice above an entry decides: a drive or folder is checked or
//! unchecked, and everything below it follows until a folder below says
//! otherwise. A name pattern, Exclude hidden and Exclude system leave out a
//! matching entry, and a matching folder with everything in it. A folder
//! above a checked folder stays in the index as the path to it, with no file
//! of its own.
//!
//! [`VolumeIndex::apply_rules`]: crate::index::VolumeIndex::apply_rules
//! [`VolumeIndex::count_kept`]: crate::index::VolumeIndex::count_kept

use crate::attributes::PackedAttributes;
use crate::fold::fold;
use crate::query::ParsedQuery;
use crate::query::wildcard::{CodePointBuf, Wildcard};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// The registry key, below `HKEY_LOCAL_MACHINE`, that holds the settings.
/// Only administrators and the system can write it, which is what proves
/// that an administrator saved them.
pub const SETTINGS_KEY: &str = r"SOFTWARE\Gozler";

/// The value of [`SETTINGS_KEY`] that holds [`IndexSettings`] as JSON. It
/// keeps the name Include from before the settings held Dates created.
pub const INCLUDE_VALUE: &str = "Include";

/// The value of [`SETTINGS_KEY`] that the window's Rebuild index sets to a
/// new token each time an administrator asks; gozlerd rebuilds every
/// indexed volume when it reads a token it has not seen.
pub const REBUILD_VALUE: &str = "Rebuild";

/// What gozlerd indexes, as the Include page edits it. A field that is
/// missing takes its default, so the defaults index everything.
///
/// Two settings are equal when they index the same entries, so an edit that
/// is undone, or patterns that differ only in spaces, case or empty entries,
/// is nothing to save or apply.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexSettings {
    /// One entry for each drive a person decided about. A drive without one
    /// follows [`Self::include_new_drives`].
    #[serde(default)]
    pub drives: Vec<DriveChoice>,
    #[serde(default = "on")]
    pub include_new_drives: bool,
    /// Name patterns with `*` and `?`, separated by `;`, as the person typed
    /// them.
    #[serde(default)]
    pub excluded_names: String,
    #[serde(default)]
    pub exclude_hidden: bool,
    #[serde(default)]
    pub exclude_system: bool,
    /// Dates created, on the Drives and service page: the index holds each
    /// entry's date created, for `dc:` and the Date created column.
    #[serde(default)]
    pub dates_created: bool,
}

/// One drive's check box and the folders below it whose check box differs
/// from the folder above.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveChoice {
    /// The volume's GUID path, such as `\\?\Volume{…}\`, so a new drive
    /// letter keeps the choices.
    pub volume: String,
    /// The drive as last seen, such as `D:\ Data`, for the row of a drive
    /// that is not connected.
    #[serde(default)]
    pub name: String,
    pub indexed: bool,
    #[serde(default)]
    pub folders: Vec<FolderChoice>,
}

/// One folder's check box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderChoice {
    /// The folder below the drive's root, such as `Users\ana`.
    pub path: String,
    pub indexed: bool,
}

fn on() -> bool {
    true
}

impl Default for IndexSettings {
    fn default() -> Self {
        IndexSettings {
            drives: Vec::new(),
            include_new_drives: true,
            excluded_names: String::new(),
            exclude_hidden: false,
            exclude_system: false,
            dates_created: false,
        }
    }
}

impl PartialEq for IndexSettings {
    fn eq(&self, other: &Self) -> bool {
        self.rules_key() == other.rules_key()
    }
}

impl Eq for IndexSettings {}

/// A drive's choices as [`IndexSettings::rules_key`] compares them.
type DriveKey = (String, bool, Vec<(Vec<Vec<u8>>, bool)>);

impl IndexSettings {
    /// What decides the index: the switches, the patterns as
    /// [`Self::rules_for`] reads them, and each drive's choices in order of
    /// the folded volume and paths. A drive's name only labels its row.
    fn rules_key(&self) -> (bool, bool, bool, bool, Vec<Vec<u8>>, Vec<DriveKey>) {
        let mut patterns = self.patterns();
        patterns.sort_unstable();
        patterns.dedup();
        let mut drives: Vec<DriveKey> = self
            .drives
            .iter()
            .map(|drive| {
                let mut folders: Vec<(Vec<Vec<u8>>, bool)> = drive
                    .folders
                    .iter()
                    .map(|f| (folded_parts(&f.path), f.indexed))
                    .collect();
                folders.sort_unstable();
                (drive.volume.to_ascii_lowercase(), drive.indexed, folders)
            })
            .collect();
        drives.sort_unstable();
        (
            self.include_new_drives,
            self.exclude_hidden,
            self.exclude_system,
            self.dates_created,
            patterns,
            drives,
        )
    }

    /// The name patterns, folded, without the spaces around each and
    /// without empty ones.
    fn patterns(&self) -> Vec<Vec<u8>> {
        self.excluded_names
            .split(';')
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| fold(pattern.as_bytes()))
            .collect()
    }

    /// The settings that [`INCLUDE_VALUE`] holds.
    ///
    /// # Errors
    ///
    /// The text is not the settings' JSON.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The settings as [`INCLUDE_VALUE`] holds them.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("the settings are plain data")
    }

    /// The choice saved for `volume`, a GUID path.
    pub fn drive(&self, volume: &str) -> Option<&DriveChoice> {
        self.drives
            .iter()
            .find(|d| d.volume.eq_ignore_ascii_case(volume))
    }

    /// The check boxes of the volume whose GUID path is `volume`.
    fn choice_tree(&self, volume: &str) -> ChoiceTree {
        let mut choices = ChoiceTree::new(
            self.drive(volume)
                .map_or(self.include_new_drives, |d| d.indexed),
        );
        if let Some(drive) = self.drive(volume) {
            for folder in &drive.folders {
                choices.add(&folder.path, folder.indexed);
            }
        }
        choices
    }

    /// Gives each of `drives`, a GUID path and the drive's name as shown,
    /// an entry with that name. A drive without one gets the check box that
    /// [`Self::include_new_drives`] gives it now, so the drive keeps it when
    /// the person turns that off.
    #[must_use]
    pub fn with_drives(mut self, drives: &[(String, String)]) -> Self {
        for (volume, name) in drives {
            match self
                .drives
                .iter_mut()
                .find(|d| d.volume.eq_ignore_ascii_case(volume))
            {
                Some(drive) => drive.name.clone_from(name),
                None => self.drives.push(DriveChoice {
                    volume: volume.clone(),
                    name: name.clone(),
                    indexed: self.include_new_drives,
                    folders: Vec::new(),
                }),
            }
        }
        self
    }

    /// How the check box of the folder `path` below the root of `volume`
    /// shows; an empty path is the drive.
    pub fn folder_check(&self, volume: &str, path: &str) -> FolderCheck {
        let choices = self.choice_tree(volume);
        let parts = folded_parts(path);
        let mut node = Some(0);
        for part in &parts {
            node = node.and_then(|n| choices.nodes[n].children.get(part).copied());
        }
        // A node has children only when a check box below it is saved.
        if node.is_some_and(|n| !choices.nodes[n].children.is_empty()) {
            FolderCheck::Partial
        } else if choices.indexed_at(&parts) {
            FolderCheck::Checked
        } else {
            FolderCheck::Unchecked
        }
    }

    /// A click on the check box of the folder `path` below the root of
    /// `volume`: a checked folder is unchecked, any other is checked, and
    /// the folders below it follow it, because their choices are dropped.
    pub fn toggle_folder(&mut self, volume: &str, path: &str) {
        let indexed = self.folder_check(volume, path) != FolderCheck::Checked;
        let parts = folded_parts(path);
        // The choices at and below the folder do not decide the folder
        // above it.
        let above = parts
            .split_last()
            .map(|(_, above)| self.choice_tree(volume).indexed_at(above));
        let new_drive = self.include_new_drives;
        let drive = match self
            .drives
            .iter()
            .position(|d| d.volume.eq_ignore_ascii_case(volume))
        {
            Some(at) => &mut self.drives[at],
            None => {
                self.drives.push(DriveChoice {
                    volume: volume.to_owned(),
                    name: String::new(),
                    indexed: new_drive,
                    folders: Vec::new(),
                });
                self.drives.last_mut().expect("the drive was just pushed")
            }
        };
        drive
            .folders
            .retain(|f| !folded_parts(&f.path).starts_with(&parts));
        match above {
            None => drive.indexed = indexed,
            Some(above) if above != indexed => drive.folders.push(FolderChoice {
                path: path.to_owned(),
                indexed,
            }),
            Some(_) => {}
        }
    }

    /// Windows' "Index these locations" for `volume`: the drive when it is
    /// checked and each checked folder, with the unchecked folders inside
    /// each, in path order.
    pub fn summary(&self, volume: &str) -> Vec<IncludedFolder> {
        let (indexed, folders) = match self.drive(volume) {
            Some(drive) => (drive.indexed, drive.folders.as_slice()),
            None => (self.include_new_drives, &[][..]),
        };
        let mut rows: Vec<(Vec<Vec<u8>>, IncludedFolder)> = folders
            .iter()
            .filter(|f| f.indexed)
            .map(|f| (folded_parts(&f.path), IncludedFolder::new(&f.path)))
            .collect();
        if indexed {
            rows.push((Vec::new(), IncludedFolder::new("")));
        }
        for folder in folders.iter().filter(|f| !f.indexed) {
            let parts = folded_parts(&folder.path);
            // The nearest checked folder above it holds it.
            let nearest = (0..parts.len())
                .rev()
                .find_map(|len| rows.iter().position(|(p, _)| *p == parts[..len]));
            if let Some(at) = nearest {
                rows[at].1.left_out.push(folder.path.clone());
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows.into_iter()
            .map(|(_, mut row)| {
                row.left_out.sort_by_key(|path| folded_parts(path));
                row
            })
            .collect()
    }

    /// The rules for the volume whose GUID path is `volume`.
    pub fn rules_for(&self, volume: &str) -> VolumeRules {
        let choices = self.choice_tree(volume);
        let names = self.patterns();
        let mut attributes = 0;
        if self.exclude_hidden {
            attributes |= FILE_ATTRIBUTE_HIDDEN;
        }
        if self.exclude_system {
            attributes |= FILE_ATTRIBUTE_SYSTEM;
        }
        VolumeRules {
            choices,
            patterns: names.iter().map(|n| Wildcard::compile(n)).collect(),
            names,
            attributes: (attributes != 0).then(|| PackedAttributes::from_record(attributes)),
            dates_created: self.dates_created,
        }
    }

    /// Whether these settings index something on any of `volumes`, GUID
    /// paths, that `applied` leaves out, so applying them must read it from
    /// the volume.
    pub fn adds_to(&self, applied: &IndexSettings, volumes: &[&str]) -> bool {
        volumes
            .iter()
            .any(|v| self.rules_for(v).adds_to(&applied.rules_for(v)))
    }

    /// Refuses `query` when it tests the date created and these settings
    /// keep none, because an empty answer would read as "no file was
    /// created then".
    ///
    /// # Errors
    ///
    /// [`DatesCreatedOff`] for such a query.
    pub fn allows(&self, query: &ParsedQuery) -> Result<(), DatesCreatedOff> {
        if query.tests_date_created() && !self.dates_created {
            return Err(DatesCreatedOff);
        }
        Ok(())
    }
}

/// A search for the date created while the index holds none.
#[derive(Debug, thiserror::Error)]
#[error(
    "dc: needs Dates created, which is off; an administrator turns it on in Settings > Drives \
     and service"
)]
pub struct DatesCreatedOff;

/// How a folder's check box shows in Indexed folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderCheck {
    Checked,
    Unchecked,
    /// Some folders below it differ.
    Partial,
}

/// One row of [`IndexSettings::summary`]: an indexed folder, such as
/// `Users\ana` below the root or the root itself as an empty path, and the
/// folders left out inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludedFolder {
    pub path: String,
    pub left_out: Vec<String>,
}

impl IncludedFolder {
    fn new(path: &str) -> Self {
        IncludedFolder {
            path: path.to_owned(),
            left_out: Vec::new(),
        }
    }
}

/// The folded names of the folders in `path`, a path below a drive's root.
fn folded_parts(path: &str) -> Vec<Vec<u8>> {
    path.split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .map(|p| fold(p.as_bytes()))
        .collect()
}

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// One volume's rules, from [`IndexSettings::rules_for`].
#[derive(Debug)]
pub struct VolumeRules {
    choices: ChoiceTree,
    /// The folded patterns, to tell whether two rule sets have the same.
    names: Vec<Vec<u8>>,
    patterns: Vec<Wildcard>,
    /// The attributes that leave an entry out, when any do.
    attributes: Option<PackedAttributes>,
    /// The index holds each entry's date created.
    pub dates_created: bool,
}

/// Where a folder stands under the rules, which decides where its entries
/// stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    /// The nearest choice at or above the folder checks it.
    indexed: bool,
    /// The folder's node in the choice tree, when a choice names it or a
    /// folder below it.
    node: Option<usize>,
}

/// What the rules decide for one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Indexed; a folder's entries stand at this place.
    Kept(Place),
    /// A folder that is left out, kept as the path to a checked folder below
    /// it; its entries stand at this place.
    Path(Place),
    /// Left out, with everything below it.
    LeftOut,
}

impl VolumeRules {
    /// Whether the rules leave the whole volume out: its drive is unchecked
    /// and no folder on it is checked.
    pub fn leaves_out_volume(&self) -> bool {
        let root = &self.choices.nodes[0];
        root.indexed == Some(false) && !root.checked_below
    }

    /// Whether the rules keep every entry, so the index needs no check.
    pub fn keeps_everything(&self) -> bool {
        self.patterns.is_empty()
            && self.attributes.is_none()
            && self.choices.nodes.iter().all(|n| n.indexed != Some(false))
    }

    /// Whether these rules keep an entry, or a date created, that `applied`
    /// leaves out.
    ///
    /// The nearest choice changes only at a folder that one of the two sets
    /// names, so comparing the two at each of those folders, the root
    /// included, compares them everywhere.
    pub fn adds_to(&self, applied: &VolumeRules) -> bool {
        // Only a read of the volume brings the dates.
        if self.dates_created && !applied.dates_created {
            return true;
        }
        if applied.names.iter().any(|name| !self.names.contains(name)) {
            return true;
        }
        if let Some(applied) = applied.attributes
            && !self.attributes.is_some_and(|ours| ours.contains(applied))
        {
            return true;
        }
        let mut folders = self.choices.folders();
        folders.extend(applied.choices.folders());
        folders
            .iter()
            .any(|f| self.choices.indexed_at(f) && !applied.choices.indexed_at(f))
    }

    /// The volume root's place.
    pub(crate) fn root(&self) -> Place {
        Place {
            indexed: self.choices.nodes[0].indexed.unwrap_or(true),
            node: Some(0),
        }
    }

    /// What the rules decide for an entry with the folded name `folded` and
    /// `attributes`, in a folder at `parent`. `buf` is scratch for the name
    /// patterns.
    pub(crate) fn verdict(
        &self,
        parent: Place,
        folded: &[u8],
        attributes: PackedAttributes,
        is_dir: bool,
        buf: &mut CodePointBuf,
    ) -> Verdict {
        if self
            .attributes
            .is_some_and(|excluded| attributes.intersects(excluded))
            || self.patterns.iter().any(|p| p.matches_into(folded, buf))
        {
            return Verdict::LeftOut;
        }
        let node = parent
            .node
            .and_then(|n| self.choices.nodes[n].children.get(folded).copied());
        let place = Place {
            indexed: node
                .and_then(|n| self.choices.nodes[n].indexed)
                .unwrap_or(parent.indexed),
            node,
        };
        if place.indexed {
            Verdict::Kept(place)
        } else if is_dir && node.is_some_and(|n| self.choices.nodes[n].checked_below) {
            Verdict::Path(place)
        } else {
            Verdict::LeftOut
        }
    }
}

/// The drive's and folders' check boxes, as a tree of folded names.
#[derive(Debug)]
struct ChoiceTree {
    /// Node 0 is the volume root.
    nodes: Vec<ChoiceNode>,
}

#[derive(Debug, Default)]
struct ChoiceNode {
    /// The folder's own check box, when the settings save one.
    indexed: Option<bool>,
    /// A folder below this one is checked.
    checked_below: bool,
    /// Each child folder that a choice names, or that is above one.
    children: FxHashMap<Vec<u8>, usize>,
}

impl ChoiceTree {
    fn new(root_indexed: bool) -> Self {
        ChoiceTree {
            nodes: vec![ChoiceNode {
                indexed: Some(root_indexed),
                ..ChoiceNode::default()
            }],
        }
    }

    /// Adds the check box of the folder `path` below the root. A path with
    /// no folder names the root, whose check box the drive already gives.
    fn add(&mut self, path: &str, indexed: bool) {
        let mut at = 0;
        let mut above = Vec::new();
        for folded in folded_parts(path) {
            above.push(at);
            at = match self.nodes[at].children.get(&folded) {
                Some(&child) => child,
                None => {
                    self.nodes.push(ChoiceNode::default());
                    let child = self.nodes.len() - 1;
                    self.nodes[at].children.insert(folded, child);
                    child
                }
            };
        }
        if at == 0 {
            return;
        }
        self.nodes[at].indexed = Some(indexed);
        if indexed {
            for node in above {
                self.nodes[node].checked_below = true;
            }
        }
    }

    /// The folded path of every folder that has a check box, the root as an
    /// empty path.
    fn folders(&self) -> Vec<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut stack = vec![(0, Vec::new())];
        while let Some((node, path)) = stack.pop() {
            if self.nodes[node].indexed.is_some() {
                out.push(path.clone());
            }
            for (name, &child) in &self.nodes[node].children {
                let mut below = path.clone();
                below.push(name.clone());
                stack.push((child, below));
            }
        }
        out
    }

    /// Whether the nearest check box at or above the folder `path` is
    /// checked.
    fn indexed_at(&self, path: &[Vec<u8>]) -> bool {
        let mut at = 0;
        let mut indexed = self.nodes[0].indexed.unwrap_or(true);
        for part in path {
            match self.nodes[at].children.get(part) {
                Some(&child) => {
                    at = child;
                    indexed = self.nodes[at].indexed.unwrap_or(indexed);
                }
                None => break,
            }
        }
        indexed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{ParseOptions, QueryStart, parse_query_with};

    const C: &str = r"\\?\Volume{c}\";

    fn settings(drive: bool, folders: &[(&str, bool)]) -> IndexSettings {
        IndexSettings {
            drives: vec![DriveChoice {
                volume: C.into(),
                name: r"C:\ Windows".into(),
                indexed: drive,
                folders: folders
                    .iter()
                    .map(|&(path, indexed)| FolderChoice {
                        path: path.into(),
                        indexed,
                    })
                    .collect(),
            }],
            ..IndexSettings::default()
        }
    }

    /// What the rules decide for the folder `path` below the root.
    fn folder(rules: &VolumeRules, path: &str) -> Verdict {
        let mut place = rules.root();
        let mut verdict = Verdict::Kept(place);
        for part in path.split('\\') {
            verdict = rules.verdict(
                place,
                &fold(part.as_bytes()),
                PackedAttributes::default(),
                true,
                &mut CodePointBuf::new(),
            );
            match verdict {
                Verdict::Kept(p) | Verdict::Path(p) => place = p,
                Verdict::LeftOut => break,
            }
        }
        verdict
    }

    fn kept(verdict: Verdict) -> bool {
        matches!(verdict, Verdict::Kept(_))
    }

    /// The nearest check box decides, in either direction and in any case,
    /// and a folder above a checked one stays as its path.
    #[test]
    fn the_nearest_choice_decides() {
        let rules = settings(true, &[(r"Windows", false), (r"windows\Temp", true)]).rules_for(C);
        assert!(kept(folder(&rules, r"Users\ana")));
        assert_eq!(folder(&rules, r"Windows\System32"), Verdict::LeftOut);
        assert!(kept(folder(&rules, r"WINDOWS\temp\x")));
        assert!(matches!(folder(&rules, "Windows"), Verdict::Path(_)));

        let only = settings(false, &[(r"Users\ana", true)]).rules_for(C);
        assert!(!only.leaves_out_volume());
        assert!(matches!(folder(&only, "Users"), Verdict::Path(_)));
        assert!(kept(folder(&only, r"Users\ana\Documents")));
        assert_eq!(folder(&only, r"Users\bob"), Verdict::LeftOut);
        assert!(
            settings(false, &[("Users", false)])
                .rules_for(C)
                .leaves_out_volume()
        );
    }

    /// A pattern or an excluded attribute leaves an entry out, even below a
    /// checked folder.
    #[test]
    fn names_and_attributes_leave_entries_out() {
        let mut include = settings(true, &[]);
        include.excluded_names = " *.TMP ; node_modules;;".into();
        include.exclude_hidden = true;
        let rules = include.rules_for(C);
        let verdict = |name: &str, attributes: u32| {
            rules.verdict(
                rules.root(),
                &fold(name.as_bytes()),
                PackedAttributes::from_record(attributes),
                false,
                &mut CodePointBuf::new(),
            )
        };
        assert_eq!(verdict("a.tmp", 0), Verdict::LeftOut);
        assert_eq!(verdict("Node_Modules", 0x10), Verdict::LeftOut);
        assert_eq!(verdict("desktop.ini", 0x2 | 0x4), Verdict::LeftOut);
        assert!(kept(verdict("a.txt", 0x4)));
    }

    /// A drive without a choice follows Include new fixed drives
    /// automatically.
    #[test]
    fn a_drive_without_a_choice_follows_include_new_drives() {
        let mut include = IndexSettings::default();
        assert!(include.rules_for(C).keeps_everything());
        include.include_new_drives = false;
        assert!(include.rules_for(C).leaves_out_volume());

        // A drive the window has shown keeps its check box when the person
        // turns Include new fixed drives off.
        let mut shown = IndexSettings::default().with_drives(&[(C.into(), r"C:\ Windows".into())]);
        shown.include_new_drives = false;
        assert!(shown.rules_for(C).keeps_everything());
    }

    /// A click sets the folder and drops the choices below it, a folder
    /// above a choice shows a partial check box, and the summary lists each
    /// checked folder with the folders left out inside it.
    #[test]
    fn a_click_sets_a_folder_and_everything_below_it() {
        let mut include = settings(true, &[]);
        include.toggle_folder(C, "Windows");
        include.toggle_folder(C, r"Windows\Temp");
        include.toggle_folder(C, r"Users\bob");
        assert_eq!(include.folder_check(C, ""), FolderCheck::Partial);
        assert_eq!(include.folder_check(C, "WINDOWS"), FolderCheck::Partial);
        assert_eq!(
            include.folder_check(C, r"Windows\Temp\x"),
            FolderCheck::Checked
        );
        assert_eq!(
            include.folder_check(C, r"Windows\System32"),
            FolderCheck::Unchecked
        );
        let summary: Vec<(String, Vec<String>)> = include
            .summary(C)
            .into_iter()
            .map(|row| (row.path, row.left_out))
            .collect();
        assert_eq!(
            summary,
            [
                (String::new(), vec![r"Users\bob".into(), "Windows".into()]),
                (r"Windows\Temp".into(), Vec::new()),
            ]
        );

        include.toggle_folder(C, "Windows");
        assert_eq!(include.folder_check(C, "Windows"), FolderCheck::Checked);
        assert_eq!(
            include.drives[0].folders,
            [FolderChoice {
                path: r"Users\bob".into(),
                indexed: false
            }]
        );
    }

    /// An edit that is undone is no change, so Settings does not ask for
    /// administrator permission to save it.
    #[test]
    fn an_undone_edit_is_no_change() {
        let mut original = settings(true, &[("Windows", false), (r"Users\bob", false)]);
        original.excluded_names = "*.tmp;node_modules".into();
        let mut edited = original.clone();
        edited.toggle_folder(C, "Windows");
        edited.toggle_folder(C, "Windows");
        edited.excluded_names = " *.TMP ; node_modules;;".into();
        assert_eq!(edited, original);

        edited.toggle_folder(C, "Windows");
        assert_ne!(edited, original);
    }

    /// Only a change that keeps something the applied settings leave out,
    /// or the dates created, needs a read of the volume.
    #[test]
    fn adds_to_finds_what_a_change_brings_back() {
        let applied = settings(true, &[("Windows", false)]);
        let adds = |edited: &IndexSettings| edited.adds_to(&applied, &[C]);
        assert!(!adds(&settings(
            true,
            &[("Windows", false), ("Users", false)]
        )));
        assert!(!adds(&settings(false, &[])));
        assert!(adds(&settings(true, &[])));
        assert!(adds(&settings(
            true,
            &[("Windows", false), (r"Windows\Temp", true)]
        )));

        let mut patterned = applied.clone();
        patterned.excluded_names = "*.tmp".into();
        patterned.exclude_system = true;
        assert!(!patterned.adds_to(&patterned, &[C]));
        assert!(applied.adds_to(&patterned, &[C]));
        assert!(!patterned.adds_to(&applied, &[C]));

        // Dates created on needs a read of the volume, and its change alone
        // is a change, or `Include::reload` would skip it.
        let dated = IndexSettings {
            dates_created: true,
            ..applied.clone()
        };
        assert_ne!(dated, applied);
        assert!(dated.adds_to(&applied, &[C]));
        assert!(!applied.adds_to(&dated, &[C]));
    }

    /// `dc:` on an index without dates would answer nothing, which reads as
    /// "no file was created then"; `dm:` needs no setting.
    #[test]
    fn dc_is_refused_while_dates_created_is_off() {
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        let parse = |text| {
            let options = ParseOptions {
                query_start: Some(&start),
                ..ParseOptions::default()
            };
            parse_query_with(text, options).expect("the query parses")
        };
        let off = IndexSettings::default();
        let on = IndexSettings {
            dates_created: true,
            ..IndexSettings::default()
        };
        assert!(off.allows(&parse("x dc:7days")).is_err());
        assert!(off.allows(&parse("x dm:7days")).is_ok());
        assert!(on.allows(&parse("x dc:7days")).is_ok());
    }
}
