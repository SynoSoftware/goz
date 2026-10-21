//! The per-volume index and the rules that mutate it.
//!
//! [`VolumeIndex`] owns the entry table, name arena, and FRN map, and applies
//! USN ops and FILE_LAYOUT enrichment to them. The invariants it upholds:
//!
//! - Paths are never stored. They are rebuilt on demand by walking parent
//!   links, which makes a directory rename O(1): renaming a directory touches
//!   exactly its own entry, and every descendant's path is correct on the next
//!   reconstruction.
//! - Every op is idempotent. USN reason bits accumulate per open-close window,
//!   so the same logical change reappears; applying a batch twice equals
//!   applying it once.
//! - Orphans are never dropped. An unknown parent becomes a placeholder under
//!   lost+found, filled in when the parent's real record arrives.
//! - Slot reuse can never alias. On NTFS the FRN map is keyed by record
//!   number and every hit is re-checked against the full FRN (sequence
//!   included); on ReFS it is keyed by the whole file ID.
//! - Folder sizes stay exact. From [`VolumeIndex::compute_folder_sizes`] on,
//!   a folder's size slot holds the sum of the known sizes of the files below
//!   it, and every change adds its difference to each folder above it, so a
//!   folder's size costs no memory and no walk of its contents.

use super::store::{
    CompactedNames, ComponentBytes, EntryFlags, EntryTable, FrnMap, IdWidth, MTIME_UNKNOWN, NameId,
    NamePair, NameRanks, NameStore, SIZE_UNKNOWN, hash_bytes, moved,
};
use crate::attributes::{Attributes, PackedAttributes};
use crate::include::VolumeRules;
use crate::layout::LayoutFile;
use crate::types::{EntryIdx, Frn, NIL};
use crate::usn::record::USN_REASON_RENAME_NEW_NAME;
use crate::usn::{ParsedUsnRecord, UsnOp, ops_for};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

mod include;
use include::Screen;

/// NTFS reserves MFT record 5 for the volume root directory. The daemon reads
/// each volume's root ID from the volume, and gives this one only to the
/// empty index of a volume not built yet; tests and benches build
/// NTFS-shaped indexes with it.
pub const NTFS_ROOT_FRN: Frn = Frn::ntfs(5, 5);

/// Synthetic name for the lost+found root (never collides: `$` names are NTFS
/// metafiles, and this FRN is unreachable).
const LOST_FOUND_NAME: &[u8] = b"$LostFound";
/// Synthetic name a placeholder carries until its real record fills it in.
const PLACEHOLDER_NAME: &[u8] = b"$Placeholder";
/// FRN for the synthetic lost+found root (record number above any real MFT
/// record; never appears in a USN record).
const LOST_FOUND_FRN: Frn = Frn(u64::MAX as u128);

/// The instance the next index takes ([`VolumeIndex::instance`]).
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(0);

fn next_instance() -> u64 {
    NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed)
}

/// Maximum parent hops before [`VolumeIndex::path_of`] declares a cycle. Deep
/// but bounded; real NTFS trees are far shallower.
pub(crate) const MAX_PATH_HOPS: usize = 4096;

/// A compaction built off the exclusive lock, ready to install.
///
/// Opaque: only [`VolumeIndex::plan_compaction`] makes one and only
/// [`VolumeIndex::apply_compaction`] consumes it, so a plan can never be applied
/// to an index it was not built from.
pub struct CompactPlan(Compaction);

enum Compaction {
    /// New name arenas, which carry the ranks, merged when they were stale.
    Arenas(CompactedNames),
    /// Merged ranks over the current arenas.
    Ranks(NameRanks),
}

/// One resolved hard link: the containing directory's entry plus the link
/// name. Passed to [`VolumeIndex::reconcile_links`] by the daemon after it
/// walks `FindFirstFileNameW`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkTarget {
    pub parent: EntryIdx,
    /// The link name in WTF-8.
    pub name: Vec<u8>,
}

/// Result of walking parent links to build a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStatus {
    /// A normal path rooted at the volume root.
    Ok,
    /// The chain terminates in lost+found (an unresolved orphan).
    InLostFound,
    /// The hop cap was hit: a loop in the parent links, which a journal replay
    /// can close until its later records open it again, or a real folder tree
    /// deeper than the cap, which a rescan rebuilds the same way. The entry has
    /// no trustworthy path.
    CycleDetected,
    /// The entry was removed from the index. Only an entry index held across
    /// index changes can name one.
    Removed,
}

/// A non-fatal irregularity observed during apply, counted for diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anomaly {
    /// A delete arrived for an FRN not in the index.
    DeleteOfUnknown { frn: Frn },
}

/// Running counters for observability, surfaced per volume in `Status`.
///
/// These are the only outward sign that the index and the volume have drifted:
/// individually they are normal on a busy volume, but a number that climbs
/// steadily is the signal that something is wrong.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub placeholders_created: u64,
    pub delete_of_unknown: u64,
    pub stale_slots: u64,
    /// Hard-link changes the daemon saw but could not reconcile: the link walk
    /// failed, or a link's folder is not indexed yet. Such a walk is skipped
    /// rather than reconciled to a partial set, so a few names may be stale
    /// until the next rescan. A file that is gone is not counted, because its
    /// delete record removes it.
    pub link_reconciles_dropped: u64,
}

/// Where one volume index's heap bytes live, component by component.
/// `used` is live data; `allocated` includes capacity slack. The breakdown
/// lets a memory change be judged against one component instead of a single
/// opaque process number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexMemory {
    /// Struct-of-arrays entry columns (frn/parent/name/flags/size/mtime/
    /// attributes/links).
    pub entries: ComponentBytes,
    /// Original-case name bytes.
    pub arena_raw: ComponentBytes,
    /// Case-folded name bytes (the query haystack).
    pub arena_folded: ComponentBytes,
    /// FRN → entry map.
    pub frn_map: ComponentBytes,
    /// Per-unique-name tables (pairs, refcounts, chain heads, intern table).
    pub name_tables: ComponentBytes,
    /// The `(parent, folded name) → dir` scope index (bucket estimate + exact
    /// key-heap bytes).
    pub dir_children: ComponentBytes,
    /// "dense" / "sparse" / "wide" (which `FrnMap` backing is in use).
    pub frn_map_kind: &'static str,
}

impl IndexMemory {
    pub fn total(&self) -> ComponentBytes {
        self.entries
            + self.arena_raw
            + self.arena_folded
            + self.frn_map
            + self.name_tables
            + self.dir_children
    }
}

/// Side effects a batch produced that the daemon must service out of band.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// FRNs whose size/mtime changed and need an external stat, which
    /// [`settle_batch`](crate::index::settle::settle_batch) reads.
    pub needs_stat: Vec<Frn>,
    /// FRNs whose hard links changed and need an external link-set walk, each
    /// once.
    ///
    /// [`settle_batch`](crate::index::settle::settle_batch) reads each FRN's
    /// real link set from the volume and applies it through
    /// [`VolumeIndex::reconcile_links`]. A walk that cannot complete is
    /// counted in `IndexStats::link_reconciles_dropped`.
    pub needs_link_reconcile: Vec<Frn>,
    /// Folders the index did not hold until a rename brought them in, so
    /// their contents are missing, each once.
    ///
    /// A folder walk lists one folder at a time, so a folder moved during the
    /// walk from a part not yet listed into one already listed is never
    /// listed, and the journal records no change to its contents.
    /// [`settle_batch`](crate::index::settle::settle_batch) walks each of
    /// these folders and inserts what it lists with
    /// [`VolumeIndex::insert_listed`].
    pub needs_walk: Vec<Frn>,
    /// Irregularities worth logging.
    pub anomalies: Vec<Anomaly>,
    /// The name arenas are dirty enough to be worth compacting, or the name
    /// ranks stale enough to merge. The caller should
    /// [`VolumeIndex::plan_compaction`] off the write lock and then
    /// [`VolumeIndex::apply_compaction`].
    pub wants_compact: bool,
}

/// Whether the index keeps a size for each folder yet, and whether that
/// size can be short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderSizes {
    /// The size and date pass has not ended, so every folder's size is
    /// unknown, as its files' sizes are.
    Unknown,
    /// Each folder's size slot holds the sum of the known sizes of the files
    /// below it, and every change keeps it exact.
    Kept,
    /// As `Kept`, but the pass stopped before it read every file's size, so
    /// a folder's size is at least the sum it holds.
    AtLeast,
}

enum UpsertResult {
    Ok,
    /// The FRN has multiple links; a rename cannot be localized from the NEW
    /// record alone, so the daemon must reconcile the file's links.
    NeedsReconcile,
}

