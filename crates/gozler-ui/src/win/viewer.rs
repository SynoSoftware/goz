//! The Viewer's native area. Its reader in [`work`] owns the file and its
//! notification lifetime, handing registration to the receiving window thread.

use super::Hwnd;
use super::gdi::{Font, muted_text, system_color};
use super::window::offer_key;
use crate::core::layout::{Dpi, Px};
use crate::core::viewer_window::ScrollBar;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, COLOR_WINDOWTEXT, ETO_CLIPPED, ETO_OPAQUE, EndPaint, ExtTextOutW,
    FillRect, GetSysColorBrush, HBRUSH, HGDIOBJ, PAINTSTRUCT, SelectObject, SetBkColor,
    SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DLGC_WANTARROWS, DLGC_WANTCHARS, DefWindowProcW, GetParent, HMENU, IDC_ARROW,
    LoadCursorW, RegisterClassExW, SB_VERT, SCROLLINFO, SIF_DISABLENOSCROLL, SIF_PAGE, SIF_POS,
    SIF_RANGE, SM_CYCAPTION, SendMessageW, WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_PAINT, WM_VSCROLL, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LEFT,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

pub(crate) mod work;

const AREA_CLASS: PCWSTR = w!("gozler.ui.viewer_area");

/// The private message with which the area asks its Viewer to paint it;
/// the Viewer answers 1 when it painted.
pub(crate) const AREA_PAINT: u32 = super::wake::WAKE + 9;

/// Registers the area's window class; once, before the first Viewer opens.
pub(crate) fn register_area() -> windows::core::Result<()> {
    // SAFETY: a null name asks for the executable's own module.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(area_procedure),
        hInstance: instance.into(),
        // SAFETY: IDC_ARROW is a predefined cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        // The documented way to name a system colour as a class brush.
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
        lpszClassName: AREA_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is complete.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Creates the area in `parent`. `Tab` reaches it, and it keeps the arrow
/// keys, which `IsDialogMessageW` would otherwise take. It clips the
/// controls created before it, which lie above it, such as the message.
pub(crate) fn create_area(parent: Hwnd, id: u16) -> windows::core::Result<Hwnd> {
    // SAFETY: a child window with an ID in place of a menu, as Windows
    // documents it.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LEFT,
            AREA_CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP | WS_CLIPSIBLINGS,
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

/// Has the Viewer paint the area, and passes the scroll bar's requests to
/// it with the area as their sender, as a scroll bar control sends them. A
/// click gives the area the focus.
unsafe extern "system" fn area_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: plain values; an area without a parent gives an error.
    let parent = || unsafe { GetParent(hwnd) }.ok();
    let to_parent = |msg: u32, wparam: WPARAM| -> Option<LRESULT> {
        let parent = parent()?;
        // SAFETY: the parameters are the ones Windows passed, or plain
        // values, with the area's own handle, which the Viewer decodes as
        // its own controls'.
        Some(unsafe { SendMessageW(parent, msg, Some(wparam), Some(LPARAM(hwnd.0 as isize))) })
    };
    match msg {
        WM_PAINT if to_parent(AREA_PAINT, WPARAM(0)).is_some_and(|painted| painted.0 != 0) => {
            LRESULT(0)
        }
        // The Viewer paints every pixel, so erasing first would flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_VSCROLL => to_parent(msg, wparam).unwrap_or_default(),
        // A modal dialog's message loop dispatches the area's keys without
        // offering them to the Viewer, as `run_message_loop` does first.
        WM_KEYDOWN if parent().is_some_and(|parent| offer_key(parent, hwnd, wparam, lparam)) => {
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS) as isize),
        WM_LBUTTONDOWN => {
            // SAFETY: plain values.
            let _ = unsafe { SetFocus(Some(hwnd)) };
            LRESULT(0)
        }
        // SAFETY: the parameters are the ones Windows passed.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The rows the area shows, and how to draw them.
pub(crate) struct AreaText<'a> {
    pub(crate) font: &'a Font,
    /// Each row's text, with how many of its first characters show in the
    /// muted text colour.
    pub(crate) rows: &'a [(&'a str, usize)],
    pub(crate) line_height: Px,
    pub(crate) char_width: Px,
    /// The space before the text and above its first row.
    pub(crate) inset: (Px, Px),
}

