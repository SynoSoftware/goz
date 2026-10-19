//! The filter pane's window: a child of the main window that holds the
//! pane's controls, so they scroll together and are cut off at its edges.
//! It decides nothing. It draws the items of its path list, and passes what
//! its controls tell it, and its scroll bar's requests, to the main window,
//! which handles them as it handles its own controls'.

use super::Hwnd;
use crate::core::layout::Px;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetParent, HMENU, IDC_ARROW, LoadCursorW, RegisterClassExW,
    SB_VERT, SCROLLINFO, SIF_PAGE, SIF_POS, SIF_RANGE, SendMessageW, WM_COMMAND, WM_CTLCOLORSTATIC,
    WM_DRAWITEM, WM_VSCROLL, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_EX_CONTROLPARENT,
    WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("gozler.ui.filters");

/// Registers the pane's window class; once, before the main window opens.
pub(crate) fn register() -> windows::core::Result<()> {
    // SAFETY: a null name asks for the executable's own module.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure),
        hInstance: instance.into(),
        // SAFETY: IDC_ARROW is a predefined cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        // The documented way to name a system colour as a class brush.
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
        lpszClassName: CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is complete.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Creates the pane in `parent`. `Tab` and the access keys reach the
/// pane's controls, as `WS_EX_CONTROLPARENT` lets `IsDialogMessageW` look
/// inside it.
pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Hwnd> {
    // SAFETY: a child window with an ID in place of a menu, as Windows
    // documents it.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            Some(parent.0),
            Some(HMENU(usize::from(id) as *mut _)),
            None,
            None,
        )
    }?;
    Ok(Hwnd(hwnd))
}

impl Hwnd {
    /// Sets the pane's scroll range to `content`, its page to `page` and
    /// its position to `position`. Windows hides the scroll bar while the
    /// page holds the whole range.
    pub(crate) fn set_vertical_scroll(self, content: Px, page: Px, position: Px) {
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content.0 - 1,
            nPage: page.0.max(0) as u32,
            nPos: position.0,
            nTrackPos: 0,
        };
        // SAFETY: `info` is complete and outlives the call.
        unsafe { SetScrollInfo(self.0, SB_VERT, &info, true) };
    }
}

/// Draws the Location list's items, and passes the controls' notifications
/// and colour requests to the main window, and the scroll bar's requests
/// with the pane as their sender, as a scroll bar control sends them.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let forward = |lparam: LPARAM| {
        // SAFETY: the parameters are the ones Windows passed, or the pane's
        // own handle, and the main window decodes them as its own controls'.
        unsafe {
            match GetParent(hwnd) {
                Ok(parent) => SendMessageW(parent, msg, Some(wparam), Some(lparam)),
                Err(_) => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    };
    match msg {
        WM_COMMAND | WM_CTLCOLORSTATIC => forward(lparam),
        WM_VSCROLL => forward(LPARAM(hwnd.0 as isize)),
        // SAFETY: the lparam is the one Windows passed with WM_DRAWITEM.
        WM_DRAWITEM if unsafe { super::controls::draw_path_item(lparam) } => LRESULT(1),
        // SAFETY: the parameters are the ones Windows passed.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
