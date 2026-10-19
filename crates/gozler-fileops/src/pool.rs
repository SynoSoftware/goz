//! The work queue that a walk's workers share: the delete's, and the copy's
//! with its lane for large files.

use std::sync::{Condvar, Mutex};

/// Which threads take an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    Workers,
    /// The one thread that copies large files one at a time.
    Large,
}

pub(crate) struct Pool<'a, W> {
    pending: Mutex<Pending<W>>,
    /// One per lane, so a push wakes one thread that can take it.
    workers_ready: Condvar,
    large_ready: Condvar,
    /// Whether the walk stops: cancelled, or a full disk.
    stopped: &'a (dyn Fn() -> bool + Sync),
}

struct Pending<W> {
    workers: Vec<W>,
    large: Vec<W>,
    /// Threads holding an item.
    busy: usize,
}

impl<'a, W> Pool<'a, W> {
    pub(crate) fn new(items: Vec<W>, stopped: &'a (dyn Fn() -> bool + Sync)) -> Self {
        Pool {
            pending: Mutex::new(Pending {
                workers: items,
                large: Vec::new(),
                busy: 0,
            }),
            workers_ready: Condvar::new(),
            large_ready: Condvar::new(),
            stopped,
        }
    }

    /// The next item of `lane`, the latest queued first, so the walk goes
    /// deep before it goes wide and fewer folders are in flight. `None` when
    /// the walk is done or stopped. Call [`Pool::finish`] when the item is
    /// done.
    pub(crate) fn next(&self, lane: Lane) -> Option<W> {
        let mut pending = self.pending.lock().expect("no worker panics");
        loop {
            let done = pending.workers.is_empty() && pending.large.is_empty() && pending.busy == 0;
            if (self.stopped)() || done {
                self.wake_all();
                return None;
            }
            let item = match lane {
                Lane::Workers => pending.workers.pop(),
                Lane::Large => pending.large.pop(),
            };
            if let Some(item) = item {
                pending.busy += 1;
                return Some(item);
            }
            let ready = match lane {
                Lane::Workers => &self.workers_ready,
                Lane::Large => &self.large_ready,
            };
            pending = ready.wait(pending).expect("no worker panics");
        }
    }

    pub(crate) fn push(&self, item: W, lane: Lane) {
        let mut pending = self.pending.lock().expect("no worker panics");
        match lane {
            Lane::Workers => {
                pending.workers.push(item);
                self.workers_ready.notify_one();
            }
            Lane::Large => {
                pending.large.push(item);
                self.large_ready.notify_one();
            }
        }
    }

    pub(crate) fn finish(&self) {
        let mut pending = self.pending.lock().expect("no worker panics");
        pending.busy -= 1;
        if pending.busy == 0 {
            self.wake_all();
        }
    }

    /// Wakes every thread to see that the walk is done or stopped.
    fn wake_all(&self) {
        self.workers_ready.notify_all();
        self.large_ready.notify_all();
    }
}
