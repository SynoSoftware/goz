//! The threads of a running operation, so that a cancel ends a call
//! blocked on a share that stopped answering, as `gozler_winfs`'s
//! `cancel_synchronous_io` does for gozler's other blocked calls. The call
//! then fails with `ERROR_OPERATION_ABORTED`, which the cancelled run does
//! not list.

use std::sync::Mutex;

use gozler_winfs::{ThreadIoHandle, cancel_synchronous_io, current_thread_io_handle};

#[derive(Debug, Default)]
pub(crate) struct IoThreads(Mutex<Vec<ThreadIoHandle>>);

impl IoThreads {
    /// Registers the calling thread until the returned guard drops. A thread
    /// whose handle cannot be read runs unregistered, and a cancel then
    /// waits for its call.
    pub(crate) fn enter(&self) -> Entered<'_> {
        let thread = current_thread_io_handle().ok();
        let id = thread.as_ref().map(|thread| thread.thread_id);
        if let Some(thread) = thread {
            self.0.lock().expect("no worker panics").push(thread);
        }
        Entered { threads: self, id }
    }

    /// Ends the synchronous call each registered thread is blocked in.
    pub(crate) fn cancel(&self) {
        for thread in self.0.lock().expect("no worker panics").iter() {
            let _ = cancel_synchronous_io(thread);
        }
    }
}

pub(crate) struct Entered<'a> {
    threads: &'a IoThreads,
    id: Option<u32>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            let mut threads = self.threads.0.lock().expect("no worker panics");
            threads.retain(|thread| thread.thread_id != id);
        }
    }
}
