//! Modal dialogs from the .rc templates, and message boxes. Windows runs a
//! template dialog's modal loop, its Tab order, `Enter` and `Esc`, and
//! scales it to its monitor's DPI.

use super::{Hwnd, wide};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{NM_CLICK, NM_RETURN, NMHDR};
use windows::Win32::UI::WindowsAndMessaging::{
    DialogBoxParamW, EndDialog, GetDlgItem, GetWindowLongPtrW, MB_ICONERROR, MB_OK, MessageBoxW,
    SetWindowLongPtrW, WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY,
};
use windows::core::{HSTRING, PCWSTR};

/// `DWLP_USER`: the dialog's pointer-sized slot after its result and its
/// procedure.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(2 * size_of::<isize>() as i32);

/// A dialog message decoded for its handler.
pub(crate) enum DialogMessage {
    Init,
    Command {
        id: u16,
    },
    /// A click or `Enter` on the SysLink control `id`.
    LinkActivated {
        id: u16,
    },
    Other,
}

/// One dialog's message handling. It answers `true` for a message it
/// handled.
pub(crate) trait DialogProc {
    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool;
}

/// Runs the dialog of template `template` owned by `owner`, until it ends.
/// `state` is borrowed for the dialog's whole life, because this returns
/// only after the dialog closes.
pub(crate) fn run_modal<D: DialogProc>(owner: Hwnd, template: u16, state: &D) -> isize {
    // SAFETY: WM_INITDIALOG stores the pointer to `state`, which outlives
    // the modal loop that this call runs.
    unsafe {
        DialogBoxParamW(
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            PCWSTR(usize::from(template) as *const u16),
            Some(owner.0),
            Some(procedure::<D>),
            LPARAM(std::ptr::from_ref(state) as isize),
        )
    }
}

unsafe extern "system" fn procedure<D: DialogProc>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: lparam is the state pointer that run_modal passed.
        unsafe { SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0) };
    }
    // SAFETY: DWLP_USER holds the state pointer from WM_INITDIALOG, or 0 for
    // the few messages before it.
    let state = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER) } as *const D;
    if state.is_null() {
        return 0;
    }
    let message = match msg {
        WM_INITDIALOG => DialogMessage::Init,
        WM_COMMAND => DialogMessage::Command {
            id: (wparam.0 & 0xffff) as u16,
        },
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lparam points to an NMHDR.
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            match header.code {
                NM_CLICK | NM_RETURN => DialogMessage::LinkActivated {
                    id: header.idFrom as u16,
                },
                _ => DialogMessage::Other,
            }
        }
        _ => DialogMessage::Other,
    };
    // SAFETY: run_modal's borrow keeps the state alive while the dialog runs.
    isize::from(unsafe { &*state }.message(Hwnd(hwnd), message))
}

/// Ends the dialog with `result`.
pub(crate) fn end(dialog: Hwnd, result: isize) {
    // SAFETY: plain values.
    let _ = unsafe { EndDialog(dialog.0, result) };
}

/// The dialog's control `id`.
pub(crate) fn item(dialog: Hwnd, id: u16) -> Option<Hwnd> {
    // SAFETY: plain values.
    unsafe { GetDlgItem(Some(dialog.0), i32::from(id)) }
        .ok()
        .map(Hwnd)
}

/// Shows an error message box. With no owner it blocks only the calling
/// thread, as the panic hook needs (design rule 11).
pub(crate) fn error_box(owner: Option<Hwnd>, text: &str, title: &str) {
    let text = wide(text);
    // SAFETY: both texts are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            owner.map(|o| o.0),
            PCWSTR(text.as_ptr()),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR,
        );
    }
}
