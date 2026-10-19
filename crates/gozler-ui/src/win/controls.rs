//! The main window's common controls. Each is created here, so a later
//! theme change has one place to hook in (docs/plans/ui-design.md: How
//! normal changes land).

use super::gdi::ImageList;
use super::{Hwnd, wide};
use crate::core::layout::Px;
use std::ops::Range;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    BST_CHECKED, EM_GETSEL, EM_SETCUEBANNER, EM_SETSEL, HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT,
    HDITEMW, HDM_GETITEMW, HDM_SETITEMW, ICC_BAR_CLASSES, ICC_LINK_CLASS, ICC_LISTVIEW_CLASSES,
    ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LIST_VIEW_ITEM_STATE_FLAGS,
    LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVIS_FOCUSED,
    LVIS_SELECTED, LVITEMW, LVM_DELETECOLUMN, LVM_ENSUREVISIBLE, LVM_GETCOLUMNORDERARRAY,
    LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE, LVM_GETHEADER, LVM_GETITEMCOUNT, LVM_GETTOPINDEX,
    LVM_INSERTCOLUMNW, LVM_REDRAWITEMS, LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNW,
    LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP,
    LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS,
    LVSICF_NOSCROLL, LVSIL_SMALL, SB_SETPARTS, SB_SETTEXTW, SBARS_SIZEGRIP, STATUSCLASSNAMEW,
    SetWindowTheme, WC_LISTVIEWW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL,
    CB_RESETCONTENT, CB_SETCURSEL, CBS_DROPDOWNLIST, CreateWindowExW, ES_AUTOHSCROLL,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HMENU, SendMessageW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COPY, WM_SIZE, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
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

/// Sends a message whose `lparam` points to data the control reads or fills
/// in.
///
/// # Safety
/// `lparam` points to a valid value of the type `message` expects, which
/// lives until this synchronous call returns, and is writable when the
/// message fills it in.
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
    /// Enables or disables the control. A control already in that state is
    /// left alone, because the window sets every control's state after
    /// every reply.
    pub(crate) fn set_enabled(self, enabled: bool) {
        // SAFETY: plain values.
        unsafe {
            if IsWindowEnabled(self.0).as_bool() != enabled {
                let _ = EnableWindow(self.0, enabled);
            }
        }
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

    pub(crate) fn text(self) -> String {
        // SAFETY: plain values.
        let length = unsafe { GetWindowTextLengthW(self.0.0) };
        let mut buffer = vec![0u16; usize::try_from(length).unwrap_or(0) + 1];
        // SAFETY: the call writes at most the buffer's length.
        let copied = unsafe { GetWindowTextW(self.0.0, &mut buffer) };
        String::from_utf16_lossy(&buffer[..usize::try_from(copied).unwrap_or(0)])
    }
}

/// A static text, such as a label before its control.
#[derive(Clone, Copy)]
pub(crate) struct Label(pub(crate) Hwnd);

impl Label {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        child(parent, w!("STATIC"), 0, WINDOW_EX_STYLE(0), id).map(Label)
    }

    /// A text that wraps at word ends and shows `&` as itself, such as a
    /// message that can quote the typed search.
    pub(crate) fn create_message(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        // SS_NOPREFIX, which the windows crate has only in a feature for
        // system services.
        const SS_NOPREFIX: u32 = 0x80;
        child(parent, w!("STATIC"), SS_NOPREFIX, WINDOW_EX_STYLE(0), id).map(Label)
    }
}

/// A push button.
#[derive(Clone, Copy)]
pub(crate) struct Button(pub(crate) Hwnd);

impl Button {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Button> {
        child(
            parent,
            w!("BUTTON"),
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Button)
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

