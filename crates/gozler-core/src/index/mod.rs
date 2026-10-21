//! The in-memory volume index: entry storage, the FRN map, USN apply rules,
//! path reconstruction, and hard-link reconciliation.
//!
//! `store` holds the mechanical primitives (name arena, struct-of-arrays entry
//! table, FRN map) and is crate-private, since reaching them directly bypasses
//! every invariant the policy layer maintains; [`volume`] holds that policy
//! ([`VolumeIndex`]). See `volume` for the invariants (O(1) directory rename,
//! idempotent apply, orphan placeholders, slot-reuse safety). [`settle`] holds
//! the folder walk and brings the index up to date after each journal batch.

// `pub(crate)`, not `pub`: `NameStore::release` and `EntryTable::set_parent`
// reach past every invariant `volume` maintains. Not `mod`, which
// query::engine's tests (not a descendant of `index`) would fail to reach.
// The `pub use` below keeps the types of the index's vocabulary public.
pub(crate) mod store;
pub mod volume;

pub mod settle;

pub use store::{ComponentBytes, IdWidth};
pub use volume::{
    Anomaly, ApplyOutcome, EntryView, IndexMemory, IndexStats, LinkTarget, NTFS_ROOT_FRN,
    PathStatus, VolumeIndex,
};

#[cfg(test)]
mod tests;
