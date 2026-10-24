//! The shortcut that shows Gozler from any program (DESIGN.md: The Keyboard
//! page). A shortcut with `Ctrl`, `Alt` and `Shift` only is a `RegisterHotKey`
//! on the main window, which then receives `WM_HOTKEY`. A shortcut with the
//! Windows key is a low-level keyboard hook on a thread of its own, because
//! Explorer registers most Windows-key combinations, and Windows removes a
//! low-level hook whose callback waits for a busy window thread. The hook posts
//! the show message to the window. While Settings' shortcut box has the focus,
//! a hook of the same kind gives it the keys pressed, the Windows key
//! included, which no Windows control can enter.

use super::Hwnd;
use super::keys::{key_of, virtual_key};
use super::wake::OpenWaker;
use crate::core::commands::Keystroke;
use crate::core::shortcuts::ShowShortcut;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use windows::Win32::Foundation::{
    CloseHandle, E_FAIL, E_OUTOFMEMORY, ERROR_HOTKEY_ALREADY_REGISTERED, HANDLE, HWND, LPARAM,
    LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, SendInput,
    UnregisterHotKey, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, HOOKPROC, KBDLLHOOKSTRUCT, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::PCWSTR;

/// A key that no keyboard has and Windows gives no meaning, which the hook
/// sends while the Windows key is held, so the shell sees a combination and
/// does not open Start when the Windows key comes up.
const UNASSIGNED: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

/// The registered shortcut, unregistered on drop, after which Windows' own
/// use of its keys works again at once.
pub(crate) struct Registration(ShowShortcut, Kind);

impl Registration {
    /// The shortcut that Windows gives Gozler.
    pub(crate) fn shortcut(&self) -> ShowShortcut {
        self.0
    }

    /// Polls native installation without waiting; a prepared hook is inactive.
    pub(crate) fn poll(&mut self) -> Option<Result<(), RegisterError>> {
        match &mut self.1 {
            Kind::Hotkey { .. } => Some(Ok(())),
            Kind::Hook { hook, .. } => hook
                .poll()
                .map(|answer| answer.map_err(RegisterError::Failed)),
        }
    }

    /// Call after [`Self::poll`] succeeds to accept the prepared hook's choice.
    pub(crate) fn activate(&self) {
        if let Kind::Hook { enabled, .. } = &self.1 {
            enabled.store(true, Ordering::Release);
        }
    }
}

enum Kind {
    Hotkey {
        window: Hwnd,
        id: i32,
    },
    Hook {
        hook: HookThread,
        enabled: Arc<AtomicBool>,
    },
}

/// Deactivates delivery before asking its native hook loop to stop.
struct HookThread {
    stop: Arc<Stop>,
    live: OpenWaker,
    started: mpsc::Receiver<windows::core::Result<()>>,
    ready: bool,
}

struct Stop(isize);

impl Stop {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: an unnamed manual-reset event, initially unsignaled, owned
        // jointly by the registration and its hook thread.
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(|handle| Self(handle.0 as isize))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }

    fn signal(&self) {
        // SAFETY: this Arc keeps the event handle open throughout the call,
        // even when the hook thread has already returned.
        let _ = unsafe { SetEvent(self.handle()) };
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        // SAFETY: both the UI owner and hook thread have released this event;
        // no waiter or signaler can still borrow its handle.
        let _ = unsafe { CloseHandle(self.handle()) };
    }
}

/// Why Windows did not let Gozler use a shortcut.
#[derive(Debug)]
pub(crate) enum RegisterError {
    /// Another program holds the shortcut (DESIGN.md: Dialogs).
    Taken,
    /// Windows refused it for another reason, which the error gives.
    Failed(windows::core::Error),
}

/// Each hotkey's own ID, so a new shortcut can be registered while the old
/// one still is, and unregistering the old one leaves the new one.
static NEXT_HOTKEY: AtomicI32 = AtomicI32::new(1);
static NEXT_CAPTURE: AtomicUsize = AtomicUsize::new(1);