/// One indexed volume.
pub struct VolumeIndex {
    entries: EntryTable,
    names: NameStore,
    by_frn: FrnMap,
    root: EntryIdx,
    lost_found: EntryIdx,
    /// Which index this is, among every index of the process; see
    /// [`Self::instance`].
    instance: u64,
    generation: u64,
    stats: IndexStats,
    /// Set by [`Self::compute_folder_sizes`].
    folder_sizes: FolderSizes,
    /// Entries structurally mutated since the last [`Self::take_write_count`].
    #[cfg(test)]
    dirty: u64,
    /// Persistent `(parent entry, hash of folded name) → entry` index over
    /// real DIRECTORIES only, maintained on every mutation. Turns `-path`
    /// scope resolution into an O(depth) walk instead of an O(all-entries)
    /// scan. Keyed by the folded name's [`hash_bytes`] (insert, remove and
    /// lookup must agree, or the index silently loses directories) rather
    /// than owned name bytes: the bytes live once in the interned name store.
    /// Lookups verify the candidate's actual folded name, so a hash collision
    /// can never resolve a scope to the wrong directory. Directories are far
    /// rarer than files, so this stays small; placeholders and
    /// root/lost+found are excluded (they are never a scope component).
    /// Values are a 1-inline SmallVec: a genuine 64-bit hash collision
    /// between two sibling directories is astronomically rare, but when it
    /// happens both live here and `child_dir` disambiguates by comparing the
    /// actual folded names.
    dir_children: FxHashMap<(EntryIdx, u64), SmallVec<[EntryIdx; 1]>>,
    /// The Include rules the index was cut down to, which every journal
    /// batch is checked against; `None` while they keep every entry.
    include: Option<Arc<VolumeRules>>,
    /// The file IDs of the folders the rules left out, so a journal record
    /// inside one is dropped.
    left_out: FxHashSet<Frn>,
}

impl VolumeIndex {
    /// [`Self::with_ids`] with the width of `root`, for a caller with no
    /// filesystem to go by, such as a test or a bench.
    pub fn new(root: Frn) -> Self {
        let ids = if root.is_wide() {
            IdWidth::Wide
        } else {
            IdWidth::Narrow
        };
        Self::with_ids(root, ids)
    }

    /// Builds an empty index of a volume whose file IDs are `ids` wide, with
    /// the volume's root folder, whose file ID is `root`, and a lost+found
    /// root. Narrow IDs take 8 bytes each and an FRN map keyed by record
    /// number, which starts sparse and which [`Self::optimize_storage`]
    /// makes dense when that is smaller; wide IDs take 16 bytes and a map
    /// keyed by the whole ID.
    pub fn with_ids(root: Frn, ids: IdWidth) -> Self {
        let mut entries = EntryTable::new(ids);
        let mut names = NameStore::new();
        let mut by_frn = FrnMap::new(ids);

        let root_name = names.intern(b"");
        let root_idx = entries.alloc(
            root,
            NIL,
            root_name,
            EntryFlags::DIR,
            SIZE_UNKNOWN,
            MTIME_UNKNOWN,
            PackedAttributes::default(),
            NIL,
        );
        by_frn.set(root, root_idx);

        // Lost+found: synthetic, not in the FRN map (never a USN target).
        let lf_name = names.intern(LOST_FOUND_NAME);
        let lost_found = entries.alloc(
            LOST_FOUND_FRN,
            NIL,
            lf_name,
            EntryFlags::DIR,
            SIZE_UNKNOWN,
            MTIME_UNKNOWN,
            PackedAttributes::default(),
            NIL,
        );

        let mut this = Self {
            entries,
            names,
            by_frn,
            root: root_idx,
            lost_found,
            instance: next_instance(),
            generation: 0,
            stats: IndexStats::default(),
            folder_sizes: FolderSizes::Unknown,
            #[cfg(test)]
            dirty: 0,
            dir_children: FxHashMap::default(),
            include: None,
            left_out: FxHashSet::default(),
        };
        this.link_same_name(root_idx, root_name);
        this.link_same_name(lost_found, lf_name);
        this
    }

    /// Whether anything sits under lost+found: a placeholder for a parent
    /// whose record has not arrived, or a child of a removed folder.
    ///
    /// False means every entry's walk ends at the root, but an entry whose
    /// walk is longer than `MAX_PATH_HOPS` still has no full path. The
    /// query engine uses this single load to skip its per-directory
    /// presentability walk on a healthy index, so such an entry counts in a
    /// search's total and is left out only when its page is read.
    pub fn has_orphans(&self) -> bool {
        self.entries.first_child(self.lost_found) != NIL
    }

    pub fn root(&self) -> EntryIdx {
        self.root
    }

    /// A number that no other index of the process has had, and that changes
    /// when the entries get new indexes. While it stays the same, an entry
    /// index names the same slot, though the slot may have died or been
    /// reused since.
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn stats(&self) -> &IndexStats {
        &self.stats
    }

