//! Top-level windows: the window class, the window procedure trampoline that
//! hands each message to a [`WindowProc`], and the message loop.
//!
//! The trampoline decodes every message whose parameters carry a pointer, so
//! no handler outside this module dereferences one.

use super::notify::{Command, Notification};
use super::{Hwnd, from_rect, to_rect, wide};
use crate::core::commands::{Key, Keystroke};
use crate::core::layout::{Dpi, Px, Rect, ScrollRequest, Size};
use crate::core::settings::Placement;
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F1, VK_F12, VK_MENU,
    VK_OEM_COMMA, VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetScrollInfo, GetSystemMetrics, GetWindowLongPtrW, HICON, IDC_ARROW, IMAGE_ICON,
    IsDialogMessageW, KillTimer, LR_DEFAULTCOLOR, LoadCursorW, LoadImageW, MINMAXINFO, MSG,
    PostQuitMessage, RegisterClassExW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_TRACKPOS, SM_CXICON,
    SM_CXSMICON, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SendMessageW, SetTimer, SetWindowLongPtrW,
    SetWindowPlacement, TranslateAcceleratorW, TranslateMessage, WA_INACTIVE, WINDOWPLACEMENT,
    WINDOWPLACEMENT_FLAGS, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_CREATE,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_ENDSESSION, WM_GETMINMAXINFO,
    WM_INITMENUPOPUP, WM_KEYDOWN, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT,
    WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WM_VSCROLL, WNDCLASSEXW,
    WS_CLIPCHILDREN, WS_EX_LEFT, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR};

/// A message decoded for its handler. Messages without a variant arrive as
/// [`Message::Other`] and go to `DefWindowProcW` unless the handler answers.
pub(crate) enum Message<'a> {
    Create,
    Activate {
        active: bool,
    },
    Size,
    DpiChanged {
        dpi: Dpi,
        suggested: Rect,
    },
    GetMinMaxInfo(MinMaxInfo<'a>),
    Command(Command),
    Notify(Notification<'a>),
    Timer(usize),
    /// A request thread has put a reply on the channel
    /// ([`super::wake::Waker`]).
    Wake,
    /// What the window posted itself with [`DEFERRED`], to act once after
    /// the messages of one user action.
    Deferred,
    /// A right-click, or the Menu key or `Shift+F10`, which give no point.
    ContextMenu {
        /// The window clicked, or the focused one for a key.
        from: Hwnd,
        /// In screen coordinates.
        point: Option<(i32, i32)>,
    },
    /// A key that [`run_message_loop`] offers the window before
    /// `IsDialogMessageW`, sent to the focused control `target`. A handler
    /// that takes it answers 1, and no `WM_CHAR` follows (design rule 8).
    /// A key that no [`Key`] names arrives as [`Message::Other`].
    KeyDown {
        target: Hwnd,
        keystroke: Keystroke,
    },
    /// A key that `IsDialogMessageW` handled moved the focus to `to`, such
    /// as `Tab` or an access key.
    FocusMoved {
        to: Hwnd,
    },
    /// A scroll bar's request; `from` is the window whose vertical scroll
    /// bar it is, such as the filter pane.
    VerticalScroll {
        from: Hwnd,
        request: ScrollRequest,
    },
    /// The mouse wheel turned over the window, or over a child that left
    /// it alone.
    MouseWheel {
        delta: i32,
        /// In screen coordinates.
        point: (i32, i32),
    },
    InitMenuPopup,
    CtlColorStatic(super::gdi::StaticColors),
    Paint,
    SettingChange {
        regional_format: bool,
    },
    EndSession {
        ending: bool,
    },
    Close,
    Destroy,
    Other(u32),
}

/// `WM_GETMINMAXINFO`'s structure, borrowed for the length of the message.
pub(crate) struct MinMaxInfo<'a>(&'a mut MINMAXINFO);

impl MinMaxInfo<'_> {
    pub(crate) fn set_min_track_size(&mut self, size: Size) {
        self.0.ptMinTrackSize = POINT {
            x: size.width.0,
            y: size.height.0,
        };
    }
}

/// One window class's message handling.
pub(crate) trait WindowProc: 'static {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT>;
}