    pub(crate) fn selected(self) -> Option<usize> {
        usize::try_from(send(self.0, CB_GETCURSEL, 0, 0)).ok()
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

    pub(crate) fn is_checked(self) -> bool {
        send(self.0, BM_GETCHECK, 0, 0) == BST_CHECKED.0 as isize
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
    /// The list clips the controls created before it, which lie above it,
    /// such as the message in its empty area. It shares the system image
    /// list, which it must not destroy.
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<ListView> {
        let list = child(
            parent,
            WC_LISTVIEWW,
            WS_TABSTOP.0
                | WS_CLIPSIBLINGS.0
                | LVS_REPORT
                | LVS_OWNERDATA
                | LVS_SHOWSELALWAYS
                | LVS_SHAREIMAGELISTS,
            WINDOW_EX_STYLE(0),
            id,
        )?;
        // LVS_EX_LABELTIP shows a cut-off cell's whole text in a tooltip.
        let extended =
            LVS_EX_DOUBLEBUFFER | LVS_EX_FULLROWSELECT | LVS_EX_HEADERDRAGDROP | LVS_EX_LABELTIP;
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

    pub(crate) fn delete_column(self, index: usize) {
        send(self.0, LVM_DELETECOLUMN, index, 0);
    }

    /// The column indexes in the order the header shows them, of `count`
    /// columns.
    pub(crate) fn column_order(self, count: usize) -> Vec<usize> {
        let mut order = vec![0i32; count];
        // SAFETY: `order` holds the `count` integers the message writes.
        unsafe { send_pointer(self.0, LVM_GETCOLUMNORDERARRAY, count, order.as_mut_ptr()) };
        order
            .into_iter()
            .map(|i| usize::try_from(i).unwrap_or(0))
            .collect()
    }

    pub(crate) fn set_column_order(self, order: &[usize]) {
        let order: Vec<i32> = order.iter().map(|&i| i as i32).collect();
        // SAFETY: `order` holds the `order.len()` integers the message reads.
        unsafe { send_pointer(self.0, LVM_SETCOLUMNORDERARRAY, order.len(), order.as_ptr()) };
    }

    /// The header control above the rows.
    pub(crate) fn header(self) -> Hwnd {
        Hwnd(windows::Win32::Foundation::HWND(
            send(self.0, LVM_GETHEADER, 0, 0) as *mut _,
        ))
    }

    /// Shows the sort arrow on the column `sorted` of `count` columns,
    /// pointing up when `ascending`, and on no other.
    pub(crate) fn show_sort(self, count: usize, sorted: Option<(usize, bool)>) {
        let header = self.header();
        for index in 0..count {
            let mut item = HDITEMW {
                mask: HDI_FORMAT,
                ..Default::default()
            };
            // SAFETY: `item` is a local that the header fills in, then reads.
            unsafe { send_pointer(header, HDM_GETITEMW, index, &raw mut item) };
            item.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
            match sorted {
                Some((column, true)) if column == index => item.fmt.0 |= HDF_SORTUP.0,
                Some((column, false)) if column == index => item.fmt.0 |= HDF_SORTDOWN.0,
                _ => {}
            }
            // SAFETY: as above.
            unsafe { send_pointer(header, HDM_SETITEMW, index, &raw const item) };
        }
    }

    pub(crate) fn set_small_images(self, images: &ImageList) {
        send(
            self.0,
            LVM_SETIMAGELIST,
            LVSIL_SMALL as usize,
            images.handle(),
        );
    }

    /// Sets the number of rows and redraws them, keeping the scroll
    /// position.
    pub(crate) fn set_count(self, count: u64) {
        send(
            self.0,
            LVM_SETITEMCOUNT,
            usize::try_from(count).unwrap_or(usize::MAX),
            LVSICF_NOSCROLL as isize,
        );
    }

    pub(crate) fn count(self) -> u64 {
        send(self.0, LVM_GETITEMCOUNT, 0, 0).max(0) as u64
    }

    pub(crate) fn ensure_visible(self, row: u64) {
        send(self.0, LVM_ENSUREVISIBLE, row as usize, 0);
    }

    /// Selects and focuses `row` alone, or clears the selection and the
    /// focus for `None`.
    pub(crate) fn select_only(self, row: Option<u64>) {
        let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        let set = |index: usize, state| {
            let item = LVITEMW {
                stateMask: both,
                state,
                ..Default::default()
            };
            // SAFETY: `item` outlives the call, which reads it.
            unsafe { send_pointer(self.0, LVM_SETITEMSTATE, index, &raw const item) };
        };
        // Index -1 is every row.
        set(usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0));
        if let Some(row) = row {
            set(row as usize, both);
        }
    }

    /// Redraws `rows`, whose cells have changed.
    pub(crate) fn redraw_rows(self, rows: Range<u64>) {
        if rows.is_empty() {
            return;
        }
        send(
            self.0,
            LVM_REDRAWITEMS,
            rows.start as usize,
            (rows.end - 1) as isize,
        );
    }

    /// The rows the list shows now, a partly shown last row included.
    pub(crate) fn visible_rows(self) -> Range<u64> {
        let top = send(self.0, LVM_GETTOPINDEX, 0, 0).max(0) as u64;
        let page = send(self.0, LVM_GETCOUNTPERPAGE, 0, 0).max(0) as u64;
        top..top + page + 1
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

    pub(crate) fn set_text(self, part: usize, text: &str) {
        let text = wide(text);
        // SAFETY: `text` is NUL-terminated and outlives the call, which
        // copies it.
        unsafe { send_pointer(self.0, SB_SETTEXTW, part, text.as_ptr()) };
    }
}
