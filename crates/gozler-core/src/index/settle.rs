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
//! 1. the descriptor of each folder that has none or whose permissions
//!    changed ([`ApplyOutcome::needs_descriptor`]);
//! 2. the folders the index missed ([`ApplyOutcome::needs_walk`]), and
//!    everything under them, with each one's descriptor, which step 1 then
//!    leaves out; a listing that fails fails the settle, and the tail answers
//!    with a rescan;
//! 3. the size and date of each changed file;
//! 4. the link set of each file whose links changed.
//!
//! Such a folder's entries are hidden until its descriptor is read, and the
//! later steps can take seconds, so the descriptors come first. The link step
//! finds each link's folder by its file ID, and that folder may be one the
//! missed-folder walk adds, so the walk comes before it.
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

    /// Lists `folder`: hands `listed` the folder's descriptor once, then each
    /// page of its names. `Ok(false)` means the folder is gone or being
    /// deleted, so it holds nothing to list. A descriptor read that fails
    /// does not fail the listing.
    fn list_folder(
        &mut self,
        folder: Frn,
        listed: &mut dyn FnMut(Listed<Self::Error>),
    ) -> Result<bool, Self::Error>;

    /// The security descriptor of folder `frn`; `Ok(None)` when the folder
    /// is gone or being deleted.
    fn folder_descriptor(&mut self, frn: Frn) -> Result<Option<Vec<u8>>, Self::Error>;

    /// The security descriptor of folder `frn`, then the USN of its last
    /// journal record, both from one open; `Ok(None)` when the folder is gone
    /// or being deleted. Only the NTFS build asks for it.
    fn folder_security(&mut self, frn: Frn) -> Result<Option<FolderSecurity>, Self::Error>;

    /// The size and date of `frn`; `Ok(None)` when there is nothing to
    /// refresh, such as a file that is gone or locked.
    fn stat(&mut self, frn: Frn) -> Result<Option<FileStat>, Self::Error>;

    /// Every current name of `frn`, each with its folder; `Ok(None)` when the
    /// file is gone.
    fn links(&mut self, frn: Frn) -> Result<Option<Vec<FileLink>>, Self::Error>;
}

/// What a folder listing hands its caller, in the order the walk reads it.
pub enum Listed<E> {
    /// Once per folder the walk opens, before its first page. The folder
    /// comes from the walk, not from a listing, because an empty folder
    /// lists no page.
    Folder {
        frn: Frn,
        descriptor: Result<Vec<u8>, E>,
    },
    /// Each page of that folder's listing.
    Page(Vec<LayoutFile>),
}

impl<E> Listed<E> {
    /// Applies one event of a walk: a folder's descriptor, or a page's names.
    /// A descriptor read that failed changes nothing, and the folder is
    /// returned, for the caller to pass to
    /// [`VolumeIndex::note_descriptor_reads_failed`] once the Include rules
    /// have checked what the walk added.
    ///
    /// A subfolder's entry comes from its parent's page, which a walk gives
    /// before the subfolder's own `Folder`, so events must be applied in walk
    /// order: [`VolumeIndex::set_folder_descriptor`] ignores a folder not yet
    /// in the index.
    #[must_use = "a failed descriptor read is counted by the caller"]
    pub fn apply(&self, index: &mut VolumeIndex) -> Option<Frn> {
        match self {
            Listed::Folder {
                frn,
                descriptor: Ok(descriptor),
            } => index.set_folder_descriptor(*frn, Some(descriptor)),
            Listed::Folder {
                frn,
                descriptor: Err(_),
            } => return Some(*frn),
            Listed::Page(files) => {
                for file in files {
                    index.insert_listed(file);
                }
            }
        }
        None
    }
}

/// A file's size and last-write time, as [`VolumeIndex::set_stat`] takes them.
pub struct FileStat {
    pub size: u64,
    pub mtime: i64,
    pub created: i64,
}