/// Prepares `shortcut`, which sends `WM_HOTKEY` or `show_message` when active.
/// Native hotkeys are ready immediately; poll and activate a Windows-key hook.
pub(crate) fn register(
    window: Hwnd,
    shortcut: ShowShortcut,
    show_message: u32,
    answered: OpenWaker,
) -> Result<Registration, RegisterError> {
    let keystroke = shortcut.keystroke();
    if !shortcut.windows() {
        let mut modifiers = MOD_NOREPEAT;
        for (held, modifier) in [
            (keystroke.ctrl, MOD_CONTROL),
            (keystroke.alt, MOD_ALT),
            (keystroke.shift, MOD_SHIFT),
        ] {
            if held {
                modifiers |= modifier;
            }
        }
        let id = NEXT_HOTKEY.fetch_add(1, Ordering::Relaxed);
        let vk = u32::from(virtual_key(keystroke.key));
        // SAFETY: plain values.
        return match unsafe { RegisterHotKey(Some(window.0), id, modifiers, vk) } {
            Ok(()) => Ok(Registration(shortcut, Kind::Hotkey { window, id })),
            Err(error) if error.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() => {
                Err(RegisterError::Taken)
            }
            Err(error) => Err(RegisterError::Failed(error)),
        };
    }
    let live = OpenWaker::posting(window, show_message);
    let enabled = Arc::new(AtomicBool::new(false));
    let target = Target {
        window: window.0.0 as isize,
        message: show_message,
        keystroke,
        live: live.clone(),
        enabled: Arc::clone(&enabled),
    };
    HookThread::start(
        live,
        Some(answered),
        move || TARGET.set(Some(target)),
        Some(keyboard_hook),
    )
    .map(|hook| Registration(shortcut, Kind::Hook { hook, enabled }))
    .map_err(RegisterError::Failed)
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Kind::Hotkey { window, id } = self.1 {
            // SAFETY: plain values.
            let _ = unsafe { UnregisterHotKey(Some(window.0), id) };
        }
    }
}

impl HookThread {
    /// Starts the thread, which runs `prepare` and then hooks the keyboard
    /// with `hook`.
    fn start(
        live: OpenWaker,
        answered: Option<OpenWaker>,
        prepare: impl FnOnce() + Send + 'static,
        hook: HOOKPROC,
    ) -> windows::core::Result<HookThread> {
        let stop = Arc::new(Stop::new()?);
        let worker_stop = Arc::clone(&stop);
        let (ready, started) = mpsc::sync_channel(1);
        let _thread = std::thread::spawn(move || {
            prepare();
            run_hook(hook, &worker_stop, &ready, answered.as_ref());
        });
        Ok(HookThread {
            stop,
            live,
            started,
            ready: false,
        })
    }

    fn poll(&mut self) -> Option<windows::core::Result<()>> {
        if self.ready {
            return Some(Ok(()));
        }
        match self.started.try_recv() {
            Ok(Ok(())) => {
                self.ready = true;
                Some(Ok(()))
            }
            Ok(Err(error)) => Some(Err(error)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(windows::core::Error::from_hresult(E_FAIL)))
            }
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        self.live.close();
        self.stop.signal();
    }
}

/// The message a [`Capture`] posts to its dialog for each key pressed in
/// the shortcut box, which [`captured`] reads.
pub(crate) const CAPTURED: u32 = super::wake::WAKE + 7;

/// [`CAPTURED`]'s bits for the modifiers held, in its `lparam`.
const CAPTURED_WINDOWS: isize = 1;
const CAPTURED_CTRL: isize = 2;
const CAPTURED_ALT: isize = 4;
const CAPTURED_SHIFT: isize = 8;

/// While it lives, each key pressed goes to the dialog as [`CAPTURED`]
/// instead of to the shortcut box or the shell, so the box can take `Win+E`,
/// which Explorer would open on. The modifiers pass, and so do `Tab` and
/// `Esc`, because Windows switches programs and opens Task Manager with
/// them, and `Enter` alone, which presses the dialog's default button.
pub(crate) struct Capture {
    _hook: HookThread,
    token: usize,
}

