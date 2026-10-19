//! The icon in the notification area, its menu and its notification (DESIGN.md:
//! Window lifetime).

use super::Hwnd;
use super::menu::Menu;
use crate::core::commands::MenuNode;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{LIM_SMALL, LoadIconMetric};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFY_ICON_DATA_FLAGS, NOTIFY_ICON_MESSAGE,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, WM_CONTEXTMENU, WM_NULL};
use windows::core::PCWSTR;

/// The private message that the icon sends the window
/// ([`super::window::Message::Tray`]).
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

/// The icon, removed on drop.
pub(crate) struct TrayIcon {
    window: Hwnd,
    icon: HICON,
    tip: String,
}

impl TrayIcon {
    /// Adds the icon `icon`, a resource of this module, for `window`, with
    /// the tooltip `tip`.
    pub(crate) fn add(window: Hwnd, icon: u16, tip: &str) -> windows::core::Result<TrayIcon> {
        // SAFETY: the icon is a resource of this module, loaded at the size
        // the notification area shows; DestroyIcon frees it on drop.
        let icon = unsafe {
            LoadIconMetric(
                GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
                PCWSTR(usize::from(icon) as *const u16),
                LIM_SMALL,
            )
        }?;
        let tray = TrayIcon {
            window,
            icon,
            tip: tip.to_owned(),
        };
        tray.add_again();
        Ok(tray)
    }

    /// Adds the icon again, as after Explorer restarted, which drops every
    /// icon (`TaskbarCreated`).
    pub(crate) fn add_again(&self) {
        let mut data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
        copy(&mut data.szTip, &self.tip);
        notify_icon(NIM_ADD, &data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        notify_icon(NIM_SETVERSION, &data);
    }

    /// Shows a Windows notification from the icon.
    pub(crate) fn notify(&self, title: &str, text: &str) {
        let mut data = self.data(NIF_INFO);
        copy(&mut data.szInfoTitle, title);
        copy(&mut data.szInfo, text);
        data.dwInfoFlags = NIIF_INFO;
        notify_icon(NIM_MODIFY, &data);
    }

    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window.0,
            uID: 1,
            uFlags: flags,
            uCallbackMessage: TRAY,
            hIcon: self.icon,
            ..Default::default()
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        notify_icon(NIM_DELETE, &self.data(NOTIFY_ICON_DATA_FLAGS(0)));
        // SAFETY: the icon is this value's own, and the shell holds its own
        // copy no more.
        let _ = unsafe { DestroyIcon(self.icon) };
    }
}

fn notify_icon(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) {
    // SAFETY: `data` is complete, with its size set, and outlives the call,
    // which copies it. A failure, such as no taskbar yet, leaves no icon,
    // which TaskbarCreated adds later.
    let _ = unsafe { Shell_NotifyIconW(message, data) };
}

/// Copies `text` into `field`, cut to fit with its NUL.
fn copy(field: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(field.len() - 1).collect();
    field[..units.len()].copy_from_slice(&units);
    field[units.len()] = 0;
}

/// Shows the icon's menu of `items` at `point`, with `default` in bold, and
/// returns the chosen item's ID. The window comes to the front first and
/// is sent a message after, as the `Shell_NotifyIconW` documentation
/// requires, so the menu closes on a click elsewhere.
pub(crate) fn track_menu(
    window: Hwnd,
    items: &[MenuNode],
    point: (i32, i32),
    default: Option<u16>,
) -> Option<u16> {
    window.set_foreground();
    let chosen = Menu::track_popup(window, items, point, &[], default);
    window.post(WM_NULL);
    chosen
}
