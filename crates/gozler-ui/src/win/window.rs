//! Top-level windows: the window class, the window procedure trampoline that
//! hands each message to a [`WindowProc`], and the message loop.
//!
//! The trampoline decodes every message whose parameters carry a pointer, so
//! no handler outside this module dereferences one.

use super::keys::key_of;
use super::menu::Accelerators;
use super::notify::{Command, Notification};
use super::tray::TrayEvent;
use super::{Hwnd, from_rect, to_rect, wide};
use crate::core::commands::{Key, Keystroke};
use crate::core::layout::{Dpi, Px, Rect, ScrollRequest, Size};
use crate::core::settings::Placement;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    MonitorFromWindow, ScreenToClient,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, GetKeyState, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_CONTROL,
    VK_LBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DBT_DEVICEQUERYREMOVE,
    DBT_DEVICEQUERYREMOVEFAILED, DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_HANDLE, DEV_BROADCAST_HANDLE,
    DEV_BROADCAST_HDR, DefWindowProcW, DispatchMessageW, EndMenu, GA_ROOT, GWLP_USERDATA,
    GetAncestor, GetCursorPos, GetMessageW, GetScrollInfo, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowThreadProcessId, HICON, HTCLIENT, IDC_ARROW, IDC_SIZENS, IMAGE_ICON, IsDialogMessageW,
    KillTimer, LR_DEFAULTCOLOR, LoadCursorW, LoadImageW, MINMAXINFO, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, RegisterClassExW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SC_KEYMENU, SCROLLINFO, SIF_TRACKPOS,
    SM_CXICON, SM_CXSMICON, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, SendMessageW,
    SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPlacement, TranslateMessage, WA_INACTIVE,
    WINDOWPLACEMENT, WINDOWPLACEMENT_FLAGS, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
    WM_COMMAND, WM_CONTEXTMENU, WM_COPYDATA, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_DEVICECHANGE, WM_DPICHANGED, WM_DRAWITEM, WM_ENDSESSION, WM_ENTERSIZEMOVE,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_HOTKEY, WM_INITMENUPOPUP, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY,
    WM_NOTIFY, WM_PAINT, WM_QUIT, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR,
    WM_SYSCOLORCHANGE, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_TIMER, WM_VSCROLL, WNDCLASSEXW,
    WPF_RESTORETOMAXIMIZED, WS_CAPTION, WS_CLIPCHILDREN, WS_EX_LEFT, WS_OVERLAPPEDWINDOW,
    WS_SYSMENU, WS_THICKFRAME,
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
    Size,
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
    /// An owner-drawn control asks to be drawn.
    DrawItem(super::gdi::OwnerDraw<'a>),
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
    /// Windows asks for the background before the window paints, or a
    /// themed control asks for what lies under its corners.
    EraseBackground(super::gdi::EraseDc),
    Paint,
    SettingChange {
        regional_format: bool,
    },
    /// The system colours changed, as a high contrast theme does.
    SysColorChange,
    EndSession {
        ending: bool,
    },
    /// The shortcut to show Gozler, registered with `RegisterHotKey`, was
    /// pressed.
    Hotkey,
    /// The tray icon was clicked or its menu asked for.
    Tray(TrayEvent),
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

    /// Lets the person resize the window only up to `height`.
    pub(crate) fn set_max_track_height(&mut self, height: Px) {
        self.0.ptMaxTrackSize.y = height.0;
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

/// Creates a hidden top-level window of the class `class`, owned by `state`,
/// that `owner` owns: it stays in front of `owner`, minimizes with it and
/// has no taskbar button of its own. It has a sizing border and a close
/// button, and Windows places it.
pub(crate) fn create_owned_window<W: WindowProc>(
    class: &str,
    state: Rc<W>,
    owner: Hwnd,
) -> windows::core::Result<Hwnd> {
    let raw = Rc::into_raw(state);
    // SAFETY: as in create_window; the owner is passed as the parent of a
    // window without WS_CHILD, which makes it the owner.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LEFT,
            &HSTRING::from(class),
            PCWSTR::null(),
            WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
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

thread_local! {
    /// The window that [`set_modeless`] names, or none.
    static MODELESS: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    /// The windows that [`own_keys`] names.
    static OWN_KEYS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
}

/// Names the top-level window besides the main one whose controls take Tab,
/// the access keys, `Enter` and `Esc` as a dialog's do, such as the File
/// operations window, or none. Its messages skip the main window's
/// accelerators and keys, which would act on the main window's selection.
pub(crate) fn set_modeless(window: Option<Hwnd>) {
    MODELESS.set(window.map_or(HWND(std::ptr::null_mut()), |window| window.0));
}

/// Names `window`, a top-level window besides the main one, as one that
/// takes its own keys, or, with `own` false, no longer, as a Viewer does
/// while it is open (file-manager-spec.md: The Viewer). The message loop
/// offers each key pressed in it to the window first, as
/// [`Message::KeyDown`], then to its `IsDialogMessageW` for `Tab` and the
/// access keys among its controls.
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

/// Runs the thread's message loop until `WM_QUIT`, giving each message to
/// the keys of the top-level window it belongs to, so a key pressed in one
/// window never runs another window's command. For the main window, the
/// accelerators come first, then the keys the main window takes for its
/// focused control, then the access keys of the main window's toolbar
/// buttons, which are not windows that `IsDialogMessageW` can match, then
/// `IsDialogMessageW` for Tab, the access keys, `Enter` and `Esc` among the
/// main window's controls (DESIGN.md: window rule 8). A key that
/// `IsDialogMessageW` moves the focus with is reported to the main window,
/// which no control notifies of it. A message for the window that
/// [`set_modeless`] names, or for one of its controls, goes only through
/// that window's `IsDialogMessageW`. A message for a window that
/// [`own_keys`] names, or for one of its controls, goes to that window's
/// keys first and then its `IsDialogMessageW`, and one for any other
/// window, such as a tooltip, is only dispatched.
///
/// `accelerators` gives the table of the moment, because Settings and the
/// mode replace it; the loop holds each table until the command it sent
/// returns.
pub(crate) fn run_message_loop(main: Hwnd, accelerators: &dyn Fn() -> Rc<Accelerators>) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local that each call fills in or reads.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            // A thread message has no window, and belongs to the main one.
            let root = if msg.hwnd.is_invalid() {
                main.0
            } else {
                GetAncestor(msg.hwnd, GA_ROOT)
            };
            if root != main.0 {
                if OWN_KEYS.with_borrow(|windows| windows.contains(&root)) {
                    if msg.message == WM_KEYDOWN
                        && offer_key(root, msg.hwnd, msg.wParam, msg.lParam)
                    {
                        continue;
                    }
                    if !IsDialogMessageW(root, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    continue;
                }
                let modeless = MODELESS.get();
                if modeless.is_invalid()
                    || root != modeless
                    || !IsDialogMessageW(modeless, &msg).as_bool()
                {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                continue;
            }
            if accelerators().translate(main, &msg) {
                continue;
            }
            if msg.message == WM_KEYDOWN && offer_key(main.0, msg.hwnd, msg.wParam, msg.lParam) {
                continue;
            }
            // Before IsDialogMessageW, whose access keys would take Alt with
            // a letter, and before the control's own handling.
            if matches!(msg.message, WM_CHAR | WM_SYSCHAR)
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

pub(crate) fn quit() {
    // SAFETY: posts WM_QUIT to this thread's queue.
    unsafe { PostQuitMessage(0) };
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
        WM_DRAWITEM => {
            // SAFETY: WM_DRAWITEM's lparam points to a DRAWITEMSTRUCT that
            // is valid for the length of the message.
            let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            Message::DrawItem(super::gdi::OwnerDraw(item))
        }
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
                DBT_DEVICEREMOVECOMPLETE => Some(super::folder::Removal::Complete),
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
        WM_ERASEBKGND => Message::EraseBackground(super::gdi::EraseDc::from_wparam(wparam)),
        WM_PAINT => Message::Paint,
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
        WM_HOTKEY => Message::Hotkey,
        super::tray::TRAY => match TrayEvent::decode(wparam, lparam) {
            Some(event) => Message::Tray(event),
            None => Message::Other(msg),
        },
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

/// Whether Shift was down when the message being handled was read.
pub(crate) fn shift_held() -> bool {
    held(VK_SHIFT)
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
