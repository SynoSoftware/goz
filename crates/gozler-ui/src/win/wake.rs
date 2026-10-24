//! Wakes the main window from a request thread (DESIGN.md: window rule 5).

use super::Hwnd;
use std::sync::{Arc, Mutex, PoisonError};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The message an [`OpenWaker`] posts; the window then drains the reply channel.
pub(crate) const WAKE: u32 = WM_APP;

/// A delivery lifetime shared by workers and the window. Closing it before
/// HWND destruction prevents an admitted reply from reaching a reused handle.
#[derive(Clone)]
pub(crate) struct OpenWaker {
    window: isize,
    message: u32,
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

    /// Posts a pointer-free message. The owner must close this lifetime while
    /// its window is still valid, before destroying the HWND.
    pub(crate) fn posting(window: Hwnd, message: u32) -> OpenWaker {
        OpenWaker {
            window: window.0.0 as isize,
            message,
            open: Arc::new(Mutex::new(true)),
        }
    }

    /// A different message on the same delivery lifetime; closing either closes both.
    pub(crate) fn for_message(&self, message: u32) -> Self {
        Self {
            message,
            ..self.clone()
        }
    }

    /// Runs `deliver`, which leaves a result for the window, and wakes the
    /// window, while the window is open. `deliver` returns whether it left
    /// one.
    pub(crate) fn deliver(&self, deliver: impl FnOnce() -> bool) {
        self.while_open(|| {
            if deliver() {
                // SAFETY: the delivery gate closes before HWND destruction;
                // this cross-thread post carries no pointer and never waits.
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(self.window as *mut _)),
                        self.message,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            }
        });
    }

    /// Runs a bounded, nonblocking action before this lifetime closes. The
    /// action must not reenter the UI, because `close` waits for this gate.
    pub(crate) fn while_open(&self, action: impl FnOnce()) -> bool {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if !*open {
            return false;
        }
        action();
        true
    }

    pub(crate) fn close(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }
}
