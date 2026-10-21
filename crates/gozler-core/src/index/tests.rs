//! Tests for the index core: targeted unit cases plus the property tests
//! that carry most of the correctness weight: a model-based op tape, the
//! bootstrap race (shuffled ENUM snapshot + forward journal replay), the
//! walk race (a folder walk during which the filesystem keeps changing, then
//! the same replay) and the folder sizes under arbitrary index changes.

use super::*;
use crate::types::Frn;
use crate::usn::record::{
    FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_BASIC_INFO_CHANGE,
    USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE, USN_REASON_RENAME_NEW_NAME,
};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

// -- record/index construction helpers -----------------------------------

/// FRN with sequence 1 for record number `rec`.
fn frn(rec: u64) -> Frn {
    Frn::ntfs(rec, 1)
}
/// FRN with an explicit sequence, for slot-reuse tests.
fn frn_seq(rec: u64, seq: u16) -> Frn {
    Frn::ntfs(rec, seq)
}

fn root_frn() -> Frn {
    NTFS_ROOT_FRN
}

fn record(frn: Frn, parent: Frn, name: &str, is_dir: bool, reason: u32) -> ParsedUsnRecord {
    ParsedUsnRecord {
        frn,
        parent_frn: parent,
        reason,
        attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
        name: name.as_bytes().to_vec(),
    }
}

fn create(frn: Frn, parent: Frn, name: &str, is_dir: bool) -> ParsedUsnRecord {
    record(frn, parent, name, is_dir, USN_REASON_FILE_CREATE)
}
fn rename(frn: Frn, new_parent: Frn, new_name: &str, is_dir: bool) -> ParsedUsnRecord {
    record(
        frn,
        new_parent,
        new_name,
        is_dir,
        USN_REASON_RENAME_NEW_NAME,
    )
}
fn delete(frn: Frn) -> ParsedUsnRecord {
    record(frn, root_frn(), "", false, USN_REASON_FILE_DELETE)
}

fn new_index() -> VolumeIndex {
    VolumeIndex::new(NTFS_ROOT_FRN)
}

/// Builds an index from ENUM records (bootstrap ingest) in the given order.
fn index_from_enum(recs: &[ParsedUsnRecord]) -> VolumeIndex {
    let mut idx = new_index();
    for r in recs {
        idx.insert_enum(r);
    }
    idx
}

fn path_set(idx: &VolumeIndex) -> BTreeSet<String> {
    idx.collect_real_paths().into_iter().collect()
}

fn path_of_frn(idx: &VolumeIndex, f: Frn) -> Option<String> {
    let head = idx.head_of(f)?;
    let mut buf = Vec::new();
    match idx.path_of(head, &mut buf) {
        PathStatus::Ok => Some(crate::wtf8::to_string_lossy(&buf)),
        _ => None,
    }
}

/// The model's number for the root folder; model entries start at 1000.
const MODEL_ROOT: u64 = 0;

/// How the model's numbers become file IDs.
#[derive(Clone, Copy, Debug)]
enum Ids {
    /// Sequence-1 NTFS file IDs, and the NTFS root.
    Ntfs,
    /// ReFS-shaped IDs that use both halves, as ReFS gives them: the number
    /// in bytes 8..15, and in bytes 0..7 one of three values that many
    /// entries share, so an index that keys on less than the whole ID mixes
    /// entries up.
    Refs,
}

impl Ids {
    fn root(self) -> Frn {
        match self {
            Ids::Ntfs => root_frn(),
            Ids::Refs => Frn(0x600 << 64),
        }
    }

    fn id(self, f: u64) -> Frn {
        match self {
            _ if f == MODEL_ROOT => self.root(),
            Ids::Ntfs => frn(f),
            Ids::Refs => Frn((u128::from(f) << 64) | u128::from(f % 3)),
        }
    }

    fn model(self, id: Frn) -> u64 {
        match self {
            _ if id == self.root() => MODEL_ROOT,
            Ids::Ntfs => id.record_number(),
            Ids::Refs => (id.0 >> 64) as u64,
        }
    }
}

fn arb_ids() -> impl Strategy<Value = Ids> {
    prop_oneof![Just(Ids::Ntfs), Just(Ids::Refs)]
}

// -- targeted unit tests -------------------------------------------------

/// The volume's filesystem decides the width of its file IDs, not its root's
/// ID: an NTFS index keeps 8-byte IDs, not 16-byte ones that would grow
/// every entry, and a ReFS index keys its map on whole
/// IDs, whose low bytes repeat across folders, also while it holds the
/// placeholder root the daemon gives a volume before its first build.
#[test]
fn the_filesystem_decides_the_width_of_the_ids() {
    let mut ntfs = VolumeIndex::with_ids(NTFS_ROOT_FRN, IdWidth::Narrow);
    ntfs.insert_enum(&create(frn(100), root_frn(), "a", false));
    assert!(!ntfs.has_wide_ids());
    assert_eq!(ntfs.memory().frn_map_kind, "sparse");
    let refs = VolumeIndex::with_ids(NTFS_ROOT_FRN, IdWidth::Wide);
    assert!(refs.has_wide_ids());
    assert_eq!(refs.memory().frn_map_kind, "wide");
}

#[test]
fn builds_simple_tree_and_reconstructs_paths() {
    let idx = index_from_enum(&[
        create(frn(100), root_frn(), "docs", true),
        create(frn(101), frn(100), "sub", true),
        create(frn(102), frn(101), "report.pdf", false),
        create(frn(103), root_frn(), "top.txt", false),
    ]);
    assert_eq!(
        path_set(&idx),
        BTreeSet::from([
            "docs".to_string(),
            "docs\\sub".to_string(),
            "docs\\sub\\report.pdf".to_string(),
            "top.txt".to_string(),
        ])
    );
}

#[test]
fn directory_rename_touches_exactly_one_entry() {
    // Build a wide subtree under TOP, then rename TOP and assert one write.
    let mut recs = vec![create(frn(100), root_frn(), "TOP", true)];
    const N: u64 = 3000;
    for i in 0..N {
        recs.push(create(frn(200 + i), frn(100), &format!("f{i}.txt"), false));
    }
    let mut idx = index_from_enum(&recs);
    idx.take_write_count(); // reset the counter after bootstrap

    let outcome = idx.apply_batch(&[rename(frn(100), root_frn(), "RENAMED", true)]);
    assert!(outcome.needs_link_reconcile.is_empty());
    assert_eq!(
        idx.take_write_count(),
        1,
        "renaming a directory must touch exactly its own entry"
    );

    // Every descendant's path now reflects the move, computed lazily.
    assert_eq!(
        path_of_frn(&idx, frn(200)).as_deref(),
        Some("RENAMED\\f0.txt")
    );
    assert_eq!(
        path_of_frn(&idx, frn(200 + N - 1)).as_deref(),
        Some(&*format!("RENAMED\\f{}.txt", N - 1))
    );
}

#[test]
fn orphan_gets_placeholder_then_reparents_when_parent_arrives() {
    let mut idx = new_index();
    // Child before its parent: parent frn 100 is unknown.
    idx.insert_enum(&create(frn(101), frn(100), "child.txt", false));
    assert_eq!(idx.stats().placeholders_created, 1);
    // The child is stranded under lost+found, so it is not a "real" path yet.
    assert!(path_of_frn(&idx, frn(101)).is_none());

    // Parent's own record arrives: the placeholder is filled in place.
    idx.insert_enum(&create(frn(100), root_frn(), "parent", true));
    assert_eq!(
        path_of_frn(&idx, frn(101)).as_deref(),
        Some("parent\\child.txt")
    );
    assert_eq!(
        path_set(&idx),
        BTreeSet::from(["parent".to_string(), "parent\\child.txt".to_string()])
    );
}

#[test]
fn slot_reuse_never_aliases() {
    // Record 100 first hosts FRN(seq 3); then it is reused by FRN(seq 4).
    let mut idx = new_index();
    idx.insert_enum(&create(frn_seq(100, 3), root_frn(), "old.txt", false));
    assert_eq!(
        path_of_frn(&idx, frn_seq(100, 3)).as_deref(),
        Some("old.txt")
    );

    // Missed delete of the old file; the new tenant arrives on the same record.
    idx.apply_batch(&[create(frn_seq(100, 4), root_frn(), "new.txt", false)]);
    assert_eq!(idx.stats().stale_slots, 1);
    assert_eq!(path_of_frn(&idx, frn_seq(100, 3)), None);
    assert_eq!(
        path_of_frn(&idx, frn_seq(100, 4)).as_deref(),
        Some("new.txt")
    );
    assert_eq!(path_set(&idx), BTreeSet::from(["new.txt".to_string()]));
}

