//! Modal dialogs from the .rc templates, their pages, and message boxes.
//! Windows runs a template dialog's modal loop, its Tab order, `Enter` and
//! `Esc`, and scales it to its monitor's DPI. [`run_modal`] sets the
//! dialog's title and captions, focuses the control that its
//! [`DialogProc`] names, keeps the fonts the dialog's controls use until it
//! ends, and ends it on an OK or Cancel that its procedure leaves alone. A
//! page, such as Settings > General, is a child dialog of its own template
//! inside the dialog ([`create_page`]), whose controls the dialog's
//! procedure handles.

use super::gdi::Font;
use super::{Hwnd, wide};
use crate::core::captions::DialogCaptions;
use crate::core::ids::DialogId;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    DTN_DATETIMECHANGE, LVN_ITEMCHANGED, NM_CLICK, NM_RETURN, NMHDR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CBN_SELCHANGE, CreateDialogParamW, DialogBoxParamW, EN_CHANGE, EndDialog,
    GetDlgItem, GetParent, GetWindowLongPtrW, IDOK, MB_ICONERROR, MB_OK, MessageBoxW, SendMessageW,
    SetWindowLongPtrW, WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY, WM_TIMER,
};
use windows::core::{HSTRING, PCWSTR};

/// `DWLP_USER`: the dialog's pointer-sized slot after its result and its
/// procedure.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(2 * size_of::<isize>() as i32);

/// The private message that tells a dialog that the main window holds a
/// reply for it ([`DialogMessage::Reply`]).
const REPLY: u32 = super::wake::WAKE + 6;

/// A dialog message decoded for its handler.
pub(crate) enum DialogMessage {
    /// A click on the button or check box `id`, or OK and Cancel, which are
    /// 1 and 2, from `Enter` and `Esc`.
    Command {
        id: u16,
    },
    /// The control `id` holds another value: an edit control's or a hotkey
    /// control's text, a list's selection, or a date picker's date or check
    /// box.
    Edited {
        id: u16,
    },
    /// A click or `Enter` on the SysLink control `id`.
    LinkActivated {
        id: u16,
    },
    /// The dialog's timer `id`.
    Timer {
        id: usize,
    },
    /// [`post_reply`] said that a reply waits for the dialog.
    Reply,
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
                BN_CLICKED => DialogMessage::Command { id },
                // A list box's LBN_SELCHANGE is a combo box's CBN_SELCHANGE.
                EN_CHANGE | CBN_SELCHANGE => DialogMessage::Edited { id },
                _ => DialogMessage::Other,
            }
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lparam points to an NMHDR.
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            let id = header.idFrom as u16;
            match header.code {
                NM_CLICK | NM_RETURN => DialogMessage::LinkActivated { id },
                DTN_DATETIMECHANGE | LVN_ITEMCHANGED => DialogMessage::Edited { id },
                _ => DialogMessage::Other,
            }
        }
        WM_TIMER => DialogMessage::Timer { id: wparam.0 },
        REPLY => DialogMessage::Reply,
        _ => DialogMessage::Other,
    };
    let command = match message {
        DialogMessage::Command { id, .. } => Some(id),
        DialogMessage::Edited { .. }
        | DialogMessage::LinkActivated { .. }
        | DialogMessage::Timer { .. }
        | DialogMessage::Reply
        | DialogMessage::Other => None,
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

/// Creates the hidden page `page` inside `dialog`, with its captions set.
/// The dialog's procedure receives the page's `WM_COMMAND` and `WM_NOTIFY`,
/// so one [`DialogProc`] handles every page; the pages' control IDs differ
/// from each other's and from the dialog's. The page lives until its
/// dialog ends.
pub(crate) fn create_page(
    dialog: Hwnd,
    page: DialogId,
    captions: &DialogCaptions,
) -> windows::core::Result<Hwnd> {
    // SAFETY: the template is a resource of this module, a child dialog
    // whose procedure reads no state.
    let hwnd = unsafe {
        CreateDialogParamW(
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            PCWSTR(usize::from(page.id()) as *const u16),
            Some(dialog.0),
            Some(page_procedure),
            LPARAM(0),
        )
    }?;
    let page = Hwnd(hwnd);
    for (id, text) in &captions.items {
        if let Some(control) = item(page, *id) {
            control.set_text(text);
        }
    }
    Ok(page)
}

unsafe extern "system" fn page_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match msg {
        // False: the hidden page takes no focus when it is created.
        WM_INITDIALOG => 0,
        WM_COMMAND | WM_NOTIFY => {
            // SAFETY: the dialog receives the message as the page did, with
            // its pointer valid while this call runs.
            unsafe {
                match GetParent(hwnd) {
                    Ok(dialog) => SendMessageW(dialog, msg, Some(wparam), Some(lparam)).0,
                    Err(_) => 0,
                }
            }
        }
        _ => 0,
    }
}

/// Tells `dialog` that the main window holds a reply for it, such as Use
/// Total Commander's, which the dialog then reads.
pub(crate) fn post_reply(dialog: Hwnd) {
    dialog.post(REPLY);
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
/// thread, as the panic hook needs (DESIGN.md: window rule 11).
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
