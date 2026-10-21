//! How a volume's index is brought up to date after each journal batch, and
//! the folder walk that this and a ReFS build share. The volume's reads are
//! behind [`VolumeReads`] and the index's lock behind [`SharedIndex`], so all
//! of it runs without a volume.
//!
//! A USN record carries a structural change (create, rename, delete), which
//! [`settle_batch`] applies at once, but no size or date. A `HARD_LINK_CHANGE`
//! record names one link and may name a dead one, so the index cannot learn a
//! file's real link set from the journal either. A folder walk lists one
//! folder at a time, so it is not a snapshot: a folder moved during the walk
//! from a part not yet listed into one already listed is never listed, and the
//! journal records no change to what it holds. After applying the batch,
//! `settle_batch` asks the volume for what the records leave out, in this
//! order:
//!
//! 1. the folders the index missed ([`ApplyOutcome::needs_walk`]), and
//!    everything under them; a listing that fails fails the settle, and the
//!    tail answers with a rescan;
//! 2. the size and date of each changed file;
//! 3. the link set of each file whose links changed.
//!
//! The link step finds each link's folder by its file ID, and that folder may
//! be one the missed-folder walk adds, so the walk comes first.
//!
//! Every read of the volume happens with the index unlocked, because one read
//! can block on a slow disk, and a lock held across it would stall every
//! search on the volume. Each step applies what it read in one short
//! [`SharedIndex::write`].

use std::collections::{HashSet, VecDeque};

use crate::index::{Anomaly, ApplyOutcome, LinkTarget, VolumeIndex};
use crate::layout::LayoutFile;
use crate::types::Frn;
use crate::usn::record::{
    FILE_ATTRIBUTE_REPARSE_POINT, ParsedUsnRecord, USN_REASON_RENAME_NEW_NAME,
};

/// What the walk and the settle read from the volume.
pub trait VolumeReads {
    /// Why a read failed, for a log line and a rescan's reason.
    type Error: std::fmt::Display;

    /// Lists `folder`, handing each page of its names to `page`. `Ok(false)`
    /// means the folder is gone or being deleted, so it holds nothing to list.
    fn list_folder(
        &mut self,
        folder: Frn,
        page: &mut dyn FnMut(Vec<LayoutFile>),
    ) -> Result<bool, Self::Error>;

    /// The size and date of `frn`; `Ok(None)` when there is nothing to
    /// refresh, such as a file that is gone or locked.
    fn stat(&mut self, frn: Frn) -> Result<Option<FileStat>, Self::Error>;

    /// Every current name of `frn`, each with its folder; `Ok(None)` when the
    /// file is gone.
    fn links(&mut self, frn: Frn) -> Result<Option<Vec<FileLink>>, Self::Error>;
}

/// A file's size and last-write time, as [`VolumeIndex::set_stat`] takes them.
pub struct FileStat {
    pub size: u64,
    pub mtime: i64,
    pub created: i64,
}

/// One hard-link name of a file.
pub struct FileLink {
    /// The file ID of the folder that holds the name.
    pub folder: Frn,
    /// The name in WTF-8.
    pub name: Vec<u8>,
}

/// A volume's index behind its lock. Each call is one lock window.
pub trait SharedIndex {
    fn read<R>(&self, f: impl FnOnce(&VolumeIndex) -> R) -> R;
    fn write<R>(&self, f: impl FnOnce(&mut VolumeIndex) -> R) -> R;
}

#[cfg(test)]
impl SharedIndex for std::cell::RefCell<VolumeIndex> {
    fn read<R>(&self, f: impl FnOnce(&VolumeIndex) -> R) -> R {
        f(&self.borrow())
    }

