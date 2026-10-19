//! The window's Win32 layer: every `unsafe` block of `gozler-ui` is in this
//! module, each the smallest that needs it and behind a safe function
//! (docs/plans/ui-design.md: Where the unsafe lives). It decides nothing:
//! `core` decides, and `app` turns messages into calls on both.
//!
//! A handle is a plain value here. A Win32 call on a stale handle fails or
//! does nothing, so a safe wrapper over one is sound.

pub(crate) mod clipboard;
pub(crate) mod controls;
pub(crate) mod dialog;
pub(crate) mod gdi;
pub(crate) mod instance;
pub(crate) mod locale;
pub(crate) mod menu;
pub(crate) mod notify;
pub(crate) mod shell;
pub(crate) mod wake;
pub(crate) mod window;

use crate::core::layout::{Px, Rect, Size};
use crate::core::settings::Placement;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, IsIconic, PostMessageW, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWDEFAULT, SW_SHOWNA,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, SetWindowTextW,
    ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
};
use windows::core::HSTRING;

/// A window handle that does not own its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hwnd(HWND);

/// A NUL-terminated UTF-16 copy of `text`.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn to_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.x.0,
        top: rect.y.0,
        right: rect.right().0,
        bottom: rect.bottom().0,
    }
}

fn from_rect(rect: RECT) -> Rect {
    Rect {
        x: Px(rect.left),
        y: Px(rect.top),
        width: Px(rect.right - rect.left),
        height: Px(rect.bottom - rect.top),
    }
}

impl Hwnd {
    pub(crate) fn set_text(self, text: &str) {
        // SAFETY: the text is a NUL-terminated copy that outlives the call.
        let _ = unsafe { SetWindowTextW(self.0, &HSTRING::from(text)) };
    }

    pub(crate) fn client_size(self) -> Size {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a local the call fills in.
        let _ = unsafe { GetClientRect(self.0, &mut rect) };
        let rect = from_rect(rect);
        Size {
            width: rect.width,
            height: rect.height,
        }
    }

    /// Moves and sizes the window, in its parent's client coordinates for
    /// a control and in screen coordinates for a top-level window.
    pub(crate) fn set_rect(self, rect: Rect) {
        // SAFETY: plain values; a stale handle only fails.
        let _ = unsafe {
            SetWindowPos(
                self.0,
                None,
                rect.x.0,
                rect.y.0,
                rect.width.0,
                rect.height.0,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    /// Shows or hides a control without activating it.
    pub(crate) fn set_visible(self, visible: bool) {
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, if visible { SW_SHOWNA } else { SW_HIDE }) };
    }

    /// The window's rectangle in screen coordinates.
    pub(crate) fn screen_rect(self) -> Rect {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a local the call fills in.
        let _ = unsafe { GetWindowRect(self.0, &mut rect) };
        from_rect(rect)
    }

    pub(crate) fn show_normal(self) {
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, SW_SHOWNORMAL) };
    }

    /// Shows a new window as the program's start asked, such as a shortcut
    /// set to start minimized.
    pub(crate) fn show_default(self) {
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, SW_SHOWDEFAULT) };
    }

    /// Restores a minimized window, which comes back maximized when it was,
    /// shows a hidden one and asks Windows to put it in front.
    pub(crate) fn bring_to_front(self) {
        // SAFETY: plain values.
        unsafe {
            let command = if IsIconic(self.0).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(self.0, command);
            let _ = SetForegroundWindow(self.0);
        }
    }

    pub(crate) fn destroy(self) {
        // SAFETY: the window procedure keeps its state alive while this
        // delivers WM_DESTROY and WM_NCDESTROY (design rule 2).
        let _ = unsafe { DestroyWindow(self.0) };
    }

    pub(crate) fn invalidate(self) {
        // SAFETY: plain values.
        let _ = unsafe { InvalidateRect(Some(self.0), None, true) };
    }

    pub(crate) fn focus(self) {
        // SAFETY: plain values.
        let _ = unsafe { SetFocus(Some(self.0)) };
    }

    /// The control that has the keyboard focus on this thread, if any.
    pub(crate) fn focused() -> Option<Hwnd> {
        // SAFETY: no parameters.
        let hwnd = unsafe { GetFocus() };
        (!hwnd.is_invalid()).then_some(Hwnd(hwnd))
    }

    /// Posts `message` with no parameters.
    pub(crate) fn post(self, message: u32) {
        // SAFETY: the message carries no pointer.
        let _ = unsafe { PostMessageW(Some(self.0), message, WPARAM(0), LPARAM(0)) };
    }

    /// The restored rectangle and whether the window is or returns
    /// maximized, as `GetWindowPlacement` gives them.
    pub(crate) fn placement(self) -> Option<Placement> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: `placement` is a local with its length set.
        unsafe { GetWindowPlacement(self.0, &mut placement) }.ok()?;
        let normal = placement.rcNormalPosition;
        let maximized = placement.showCmd
            == windows::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED.0 as u32
            || (placement.showCmd
                == windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINIMIZED.0 as u32
                && placement.flags.contains(WPF_RESTORETOMAXIMIZED));
        Some(Placement {
            left: normal.left,
            top: normal.top,
            right: normal.right,
            bottom: normal.bottom,
            maximized,
        })
    }

    fn styles(self) -> (WINDOW_STYLE, WINDOW_EX_STYLE) {
        // SAFETY: plain values.
        unsafe {
            (
                WINDOW_STYLE(GetWindowLongPtrW(self.0, GWL_STYLE) as u32),
                WINDOW_EX_STYLE(GetWindowLongPtrW(self.0, GWL_EXSTYLE) as u32),
            )
        }
    }
}

/// Windows' own text for an operating-system error, in the Windows display
/// language, such as "Access is denied."
pub(crate) fn error_text(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => {
            windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code as u32))
                .message()
                .trim()
                .to_owned()
        }
        None => error.to_string(),
    }
}
