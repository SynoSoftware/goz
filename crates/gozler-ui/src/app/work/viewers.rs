//! Each Viewer admits one persistent native reader. Reopening replaces its
//! pending command, so a blocked read cannot leave additional readers behind.

use crate::core::work::Replacement;
use crate::win::Hwnd;
use crate::win::folder::Register;
use crate::win::viewer::ViewerEvent;
use crate::win::viewer::work::{self, Command, Mailbox, Session};
use crate::win::wake::OpenWaker;
use crate::win::work::{Task, spawn};
use std::sync::Arc;

pub(in crate::app) struct Reader {
    task: Task<Session, ()>,
    mailbox: Arc<Mailbox>,
    generation: u64,
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
        }
    }

    pub(in crate::app) fn open(&mut self, path: Vec<u16>) {
        self.command(Command::Open(path), true);
    }

    pub(in crate::app) fn read(&mut self, index: u64) {
        self.command(Command::Read(index), false);
    }

    pub(in crate::app) fn take(&mut self) -> Option<ViewerEvent> {
        self.mailbox.take(self.generation)
    }

    pub(in crate::app) fn take_registration(&self) -> Option<Register> {
        self.mailbox.take_registration(self.generation)
    }

    pub(in crate::app) fn is_registration(&self, notification: isize) -> bool {
        self.mailbox.is_registration(notification)
    }

    pub(in crate::app) fn release(&mut self, keep_registration: bool) {
        self.command(Command::Release { keep_registration }, true);
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