impl Capture {
    pub(crate) fn accepts(&self, token: usize) -> bool {
        self.token == token
    }
}

/// Captures the keys for `dialog` while the shortcut box has the focus.
pub(crate) fn capture(dialog: Hwnd) -> windows::core::Result<Capture> {
    let live = OpenWaker::posting(dialog, CAPTURED);
    #[allow(deprecated, reason = "try_update requires Rust 1.95; MSRV is 1.94")]
    let token = NEXT_CAPTURE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1).filter(|next| *next <= usize::MAX >> 8)
        })
        .map_err(|_| windows::core::Error::from_hresult(E_OUTOFMEMORY))?;
    let target = CaptureTarget {
        dialog: dialog.0.0 as isize,
        live: live.clone(),
        token,
    };
    HookThread::start(
        live,
        None,
        move || CAPTURE.set(Some(target)),
        Some(capture_hook),
    )
    .map(|hook| Capture { _hook: hook, token })
}

/// The keys a [`CAPTURED`] message carries: whether the Windows key was
/// held, and the others, if a shortcut can be them.
pub(crate) fn captured(wparam: WPARAM, lparam: LPARAM) -> Option<(usize, bool, Keystroke)> {
    let held = |bit| lparam.0 & bit != 0;
    let key = key_of((wparam.0 & 0xff) as u16)?;
    Some((
        wparam.0 >> 8,
        held(CAPTURED_WINDOWS),
        Keystroke {
            ctrl: held(CAPTURED_CTRL),
            alt: held(CAPTURED_ALT),
            shift: held(CAPTURED_SHIFT),
            key,
        },
    ))
}

/// What the hook catches and whom it tells.
#[derive(Clone)]
struct Target {
    /// The main window's handle, as a number, so the thread may hold it.
    window: isize,
    message: u32,
    /// The keys besides the Windows key.
    keystroke: Keystroke,
    live: OpenWaker,
    enabled: Arc<AtomicBool>,
}

#[derive(Clone)]
struct CaptureTarget {
    dialog: isize,
    live: OpenWaker,
    token: usize,
}

thread_local! {
    /// The hook thread's target, which the callback reads.
    static TARGET: RefCell<Option<Target>> = const { RefCell::new(None) };
    /// Whether the shortcut's key went down, so its repeats and its
    /// release are swallowed too.
    static SWALLOWING: Cell<bool> = const { Cell::new(false) };
    /// A capture thread's dialog and its independent delivery lifetime.
    static CAPTURE: RefCell<Option<CaptureTarget>> = const { RefCell::new(None) };
}