    /// Live entry count (includes root + lost+found).
    pub fn len(&self) -> usize {
        self.entries.live()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entries structurally mutated since the previous call. The
    /// directory-rename test asserts this is exactly 1.
    #[cfg(test)]
    pub fn take_write_count(&mut self) -> u64 {
        core::mem::take(&mut self.dirty)
    }

    // -- bootstrap ingestion ---------------------------------------------

    /// Ingests one `FSCTL_ENUM_USN_DATA` record (bootstrap): the file exists
    /// with this name/parent and these attributes. Idempotent.
    pub fn insert_enum(&mut self, rec: &ParsedUsnRecord) {
        self.upsert_link(rec.frn, rec.parent_frn, &rec.name, rec.is_dir());
        self.set_attributes(rec.frn, rec.attributes);
    }

    /// Applies FILE_LAYOUT enrichment: sets size/mtime and attributes and
    /// reconciles the file's hard-link names (dropping 8.3 DOS-only aliases).
    pub fn enrich(&mut self, file: &LayoutFile) {
        let is_dir = file.is_dir();

        // Enrichment is ADD-ONLY: it never tombstones or replaces an existing
        // entry. This is critical: the entry may be a directory that already
        // has children pointing at its EntryIdx, and tombstone + realloc here
        // would orphan those children under a reused slot.
        if self.head_of(file.frn).is_none() {
            // No entry of this FRN exists, so nothing of it is orphaned:
            // build the chain from LAYOUT.
            let desired: Vec<LinkTarget> = file
                .names
                .iter()
                .filter(|n| !n.dos_only)
                .map(|n| LinkTarget {
                    parent: self.resolve_or_placeholder_parent(n.parent_frn),
                    name: n.name.clone(),
                })
                .collect();
            if desired.is_empty() {
                return;
            }
            self.set_chain_links(file.frn, is_dir, &desired);
        }

        // Add hard-link names ENUM did not report (files only: a directory
        // has exactly one link and its name is authoritative from ENUM/the
        // journal, so we never touch it here).
        if !is_dir {
            for n in file.names.iter().filter(|n| !n.dos_only) {
                let parent_idx = self.resolve_or_placeholder_parent(n.parent_frn);
                if !self.chain_has_link(file.frn, parent_idx, &n.name) {
                    self.add_link(file.frn, parent_idx, &n.name);
                }
            }
        }

        if let Some(head) = self.head_of(file.frn) {
            self.set_chain_stat(head, file.size, file.mtime_ft, file.created_ft);
        }
        self.set_attributes(file.frn, file.attributes);
    }

    /// Ingests one FILE_LAYOUT record, the bootstrap's first and usually only
    /// pass: its first name stands in for the ENUM record, which also fills a
    /// folder that an earlier child left as a placeholder, then
    /// [`Self::enrich`] adds the other hard-link names, the size and the date.
    /// An ENUM record ingested later for the same file rewrites a single-name
    /// entry in place and leaves a hard-link chain as it is. The root folder's
    /// own record names itself as its parent, unlike any ENUM record; it only
    /// enriches the root that [`Self::new`] made, because taking its name and
    /// parent would make the root its own parent and leave no entry a path.
    pub fn insert_layout(&mut self, file: &LayoutFile) {
        let is_dir = file.is_dir();
        let first_name = file.names.iter().find(|n| !n.dos_only);
        if let Some(name) = first_name.filter(|n| n.parent_frn != file.frn) {
            self.upsert_link(file.frn, name.parent_frn, &name.name, is_dir);
        }
        self.enrich(file);
    }

    /// Ingests one name a folder walk listed: `file` has that single name.
    ///
    /// A walk is not a snapshot, so it can meet an entry again. A file met in
    /// a second folder is a hard link or moved during the walk, so its name
    /// is added beside the others, and the link reconcile of its replayed
    /// rename record leaves only its real names (see
    /// [`Batch::replaying_walk`](crate::index::settle::Batch::replaying_walk)).
    /// A folder met again moved during the walk, and its replayed rename
    /// record moves it. So a known entry is only enriched, never renamed. An entry the index holds as a
    /// placeholder takes the listed name and folder, because its own record
    /// never reached the index.
    pub fn insert_listed(&mut self, file: &LayoutFile) {
        if self.lacks(file.frn)
            && let Some(name) = file.names.first()
        {
            self.upsert_link(file.frn, name.parent_frn, &name.name, file.is_dir());
        }
        self.enrich(file);
    }

    // -- live apply -------------------------------------------------------

    /// Applies a batch of journal records, returning the side effects the
    /// daemon must service. Bumps the generation once and reports through
    /// [`ApplyOutcome::wants_compact`] whether the arenas need compacting or
    /// the name ranks need a merge.
    pub fn apply_batch(&mut self, records: &[ParsedUsnRecord]) -> ApplyOutcome {
        let mut outcome = ApplyOutcome::default();
        let mut screening = self.include.clone().map(include::Screening::new);
        for rec in records {
            let frn = rec.frn;
            let mut ops = ops_for(rec);
            let screen = match &mut screening {
                Some(screening) => self.screen(screening, rec),
                None => Screen::Apply,
            };
            match screen {
                Screen::Apply => {}
                Screen::Drop => continue,
                Screen::Relink => {
                    // The link reconcile judges each name by the stored
                    // attributes, so they must be the record's.
                    self.set_attributes(frn, rec.attributes);
                    outcome.needs_link_reconcile.push(frn);
                    continue;
                }
                Screen::Missed => ops.insert(0, UsnOp::Upsert),
            }
            for op in ops {
                match op {
                    UsnOp::Upsert => {
                        // A rename of an entry the index lacks: the build
                        // missed it, so its size, its date and, for a folder,
                        // its contents are missing too. Reason bits
                        // accumulate while a handle stays open, so the rename
                        // of an entry created before the build can carry
                        // FILE_CREATE too; only the rename bit tells. An
                        // entry the Include rules left out is missed in the
                        // same way when it comes back.
                        let missed = (rec.reason & USN_REASON_RENAME_NEW_NAME != 0
                            || screen == Screen::Missed)
                            && self.lacks(frn);
                        if let UpsertResult::NeedsReconcile =
                            self.upsert_link(frn, rec.parent_frn, &rec.name, rec.is_dir())
                        {
                            outcome.needs_link_reconcile.push(frn);
                        }
                        if missed {
                            outcome.needs_stat.push(frn);
                            if rec.is_dir() {
                                outcome.needs_walk.push(frn);
                            }
                        } else if rec.is_dir()
                            && let Some(screening) = &mut screening
                        {
                            // A folder that moved brings what is below it to
                            // a place the rules may decide differently.
                            self.recheck_below(screening, frn);
                        }
                    }
                    UsnOp::Delete => {
                        if !self.delete_frn(frn) {
                            self.stats.delete_of_unknown += 1;
                            outcome.anomalies.push(Anomaly::DeleteOfUnknown { frn });
                        }
                    }
                    UsnOp::LinkDirty => outcome.needs_link_reconcile.push(frn),
                    UsnOp::StatDirty => outcome.needs_stat.push(frn),
                }
            }
            // Every record carries the file's attributes as of the record, so
            // the last record of a batch leaves the current ones, whatever its
            // reason bits: an attribute change, a compression or encryption
            // change and a reparse point change each write one.
            self.set_attributes(frn, rec.attributes);
        }
        // Reason bits accumulate, so one change reaches a batch in several
        // records, and each FRN reported costs the daemon a link walk.
        outcome.needs_link_reconcile.sort_unstable();
        outcome.needs_link_reconcile.dedup();
        outcome.needs_walk.sort_unstable();
        outcome.needs_walk.dedup();
        self.generation += 1;
        // Compaction is NOT performed here: it is reported so the caller can do
        // the expensive part off the write lock, where it would block queries.
        outcome.wants_compact = self.names.should_compact() || self.names.ranks_stale();
        outcome
    }

    /// Records that `n` hard-link walks could not complete and so were not
    /// applied, so the gap is visible in `Status` rather than silent. The
    /// successful walks are applied by [`Self::reconcile_links`].
    pub fn note_link_reconciles_dropped(&mut self, n: u64) {
        self.stats.link_reconciles_dropped += n;
    }

    /// Reconciles the hard-link set of `frn` to exactly `actual` (the daemon's
    /// `FindFirstFileNameW` result). An empty `actual` deletes the file.
    pub fn reconcile_links(&mut self, frn: Frn, actual: &[LinkTarget]) {
        let kept;
        let actual = match self.include.clone() {
            Some(rules) => {
                kept = self.kept_links(&rules, frn, actual);
                &kept[..]
            }
            None => actual,
        };
        if actual.is_empty() {
            self.delete_frn(frn);
            return;
        }
        let is_dir = self
            .head_of(frn)
            .map(|h| self.entries.flags(h).contains(EntryFlags::DIR))
            .unwrap_or(false);
        self.set_chain_links(frn, is_dir, actual);
    }

    /// Live enricher: sets a file's fresh `size`/`mtime` on every entry in its
    /// hard-link chain (a query may return any of its names). USN records carry
    /// neither, so the daemon stats the FRN after a create/data/basic-info
    /// change and applies it here. A folder takes only the date, because its
    /// size is the sum of its files. A no-op if the FRN is unknown (e.g.
    /// deleted between apply and stat).
    pub fn set_stat(&mut self, frn: Frn, size: u64, mtime: i64) {
        let Some(head) = self.head_of(frn) else {
            return;
        };
        self.set_chain_stat(head, Some(size), Some(mtime), None);
    }

    /// Sets a file's date created on every entry in its hard-link chain,
    /// when the index holds dates created ([`Self::keeps_created`]), as
    /// [`Self::set_stat`] sets its date modified.
    pub fn set_created(&mut self, frn: Frn, created: i64) {
        let Some(head) = self.head_of(frn) else {
            return;
        };
        self.set_chain_stat(head, None, None, Some(created));
    }

    /// Whether the index holds each entry's date created, which Dates
    /// created turns on ([`VolumeRules::dates_created`]).
    pub fn keeps_created(&self) -> bool {
        self.entries.keeps_created()
    }

    /// Starts or stops holding each entry's date created. A build starts it
    /// before it reads the volume, because only that read brings the dates.
    pub fn keep_created(&mut self, keep: bool) {
        self.entries.keep_created(keep);
    }

    /// Ends the size and date pass: gives every folder the sum of the known
    /// sizes of the files below it, each name of a hard-linked file counted,
    /// and from then on every change keeps each sum exact. Until it runs,
    /// every folder's size is unknown. The daemon calls it when a build's
    /// pass ends, also after a pass that stopped, whose sums then leave out
    /// the files it did not read ([`Self::record_size_pass_stopped`]). The
    /// index calls it again for the two changes that move more than one
    /// entry's size: a move that opens or closes a loop in the parent links,
    /// and a file that becomes a folder.
    ///
    /// A folder counts each file whose parent links pass through it once, so
    /// inside a loop every folder of the loop holds the whole loop's files.
    pub fn compute_folder_sizes(&mut self) {
        if self.folder_sizes == FolderSizes::Unknown {
            self.folder_sizes = FolderSizes::Kept;
        }
        let slots = 0..self.entries.capacity() as EntryIdx;
        for idx in slots.clone() {
            if !self.entries.is_tombstone(idx) && self.entries.flags(idx).contains(EntryFlags::DIR)
            {
                self.entries.set_size(idx, 0);
            }
        }
        for idx in slots {
            if !self.entries.is_tombstone(idx) {
                self.add_to_folders_from(self.entries.parent(idx), self.counted_size(idx));
            }
        }
    }

    /// Records that the size and date pass stopped before it read every
    /// file's size, after [`Self::compute_folder_sizes`] summed what it read:
    /// each folder's size is then at least the sum it holds, which
    /// [`Self::folder_sizes_at_least`] tells the search.
    pub fn record_size_pass_stopped(&mut self) {
        self.folder_sizes = FolderSizes::AtLeast;
    }

    /// Whether a folder's size is only a lower bound, because the size and
    /// date pass stopped before it read every file's size.
    pub fn folder_sizes_at_least(&self) -> bool {
        self.folder_sizes == FolderSizes::AtLeast
    }

    /// Sets the attributes of a record's `FILE_ATTRIBUTE_*` bits on every link
    /// of `frn`, which all name one file. A no-op if the FRN is unknown.
    fn set_attributes(&mut self, frn: Frn, bits: u32) {
        let attributes = PackedAttributes::from_record(bits);
        let mut cur = self.head_of(frn).unwrap_or(NIL);
        while cur != NIL {
            self.entries.set_attributes(cur, attributes);
            cur = self.entries.next_link(cur);
        }
    }

    /// Sets `size`, `mtime` and `created` on EVERY link of the file whose
    /// chain starts at `head`, not just the head: a query may return any of
    /// a hard-linked file's names, and they all describe the same bytes.
    /// `size` reaches files only, because a folder's size slot holds its
    /// sum; `None` leaves that value as it was.
    fn set_chain_stat(
        &mut self,
        head: EntryIdx,
        size: Option<u64>,
        mtime: Option<i64>,
        created: Option<i64>,
    ) {
        let mut cur = head;
        while cur != NIL {
            if let Some(size) = size
                && !self.entries.flags(cur).contains(EntryFlags::DIR)
            {
                let before = self.counted_size(cur);
                self.entries.set_size(cur, size);
                let change = self.counted_size(cur).wrapping_sub(before);
                self.add_to_folders_from(self.entries.parent(cur), change);
            }
            if let Some(mtime) = mtime {
                self.entries.set_mtime(cur, mtime);
            }
            if let Some(created) = created {
                self.entries.set_created(cur, created);
            }
            cur = self.entries.next_link(cur);
        }
    }

    // -- path reconstruction ---------------------------------------------

    /// Appends `dir\dir\name` (no volume/mount prefix) for `idx` into `out` as
    /// WTF-8, returning how the walk terminated.
    pub fn path_of(&self, idx: EntryIdx, out: &mut Vec<u8>) -> PathStatus {
        // A live entry's parent is always live, so only the start can be dead.
        if self.entries.is_tombstone(idx) {
            return PathStatus::Removed;
        }
        let mut chain: SmallVec<[EntryIdx; 32]> = SmallVec::new();
        let mut cur = idx;
        for _ in 0..MAX_PATH_HOPS {
            chain.push(cur);
            let p = self.entries.parent(cur);
            if p == NIL {
                break;
            }
            cur = p;
        }
        // The loop above pushes `cur` on its first iteration (MAX_PATH_HOPS > 0),
        // so `chain` always holds at least the starting entry.
        let top = *chain
            .last()
            .expect("path_of chain always contains the starting entry");
        if self.entries.parent(top) != NIL {
            return PathStatus::CycleDetected;
        }
        let in_lost_found = top == self.lost_found;

        // Emit from the top down, skipping the volume root (its name is empty
        // and the mount prefix is added by the daemon).
        for &e in chain.iter().rev() {
            if e == self.root {
                continue;
            }
            if !out.is_empty() {
                out.push(b'\\');
            }
            out.extend_from_slice(self.names.raw_bytes(self.entries.name_id(e)));
        }

        if in_lost_found {
            PathStatus::InLostFound
        } else {
            PathStatus::Ok
        }
    }

    /// The chain-head entry for `frn`, if present and not stale.
    pub fn head_of(&self, frn: Frn) -> Option<EntryIdx> {
        let head = self.by_frn.get(frn)?;
        (self.entries.frn(head) == frn).then_some(head)
    }

    /// Looks up a child directory by its folded name via the persistent scope
    /// index: O(1), no scan. Case-insensitive by construction (the key hashes
    /// the folded name); the query engine's `resolve_scope` walks it component
    /// by component. Every candidate under the hash is verified against the
    /// real folded name, so a collision costs one extra compare, never a
    /// wrong directory.
    pub fn child_dir(&self, parent: EntryIdx, folded_name: Vec<u8>) -> Option<EntryIdx> {
        let key = (parent, hash_bytes(&folded_name));
        let cands = self.dir_children.get(&key)?;
        cands
            .iter()
            .copied()
            .find(|&d| self.folded_name(d) == folded_name.as_slice())
    }

    /// Whether slot `idx` holds an entry.
    pub(crate) fn is_live(&self, idx: EntryIdx) -> bool {
        !self.entries.is_tombstone(idx)
    }

    /// Whether slot `idx` holds the entry of file `frn`.
    pub(crate) fn holds(&self, idx: EntryIdx, frn: Frn) -> bool {
        self.is_live(idx) && self.entries.frn(idx) == frn
    }

    /// Whether some file ID is wider than 64 bits, as every ReFS ID is.
    pub(crate) fn has_wide_ids(&self) -> bool {
        self.entries.has_wide_ids()
    }

    /// Marks the slots that new entries took over so far, for
    /// [`Self::reused_since`].
    pub(crate) fn reuse_mark(&self) -> u64 {
        self.entries.reuse_mark()
    }

    /// The slots that new entries took over since `mark`, from
    /// [`Self::reuse_mark`] on this instance, or `None` when they are too
    /// many to have been kept. An entry that was live at the mark and whose
    /// slot is listed has died since, whatever the slot holds now.
    pub(crate) fn reused_since(&self, mark: u64) -> Option<impl Iterator<Item = EntryIdx> + '_> {
        self.entries.reused_since(mark)
    }