    fn write<R>(&self, f: impl FnOnce(&mut VolumeIndex) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

/// One batch of journal records for [`settle_batch`].
pub struct Batch<'a> {
    pub records: &'a [ParsedUsnRecord],
    /// The reads are still replaying the changes made while a folder walk
    /// listed the volume: a ReFS build's walk, or the walk of folders the
    /// index missed.
    pub replaying_walk: bool,
}

/// What a settled batch leaves for the caller to log and do.
pub struct Settled {
    pub anomalies: Vec<Anomaly>,
    /// The folders the index missed, which the settle listed, and the names it
    /// listed in them.
    pub missed_folders: usize,
    pub missed_entries: usize,
    /// As [`ApplyOutcome::wants_compact`].
    pub wants_compact: bool,
}

/// A folder could not be listed, so the index cannot tell which names it
/// missed.
#[derive(Debug, thiserror::Error)]
#[error("listing folder {folder:?}: {error}")]
pub struct WalkFailed<E> {
    pub folder: Frn,
    pub error: E,
}

/// How a [`walk_folders`] that met no error ended.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum Walk {
    /// Every folder was listed.
    Complete,
    /// `cancelled` stopped the walk before it listed every folder.
    Cancelled,
}

/// Lists every folder in `tops` and every folder under them, and hands each
/// page of listed names to `insert`. `cancelled` is asked before each folder.
///
/// A folder that is gone when the walk lists it is skipped: the journal's
/// delete record removes it. Any other failure fails the walk. A folder
/// reparse point, such as a junction, is listed but not entered, so the walk
/// cannot loop or leave the volume.
///
/// Each folder is listed once. A folder that moved during the walk can be met
/// again, and the journal records every change after its first listing; a
/// listing that names a folder inside itself, which only corrupt data can
/// do, would otherwise make the walk endless.
///
/// # Errors
///
/// [`WalkFailed`] names the first folder that could not be listed.
pub fn walk_folders<R: VolumeReads>(
    reads: &mut R,
    tops: &[Frn],
    mut cancelled: impl FnMut() -> bool,
    mut insert: impl FnMut(Vec<LayoutFile>),
) -> Result<Walk, WalkFailed<R::Error>> {
    let mut folders: VecDeque<Frn> = tops.iter().copied().collect();
    let mut listed: HashSet<Frn> = HashSet::new();
    while let Some(folder) = folders.pop_front() {
        if !listed.insert(folder) {
            continue;
        }
        if cancelled() {
            return Ok(Walk::Cancelled);
        }
        reads
            .list_folder(folder, &mut |files| {
                folders.extend(
                    files
                        .iter()
                        .filter(|f| f.is_dir() && f.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0)
                        .map(|f| f.frn),
                );
                insert(files);
            })
            .map_err(|error| WalkFailed { folder, error })?;
    }
    Ok(Walk::Complete)
}

