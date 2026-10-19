//! Reason-bitmask normalization: raw USN records into idempotent index ops.
//!
//! Reason bits accumulate per open-close window: every sub-operation
//! appends a record whose `Reason` is the OR of all reasons so far since the
//! open, and the final record adds `USN_REASON_CLOSE`. The same logical
//! event therefore reappears in later records; consumers must apply ops
//! idempotently (upsert / remove-if-present), and all bit tests here are
//! bitwise `&`, never equality.

use smallvec::SmallVec;

use super::record::{
    ParsedUsnRecord, USN_REASON_BASIC_INFO_CHANGE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
};

/// One normalized index operation derived from a USN record. It applies to
/// the record it came from, which carries the file ID, name and parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsnOp {
    /// Create or update the file's entry: the record's name and parent are
    /// the file's current state (emitted for `FILE_CREATE` and for
    /// `RENAME_NEW_NAME`, which is authoritative and self-sufficient).
    Upsert,
    /// The file record died (last hard link gone): remove every entry for
    /// the file.
    Delete,
    /// A hard link was added or removed. The record may name a DEAD link, so
    /// the index must never upsert from it.
    ///
    /// Reconciling the file's actual link set needs an external link-set walk
    /// fed to [`crate::index::VolumeIndex::reconcile_links`]. The daemon does
    /// that walk (`gozler_winfs::file_links`) and applies the result, so names
    /// added or removed by a hard-link change are reconciled live.
    LinkDirty,
    /// Size and/or timestamps may have changed; USN records carry neither,
    /// so the entry needs an external re-stat.
    StatDirty,
}

