//! Each Viewer admits one persistent native reader. Reopening replaces its
//! pending command, so a blocked read cannot leave additional readers behind.

use crate::core::clipboard::Publication;
use crate::core::viewer_window::CopyText;
use crate::core::work::{Interest, Replacement, Waiting};
use crate::win::Hwnd;
use crate::win::folder::Register;
use crate::win::viewer::ViewerEvent;
use crate::win::viewer::work::{self, Command, Mailbox, Session};
use crate::win::wake::OpenWaker;
use crate::win::work::{Task, spawn};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy)]
enum Read {
    Open,
    Block,
    Copy,
}

pub(in crate::app) struct Reader {
    task: Task<Session, ()>,
    mailbox: Arc<Mailbox>,
    generation: u64,
    pending: Option<(Read, Instant)>,
    vetoed: bool,
}

impl Reader {
    pub(in crate::app) fn new(waker: OpenWaker) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let mut task = Task::new(waker.clone(), Replacement::Cancel, work::run);
        task.replace(Session {
            mailbox: Arc::clone(&mailbox),
            waker,
        });
        Self {
            task,
            mailbox,
            generation: 0,
            pending: None,
            vetoed: false,
        }
    }

    pub(in crate::app) fn open(&mut self, path: Vec<u16>) {
        self.vetoed = false;
        self.pending = Some((Read::Open, Instant::now()));
        self.command(Command::Open(path), true);
    }

    pub(in crate::app) fn read(&mut self, index: u64) {
        self.pending = Some((Read::Block, Instant::now()));
        self.command(Command::Read(index), false);
    }

    /// Reads the file whole and puts its text on the clipboard. It replaces
    /// a block read in flight, which the page then asks for again.
    pub(in crate::app) fn copy(&mut self, copy: CopyText, publication: Publication) {
        self.pending = Some((Read::Copy, Instant::now()));
        self.command(Command::Copy(copy, publication), false);
    }

    /// Whether a copy runs, which a block read would replace.
    pub(in crate::app) fn copying(&self) -> bool {
        matches!(self.pending, Some((Read::Copy, _)))
    }

    pub(in crate::app) fn take(&mut self) -> Option<ViewerEvent> {
        let event = self.mailbox.take(self.generation)?;
        self.pending = None;
        Some(event)
    }

    pub(in crate::app) fn take_registration(&mut self) -> Option<Register> {
        let request = self.mailbox.take_registration(self.generation)?;
        if let Some((_, since)) = self.pending.as_mut() {
            *since = Instant::now();
        }
        Some(request)
    }

    pub(in crate::app) fn waiting(&self, now: Instant) -> Option<Waiting> {
        let (_, since) = self.pending?;
        Some(Interest::Awaited.after(now.saturating_duration_since(since)))
    }

    pub(in crate::app) fn cancel(&mut self) -> bool {
        let Some((read, _)) = self.pending.take() else {
            return false;
        };
        let command = match read {
            Read::Open => Command::Release {
                keep_registration: false,
            },
            Read::Block | Read::Copy => Command::Cancel,
        };
        self.command(command, true);
        true
    }

    pub(in crate::app) fn is_registration(&self, notification: isize) -> bool {
        self.mailbox.is_registration(notification)
    }

    pub(in crate::app) fn release(&mut self, keep_registration: bool) {
        self.pending = None;
        self.command(Command::Release { keep_registration }, true);
    }

    pub(in crate::app) fn query_remove(&mut self) -> bool {
        self.release(true);
        let allowed = self.mailbox.released();
        self.vetoed = !allowed;
        allowed
    }

    pub(in crate::app) fn removal_failed(&self) -> bool {
        !self.vetoed
    }

    fn command(&mut self, command: Command, interrupt: bool) {
        self.generation = self.generation.wrapping_add(1);
        if interrupt {
            // Reject the old answer without admitting replacement I/O yet.
            self.mailbox.invalidate(self.generation);
            self.task.interrupt_io();
        }
        self.mailbox.post(self.generation, command);
    }
}

pub(in crate::app) fn register(request: Register, window: Hwnd) {
    let retired = request.complete(window);
    if !retired.is_empty() {
        spawn(move |_| drop(retired));
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.mailbox.close();
        self.task.cancel();
    }
}
