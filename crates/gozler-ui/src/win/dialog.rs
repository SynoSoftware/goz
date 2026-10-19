//! Modal dialogs from the .rc templates, and message boxes. Windows runs a
//! template dialog's modal loop, its Tab order, `Enter` and `Esc`, and
//! scales it to its monitor's DPI. [`run_modal`] sets the dialog's title
//! and captions, focuses the control that its [`DialogProc`] names, keeps
//! the fonts the dialog's controls use until it ends, and ends it on an OK
//! or Cancel that its procedure leaves alone.

use super::gdi::Font;
use super::{Hwnd, wide};
use crate::core::captions::DialogCaptions;
use crate::core::ids::DialogId;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DTN_DATETIMECHANGE, NM_CLICK, NM_RETURN, NMHDR};
use windows::Win32::UI::WindowsAndMessaging::{
    CBN_SELCHANGE, DialogBoxParamW, EN_CHANGE, EndDialog, GetDlgItem, GetWindowLongPtrW, IDOK,
    MB_ICONERROR, MB_OK, MessageBoxW, SetWindowLongPtrW, WINDOW_LONG_PTR_INDEX, WM_COMMAND,
    WM_INITDIALOG, WM_NOTIFY,
};
use windows::core::{HSTRING, PCWSTR};

/// `DWLP_USER`: the dialog's pointer-sized slot after its result and its
/// procedure.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(2 * size_of::<isize>() as i32);

/// A dialog message decoded for its handler.
pub(crate) enum DialogMessage {
    /// `WM_COMMAND` from the control `id`, such as a click on a button; OK
    /// and Cancel are 1 and 2.
    Command {
        id: u16,
    },
    /// A control holds another value: an edit control's text, a drop-down
    /// list's selection, or a date picker's date or check box.
    Edited,
    /// A click or `Enter` on the SysLink control `id`.
    LinkActivated {
        id: u16,
    },
    Other,
}

/// What a dialog sets up when it opens, besides its captions.
pub(crate) struct Opened {
    /// The control the focus starts on.
    pub(crate) focus: u16,
    /// The fonts the dialog gave its controls. They live until the dialog
    /// ends, because `WM_SETFONT` does not take ownership.
    pub(crate) fonts: Vec<Font>,
}

/// One dialog's message handling.
pub(crate) trait DialogProc {
    /// Sets what the captions do not, such as a text built from a fact, a
    /// list's items or a font. Runs after the captions are set.
    fn init(&self, dialog: Hwnd) -> Opened;
    /// Answers `true` for a message it handled. An OK or Cancel that it
    /// leaves ends the dialog with that result.
    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool;
}

/// How a dialog ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogEnd {
    Ok,
    Cancel,
}

/// What the dialog procedure reads through `DWLP_USER` while the dialog
/// runs.
struct Running<'a, D> {
    state: &'a D,
    captions: &'a DialogCaptions,
    fonts: RefCell<Vec<Font>>,
}

/// Runs the dialog `dialog` owned by `owner`, until it ends. `state` is
/// borrowed for the dialog's whole life, because this returns only after
/// the dialog closes. Fails when Windows cannot create the dialog, such as
/// when it runs out of resources.
pub(crate) fn run_modal<D: DialogProc>(
    owner: Hwnd,
    dialog: DialogId,
    captions: &DialogCaptions,
    state: &D,
) -> windows::core::Result<DialogEnd> {
    let running = Running {
        state,
        captions,
        fonts: RefCell::new(Vec::new()),
    };
    // SAFETY: WM_INITDIALOG stores the pointer to `running`, which outlives
    // the modal loop that this call runs, and its fonts outlive the
    // dialog's controls.
    let result = unsafe {
        DialogBoxParamW(
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            PCWSTR(usize::from(dialog.id()) as *const u16),
            Some(owner.0),
            Some(procedure::<D>),
            LPARAM(std::ptr::from_ref(&running) as isize),
        )
    };
    match result {
        // 0 is an owner that is not a window, -1 any other failure.
        0 | -1 => Err(windows::core::Error::from_thread()),
        result if result == IDOK.0 as isize => Ok(DialogEnd::Ok),
        _ => Ok(DialogEnd::Cancel),
    }
}

unsafe extern "system" fn procedure<D: DialogProc>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: lparam is the pointer to the `Running` that run_modal
        // passed.
        unsafe { SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0) };
    }
    // SAFETY: DWLP_USER holds the pointer from WM_INITDIALOG, or 0 for the
    // few messages before it.
    let running = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER) } as *const Running<D>;
    if running.is_null() {
        return 0;
    }
    // SAFETY: run_modal's local keeps the `Running` alive while the dialog
    // runs.
    let running = unsafe { &*running };
    let dialog = Hwnd(hwnd);
    let message = match msg {
        WM_INITDIALOG => {
            open(dialog, running);
            // False: `open` has set the focus.
            return 0;
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as u16;
            match ((wparam.0 >> 16) & 0xffff) as u32 {
                EN_CHANGE | CBN_SELCHANGE => DialogMessage::Edited,
                _ => DialogMessage::Command { id },
            }
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lparam points to an NMHDR.
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            let id = header.idFrom as u16;
            match header.code {
                NM_CLICK | NM_RETURN => DialogMessage::LinkActivated { id },
                DTN_DATETIMECHANGE => DialogMessage::Edited,
                _ => DialogMessage::Other,
            }
        }
        _ => DialogMessage::Other,
    };
    let command = match message {
        DialogMessage::Command { id, .. } => Some(id),
        DialogMessage::Edited | DialogMessage::LinkActivated { .. } | DialogMessage::Other => None,
    };
    if running.state.message(dialog, message) {
        return 1;
    }
    match command {
        Some(1) => end(dialog, DialogEnd::Ok),
        Some(2) => end(dialog, DialogEnd::Cancel),
        _ => return 0,
    }
    1
}

/// Sets the captions, lets the dialog set up the rest, and focuses the
/// control it names.
fn open<D: DialogProc>(dialog: Hwnd, running: &Running<D>) {
    dialog.set_text(&running.captions.title);
    for (id, text) in &running.captions.items {
        if let Some(control) = item(dialog, *id) {
            control.set_text(text);
        }
    }
    let opened = running.state.init(dialog);
    running.fonts.replace(opened.fonts);
    if let Some(control) = item(dialog, opened.focus) {
        control.focus();
    }
}

/// Ends the dialog; [`run_modal`] returns `end`.
pub(crate) fn end(dialog: Hwnd, end: DialogEnd) {
    let result = match end {
        DialogEnd::Ok => 1,
        DialogEnd::Cancel => 2,
    };
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
