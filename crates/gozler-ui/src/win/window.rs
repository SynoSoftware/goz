//! Top-level windows: the window class, the window procedure trampoline that
//! hands each message to a [`WindowProc`], and the message loop.
//!
//! The trampoline decodes every message whose parameters carry a pointer, so
//! no handler outside this module dereferences one.

use super::keys::key_of;
use super::menu::Accelerators;
use super::notify::{Command, Notification};
use super::{Hwnd, from_rect, to_rect, wide};
use crate::core::commands::{Key, Keystroke};
use crate::core::layout::{Dpi, Px, Rect, ScrollRequest, Size};
use crate::core::settings::Placement;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromRect, MonitorFromWindow, ScreenToClient,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, GetKeyState, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_CONTROL,
    VK_LBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CW_USEDEFAULT, CallNextHookEx, CreateWindowExW, DBT_DEVICEQUERYREMOVE,
    DBT_DEVICEQUERYREMOVEFAILED, DBT_DEVICEREMOVECOMPLETE, DBT_DEVICEREMOVEPENDING,
    DBT_DEVTYP_HANDLE, DEV_BROADCAST_HANDLE, DEV_BROADCAST_HDR, DefWindowProcW, DispatchMessageW,
    EndMenu, GA_ROOT, GWLP_USERDATA, GetAncestor, GetCursorPos, GetMessageW, GetScrollInfo,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowThreadProcessId, HHOOK, HICON, HTCLIENT,
    IDC_ARROW, IDC_SIZENS, IMAGE_ICON, IsDialogMessageW, KillTimer, LR_DEFAULTCOLOR, LoadCursorW,
    LoadImageW, MINMAXINFO, MSG, MSGF_DIALOGBOX, MSGF_MESSAGEBOX, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostQuitMessage, PostThreadMessageW,
    QS_ALLINPUT, RegisterClassExW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SC_KEYMENU, SCROLLINFO, SIF_TRACKPOS,
    SM_CXICON, SM_CXSMICON, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, SendMessageW,
    SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPlacement, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WA_INACTIVE, WH_MSGFILTER, WINDOWPLACEMENT,
    WINDOWPLACEMENT_FLAGS, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_COMMAND,
    WM_CONTEXTMENU, WM_COPYDATA, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_DEVICECHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_ENTERSIZEMOVE, WM_EXITMENULOOP,
    WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_HOTKEY, WM_INITMENUPOPUP, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY,
    WM_NOTIFY, WM_QUIT, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR, WM_SYSCOLORCHANGE,
    WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_VSCROLL, WM_WINDOWPOSCHANGED,
    WNDCLASSEXW, WPF_RESTORETOMAXIMIZED, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_EX_CONTROLPARENT, WS_EX_LEFT, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR};