/// A folder's descriptor and the USN of its last journal record, read in
/// that order, so a permission change after the descriptor read gives a
/// later USN.
pub struct FolderSecurity {
    pub descriptor: Vec<u8>,
    pub usn: i64,
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
/// folder's descriptor and each page of listed names to `insert`, in walk
/// order. `cancelled` is asked before each folder.
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
    mut insert: impl FnMut(Listed<R::Error>),
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
            .list_folder(folder, &mut |event| {
                if let Listed::Page(files) = &event {
                    folders.extend(
                        files
                            .iter()
                            .filter(|f| {
                                f.is_dir() && f.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
                            })
                            .map(|f| f.frn),
                    );
                }
                insert(event);
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
/// A descriptor read that fails leaves its folder with none, which hides the
/// folder's entries, and is counted in `IndexStats::descriptor_reads_failed`
/// while the index holds the folder.
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
        needs_descriptor,
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

    // The missed-folder walk reads the descriptor of each folder it lists.
    let needs_descriptor: Vec<Frn> = needs_descriptor
        .into_iter()
        .filter(|frn| needs_walk.binary_search(frn).is_err())
        .collect();
    // A new subfolder of a missed folder is read by both steps, so the
    // failures of both are counted once, after the rules checked the walk.
    let mut failed = Vec::new();
    if !needs_descriptor.is_empty() {
        let mut descriptors = Vec::with_capacity(needs_descriptor.len());
        for frn in needs_descriptor {
            if cancelled() {
                return Ok(settled);
            }
            match reads.folder_descriptor(frn) {
                Ok(Some(descriptor)) => descriptors.push((frn, descriptor)),
                // Gone: the journal's delete record removes it.
                Ok(None) => {}
                Err(_) => failed.push(frn),
            }
        }
        index.write(|i| {
            for (frn, descriptor) in &descriptors {
                i.set_folder_descriptor(*frn, Some(descriptor));
            }
        });
    }

    if !needs_walk.is_empty() {
        let mut events = Vec::new();
        let walk = walk_folders(reads, &needs_walk, &mut cancelled, |listed| {
            events.push(listed);
        })?;
        if walk == Walk::Cancelled {
            return Ok(settled);
        }
        let entries = events
            .iter()
            .map(|e| match e {
                Listed::Page(files) => files.len(),
                Listed::Folder { .. } => 0,
            })
            .sum();
        index.write(|i| {
            failed.extend(events.iter().filter_map(|event| event.apply(i)));
            i.recheck_listed(&needs_walk);
        });
        settled.missed_folders = needs_walk.len();
        settled.missed_entries = entries;
    }
    if !failed.is_empty() {
        failed.sort_unstable();
        failed.dedup();
        index.write(|i| i.note_descriptor_reads_failed(&failed));
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
        Batch, FileLink, FileStat, FolderSecurity, Listed, VolumeReads, Walk, WalkFailed,
        settle_batch, walk_folders,
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
            security_id: None,
            usn: None,
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
    /// map is gone. A folder's descriptor is its file ID's bytes unless
    /// `descriptors` says otherwise.
    #[derive(Default)]
    struct Fake {
        listings: HashMap<Frn, Result<Vec<LayoutFile>, &'static str>>,
        links: HashMap<Frn, Result<Vec<FileLink>, &'static str>>,
        descriptors: HashMap<Frn, Result<Option<Vec<u8>>, &'static str>>,
    }

    fn descriptor(frn: Frn) -> Vec<u8> {
        frn.0.to_le_bytes().to_vec()
    }

    impl VolumeReads for Fake {
        type Error = &'static str;

        fn list_folder(
            &mut self,
            folder: Frn,
            listed: &mut dyn FnMut(Listed<Self::Error>),
        ) -> Result<bool, Self::Error> {
            match self.listings.get(&folder) {
                None => Ok(false),
                Some(Ok(files)) => {
                    let descriptor = match self.descriptors.get(&folder) {
                        Some(Err(e)) => Err(*e),
                        _ => Ok(descriptor(folder)),
                    };
                    listed(Listed::Folder {
                        frn: folder,
                        descriptor,
                    });
                    listed(Listed::Page(files.clone()));
                    Ok(true)
                }
                Some(Err(e)) => Err(*e),
            }
        }

        fn folder_descriptor(&mut self, frn: Frn) -> Result<Option<Vec<u8>>, Self::Error> {
            self.descriptors
                .get(&frn)
                .cloned()
                .unwrap_or_else(|| Ok(Some(descriptor(frn))))
        }

        fn folder_security(&mut self, _: Frn) -> Result<Option<FolderSecurity>, Self::Error> {
            unreachable!("the settle and the walk read no folder security")
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
            |listed| {
                if let Listed::Page(files) = listed {
                    names.extend(files.into_iter().map(|f| f.frn));
                }
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
    /// listed, and the walk's names that the rules leave out stay out. A
    /// descriptor read that fails is counted once, also for a new subfolder
    /// that the settle and the walk both read, and only for a folder the
    /// index keeps, so `--status` names no hidden folder twice and none that
    /// does not exist.
    #[test]
    fn a_folder_moved_out_of_a_left_out_folder_is_listed_by_the_rules() {
        let (windows, users, old, file, modules, js, new) = (
            id(100),
            id(101),
            id(102),
            id(200),
            id(103),
            id(201),
            id(104),
        );
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
                listed(new, old, "New", true),
            ]),
        );
        reads
            .listings
            .insert(modules, Ok(vec![listed(js, modules, "m.js", false)]));
        reads.listings.insert(new, Ok(vec![]));
        for folder in [old, modules, new] {
            reads.descriptors.insert(folder, Err("access denied"));
        }
        let records = [
            record(old, users, "Old", true, USN_REASON_RENAME_NEW_NAME),
            record(new, old, "New", true, USN_REASON_FILE_CREATE),
        ];

        settle(&index, &mut reads, &records, false).expect("settles");

        assert_eq!(
            paths(&index),
            set(&["Users", r"Users\Old", r"Users\Old\a.txt", r"Users\Old\New"])
        );
        let stats = index.borrow().stats().clone();
        assert_eq!(stats.placeholders_created, 0);
        assert_eq!(stats.descriptor_reads_failed, 2, "Old and New, once each");
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

    /// A new folder's descriptor is read in the batch that adds it, or its
    /// entries stay hidden from every filtered user until a rescan. A read
    /// that fails is counted, so `--status` shows the hidden folder.
    #[test]
    fn a_new_folder_gets_its_descriptor_in_the_same_batch() {
        let (read, failing) = (id(100), id(101));
        let index = index_of(&[]);
        let mut reads = Fake::default();
        reads.descriptors.insert(failing, Err("access denied"));
        let records = [
            record(read, ROOT, "read", true, USN_REASON_FILE_CREATE),
            record(failing, ROOT, "failing", true, USN_REASON_FILE_CREATE),
        ];

        settle(&index, &mut reads, &records, false).expect("settles");

        let index = index.borrow();
        assert_eq!(index.folder_descriptor(read), Some(&descriptor(read)[..]));
        assert_eq!(index.folder_descriptor(failing), None);
        assert_eq!(index.stats().descriptor_reads_failed, 1);
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
