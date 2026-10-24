//! The icon in the notification area, its menu and its notification (DESIGN.md:
//! Window lifetime).

use super::wake::OpenWaker;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIN_SELECT, NINF_KEY};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE,
    RegisterClassW, SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CONTEXTMENU, WM_NCCREATE,
    WM_NCDESTROY, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

mod work;
pub(crate) use work::{Notice, Session, Update};

/// The private message that the icon sends its callback endpoint.
pub(crate) const TRAY: u32 = super::wake::WAKE + 4;

/// What the person did with the icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrayEvent {
    /// A click, or `Enter` or `Space` on the focused icon.
    Show,
    /// A right-click, or the Menu key or `Shift+F10`, with the point to
    /// show the menu at, in screen coordinates.
    Menu { point: (i32, i32) },
}

impl TrayEvent {
    /// The event of a [`TRAY`] message, as `NOTIFYICON_VERSION_4` sends it:
    /// the event in `lparam`'s low word and the point in `wparam`.
    pub(super) fn decode(wparam: WPARAM, lparam: LPARAM) -> Option<TrayEvent> {
        let low = |v: usize| (v & 0xffff) as u16;
        let high = |v: usize| ((v >> 16) & 0xffff) as u16;
        match u32::from(low(lparam.0 as usize)) {
            NIN_SELECT => Some(TrayEvent::Show),
            event if event == NIN_SELECT | NINF_KEY => Some(TrayEvent::Show),
            WM_CONTEXTMENU => Some(TrayEvent::Menu {
                // The coordinates are signed words.
                point: (
                    i32::from(low(wparam.0) as i16),
                    i32::from(high(wparam.0) as i16),
                ),
            }),
            _ => None,
        }
    }
}

pub(crate) struct Endpoint {
    window: isize,
    events: Receiver<TrayEvent>,
}

struct Callbacks {
    sender: SyncSender<TrayEvent>,
    waker: OpenWaker,
}

impl Endpoint {
    /// Create once on the window's UI thread with its shared delivery lifetime.
    /// The message-only window remains until process teardown: an Explorer call
    /// in flight must never target a reused HWND at Exit.
    pub(crate) fn new(waker: OpenWaker) -> windows::core::Result<Self> {
        // SAFETY: the class procedure has process lifetime; this module owns
        // the class name and creates its windows only on the UI thread.
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: w!("gozler.ui.tray"),
            ..Default::default()
        };
        // SAFETY: RegisterClassW copies this complete class description.
        if unsafe { RegisterClassW(&class) } == 0 {
            let error = windows::core::Error::from_thread();
            if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(error);
            }
        }
        let (sender, events) = sync_channel(8);
        let callbacks = Box::new(Callbacks { sender, waker });
        // SAFETY: callbacks remains live throughout creation. WM_NCCREATE
        // stores its pointer; ownership transfers only after successful creation.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class.lpszClassName,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                Some((&*callbacks as *const Callbacks).cast()),
            )
        }?;
        let _ = Box::into_raw(callbacks);
        Ok(Self {
            window: window.0 as isize,
            events,
        })
    }

    pub(crate) fn take_event(&self) -> Option<TrayEvent> {
        self.events.try_recv().ok()
    }
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: CreateWindowExW supplies a CREATESTRUCTW whose parameter is
        // the live Box from Endpoint::new, borrowed until creation succeeds.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        // SAFETY: this class owns its user-data slot.
        let _ = unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    // SAFETY: only this procedure writes the slot, and the Box remains live
    // until WM_NCDESTROY. The endpoint is never destroyed during execution.
    let callbacks = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut Callbacks;
    if !callbacks.is_null() && message == TRAY {
        // SAFETY: the UI thread owns the endpoint and its stored Box.
        let callbacks = unsafe { &*callbacks };
        if let Some(event) = TrayEvent::decode(wparam, lparam) {
            callbacks
                .waker
                .deliver(|| callbacks.sender.try_send(event).is_ok());
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        // Endpoint::new owns the Box until creation succeeds; an unsuccessful
        // creation can also send WM_NCDESTROY, so process teardown owns cleanup.
        // SAFETY: clearing this class's slot prevents further callback access.
        let _ = unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
    }
    // SAFETY: unhandled messages and their original parameters go to Windows.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