/// Applies `batch` to `index`, then reads from `reads` what its records leave
/// out and applies that too, in the order the module doc gives.
///
/// `cancelled` is asked before each folder and each file. Once it says stop,
/// the settle returns at once and applies nothing more, compaction included,
/// because the daemon is exiting and would never serve the result.
///
/// A size or date read that fails skips that file. A link read that fails, or
/// a link in a folder the index does not hold yet, skips that file too, and is
/// counted in `IndexStats::link_reconciles_dropped`, because the file's names
/// may then be stale until the next rescan.
///
/// # Errors
///
/// [`WalkFailed`] when a missed folder could not be listed. The batch is
/// applied, but the folder's contents stay missing until a rescan.
pub fn settle_batch<R: VolumeReads>(
    index: &impl SharedIndex,
    reads: &mut R,
    batch: Batch<'_>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Settled, WalkFailed<R::Error>> {
    let ApplyOutcome {
        needs_stat,
        needs_link_reconcile: mut relink,
        needs_walk,
        anomalies,
        wants_compact,
    } = index.write(|i| i.apply_batch(batch.records));
    let mut settled = Settled {
        anomalies,
        missed_folders: 0,
        missed_entries: 0,
        wants_compact: false,
    };

    if batch.replaying_walk {
        // A hard link renamed during the walk from a folder not yet listed
        // into one already listed was never listed, and its rename record
        // replaced the file's other name. The link step restores the file's
        // real names.
        relink.extend(
            batch
                .records
                .iter()
                .filter(|r| r.reason & USN_REASON_RENAME_NEW_NAME != 0 && !r.is_dir())
                .map(|r| r.frn),
        );
        relink.sort_unstable();
        relink.dedup();
    }

    if !needs_walk.is_empty() {
        let mut listed = Vec::new();
        let walk = walk_folders(reads, &needs_walk, &mut cancelled, |files| {
            listed.extend(files);
        })?;
        if walk == Walk::Cancelled {
            return Ok(settled);
        }
        index.write(|i| {
            for file in &listed {
                i.insert_listed(file);
            }
            i.recheck_listed(&needs_walk);
        });
        settled.missed_folders = needs_walk.len();
        settled.missed_entries = listed.len();
    }

    // A file created or changed after the build has no size or date in its
    // record; without this it stays unknown, which the CSV output renders as
    // a folder. A file whose links changed is read too; that does not refresh
    // its names.
    let mut dirty: Vec<Frn> = needs_stat.iter().chain(&relink).copied().collect();
    dirty.sort_unstable();
    dirty.dedup();
    let mut stats = Vec::with_capacity(dirty.len());
    for frn in dirty {
        if cancelled() {
            return Ok(settled);
        }
        if let Ok(Some(stat)) = reads.stat(frn) {
            stats.push((frn, stat));
        }
    }
    if !stats.is_empty() {
        index.write(|i| {
            for (frn, stat) in &stats {
                i.set_stat(*frn, stat.size, stat.mtime);
                i.set_created(*frn, stat.created);
            }
        });
    }

    let mut resolved: Vec<(Frn, Vec<LinkTarget>)> = Vec::new();
    let mut dropped = 0u64;
    for frn in relink {
        if cancelled() {
            return Ok(settled);
        }
        let links = match reads.links(frn) {
            Ok(Some(links)) if !links.is_empty() => links,
            // Gone: leave the entry alone, the journal's delete record removes
            // it. Never reconcile to an empty set, which deletes the file.
            Ok(_) => continue,
            Err(_) => {
                dropped += 1;
                continue;
            }
        };
        // By the folder's file ID: its path would do only if the index were
        // current, and during the replay of a build it is not, because a path
        // can name a folder that a later record renames or swaps.
        let targets = index.read(|i| {
            links
                .into_iter()
                // A name in a folder the Include rules left out is no name
                // the index keeps.
                .filter(|link| !i.is_left_out(link.folder))
                .map(|link| {
                    let parent = i.head_of(link.folder)?;
                    Some(LinkTarget {
                        parent,
                        name: link.name,
                    })
                })
                .collect::<Option<Vec<_>>>()
        });
        // A folder not indexed yet: reconciling to a partial set would delete
        // the links that could not be resolved.
        match targets {
            Some(targets) => resolved.push((frn, targets)),
            None => dropped += 1,
        }
    }
    if dropped > 0 || !resolved.is_empty() {
        index.write(|i| {
            i.note_link_reconciles_dropped(dropped);
            for (frn, targets) in &resolved {
                i.reconcile_links(*frn, targets);
            }
        });
    }

    settled.wants_compact = wants_compact;
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::{
        Batch, FileLink, FileStat, VolumeReads, Walk, WalkFailed, settle_batch, walk_folders,
    };
    use crate::index::{NTFS_ROOT_FRN, VolumeIndex};
    use crate::layout::{LayoutFile, LayoutName};
    use crate::types::Frn;
    use crate::usn::record::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, ParsedUsnRecord,
        USN_REASON_FILE_CREATE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
    };
    use std::cell::RefCell;
    use std::collections::{BTreeSet, HashMap};

    const ROOT: Frn = NTFS_ROOT_FRN;

    fn id(n: u64) -> Frn {
        Frn::ntfs(n, 1)
    }

    fn attributes(is_dir: bool) -> u32 {
        if is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            0x20
        }
    }

    fn record(frn: Frn, parent: Frn, name: &str, is_dir: bool, reason: u32) -> ParsedUsnRecord {
        ParsedUsnRecord {
            frn,
            parent_frn: parent,
            reason,
            attributes: attributes(is_dir),
            name: name.as_bytes().to_vec(),
        }
    }

    fn listed(frn: Frn, folder: Frn, name: &str, is_dir: bool) -> LayoutFile {
        LayoutFile {
            frn,
            attributes: attributes(is_dir),
            size: (!is_dir).then_some(0),
            mtime_ft: Some(0),
            created_ft: None,
            names: vec![LayoutName {
                parent_frn: folder,
                name: name.as_bytes().to_vec(),
                dos_only: false,
            }],
        }
    }

    fn link(folder: Frn, name: &str) -> FileLink {
        FileLink {
            folder,
            name: name.as_bytes().to_vec(),
        }
    }

    /// An index built from `creates`, each a name under its folder.
    fn index_of(creates: &[(Frn, Frn, &str, bool)]) -> RefCell<VolumeIndex> {
        let mut index = VolumeIndex::new(ROOT);
        for &(frn, parent, name, is_dir) in creates {
            index.insert_enum(&record(frn, parent, name, is_dir, USN_REASON_FILE_CREATE));
        }
        RefCell::new(index)
    }

    fn paths(index: &RefCell<VolumeIndex>) -> BTreeSet<String> {
        index.borrow().collect_real_paths().into_iter().collect()
    }

    /// A volume whose folders and files answer from maps; anything not in a
    /// map is gone.
    #[derive(Default)]
    struct Fake {
        listings: HashMap<Frn, Result<Vec<LayoutFile>, &'static str>>,
        links: HashMap<Frn, Result<Vec<FileLink>, &'static str>>,
    }

    impl VolumeReads for Fake {
        type Error = &'static str;

        fn list_folder(
            &mut self,
            folder: Frn,
            page: &mut dyn FnMut(Vec<LayoutFile>),
        ) -> Result<bool, Self::Error> {
            match self.listings.get(&folder) {
                None => Ok(false),
                Some(Ok(files)) => {
                    page(files.clone());
                    Ok(true)
                }
                Some(Err(e)) => Err(*e),
            }
        }

        fn stat(&mut self, _: Frn) -> Result<Option<FileStat>, Self::Error> {
            Ok(None)
        }

        fn links(&mut self, frn: Frn) -> Result<Option<Vec<FileLink>>, Self::Error> {
            match self.links.remove(&frn) {
                None => Ok(None),
                Some(links) => links.map(Some),
            }
        }
    }

    fn settle(
        index: &RefCell<VolumeIndex>,
        reads: &mut Fake,
        records: &[ParsedUsnRecord],
        replaying_walk: bool,
    ) -> Result<(), WalkFailed<&'static str>> {
        let batch = Batch {
            records,
            replaying_walk,
        };
        settle_batch(index, reads, batch, || false).map(|_| ())
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    /// A junction can point outside the volume or at a folder above it, so
    /// entering it would index another volume's files or never end.
    #[test]
    fn a_walk_lists_a_junction_but_does_not_enter_it() {
        let (junction, inside) = (id(100), id(101));
        let mut reads = Fake::default();
        let mut entry = listed(junction, ROOT, "link", true);
        entry.attributes |= FILE_ATTRIBUTE_REPARSE_POINT;
        reads.listings.insert(ROOT, Ok(vec![entry]));
        reads
            .listings
            .insert(junction, Ok(vec![listed(inside, junction, "a.txt", false)]));

        let mut names = Vec::new();
        let walk = walk_folders(
            &mut reads,
            &[ROOT],
            || false,
            |files| {
                names.extend(files.into_iter().map(|f| f.frn));
            },
        );

        assert_eq!(walk.expect("lists"), Walk::Complete);
        assert_eq!(names, [junction]);
    }

    /// On ReFS a walk lists `a` and then, before it lists `c`, the file's name
    /// in `c` is renamed to `a\two.txt`. The walk met the file once, as
    /// `a\one.txt`, and the replayed rename moves that one name. Without the
    /// replaying-walk rule `a\one.txt` vanishes until a rescan.
    #[test]
    fn a_link_renamed_during_a_walk_keeps_the_files_other_names() {
        let (a, file) = (id(100), id(200));
        let index = index_of(&[(a, ROOT, "a", true), (file, a, "one.txt", false)]);
        let mut reads = Fake::default();
        reads
            .links
            .insert(file, Ok(vec![link(a, "one.txt"), link(a, "two.txt")]));
        let rename = record(file, a, "two.txt", false, USN_REASON_RENAME_NEW_NAME);

        settle(&index, &mut reads, &[rename], true).expect("settles");

        assert_eq!(paths(&index), set(&["a", r"a\one.txt", r"a\two.txt"]));
    }

    /// A folder renamed in from outside the index holds `s`, and a file whose
    /// links changed now lives in `s`. The link step finds `s` only after the
    /// missed-folder walk has listed it; the other order drops the reconcile
    /// and leaves the file's old name.
    #[test]
    fn a_missed_folder_is_listed_before_its_links_resolve() {
        let (m, s, file) = (id(100), id(101), id(200));
        let index = index_of(&[(file, ROOT, "f.txt", false)]);
        let mut reads = Fake::default();
        reads.listings.insert(m, Ok(vec![listed(s, m, "s", true)]));
        reads
            .listings
            .insert(s, Ok(vec![listed(file, s, "f.txt", false)]));
        reads.links.insert(file, Ok(vec![link(s, "f.txt")]));
        let records = [
            record(m, ROOT, "m", true, USN_REASON_RENAME_NEW_NAME),
            record(file, ROOT, "f.txt", false, USN_REASON_HARD_LINK_CHANGE),
        ];

        settle(&index, &mut reads, &records, false).expect("settles");

        assert_eq!(paths(&index), set(&["m", r"m\s", r"m\s\f.txt"]));
        assert_eq!(index.borrow().stats().link_reconciles_dropped, 0);
    }

    /// A missed folder that cannot be listed fails the settle, so the tail
    /// rescans. Treated like a gone folder, its contents would be lost for
    /// good, because the journal records no change to them.
    /// A folder that moves out of a folder the Include rules left out is
    /// listed, and the walk's names that the rules leave out stay out.
    #[test]
    fn a_folder_moved_out_of_a_left_out_folder_is_listed_by_the_rules() {
        let (windows, users, old, file, modules, js) =
            (id(100), id(101), id(102), id(200), id(103), id(201));
        let index = index_of(&[
            (windows, ROOT, "Windows", true),
            (users, ROOT, "Users", true),
            (old, windows, "Old", true),
            (file, old, "a.txt", false),
            (modules, old, "node_modules", true),
            (js, modules, "m.js", false),
        ]);
        let settings = crate::include::IndexSettings {
            excluded_names: "node_modules".into(),
            drives: vec![crate::include::DriveChoice {
                volume: "V".into(),
                name: String::new(),
                indexed: true,
                folders: vec![crate::include::FolderChoice {
                    path: "Windows".into(),
                    indexed: false,
                }],
            }],
            ..crate::include::IndexSettings::default()
        };
        index
            .borrow_mut()
            .apply_rules(std::sync::Arc::new(settings.rules_for("V")));
        let mut reads = Fake::default();
        reads.listings.insert(
            old,
            Ok(vec![
                listed(file, old, "a.txt", false),
                listed(modules, old, "node_modules", true),
            ]),
        );
        reads
            .listings
            .insert(modules, Ok(vec![listed(js, modules, "m.js", false)]));
        let moved = record(old, users, "Old", true, USN_REASON_RENAME_NEW_NAME);

        settle(&index, &mut reads, &[moved], false).expect("settles");

        assert_eq!(
            paths(&index),
            set(&["Users", r"Users\Old", r"Users\Old\a.txt"])
        );
        assert_eq!(index.borrow().stats().placeholders_created, 0);
    }

    #[test]
    fn a_folder_that_cannot_be_listed_fails_the_settle() {
        let m = id(100);
        let index = index_of(&[]);
        let mut reads = Fake::default();
        reads.listings.insert(m, Err("access denied"));
        let rename = record(m, ROOT, "m", true, USN_REASON_RENAME_NEW_NAME);

        let failed = settle(&index, &mut reads, &[rename], false);

        assert!(matches!(failed, Err(WalkFailed { folder, .. }) if folder == m));
    }

    /// A link read that fails leaves the file's names as they are and is
    /// counted. Mapped to an empty link set, it would erase a live file.
    #[test]
    fn a_link_read_that_fails_leaves_the_file_and_is_counted() {
        let file = id(200);
        let index = index_of(&[(file, ROOT, "a.txt", false)]);
        let mut reads = Fake::default();
        reads.links.insert(file, Err("sharing violation"));
        let change = record(file, ROOT, "a.txt", false, USN_REASON_HARD_LINK_CHANGE);

        settle(&index, &mut reads, &[change], false).expect("settles");

        assert_eq!(paths(&index), set(&["a.txt"]));
        assert_eq!(index.borrow().stats().link_reconciles_dropped, 1);
    }
}