/// The same reuse, reached through the PARENT side.
///
/// `upsert_link` reclaims a re-tenanted record; `resolve_or_placeholder_parent`
/// must too. Overwriting the `by_frn` entry alone would leave the old
/// directory's whole subtree live in the entry table but unreachable from the
/// map: ghost files that keep matching queries forever and whose slots can
/// never be reclaimed. Nothing else covers this path, because the child-side
/// test above never resolves a stale record as a parent.
#[test]
fn parent_slot_reuse_never_leaks_the_stale_chain() {
    let mut idx = new_index();
    // Record 100 hosts a directory that holds a file.
    idx.insert_enum(&create(frn_seq(100, 3), root_frn(), "olddir", true));
    idx.insert_enum(&create(frn(200), frn_seq(100, 3), "ghost.txt", false));
    assert_eq!(
        path_of_frn(&idx, frn(200)).as_deref(),
        Some(r"olddir\ghost.txt")
    );

    // The delete is missed and NTFS re-tenants record 100. The new file arrives
    // naming the reused record as its PARENT, so the only path that sees the
    // stale slot is the parent resolver.
    idx.apply_batch(&[create(frn(201), frn_seq(100, 4), "new.txt", false)]);

    assert_eq!(
        idx.stats().stale_slots,
        1,
        "the parent resolver must report the reuse, exactly as upsert_link does"
    );
    assert_eq!(
        path_of_frn(&idx, frn(200)),
        None,
        "the stale chain must be tombstoned, not left searchable"
    );
    assert!(
        !path_set(&idx).iter().any(|p| p.contains("ghost.txt")),
        "ghost.txt outlived its directory: {:?}",
        path_set(&idx)
    );
}

/// Corrupt data can name an entry's own record's previous tenant as its
/// parent. Reclaiming that tenant must not leave the entry linked under the
/// slot it just freed (#77): the entry goes under lost+found, and the
/// folders' child chains stay those a rebuild makes.
#[test]
fn a_parent_that_is_its_own_records_previous_tenant_leaves_no_freed_parent() {
    let mut idx = new_index();
    idx.apply_batch(&[create(frn_seq(1003, 1), root_frn(), "a", false)]);
    // A second name of the same file, so its chain holds two links.
    idx.insert_listed(&listed(frn_seq(1003, 1), root_frn(), "b", false));
    idx.apply_batch(&[create(frn_seq(1003, 2), frn_seq(1003, 1), "c", false)]);
    assert!(idx.child_chains_match_rebuild());
    assert_eq!(
        path_of_frn(&idx, frn_seq(1003, 2)),
        None,
        "under lost+found"
    );
}

/// The same reuse, reached through the FILE_LAYOUT pass.
///
/// A file the MFT enumeration saw is deleted, and NTFS gives its record to a
/// new file before the layout pass reaches it. The layout pass must remove the
/// old chain before it builds the new tenant's; otherwise the deleted file
/// stays live but unreachable by FRN, the replayed delete cannot find it, and
/// it stays in results until the next rescan.
#[test]
fn layout_pass_never_leaks_a_retenanted_record() {
    let gone = frn_seq(100, 3);
    let tenant = frn_seq(100, 4);
    let mut idx = index_from_enum(&[create(gone, root_frn(), "gone.tmp", false)]);

    idx.enrich(&LayoutFile {
        frn: tenant,
        attributes: 0x20,
        size: Some(10),
        mtime_ft: Some(555),
        created_ft: None,
        names: vec![crate::layout::LayoutName {
            parent_frn: root_frn(),
            name: b"tenant.txt".to_vec(),
            dos_only: false,
        }],
    });
    // The journal replay from the cursor captured before the build.
    idx.apply_batch(&[
        delete(gone),
        create(tenant, root_frn(), "tenant.txt", false),
    ]);

    assert_eq!(path_set(&idx), BTreeSet::from(["tenant.txt".to_string()]));
}

/// The daemon caches the entry of the last `-path` folder and checks it with
/// `path_of` on the next query. A removed entry kept its old name id, which a
/// compaction gives to another name or to none, so the check built a wrong
/// path or panicked on every later `-path` query.
#[test]
fn removed_entry_has_no_path_after_compaction() {
    let mut idx = new_index();
    let mut batch: Vec<_> = (0..400)
        .map(|i| create(frn(1000 + i), root_frn(), &format!("{i:040}"), false))
        .collect();
    batch.push(create(frn(100), root_frn(), "proj", true));
    idx.apply_batch(&batch);
    let proj = idx.head_of(frn(100)).unwrap();

    let deletes: Vec<_> = (0..400)
        .map(|i| delete(frn(1000 + i)))
        .chain([delete(frn(100))])
        .collect();
    idx.apply_batch(&deletes);
    let plan = idx
        .plan_compaction()
        .expect("every deleted name is garbage");
    idx.apply_compaction(plan);

    assert_eq!(idx.path_of(proj, &mut Vec::new()), PathStatus::Removed);
}

/// A folder can be removed while a child is still indexed, because the
/// child's own delete was missed. The child kept pointing at the folder's
/// freed slot, so the next entry in that slot showed the child under its own
/// path, and `-path` resolved the child as that entry's subfolder.
#[test]
fn children_of_a_removed_folder_move_to_lost_found() {
    let mut idx = index_from_enum(&[
        create(frn(100), root_frn(), "old", true),
        create(frn(101), frn(100), "sub", true),
    ]);
    // The delete of `sub` is missed; `old` goes, and `new` reuses its slot.
    idx.apply_batch(&[delete(frn(100)), create(frn(102), root_frn(), "new", true)]);
    let new = idx.head_of(frn(102)).unwrap();
    assert_eq!(path_set(&idx), BTreeSet::from(["new".to_string()]));
    assert_eq!(idx.child_dir(new, b"sub".to_vec()), None);
    assert!(
        idx.has_orphans(),
        "queries must check for entries without a path"
    );
    assert!(idx.dir_index_matches_rebuild());
    assert!(idx.child_chains_match_rebuild());

    // The child's own record re-parents it.
    idx.apply_batch(&[rename(frn(101), frn(102), "sub", true)]);
    assert_eq!(
        path_set(&idx),
        BTreeSet::from(["new".to_string(), "new\\sub".to_string()])
    );
    assert!(!idx.has_orphans());
}

/// `optimize_storage` trims the columns to their length plus a small headroom.
/// Growth past it reallocated each column at twice its size, so a few percent
/// of new files pinned up to twice the index's memory for the daemon's
/// lifetime.
#[test]
fn growth_after_the_trim_keeps_little_slack() {
    let file = |i: u64| create(frn(100 + i), root_frn(), &format!("{i}.txt"), false);
    let mut idx = index_from_enum(&(0..10_000).map(file).collect::<Vec<_>>());
    idx.optimize_storage();
    idx.apply_batch(&(10_000..10_500).map(file).collect::<Vec<_>>());
    let memory = idx.memory();
    for (column, bytes) in [
        ("entries", memory.entries),
        ("folded names", memory.arena_folded),
        ("frn map", memory.frn_map),
    ] {
        assert!(
            bytes.allocated * 4 < bytes.used * 5,
            "{column}: {bytes:?} holds more than a quarter of slack"
        );
    }
}

/// After `optimize_storage` the FRN map is a `Vec` indexed by record number. A
/// journal record with a record number near 2^48 grew it to petabytes, and the
/// failed allocation aborted the daemon.
#[test]
fn far_record_number_does_not_grow_the_dense_map() {
    let files: Vec<_> = (0..200)
        .map(|i| create(frn(100 + i), root_frn(), &format!("{i}.txt"), false))
        .collect();
    let mut idx = index_from_enum(&files);
    idx.optimize_storage();
    assert_eq!(idx.memory().frn_map_kind, "dense");

    let far = frn((1 << 48) - 1);
    idx.apply_batch(&[create(far, root_frn(), "far.txt", false)]);
    assert_eq!(path_of_frn(&idx, far).as_deref(), Some("far.txt"));
    assert_eq!(path_of_frn(&idx, frn(100)).as_deref(), Some("0.txt"));
}

