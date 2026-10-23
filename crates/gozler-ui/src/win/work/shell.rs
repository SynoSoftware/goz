//! A slot's retained shell apartment. Returning from URI activation does not
//! establish that the handler has finished using its invoking apartment.

use super::{Id, OffThread, OpenWaker, Started};
use gozler_client::CancelToken;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, PostThreadMessageW, QS_ALLINPUT, TranslateMessage, WM_APP, WM_QUIT,
};

const INPUT: u32 = WM_APP + 1;

struct Call<I> {
    started: Started<I>,
    cancel: CancelToken,
}

pub(super) struct Thread<I> {
    sender: SyncSender<Call<I>>,
    thread: Arc<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
}

impl<I: Send + 'static> Thread<I> {
    pub(super) fn new<O: Send + 'static>(
        run: fn(I, &crate::win::shell::Session, &OffThread, &CancelToken) -> O,
        replies: SyncSender<(Id, O)>,
        waker: OpenWaker,
    ) -> Self {
        let (sender, calls) = sync_channel::<Call<I>>(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let thread = super::spawn(move |worker| {
            let session = crate::win::shell::Session::new(worker);
            let mut message = MSG::default();
            while !worker_stopped.load(Ordering::Acquire) {
                // SAFETY: this thread owns the queue and local message. Create
                // the queue before checking input, including a pre-start submit.
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    if message.message != WM_QUIT {
                        // SAFETY: PeekMessageW initialized the message.
                        unsafe {
                            let _ = TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                }
                if worker_stopped.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(call) = calls.try_recv() {
                    let output = run(call.started.input, &session, worker, &call.cancel);
                    super::deliver(&replies, &waker, call.started.id, output);
                    continue;
                }
                // SAFETY: no handles are borrowed. Input is sent before INPUT
                // is posted, so arrival between try_recv and this wait wakes it.
                unsafe {
                    MsgWaitForMultipleObjectsEx(None, u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            }
        });
        Self {
            sender,
            thread: Arc::new(thread),
            stopped,
        }
    }

    pub(super) fn submit(&self, started: Started<I>, cancel: CancelToken) -> Arc<JoinHandle<()>> {
        self.sender
            .try_send(Call { started, cancel })
            .unwrap_or_else(|_| panic!("only an admitted invocation reaches the shell apartment"));
        self.wake();
        Arc::clone(&self.thread)
    }
}

impl<I> Thread<I> {
    #[cfg(test)]
    pub(super) fn handle(&self) -> Arc<JoinHandle<()>> {
        Arc::clone(&self.thread)
    }

    fn wake(&self) {
        // SAFETY: the JoinHandle keeps the thread handle valid; INPUT carries
        // no pointers. Before queue creation the first input check suffices.
        unsafe {
            let id = GetThreadId(HANDLE(self.thread.as_raw_handle()));
            let _ = PostThreadMessageW(id, INPUT, WPARAM(0), LPARAM(0));
        }
    }
}

impl<I> Drop for Thread<I> {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.wake();
    }
}
