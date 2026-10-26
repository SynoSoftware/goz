//! gozler-fileops: the copy, move and delete engine of gozler's window.
//!
//! The window talks to one object, the [`Queue`]. It prepares an operation,
//! which reads the selected entries from the disk on a queue thread, asks
//! gozlerd for their totals instead of walking to count them, and ends
//! prepared; the window submits that operation with its accepted policy.
//! It runs on its own thread with its workers. The window serializes its
//! Queue commands on a worker, because state decisions can acquire engine
//! locks. The queue calls its wake function after each change of state.
//! Synchronization compares complete trees under device admission, releases
//! its grants for preview, and reacquires them after explicit commit. Direct
//! Sync/Mirror commands give that consent before comparison and apply it
//! without a preview, while refusing file/folder conflicts.
//! Mirror verifies required output before deleting unchanged approved extras;
//! recovery always compares again rather than restoring deletion approval.
//!
//! The engine runs in the window's process with the person's token, never
//! in gozlerd, because a pipe request to an elevated service would let any
//! local user delete files as SYSTEM. It never uses the Recycle Bin (owner
//! ruling): every delete is permanent.
//!
//! Walks open entries relative to their parents without following name
//! surrogates. Copy, move and delete handle a selected link as the link
//! itself. Copy output still uses full paths, without locking intermediate
//! names against replacement. Mirror separately verifies required output and
//! approved deletions through relative no-follow opens. Paths are compared by
//! what they name on disk (`identity`), not by their text.
//!
//! A file is copied straight to its final name with a create that refuses
//! an existing name, so a name that another program took meanwhile is never
//! replaced outside the accepted policy. An interrupted copy leaves its file
//! there unfinished, and Continue resumes it.
//!
//! The decisions (names, conflicts, identities, the waits between
//! operations, taken entries, retries, the saved file, the speed settings
//! and the ring's plan) build and are tested on every OS. The disk work is
//! Windows only, and every Win32 call goes through `gozler-winfs`, so this
//! crate holds no `unsafe`.
//!
//! The design and its evidence are in `docs/DESIGN.md`, "The copy, move and delete
//! engine", and `docs/research/fast-copy-delete.md`.

#![forbid(unsafe_code)]
// The decisions are called only by the Windows disk work.
#![cfg_attr(not(windows), allow(dead_code))]
// The docs link the Windows-only queue and walks, which Windows' docs check.
#![cfg_attr(not(windows), allow(rustdoc::broken_intra_doc_links))]

mod conflicts;
#[cfg(windows)]
mod copy;
mod delete;
mod identity;
#[cfg(windows)]
mod io_threads;
#[cfg(windows)]
mod listing;
#[cfg(windows)]
mod moves;
mod names;
mod operation;
#[cfg(windows)]
mod place;
mod pool;
#[cfg(windows)]
mod prepare;
#[cfg(windows)]
mod queue;
#[cfg(windows)]
mod receive;
mod retry;
mod ring_plan;
mod saved;
mod settings;
#[cfg(windows)]
mod survey;
mod sync;
mod taken;
mod totals;
#[cfg(windows)]
mod tree;
mod volumes;
mod waits;
mod win32;

pub use conflicts::Policy;
pub use operation::{
    Current, Destination, End, Listed, NoRoom, Operation, OperationId, OrderRefused, Outcome,
    Prepared, Reason, Refusal, Request, RingCounts, Running, RunningPhase, SourcesFolder, State,
    Total, Wait,
};
#[cfg(windows)]
pub use queue::{NotCompared, NotInterrupted, NotPrepared, Queue, ReservedId};
pub use settings::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, Preset, RingBlock, RingBuffer,
    RingHardDisk, RingSsd, SettingError, SpeedSettings, TransferLimit,
};
#[cfg(windows)]
pub use survey::{Measured, measure};
pub use sync::{Comparison, ComparisonEntry, ComparisonSummary, SyncAction};
pub use taken::{Taken, TakenFolder};

#[cfg(all(test, windows))]
mod tests;
