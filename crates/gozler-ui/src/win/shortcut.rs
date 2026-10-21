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
use crate::core::commands::Keystroke;
use crate::core::shortcuts::ShowShortcut;
use std::cell::Cell;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, SendInput,
    UnregisterHotKey, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HOOKPROC, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE,
    PeekMessageW, PostMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
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
}

enum Kind {
    Hotkey { window: Hwnd, id: i32 },
    Hook { _hook: HookThread },
}

/// A low-level keyboard hook on a thread of its own, removed on drop.
struct HookThread {
    thread: u32,
    join: Option<JoinHandle<()>>,
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

/// Registers `shortcut`, which shows the window by sending it `WM_HOTKEY`
/// or `show_message`.
pub(crate) fn register(
    window: Hwnd,
    shortcut: ShowShortcut,
    show_message: u32,
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
    let target = Target {
        window: window.0.0 as isize,
        message: show_message,
        keystroke,
    };
    HookThread::start(move || TARGET.set(Some(target)), Some(keyboard_hook))
        .map(|hook| Registration(shortcut, Kind::Hook { _hook: hook }))
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
        prepare: impl FnOnce() + Send + 'static,
        hook: HOOKPROC,
    ) -> windows::core::Result<HookThread> {
        let (ready, started) = mpsc::channel();
        let join = std::thread::spawn(move || {
            prepare();
            run_hook(hook, &ready);
        });
        match started.recv() {
            Ok(Ok(thread)) => Ok(HookThread {
                thread,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            // The thread panicked, which the panic hook reports.
            Err(_) => Err(windows::core::Error::from_thread()),
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        // SAFETY: plain values; the thread made its queue before it said
        // that it runs.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
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
}

/// Captures the keys for `dialog` while the shortcut box has the focus.
pub(crate) fn capture(dialog: Hwnd) -> windows::core::Result<Capture> {
    let dialog = dialog.0.0 as isize;
    HookThread::start(move || CAPTURE.set(Some(dialog)), Some(capture_hook))
        .map(|hook| Capture { _hook: hook })
}

/// The keys a [`CAPTURED`] message carries: whether the Windows key was
/// held, and the others, if a shortcut can be them.
pub(crate) fn captured(wparam: WPARAM, lparam: LPARAM) -> Option<(bool, Keystroke)> {
    let held = |bit| lparam.0 & bit != 0;
    let key = key_of(wparam.0 as u16)?;
    Some((
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
#[derive(Clone, Copy)]
struct Target {
    /// The main window's handle, as a number, so the thread may hold it.
    window: isize,
    message: u32,
    /// The keys besides the Windows key.
    keystroke: Keystroke,
}

thread_local! {
    /// The hook thread's target, which the callback reads.
    static TARGET: Cell<Option<Target>> = const { Cell::new(None) };
    /// Whether the shortcut's key went down, so its repeats and its
    /// release are swallowed too.
    static SWALLOWING: Cell<bool> = const { Cell::new(false) };
    /// A capture thread's dialog, as a number, which the callback reads.
    static CAPTURE: Cell<Option<isize>> = const { Cell::new(None) };
}

/// The hook thread: it installs `hook`, says so through `ready`, and runs
/// the message loop in which Windows calls the hook, until `WM_QUIT`.
fn run_hook(hook: HOOKPROC, ready: &mpsc::Sender<windows::core::Result<u32>>) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local each call fills in; PeekMessageW makes the
    // thread's queue, so WM_QUIT can be posted to it; the hook is removed
    // on this thread before it ends.
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
                let _ = ready.send(Err(error));
                return;
            }
        };
        let _ = ready.send(Ok(GetCurrentThreadId()));
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
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
    let Some(target) = TARGET.get() else {
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
    if released && SWALLOWING.replace(false) {
        return LRESULT(1);
    }
    let held = (down(VK_LWIN) || down(VK_RWIN))
        && down(VK_CONTROL) == keystroke.ctrl
        && down(VK_MENU) == keystroke.alt
        && down(VK_SHIFT) == keystroke.shift;
    if !pressed || !held {
        return next();
    }
    if !SWALLOWING.replace(true) {
        press_unassigned_key();
        // SAFETY: the message carries no pointer; a closed window only
        // fails.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(target.window as *mut _)),
                target.message,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
    LRESULT(1)
}

unsafe extern "system" fn capture_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let next = || {
        // SAFETY: the parameters are the ones Windows passed.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    };
    let Some(dialog) = CAPTURE.get() else {
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
    if windows {
        press_unassigned_key();
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
    // SAFETY: the message carries no pointer; a closed dialog only fails.
    let _ = unsafe {
        PostMessageW(
            Some(HWND(dialog as *mut _)),
            CAPTURED,
            WPARAM(usize::from(key.0)),
            LPARAM(held),
        )
    };
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
