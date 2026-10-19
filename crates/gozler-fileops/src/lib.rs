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
//! The decisions (names, identities, retries, the speed settings) build
//! and are tested on every OS. The disk work is Windows only, and every
//! Win32 call goes through `gozler-winfs`, so this crate holds no `unsafe`.
//!
//! The design and its evidence are in `docs/plans/fast-copy-delete-spec.md`
//! and `docs/research/fast-copy-delete.md`.

#![forbid(unsafe_code)]
// The decisions are called only by the Windows disk work.
#![cfg_attr(not(windows), allow(dead_code))]

mod delete;
mod identity;
// Stage B's copy names its files with these.
#[cfg_attr(not(test), allow(dead_code))]
mod names;
mod operation;
#[cfg(windows)]
mod queue;
#[cfg(windows)]
mod receive;
mod retry;
mod settings;
mod win32;

pub use operation::{End, Failure, OperationId, Outcome, Refusal, Request, State};
#[cfg(windows)]
pub use queue::{NotPrepared, Queue};
pub use settings::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, RingBlock, RingBuffer, RingHardDisk,
    RingSsd, SettingError, SpeedSettings,
};

#[cfg(all(test, windows))]
mod tests;
