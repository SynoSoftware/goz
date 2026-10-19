//! Wakes the main window from a request thread (design rule 5).

use super::Hwnd;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The message a [`Waker`] posts; the window then drains the reply channel.
pub(crate) const WAKE: u32 = WM_APP;

/// Posts [`WAKE`] to the main window. It carries no data: the reply travels
/// on the channel.
#[derive(Clone)]
pub(crate) struct Waker(isize);

// SAFETY: PostMessageW may be called from any thread. The window joins every
// request thread in WM_DESTROY, while its handle is valid, so the handle
// never names a destroyed or reused window.
unsafe impl Send for Waker {}

impl Waker {
    pub(crate) fn new(window: Hwnd) -> Waker {
        Waker(window.0.0 as isize)
    }

    pub(crate) fn wake(&self) {
        // SAFETY: the message carries no pointer; see the Send impl for why
        // the handle is valid.
        let _ = unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WAKE, WPARAM(0), LPARAM(0)) };
    }
}
