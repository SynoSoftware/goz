//! The main window's common controls. Each is created here, so a later
//! theme change has one place to hook in (docs/plans/ui-design.md: How
//! normal changes land).

use super::{Hwnd, wide};
use crate::core::layout::Px;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    EM_GETSEL, EM_SETCUEBANNER, EM_SETSEL, ICC_BAR_CLASSES, ICC_LINK_CLASS, ICC_LISTVIEW_CLASSES,
    ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_FMT, LVCF_TEXT,
    LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVM_GETCOLUMNWIDTH, LVM_INSERTCOLUMNW,
    LVM_SETCOLUMNW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS,
    SB_SETPARTS, SBARS_SIZEGRIP, STATUSCLASSNAMEW, SetWindowTheme, WC_LISTVIEWW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, BS_AUTOCHECKBOX, CB_ADDSTRING, CB_RESETCONTENT, CB_SETCURSEL, CBS_DROPDOWNLIST,
    CreateWindowExW, ES_AUTOHSCROLL, GetWindowRect, HMENU, SendMessageW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COPY, WM_SIZE, WS_CHILD, WS_EX_CLIENTEDGE, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, PWSTR, w};

/// Registers the common control classes the window and its dialogs use.
pub(crate) fn initialize() {
    let classes = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_LINK_CLASS,
    };
    // SAFETY: `classes` is complete.
    let _ = unsafe { InitCommonControlsEx(&classes) };
}

/// Creates a visible child of `parent`; its place comes later from the
/// layout.
fn child(
    parent: Hwnd,
    class: PCWSTR,
    style: u32,
    ex_style: WINDOW_EX_STYLE,
    id: u16,
) -> windows::core::Result<Hwnd> {
    // SAFETY: a child window with an ID in place of a menu, as Windows
    // documents it.
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            class,
            PCWSTR::null(),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
            0,
            0,
            0,
            0,
            Some(parent.0),
            Some(HMENU(usize::from(id) as *mut _)),
            None,
            None,
        )
    }?;
    Ok(Hwnd(hwnd))
}