#[test]
fn double_apply_equals_single_apply() {
    let mut once = index_from_enum(&[create(frn(100), root_frn(), "dir", true)]);
    let batch = vec![
        create(frn(101), frn(100), "a.txt", false),
        rename(frn(101), frn(100), "b.txt", false),
        create(frn(102), root_frn(), "c.txt", false),
        delete(frn(102)),
    ];
    let mut twice = index_from_enum(&[create(frn(100), root_frn(), "dir", true)]);

    once.apply_batch(&batch);
    twice.apply_batch(&batch);
    twice.apply_batch(&batch); // idempotent replay

    assert_eq!(path_set(&once), path_set(&twice));
    assert_eq!(
        path_set(&once),
        BTreeSet::from(["dir".to_string(), "dir\\b.txt".to_string()])
    );
}

#[test]
fn delete_of_unknown_is_counted_not_fatal() {
    let mut idx = new_index();
    let outcome = idx.apply_batch(&[delete(frn(999))]);
    assert_eq!(idx.stats().delete_of_unknown, 1);
    assert_eq!(
        outcome.anomalies,
        vec![Anomaly::DeleteOfUnknown { frn: frn(999) }]
    );
}

#[test]
fn created_and_deleted_in_same_batch_nets_to_nothing() {
    let mut idx = index_from_enum(&[create(frn(100), root_frn(), "dir", true)]);
    idx.apply_batch(&[
        create(frn(101), frn(100), "temp.tmp", false),
        delete(frn(101)),
    ]);
    assert_eq!(path_set(&idx), BTreeSet::from(["dir".to_string()]));
}

/// USN reason bits accumulate, so one hard-link creation writes a
/// `HARD_LINK_CHANGE` record and then a `HARD_LINK_CHANGE | CLOSE` one. Each
/// FRN the batch reports costs the tail a file open and a link walk, so a
/// file is reported once however many of its records the batch holds.
#[test]
fn a_hard_link_change_is_reported_once_per_batch() {
    use crate::usn::record::{USN_REASON_CLOSE, USN_REASON_HARD_LINK_CHANGE};
    let mut idx = index_from_enum(&[create(frn(100), root_frn(), "a.txt", false)]);
    let change = |reason| record(frn(100), root_frn(), "b.txt", false, reason);
    let outcome = idx.apply_batch(&[
        change(USN_REASON_HARD_LINK_CHANGE),
        change(USN_REASON_HARD_LINK_CHANGE | USN_REASON_CLOSE),
    ]);
    assert_eq!(outcome.needs_link_reconcile, vec![frn(100)]);
}

/// A folder the index lacks that a rename brings in must be walked, even
/// when it was created through the same handle: reason bits accumulate, so
/// that rename record carries FILE_CREATE too. A record of the create alone
/// is a new folder and needs no walk. Every entry the index missed, file or
/// folder, is also read for its size and date, which it would otherwise
/// lack; a rename alone, unlike a create, asks for that only through this
/// rule.
#[test]
fn a_renamed_folder_the_index_lacks_is_reported_for_a_walk() {
    let mut idx = new_index();
    let created = idx.apply_batch(&[create(frn(100), root_frn(), "new", true)]);
    assert!(created.needs_walk.is_empty());
    let moved_in = |idx: &mut VolumeIndex, f: Frn, is_dir: bool, reason: u32| {
        idx.apply_batch(&[record(f, root_frn(), "moved", is_dir, reason)])
    };
    let folder = moved_in(
        &mut idx,
        frn(200),
        true,
        USN_REASON_FILE_CREATE | USN_REASON_RENAME_NEW_NAME,
    );
    assert_eq!(folder.needs_walk, vec![frn(200)]);
    let folder = moved_in(&mut idx, frn(300), true, USN_REASON_RENAME_NEW_NAME);
    assert_eq!(
        (folder.needs_walk, folder.needs_stat),
        (vec![frn(300)], vec![frn(300)])
    );
    let file = moved_in(&mut idx, frn(400), false, USN_REASON_RENAME_NEW_NAME);
    assert_eq!((file.needs_walk, file.needs_stat), (vec![], vec![frn(400)]));
}

// -- hard links ----------------------------------------------------------

use crate::layout::LayoutFile;
use crate::layout::fixtures::{LayoutFileFixture, LayoutNameFixture, LayoutStreamFixture};

/// `f` as a FILE_LAYOUT record holds it: NTFS file IDs are 64 bits.
fn layout_id(f: Frn) -> u64 {
    u64::try_from(f.0).expect("an NTFS file ID fits in 64 bits")
}

fn enrich_links(idx: &mut VolumeIndex, fixture: &LayoutFileFixture) {
    let file: LayoutFile = fixture.expected();
    idx.enrich(&file);
}

/// The FILE_LAYOUT pass returns the root folder's own record, which names the
/// root as its parent. Taken as a name, it made the root its own parent and
/// left no entry a path.
#[test]
fn layout_pass_builds_every_path_with_the_roots_own_record_and_hard_links() {
    let root = layout_id(root_frn());
    let records = [
        LayoutFileFixture {
            frn: root,
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            names: vec![LayoutNameFixture::primary(root, ".")],
            info: None,
            streams: vec![],
        },
        // Listed before the folder of its second name, which starts as a
        // placeholder.
        LayoutFileFixture {
            frn: layout_id(frn(100)),
            attributes: 0x20,
            names: vec![
                LayoutNameFixture::primary(root, "report.txt"),
                LayoutNameFixture::dos_only(root, "REPORT~1.TXT"),
                LayoutNameFixture::primary(layout_id(frn(10)), "copy.txt"),
            ],
            info: None,
            streams: vec![LayoutStreamFixture::unnamed_data(4096)],
        },
        LayoutFileFixture {
            frn: layout_id(frn(10)),
            attributes: FILE_ATTRIBUTE_DIRECTORY,
            names: vec![LayoutNameFixture::primary(root, "docs")],
            info: None,
            streams: vec![],
        },
    ];
    let mut idx = new_index();
    for record in &records {
        idx.insert_layout(&record.expected());
    }

    assert_eq!(
        path_set(&idx),
        BTreeSet::from(["docs", "docs\\copy.txt", "report.txt"].map(String::from))
    );
}

#[test]
fn enrich_builds_hard_link_chain_findable_under_every_name() {
    // Two dirs and a file that FILE_LAYOUT reports as hard-linked under both.
    let mut idx = index_from_enum(&[
        create(frn(10), root_frn(), "dirA", true),
        create(frn(11), root_frn(), "dirB", true),
        create(frn(100), frn(10), "link1.bin", false),
    ]);
    enrich_links(
        &mut idx,
        &LayoutFileFixture {
            frn: layout_id(frn(100)),
            attributes: 0x20,
            names: vec![
                LayoutNameFixture::primary(layout_id(frn(10)), "link1.bin"),
                LayoutNameFixture {
                    parent_frn: layout_id(frn(11)),
                    units: "link2.bin".encode_utf16().collect(),
                    flags: 0,
                },
            ],
            info: None,
            streams: vec![LayoutStreamFixture::unnamed_data(4096)],
        },
    );

    assert_eq!(
        path_set(&idx),
        BTreeSet::from([
            "dirA".to_string(),
            "dirB".to_string(),
            "dirA\\link1.bin".to_string(),
            "dirB\\link2.bin".to_string(),
        ])
    );
    // Size enrichment reaches EVERY link of the file, not just the head: a
    // query may return any name, and all links describe the same bytes.
    let mut buf = Vec::new();
    for f in [100u64] {
        let head = idx.head_of(frn(f)).unwrap();
        let mut cur = head;
        let mut link_count = 0;
        while cur != crate::types::NIL {
            assert_eq!(
                idx.entry(cur).size(),
                Some(4096),
                "every hard-link entry must carry the file's size"
            );
            link_count += 1;
            buf.clear();
            let _ = idx.path_of(cur, &mut buf);
            cur = idx.entry(cur).next_link();
        }
        assert_eq!(link_count, 2, "both links present");
    }
}

#[test]
fn file_delete_removes_the_whole_link_chain() {
    let mut idx = index_from_enum(&[
        create(frn(10), root_frn(), "dirA", true),
        create(frn(11), root_frn(), "dirB", true),
        create(frn(100), frn(10), "l1", false),
    ]);
    enrich_links(
        &mut idx,
        &LayoutFileFixture {
            frn: layout_id(frn(100)),
            attributes: 0x20,
            names: vec![
                LayoutNameFixture::primary(layout_id(frn(10)), "l1"),
                LayoutNameFixture {
                    parent_frn: layout_id(frn(11)),
                    units: "l2".encode_utf16().collect(),
                    flags: 0,
                },
            ],
            info: None,
            streams: vec![],
        },
    );
    assert_eq!(idx.collect_real_paths().len(), 4); // 2 dirs + 2 links

    // FILE_DELETE = the file record died: every link disappears.
    idx.apply_batch(&[delete(frn(100))]);
    assert_eq!(
        path_set(&idx),
        BTreeSet::from(["dirA".to_string(), "dirB".to_string()])
    );
}