/// Paints the area for its `WM_PAINT`: `text`'s rows from the top, and the
/// window colour around them.
pub(crate) fn paint_area(area: Hwnd, text: &AreaText) {
    let size = area.client_size();
    let (x, y) = (text.inset.0.0, text.inset.1.0);
    let height = text.line_height.0;
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: BeginPaint and EndPaint pair on the same window and
    // structure, and the font is selected back out before EndPaint; each
    // string outlives its call, and system brushes are never freed.
    unsafe {
        let dc = BeginPaint(area.0, &mut paint);
        let previous = SelectObject(dc, HGDIOBJ(text.font.handle().0));
        let window = GetSysColorBrush(COLOR_WINDOW);
        SetBkColor(dc, system_color(COLOR_WINDOW));
        let fill = |top: i32, bottom: i32| {
            if top < bottom {
                let rect = RECT {
                    left: 0,
                    top,
                    right: size.width.0,
                    bottom,
                };
                FillRect(dc, &rect, window);
            }
        };
        fill(0, y);
        let mut top = y;
        for (row, muted) in text.rows {
            if top >= size.height.0 {
                break;
            }
            let line = RECT {
                left: 0,
                top,
                right: size.width.0,
                bottom: top + height,
            };
            let units: Vec<u16> = row.encode_utf16().collect();
            let split = row.char_indices().nth(*muted).map_or(row.len(), |(i, _)| i);
            let muted_units = row[..split].encode_utf16().count();
            // The muted part, and the row's ground over the whole line.
            SetTextColor(dc, system_color(muted_text()));
            let _ = ExtTextOutW(
                dc,
                x,
                top,
                ETO_OPAQUE | ETO_CLIPPED,
                Some(&raw const line),
                PCWSTR(units.as_ptr()),
                muted_units as u32,
                None,
            );
            SetTextColor(dc, system_color(COLOR_WINDOWTEXT));
            let rest = &units[muted_units..];
            let _ = ExtTextOutW(
                dc,
                x + text.char_width.0 * *muted as i32,
                top,
                ETO_CLIPPED,
                Some(&raw const line),
                PCWSTR(rest.as_ptr()),
                rest.len() as u32,
                None,
            );
            top += height;
        }
        fill(top, size.height.0);
        SelectObject(dc, previous);
        let _ = EndPaint(area.0, &paint);
    }
}

/// The height of a window's caption at `dpi`, by which a Viewer opens
/// below and to the right of the one opened before it.
pub(crate) fn caption_height(dpi: Dpi) -> Px {
    // SAFETY: plain values.
    Px(unsafe { GetSystemMetricsForDpi(SM_CYCAPTION, dpi.get()) })
}

/// Sets the area's scroll bar to `bar`. Its page holds the whole range
/// while the whole file shows, which disables it, as Notepad's is, instead
/// of hiding it, so the text's width never changes with it.
pub(crate) fn set_area_scroll(area: Hwnd, bar: ScrollBar) {
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: bar.max,
        nPage: bar.page,
        nPos: bar.position,
        nTrackPos: 0,
    };
    // SAFETY: `info` is complete and outlives the call.
    unsafe { SetScrollInfo(area.0, SB_VERT, &info, true) };
}

/// What the reading thread found.
pub(crate) enum ViewerEvent {
    /// The file opened: its length, and its first block.
    Opened { len: u64, first: Vec<u8> },
    /// The block `index`, fewer bytes than asked when the file shrank.
    Block { index: u64, bytes: Vec<u8> },
    /// Windows could not open the file, or, when `opened`, read it.
    Failed { opened: bool, reason: String },
}
