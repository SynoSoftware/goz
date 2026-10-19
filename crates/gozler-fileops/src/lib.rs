//! gozler-fileops: the copy, move and delete engine of gozler's window.
//!
//! The window talks to one object, the [`Queue`]. It prepares an operation,
//! which reads the selected entries from the disk on a queue thread and
//! ends prepared; the window asks the person, then submits it, and the
//! operation runs on its own thread with its workers. The window never
//! waits for the disk: every call returns at once, and the queue calls the
//! window's wake function after each change of an operation's state.
//!
//! The engine runs in the window's process with the person's token, never
//! in gozlerd, because a pipe request to an elevated service would let any
//! local user delete files as SYSTEM. It never uses the Recycle Bin (owner
//! ruling): every delete is permanent.
//!
//! The engine never acts on a link's target by accident. It opens every
//! selected entry, and every entry its walks meet, without following a
//! reparse point, and reads what the entry is on its own handle; a junction,
//! a symbolic link or a mount point is deleted, never entered. Paths are
//! compared by what they name on disk (`identity`), not by their text.
//!
//! A file being copied is written under a temporary name and renamed when
//! complete, so a file at its final name is always complete, and a name
//! that another program took meanwhile is never replaced without the
//! person's choice.
//!
//! The decisions (names, conflicts, identities, the waits between
//! operations, taken entries, retries, the saved file and the speed
//! settings) build and are tested on every OS. The disk work is Windows only, and every
//! Win32 call goes through `gozler-winfs`, so this crate holds no `unsafe`.
//!
//! The design and its evidence are in `docs/plans/fast-copy-delete-spec.md`
//! and `docs/research/fast-copy-delete.md`.

#![forbid(unsafe_code)]
// The decisions are called only by the Windows disk work.
#![cfg_attr(not(windows), allow(dead_code))]

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
mod saved;
mod settings;
#[cfg(windows)]
mod survey;
mod taken;
#[cfg(windows)]
mod tree;
mod volumes;
mod waits;
mod win32;

pub use conflicts::Choice;
pub use operation::{
    End, Listed, OperationId, Outcome, Prepared, Reason, Refusal, Request, State, Survey, Wait,
};
#[cfg(windows)]
pub use queue::{NotInterrupted, NotPrepared, Queue};
pub use settings::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, Preset, RingBlock, RingBuffer,
    RingHardDisk, RingSsd, SettingError, SpeedSettings,
};
#[cfg(windows)]
pub use survey::{Measured, measure};
pub use taken::Taken;

#[cfg(all(test, windows))]
mod tests;