#[test]
fn reconcile_links_adds_and_removes_individual_links() {
    let mut idx = index_from_enum(&[
        create(frn(10), root_frn(), "dirA", true),
        create(frn(11), root_frn(), "dirB", true),
        create(frn(100), frn(10), "only", false),
    ]);
    let a = idx.head_of(frn(10)).unwrap();
    let b = idx.head_of(frn(11)).unwrap();

    // A second link appears in dirB.
    idx.reconcile_links(
        frn(100),
        &[
            LinkTarget {
                parent: a,
                name: b"only".to_vec(),
            },
            LinkTarget {
                parent: b,
                name: b"second".to_vec(),
            },
        ],
    );
    assert_eq!(
        path_set(&idx),
        BTreeSet::from([
            "dirA".to_string(),
            "dirB".to_string(),
            "dirA\\only".to_string(),
            "dirB\\second".to_string(),
        ])
    );

    // The dirB link is removed again.
    idx.reconcile_links(
        frn(100),
        &[LinkTarget {
            parent: a,
            name: b"only".to_vec(),
        }],
    );
    assert_eq!(
        path_set(&idx),
        BTreeSet::from([
            "dirA".to_string(),
            "dirB".to_string(),
            "dirA\\only".to_string(),
        ])
    );
}

/// The tail stats a file before it reconciles the file's links, and nothing
/// stats a link that reconciliation adds. A new hard link, or the new name of
/// a renamed link, must still carry the size and date every link shares.
#[test]
fn a_reconciled_link_gets_the_size_and_date_of_its_file() {
    let mut idx = index_from_enum(&[
        create(frn(10), root_frn(), "dirA", true),
        create(frn(11), root_frn(), "dirB", true),
        create(frn(100), frn(10), "orig.txt", false),
    ]);
    idx.set_stat(frn(100), 17, 1234);
    let a = idx.head_of(frn(10)).unwrap();
    let b = idx.head_of(frn(11)).unwrap();
    let link = |parent, name: &str| LinkTarget {
        parent,
        name: name.as_bytes().to_vec(),
    };
    let stats = |idx: &VolumeIndex| -> Vec<(String, Option<u64>, Option<i64>)> {
        let mut out = Vec::new();
        let mut cur = idx.head_of(frn(100)).unwrap();
        while cur != crate::types::NIL {
            let e = idx.entry(cur);
            out.push((
                String::from_utf8_lossy(e.name()).into_owned(),
                e.size(),
                e.mtime(),
            ));
            cur = e.next_link();
        }
        out.sort();
        out
    };

    // `fsutil hardlink create dirB\link.txt dirA\orig.txt`.
    idx.reconcile_links(frn(100), &[link(a, "orig.txt"), link(b, "link.txt")]);
    assert_eq!(
        stats(&idx),
        [
            ("link.txt".to_string(), Some(17), Some(1234)),
            ("orig.txt".to_string(), Some(17), Some(1234)),
        ]
    );

    // A name-only rename of the first link; no stat follows.
    idx.reconcile_links(frn(100), &[link(a, "renamed.txt"), link(b, "link.txt")]);
    assert_eq!(
        stats(&idx),
        [
            ("link.txt".to_string(), Some(17), Some(1234)),
            ("renamed.txt".to_string(), Some(17), Some(1234)),
        ]
    );
}

/// A journal record names one link, but the attributes belong to the file, so
/// an attribute change must reach every link, and a link that reconciliation
/// adds later must start with them; otherwise `/ah` finds a hidden file under
/// some of its names only.
#[test]
fn attributes_reach_every_link_of_a_file() {
    let mut idx = index_from_enum(&[
        create(frn(10), root_frn(), "dirA", true),
        create(frn(11), root_frn(), "dirB", true),
        create(frn(100), frn(10), "orig.txt", false),
    ]);
    let a = idx.head_of(frn(10)).unwrap();
    let b = idx.head_of(frn(11)).unwrap();
    let link = |parent, name: &str| LinkTarget {
        parent,
        name: name.as_bytes().to_vec(),
    };
    idx.reconcile_links(frn(100), &[link(a, "orig.txt"), link(b, "link.txt")]);

    // `attrib +h dirA\orig.txt` writes one record, which names that link.
    let mut hide = record(
        frn(100),
        frn(10),
        "orig.txt",
        false,
        USN_REASON_BASIC_INFO_CHANGE,
    );
    hide.attributes = 0x2 | 0x20;
    idx.apply_batch(&[hide]);
    idx.reconcile_links(
        frn(100),
        &[
            link(a, "orig.txt"),
            link(b, "link.txt"),
            link(b, "third.txt"),
        ],
    );

    let mut letters = Vec::new();
    let mut cur = idx.head_of(frn(100)).unwrap();
    while cur != crate::types::NIL {
        let e = idx.entry(cur);
        letters.push((
            String::from_utf8_lossy(e.name()).into_owned(),
            e.attributes().letters(),
        ));
        cur = e.next_link();
    }
    letters.sort();
    let hidden = |name: &str| (name.to_string(), "HA".to_string());
    assert_eq!(
        letters,
        [hidden("link.txt"), hidden("orig.txt"), hidden("third.txt")]
    );
}

#[test]
fn lone_surrogate_names_round_trip_through_the_index() {
    let mut idx = new_index();
    let mut name = Vec::new();
    crate::wtf8::from_utf16(&[0x0061, 0xD800, 0x0062], &mut name);
    let rec = ParsedUsnRecord {
        frn: frn(100),
        parent_frn: root_frn(),
        reason: USN_REASON_FILE_CREATE,
        attributes: 0,
        name: name.clone(),
    };
    idx.insert_enum(&rec);
    let head = idx.head_of(frn(100)).unwrap();
    assert_eq!(idx.entry(head).name(), name.as_slice());
    assert_eq!(
        crate::wtf8::to_utf16(idx.entry(head).name()),
        vec![0x0061, 0xD800, 0x0062]
    );
}

#[test]
fn path_cycle_is_detected_not_looped() {
    // Build a -> b (b under a), then force a's parent to b, making the 2-cycle
    // a -> b -> a. The public API cannot produce this; force_parent simulates
    // corrupt/raced links so we can prove path_of reports it instead of
    // hanging.
    let mut idx = index_from_enum(&[
        create(frn(100), root_frn(), "a", true),
        create(frn(101), frn(100), "b", true),
    ]);
    let a = idx.head_of(frn(100)).unwrap();
    let b = idx.head_of(frn(101)).unwrap();
    idx.force_parent(a, b);
    let mut buf = Vec::new();
    assert_eq!(idx.path_of(a, &mut buf), PathStatus::CycleDetected);
}

// -- model-based property test -------------------------------------------

/// A reference filesystem the index must mirror.
#[derive(Clone, Default)]
struct Model {
    // frn -> (parent frn, name, is_dir)
    entries: HashMap<u64, (u64, String, bool)>,
}

impl Model {
    fn dirs(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, (_, _, d))| *d)
            .map(|(f, _)| *f)
            .collect();
        v.push(MODEL_ROOT);
        v.sort_unstable();
        v
    }
    fn all(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.entries.keys().copied().collect();
        v.sort_unstable();
        v
    }
    fn is_descendant_or_self(&self, node: u64, maybe_ancestor: u64) -> bool {
        let mut cur = node;
        for _ in 0..10_000 {
            if cur == maybe_ancestor {
                return true;
            }
            match self.entries.get(&cur) {
                Some((p, _, _)) => cur = *p,
                None => return false,
            }
        }
        false
    }
    fn has_children(&self, node: u64) -> bool {
        self.entries.values().any(|(p, _, _)| *p == node)
    }
    fn path_of(&self, node: u64) -> String {
        let mut parts = Vec::new();
        let mut cur = node;
        while let Some((p, name, _)) = self.entries.get(&cur) {
            parts.push(name.clone());
            cur = *p;
        }
        parts.reverse();
        parts.join("\\")
    }
    fn path_set(&self) -> BTreeSet<String> {
        self.entries.keys().map(|f| self.path_of(*f)).collect()
    }
    fn exists(&self, node: u64) -> bool {
        node == MODEL_ROOT || self.entries.contains_key(&node)
    }
    /// What a folder walk lists for `folder` now: one single-name entry per
    /// name in it.
    fn listing(&self, ids: Ids, folder: u64) -> Vec<LayoutFile> {
        let mut children: Vec<(&u64, &(u64, String, bool))> = self
            .entries
            .iter()
            .filter(|(_, (p, _, _))| *p == folder)
            .collect();
        children.sort_unstable_by_key(|(f, _)| **f);
        children
            .into_iter()
            .map(|(f, (_, name, is_dir))| listed(ids.id(*f), ids.id(folder), name, *is_dir))
            .collect()
    }
}