/// Normalizes one record's accumulated reason bitmask into index ops.
///
/// Decision table (rows evaluated top to bottom; rows 2-4 are cumulative,
/// each `&`-test independently appends its op):
///
/// | # | Condition (bitwise `&`)                                              | Effect                              |
/// |---|----------------------------------------------------------------------|-------------------------------------|
/// | 1 | `FILE_DELETE`                                                        | `[Delete]` and NOTHING else: delete wins over all accumulated bits |
/// | 2 | `RENAME_NEW_NAME` or `FILE_CREATE`                                   | push `Upsert` (record name/parent are current state; no OLD/NEW pairing state machine exists by design) |
/// | 3 | `HARD_LINK_CHANGE`                                                   | push `LinkDirty` (never upsert: the named link may be dead) |
/// | 4 | `FILE_CREATE`/`DATA_OVERWRITE`/`DATA_EXTEND`/`DATA_TRUNCATION`/`BASIC_INFO_CHANGE` | push `StatDirty` (a USN record carries no size/mtime, so a create needs one too) |
/// | 5 | none of the above (e.g. CLOSE-only, security-only, `RENAME_OLD_NAME` alone: the NEW record carries the committed state) | empty |
pub fn ops_for(rec: &ParsedUsnRecord) -> SmallVec<[UsnOp; 3]> {
    let mut ops = SmallVec::new();
    let reason = rec.reason;

    // Row 1.
    if reason & USN_REASON_FILE_DELETE != 0 {
        ops.push(UsnOp::Delete);
        return ops;
    }

    // Row 2.
    if reason & (USN_REASON_RENAME_NEW_NAME | USN_REASON_FILE_CREATE) != 0 {
        ops.push(UsnOp::Upsert);
    }

    // Row 3.
    if reason & USN_REASON_HARD_LINK_CHANGE != 0 {
        ops.push(UsnOp::LinkDirty);
    }

    // Row 4. FILE_CREATE is here as well as in row 2: a USN record carries no
    // size or timestamp, so a newly created file has no metadata anywhere until
    // something stats it. A file created and never written emits CREATE with no
    // DATA_* bit ever following, so without this its size stays unknown for the
    // life of the index, and the es CSV contract renders an empty Size column as
    // a folder: a real file reported to the consumer as a directory.
    if reason
        & (USN_REASON_FILE_CREATE
            | USN_REASON_DATA_OVERWRITE
            | USN_REASON_DATA_EXTEND
            | USN_REASON_DATA_TRUNCATION
            | USN_REASON_BASIC_INFO_CHANGE)
        != 0
    {
        ops.push(UsnOp::StatDirty);
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Frn;
    use crate::usn::record::{USN_REASON_CLOSE, USN_REASON_RENAME_OLD_NAME};

    /// Reason bit not in any of our decision-table sets
    /// (`USN_REASON_SECURITY_CHANGE`).
    const SECURITY_ONLY: u32 = 0x0000_0800;

    fn rec(reason: u32) -> ParsedUsnRecord {
        ParsedUsnRecord {
            frn: Frn(7),
            parent_frn: Frn(3),
            reason,
            attributes: 0x20,
            name: b"a.txt".to_vec(),
        }
    }

    /// A create carries no size or mtime, so it must be stat-dirty as well as an
    /// upsert.
    #[test]
    fn create_emits_upsert_and_stat() {
        let r = rec(USN_REASON_FILE_CREATE);
        assert_eq!(
            ops_for(&r).into_vec(),
            vec![UsnOp::Upsert, UsnOp::StatDirty]
        );
    }

    /// Row 4 includes the CREATE bit because a file created and never written
    /// emits CREATE (plus CLOSE) and no DATA_* bit will ever follow.
    #[test]
    fn created_but_never_written_file_is_still_stat_dirty() {
        let r = rec(USN_REASON_FILE_CREATE | USN_REASON_CLOSE);
        assert!(
            ops_for(&r).contains(&UsnOp::StatDirty),
            "a create with no DATA_* bit must still schedule a stat, or the size stays unknown"
        );
    }

    #[test]
    fn rename_new_emits_upsert() {
        let r = rec(USN_REASON_RENAME_NEW_NAME);
        assert_eq!(ops_for(&r).into_vec(), vec![UsnOp::Upsert]);
    }

    #[test]
    fn accumulated_create_data_close_emits_upsert_and_stat() {
        let r = rec(USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND | USN_REASON_CLOSE);
        assert_eq!(
            ops_for(&r).into_vec(),
            vec![UsnOp::Upsert, UsnOp::StatDirty]
        );
    }

    /// The OLD record names the file's previous name and parent; upserting
    /// from it would move the entry back until the NEW record arrives.
    #[test]
    fn rename_old_alone_emits_nothing() {
        let r = rec(USN_REASON_RENAME_OLD_NAME);
        assert!(ops_for(&r).is_empty());
    }

    #[test]
    fn rename_old_and_new_in_one_record_emits_upsert_only() {
        let r = rec(USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME);
        assert_eq!(ops_for(&r).into_vec(), vec![UsnOp::Upsert]);
    }

    #[test]
    fn delete_wins_over_all_accumulated_bits() {
        let r = rec(USN_REASON_FILE_DELETE
            | USN_REASON_FILE_CREATE
            | USN_REASON_RENAME_OLD_NAME
            | USN_REASON_HARD_LINK_CHANGE
            | USN_REASON_DATA_EXTEND
            | USN_REASON_CLOSE);
        assert_eq!(ops_for(&r).into_vec(), vec![UsnOp::Delete]);
    }

    #[test]
    fn hard_link_change_emits_link_dirty_never_upsert() {
        let r = rec(USN_REASON_HARD_LINK_CHANGE | USN_REASON_CLOSE);
        assert_eq!(ops_for(&r).into_vec(), vec![UsnOp::LinkDirty]);
    }

    #[test]
    fn each_stat_reason_emits_stat_dirty() {
        for reason in [
            USN_REASON_DATA_OVERWRITE,
            USN_REASON_DATA_EXTEND,
            USN_REASON_DATA_TRUNCATION,
            USN_REASON_BASIC_INFO_CHANGE,
        ] {
            let r = rec(reason);
            assert_eq!(
                ops_for(&r).into_vec(),
                vec![UsnOp::StatDirty],
                "reason {reason:#x}"
            );
        }
    }

    #[test]
    fn close_only_emits_nothing() {
        let r = rec(USN_REASON_CLOSE);
        assert!(ops_for(&r).is_empty());
    }

    #[test]
    fn security_only_emits_nothing() {
        let r = rec(SECURITY_ONLY);
        assert!(ops_for(&r).is_empty());
    }

    #[test]
    fn unrelated_bits_do_not_mask_matches() {
        // & semantics: extra accumulated bits never turn a match off.
        let r = rec(USN_REASON_FILE_CREATE | SECURITY_ONLY | USN_REASON_CLOSE);
        assert_eq!(
            ops_for(&r).into_vec(),
            vec![UsnOp::Upsert, UsnOp::StatDirty]
        );
    }

    #[test]
    fn full_accumulation_without_delete_emits_all_applicable_ops() {
        let r = rec(USN_REASON_RENAME_NEW_NAME
            | USN_REASON_HARD_LINK_CHANGE
            | USN_REASON_BASIC_INFO_CHANGE
            | USN_REASON_CLOSE);
        assert_eq!(
            ops_for(&r).into_vec(),
            vec![UsnOp::Upsert, UsnOp::LinkDirty, UsnOp::StatDirty]
        );
    }
}