    /// Read access to a live entry's fields (for the query engine / tests).
    pub fn entry(&self, idx: EntryIdx) -> EntryView<'_> {
        EntryView { index: self, idx }
    }

    /// Every live entry index (includes root, lost+found, placeholders).
    pub fn live_entries(&self) -> impl Iterator<Item = EntryIdx> + '_ {
        self.entries.iter_live()
    }

    /// `true` for the root, lost+found, and any placeholder/orphan entry: the
    /// entries that do not correspond to a real, fully-resolved filesystem
    /// object.
    pub fn is_synthetic(&self, idx: EntryIdx) -> bool {
        idx == self.root
            || idx == self.lost_found
            || self.entries.flags(idx).contains(EntryFlags::PLACEHOLDER)
    }

    /// Whether the index holds no real entry of `frn`: none at all, or only a
    /// placeholder that a child's record left.
    fn lacks(&self, frn: Frn) -> bool {
        self.head_of(frn).is_none_or(|h| self.is_synthetic(h))
    }

    /// Forces an entry's parent link. Test-only: the public API never
    /// produces a cycle, so this exists solely to exercise the defensive
    /// cycle guard in [`Self::path_of`].
    #[cfg(test)]
    pub fn force_parent(&mut self, idx: EntryIdx, parent: EntryIdx) {
        self.entries.set_parent(idx, parent);
    }

    #[cfg(test)]
    pub(crate) fn name_id(&self, idx: EntryIdx) -> NameId {
        self.entries.name_id(idx)
    }

    /// The invariant every mutation must preserve: the incrementally-maintained
    /// directory index equals one rebuilt from a full scan of the live entries.
    /// The model-op-tape proptest asserts this after every applied op.
    #[cfg(test)]
    pub fn dir_index_matches_rebuild(&self) -> bool {
        let mut fresh: FxHashMap<(EntryIdx, u64), SmallVec<[EntryIdx; 1]>> = FxHashMap::default();
        for idx in self.entries.iter_live() {
            let flags = self.entries.flags(idx);
            if !flags.contains(EntryFlags::DIR)
                || flags.contains(EntryFlags::PLACEHOLDER)
                || idx == self.root
                || idx == self.lost_found
            {
                continue;
            }
            let key = (self.entries.parent(idx), hash_bytes(self.folded_name(idx)));
            fresh.entry(key).or_default().push(idx);
        }
        if fresh.len() != self.dir_children.len() {
            return false;
        }
        fresh.iter().all(|(k, want)| {
            self.dir_children.get(k).is_some_and(|got| {
                let mut a: Vec<_> = want.to_vec();
                let mut b: Vec<_> = got.to_vec();
                a.sort_unstable();
                b.sort_unstable();
                a == b
            })
        })
    }

    /// Collects the reconstructed path of every real entry as a lossy `String`
    /// (a test/diagnostic convenience; orphans in lost+found are excluded).
    pub fn collect_real_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        let mut buf = Vec::new();
        for idx in self.live_entries() {
            if self.is_synthetic(idx) {
                continue;
            }
            buf.clear();
            if let PathStatus::Ok = self.path_of(idx, &mut buf) {
                paths.push(crate::wtf8::to_string_lossy(&buf));
            }
        }
        paths
    }

    // -- internals --------------------------------------------------------

    /// Pushes `idx` onto the front of `id`'s same-name chain. Every entry
    /// allocation must pair with exactly one of these, and every tombstone
    /// with one [`Self::unlink_same_name`]: the chains are EXACT (a scan hit
    /// expands to precisely the live entries bearing the name), which is what
    /// spares the query path any staleness guard.
    fn link_same_name(&mut self, idx: EntryIdx, id: NameId) {
        let old_head = self.names.chain_head(id);
        self.entries.set_next_same(idx, old_head);
        self.entries.set_prev_same(idx, NIL);
        if old_head != NIL {
            self.entries.set_prev_same(old_head, idx);
        }
        self.names.set_chain_head(id, idx);
    }

    /// Pushes `idx` onto the front of its parent's child chain. Every entry
    /// allocation (and every parent change) must pair with exactly one of
    /// these.
    fn link_child(&mut self, idx: EntryIdx) {
        let parent = self.entries.parent(idx);
        if parent == NIL {
            return; // the root and lost+found have no parent chain
        }
        let old_head = self.entries.first_child(parent);
        self.entries.set_next_child(idx, old_head);
        self.entries.set_prev_child(idx, NIL);
        if old_head != NIL {
            self.entries.set_prev_child(old_head, idx);
        }
        self.entries.set_first_child(parent, idx);
    }

    /// O(1) removal from the parent's child chain. Must run BEFORE the
    /// entry's parent is overwritten or its slot tombstoned, for the same
    /// reason as [`Self::unlink_same_name`].
    fn unlink_child(&mut self, idx: EntryIdx) {
        let parent = self.entries.parent(idx);
        if parent == NIL {
            return;
        }
        let prev = self.entries.prev_child(idx);
        let next = self.entries.next_child(idx);
        if prev == NIL {
            self.entries.set_first_child(parent, next);
        } else {
            self.entries.set_next_child(prev, next);
        }
        if next != NIL {
            self.entries.set_prev_child(next, prev);
        }
        self.entries.set_next_child(idx, NIL);
        self.entries.set_prev_child(idx, NIL);
    }

    /// Moves every child of a directory that is being removed to lost+found.
    /// A removed directory still has children only after a missed delete: of
    /// the directory itself, whose record NTFS then gives to a new file, or of
    /// a child. Moving them keeps every live entry's parent live, so no path
    /// runs through a freed slot or the unrelated entry that reuses it. The
    /// children have no full path until their own records re-parent them.
    fn orphan_children(&mut self, dir: EntryIdx) {
        let mut cur = self.entries.first_child(dir);
        while cur != NIL {
            let next = self.entries.next_child(cur);
            self.dir_index_remove(cur);
            self.move_entry(cur, self.lost_found);
            self.dir_index_insert(cur);
            cur = next;
        }
    }

    /// Makes `new_parent` the parent of `idx`, and moves the size of `idx`
    /// and of everything below it from the folders above the old parent to
    /// the folders above `new_parent`.
    fn move_entry(&mut self, idx: EntryIdx, new_parent: EntryIdx) {
        let old_parent = self.entries.parent(idx);
        let folders = (self.folder_sizes != FolderSizes::Unknown && old_parent != new_parent)
            .then(|| (self.chain_up(old_parent), self.chain_up(new_parent)));
        self.unlink_child(idx);
        self.entries.set_parent(idx, new_parent);
        self.link_child(idx);
        let Some((old_folders, new_folders)) = folders else {
            return;
        };
        if old_folders.contains(&idx) || new_folders.contains(&idx) {
            // The move opens or closes a loop in the parent links, which
            // changes each folder of the loop by more than what moved.
            self.compute_folder_sizes();
            return;
        }
        let moved = self.size_with_contents(idx);
        self.add_to_folders(&old_folders, moved.wrapping_neg());
        self.add_to_folders(&new_folders, moved);
    }

    /// What `idx` adds by itself to the size of each folder above it: its
    /// size when it is a file whose size is known, and otherwise nothing.
    fn counted_size(&self, idx: EntryIdx) -> u64 {
        let size = self.entries.size(idx);
        if self.entries.flags(idx).contains(EntryFlags::DIR) || size == SIZE_UNKNOWN {
            0
        } else {
            size
        }
    }

    /// What `idx` and everything below it add to the size of each folder
    /// above it, while folder sizes are kept. No loop in the parent links may
    /// pass through `idx`.
    fn size_with_contents(&self, idx: EntryIdx) -> u64 {
        if self.entries.flags(idx).contains(EntryFlags::DIR) {
            return self.entries.size(idx);
        }
        // A file holds entries only after a journal record named it as a
        // folder, which only corrupt data does.
        self.subtree_entries(idx)
            .fold(self.counted_size(idx), |sum, e| {
                sum.wrapping_add(self.counted_size(e))
            })
    }

    /// `start` and every entry above it, each once: up to the top, or once
    /// around a loop in the parent links, which a journal replay can close
    /// for a while ([`PathStatus::CycleDetected`]).
    fn chain_up(&self, start: EntryIdx) -> SmallVec<[EntryIdx; 32]> {
        // Deeper than almost every real folder tree, and few enough that an
        // entry below a loop costs little more than its own depth, which
        // matters when a recompute walks up from every entry under the lock.
        const HOPS_BEFORE_LOOP_CHECK: usize = 64;
        let mut chain = SmallVec::new();
        let mut cur = start;
        while cur != NIL && chain.len() < HOPS_BEFORE_LOOP_CHECK {
            chain.push(cur);
            cur = self.entries.parent(cur);
        }
        if cur != NIL {
            // A loop, or a deep tree: walk again, and stop at the first entry
            // met twice.
            chain.clear();
            let mut seen = FxHashSet::default();
            cur = start;
            while cur != NIL && seen.insert(cur) {
                chain.push(cur);
                cur = self.entries.parent(cur);
            }
        }
        chain
    }

    /// Adds `delta` to the size of `start` and of each folder above it, while
    /// folder sizes are kept.
    fn add_to_folders_from(&mut self, start: EntryIdx, delta: u64) {
        if self.folder_sizes != FolderSizes::Unknown && delta != 0 {
            let chain = self.chain_up(start);
            self.add_to_folders(&chain, delta);
        }
    }

    /// Adds `delta` to the size of each folder of `chain`. A size is taken
    /// away as its `wrapping_neg`: wrapping sums reverse exactly and never
    /// panic, and only a corrupt size can bring one near 2^64.
    fn add_to_folders(&mut self, chain: &[EntryIdx], delta: u64) {
        for &e in chain {
            if self.entries.flags(e).contains(EntryFlags::DIR) {
                let sum = self.entries.size(e).wrapping_add(delta);
                self.entries.set_size(e, sum);
            }
        }
    }

    /// Adds one entry: interns its name and links it into its same-name chain,
    /// its parent's child chain and the scope index, and adds a file's known
    /// `size` to the folders above it. A folder ignores `size`: it holds
    /// nothing yet. The caller links the entry into its file's hard-link
    /// chain and the FRN map.
    #[allow(clippy::too_many_arguments)]
    fn alloc_entry(
        &mut self,
        frn: Frn,
        parent: EntryIdx,
        name: &[u8],
        flags: EntryFlags,
        size: u64,
        mtime: i64,
        attributes: PackedAttributes,
    ) -> EntryIdx {
        let size = match (flags.contains(EntryFlags::DIR), self.folder_sizes) {
            (false, _) => size,
            (true, FolderSizes::Unknown) => SIZE_UNKNOWN,
            (true, FolderSizes::Kept | FolderSizes::AtLeast) => 0,
        };
        let id = self.names.intern(name);
        let idx = self
            .entries
            .alloc(frn, parent, id, flags, size, mtime, attributes, NIL);
        self.link_same_name(idx, id);
        self.link_child(idx);
        self.dir_index_insert(idx);
        self.add_to_folders_from(parent, self.counted_size(idx));
        #[cfg(test)]
        {
            self.dirty += 1;
        }
        idx
    }

    /// Removes one entry: takes it out of every chain and the scope index,
    /// moves its children to lost+found, takes its size from the folders above
    /// it, releases its name and frees its slot.
    fn remove_entry(&mut self, idx: EntryIdx) {
        self.dir_index_remove(idx); // while the name/parent are still readable
        self.unlink_same_name(idx);
        self.unlink_child(idx);
        self.orphan_children(idx);
        // It holds nothing now, so only its own size leaves those folders.
        let parent = self.entries.parent(idx);
        self.add_to_folders_from(parent, self.counted_size(idx).wrapping_neg());
        self.names.release(self.entries.name_id(idx));
        self.entries.tombstone(idx);
        #[cfg(test)]
        {
            self.dirty += 1;
        }
    }

    /// O(1) removal from the doubly-linked same-name chain. Must run BEFORE
    /// the entry's name id is overwritten or its slot tombstoned: a stale
    /// chain pointer surviving into a reused slot can cross-link chains.
    fn unlink_same_name(&mut self, idx: EntryIdx) {
        let id = self.entries.name_id(idx);
        let prev = self.entries.prev_same(idx);
        let next = self.entries.next_same(idx);
        if prev == NIL {
            self.names.set_chain_head(id, next);
        } else {
            self.entries.set_next_same(prev, next);
        }
        if next != NIL {
            self.entries.set_prev_same(next, prev);
        }
        self.entries.set_next_same(idx, NIL);
        self.entries.set_prev_same(idx, NIL);
    }

    /// Records a real directory's current `(parent, folded name) → idx` in the
    /// persistent scope index. No-op for files, placeholders, and
    /// root/lost+found (none are ever a `-path` component). Reads the entry's
    /// CURRENT fields, so call it AFTER the entry's parent/name/flags are set.
    fn dir_index_insert(&mut self, idx: EntryIdx) {
        let flags = self.entries.flags(idx);
        if !flags.contains(EntryFlags::DIR)
            || flags.contains(EntryFlags::PLACEHOLDER)
            || idx == self.root
            || idx == self.lost_found
        {
            return;
        }
        let key = (self.entries.parent(idx), hash_bytes(self.folded_name(idx)));
        let slot = self.dir_children.entry(key).or_default();
        if !slot.contains(&idx) {
            slot.push(idx);
        }
    }

    /// Removes a directory's current `(parent, folded name)` key. Call BEFORE a
    /// rename/tombstone overwrites its parent/name (it reads them). Guarded to
    /// only remove a key that still points at THIS entry, so it never disturbs a
    /// sibling that legitimately owns the same key.
    fn dir_index_remove(&mut self, idx: EntryIdx) {
        if !self.entries.flags(idx).contains(EntryFlags::DIR) {
            return;
        }
        let key = (self.entries.parent(idx), hash_bytes(self.folded_name(idx)));
        if let Some(slot) = self.dir_children.get_mut(&key) {
            slot.retain(|&mut d| d != idx);
            if slot.is_empty() {
                self.dir_children.remove(&key);
            }
        }
    }

    fn upsert_link(
        &mut self,
        frn: Frn,
        parent_frn: Frn,
        name: &[u8],
        is_dir: bool,
    ) -> UpsertResult {
        // Record number reused by a different FRN: reclaim the stale chain
        // first, so the parent resolved below is never a slot it frees.
        if let Some(head) = self.by_frn.get(frn)
            && self.entries.frn(head) != frn
        {
            self.reclaim_stale_slot(head);
        }
        // A parent with the entry's own record number is that record's
        // previous tenant, which only corrupt data names: its placeholder
        // would take the entry's own place in the map, so the entry goes
        // under lost+found, with no full path.
        let parent_idx = if parent_frn != frn && self.by_frn.same_key(parent_frn, frn) {
            self.lost_found
        } else {
            self.resolve_or_placeholder_parent(parent_frn)
        };

        if let Some(head) = self.by_frn.get(frn) {
            if self.entries.next_link(head) == NIL {
                self.rewrite_entry(head, parent_idx, name, is_dir);
                return UpsertResult::Ok;
            }
            return UpsertResult::NeedsReconcile;
        }

        let idx = self.alloc_entry(
            frn,
            parent_idx,
            name,
            EntryFlags::EMPTY.with(EntryFlags::DIR, is_dir),
            SIZE_UNKNOWN,
            MTIME_UNKNOWN,
            PackedAttributes::default(),
        );
        self.by_frn.set(frn, idx);
        UpsertResult::Ok
    }

    fn rewrite_entry(&mut self, idx: EntryIdx, parent_idx: EntryIdx, name: &[u8], is_dir: bool) {
        // Drop the OLD (parent, name) dir key before the fields are overwritten;
        // re-index under the NEW ones afterward. This covers rename, a
        // placeholder being filled in, and dir↔file transitions uniformly.
        self.dir_index_remove(idx);
        self.unlink_same_name(idx);
        // Interned before the old name is released, so an entry that keeps
        // its name keeps its name id, and with it its rank.
        let old = self.entries.name_id(idx);
        let id = self.names.intern(name);
        self.names.release(old);
        self.entries.set_name_id(idx, id);
        self.link_same_name(idx, id);
        self.move_entry(idx, parent_idx);
        let was_dir = self.entries.flags(idx).contains(EntryFlags::DIR);
        let flags = self
            .entries
            .flags(idx)
            .with(EntryFlags::DIR, is_dir)
            .with(EntryFlags::PLACEHOLDER, false);
        self.entries.set_flags(idx, flags);
        if was_dir != is_dir {
            // A folder's size slot holds its sum and a file's its own size,
            // so neither carries over. A folder that becomes a file of
            // unknown size leaves the sums above it as they are; a file that
            // becomes a folder takes its size from them and needs its own sum.
            self.entries.set_size(idx, SIZE_UNKNOWN);
            if is_dir && self.folder_sizes != FolderSizes::Unknown {
                self.compute_folder_sizes();
            }
        }
        self.dir_index_insert(idx);
        #[cfg(test)]
        {
            self.dirty += 1;
        }
    }

    /// Reclaims a record number NTFS has re-tenanted: tombstones the stale chain
    /// and drops the map entry, so the dead names stop matching and the slots
    /// return to the free list.
    ///
    /// Every site that finds a live `by_frn` entry whose FRN does not match must
    /// call this before overwriting the map. Skipping it leaves the stale chain
    /// live in the entry table but unreachable from `by_frn`, so its names keep
    /// matching queries and its slots never return to the free list.
    ///
    /// Reported through `IndexStats::stale_slots` rather than an `Anomaly`: the
    /// callers are deep helpers with no access to the batch's
    /// `ApplyOutcome`, and buffering per-event anomalies on the index would grow
    /// unbounded across a multi-million-entry bootstrap. The counter reaches
    /// `Status` either way.
    fn reclaim_stale_slot(&mut self, head: EntryIdx) {
        let stale = self.entries.frn(head);
        self.tombstone_chain(head);
        self.by_frn.remove(stale);
        self.stats.stale_slots += 1;
    }

    /// Resolves a parent FRN to its entry, synthesizing a placeholder under
    /// lost+found if the parent has not been seen yet.
    fn resolve_or_placeholder_parent(&mut self, parent_frn: Frn) -> EntryIdx {
        if let Some(idx) = self.by_frn.get(parent_frn) {
            if self.entries.frn(idx) == parent_frn {
                return idx;
            }
            // NTFS handed this record number to a different file.
            self.reclaim_stale_slot(idx);
        }
        let idx = self.alloc_entry(
            parent_frn,
            self.lost_found,
            PLACEHOLDER_NAME,
            EntryFlags::DIR.with(EntryFlags::PLACEHOLDER, true),
            SIZE_UNKNOWN,
            MTIME_UNKNOWN,
            PackedAttributes::default(),
        );
        self.by_frn.set(parent_frn, idx);
        self.stats.placeholders_created += 1;
        idx
    }

    fn delete_frn(&mut self, frn: Frn) -> bool {
        let Some(head) = self.by_frn.get(frn) else {
            return false;
        };
        if self.entries.frn(head) != frn {
            return false;
        }
        self.tombstone_chain(head);
        self.by_frn.remove(frn);
        true
    }

    fn tombstone_chain(&mut self, head: EntryIdx) {
        let mut cur = head;
        while cur != NIL {
            let next = self.entries.next_link(cur);
            self.remove_entry(cur);
            cur = next;
        }
    }

    /// Is a link with this `(parent, name)` already in `frn`'s chain?
    fn chain_has_link(&self, frn: Frn, parent_idx: EntryIdx, name: &[u8]) -> bool {
        let Some(head) = self.head_of(frn) else {
            return false;
        };
        let mut cur = head;
        while cur != NIL {
            if self.entries.parent(cur) == parent_idx
                && self.names.raw_bytes(self.entries.name_id(cur)) == name
            {
                return true;
            }
            cur = self.entries.next_link(cur);
        }
        false
    }

    /// Appends a new link (a file's additional hard-link name) to the tail of
    /// `frn`'s chain, leaving the existing head, and its identity, untouched.
    fn add_link(&mut self, frn: Frn, parent_idx: EntryIdx, name: &[u8]) {
        let idx = self.alloc_entry(
            frn,
            parent_idx,
            name,
            EntryFlags::EMPTY,
            SIZE_UNKNOWN,
            MTIME_UNKNOWN,
            PackedAttributes::default(),
        );
        if let Some(head) = self.by_frn.get(frn) {
            let mut tail = head;
            while self.entries.next_link(tail) != NIL {
                tail = self.entries.next_link(tail);
            }
            self.entries.set_next_link(tail, idx);
        } else {
            self.by_frn.set(frn, idx);
        }
    }

    /// Makes the hard-link chain for `frn` exactly `desired`, preserving
    /// entries that already match by (parent, name) so their EntryIdx, and
    /// thus any children pointing at them, survive. Only used by
    /// [`Self::reconcile_links`] (live `HARD_LINK_CHANGE`, always a file, which
    /// has no children) and by the ENUM-missed branch of [`Self::enrich`]
    /// (nothing exists yet to orphan).
    fn set_chain_links(&mut self, frn: Frn, is_dir: bool, desired: &[LinkTarget]) {
        // Collect the current chain if it is really this FRN's; a chain of the
        // record's previous tenant is reclaimed before the map is overwritten.
        let mut current: SmallVec<[EntryIdx; 4]> = SmallVec::new();
        if let Some(head) = self.by_frn.get(frn) {
            if self.entries.frn(head) == frn {
                let mut cur = head;
                while cur != NIL {
                    current.push(cur);
                    cur = self.entries.next_link(cur);
                }
            } else {
                self.reclaim_stale_slot(head);
            }
        }

        // Every link names the same file record, so a new link starts with the
        // size, date and attributes the chain already knows. Nothing stats a
        // link that reconciliation adds, so without this a new hard link, or
        // the new name of a renamed link, has no size, date or attributes.
        let (size, mtime, created, attributes) = current.first().map_or(
            (
                SIZE_UNKNOWN,
                MTIME_UNKNOWN,
                MTIME_UNKNOWN,
                PackedAttributes::default(),
            ),
            |&e| {
                (
                    self.entries.size(e),
                    self.entries.mtime(e),
                    self.entries.created(e).unwrap_or(MTIME_UNKNOWN),
                    self.entries.attributes(e),
                )
            },
        );

        let mut kept: SmallVec<[EntryIdx; 4]> = SmallVec::new();
        let mut used: SmallVec<[bool; 4]> = SmallVec::from_elem(false, current.len());
        for target in desired {
            let existing = current.iter().enumerate().position(|(i, &e)| {
                !used[i]
                    && self.entries.parent(e) == target.parent
                    && self.names.raw_bytes(self.entries.name_id(e)) == target.name.as_slice()
            });
            if let Some(pos) = existing {
                used[pos] = true;
                kept.push(current[pos]);
            } else {
                let idx = self.alloc_entry(
                    frn,
                    target.parent,
                    &target.name,
                    EntryFlags::EMPTY.with(EntryFlags::DIR, is_dir),
                    size,
                    mtime,
                    attributes,
                );
                self.entries.set_created(idx, created);
                kept.push(idx);
            }
        }

        for (i, &e) in current.iter().enumerate() {
            if !used[i] {
                self.remove_entry(e);
            }
        }

        for w in 0..kept.len() {
            let next = if w + 1 < kept.len() { kept[w + 1] } else { NIL };
            self.entries.set_next_link(kept[w], next);
        }
        self.by_frn.set(frn, kept[0]);
    }

    /// Builds a compaction WITHOUT mutating the index, or `None` if neither
    /// the arenas are dirty enough nor the name ranks stale enough to be
    /// worth it.
    ///
    /// Takes `&self` on purpose. This is the expensive half (it copies every
    /// live name into fresh arenas), so callers plan here, off the exclusive
    /// lock, where queries keep reading, then [`Self::apply_compaction`] the
    /// result.
    ///
    /// The plan is only valid while the index has not changed, so hold an
    /// upgradable read across both calls: it keeps every writer out until it
    /// upgrades, so the plan cannot go stale whoever else writes.
    ///
    /// The plan also merges the name ranks when they are stale, sorting only
    /// the names added since the ranks were made ([`Self::rank_names`]).
    pub fn plan_compaction(&self) -> Option<CompactPlan> {
        let ranks = self.names.ranks_stale().then(|| self.names.plan_ranks());
        if self.names.should_compact() {
            let ranks = ranks.as_ref().unwrap_or(self.names.ranks());
            return Some(CompactPlan(Compaction::Arenas(
                self.names.plan_compact(ranks),
            )));
        }
        ranks.map(|ranks| CompactPlan(Compaction::Ranks(ranks)))
    }

    /// Installs a [`CompactPlan`]: swaps in the compacted name storage and
    /// rewrites every live entry's name id through the remap, or swaps in the
    /// merged ranks. Dead ids cannot be referenced by construction (an id only
    /// dies when its refcount hits zero, and each live entry holds a
    /// reference), so the remap lookup always yields a live new id.
    ///
    /// This is the half that must hold the exclusive lock: buffer swaps and
    /// column writes, no copying or hashing of names.
    pub fn apply_compaction(&mut self, plan: CompactPlan) {
        let compacted = match plan.0 {
            Compaction::Arenas(compacted) => compacted,
            Compaction::Ranks(ranks) => return self.names.install_ranks(ranks),
        };
        let remap = self.names.install_compacted(compacted);
        for idx in 0..self.entries.capacity() as EntryIdx {
            if self.entries.is_tombstone(idx) {
                continue;
            }
            let new_id = remap[self.entries.name_id(idx) as usize];
            debug_assert_ne!(new_id, NIL, "live entry referenced a dead name id");
            self.entries.set_name_id(idx, new_id);
        }
    }

    /// Per-component heap accounting for this index. O(directories) for the
    /// exact `dir_children` key bytes; everything else is O(1) reads of
    /// len/capacity. Called on `Status`, not on any hot path.
    pub fn memory(&self) -> IndexMemory {
        let dir_children = {
            // Spilled SmallVecs (hash collisions) are so rare their heap is
            // ignored; the bucket estimate mirrors `FrnMap::memory`.
            let buckets = ((self.dir_children.capacity() as u64 * 8) / 7).next_power_of_two();
            let per_bucket = (size_of::<((EntryIdx, u64), SmallVec<[EntryIdx; 1]>)>() + 1) as u64;
            ComponentBytes {
                used: self.dir_children.len() as u64 * per_bucket,
                allocated: buckets * per_bucket,
            }
        };
        IndexMemory {
            entries: self.entries.memory(),
            arena_raw: self.names.raw_mem(),
            arena_folded: self.names.folded_mem(),
            frn_map: self.by_frn.memory(),
            name_tables: self.names.tables_mem(),
            dir_children,
            frn_map_kind: self.by_frn.kind(),
        }
    }

    /// Returns growth slack (column growth, hash-map overshoot) to the
    /// allocator, keeping headroom so post-publish journal churn does not
    /// immediately reallocate every column. Called once when a volume
    /// goes live: the hash maps grow by doubling, so up to half their
    /// allocation at that moment is slack that would otherwise be pinned for
    /// the daemon's lifetime.
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_with_headroom();
        self.names.shrink_with_headroom();
        self.dir_children.shrink_to_fit();
        if let FrnMap::Dense(v) = &mut self.by_frn {
            // The dense map grows only when the journal brings a record number
            // past its end, which the MFT does rarely, and then by an eighth,
            // so an exact fit costs one reallocation per eighth of growth.
            v.shrink_to_fit();
        }
    }

    /// One-shot storage optimization for a freshly bootstrapped volume:
    /// swap the FRN map to its cheaper dense backing when the record space
    /// allows it, then return all growth slack. Runs under the write lock; call
    /// it once per volume, not per batch. It can give the entries new indexes,
    /// and then the index takes a new instance (`compact_entries`).
    pub fn optimize_storage(&mut self) {
        self.compact_entries();
        self.by_frn.densify_if_smaller();
        self.shrink_to_fit();
    }

    /// Ranks every name by its folded bytes, so a search sorts its matches by
    /// numbers. A build calls it once, before it shares the index, because it
    /// sorts every distinct name. A name added later stays unranked, which a
    /// search places by a binary search, until [`Self::plan_compaction`]
    /// merges it in.
    pub fn rank_names(&mut self) {
        let ranks = self.names.plan_ranks();
        self.names.install_ranks(ranks);
    }

    /// Moves the live entries over the slots that removed entries left, so
    /// their memory returns: after the Include rules removed many entries,
    /// the free slots would otherwise hold it until new entries reuse them.
    /// The entries get new indexes, so the index takes a new instance, which
    /// tells a search that its matches no longer name these entries.
    fn compact_entries(&mut self) {
        if !self.entries.has_free_slots() {
            return;
        }
        let remap = self.entries.compact();
        self.instance = next_instance();
        self.root = moved(&remap, self.root);
        self.lost_found = moved(&remap, self.lost_found);
        self.names.remap_heads(&remap);
        self.by_frn.remap(&remap);
        self.dir_children = std::mem::take(&mut self.dir_children)
            .into_iter()
            .map(|((parent, hash), dirs)| {
                let dirs = dirs.into_iter().map(|d| moved(&remap, d)).collect();
                ((moved(&remap, parent), hash), dirs)
            })
            .collect();
    }

    // -- folded-arena query support --------------------------------------

    /// The contiguous folded UNIQUE-name buffer to `memmem`-scan. Each
    /// distinct name appears once; a hit expands to entries through the
    /// same-name chain (`name_chain`, crate-internal).
    #[inline]
    pub fn folded_bytes(&self) -> &[u8] {
        self.names.folded_haystack()
    }

    /// This entry's folded name (precomputed, shared with every entry bearing
    /// the same name).
    #[inline]
    pub fn folded_name(&self, idx: EntryIdx) -> &[u8] {
        self.names.folded_bytes_of(self.entries.name_id(idx))
    }

    /// Where this entry's folded name sorts among every name of the index
    /// ([`NameStore::name_key`]).
    #[inline]
    pub(crate) fn name_key(&self, idx: EntryIdx) -> u32 {
        self.names.name_key(self.entries.name_id(idx))
    }

    /// The interned pairs, indexed by [`NameId`], `folded.off` ascending: the
    /// scan's merge-walk maps ascending hit offsets to ids with a forward
    /// cursor over this.
    #[inline]
    pub(crate) fn name_pairs(&self) -> &[NamePair] {
        self.names.pairs()
    }

    /// Iterates the live entries bearing name `id` (exact: no staleness
    /// filtering required).
    #[inline]
    pub(crate) fn name_chain(&self, id: NameId) -> NameChain<'_> {
        NameChain {
            entries: &self.entries,
            cur: self.names.chain_head(id),
        }
    }

    /// Depth-first walk of every live entry strictly under `root` (the scope
    /// directory itself is not yielded, matching `in_scope` semantics). Cost
    /// is proportional to the SUBTREE, not the volume: this is what makes a
    /// folder-scoped query independent of disk size.
    pub(crate) fn subtree_entries(&self, root: EntryIdx) -> SubtreeWalk<'_> {
        SubtreeWalk {
            entries: &self.entries,
            root,
            cur: self.entries.first_child(root),
        }
    }

    /// The invariant the child chains must uphold: every live entry with a
    /// parent appears exactly once in that parent's child chain, and chains
    /// contain nothing else. The model-op-tape proptest asserts this after
    /// every applied op.
    #[cfg(test)]
    pub fn child_chains_match_rebuild(&self) -> bool {
        let mut want: FxHashMap<EntryIdx, FxHashSet<EntryIdx>> = FxHashMap::default();
        for idx in self.entries.iter_live() {
            let p = self.entries.parent(idx);
            if p != NIL {
                want.entry(p).or_default().insert(idx);
            }
        }
        for parent in self.entries.iter_live() {
            let mut seen: FxHashSet<EntryIdx> = FxHashSet::default();
            let mut cur = self.entries.first_child(parent);
            while cur != NIL {
                if self.entries.parent(cur) != parent || !seen.insert(cur) {
                    return false; // foreign member or revisit = corrupt chain
                }
                cur = self.entries.next_child(cur);
            }
            if seen != want.remove(&parent).unwrap_or_default() {
                return false;
            }
        }
        want.is_empty()
    }

    /// The invariant [`Self::compute_folder_sizes`] sets up and every later
    /// change keeps: before it runs every folder's size is unknown, and after
    /// it each folder's size is the sum of the known sizes of the files the
    /// child chains reach below it, each once. The folder-size proptest
    /// asserts this after every change.
    #[cfg(test)]
    pub fn folder_sizes_match_rebuild(&self) -> bool {
        let rebuilt = |dir: EntryIdx| {
            let mut seen: FxHashSet<EntryIdx> = FxHashSet::from_iter([dir]);
            let mut heads = vec![self.entries.first_child(dir)];
            let mut sum = 0u64;
            while let Some(mut cur) = heads.pop() {
                while cur != NIL {
                    if seen.insert(cur) {
                        let entry = self.entry(cur);
                        if !entry.is_dir() {
                            sum = sum.wrapping_add(entry.size().unwrap_or(0));
                        }
                        heads.push(self.entries.first_child(cur));
                    }
                    cur = self.entries.next_child(cur);
                }
            }
            sum
        };
        self.entries
            .iter_live()
            .filter(|&e| self.entry(e).is_dir())
            .all(|dir| {
                let want = (self.folder_sizes != FolderSizes::Unknown).then(|| rebuilt(dir));
                self.entry(dir).size() == want
            })
    }

    /// The invariant the same-name chains must uphold: walking every name's
    /// chain visits exactly the live entries bearing that name, each once.
    /// The model-op-tape proptest asserts this after every applied op.
    #[cfg(test)]
    pub fn name_chains_match_rebuild(&self) -> bool {
        let mut want: FxHashMap<NameId, FxHashSet<EntryIdx>> = FxHashMap::default();
        for idx in self.entries.iter_live() {
            want.entry(self.entries.name_id(idx))
                .or_default()
                .insert(idx);
        }
        for id in 0..self.names.ids() as NameId {
            let mut seen: FxHashSet<EntryIdx> = FxHashSet::default();
            for e in self.name_chain(id) {
                if !seen.insert(e) {
                    return false; // revisit = corrupt chain
                }
            }
            if seen != want.remove(&id).unwrap_or_default() {
                return false;
            }
        }
        want.is_empty()
    }
}