/// One name a folder walk lists.
fn listed(f: Frn, folder: Frn, name: &str, is_dir: bool) -> LayoutFile {
    LayoutFile {
        frn: f,
        attributes: if is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            0x20
        },
        size: (!is_dir).then_some(0),
        mtime_ft: Some(0),
        created_ft: None,
        names: vec![crate::layout::LayoutName {
            parent_frn: folder,
            name: name.as_bytes().to_vec(),
            dos_only: false,
        }],
    }
}

/// The volume a walk race reads: `states[t]` is the model after `t` changes,
/// and `now` is the state each read sees. While `listing_changes` is
/// `Some`, the model changes before each listing by the next count, cycled,
/// as a volume does while a walk builds its index.
///
/// Each read checks that `index` is not locked, because a volume read made
/// under the index lock would stall every search on the volume.
struct ModelReads<'a> {
    index: &'a RefCell<VolumeIndex>,
    ids: Ids,
    states: &'a [Model],
    now: usize,
    listing_changes: Option<std::iter::Cycle<std::slice::Iter<'a, usize>>>,
}

impl ModelReads<'_> {
    fn assert_unlocked(&self) {
        assert!(
            self.index.try_borrow_mut().is_ok(),
            "a volume read under the index lock"
        );
    }
}

impl settle::VolumeReads for ModelReads<'_> {
    type Error = std::convert::Infallible;

    fn list_folder(
        &mut self,
        folder: Frn,
        page: &mut dyn FnMut(Vec<LayoutFile>),
    ) -> Result<bool, Self::Error> {
        self.assert_unlocked();
        if let Some(changes) = &mut self.listing_changes {
            let last = self.states.len() - 1;
            self.now = (self.now + changes.next().expect("cycled")).min(last);
        }
        let model = &self.states[self.now];
        let folder = self.ids.model(folder);
        if !model.exists(folder) {
            return Ok(false);
        }
        page(model.listing(self.ids, folder));
        Ok(true)
    }

    fn stat(&mut self, _: Frn) -> Result<Option<settle::FileStat>, Self::Error> {
        self.assert_unlocked();
        Ok(Some(settle::FileStat {
            size: 0,
            mtime: 0,
            created: 0,
        }))
    }

    /// The model gives each file one name.
    fn links(&mut self, frn: Frn) -> Result<Option<Vec<settle::FileLink>>, Self::Error> {
        self.assert_unlocked();
        let entry = self.states[self.now].entries.get(&self.ids.model(frn));
        Ok(entry.map(|(parent, name, _)| {
            vec![settle::FileLink {
                folder: self.ids.id(*parent),
                name: name.clone().into_bytes(),
            }]
        }))
    }
}

/// A raw action carrying selection seeds; the interpreter resolves them
/// against the live model so every executed action is valid.
#[derive(Clone, Debug)]
enum RawAction {
    Create {
        parent_sel: usize,
        name_sel: u16,
        is_dir: bool,
    },
    Rename {
        target_sel: usize,
        parent_sel: usize,
        name_sel: u16,
    },
    Delete {
        target_sel: usize,
    },
}

/// Unique-per-entry name so distinct entries never share a path; the `sel`
/// prefix lets a rename actually change the name.
fn model_name(sel: u16, frn: u64) -> String {
    format!("{sel}-{frn}")
}

use proptest::prelude::*;

fn raw_action_strategy() -> impl Strategy<Value = RawAction> {
    prop_oneof![
        (any::<u16>(), any::<u16>(), any::<bool>()).prop_map(|(p, n, d)| RawAction::Create {
            parent_sel: p as usize,
            name_sel: n,
            is_dir: d
        }),
        (any::<u16>(), any::<u16>(), any::<u16>()).prop_map(|(t, p, n)| RawAction::Rename {
            target_sel: t as usize,
            parent_sel: p as usize,
            name_sel: n
        }),
        any::<u16>().prop_map(|t| RawAction::Delete {
            target_sel: t as usize
        }),
    ]
}

/// Mutates the model for one action and returns the corresponding USN record
/// (or `None` when the action is not currently valid, e.g. delete with no
/// leaves).
fn step_model(
    model: &mut Model,
    next_frn: &mut u64,
    ids: Ids,
    action: &RawAction,
) -> Option<ParsedUsnRecord> {
    match action {
        RawAction::Create {
            parent_sel,
            name_sel,
            is_dir,
        } => {
            let dirs = model.dirs();
            let parent = dirs[parent_sel % dirs.len()];
            let f = *next_frn;
            *next_frn += 1;
            let name = model_name(*name_sel, f);
            model.entries.insert(f, (parent, name.clone(), *is_dir));
            Some(create(ids.id(f), ids.id(parent), &name, *is_dir))
        }
        RawAction::Rename {
            target_sel,
            parent_sel,
            name_sel,
        } => {
            let all = model.all();
            if all.is_empty() {
                return None;
            }
            let target = all[target_sel % all.len()];
            let is_dir = model.entries[&target].2;
            let candidates: Vec<u64> = model
                .dirs()
                .into_iter()
                .filter(|d| !model.is_descendant_or_self(*d, target))
                .collect();
            if candidates.is_empty() {
                return None;
            }
            let new_parent = candidates[parent_sel % candidates.len()];
            let name = model_name(*name_sel, target);
            let e = model.entries.get_mut(&target).unwrap();
            e.0 = new_parent;
            e.1 = name.clone();
            Some(rename(ids.id(target), ids.id(new_parent), &name, is_dir))
        }
        RawAction::Delete { target_sel } => {
            let leaves: Vec<u64> = model
                .all()
                .into_iter()
                .filter(|f| !model.has_children(*f))
                .collect();
            if leaves.is_empty() {
                return None;
            }
            let target = leaves[target_sel % leaves.len()];
            model.entries.remove(&target);
            Some(delete(ids.id(target)))
        }
    }
}