/// A message decoded for its handler. Messages without a variant arrive as
/// [`Message::Other`] and go to `DefWindowProcW` unless the handler answers.
pub(crate) enum Message<'a> {
    Create,
    /// The window became the active one; `from_another_program` when the
    /// window that lost activation was another program's, or none.
    Activate {
        from_another_program: bool,
    },
    Deactivate,
    ModifiersChanged,
    Size,
    /// The window moved, was resized, or was shown or hidden, by any means:
    /// a drag, maximize, restore, a snap, `Win+Arrow`, or a move to a
    /// monitor of the same size, which sends no `WM_SIZE`. A handler that
    /// answers `None` still gets `WM_SIZE` and `WM_MOVE`.
    WindowPosChanged,
    /// The person starts dragging the window's border or title bar.
    EnterSizeMove,
    /// The person ends that drag.
    ExitSizeMove,
    DpiChanged {
        dpi: Dpi,
        suggested: Rect,
    },
    GetMinMaxInfo(MinMaxInfo<'a>),
    Command(Command),
    Notify(Notification<'a>),
    Timer(usize),
    /// A request thread has put a reply on the channel
    /// ([`super::wake::OpenWaker`]).
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
    /// `IsDialogMessageW`, sent to the focused control `target`, and
    /// `repeat` when Windows repeats it because it is held down. A handler
    /// that takes it answers 1, and no `WM_CHAR` follows (DESIGN.md:
    /// window rule 8). A key that no [`Key`] names arrives as
    /// [`Message::Other`].
    KeyDown {
        target: Hwnd,
        keystroke: Keystroke,
        repeat: bool,
    },
    /// A character typed in the control `target`, with `Alt` when `alt`,
    /// which [`run_message_loop`] offers the window before the control sees
    /// it, as a panel's quick search takes it. A handler that takes it
    /// answers 1. The key's `WM_SYSKEYDOWN` went its way, so releasing
    /// `Alt` after it opens no menu.
    Char {
        target: Hwnd,
        character: char,
        alt: bool,
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
    /// Windows asks for the cursor over `over`, in its client area when
    /// `client` is set; a handler that sets one answers 1.
    SetCursor {
        over: Hwnd,
        client: bool,
    },
    /// The left button went down on the window itself, not on a control,
    /// at `point` in client coordinates.
    LeftButtonDown {
        point: (i32, i32),
    },
    /// The mouse moved over the window, or anywhere while the window has
    /// the mouse captured, to `point` in client coordinates.
    MouseMove {
        point: (i32, i32),
    },
    LeftButtonUp,
    /// The window lost the mouse capture, by its own release or another
    /// window's.
    CaptureChanged,
    InitMenuPopup,
    /// A label or check box asks for its colors.
    CtlColorStatic {
        colors: super::gdi::StaticColors,
        control: Hwnd,
    },
    /// An edit box asks for its colors; `DefWindowProcW`'s are the edit
    /// box's own.
    CtlColorEdit {
        colors: super::gdi::StaticColors,
        control: Hwnd,
    },
    /// Windows tells of a device's removal that a registration of the
    /// window names (`WM_DEVICECHANGE` for a handle).
    DeviceRemoval {
        removal: super::folder::Removal,
        /// The registration, as `RegisterDeviceNotificationW` returned it.
        notification: isize,
    },
    SettingChange {
        regional_format: bool,
    },
    /// The system colours changed, as a high contrast theme does.
    SysColorChange,
    EndSession {
        ending: bool,
    },
    /// A native hotkey or Windows-key hook delivered its registration ID.
    Hotkey(i32),
    /// The tray icon was clicked or its menu asked for.
    /// Another process sent `text` with `WM_COPYDATA`, under `tag`.
    CopyData {
        tag: usize,
        text: String,
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

/// The system colour that a window class fills its windows' background
/// with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Background {
    /// The window colour, as File Explorer's views have.
    Window,
    /// The system dialog colour.
    Dialog,
}

/// Registers the class `name`, whose windows run `W`'s handler, with the
/// executable's icon and `background`.
pub(crate) fn register_class<W: WindowProc>(
    name: &str,
    icon: u16,
    background: Background,
) -> windows::core::Result<()> {
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
        hbrBackground: HBRUSH(
            (match background {
                Background::Window => COLOR_WINDOW,
                Background::Dialog => COLOR_BTNFACE,
            }
            .0 + 1) as isize as *mut _,
        ),
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
            // The window's background never paints over its controls.
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

/// Creates hidden child content. The parent owns its placement and keyboard
/// traversal; the child owns its Rc until WM_NCDESTROY.
pub(crate) fn create_child_window<W: WindowProc>(
    class: &str,
    state: Rc<W>,
    owner: Hwnd,
) -> windows::core::Result<Hwnd> {
    let raw = Rc::into_raw(state);
    // SAFETY: WM_NCCREATE stores the Rc and WM_NCDESTROY releases it, as in
    // create_window. The live owner is the native parent of this child.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            &HSTRING::from(class),
            PCWSTR::null(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(owner.0),
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
    place(hwnd, placement, false);
}

/// Shows a window that [`create_window`] made minimized, without taking the
/// focus from the program in front, as a program started at sign-in does;
/// it restores to `placement`, maximized when it was.
pub(crate) fn show_minimized(hwnd: Hwnd, placement: Option<Placement>) {
    place(hwnd, placement, true);
}

fn place(hwnd: Hwnd, placement: Option<Placement>, minimized: bool) {
    match placement {
        Some(p) => {
            let placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                flags: if minimized && p.maximized {
                    WPF_RESTORETOMAXIMIZED
                } else {
                    WINDOWPLACEMENT_FLAGS(0)
                },
                showCmd: (match (minimized, p.maximized) {
                    (true, _) => SW_SHOWMINNOACTIVE,
                    (false, true) => SW_SHOWMAXIMIZED,
                    (false, false) => SW_SHOWNORMAL,
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
                if minimized {
                    hwnd.show_minimized();
                } else {
                    hwnd.show_normal();
                }
            }
        }
        None if minimized => hwnd.show_minimized(),
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

/// The private message that offers a key to the main window, or to a window
/// that [`own_keys`] names ([`Message::KeyDown`]).
const KEY_DOWN: u32 = super::wake::WAKE + 1;

/// The private message a window posts itself ([`Message::Deferred`]).
pub(crate) const DEFERRED: u32 = super::wake::WAKE + 2;

/// The private message that tells the main window where a key moved the
/// focus ([`Message::FocusMoved`]).
const FOCUS_MOVED: u32 = super::wake::WAKE + 3;

/// The private message that offers a character to the main window
/// ([`Message::Char`]).
const CHAR: u32 = super::wake::WAKE + 7;

const MODIFIERS_CHANGED: u32 = super::wake::WAKE + 8;

thread_local! {

    /// The windows that [`own_keys`] names.
    static OWN_KEYS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// Whether [`message_filter`] is routing a message.
    static FILTERING: Cell<bool> = const { Cell::new(false) };
    /// How many times Shift went down in [`run_messages`] ([`shift_presses`]).
    static SHIFT_PRESSES: Cell<u32> = const { Cell::new(0) };
}

/// Names `window`, a top-level window besides the main one, as one that
/// takes its own keys, or, with `own` false, no longer, as a Viewer does
/// while it is open (DESIGN.md: The Viewer). The message loop offers each key
/// pressed in it to the window first, as [`Message::KeyDown`], then to its
/// `IsDialogMessageW` for `Tab` and the access keys among its controls.
pub(crate) fn own_keys(window: Hwnd, own: bool) {
    OWN_KEYS.with_borrow_mut(|windows| {
        windows.retain(|named| *named != window.0);
        if own {
            windows.push(window.0);
        }
    });
}

/// Offers the `WM_KEYDOWN` of `target` whose parameters are `key` and
/// `flags` to `window` as [`Message::KeyDown`]; whether the window took it.
pub(super) fn offer_key(window: HWND, target: HWND, key: WPARAM, flags: LPARAM) -> bool {
    // Bit 30 of a WM_KEYDOWN's flags: the key was down before.
    let repeat = (flags.0 as usize >> 30) & 1;
    // SAFETY: plain values; `target` is decoded as a handle, never used as
    // a pointer.
    unsafe {
        SendMessageW(
            window,
            KEY_DOWN,
            Some(WPARAM(key.0 & 0xffff | repeat << 31)),
            Some(LPARAM(target.0 as isize)),
        )
    }
    .0 != 0
}

/// Gives `msg`, a message of the top-level window `root` besides the main
/// one, to that window's keys: for a window that [`own_keys`] names, first
/// as [`Message::KeyDown`], then to its `IsDialogMessageW`. Whether the
/// window took the message. Any other window, such as a tooltip, takes none,
/// and the caller lets the message go on.
fn route_other(root: HWND, msg: &MSG) -> bool {
    let own_keys = OWN_KEYS.with_borrow(|windows| windows.contains(&root));
    if own_keys && msg.message == WM_KEYDOWN && offer_key(root, msg.hwnd, msg.wParam, msg.lParam) {
        return true;
    }
    if !own_keys {
        return false;
    }
    // SAFETY: `msg` is a message that a loop read; a stale window only
    // fails.
    unsafe { IsDialogMessageW(root, msg) }.as_bool()
}

/// Runs the thread's message loop until `WM_QUIT`, giving each message to
/// the keys of the top-level window it belongs to, so a key pressed in one
/// window never runs another window's command. For the main window, the
/// accelerators come first, then the keys the main window takes for its
/// focused control, then the access keys of the main window's toolbar
/// buttons, which are not windows that `IsDialogMessageW` can match, then
/// `IsDialogMessageW` for Tab, the access keys, `Enter` and `Esc` among the
/// main window's controls (DESIGN.md: window rule 8). A key that
/// `IsDialogMessageW` moves the focus with is reported to the main window,
/// which no control notifies of it. A message for a window that
/// [`own_keys`] names, or for one
/// of their controls, goes through `route_other`, and one for any other
/// window, such as a tooltip, is only dispatched.
///
/// A dialog or message box that the main window opens runs a loop of its own,
/// which never reaches this one. For its length, a message hook gives the
/// same messages to `route_other`, so the windows besides the main one keep
/// their keys ([`MessageFilter`]).
///
/// `accelerators` gives the table of the moment, because Settings and the
/// mode replace it; the loop holds each table until the command it sent
/// returns.
pub(crate) fn run_message_loop(main: Hwnd, accelerators: &dyn Fn() -> Rc<Accelerators>) {
    let _filter = MessageFilter::install();
    run_messages(main, accelerators, &|| false);
}

/// Runs [`run_message_loop`]'s loop inside a call that must not return
/// yet, until `done` gives true after a message. A `WM_QUIT` ends it too,
/// and is posted again for the loop under it.
pub(crate) fn run_messages_until(
    main: Hwnd,
    accelerators: &dyn Fn() -> Rc<Accelerators>,
    done: &dyn Fn() -> bool,
) {
    if let Some(code) = run_messages(main, accelerators, done) {
        // SAFETY: posts WM_QUIT to this thread's queue.
        unsafe { PostQuitMessage(code) };
    }
}

#[cfg(test)]
pub(crate) fn post_key(target: Hwnd, key: VIRTUAL_KEY) {
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    // SAFETY: the key messages carry values only; the caller owns the window.
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::SetKeyboardState(&[0; 256])
            .expect("the test thread owns its keyboard state");
        PostMessageW(
            Some(target.0),
            WM_KEYDOWN,
            WPARAM(key.0 as usize),
            LPARAM(1),
        )
        .expect("the test's key target remains alive");
        PostMessageW(
            Some(target.0),
            windows::Win32::UI::WindowsAndMessaging::WM_KEYUP,
            WPARAM(key.0 as usize),
            LPARAM((3_u32 << 30 | 1) as isize),
        )
        .expect("the test's key target remains alive");
    }
}

/// The loop of [`run_message_loop`], until `done` gives true; gives the
/// exit code of the `WM_QUIT` that ended it.
fn run_messages(
    main: Hwnd,
    accelerators: &dyn Fn() -> Rc<Accelerators>,
    done: &dyn Fn() -> bool,
) -> Option<i32> {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local that each call fills in or reads.
    unsafe {
        while !done() && GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            // A thread message has no window, and belongs to the main one.
            let root = if msg.hwnd.is_invalid() {
                main.0
            } else {
                GetAncestor(msg.hwnd, GA_ROOT)
            };
            if root != main.0 {
                if !route_other(root, &msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                continue;
            }
            if matches!(
                msg.message,
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
            ) && [VK_MENU, VK_CONTROL, VK_SHIFT]
                .iter()
                .any(|key| msg.wParam.0 == usize::from(key.0))
            {
                SendMessageW(main.0, MODIFIERS_CHANGED, None, None);
            }
            // Bit 30 of a key down's flags: the key was down before.
            if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN)
                && msg.wParam.0 == usize::from(VK_SHIFT.0)
                && (msg.lParam.0 as usize >> 30) & 1 == 0
            {
                SHIFT_PRESSES.set(SHIFT_PRESSES.get().wrapping_add(1));
            }
            if accelerators().translate(main, &msg) {
                continue;
            }
            if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN)
                && offer_key(main.0, msg.hwnd, msg.wParam, msg.lParam)
            {
                continue;
            }
            // Quick search handles characters before dialog access keys.
            // Ctrl+Space belongs to the control's native selection handling.
            if matches!(msg.message, WM_CHAR | WM_SYSCHAR)
                && !(msg.wParam.0 == ' ' as usize && held(VK_CONTROL))
                && SendMessageW(
                    main.0,
                    CHAR,
                    Some(WPARAM(
                        msg.wParam.0 | usize::from(msg.message == WM_SYSCHAR) << 31,
                    )),
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
            if msg.message == WM_SYSCHAR && super::toolbar::press_access_key(main, msg.wParam) {
                continue;
            }
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
    (msg.message == WM_QUIT).then_some(msg.wParam.0 as i32)
}

/// The thread's `WH_MSGFILTER` hook, removed on drop. The loops of dialog
/// boxes and message boxes call it with `MSGF_DIALOGBOX` or
/// `MSGF_MESSAGEBOX` before they process a message. While the main window
/// has one open, [`run_message_loop`] does not run, so without the hook the
/// Viewers, which stay usable, would get no
/// Tab, access keys, `Enter` or `Esc` (DESIGN.md: window rule 8).
struct MessageFilter(Option<HHOOK>);

impl MessageFilter {
    /// Installs the hook for the calling thread. When Windows refuses, the
    /// modal loops stay as they were.
    fn install() -> MessageFilter {
        // SAFETY: `message_filter` is code of this program, so no module is
        // named, and the hook is for this thread only. `Drop` removes it on
        // the same thread.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_MSGFILTER,
                Some(message_filter),
                None,
                GetCurrentThreadId(),
            )
        };
        MessageFilter(hook.ok())
    }
}

impl Drop for MessageFilter {
    fn drop(&mut self) {
        if let Some(hook) = self.0 {
            // SAFETY: the hook is this value's own and not yet removed.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
    }
}

/// Clears [`FILTERING`] when [`message_filter`] ends.
struct Filtering;

impl Drop for Filtering {
    fn drop(&mut self) {
        FILTERING.set(false);
    }
}

/// The procedure of [`MessageFilter`]: gives the message of a dialog's or
/// message box's loop to `route_other`, and the loop skips a message that a
/// window took. Routing a key can run a handler that opens another dialog,
/// whose loop calls this hook again while the first call is still running.
/// That call passes every message on, so a key is not routed into the
/// handler that is running.
unsafe extern "system" fn message_filter(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let modal =
        u32::try_from(code).is_ok_and(|code| code == MSGF_DIALOGBOX || code == MSGF_MESSAGEBOX);
    if modal && !FILTERING.replace(true) {
        let _filtering = Filtering;
        // SAFETY: for these codes lparam points to the MSG that the loop is
        // about to process, valid for this call.
        let msg = unsafe { &*(lparam.0 as *const MSG) };
        if !msg.hwnd.is_invalid() {
            // SAFETY: a plain value; a stale window gives none.
            let root = unsafe { GetAncestor(msg.hwnd, GA_ROOT) };
            if route_other(root, msg) {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: the parameters are the ones Windows passed.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Opens the menu bar with its first menu highlighted, as `Alt` pressed
/// and released alone does.
pub(crate) fn open_menu_bar(window: Hwnd) {
    // SAFETY: a plain system command to the window.
    unsafe {
        SendMessageW(
            window.0,
            WM_SYSCOMMAND,
            Some(WPARAM(SC_KEYMENU as usize)),
            Some(LPARAM(0)),
        )
    };
}

/// Keeps the message loop alive until background shutdown finishes, without
/// waiting on the window thread or using a window handle after destruction.
pub(crate) fn quit_after(
    finish: impl FnOnce(&super::work::OffThread) + Send + 'static,
) -> Shutdown {
    // SAFETY: the calling thread owns the queue that finalization will wake.
    let thread = unsafe { GetCurrentThreadId() };
    let finished = Arc::new(AtomicBool::new(false));
    let completed = Arc::clone(&finished);
    super::work::spawn(move |worker| {
        finish(worker);
        completed.store(true, Ordering::Release);
        // SAFETY: WM_QUIT has no pointer parameters or HWND.
        let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    });
    Shutdown { finished }
}

/// Actual persistence completion, independent of WM_QUIT, which setup may
/// already have posted before finalization starts. No window owns this lifetime.
#[derive(Clone)]
pub(crate) struct Shutdown {
    finished: Arc<AtomicBool>,
}

impl Shutdown {
    /// Called after every window is gone. The process may remain for its writer;
    /// the window thread dispatches messages instead of joining that worker.
    pub(crate) fn finish(&self) {
        self.dispatch_until_finished(None);
    }

    /// Windows may terminate the writer after WM_ENDSESSION returns. Give it
    /// only the owner's short budget; the last valid file survives an unfinished
    /// atomic replacement, but the newest state is not guaranteed at sign-out.
    pub(crate) fn finish_session(&self) {
        self.dispatch_until_finished(Some(crate::core::work::SESSION_FINALIZATION));
    }

    fn dispatch_until_finished(&self, budget: Option<Duration>) -> bool {
        let deadline = budget.map(|budget| Instant::now() + budget);
        let mut quit = None;
        let mut message = MSG::default();
        while !self.finished.load(Ordering::Acquire) {
            let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            if left.is_some_and(|left| left.is_zero()) {
                break;
            }
            // SAFETY: message is a local. The caller destroyed the window and
            // holds no model borrow while sent/posted messages are dispatched.
            if unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                if message.message == WM_QUIT {
                    quit.get_or_insert(message.wParam.0 as i32);
                } else {
                    // SAFETY: PeekMessageW initialized the message.
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                continue;
            }
            let timeout = left.map_or(u32::MAX, |left| {
                u32::try_from(left.as_millis().max(1)).unwrap_or(u32::MAX - 1)
            });
            // SAFETY: there are no handles to borrow; only queue input or the
            // remaining budget wakes this wait. The finalizer posts WM_QUIT.
            unsafe {
                MsgWaitForMultipleObjectsEx(None, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
        if let Some(code) = quit {
            // SAFETY: restore any quit consumed here for the outer/native loop.
            unsafe { PostQuitMessage(code) };
        }
        self.finished.load(Ordering::Acquire)
    }
}

/// Ends the message loop from inside any modal loop that runs on this
/// thread. A dialog's, a message box's and the file dialog's loops end on
/// `WM_QUIT` and post it again, but a menu's neither ends on it nor posts
/// it again, so an open menu is ended first.
pub(crate) fn quit_modal_loops() {
    // SAFETY: plain calls on this thread's menu and queue; with no menu
    // open, EndMenu does nothing.
    unsafe {
        let _ = EndMenu();
        PostQuitMessage(0);
    }
}

/// Whether a `WM_QUIT` waits in this thread's queue, which stays there.
pub(crate) fn quit_pending() -> bool {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local that the call fills in. Taking WM_QUIT out
    // and posting it again leaves the queue as it was.
    unsafe {
        let pending = PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool();
        if pending {
            PostQuitMessage(msg.wParam.0 as i32);
        }
        pending
    }
}

/// The window procedure of every window whose class [`register_class`]
/// registered for `W`.
unsafe extern "system" fn procedure<W: WindowProc>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: these are the original parameters of this native message.
    let _context = unsafe { super::watchdog::enter(msg, wparam, lparam) };
    // SAFETY: as above.
    let _update = unsafe { super::present::enter(hwnd, msg, lparam) };
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
        // handler still running, such as Exit's, holds its own clone
        // (DESIGN.md: window rule 2).
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
    if msg == WM_NOTIFY {
        // SAFETY: WM_NOTIFY's lparam is its NMHDR.
        let typed_row = unsafe { super::notify::typed_row_answer(lparam) };
        if let Some(answer) = typed_row {
            return LRESULT(answer);
        }
    }
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
/// Whether `previous`, the window that lost activation, is another
/// program's or none. A window that no longer exists counts as this
/// program's, so a dialog of Gozler's that is already gone still gives the
/// focus back to the control that opened it.
fn from_another_program(previous: HWND) -> bool {
    if previous.is_invalid() {
        return true;
    }
    let mut process = 0;
    // SAFETY: `process` is a local the call fills in; a handle that no
    // longer exists gives thread 0.
    let thread = unsafe { GetWindowThreadProcessId(previous, Some(&mut process)) };
    thread != 0 && process != std::process::id()
}

unsafe fn decode<'a>(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Message<'a> {
    let low = |v: usize| (v & 0xffff) as u16;
    let high = |v: usize| ((v >> 16) & 0xffff) as u16;
    match msg {
        WM_CREATE => Message::Create,
        WM_ACTIVATE if u32::from(low(wparam.0)) == WA_INACTIVE => Message::Deactivate,
        WM_ACTIVATE => Message::Activate {
            from_another_program: from_another_program(HWND(lparam.0 as *mut _)),
        },
        WM_SIZE => Message::Size,
        WM_WINDOWPOSCHANGED => Message::WindowPosChanged,
        WM_ENTERSIZEMOVE => Message::EnterSizeMove,
        WM_EXITSIZEMOVE => Message::ExitSizeMove,
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
        MODIFIERS_CHANGED | WM_EXITMENULOOP => Message::ModifiersChanged,
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
                repeat: wparam.0 & (1 << 31) != 0,
            },
            None => Message::Other(msg),
        },
        FOCUS_MOVED => Message::FocusMoved {
            to: Hwnd(HWND(lparam.0 as *mut _)),
        },
        CHAR => match char::from_u32((wparam.0 & 0xffff) as u32) {
            Some(character) => Message::Char {
                target: Hwnd(HWND(lparam.0 as *mut _)),
                character,
                alt: wparam.0 & (1 << 31) != 0,
            },
            None => Message::Other(msg),
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
        WM_SETCURSOR => Message::SetCursor {
            over: Hwnd(HWND(wparam.0 as *mut _)),
            client: u32::from(low(lparam.0 as usize)) == HTCLIENT,
        },
        WM_LBUTTONDOWN => Message::LeftButtonDown {
            point: client_point(lparam),
        },
        WM_MOUSEMOVE => Message::MouseMove {
            point: client_point(lparam),
        },
        WM_LBUTTONUP => Message::LeftButtonUp,
        WM_CAPTURECHANGED => Message::CaptureChanged,
        WM_INITMENUPOPUP => Message::InitMenuPopup,
        WM_CTLCOLORSTATIC => Message::CtlColorStatic {
            colors: super::gdi::StaticColors::from_wparam(wparam),
            control: Hwnd(HWND(lparam.0 as *mut _)),
        },
        WM_CTLCOLOREDIT => Message::CtlColorEdit {
            colors: super::gdi::StaticColors::from_wparam(wparam),
            control: Hwnd(HWND(lparam.0 as *mut _)),
        },
        WM_DEVICECHANGE => {
            let removal = match wparam.0 as u32 {
                DBT_DEVICEQUERYREMOVE => Some(super::folder::Removal::Query),
                DBT_DEVICEQUERYREMOVEFAILED => Some(super::folder::Removal::Failed),
                DBT_DEVICEREMOVEPENDING | DBT_DEVICEREMOVECOMPLETE => {
                    Some(super::folder::Removal::Complete)
                }
                _ => None,
            };
            // SAFETY: these events carry a DEV_BROADCAST_HDR, and one of
            // the handle type a DEV_BROADCAST_HANDLE, valid for the length
            // of the message.
            let handle = removal
                .filter(|_| lparam.0 != 0)
                .and_then(|removal| unsafe {
                    let header = &*(lparam.0 as *const DEV_BROADCAST_HDR);
                    (header.dbch_devicetype == DBT_DEVTYP_HANDLE).then(|| {
                        let handle = &*(lparam.0 as *const DEV_BROADCAST_HANDLE);
                        (removal, handle.dbch_hdevnotify.0 as isize)
                    })
                });
            match handle {
                Some((removal, notification)) => Message::DeviceRemoval {
                    removal,
                    notification,
                },
                None => Message::Other(msg),
            }
        }
        WM_SETTINGCHANGE => Message::SettingChange {
            // SAFETY: WM_SETTINGCHANGE's lparam is null or a NUL-terminated
            // string that names the changed area.
            regional_format: lparam.0 != 0
                && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }
                    .is_ok_and(|area| area == "intl"),
        },
        WM_SYSCOLORCHANGE => Message::SysColorChange,
        WM_ENDSESSION => Message::EndSession {
            ending: wparam.0 != 0,
        },
        WM_HOTKEY => Message::Hotkey(wparam.0 as i32),
        WM_COPYDATA => {
            // SAFETY: WM_COPYDATA's lparam points to a COPYDATASTRUCT whose
            // data, `cbData` bytes, lives while the message is sent.
            let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            let units = data.cbData as usize / 2;
            let text = if units == 0 || data.lpData.is_null() {
                String::new()
            } else {
                // SAFETY: as above; the sender writes UTF-16 code units, and
                // an unaligned or odd buffer is read as bytes.
                let bytes =
                    unsafe { std::slice::from_raw_parts(data.lpData as *const u8, units * 2) };
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect();
                String::from_utf16_lossy(&units)
            };
            Message::CopyData {
                tag: data.dwData,
                text,
            }
        }
        WM_CLOSE => Message::Close,
        WM_DESTROY => Message::Destroy,
        _ => Message::Other(msg),
    }
}

/// A mouse message's point, two signed words, because a captured mouse can
/// be left of or above the window.
fn client_point(lparam: LPARAM) -> (i32, i32) {
    let value = lparam.0 as usize;
    (
        i32::from((value & 0xffff) as u16 as i16),
        i32::from(((value >> 16) & 0xffff) as u16 as i16),
    )
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
    let key: Key = key_of(key.0)?;
    Some(Keystroke {
        ctrl: held(VK_CONTROL),
        alt: held(VK_MENU),
        shift: held(VK_SHIFT),
        key,
    })
}

/// How many times Shift has gone down in the main window's message loop,
/// so a changed count says that Shift was released and pressed again.
pub(crate) fn shift_presses() -> u32 {
    SHIFT_PRESSES.get()
}

/// Whether Shift was down when the message being handled was read.
pub(crate) fn shift_held() -> bool {
    held(VK_SHIFT)
}

pub(crate) fn only_alt_held() -> bool {
    held(VK_MENU) && !held(VK_CONTROL) && !held(VK_SHIFT)
}

/// Whether `key` was down when the message being handled was read.
fn held(key: VIRTUAL_KEY) -> bool {
    // SAFETY: plain values; the high bit of the state is set while the key
    // is down.
    unsafe { GetKeyState(i32::from(key.0)) < 0 }
}

/// Whether the left mouse button was down when the message being handled
/// was read, as it is while a click gives a control the focus.
pub(crate) fn left_button_down() -> bool {
    held(VK_LBUTTON)
}

/// Whether the left or the right mouse button is down now, whichever
/// program has the mouse, as during a drag from another program.
pub(crate) fn mouse_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON]
        .into_iter()
        // SAFETY: plain values; the high bit is set while the button is
        // down.
        .any(|button| unsafe { GetAsyncKeyState(i32::from(button.0)) } < 0)
}

impl Hwnd {
    /// The DPI of the monitor that shows the window.
    pub(crate) fn dpi(self) -> Dpi {
        // SAFETY: a stale handle gives 0, which falls back to 96.
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.0) };
        Dpi::new(dpi).unwrap_or(Dpi::new(96).expect("96 is not zero"))
    }

    /// The width of the work area of the monitor that shows the window,
    /// which leaves out the taskbar.
    pub(crate) fn work_area_width(self) -> Px {
        self.work_area().width
    }

    /// The work area of the monitor that shows the window, which leaves out
    /// the taskbar, in screen coordinates; empty when Windows cannot say.
    pub(crate) fn work_area(self) -> Rect {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is a local with its size set; a stale handle
        // gives the nearest monitor.
        let found = unsafe {
            let monitor = MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST);
            GetMonitorInfoW(monitor, &mut info).as_bool()
        };
        if found {
            from_rect(info.rcWork)
        } else {
            Rect::default()
        }
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

    /// Sends the window every mouse message until [`release_mouse`], as a
    /// drag needs.
    pub(crate) fn capture_mouse(self) {
        // SAFETY: plain values.
        unsafe { SetCapture(self.0) };
    }

    /// Where the cursor is, in the window's client coordinates.
    pub(crate) fn cursor_point(self) -> Option<(i32, i32)> {
        let mut point = POINT::default();
        // SAFETY: `point` is a local each call fills in or converts.
        unsafe {
            GetCursorPos(&mut point).ok()?;
            ScreenToClient(self.0, &mut point).ok().ok()?;
        }
        Some((point.x, point.y))
    }
}

/// Ends this thread's mouse capture, which sends `WM_CAPTURECHANGED`.
pub(crate) fn release_mouse() {
    // SAFETY: no parameters; releasing a capture nobody holds is no error
    // here.
    let _ = unsafe { ReleaseCapture() };
}

/// Shows the cursor that resizes up and down, as over a splitter.
pub(crate) fn show_resize_cursor() {
    // SAFETY: IDC_SIZENS is a predefined cursor, which is never destroyed.
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, IDC_SIZENS) {
            SetCursor(Some(cursor));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_quit_is_preserved_without_acknowledging_an_unfinished_writer() {
        let shutdown = Shutdown {
            finished: Arc::new(AtomicBool::new(false)),
        };
        // SAFETY: this test thread owns its message queue; no window is opened.
        unsafe { PostQuitMessage(73) };
        let finished = shutdown.dispatch_until_finished(Some(Duration::from_millis(1)));
        let mut message = MSG::default();
        // SAFETY: message is a local; remove the test's restored quit.
        let has_quit = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool();
        assert!(!finished);
        assert!(has_quit);
        assert_eq!((message.message, message.wParam.0), (WM_QUIT, 73));
    }
}