/// Registers the class `name`, whose windows run `W`'s handler, with the
/// executable's icon and the window colour as background.
pub(crate) fn register_class<W: WindowProc>(name: &str, icon: u16) -> windows::core::Result<()> {
    // SAFETY: a null name asks for the executable's own module.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let load_icon = |metric| {
        // SAFETY: the icon is a resource of this module, loaded at the size
        // of a system metric; a missing one is an error, not a crash.
        unsafe {
            let size = GetSystemMetrics(metric);
            LoadImageW(
                Some(instance.into()),
                PCWSTR(usize::from(icon) as *const u16),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        }
        .map(|handle| HICON(handle.0))
        .unwrap_or_default()
    };
    let name = wide(name);
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(procedure::<W>),
        hInstance: instance.into(),
        hIcon: load_icon(SM_CXICON),
        hIconSm: load_icon(SM_CXSMICON),
        // SAFETY: IDC_ARROW is a predefined cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        // The documented way to name a system colour as a class brush.
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `class` is complete, and RegisterClassExW copies the name.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Creates a hidden top-level window of the class `class`, owned by `state`.
/// With a saved `placement`, it is created on that placement's monitor, so it
/// starts at that monitor's DPI; [`show`] then restores the placement.
pub(crate) fn create_window<W: WindowProc>(
    class: &str,
    state: Rc<W>,
    placement: Option<Placement>,
) -> windows::core::Result<Hwnd> {
    let (x, y, width, height) = match placement {
        Some(p) => {
            let rect = screen_rect(p);
            (
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            )
        }
        None => (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
    };
    let raw = Rc::into_raw(state);
    // SAFETY: WM_NCCREATE stores `raw` in the window, and WM_NCDESTROY takes
    // it back. A failed creation that never reached WM_NCCREATE leaks one
    // state, and the caller ends the process.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LEFT,
            &HSTRING::from(class),
            PCWSTR::null(),
            // The window's background and dividers never paint over its
            // controls.
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
            None,
            None,
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            Some(raw.cast()),
        )
    }?;
    Ok(Hwnd(hwnd))
}

/// The placement's rectangle in screen coordinates. `GetWindowPlacement`
/// gives workspace coordinates, which start at the top left of the work area
/// of the monitor that shows the rectangle, so they differ from screen
/// coordinates when the taskbar is on the left or at the top.
fn screen_rect(p: Placement) -> RECT {
    let mut rect = RECT {
        left: p.left,
        top: p.top,
        right: p.right,
        bottom: p.bottom,
    };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `rect` and `info` are locals; `info` has its size set.
    let found = unsafe {
        let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    if found {
        let dx = info.rcWork.left - info.rcMonitor.left;
        let dy = info.rcWork.top - info.rcMonitor.top;
        // Saturating, because a damaged file can hold any coordinate.
        rect = RECT {
            left: rect.left.saturating_add(dx),
            top: rect.top.saturating_add(dy),
            right: rect.right.saturating_add(dx),
            bottom: rect.bottom.saturating_add(dy),
        };
    }
    rect
}

/// Shows a window that [`create_window`] made: at `placement`, maximized
/// when it was, or where Windows puts a new window. A placement that no
/// monitor shows any more is moved onto one by `SetWindowPlacement`.
pub(crate) fn show(hwnd: Hwnd, placement: Option<Placement>) {
    match placement {
        Some(p) => {
            let placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                flags: WINDOWPLACEMENT_FLAGS(0),
                showCmd: (if p.maximized {
                    SW_SHOWMAXIMIZED
                } else {
                    SW_SHOWNORMAL
                })
                .0 as u32,
                rcNormalPosition: RECT {
                    left: p.left,
                    top: p.top,
                    right: p.right,
                    bottom: p.bottom,
                },
                ..Default::default()
            };
            // SAFETY: `placement` is complete; a stale window only fails.
            if unsafe { SetWindowPlacement(hwnd.0, &placement) }.is_err() {
                hwnd.show_normal();
            }
        }
        None => hwnd.show_default(),
    }
}

/// The window size whose client area is `client`, for a window with a menu
/// bar at `dpi`.
pub(crate) fn window_size(hwnd: Hwnd, client: Size, dpi: Dpi) -> Size {
    let mut rect = to_rect(Rect {
        x: Px(0),
        y: Px(0),
        width: client.width,
        height: client.height,
    });
    let (style, ex_style) = hwnd.styles();
    // SAFETY: `rect` is a local the call adjusts in place.
    if unsafe { AdjustWindowRectExForDpi(&mut rect, style, true, ex_style, dpi.get()) }.is_err() {
        return client;
    }
    let rect = from_rect(rect);
    Size {
        width: rect.width,
        height: rect.height,
    }
}

/// The private message that offers a key to the main window
/// ([`Message::KeyDown`]).
const KEY_DOWN: u32 = super::wake::WAKE + 1;

/// The private message a window posts itself ([`Message::Deferred`]).
pub(crate) const DEFERRED: u32 = super::wake::WAKE + 2;

/// The private message that tells the main window where a key moved the
/// focus ([`Message::FocusMoved`]).
const FOCUS_MOVED: u32 = super::wake::WAKE + 3;