/// Deterministic in-place shuffle (xorshift; no `Date`/`rand` needed).
fn shuffle<T>(v: &mut [T], seed: u64) {
    let mut state = seed | 1;
    for i in (1..v.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(120))]

    /// The index mirrors a model filesystem across arbitrary create/rename/
    /// delete tapes, including O(1) directory renames, orphan resolution, and
    /// idempotent replays (each record applied twice).
    #[test]
    fn model_op_tape_convergence(
        actions in prop::collection::vec(raw_action_strategy(), 0..80),
        ids in arb_ids(),
    ) {
        let mut model = Model::default();
        let mut idx = VolumeIndex::new(ids.root());
        let mut next_frn: u64 = 1000;
        for action in &actions {
            if let Some(rec) = step_model(&mut model, &mut next_frn, ids, action) {
                idx.apply_batch(std::slice::from_ref(&rec));
                prop_assert_eq!(path_set(&idx), model.path_set());
                prop_assert!(idx.dir_index_matches_rebuild(), "dir index drifted after apply");
                prop_assert!(idx.name_chains_match_rebuild(), "name chains drifted after apply");
                prop_assert!(idx.child_chains_match_rebuild(), "child chains drifted after apply");
                idx.apply_batch(std::slice::from_ref(&rec)); // idempotent replay
                prop_assert_eq!(path_set(&idx), model.path_set());
                prop_assert!(idx.dir_index_matches_rebuild(), "dir index drifted after replay");
                prop_assert!(idx.name_chains_match_rebuild(), "name chains drifted after replay");
                prop_assert!(idx.child_chains_match_rebuild(), "child chains drifted after replay");
            }
        }
    }

    /// Bootstrap race: read a snapshot in ARBITRARY order (parents may follow
    /// children → placeholder synthesis), then replay the journal tape from
    /// that point. The result must equal the final model regardless of the
    /// order, and whether the FILE_LAYOUT pass completed or stopped part way
    /// and left the enumeration to name the rest, as the bootstrap does. ReFS
    /// IDs run through the same build and replay, so a lookup that keys on
    /// less than the whole ID fails here too.
    #[test]
    fn bootstrap_race_converges(
        actions in prop::collection::vec(raw_action_strategy(), 0..70),
        split in 0.0f64..1.0,
        shuffle_seed in any::<u64>(),
        // `None`: the FILE_LAYOUT pass completed. `Some`: it stopped after
        // that fraction of the snapshot, and the enumeration read all of it.
        layout_stop in prop::option::of(0.0f64..1.0),
        ids in arb_ids(),
    ) {
        let mut model = Model::default();
        let mut next_frn: u64 = 1000;
        let mut journal: Vec<ParsedUsnRecord> = Vec::new();
        let split_at = (actions.len() as f64 * split) as usize;
        let mut snapshot: Option<Model> = None;

        for (i, action) in actions.iter().enumerate() {
            if i == split_at {
                snapshot = Some(model.clone());
            }
            if let Some(rec) = step_model(&mut model, &mut next_frn, ids, action)
                && i >= split_at
            {
                journal.push(rec);
            }
        }
        let snapshot = snapshot.unwrap_or_else(|| model.clone());

        // Read the snapshot in a shuffled order (the race the ordering must survive).
        let mut enum_recs: Vec<ParsedUsnRecord> = snapshot
            .entries
            .iter()
            .map(|(f, (p, name, is_dir))| create(ids.id(*f), ids.id(*p), name, *is_dir))
            .collect();
        shuffle(&mut enum_recs, shuffle_seed);

        let layout_count = layout_stop
            .map_or(enum_recs.len(), |stop| (enum_recs.len() as f64 * stop) as usize);
        let mut idx = VolumeIndex::new(ids.root());
        for r in &enum_recs[..layout_count] {
            idx.insert_layout(&LayoutFile {
                frn: r.frn,
                attributes: r.attributes,
                size: None,
                mtime_ft: None,
                created_ft: None,
                names: vec![crate::layout::LayoutName {
                    parent_frn: r.parent_frn,
                    name: r.name.clone(),
                    dos_only: false,
                }],
            });
        }
        if layout_stop.is_some() {
            for r in &enum_recs {
                idx.insert_enum(r);
            }
        }
        idx.apply_batch(&journal);

        prop_assert_eq!(path_set(&idx), model.path_set());
        prop_assert!(idx.dir_index_matches_rebuild(), "dir index drifted after bootstrap race");
        prop_assert!(idx.name_chains_match_rebuild(), "name chains drifted after bootstrap race");
        prop_assert!(idx.child_chains_match_rebuild(), "child chains drifted after bootstrap race");
    }

    /// Walk race: a folder walk is not a snapshot, so the model keeps changing
    /// between the folders it lists. The journal then replays from the cursor
    /// taken before the walk, in batches, and `settle::settle_batch` settles
    /// each batch, reading the volume at that moment or later, as the tail
    /// does. The result must equal the final model. Without the missed-folder
    /// rule a
    /// folder renamed into an already listed part loses its contents, and a
    /// folder listed twice must not duplicate them.
    #[test]
    fn walk_race_converges(
        actions in prop::collection::vec(raw_action_strategy(), 20..120),
        // Where the cursor falls: late enough that a tree exists for the
        // walk, early enough that changes run through it.
        cursor in 0.4f64..0.8,
        // Changes made before each listing, cycled.
        changes_per_listing in prop::collection::vec(0usize..6, 1..8),
        // Whether a replay batch ends after each record, cycled.
        batch_ends in prop::collection::vec(any::<bool>(), 1..8),
        // How many changes later than its batch a missed-folder walk lists.
        walk_delays in prop::collection::vec(0usize..4, 1..8),
        ids in arb_ids(),
    ) {
        // states[t] is the model after t changes; a record is tagged with
        // the state its change produced.
        let mut model = Model::default();
        let mut next_frn: u64 = 1000;
        let mut states = vec![model.clone()];
        let mut records: Vec<(usize, ParsedUsnRecord)> = Vec::new();
        for action in &actions {
            if let Some(rec) = step_model(&mut model, &mut next_frn, ids, action) {
                states.push(model.clone());
                records.push((states.len() - 1, rec));
            }
        }
        let last = states.len() - 1;
        let start = (last as f64 * cursor) as usize;

        let index = RefCell::new(VolumeIndex::new(ids.root()));
        let mut reads = ModelReads {
            index: &index,
            ids,
            states: &states,
            now: start,
            listing_changes: Some(changes_per_listing.iter().cycle()),
        };
        let walked = settle::walk_folders(&mut reads, &[ids.root()], || false, |files| {
            files.iter().for_each(|f| index.borrow_mut().insert_listed(f));
        });
        prop_assert!(matches!(walked, Ok(settle::Walk::Complete)));
        reads.listing_changes = None;

        let mut delays = walk_delays.iter().cycle();
        let mut ends = batch_ends.iter().cycle();
        let mut batch = Vec::new();
        let mut walked_at = reads.now;
        let journal = records.iter().filter(|(t, _)| *t > start);
        let count = journal.clone().count();
        for (i, (t, rec)) in journal.enumerate() {
            batch.push(rec.clone());
            if !*ends.next().expect("cycled") && i + 1 < count {
                continue;
            }
            walked_at = (walked_at.max(*t) + delays.next().expect("cycled")).min(last);
            reads.now = walked_at;
            let settled = settle::settle_batch(
                &index,
                &mut reads,
                settle::Batch { records: &batch, replaying_walk: true },
                || false,
            );
            prop_assert!(settled.is_ok());
            batch.clear();
        }

        let idx = index.borrow();
        prop_assert_eq!(path_set(&idx), model.path_set());
        prop_assert!(idx.dir_index_matches_rebuild(), "dir index drifted after walk race");
        prop_assert!(idx.name_chains_match_rebuild(), "name chains drifted after walk race");
        prop_assert!(idx.child_chains_match_rebuild(), "child chains drifted after walk race");
    }
}

/// A folder walk meets a hard-linked file once in each of its folders, and a
/// folder that moved during the walk can be listed twice. The model above has
/// no hard links, so this pins both: the file keeps every name, and the
/// second listing of a folder adds nothing.
#[test]
fn a_walk_keeps_hard_links_and_adds_a_folder_listed_twice_once() {
    let ids = Ids::Refs;
    let root = ids.root();
    let mut idx = VolumeIndex::new(root);
    let (a, b, file) = (ids.id(1000), ids.id(1001), ids.id(1002));
    for listing in [
        listed(a, root, "a", true),
        listed(b, root, "b", true),
        listed(file, a, "one.txt", false),
        listed(file, b, "two.txt", false),
        listed(b, a, "b", true),
    ] {
        idx.insert_listed(&listing);
    }
    assert_eq!(
        path_set(&idx),
        BTreeSet::from(["a", "a\\one.txt", "b", "b\\two.txt"].map(String::from))
    );
    assert_eq!(idx.len(), 2 + 4, "root, lost+found and four entries");
}

#[test]
fn enrich_with_renamed_directory_never_orphans_children() {
    // The ENUM pass sees dir "docs" (frn 100) with child "a.txt"; a rename to
    // "documents" is applied (in place) before the FILE_LAYOUT pass, whose
    // snapshot still reports the old name "docs". Enrichment must NOT
    // tombstone the directory entry (which would orphan the child under a
    // reused slot).
    let mut idx = index_from_enum(&[
        create(frn(100), root_frn(), "docs", true),
        create(frn(101), frn(100), "a.txt", false),
    ]);
    idx.apply_batch(&[rename(frn(100), root_frn(), "documents", true)]);
    assert_eq!(
        path_of_frn(&idx, frn(101)).as_deref(),
        Some("documents\\a.txt")
    );

    idx.enrich(&LayoutFile {
        frn: frn(100),
        attributes: FILE_ATTRIBUTE_DIRECTORY,
        size: None,
        mtime_ft: Some(555),
        created_ft: None,
        names: vec![crate::layout::LayoutName {
            parent_frn: root_frn(),
            name: b"docs".to_vec(),
            dos_only: false,
        }],
    });

    // The child is still correctly under the (renamed) directory, and a later
    // allocation reusing any freed slot cannot misparent it.
    idx.apply_batch(&[create(frn(200), root_frn(), "unrelated.txt", false)]);
    assert_eq!(
        path_of_frn(&idx, frn(101)).as_deref(),
        Some("documents\\a.txt")
    );
    assert_eq!(idx.entry(idx.head_of(frn(100)).unwrap()).mtime(), Some(555));
}

#[test]
fn model_names_are_path_unique() {
    // Guards the proptest's assumption; also checks same-name-different-dir.
    let mut seen = HashSet::new();
    let idx = index_from_enum(&[
        create(frn(100), root_frn(), "x", true),
        create(frn(101), frn(100), "x", true),
    ]);
    for p in idx.collect_real_paths() {
        assert!(seen.insert(p), "paths must be unique");
    }
}

