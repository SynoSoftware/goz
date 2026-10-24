//! The window's Win32 layer: every `unsafe` block of `gozler-ui` is in this
//! module, each the smallest that needs it and behind a safe function
//! (DESIGN.md: Where the unsafe lives). It decides nothing:
//! `core` decides, and `app` turns messages into calls on both.
//!
//! A handle is a plain value here. A Win32 call on a stale handle fails or
//! does nothing, so a safe wrapper over one is sound.

pub(crate) mod clipboard;
pub(crate) mod controls;
pub(crate) mod dialog;
pub(crate) mod drop_target;
pub(crate) mod file_operations;
pub(crate) mod folder;
pub(crate) mod gdi;
pub(crate) mod instance;
pub(crate) mod keys;
pub(crate) mod locale;
pub(crate) mod menu;
pub(crate) mod notify;
pub(crate) mod pairing;
pub(crate) mod pane;
pub(crate) mod shell;
pub(crate) mod shortcut;
pub(crate) mod status_transfer;
pub(crate) mod toolbar;
pub(crate) mod tray;
pub(crate) mod update;
pub(crate) mod viewer;
pub(crate) mod wake;
pub(crate) mod watchdog;
pub(crate) mod window;
pub(crate) mod work;

use crate::core::layout::{Px, Rect, Size};
use crate::core::settings::Placement;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    InvalidateRect, MapWindowPoints, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE,
    RDW_UPDATENOW, REDRAW_WINDOW_FLAGS, RedrawWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, DestroyWindow, EndDeferWindowPos, GWL_EXSTYLE, GWL_STYLE,
    GetClassNameW, GetClientRect, GetParent, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, IsChild, IsIconic, IsWindowVisible, PostMessageW,
    SET_WINDOW_POS_FLAGS, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWDEFAULT, SW_SHOWMINNOACTIVE,
    SW_SHOWNA, SW_SHOWNORMAL, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageW, SetForegroundWindow, SetWindowPos,
    SetWindowTextW, ShowWindow, UIS_CLEAR, UISF_HIDEFOCUS, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINDOWPLACEMENT, WM_CHANGEUISTATE, WM_SETREDRAW, WPF_RESTORETOMAXIMIZED, WS_VISIBLE,
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
        if self.text() == text {
            return;
        }
        // SAFETY: the text is a NUL-terminated copy that outlives the call.
        let _ = unsafe { SetWindowTextW(self.0, &HSTRING::from(text)) };
    }

    pub(crate) fn text(self) -> String {
        // SAFETY: plain values.
        let length = unsafe { GetWindowTextLengthW(self.0) };
        let mut buffer = vec![0u16; usize::try_from(length).unwrap_or(0) + 1];
        // SAFETY: the call writes at most the buffer's length.
        let copied = unsafe { GetWindowTextW(self.0, &mut buffer) };
        String::from_utf16_lossy(&buffer[..usize::try_from(copied).unwrap_or(0)])
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
        self.position(rect, SWP_NOZORDER | SWP_NOACTIVATE);
    }

    fn position(self, rect: Rect, flags: SET_WINDOW_POS_FLAGS) {
        // SAFETY: plain values; a stale handle only fails.
        let _ = unsafe {
            SetWindowPos(
                self.0,
                None,
                rect.x.0,
                rect.y.0,
                rect.width.0,
                rect.height.0,
                flags,
            )
        };
    }

    /// Shows or hides a control without activating it. A control already
    /// shown or hidden is left alone, because `ShowWindow` takes the window
    /// manager's lock even when nothing changes, and layout calls this for
    /// every control on each resize.
    pub(crate) fn set_visible(self, visible: bool) {
        if self.styles().0.contains(WS_VISIBLE) == visible {
            return;
        }
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, if visible { SW_SHOWNA } else { SW_HIDE }) };
    }

    /// Whether the window already stands at `rect`, in its parent's client
    /// coordinates, for [`move_children`]. A drop-down list's height in
    /// `rect` includes its open list, which the themed combo box sizes
    /// from its items instead, so only its position and width count.
    fn is_placed_at(self, rect: Rect) -> bool {
        let mut class = [0u16; 16];
        // SAFETY: `class` is a local whose length the call is given.
        let length = unsafe { GetClassNameW(self.0, &mut class) };
        let is_combo_box = usize::try_from(length)
            .is_ok_and(|n| String::from_utf16_lossy(&class[..n]).eq_ignore_ascii_case("ComboBox"));
        // SAFETY: plain values.
        let parent = Hwnd(unsafe { GetParent(self.0) }.unwrap_or_default());
        let placed = self.rect_in(parent);
        if is_combo_box {
            (placed.x, placed.y, placed.width) == (rect.x, rect.y, rect.width)
        } else {
            placed == rect
        }
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

    /// Hides a top-level window, as closing it to the notification area
    /// does.
    pub(crate) fn hide(self) {
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, SW_HIDE) };
    }

    /// Shows a new window minimized, without taking the focus from the
    /// program in front, as a program started at sign-in does.
    pub(crate) fn show_minimized(self) {
        // SAFETY: plain values.
        let _ = unsafe { ShowWindow(self.0, SW_SHOWMINNOACTIVE) };
    }

    pub(crate) fn is_visible(self) -> bool {
        // SAFETY: plain values.
        unsafe { IsWindowVisible(self.0) }.as_bool()
    }

    /// Whether the window is on screen: visible and not minimized.
    pub(crate) fn is_on_screen(self) -> bool {
        self.is_visible() && !self.is_minimized()
    }

    pub(crate) fn is_minimized(self) -> bool {
        // SAFETY: plain values.
        unsafe { IsIconic(self.0) }.as_bool()
    }

    /// Whether the window takes input, which a modal dialog it owns stops.
    pub(crate) fn is_enabled(self) -> bool {
        // SAFETY: plain values.
        unsafe { IsWindowEnabled(self.0) }.as_bool()
    }

    /// Asks Windows to put the window in front without showing it, as a
    /// tray icon's menu needs so that it closes on a click elsewhere.
    pub(crate) fn set_foreground(self) {
        // SAFETY: plain values.
        let _ = unsafe { SetForegroundWindow(self.0) };
    }

    /// The control's parent window.
    pub(crate) fn parent(self) -> Hwnd {
        // SAFETY: plain values.
        Hwnd(unsafe { GetParent(self.0) }.unwrap_or_default())
    }

    /// The window's rectangle in the client coordinates of `parent`.
    pub(crate) fn rect_in(self, parent: Hwnd) -> Rect {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a local the calls fill in and convert; it holds
        // the two points MapWindowPoints converts.
        unsafe {
            let _ = GetWindowRect(self.0, &mut rect);
            let points = std::slice::from_raw_parts_mut((&raw mut rect).cast::<POINT>(), 2);
            MapWindowPoints(None, Some(parent.0), points);
        }
        from_rect(rect)
    }

    /// Moves and sizes a child window to `rect`, in its parent's client
    /// coordinates, and puts it after `after` in the Tab order.
    pub(crate) fn place_after(self, after: Hwnd, rect: Rect) {
        // SAFETY: plain values; a stale handle only fails.
        let _ = unsafe {
            SetWindowPos(
                self.0,
                Some(after.0),
                rect.x.0,
                rect.y.0,
                rect.width.0,
                rect.height.0,
                SWP_NOACTIVATE,
            )
        };
    }

    pub(crate) fn destroy(self) {
        // SAFETY: the window procedure keeps its state alive while this
        // delivers WM_DESTROY and WM_NCDESTROY (DESIGN.md: window rule 2).
        let _ = unsafe { DestroyWindow(self.0) };
    }

    pub(crate) fn invalidate(self) {
        // SAFETY: plain values.
        let _ = unsafe { InvalidateRect(Some(self.0), None, true) };
    }

    /// Runs `change` with redraw disabled on this window, then invalidates
    /// its client, frame and descendants for native repainting. Descendants
    /// paint independently. A hidden window only runs `change`, because
    /// `DefWindowProc` answers `WM_SETREDRAW` TRUE by making the window
    /// visible, as a regional-format change would make the main window
    /// while it is hidden in the notification area.
    pub(crate) fn draw_once_after(self, change: impl FnOnce()) {
        self.redraw_after(change, RDW_ERASE);
    }

    /// As [`Self::draw_once_after`], but processes pending native paints
    /// before returning.
    pub(crate) fn draw_now_after(self, change: impl FnOnce()) {
        self.redraw_after(change, RDW_ERASE | RDW_UPDATENOW);
    }

    fn redraw_after(self, change: impl FnOnce(), flags: REDRAW_WINDOW_FLAGS) {
        if !self.styles().0.contains(WS_VISIBLE) {
            change();
            return;
        }
        // SAFETY: plain values.
        unsafe { SendMessageW(self.0, WM_SETREDRAW, Some(WPARAM(0)), None) };
        change();
        // SAFETY: plain values.
        unsafe {
            SendMessageW(self.0, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = RedrawWindow(
                Some(self.0),
                None,
                None,
                RDW_FRAME | RDW_INVALIDATE | RDW_ALLCHILDREN | flags,
            );
        }
    }

    pub(crate) fn focus(self) {
        // SAFETY: plain values.
        let _ = unsafe { SetFocus(Some(self.0)) };
    }

    /// Shows the focus rectangles in this top-level window, as
    /// `IsDialogMessageW` does when the person presses `Tab`.
    pub(crate) fn show_focus_cues(self) {
        let action = (UISF_HIDEFOCUS << 16) | UIS_CLEAR;
        // SAFETY: plain values.
        unsafe {
            SendMessageW(
                self.0,
                WM_CHANGEUISTATE,
                Some(WPARAM(action as usize)),
                None,
            )
        };
    }

    /// Whether `window` is a child of this window, or a child of one.
    pub(crate) fn is_parent_of(self, window: Hwnd) -> bool {
        // SAFETY: plain values.
        unsafe { IsChild(self.0, window.0) }.as_bool()
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

/// Moves and sizes child windows of one parent to their rectangles, in the
/// parent's client coordinates, as one batch that Windows applies in a
/// single pass. A child already at its rectangle is left alone. Each move
/// costs Windows far more than reading where a child is, up to a
/// millisecond while other programs hold the window manager's lock, so a
/// resize that moves every control one at a time falls far behind the
/// mouse.
pub(crate) fn move_children(moves: &[(Hwnd, Rect)]) {
    place_children(moves, &[]);
}

/// Places and shows children in one native batch. All children must have
/// the same parent; a child omitted from `moves` keeps its rectangle.
pub(crate) fn place_children(moves: &[(Hwnd, Rect)], shown: &[(Hwnd, bool)]) {
    let visibility = |child: Hwnd, visible| {
        if child.styles().0.contains(WS_VISIBLE) == visible {
            SET_WINDOW_POS_FLAGS(0)
        } else if visible {
            SWP_SHOWWINDOW
        } else {
            SWP_HIDEWINDOW
        }
    };
    let mut placements = Vec::new();
    for &(child, rect) in moves {
        let moved = !child.is_placed_at(rect);
        let show = shown
            .iter()
            .find(|(hwnd, _)| *hwnd == child)
            .map_or(SET_WINDOW_POS_FLAGS(0), |&(_, visible)| {
                visibility(child, visible)
            });
        if moved || show.0 != 0 {
            // Copied client pixels can contain a sibling's old drawing.
            let geometry = if moved {
                SWP_NOCOPYBITS
            } else {
                SWP_NOMOVE | SWP_NOSIZE
            };
            placements.push((child, rect, geometry | show));
        }
    }
    for &(child, visible) in shown {
        if moves.iter().any(|(hwnd, _)| *hwnd == child) {
            continue;
        }
        let show = visibility(child, visible);
        if show.0 != 0 {
            placements.push((child, Rect::default(), SWP_NOMOVE | SWP_NOSIZE | show));
        }
    }
    if placements.is_empty() {
        return;
    }
    let flags = SWP_NOZORDER | SWP_NOACTIVATE;
    // SAFETY: plain values. Each DeferWindowPos takes the batch handle and
    // returns the next one; EndDeferWindowPos, or a failed DeferWindowPos,
    // frees it.
    let applied = unsafe {
        let mut batch = BeginDeferWindowPos(placements.len() as i32).ok();
        for (child, rect, change) in &placements {
            batch = batch.and_then(|b| {
                DeferWindowPos(
                    b,
                    child.0,
                    None,
                    rect.x.0,
                    rect.y.0,
                    rect.width.0,
                    rect.height.0,
                    flags | *change,
                )
                .ok()
            });
        }
        batch.is_some_and(|b| EndDeferWindowPos(b).is_ok())
    };
    if !applied {
        for (child, rect, change) in placements {
            child.position(rect, flags | change);
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