/// Sends a message whose parameters are plain values.
fn send(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: neither parameter is a pointer, so the control reads no memory
    // of this process through them.
    unsafe { SendMessageW(hwnd.0, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

/// Sends a message whose `lparam` points to data the control reads.
///
/// # Safety
/// `lparam` points to a valid value of the type `message` expects, which
/// lives until this synchronous call returns.
unsafe fn send_pointer<T>(hwnd: Hwnd, message: u32, wparam: usize, lparam: *const T) -> isize {
    // SAFETY: the caller keeps `lparam` valid for the call.
    unsafe {
        SendMessageW(
            hwnd.0,
            message,
            Some(WPARAM(wparam)),
            Some(LPARAM(lparam as isize)),
        )
    }
    .0
}

impl Hwnd {
    pub(crate) fn set_enabled(self, enabled: bool) {
        // SAFETY: plain values.
        let _ = unsafe { EnableWindow(self.0, enabled) };
    }
}

/// The search box.
#[derive(Clone, Copy)]
pub(crate) struct Edit(pub(crate) Hwnd);

impl Edit {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Edit> {
        child(
            parent,
            w!("EDIT"),
            WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            id,
        )
        .map(Edit)
    }

    /// The grey hint the empty box shows, also while it has the focus.
    pub(crate) fn set_cue(self, text: &str) {
        let text = wide(text);
        // SAFETY: `text` is a NUL-terminated string that outlives the call.
        unsafe { send_pointer(self.0, EM_SETCUEBANNER, 1, text.as_ptr()) };
    }

    pub(crate) fn has_selected_text(self) -> bool {
        let selection = send(self.0, EM_GETSEL, 0, 0) as usize;
        selection & 0xffff != (selection >> 16) & 0xffff
    }

    pub(crate) fn select_all(self) {
        send(self.0, EM_SETSEL, 0, -1);
    }

    /// Copies the selected text, as `Ctrl+C` does in a text box.
    pub(crate) fn copy(self) {
        send(self.0, WM_COPY, 0, 0);
    }
}

/// A static text, such as a label before its control.
#[derive(Clone, Copy)]
pub(crate) struct Label(pub(crate) Hwnd);

impl Label {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        child(parent, w!("STATIC"), 0, WINDOW_EX_STYLE(0), id).map(Label)
    }
}

/// A drop-down list, such as the Match list.
#[derive(Clone, Copy)]
pub(crate) struct DropDownList(pub(crate) Hwnd);

impl DropDownList {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<DropDownList> {
        child(
            parent,
            w!("COMBOBOX"),
            WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(DropDownList)
    }

    /// Replaces the items and selects `selected`. A selection set by a
    /// message sends no `CBN_SELCHANGE`, so nothing reacts to it.
    pub(crate) fn set_items(self, items: &[&str], selected: usize) {
        send(self.0, CB_RESETCONTENT, 0, 0);
        for item in items {
            let item = wide(item);
            // SAFETY: `item` is a NUL-terminated string that outlives the
            // call; the list keeps its own copy.
            unsafe { send_pointer(self.0, CB_ADDSTRING, 0, item.as_ptr()) };
        }
        send(self.0, CB_SETCURSEL, selected, 0);
    }
}

/// A check box.
#[derive(Clone, Copy)]
pub(crate) struct CheckBox(pub(crate) Hwnd);

impl CheckBox {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<CheckBox> {
        child(
            parent,
            w!("BUTTON"),
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(CheckBox)
    }

    pub(crate) fn set_checked(self, checked: bool) {
        send(self.0, BM_SETCHECK, usize::from(checked), 0);
    }
}

/// The results list: a list view in report and virtual mode, drawn as File
/// Explorer draws its list (spec: Look).
#[derive(Clone, Copy)]
pub(crate) struct ListView(pub(crate) Hwnd);

/// One column as the list shows it.
pub(crate) struct ListColumn<'a> {
    pub(crate) header: &'a str,
    pub(crate) width: Px,
    pub(crate) right_aligned: bool,
}

impl ListView {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<ListView> {
        let list = child(
            parent,
            WC_LISTVIEWW,
            WS_TABSTOP.0 | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS,
            WINDOW_EX_STYLE(0),
            id,
        )?;
        let extended = LVS_EX_DOUBLEBUFFER | LVS_EX_FULLROWSELECT | LVS_EX_HEADERDRAGDROP;
        send(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(list.0, w!("Explorer"), PCWSTR::null()) };
        Ok(ListView(list))
    }

    pub(crate) fn insert_column(self, index: usize, column: &ListColumn) {
        self.column_message(LVM_INSERTCOLUMNW, index, column.header, Some(column));
    }

    /// Sets a column's header text and keeps its width and order.
    pub(crate) fn set_header(self, index: usize, header: &str) {
        self.column_message(LVM_SETCOLUMNW, index, header, None);
    }

    fn column_message(self, message: u32, index: usize, header: &str, column: Option<&ListColumn>) {
        let mut header = wide(header);
        let mut item = LVCOLUMNW {
            mask: LVCF_TEXT,
            pszText: PWSTR(header.as_mut_ptr()),
            ..Default::default()
        };
        if let Some(column) = column {
            item.mask |= LVCF_WIDTH | LVCF_FMT;
            item.cx = column.width.0;
            item.fmt = if column.right_aligned {
                LVCFMT_RIGHT
            } else {
                LVCFMT_LEFT
            };
        }
        // SAFETY: `item` and the header text it points to outlive the call,
        // which copies them.
        unsafe { send_pointer(self.0, message, index, &raw const item) };
    }

    pub(crate) fn column_width(self, index: usize) -> Px {
        Px(send(self.0, LVM_GETCOLUMNWIDTH, index, 0) as i32)
    }

    pub(crate) fn set_column_width(self, index: usize, width: Px) {
        send(self.0, LVM_SETCOLUMNWIDTH, index, width.0 as isize);
    }
}

/// The status bar, which sizes itself to its font at the window's bottom.
#[derive(Clone, Copy)]
pub(crate) struct StatusBar(pub(crate) Hwnd);

impl StatusBar {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<StatusBar> {
        child(
            parent,
            STATUSCLASSNAMEW,
            SBARS_SIZEGRIP,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(StatusBar)
    }

    /// Fits the status bar to the parent's new size and returns its height.
    pub(crate) fn fit(self) -> Px {
        send(self.0, WM_SIZE, 0, 0);
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: `rect` is a local the call fills in.
        let _ = unsafe { GetWindowRect(self.0.0, &mut rect) };
        Px(rect.bottom - rect.top)
    }

    /// Sets the widths of the first parts; the last part takes the rest.
    pub(crate) fn set_parts(self, widths: &[Px]) {
        let mut right = 0;
        let mut edges: Vec<i32> = widths
            .iter()
            .map(|w| {
                right += w.0;
                right
            })
            .collect();
        edges.push(-1);
        // SAFETY: `edges` holds the `edges.len()` integers the message reads,
        // and outlives the call.
        unsafe { send_pointer(self.0, SB_SETPARTS, edges.len(), edges.as_ptr()) };
    }
}