// -- folder sizes ----------------------------------------------------------

/// One change to an index. Each `u8` selects from a small pool of file IDs,
/// so that changes meet the same entries often.
#[derive(Clone, Debug)]
enum IndexChange {
    /// A create record, also for an ID the index already holds.
    Create {
        target: u8,
        parent: u8,
        is_dir: bool,
    },
    /// A rename record into any folder, the entry's own descendants and files
    /// included, so loops in the parent links and files that hold entries
    /// happen.
    Rename {
        target: u8,
        parent: u8,
    },
    Delete {
        target: u8,
    },
    /// The stat that follows a change of size.
    Stat {
        target: u8,
        size: u16,
    },
    /// A hard-link reconcile to one name in each of these folders.
    Links {
        target: u8,
        parents: Vec<u8>,
    },
    /// One name a folder walk lists, as the tail lists a folder it missed.
    Listed {
        target: u8,
        parent: u8,
        is_dir: bool,
        size: u16,
    },
}

/// Twelve record numbers, each with two sequence numbers, so that a create
/// can reuse a record number as NTFS does.
fn pool_id(sel: u8) -> Frn {
    frn_seq(1000 + u64::from(sel % 12), 1 + u16::from(sel / 12 % 2))
}

/// A folder of the pool, or the root.
fn pool_parent(sel: u8) -> Frn {
    if sel.is_multiple_of(5) {
        root_frn()
    } else {
        pool_id(sel)
    }
}

fn index_change_strategy() -> impl Strategy<Value = IndexChange> {
    prop_oneof![
        (any::<u8>(), any::<u8>(), any::<bool>()).prop_map(|(target, parent, is_dir)| {
            IndexChange::Create {
                target,
                parent,
                is_dir,
            }
        }),
        (any::<u8>(), any::<u8>())
            .prop_map(|(target, parent)| IndexChange::Rename { target, parent }),
        any::<u8>().prop_map(|target| IndexChange::Delete { target }),
        (any::<u8>(), any::<u16>()).prop_map(|(target, size)| IndexChange::Stat { target, size }),
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..3))
            .prop_map(|(target, parents)| IndexChange::Links { target, parents }),
        (any::<u8>(), any::<u8>(), any::<bool>(), any::<u16>()).prop_map(
            |(target, parent, is_dir, size)| IndexChange::Listed {
                target,
                parent,
                is_dir,
                size,
            }
        ),
    ]
}

fn apply_change(idx: &mut VolumeIndex, change: &IndexChange) {
    match *change {
        IndexChange::Create {
            target,
            parent,
            is_dir,
        } => {
            let name = format!("c{target}");
            idx.apply_batch(&[create(pool_id(target), pool_parent(parent), &name, is_dir)]);
        }
        IndexChange::Rename { target, parent } => {
            let id = pool_id(target);
            let is_dir = idx.head_of(id).is_some_and(|h| idx.entry(h).is_dir());
            let name = format!("r{target}");
            idx.apply_batch(&[rename(id, pool_parent(parent), &name, is_dir)]);
        }
        IndexChange::Delete { target } => {
            idx.apply_batch(&[delete(pool_id(target))]);
        }
        IndexChange::Stat { target, size } => idx.set_stat(pool_id(target), size.into(), 0),
        IndexChange::Links {
            target,
            ref parents,
        } => {
            let id = pool_id(target);
            if idx.head_of(id).is_some_and(|h| !idx.entry(h).is_dir()) {
                let mut folders: Vec<_> = parents
                    .iter()
                    .filter_map(|&p| idx.head_of(pool_parent(p)))
                    .collect();
                folders.sort_unstable();
                folders.dedup();
                let links: Vec<LinkTarget> = folders
                    .into_iter()
                    .map(|parent| LinkTarget {
                        parent,
                        name: b"link".to_vec(),
                    })
                    .collect();
                idx.reconcile_links(id, &links);
            }
        }
        IndexChange::Listed {
            target,
            parent,
            is_dir,
            size,
        } => idx.insert_listed(&LayoutFile {
            size: (!is_dir).then_some(size.into()),
            ..listed(
                pool_id(target),
                pool_parent(parent),
                &format!("l{target}"),
                is_dir,
            )
        }),
    }
}

proptest! {
    /// After the size and date pass ends, every way the journal changes the
    /// index keeps each folder's size equal to the sum recomputed from the
    /// files below it: creates, deletes, size changes, moves of files and of
    /// folders, hard links added and removed, placeholders, record numbers
    /// reused, deletes of unknown IDs, and loops in the parent links. Before
    /// the pass ends, every folder's size is unknown.
    #[test]
    fn folder_sizes_stay_the_sum_of_their_files(
        changes in prop::collection::vec(index_change_strategy(), 0..60),
        pass_end in 0usize..20,
    ) {
        let mut idx = new_index();
        for (i, change) in changes.iter().enumerate() {
            if i == pass_end {
                idx.compute_folder_sizes();
            }
            apply_change(&mut idx, change);
            prop_assert!(idx.child_chains_match_rebuild(), "child chains drifted after {:?}", change);
            prop_assert!(idx.folder_sizes_match_rebuild(), "folder sizes drifted after {:?}", change);
        }
    }
}

// -- the Include rules ---------------------------------------------------

use crate::include::{DriveChoice, FolderChoice, IndexSettings};
use std::sync::Arc;

/// The rules of a volume whose drive is `drive`, with `folders` and the
/// name patterns `names`.
fn include_rules(
    drive: bool,
    folders: &[(&str, bool)],
    names: &str,
) -> Arc<crate::include::VolumeRules> {
    let settings = IndexSettings {
        drives: vec![DriveChoice {
            volume: "V".into(),
            name: String::new(),
            indexed: drive,
            folders: folders
                .iter()
                .map(|&(path, indexed)| FolderChoice {
                    path: path.into(),
                    indexed,
                })
                .collect(),
        }],
        excluded_names: names.into(),
        ..IndexSettings::default()
    };
    Arc::new(settings.rules_for("V"))
}

/// `Windows\System32\a.dll`, `Windows\Temp\t.txt`, `Users\ana\doc.txt`,
/// `Users\ana\x.tmp` and `Users\ana\node_modules\m.js`, with the folders.
fn include_fixture() -> VolumeIndex {
    let r = root_frn();
    index_from_enum(&[
        create(frn(10), r, "Windows", true),
        create(frn(11), frn(10), "System32", true),
        create(frn(12), frn(11), "a.dll", false),
        create(frn(13), frn(10), "Temp", true),
        create(frn(14), frn(13), "t.txt", false),
        create(frn(20), r, "Users", true),
        create(frn(21), frn(20), "ana", true),
        create(frn(22), frn(21), "doc.txt", false),
        create(frn(23), frn(21), "x.tmp", false),
        create(frn(24), frn(21), "node_modules", true),
        create(frn(25), frn(24), "m.js", false),
    ])
}

fn paths(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|p| p.to_string()).collect()
}

/// The rules remove what they leave out, keep the folder above a checked
/// folder as its path, and count before they apply what applying leaves.
/// The compaction that returns the memory keeps every chain and the scope
/// index exact.
#[test]
fn include_rules_remove_what_they_leave_out() {
    let mut idx = include_fixture();
    let rules = include_rules(
        true,
        &[("Windows", false), (r"Windows\Temp", true)],
        "*.tmp;node_modules",
    );
    let counted = idx.count_kept(&rules);
    idx.apply_rules(rules);
    idx.optimize_storage();
    assert_eq!(counted, idx.len() as u64);
    assert_eq!(
        path_set(&idx),
        paths(&[
            "Windows",
            r"Windows\Temp",
            r"Windows\Temp\t.txt",
            "Users",
            r"Users\ana",
            r"Users\ana\doc.txt"
        ])
    );
    assert!(idx.dir_index_matches_rebuild());
    assert!(idx.child_chains_match_rebuild());
    assert!(idx.name_chains_match_rebuild());
    assert!(idx.is_left_out(frn(11)) && idx.is_left_out(frn(24)));
}

