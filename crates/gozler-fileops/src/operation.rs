//! What the window asks the queue for, and what it reads back.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// An operation the window asks for. Every path is a full path: the active
/// panel's selection, or Search's selection, which is the files of one
/// moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Deletes the entries permanently.
    Delete { sources: Vec<PathBuf> },
}

/// Names an operation in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub(crate) u64);

/// An operation's state, as the window reads it.
#[derive(Debug, Clone)]
pub enum State {
    Preparing,
    /// Waiting for the person's answer.
    Prepared {
        /// The selected entries the operation acts on.
        entries: usize,
    },
    Running {
        /// Items done so far: for a delete, the items deleted.
        items: u64,
    },
    /// Stays until the window removes it.
    Ended(Arc<Outcome>),
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Complete,
    Cancelled,
}

/// What an operation refuses at prepare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the destination is inside a selected folder")]
    DestinationInsideSource,
    #[error("the destination does not exist")]
    DestinationMissing,
    #[error("the destination cannot be read: Windows error {0}")]
    DestinationUnreadable(u32),
}

/// An entry the operation could not act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: PathBuf,
    /// Windows' error code.
    pub error: u32,
}

/// What an ended operation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub end: End,
    /// Items done: for a delete, the items deleted, a selected entry that
    /// no longer existed included.
    pub items: u64,
    /// In the order they happened.
    pub failures: Vec<Failure>,
}

/// A running operation's counters, which the window reads about twice a
/// second.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    items: AtomicU64,
}

impl Progress {
    pub(crate) fn add_items(&self, count: u64) {
        self.items.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn items(&self) -> u64 {
        self.items.load(Ordering::Relaxed)
    }
}
