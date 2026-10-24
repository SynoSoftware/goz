//! Explorer calls retain their callback endpoint and icon for their whole
//! lifetime; neither a settings change nor Exit waits for Explorer.

use super::{Endpoint, TRAY};
use crate::win::work::OffThread;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{LIM_SMALL, LoadIconMetric};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFY_ICON_DATA_FLAGS, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};
use windows::core::PCWSTR;

pub(crate) struct Notice {
    pub(crate) title: String,
    pub(crate) text: String,
}

pub(crate) struct Update {
    pub(crate) enabled: bool,
    pub(crate) tip: String,
    pub(crate) restart: bool,
    pub(crate) notice: Option<Notice>,
}

pub(crate) struct Session {
    endpoint: isize,
    resource: u16,
    icon: Option<isize>,
    enabled: bool,
    tip: String,
}

impl Session {
    pub(crate) fn new(endpoint: &Endpoint, resource: u16) -> Self {
        Self {
            endpoint: endpoint.window,
            resource,
            icon: None,
            enabled: false,
            tip: String::new(),
        }
    }

    pub(crate) fn update(&mut self, update: Update, worker: &OffThread) {
        if !update.enabled {
            self.remove(worker);
            return;
        }
        if self.icon.is_none() {
            // SAFETY: the identifier names a module resource; this Session
            // owns the returned icon until all Explorer calls have returned.
            self.icon = unsafe {
                LoadIconMetric(
                    GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
                    PCWSTR(usize::from(self.resource) as *const u16),
                    LIM_SMALL,
                )
            }
            .ok()
            .map(|icon| icon.0 as isize);
        }
        if self.icon.is_none() {
            return;
        }
        if !self.enabled || update.restart {
            let mut data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
            copy(&mut data.szTip, &update.tip);
            notify_icon(NIM_ADD, &data, worker);
            self.enabled = true;
            self.tip = update.tip;
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            notify_icon(NIM_SETVERSION, &data, worker);
        } else if self.tip != update.tip {
            let mut data = self.data(NIF_TIP);
            copy(&mut data.szTip, &update.tip);
            notify_icon(NIM_MODIFY, &data, worker);
            self.tip = update.tip;
        }
        if let Some(notice) = update.notice {
            let mut data = self.data(NIF_INFO);
            copy(&mut data.szInfoTitle, &notice.title);
            copy(&mut data.szInfo, &notice.text);
            data.dwInfoFlags = NIIF_INFO;
            notify_icon(NIM_MODIFY, &data, worker);
        }
    }

    pub(crate) fn remove(&mut self, worker: &OffThread) {
        if self.enabled {
            notify_icon(NIM_DELETE, &self.data(NOTIFY_ICON_DATA_FLAGS(0)), worker);
            self.enabled = false;
        }
    }

    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(self.endpoint as *mut _),
            uID: 1,
            uFlags: flags,
            uCallbackMessage: TRAY,
            hIcon: HICON(self.icon.unwrap_or_default() as *mut _),
            ..Default::default()
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(icon) = self.icon.take() {
            // SAFETY: Session owns this loaded icon, and moves through one
            // serialized worker slot, so no native call still borrows it.
            let _ = unsafe { DestroyIcon(HICON(icon as *mut _)) };
        }
    }
}

fn notify_icon(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW, _worker: &OffThread) {
    // SAFETY: the complete data and owned icon outlive this copying call;
    // its message-only callback window is retained until process teardown.
    let _ = unsafe { Shell_NotifyIconW(message, data) };
}

fn copy(field: &mut [u16], text: &str) {
    for (index, unit) in text.encode_utf16().take(field.len() - 1).enumerate() {
        field[index] = unit;
    }
}