/// A record inside a left-out folder is dropped and leaves no placeholder;
/// an entry that moves or is renamed to where the rules leave it out leaves
/// the index with what is below it; a folder that moves out of a left-out
/// folder is reported for a walk.
#[test]
fn journal_records_follow_the_include_rules() {
    let mut idx = include_fixture();
    idx.apply_rules(include_rules(true, &[("Windows", false)], "*.tmp"));

    let outcome = idx.apply_batch(&[
        create(frn(30), frn(11), "new.dll", false),
        create(frn(31), frn(11), "Sub", true),
        create(frn(32), frn(31), "deep.txt", false),
        rename(frn(22), frn(21), "doc.tmp", false),
        rename(frn(21), frn(10), "ana", true),
        rename(frn(13), frn(20), "Temp", true),
    ]);

    assert_eq!(idx.stats().placeholders_created, 0);
    assert!(!idx.has_orphans());
    assert_eq!(path_set(&idx), paths(&["Users", r"Users\Temp"]));
    assert_eq!(outcome.needs_walk, vec![frn(13)]);
    assert!(idx.is_left_out(frn(31)) && idx.is_left_out(frn(21)));
}

/// An entry that Exclude hidden left out comes back when it is no longer
/// hidden, through a record that neither creates nor renames it.
#[test]
fn an_entry_that_is_no_longer_hidden_comes_back() {
    let r = root_frn();
    let mut idx = index_from_enum(&[ParsedUsnRecord {
        attributes: 0x2 | FILE_ATTRIBUTE_DIRECTORY,
        ..create(frn(10), r, "secret", true)
    }]);
    let settings = IndexSettings {
        exclude_hidden: true,
        ..IndexSettings::default()
    };
    idx.apply_rules(Arc::new(settings.rules_for("V")));
    assert!(path_set(&idx).is_empty());

    let shown = ParsedUsnRecord {
        attributes: FILE_ATTRIBUTE_DIRECTORY,
        ..record(frn(10), r, "secret", true, USN_REASON_BASIC_INFO_CHANGE)
    };
    let outcome = idx.apply_batch(&[shown]);

    assert_eq!(path_set(&idx), paths(&["secret"]));
    assert_eq!(outcome.needs_walk, vec![frn(10)]);
}

/// A file with two names that becomes hidden leaves the index under
/// Exclude hidden: the link reconcile judges its names by the new
/// attributes, not the ones the build stored.
#[test]
fn a_hard_linked_file_that_becomes_hidden_leaves_the_index() {
    let r = root_frn();
    let mut idx = index_from_enum(&[
        create(frn(10), r, "dirA", true),
        create(frn(11), r, "dirB", true),
        create(frn(100), frn(10), "only", false),
    ]);
    let names = [
        LinkTarget {
            parent: idx.head_of(frn(10)).unwrap(),
            name: b"only".to_vec(),
        },
        LinkTarget {
            parent: idx.head_of(frn(11)).unwrap(),
            name: b"second".to_vec(),
        },
    ];
    idx.reconcile_links(frn(100), &names);
    let settings = IndexSettings {
        exclude_hidden: true,
        ..IndexSettings::default()
    };
    idx.apply_rules(Arc::new(settings.rules_for("V")));
    assert_eq!(
        path_set(&idx),
        paths(&["dirA", "dirB", "dirA\\only", "dirB\\second"])
    );

    let hidden = ParsedUsnRecord {
        attributes: 0x2 | 0x20,
        ..record(
            frn(100),
            frn(10),
            "only",
            false,
            USN_REASON_BASIC_INFO_CHANGE,
        )
    };
    let outcome = idx.apply_batch(&[hidden]);
    assert_eq!(outcome.needs_link_reconcile, vec![frn(100)]);
    idx.reconcile_links(frn(100), &names);

    assert_eq!(path_set(&idx), paths(&["dirA", "dirB"]));
}

proptest! {
    /// Removing what the rules leave out and compacting the entry table
    /// keep every chain, the scope index and the folder sizes exact, after
    /// any history of index changes, and the count beforehand is what stays.
    #[test]
    fn applying_rules_keeps_the_index_exact(
        changes in prop::collection::vec(index_change_strategy(), 0..60),
    ) {
        let mut idx = new_index();
        for change in &changes {
            apply_change(&mut idx, change);
        }
        idx.compute_folder_sizes();
        let rules = include_rules(true, &[], "l1*;l2?");
        let counted = idx.count_kept(&rules);
        idx.apply_rules(rules);
        idx.optimize_storage();
        prop_assert_eq!(counted, idx.len() as u64);
        prop_assert!(idx.dir_index_matches_rebuild());
        prop_assert!(idx.child_chains_match_rebuild());
        prop_assert!(idx.name_chains_match_rebuild());
        prop_assert!(idx.folder_sizes_match_rebuild());
    }
}

// -- name ranks -------------------------------------------------------------

/// A rename that keeps an entry's name keeps its name id, and with it the
/// name's rank. Releasing first would kill a unique name and intern an
/// unranked copy of it, so moves alone would make the ranks go stale.
#[test]
fn a_move_keeps_the_name_id_of_a_uniquely_named_file() {
    let mut idx = index_from_enum(&[
        create(frn(100), root_frn(), "a", true),
        create(frn(101), root_frn(), "b", true),
        create(frn(102), frn(100), "unique.txt", false),
    ]);
    let id = idx.name_id(idx.head_of(frn(102)).unwrap());
    idx.apply_batch(&[rename(frn(102), frn(101), "unique.txt", false)]);
    assert_eq!(idx.name_id(idx.head_of(frn(102)).unwrap()), id);
    assert_eq!(
        path_of_frn(&idx, frn(102)).as_deref(),
        Some(r"b\unique.txt")
    );
}

/// One change to a name store in [`name_keys_follow_the_folded_byte_order`].
#[derive(Clone, Debug)]
enum NameChange {
    Intern(String),
    /// Drops one reference to a name.
    Release(prop::sample::Index),
    Merge,
    Compact,
    /// A merge whose ranks the compaction then moves to the new ids, as
    /// [`VolumeIndex::plan_compaction`] does when both are due.
    MergeAndCompact,
}

fn name_change_strategy() -> impl Strategy<Value = NameChange> {
    prop_oneof![
        3 => "[aAbB~ ]{0,3}".prop_map(NameChange::Intern),
        2 => any::<prop::sample::Index>().prop_map(NameChange::Release),
        1 => Just(NameChange::Merge),
        1 => Just(NameChange::Compact),
        1 => Just(NameChange::MergeAndCompact),
    ]
}

proptest! {
    /// The name keys order names as their folded bytes compare, after names
    /// are added and released, with merges and compactions between, or a
    /// sort by name prints rows out of the result order. A name added since
    /// the last merge may share its key with its neighbors, which the tie
    /// runs then order; after a merge every name is ranked, and the keys
    /// order the names exactly as their folded bytes do. `~` sorts after
    /// every letter and the space before them, and `A` folds to `a`.
    #[test]
    fn name_keys_follow_the_folded_byte_order(
        first in prop::collection::vec("[aAbB~ ]{0,3}", 0..30),
        changes in prop::collection::vec(name_change_strategy(), 0..40),
    ) {
        let mut names = store::NameStore::new();
        // One id for each reference held.
        let mut held: Vec<store::NameId> =
            first.iter().map(|n| names.intern(n.as_bytes())).collect();
        names.install_ranks(names.plan_ranks());
        for change in &changes {
            let mut exact = false;
            match change {
                NameChange::Intern(name) => held.push(names.intern(name.as_bytes())),
                NameChange::Release(at) if !held.is_empty() => {
                    names.release(held.swap_remove(at.index(held.len())));
                }
                NameChange::Release(_) => {}
                NameChange::Merge => {
                    names.install_ranks(names.plan_ranks());
                    exact = true;
                }
                NameChange::Compact => {
                    let remap = names.install_compacted(names.plan_compact(names.ranks()));
                    for id in &mut held {
                        *id = remap[*id as usize];
                    }
                }
                NameChange::MergeAndCompact => {
                    let ranks = names.plan_ranks();
                    let remap = names.install_compacted(names.plan_compact(&ranks));
                    for id in &mut held {
                        *id = remap[*id as usize];
                    }
                    exact = true;
                }
            }
            for &a in &held {
                for &b in &held {
                    let folded = names.folded_bytes_of(a).cmp(names.folded_bytes_of(b));
                    let keys = names.name_key(a).cmp(&names.name_key(b));
                    if exact {
                        prop_assert_eq!(keys, folded);
                    } else {
                        prop_assert!(
                            keys == folded || keys == core::cmp::Ordering::Equal,
                            "{:?} against {:?}",
                            names.raw_bytes(a),
                            names.raw_bytes(b),
                        );
                        prop_assert!(folded != core::cmp::Ordering::Equal || keys == folded);
                    }
                }
            }
        }
    }
}