/// Depth-first iterator over a directory's live descendants, driven purely
/// by the child chains (no allocation at all: the walk backtracks through
/// parent pointers, so there is no explicit stack).
pub(crate) struct SubtreeWalk<'a> {
    entries: &'a EntryTable,
    root: EntryIdx,
    cur: EntryIdx,
}

impl Iterator for SubtreeWalk<'_> {
    type Item = EntryIdx;
    fn next(&mut self) -> Option<EntryIdx> {
        if self.cur == NIL {
            return None;
        }
        let yielded = self.cur;
        // Advance: descend first, else next sibling, else climb until a
        // sibling exists or the root is reached.
        let down = self.entries.first_child(yielded);
        if down != NIL {
            self.cur = down;
        } else {
            let mut at = yielded;
            loop {
                let sib = self.entries.next_child(at);
                if sib != NIL {
                    self.cur = sib;
                    break;
                }
                at = self.entries.parent(at);
                if at == self.root || at == NIL {
                    self.cur = NIL;
                    break;
                }
            }
        }
        Some(yielded)
    }
}

/// Iterator over one interned name's live entries (see
/// [`VolumeIndex::name_chain`]).
pub(crate) struct NameChain<'a> {
    entries: &'a EntryTable,
    cur: EntryIdx,
}