/// The hook thread: it installs `hook`, says so through `ready`, and runs
/// the message loop in which Windows calls the hook, until deactivation.
fn run_hook(
    hook: HOOKPROC,
    stop: &Stop,
    ready: &mpsc::SyncSender<windows::core::Result<()>>,
    answered: Option<&OpenWaker>,
) {
    let answer = |result| {
        let _ = ready.try_send(result);
        if let Some(answered) = answered {
            answered.deliver(|| true);
        }
    };
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local each call fills in; PeekMessageW makes the
    // thread's queue. The shared event outlives every wait, and the hook is
    // removed on this thread before it ends.
    unsafe {
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let hook = match SetWindowsHookExW(
            WH_KEYBOARD_LL,
            hook,
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            0,
        ) {
            Ok(hook) => hook,
            Err(error) => {
                answer(Err(error));
                return;
            }
        };
        answer(Ok(()));
        loop {
            let wake = MsgWaitForMultipleObjectsEx(
                Some(&[stop.handle()]),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
            if wake.0 != WAIT_OBJECT_0.0 + 1 {
                break;
            }
            if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() && msg.message == WM_QUIT {
                break;
            }
        }
        let _ = UnhookWindowsHookEx(hook);
    }
}

/// Whether `key` is down now.
fn down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: plain values; the high bit is set while the key is down.
    unsafe { GetAsyncKeyState(i32::from(key.0)) < 0 }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let next = || {
        // SAFETY: the parameters are the ones Windows passed.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    let Some(target) = TARGET.with(|target| target.borrow().clone()) else {
        return next();
    };
    if code != HC_ACTION as i32 {
        return next();
    }
    // SAFETY: a low-level keyboard hook's lparam points to a
    // KBDLLHOOKSTRUCT for the length of the call.
    let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let keystroke = target.keystroke;
    if event.vkCode != u32::from(virtual_key(keystroke.key)) {
        return next();
    }
    let message = wparam.0 as u32;
    let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let released = message == WM_KEYUP || message == WM_SYSKEYUP;
    let held = pressed
        && (down(VK_LWIN) || down(VK_RWIN))
        && down(VK_CONTROL) == keystroke.ctrl
        && down(VK_MENU) == keystroke.alt
        && down(VK_SHIFT) == keystroke.shift;
    let mut swallow = false;
    let mut first = false;
    target.live.while_open(|| {
        if !target.enabled.load(Ordering::Acquire) {
            return;
        }
        if released && SWALLOWING.replace(false) {
            swallow = true;
        } else if held {
            swallow = true;
            first = !SWALLOWING.replace(true);
            if first {
                // SAFETY: Drop closes this gate before the target can be
                // destroyed. Posting carries no pointers and never waits.
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(target.window as *mut _)),
                        target.message,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            }
        }
    });
    if !swallow {
        return next();
    }
    if first {
        // Input injection and the external hook chain run outside the gate;
        // only a key already admitted while live reaches this point.
        press_unassigned_key();
    }
    LRESULT(1)
}

unsafe extern "system" fn capture_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let next = || {
        // SAFETY: the parameters are the ones Windows passed.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    let Some(target) = CAPTURE.with(|target| target.borrow().clone()) else {
        return next();
    };
    let message = wparam.0 as u32;
    if code != HC_ACTION as i32 || !(message == WM_KEYDOWN || message == WM_SYSKEYDOWN) {
        return next();
    }
    // SAFETY: a low-level keyboard hook's lparam points to a
    // KBDLLHOOKSTRUCT for the length of the call.
    let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let key = VIRTUAL_KEY(event.vkCode as u16);
    let windows = down(VK_LWIN) || down(VK_RWIN);
    let (ctrl, alt, shift) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT));
    let modifier = [
        VK_LWIN,
        VK_RWIN,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
        VK_LSHIFT,
        VK_RSHIFT,
    ]
    .contains(&key);
    let alone = !(windows || ctrl || alt || shift);
    // The key this hook sends passes too, or the shell would not see it.
    if modifier || [VK_TAB, VK_ESCAPE, UNASSIGNED].contains(&key) || (key == VK_RETURN && alone) {
        return next();
    }
    let mut held = 0;
    for (is_held, bit) in [
        (windows, CAPTURED_WINDOWS),
        (ctrl, CAPTURED_CTRL),
        (alt, CAPTURED_ALT),
        (shift, CAPTURED_SHIFT),
    ] {
        if is_held {
            held |= bit;
        }
    }
    let swallow = target.live.while_open(|| {
        // SAFETY: Drop closes this gate before the dialog can be destroyed.
        // Posting carries no pointers and never waits.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(target.dialog as *mut _)),
                CAPTURED,
                WPARAM((target.token << 8) | usize::from(key.0)),
                LPARAM(held),
            )
        };
    });
    if !swallow {
        return next();
    }
    if windows {
        press_unassigned_key();
    }
    LRESULT(1)
}

/// Presses and releases [`UNASSIGNED`].
fn press_unassigned_key() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: UNASSIGNED,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    // SAFETY: two complete keyboard inputs, which the call copies.
    unsafe {
        SendInput(
            &[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)],
            size_of::<INPUT>() as i32,
        )
    };
}
