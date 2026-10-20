//! What the window asks the queue for, and what it reads back.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// An operation the window asks for. Every path is a full path: the active
/// panel's selection, or Search's selection, which is the files of one
/// moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Copies the entries into the folder `destination`.
    Copy {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    /// Moves the entries into the folder `destination`: a rename on one
    /// volume, a copy and a delete across volumes.
    Move {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    /// Deletes the entries permanently.
    Delete { sources: Vec<PathBuf> },
    /// Renames the entry `path` to `name` in its folder; an existing name is
    /// refused, never replaced.
    Rename { path: PathBuf, name: OsString },
    /// Creates the folder `path`; an existing name is refused.
    NewFolder { path: PathBuf },
}

/// Names an operation in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub(crate) u64);

/// An operation's state, as the window reads it.
#[derive(Debug, Clone)]
pub enum State {
    Preparing,
    /// Waiting for the person's answer.
    Prepared(Prepared),
    /// Submitted, and waiting for an earlier operation that has not ended.
    Waiting {
        on: OperationId,
        reason: Wait,
    },
    Running {
        /// Items done so far: entries copied, moved or deleted, skipped
        /// and failed ones included.
        items: u64,
        /// Bytes copied so far.
        bytes: u64,
    },
    /// Stays until the window removes it.
    Ended(Arc<Outcome>),
    /// Saved by a run that did not end, as after a crash; Continue or
    /// Dismiss it.
    Interrupted(Arc<Request>),
}

/// What prepare found, so the window can ask the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// The selected entries the operation acts on.
    pub entries: usize,
    /// For a copy or a move; a delete starts without a walk, and its
    /// progress has no total.
    pub survey: Option<Survey>,
}

/// The counts of a copy's or a move's prepare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Survey {
    /// Files and folders to copy or move.
    pub items: u64,
    /// Bytes to copy; none for a move by rename.
    pub bytes: u64,
    /// Files at the destination that a source would replace.
    pub conflicts: u64,
    /// Selected entries that share a name with another selected entry.
    pub same_name: u64,
}

/// Why a submitted operation waits (D14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// One of the two writes or deletes a path that the other reads or
    /// writes.
    Paths,
    /// Both move bytes on one disk that seeks.
    Disk,
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Complete,
    Cancelled,
    /// The destination disk filled, which would fail every later file too.
    DiskFull,
    Refused(Refusal),
}

/// What an operation refuses at prepare, or a copy at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the destination is inside a selected folder")]
    DestinationInsideSource,
    #[error("the destination does not exist")]
    DestinationMissing,
    #[error("the destination cannot be read: Windows error {0}")]
    DestinationUnreadable(u32),
    #[error("the copy needs {needed} bytes and the destination has {free} free")]
    TooLittleSpace { needed: u64, free: u64 },
}

/// An entry the result lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// With Windows' error code.
    Failed(u32),
    /// A conflict that the choice skipped, or that nobody was asked about.
    Skipped,
    /// A continued move found the file already at the destination and kept
    /// its source, because equal size and time are a guess.
    SourceKept,
    /// A move kept a source that another program changed or replaced while
    /// it was copied, so its copy may not hold what it holds now.
    SourceChanged,
}

/// What an ended operation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// Items done, as [`State::Running`] counts them; for a delete, a
    /// selected entry that no longer existed included.
    pub items: u64,
    pub bytes: u64,
    /// In the order they happened.
    pub listed: Vec<Listed>,
    pub ring: RingCounts,
}

impl Outcome {
    pub(crate) fn ended(end: End) -> Outcome {
        Outcome {
            end,
            items: 0,
            bytes: 0,
            listed: Vec::new(),
            ring: RingCounts::default(),
        }
    }
}

/// What the ring did not do itself, so a ring or a clone that never runs
/// shows in the result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingCounts {
    /// Files that the ring could not copy, which the Windows copy function
    /// copied again.
    pub to_windows: u64,
    /// Clones that the volume refused, which the ring copied instead.
    pub clones_refused: u64,
}

/// A running operation's counters, which the window reads about twice a
/// second.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    items: AtomicU64,
    bytes: AtomicU64,
}

impl Progress {
    pub(crate) fn add_items(&self, count: u64) {
        self.items.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn add_bytes(&self, count: u64) {
        self.bytes.fetch_add(count, Ordering::Relaxed);
    }

    /// Takes back the bytes of a file that was not copied after all.
    pub(crate) fn remove_bytes(&self, count: u64) {
        self.bytes.fetch_sub(count, Ordering::Relaxed);
    }

    pub(crate) fn items(&self) -> u64 {
        self.items.load(Ordering::Relaxed)
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}