impl Iterator for NameChain<'_> {
    type Item = EntryIdx;
    fn next(&mut self) -> Option<EntryIdx> {
        if self.cur == NIL {
            return None;
        }
        let e = self.cur;
        self.cur = self.entries.next_same(e);
        Some(e)
    }
}

/// A borrowed view of one entry's fields.
pub struct EntryView<'a> {
    index: &'a VolumeIndex,
    idx: EntryIdx,
}

impl<'a> EntryView<'a> {
    pub fn frn(&self) -> Frn {
        self.index.entries.frn(self.idx)
    }
    pub fn name(&self) -> &'a [u8] {
        self.index
            .names
            .raw_bytes(self.index.entries.name_id(self.idx))
    }
    pub fn folded_name(&self) -> &[u8] {
        self.index.folded_name(self.idx)
    }
    pub fn is_dir(&self) -> bool {
        self.index.entries.flags(self.idx).contains(EntryFlags::DIR)
    }
    pub fn size(&self) -> Option<u64> {
        let s = self.index.entries.size(self.idx);
        (s != SIZE_UNKNOWN).then_some(s)
    }
    pub fn mtime(&self) -> Option<i64> {
        let m = self.index.entries.mtime(self.idx);
        (m != MTIME_UNKNOWN).then_some(m)
    }
    /// The date created, `None` while unknown or while the index holds no
    /// dates created.
    pub fn created(&self) -> Option<i64> {
        let c = self.index.entries.created(self.idx)?;
        (c != MTIME_UNKNOWN).then_some(c)
    }
    pub fn attributes(&self) -> Attributes {
        self.packed_attributes().unpack(self.is_dir())
    }
    /// The attributes but D, as the query engine tests them.
    pub(crate) fn packed_attributes(&self) -> PackedAttributes {
        self.index.entries.attributes(self.idx)
    }
    pub fn parent(&self) -> EntryIdx {
        self.index.entries.parent(self.idx)
    }
    /// The next entry in this file's hard-link chain, or [`NIL`].
    pub fn next_link(&self) -> EntryIdx {
        self.index.entries.next_link(self.idx)
    }
}