/// Runs the thread's message loop until `WM_QUIT`. The accelerators come
/// first, then the keys the main window takes for its focused control, then
/// `IsDialogMessageW` for Tab, the access keys, `Enter` and `Esc` among the
/// main window's controls (design rule 8). A key that `IsDialogMessageW`
/// moves the focus with is reported to the main window, which no control
/// notifies of it.
pub(crate) fn run_message_loop(main: Hwnd, accelerators: &super::menu::Accelerators) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local that each call fills in or reads.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if TranslateAcceleratorW(main.0, accelerators.handle(), &msg) != 0 {
                continue;
            }
            if msg.message == WM_KEYDOWN
                && SendMessageW(
                    main.0,
                    KEY_DOWN,
                    Some(msg.wParam),
                    Some(LPARAM(msg.hwnd.0 as isize)),
                )
                .0 != 0
            {
                continue;
            }
            let key = matches!(
                msg.message,
                WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR
            );
            let focused = GetFocus();
            if IsDialogMessageW(main.0, &msg).as_bool() {
                let now = GetFocus();
                if key && now != focused && !now.is_invalid() {
                    SendMessageW(main.0, FOCUS_MOVED, None, Some(LPARAM(now.0 as isize)));
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub(crate) fn quit() {
    // SAFETY: posts WM_QUIT to this thread's queue.
    unsafe { PostQuitMessage(0) };
}

/// The window procedure of every window whose class [`register_class`]
/// registered for `W`.
unsafe extern "system" fn procedure<W: WindowProc>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: WM_NCCREATE's lparam is the CREATESTRUCTW of the
        // CreateWindowExW call, whose lpCreateParams is the Rc::into_raw
        // that create_window passed.
        unsafe {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    // SAFETY: GWLP_USERDATA holds the state pointer from WM_NCCREATE, or 0.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const W;
    if raw.is_null() {
        // SAFETY: the parameters are the ones Windows passed.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        // SAFETY: the window's own reference is taken back exactly once, and
        // GWLP_USERDATA is cleared first so no later message finds it. A
        // handler still running, such as Exit's, holds its own clone (design
        // rule 2).
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(raw));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    // SAFETY: the window's reference keeps the state alive until
    // WM_NCDESTROY; this clone keeps it alive for the length of this call.
    let state = unsafe {
        Rc::increment_strong_count(raw);
        Rc::from_raw(raw)
    };
    // SAFETY: each pointer is decoded only for the message that carries it.
    let message = unsafe { decode(msg, wparam, lparam) };
    match state.message(Hwnd(hwnd), message) {
        Some(result) => result,
        // SAFETY: the parameters are the ones Windows passed.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// # Safety
/// `wparam` and `lparam` are the parameters Windows sent with `msg`.
unsafe fn decode<'a>(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Message<'a> {
    let low = |v: usize| (v & 0xffff) as u16;
    let high = |v: usize| ((v >> 16) & 0xffff) as u16;
    match msg {
        WM_CREATE => Message::Create,
        WM_ACTIVATE => Message::Activate {
            active: u32::from(low(wparam.0)) != WA_INACTIVE,
        },
        WM_SIZE => Message::Size,
        WM_DPICHANGED => match Dpi::new(u32::from(high(wparam.0))) {
            Some(dpi) => {
                // SAFETY: WM_DPICHANGED's lparam points to the suggested
                // RECT.
                let suggested = unsafe { *(lparam.0 as *const RECT) };
                Message::DpiChanged {
                    dpi,
                    suggested: from_rect(suggested),
                }
            }
            None => Message::Other(msg),
        },
        WM_GETMINMAXINFO => {
            // SAFETY: WM_GETMINMAXINFO's lparam points to a MINMAXINFO that
            // is valid and unaliased for the length of the message.
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            Message::GetMinMaxInfo(MinMaxInfo(info))
        }
        WM_COMMAND => Message::Command(super::notify::command(wparam, lparam)),
        // SAFETY: WM_NOTIFY's lparam is the NMHDR of the notification, valid
        // for the length of the message.
        WM_NOTIFY => Message::Notify(unsafe { Notification::new(lparam) }),
        WM_TIMER => Message::Timer(wparam.0),
        super::wake::WAKE => Message::Wake,
        DEFERRED => Message::Deferred,
        WM_CONTEXTMENU => Message::ContextMenu {
            from: Hwnd(HWND(wparam.0 as *mut _)),
            // -1, -1 is the keyboard's; the coordinates are signed words.
            point: (lparam.0 != -1).then(|| {
                (
                    i32::from(low(lparam.0 as usize) as i16),
                    i32::from(high(lparam.0 as usize) as i16),
                )
            }),
        },
        KEY_DOWN => match keystroke(VIRTUAL_KEY(low(wparam.0))) {
            Some(keystroke) => Message::KeyDown {
                target: Hwnd(HWND(lparam.0 as *mut _)),
                keystroke,
            },
            None => Message::Other(msg),
        },
        FOCUS_MOVED => Message::FocusMoved {
            to: Hwnd(HWND(lparam.0 as *mut _)),
        },
        WM_VSCROLL => {
            let from = Hwnd(HWND(lparam.0 as *mut _));
            match scroll_request(from, low(wparam.0)) {
                Some(request) => Message::VerticalScroll { from, request },
                None => Message::Other(msg),
            }
        }
        WM_MOUSEWHEEL => Message::MouseWheel {
            delta: i32::from(high(wparam.0) as i16),
            point: (
                i32::from(low(lparam.0 as usize) as i16),
                i32::from(high(lparam.0 as usize) as i16),
            ),
        },
        WM_INITMENUPOPUP => Message::InitMenuPopup,
        WM_CTLCOLORSTATIC => Message::CtlColorStatic(super::gdi::StaticColors::from_wparam(wparam)),
        WM_PAINT => Message::Paint,
        WM_SETTINGCHANGE => Message::SettingChange {
            // SAFETY: WM_SETTINGCHANGE's lparam is null or a NUL-terminated
            // string that names the changed area.
            regional_format: lparam.0 != 0
                && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }
                    .is_ok_and(|area| area == "intl"),
        },
        WM_ENDSESSION => Message::EndSession {
            ending: wparam.0 != 0,
        },
        WM_CLOSE => Message::Close,
        WM_DESTROY => Message::Destroy,
        _ => Message::Other(msg),
    }
}

/// What the `WM_VSCROLL` code `code` asks of `from`'s vertical scroll bar;
/// `None` for the end of a drag, which asks nothing.
fn scroll_request(from: Hwnd, code: u16) -> Option<ScrollRequest> {
    Some(match i32::from(code) {
        code if code == SB_LINEUP.0 => ScrollRequest::LineUp,
        code if code == SB_LINEDOWN.0 => ScrollRequest::LineDown,
        code if code == SB_PAGEUP.0 => ScrollRequest::PageUp,
        code if code == SB_PAGEDOWN.0 => ScrollRequest::PageDown,
        code if code == SB_TOP.0 => ScrollRequest::Top,
        code if code == SB_BOTTOM.0 => ScrollRequest::Bottom,
        code if code == SB_THUMBTRACK.0 || code == SB_THUMBPOSITION.0 => {
            // The message's own position has 16 bits; the tracked one has 32.
            let mut info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_TRACKPOS,
                ..Default::default()
            };
            // SAFETY: `info` is a local with its size set, which the call
            // fills in.
            unsafe { GetScrollInfo(from.0, SB_VERT, &mut info) }.ok()?;
            ScrollRequest::To(Px(info.nTrackPos))
        }
        _ => return None,
    })
}

/// The key `key` with the modifiers held while its message was read, if a
/// shortcut or a control's key can be it.
fn keystroke(key: VIRTUAL_KEY) -> Option<Keystroke> {
    let key = match key {
        // A capital letter's virtual-key code is its ASCII code.
        VIRTUAL_KEY(code @ 0x41..=0x5A) => Key::Letter(code as u8),
        VIRTUAL_KEY(code) if (VK_F1.0..=VK_F12.0).contains(&code) => {
            Key::Function((code - VK_F1.0 + 1) as u8)
        }
        VK_RETURN => Key::Enter,
        VK_OEM_COMMA => Key::Comma,
        VK_DOWN => Key::Down,
        VK_ESCAPE => Key::Escape,
        _ => return None,
    };
    // SAFETY: plain values; the high bit of the state is set while the key
    // is down.
    let held = |modifier: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(modifier.0)) } < 0;
    Some(Keystroke {
        ctrl: held(VK_CONTROL),
        alt: held(VK_MENU),
        shift: held(VK_SHIFT),
        key,
    })
}

impl Hwnd {
    /// The DPI of the monitor that shows the window.
    pub(crate) fn dpi(self) -> Dpi {
        // SAFETY: a stale handle gives 0, which falls back to 96.
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.0) };
        Dpi::new(dpi).unwrap_or(Dpi::new(96).expect("96 is not zero"))
    }

    /// Starts or restarts the timer `id`, which sends `WM_TIMER` every
    /// `milliseconds`.
    pub(crate) fn set_timer(self, id: usize, milliseconds: u32) {
        // SAFETY: plain values; no timer procedure.
        unsafe { SetTimer(Some(self.0), id, milliseconds, None) };
    }

    pub(crate) fn kill_timer(self, id: usize) {
        // SAFETY: plain values; a timer that does not run is no error here.
        let _ = unsafe { KillTimer(Some(self.0), id) };
    }
}
