//! Wakes the main window from a request thread (DESIGN.md: window rule 5).

use super::Hwnd;
use std::sync::{Arc, Mutex, PoisonError};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The message a [`Waker`] posts; the window then drains the reply channel.
pub(crate) const WAKE: u32 = WM_APP;

/// Posts its message, [`WAKE`] unless it was made with another, to the main
/// window. It carries no data: the reply travels on the channel.
#[derive(Clone)]
pub(crate) struct Waker {
    window: isize,
    message: u32,
}

// SAFETY: PostMessageW may be called from any thread. In WM_DESTROY, while
// its handle is valid, the window joins every request thread, and stops the
// threads it does not join from waking it under a lock, so the handle never
// names a destroyed or reused window.
unsafe impl Send for Waker {}

impl Waker {
    pub(crate) fn new(window: Hwnd) -> Waker {
        Waker::posting(window, WAKE)
    }

    /// A waker that posts `message`, for a source of wakes that the window
    /// reads apart from the reply channel.
    pub(crate) fn posting(window: Hwnd, message: u32) -> Waker {
        Waker {
            window: window.0.0 as isize,
            message,
        }
    }

    pub(crate) fn wake(&self) {
        // SAFETY: the message carries no pointer; see the Send impl for why
        // the handle is valid.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut _)),
                self.message,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
}

/// A [`Waker`] for the threads the window does not join, because a WinHTTP
/// call cannot be ended at once and joining it would hold Exit for as long
/// as the network waits: the update check, the release's age, a fix's
/// download, and the file operations' queue, which is never dropped. It
/// wakes the window only while the window is open, and
/// [`OpenWaker::close`], which `WM_DESTROY` calls while the handle is still
/// valid, stops every copy under the lock, so no wake-up reaches a reused
/// handle (DESIGN.md: window rule 5).
#[derive(Clone)]
pub(crate) struct OpenWaker {
    waker: Waker,
    open: Arc<Mutex<bool>>,
}

impl OpenWaker {
    #[cfg(test)]
    pub(crate) fn closed() -> Self {
        let waker = Self::new(Hwnd(HWND::default()));
        waker.close();
        waker
    }

    pub(crate) fn new(window: Hwnd) -> OpenWaker {
        OpenWaker::posting(window, WAKE)
    }

    /// An open waker that posts `message` ([`Waker::posting`]).
    pub(crate) fn posting(window: Hwnd, message: u32) -> OpenWaker {
        OpenWaker {
            waker: Waker::posting(window, message),
            open: Arc::new(Mutex::new(true)),
        }
    }

    /// A different message on the same delivery lifetime; closing either closes both.
    pub(crate) fn for_message(&self, message: u32) -> Self {
        Self {
            waker: Waker {
                message,
                ..self.waker.clone()
            },
            open: Arc::clone(&self.open),
        }
    }

    /// Runs `deliver`, which leaves a result for the window, and wakes the
    /// window, while the window is open. `deliver` returns whether it left
    /// one.
    pub(crate) fn deliver(&self, deliver: impl FnOnce() -> bool) {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if *open && deliver() {
            self.waker.wake();
        }
    }

    pub(crate) fn close(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }
}
