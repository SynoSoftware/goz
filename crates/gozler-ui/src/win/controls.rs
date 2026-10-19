//! The common controls of the main window, which are created here, and the
//! wrappers of the dialogs' controls, which their templates create.

use super::gdi::{ImageList, draw_text};
use super::keys::{is_extended, key_of, virtual_key};
use super::notify::ControlEvent;
use super::{Hwnd, wide};
use crate::core::commands::Keystroke;
use crate::core::layout::{Px, list_box_text_inset};
use std::cell::RefCell;
use std::ops::Range;
use windows::Win32::Foundation::{
    COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, ClientToScreen,
    DT_CALCRECT, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DrawFocusRect, FillRect,
    GetDC, GetSysColor, GetSysColorBrush, HGDIOBJ, ReleaseDC, ScreenToClient, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    BST_CHECKED, DRAWITEMSTRUCT, DTM_GETSYSTEMTIME, DTM_SETSYSTEMTIME, EM_GETSEL, EM_SETCUEBANNER,
    EM_SETSEL, GDT_NONE, GDT_VALID, HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW,
    HDM_SETITEMW, HKCOMB_A, HKCOMB_C, HKCOMB_NONE, HKCOMB_S, HKM_GETHOTKEY, HKM_SETHOTKEY,
    HKM_SETRULES, HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_EXT, HOTKEYF_SHIFT, ICC_BAR_CLASSES,
    ICC_DATE_CLASSES, ICC_HOTKEY_CLASS, ICC_LINK_CLASS, ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES,
    INITCOMMONCONTROLSEX, InitCommonControlsEx, LIST_VIEW_ITEM_STATE_FLAGS, LVCF_FMT, LVCF_TEXT,
    LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVHITTESTINFO, LVIF_TEXT, LVIR_BOUNDS,
    LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS, LVM_DELETECOLUMN, LVM_ENSUREVISIBLE,
    LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE, LVM_GETHEADER,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETTOPINDEX,
    LVM_HITTEST, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_REDRAWITEMS, LVM_SETCOLUMNORDERARRAY,
    LVM_SETCOLUMNW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST,
    LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVNI_FOCUSED, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_EX_LABELTIP,
    LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS, LVSICF_NOSCROLL,
    LVSIL_SMALL, ODS_FLAGS, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED, ODT_LISTBOX, SB_SETPARTS,
    SB_SETTEXTW, SBARS_SIZEGRIP, STATUSCLASSNAMEW, SetWindowTheme, TOOLTIPS_CLASSW, TTF_ABSOLUTE,
    TTF_SUBCLASS, TTF_TRACK, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE,
    TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTS_BALLOON, TTS_NOPREFIX, TTTOOLINFOW, WC_LISTVIEWW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_GETCOUNT, CB_GETCURSEL, CB_GETDROPPEDSTATE, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
    CB_SETCURSEL, CBN_CLOSEUP, CBN_SELCHANGE, CBN_SELENDCANCEL, CBN_SELENDOK, CBS_DROPDOWNLIST,
    CreateWindowExW, EN_CHANGE, EN_SETFOCUS, ES_AUTOHSCROLL, ES_READONLY, GetSystemMetrics,
    GetWindowRect, HMENU, LB_ADDSTRING, LB_GETCOUNT, LB_GETCURSEL, LB_GETITEMRECT, LB_GETTEXT,
    LB_GETTEXTLEN, LB_ITEMFROMPOINT, LB_RESETCONTENT, LB_SETCURSEL, LB_SETITEMHEIGHT,
    LBN_SELCHANGE, LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LBS_OWNERDRAWFIXED,
    SM_CXVSCROLL, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPY, WM_GETFONT, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_SIZE, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_EX_TOPMOST, WS_POPUP,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, PWSTR, w};

/// Registers the common control classes the window and its dialogs use.
pub(crate) fn initialize() {
    let classes = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES
            | ICC_LISTVIEW_CLASSES
            | ICC_BAR_CLASSES
            | ICC_LINK_CLASS
            | ICC_DATE_CLASSES
            | ICC_HOTKEY_CLASS,
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

impl ControlEvent {
    /// Whether `control` sent this with the notification code `code`.
    fn is(self, control: Hwnd, code: u32) -> bool {
        self.from == control && self.code == code
    }
}

/// The texts of a list's items, read with the list's count, length and
/// text messages, such as `CB_GETCOUNT`, `CB_GETLBTEXTLEN` and
/// `CB_GETLBTEXT`.
fn item_texts(list: Hwnd, count: u32, length: u32, text: u32) -> Vec<String> {
    (0..send(list, count, 0, 0).max(0) as usize)
        .map(|index| item_text(list, length, text, index))
        .collect()
}

/// The text of the item at `index`, read with the list's length and text
/// messages.
fn item_text(list: Hwnd, length: u32, text: u32, index: usize) -> String {
    let length = send(list, length, index, 0).max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: the buffer holds the item's text and its NUL.
    let copied = unsafe { send_pointer(list, text, index, buffer.as_mut_ptr()) };
    String::from_utf16_lossy(&buffer[..copied.clamp(0, length as isize) as usize])
}

/// A text box: the search box, or an inspector value.
#[derive(Clone, Copy)]
pub(crate) struct Edit(Hwnd);

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

    /// A value that reads as text but whose every part the person can
    /// select and copy, as File Explorer's Properties shows a location.
    pub(crate) fn create_read_only(parent: Hwnd, id: u16) -> windows::core::Result<Edit> {
        child(
            parent,
            w!("EDIT"),
            WS_TABSTOP.0 | (ES_AUTOHSCROLL | ES_READONLY) as u32,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Edit)
    }

    /// The edit control `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<Edit> {
        super::dialog::item(dialog, id).map(Edit)
    }

    /// Whether `event` says that the box got the focus.
    pub(crate) fn focused(self, event: ControlEvent) -> bool {
        event.is(self.0, EN_SETFOCUS)
    }

    /// The grey hint the empty box shows, also while it has the focus.
    pub(crate) fn set_cue(self, text: &str) {
        let text = wide(text);
        // SAFETY: `text` is a NUL-terminated string that outlives the call.
        unsafe { send_pointer(self.0, EM_SETCUEBANNER, 1, text.as_ptr()) };
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Whether `event` says that the text changed.
    pub(crate) fn changed(self, event: ControlEvent) -> bool {
        event.is(self.0, EN_CHANGE)
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
        self.0.text()
    }
}

/// A static text, such as a label before its control.
#[derive(Clone, Copy)]
pub(crate) struct Label(Hwnd);

impl Label {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        child(parent, w!("STATIC"), 0, WINDOW_EX_STYLE(0), id).map(Label)
    }

    /// A text that wraps at word ends and shows `&` as itself, such as a
    /// message that can quote the typed search.
    pub(crate) fn create_message(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        child(parent, w!("STATIC"), SS_NOPREFIX, WINDOW_EX_STYLE(0), id).map(Label)
    }

    /// One line, centred in its height, that shows `&` as itself and ends
    /// in an ellipsis when it is too long, such as a file's name.
    pub(crate) fn create_heading(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        const SS_CENTERIMAGE: u32 = 0x200;
        const SS_ENDELLIPSIS: u32 = 0x4000;
        child(
            parent,
            w!("STATIC"),
            SS_NOPREFIX | SS_CENTERIMAGE | SS_ENDELLIPSIS,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Label)
    }
}

/// The static control styles, which the windows crate has only in a feature
/// for system services.
const SS_NOPREFIX: u32 = 0x80;

/// A push button.
#[derive(Clone, Copy)]
pub(crate) struct Button(Hwnd);

impl Button {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn clicked(self, event: ControlEvent) -> bool {
        event.is(self.0, BN_CLICKED)
    }

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
pub(crate) struct DropDownList(Hwnd);

/// How a drop-down list's open list closed: on an item, or without a
/// choice, such as with `Esc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListClosing {
    Chosen,
    Cancelled,
}

impl DropDownList {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// The drop-down list `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<DropDownList> {
        super::dialog::item(dialog, id).map(DropDownList)
    }

    /// Whether `event` says that another item is selected, by the person's
    /// choice or by an arrow key on the closed list.
    pub(crate) fn selection_changed(self, event: ControlEvent) -> bool {
        event.is(self.0, CBN_SELCHANGE)
    }

    /// How the open list is closing, when `event` says that it is; its
    /// `CBN_CLOSEUP` follows.
    pub(crate) fn closing(self, event: ControlEvent) -> Option<ListClosing> {
        if event.is(self.0, CBN_SELENDOK) {
            Some(ListClosing::Chosen)
        } else if event.is(self.0, CBN_SELENDCANCEL) {
            Some(ListClosing::Cancelled)
        } else {
            None
        }
    }

    /// Whether `event` says that the open list closed.
    pub(crate) fn closed(self, event: ControlEvent) -> bool {
        event.is(self.0, CBN_CLOSEUP)
    }

    /// Whether the list is open.
    pub(crate) fn is_open(self) -> bool {
        send(self.0, CB_GETDROPPEDSTATE, 0, 0) != 0
    }

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

    /// Gives the list `items`, unless it has them, and selects `selected`.
    /// A selection set by a message sends no `CBN_SELCHANGE`, so nothing
    /// reacts to it; unchanged items keep an open list open.
    pub(crate) fn set_items<S: AsRef<str>>(self, items: &[S], selected: usize) {
        let shown = item_texts(self.0, CB_GETCOUNT, CB_GETLBTEXTLEN, CB_GETLBTEXT);
        if !shown
            .iter()
            .map(String::as_str)
            .eq(items.iter().map(AsRef::as_ref))
        {
            send(self.0, CB_RESETCONTENT, 0, 0);
            for item in items {
                let item = wide(item.as_ref());
                // SAFETY: `item` is a NUL-terminated string that outlives
                // the call; the list keeps its own copy.
                unsafe { send_pointer(self.0, CB_ADDSTRING, 0, item.as_ptr()) };
            }
        }
        self.select(selected);
    }

    pub(crate) fn selected(self) -> Option<usize> {
        usize::try_from(send(self.0, CB_GETCURSEL, 0, 0)).ok()
    }

    /// Selects `index`, unless it is selected.
    pub(crate) fn select(self, index: usize) {
        if self.selected() != Some(index) {
            send(self.0, CB_SETCURSEL, index, 0);
        }
    }
}

/// A list box that shows every item, such as the filter pane's Kind list.
#[derive(Clone, Copy)]
pub(crate) struct ListBox(Hwnd);

impl ListBox {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// The list box `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<ListBox> {
        super::dialog::item(dialog, id).map(ListBox)
    }

    /// Its height is the layout's, not a whole number of rows.
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<ListBox> {
        ListBox::create_styled(parent, id, 0)
    }

    /// A list of paths, such as the Location list. A path wider than its
    /// row loses its middle (`DT_PATH_ELLIPSIS`), so the drive and the last
    /// folder stay visible, and a tooltip shows it whole. Its parent draws
    /// its items with [`draw_path_item`], and gives it its row height with
    /// [`ListBox::set_item_height`].
    pub(crate) fn create_paths(parent: Hwnd, id: u16) -> windows::core::Result<ListBox> {
        let style = (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32;
        let list = ListBox::create_styled(parent, id, style)?;
        PathTip::attach(list.0)?;
        Ok(list)
    }

    fn create_styled(parent: Hwnd, id: u16, style: u32) -> windows::core::Result<ListBox> {
        child(
            parent,
            w!("LISTBOX"),
            WS_TABSTOP.0 | WS_VSCROLL.0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32 | style,
            WS_EX_CLIENTEDGE,
            id,
        )
        .map(ListBox)
    }

    /// Sets the row height of a list whose parent draws its items, which
    /// a font does not change; a list Windows draws takes it from its font.
    pub(crate) fn set_item_height(self, height: Px) {
        send(self.0, LB_SETITEMHEIGHT, 0, height.0 as isize);
    }

    /// Whether `event` says that the person selected another item.
    pub(crate) fn selection_changed(self, event: ControlEvent) -> bool {
        event.is(self.0, LBN_SELCHANGE)
    }

    /// Gives the list `items`, unless it has them, and selects `selected`,
    /// so a list that does not change keeps its scroll position.
    pub(crate) fn set_items<S: AsRef<str>>(self, items: &[S], selected: usize) {
        let shown = item_texts(self.0, LB_GETCOUNT, LB_GETTEXTLEN, LB_GETTEXT);
        if !shown
            .iter()
            .map(String::as_str)
            .eq(items.iter().map(AsRef::as_ref))
        {
            send(self.0, LB_RESETCONTENT, 0, 0);
            for item in items {
                let item = wide(item.as_ref());
                // SAFETY: `item` is a NUL-terminated string that outlives
                // the call; the list keeps its own copy.
                unsafe { send_pointer(self.0, LB_ADDSTRING, 0, item.as_ptr()) };
            }
        }
        if self.selected() != Some(selected) {
            send(self.0, LB_SETCURSEL, selected, 0);
        }
    }

    pub(crate) fn selected(self) -> Option<usize> {
        usize::try_from(send(self.0, LB_GETCURSEL, 0, 0)).ok()
    }
}

/// Draws the item of `WM_DRAWITEM`'s `lparam` when it belongs to a list
/// from [`ListBox::create_paths`], in the system colours, and returns
/// whether it did.
///
/// # Safety
/// `lparam` is the `lparam` of a `WM_DRAWITEM` that is being handled.
pub(crate) unsafe fn draw_path_item(lparam: LPARAM) -> bool {
    // SAFETY: the caller passes WM_DRAWITEM's lparam, which points to a
    // DRAWITEMSTRUCT that lives while the message is handled.
    let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
    if item.CtlType != ODT_LISTBOX {
        return false;
    }
    let list = Hwnd(item.hwndItem);
    let state = |flag: ODS_FLAGS| item.itemState.0 & flag.0 != 0;
    let (background, foreground) = if state(ODS_SELECTED) {
        (COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT)
    } else {
        (COLOR_WINDOW, COLOR_WINDOWTEXT)
    };
    // SAFETY: the DC and the rectangle are the ones Windows passed for this
    // item.
    unsafe {
        FillRect(item.hDC, &item.rcItem, GetSysColorBrush(background));
        // An empty list draws only its focus rectangle.
        if item.itemID != u32::MAX {
            let text = item_text(list, LB_GETTEXTLEN, LB_GETTEXT, item.itemID as usize);
            let mut rect = path_text_rect(list, item.rcItem);
            SetTextColor(item.hDC, COLORREF(GetSysColor(foreground)));
            SetBkMode(item.hDC, TRANSPARENT);
            draw_text(
                item.hDC,
                &text,
                &mut rect,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_PATH_ELLIPSIS,
            );
        }
        if state(ODS_FOCUS) && !state(ODS_NOFOCUSRECT) {
            let _ = DrawFocusRect(item.hDC, &item.rcItem);
        }
    }
    true
}

/// Where a path list's row `row` shows its text, which lines up with the
/// text of a list that Windows draws.
fn path_text_rect(list: Hwnd, row: RECT) -> RECT {
    let inset = list_box_text_inset(list.dpi()).0;
    RECT {
        left: row.left + inset,
        right: row.right - inset,
        ..row
    }
}

/// The tooltip of a list from [`ListBox::create_paths`]. Its one tool
/// covers the row under the mouse and holds that row's whole path when
/// the row shortens it, so moving onto another row leaves the tool, and
/// the tip shows the new row's path once the mouse rests.
struct PathTip {
    tip: Hwnd,
    /// The row the tool covers, in the list's client coordinates, and the
    /// path in that row, because after the list scrolls another path has
    /// the same rectangle.
    covered: RefCell<(RECT, String)>,
}

impl PathTip {
    /// Gives `list` its tooltip.
    fn attach(list: Hwnd) -> windows::core::Result<()> {
        // SAFETY: a tooltip window, as Windows documents it. The list is a
        // child window, so Windows makes the list's top-level window the
        // tip's owner, which destroys it.
        let tip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0 | TTS_NOPREFIX),
                0,
                0,
                0,
                0,
                Some(list.0),
                None,
                None,
                None,
            )
        }?;
        let data = Box::into_raw(Box::new(PathTip {
            tip: Hwnd(tip),
            covered: RefCell::new((RECT::default(), String::new())),
        }));
        // The list's procedure is subclassed before the tooltip subclasses
        // it for its tool, so the tooltip sees each mouse move first: a
        // move onto another row leaves the old tool before it covers the
        // new row.
        // SAFETY: `data` lives until the procedure frees it on
        // WM_NCDESTROY, the list's last message.
        if !unsafe { SetWindowSubclass(list.0, Some(path_list_procedure), 0, data as usize) }
            .as_bool()
        {
            // SAFETY: the subclass did not take `data`.
            drop(unsafe { Box::from_raw(data) });
            return Err(windows::core::Error::from_thread());
        }
        let mut empty = wide("");
        let tool = PathTip::tool(list, RECT::default(), &mut empty);
        // SAFETY: `tool` and its text outlive the call, which copies them.
        unsafe { send_pointer(Hwnd(tip), TTM_ADDTOOLW, 0, &raw const tool) };
        Ok(())
    }

    /// The list's one tool, which covers `rect` and shows `text`.
    fn tool(list: Hwnd, rect: RECT, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: list.0,
            uId: 0,
            rect,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..TTTOOLINFOW::default()
        }
    }

    /// Covers the row at `point`, `WM_MOUSEMOVE`'s `lparam`, with the
    /// tool, unless the tool covers it already.
    fn cover(&self, list: Hwnd, point: LPARAM) {
        // The low word is the row, the high word 1 below the last row.
        let hit = send(list, LB_ITEMFROMPOINT, 0, point.0);
        let row = ((hit >> 16) & 0xffff == 0).then_some((hit & 0xffff) as usize);
        let mut rect = RECT::default();
        if let Some(row) = row {
            // SAFETY: `rect` is a local the call fills in.
            unsafe { send_pointer(list, LB_GETITEMRECT, row, &raw mut rect) };
        }
        let path = row
            .map(|row| item_text(list, LB_GETTEXTLEN, LB_GETTEXT, row))
            .unwrap_or_default();
        {
            let (covered_rect, covered_path) = &*self.covered.borrow();
            if *covered_rect == rect && *covered_path == path {
                return;
            }
        }
        // An empty text shows no tip.
        let shown = if shortened(list, &path, rect) {
            path.as_str()
        } else {
            ""
        };
        let mut text = wide(shown);
        *self.covered.borrow_mut() = (rect, path);
        let tool = PathTip::tool(list, rect, &mut text);
        // SAFETY: `tool` and its text outlive each call, which copies them.
        unsafe {
            send_pointer(self.tip, TTM_NEWTOOLRECTW, 0, &raw const tool);
            send_pointer(self.tip, TTM_UPDATETIPTEXTW, 0, &raw const tool);
        }
    }
}

/// Whether the row `row` of a path list shortens `path`.
fn shortened(list: Hwnd, path: &str, row: RECT) -> bool {
    let shown = path_text_rect(list, row);
    let mut measured = RECT::default();
    // SAFETY: the list's DC is released, with its font restored, before
    // the block ends.
    unsafe {
        let dc = GetDC(Some(list.0));
        let font = HGDIOBJ(send(list, WM_GETFONT, 0, 0) as *mut _);
        let previous = SelectObject(dc, font);
        draw_text(
            dc,
            path,
            &mut measured,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, previous);
        ReleaseDC(Some(list.0), dc);
    }
    measured.right > shown.right - shown.left
}

/// The procedure of a list from [`ListBox::create_paths`], before the
/// list's own: it moves the tooltip's tool to the row under the mouse.
unsafe extern "system" fn path_list_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let list = Hwnd(hwnd);
    match msg {
        WM_MOUSEMOVE => {
            // SAFETY: `data` is the PathTip that `attach` gave this
            // subclass, which lives until WM_NCDESTROY.
            unsafe { &*(data as *const PathTip) }.cover(list, lparam);
        }
        WM_NCDESTROY => {
            // SAFETY: the list gets no message after this one, so nothing
            // reads `data` again.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(path_list_procedure), 0);
                drop(Box::from_raw(data as *mut PathTip));
            }
        }
        _ => {}
    }
    // SAFETY: the parameters are the ones Windows passed.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// A date picker with a check box, which a dialog's template declares.
#[derive(Clone, Copy)]
pub(crate) struct DatePicker(Hwnd);

impl DatePicker {
    /// The date picker `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<DatePicker> {
        super::dialog::item(dialog, id).map(DatePicker)
    }

    /// The day it shows, or `None` while its check box is cleared.
    pub(crate) fn day(self) -> Option<jiff::civil::Date> {
        let mut time = SYSTEMTIME::default();
        // SAFETY: `time` is a local the picker fills in.
        let result = unsafe { send_pointer(self.0, DTM_GETSYSTEMTIME, 0, &raw mut time) };
        if result != GDT_VALID.0 as isize {
            return None;
        }
        jiff::civil::Date::new(
            i16::try_from(time.wYear).ok()?,
            i8::try_from(time.wMonth).ok()?,
            i8::try_from(time.wDay).ok()?,
        )
        .ok()
    }

    /// Shows `day` with its check box set, or clears the check box.
    pub(crate) fn set_day(self, day: Option<jiff::civil::Date>) {
        match day {
            Some(day) => {
                let time = SYSTEMTIME {
                    wYear: day.year() as u16,
                    wMonth: day.month() as u16,
                    wDay: day.day() as u16,
                    ..SYSTEMTIME::default()
                };
                // SAFETY: `time` outlives the call, which reads it.
                unsafe {
                    send_pointer(
                        self.0,
                        DTM_SETSYSTEMTIME,
                        GDT_VALID.0 as usize,
                        &raw const time,
                    )
                };
            }
            None => {
                send(self.0, DTM_SETSYSTEMTIME, GDT_NONE.0 as usize, 0);
            }
        }
    }
}

/// A balloon tip that points at a control and says what to enter there
/// (spec: Dialogs). Windows destroys it with its owner.
pub(crate) struct Balloon {
    tip: Hwnd,
    owner: Hwnd,
}

impl Balloon {
    /// A hidden balloon owned by the dialog `owner`.
    pub(crate) fn create(owner: Hwnd) -> windows::core::Result<Balloon> {
        // SAFETY: a tooltip window, as Windows documents it.
        let tip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0 | TTS_NOPREFIX | TTS_BALLOON),
                0,
                0,
                0,
                0,
                Some(owner.0),
                None,
                None,
                None,
            )
        }?;
        let balloon = Balloon {
            tip: Hwnd(tip),
            owner,
        };
        let mut empty = wide("");
        let tool = balloon.tool(&mut empty);
        // SAFETY: `tool` and its text outlive the call, which copies them.
        unsafe { send_pointer(balloon.tip, TTM_ADDTOOLW, 0, &raw const tool) };
        // A width makes the tip wrap its text instead of running across
        // the screen.
        let width = owner.screen_rect().width;
        send(balloon.tip, TTM_SETMAXTIPWIDTH, 0, width.0 as isize);
        Ok(balloon)
    }

    /// The balloon's one tool, which it shows where it is told.
    fn tool(&self, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_TRACK | TTF_ABSOLUTE,
            hwnd: self.owner.0,
            uId: 1,
            lpszText: windows::core::PWSTR(text.as_mut_ptr()),
            ..TTTOOLINFOW::default()
        }
    }

    /// Shows `text` pointing at the bottom of `control`, until
    /// [`Balloon::hide`].
    pub(crate) fn show(&self, control: Hwnd, text: &str) {
        let mut text = wide(text);
        let tool = self.tool(&mut text);
        let rect = control.screen_rect();
        let x = rect.x.0 + rect.width.0 / 2;
        let y = rect.bottom().0;
        // The screen coordinates are signed words.
        let point = (x as u16 as u32 | (y as u16 as u32) << 16) as isize;
        // SAFETY: `tool` and its text outlive each call, which copies them.
        unsafe {
            send_pointer(self.tip, TTM_UPDATETIPTEXTW, 0, &raw const tool);
            send(self.tip, TTM_TRACKPOSITION, 0, point);
            send_pointer(self.tip, TTM_TRACKACTIVATE, 1, &raw const tool);
        }
    }

    pub(crate) fn hide(&self) {
        let mut empty = wide("");
        let tool = self.tool(&mut empty);
        // SAFETY: `tool` outlives the call, which reads it.
        unsafe { send_pointer(self.tip, TTM_TRACKACTIVATE, 0, &raw const tool) };
    }
}

/// A check box.
#[derive(Clone, Copy)]
pub(crate) struct CheckBox(Hwnd);

impl CheckBox {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn clicked(self, event: ControlEvent) -> bool {
        event.is(self.0, BN_CLICKED)
    }

    /// The check box `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<CheckBox> {
        super::dialog::item(dialog, id).map(CheckBox)
    }

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

    /// Checks or clears the box, unless it already shows `checked`.
    pub(crate) fn set_checked(self, checked: bool) {
        if self.is_checked() != checked {
            send(self.0, BM_SETCHECK, usize::from(checked), 0);
        }
    }

    pub(crate) fn is_checked(self) -> bool {
        send(self.0, BM_GETCHECK, 0, 0) == BST_CHECKED.0 as isize
    }
}

/// The results list: a list view in report and virtual mode, drawn as File
/// Explorer draws its list (spec: Look). Only [`ListView::create`] makes
/// one, so its handle is a list view's, which decoding its notifications
/// relies on.
#[derive(Clone, Copy)]
pub(crate) struct ListView(Hwnd);

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

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
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
            let shown = item.fmt.0;
            item.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
            match sorted {
                Some((column, true)) if column == index => item.fmt.0 |= HDF_SORTUP.0,
                Some((column, false)) if column == index => item.fmt.0 |= HDF_SORTDOWN.0,
                _ => {}
            }
            // The window shows the sort after every reply, and a header
            // given its own format still repaints.
            if item.fmt.0 != shown {
                // SAFETY: as above.
                unsafe { send_pointer(header, HDM_SETITEMW, index, &raw const item) };
            }
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

    /// Sets the states `mask` of the row `index`, or of every row for
    /// `usize::MAX`, which is -1, to `state`.
    fn set_state(
        self,
        index: usize,
        mask: LIST_VIEW_ITEM_STATE_FLAGS,
        state: LIST_VIEW_ITEM_STATE_FLAGS,
    ) {
        let item = LVITEMW {
            stateMask: mask,
            state,
            ..Default::default()
        };
        // SAFETY: `item` outlives the call, which reads it.
        unsafe { send_pointer(self.0, LVM_SETITEMSTATE, index, &raw const item) };
    }

    /// Selects and focuses `row` alone, or clears the selection and the
    /// focus for `None`.
    pub(crate) fn select_only(self, row: Option<u64>) {
        let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        self.set_state(usize::MAX, both, LIST_VIEW_ITEM_STATE_FLAGS(0));
        if let Some(row) = row {
            self.set_state(row as usize, both, both);
        }
    }

    /// Selects every row (spec: C11).
    pub(crate) fn select_all(self) {
        self.set_state(usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
    }

    /// The selected rows, as sorted ranges that do not touch. A virtual
    /// list knows them only row by row, so a selection of every row, which
    /// Select all makes, is read without asking for each.
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "a selection is a list of row ranges"
    )]
    pub(crate) fn selected(self) -> Vec<Range<u64>> {
        let selected = send(self.0, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
        if selected == 0 {
            return Vec::new();
        }
        let count = self.count();
        if selected == count {
            return vec![0..count];
        }
        let mut ranges: Vec<Range<u64>> = Vec::new();
        let mut after = -1isize;
        loop {
            let next = send(
                self.0,
                LVM_GETNEXTITEM,
                after as usize,
                LVNI_SELECTED as isize,
            );
            let Ok(row) = u64::try_from(next) else { break };
            match ranges.last_mut() {
                Some(last) if last.end == row => last.end = row + 1,
                _ => ranges.push(row..row + 1),
            }
            after = next;
        }
        ranges
    }

    /// The row at `point`, in screen coordinates, if one is there.
    pub(crate) fn row_at(self, point: (i32, i32)) -> Option<u64> {
        let mut hit = LVHITTESTINFO {
            pt: POINT {
                x: point.0,
                y: point.1,
            },
            ..Default::default()
        };
        // SAFETY: `hit.pt` is a local the call converts in place.
        if !unsafe { ScreenToClient(self.0.0, &mut hit.pt) }.as_bool() {
            return None;
        }
        // SAFETY: `hit` is a local that the list reads and fills in.
        let row = unsafe { send_pointer(self.0, LVM_HITTEST, 0, &raw mut hit) };
        u64::try_from(row).ok()
    }

    /// Where a context menu for the focused row opens, in screen
    /// coordinates: the bottom left of the row, as File Explorer opens it
    /// for Shift+F10. Select all selects rows without focusing one, so
    /// without a focused row the first selected one stands for it. The row
    /// is scrolled into view first.
    pub(crate) fn focused_row_point(self) -> Option<(i32, i32)> {
        let row = [LVNI_FOCUSED, LVNI_SELECTED].into_iter().find_map(|flag| {
            usize::try_from(send(self.0, LVM_GETNEXTITEM, usize::MAX, flag as isize)).ok()
        })?;
        self.ensure_visible(row as u64);
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        // SAFETY: `rect` is a local that the list reads the part from and
        // fills in.
        if unsafe { send_pointer(self.0, LVM_GETITEMRECT, row, &raw mut rect) } == 0 {
            return None;
        }
        let mut point = POINT {
            x: rect.left,
            y: rect.bottom,
        };
        // SAFETY: `point` is a local the call converts in place.
        unsafe { ClientToScreen(self.0.0, &mut point) }
            .as_bool()
            .then_some((point.x, point.y))
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
pub(crate) struct StatusBar(Hwnd);

impl StatusBar {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

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

/// What a hotkey control holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HotkeyValue {
    Empty,
    Keys(Keystroke),
    /// A key that no [`crate::core::commands::Key`] names, such as `;`,
    /// whose name differs between keyboard layouts.
    Unsupported,
}

/// A hotkey control, which shows the keys the person presses, as the
/// Shortcut key field of a Windows shortcut does. A dialog's template
/// declares it.
#[derive(Clone, Copy)]
pub(crate) struct HotkeyControl(Hwnd);

impl HotkeyControl {
    /// The hotkey control `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<HotkeyControl> {
        super::dialog::item(dialog, id).map(HotkeyControl)
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn value(self) -> HotkeyValue {
        // The key's code is the low byte; its modifiers are the high one.
        let word = send(self.0, HKM_GETHOTKEY, 0, 0) as u32 & 0xffff;
        let (code, flags) = (word & 0xff, word >> 8);
        if code == 0 {
            return HotkeyValue::Empty;
        }
        match key_of(code as u16) {
            Some(key) => HotkeyValue::Keys(Keystroke {
                ctrl: flags & HOTKEYF_CONTROL != 0,
                alt: flags & HOTKEYF_ALT != 0,
                shift: flags & HOTKEYF_SHIFT != 0,
                key,
            }),
            None => HotkeyValue::Unsupported,
        }
    }

    /// Shows `keystroke`, or nothing.
    pub(crate) fn set(self, keystroke: Option<Keystroke>) {
        let word = keystroke.map_or(0, |keystroke| {
            let mut flags = 0;
            for (held, flag) in [
                (keystroke.ctrl, HOTKEYF_CONTROL),
                (keystroke.alt, HOTKEYF_ALT),
                (keystroke.shift, HOTKEYF_SHIFT),
                (is_extended(keystroke.key), HOTKEYF_EXT),
            ] {
                if held {
                    flags |= flag;
                }
            }
            u32::from(virtual_key(keystroke.key)) | flags << 8
        });
        send(self.0, HKM_SETHOTKEY, word as usize, 0);
    }

    /// With `required`, the control turns a key with fewer than two of
    /// `Ctrl`, `Alt` and `Shift` into `Ctrl+Alt` and the key; without it,
    /// it takes any key.
    pub(crate) fn require_two_modifiers(self, required: bool) {
        let (invalid, replacement) = if required {
            (
                HKCOMB_NONE | HKCOMB_S | HKCOMB_C | HKCOMB_A,
                HOTKEYF_CONTROL | HOTKEYF_ALT,
            )
        } else {
            (0, 0)
        };
        send(self.0, HKM_SETRULES, invalid as usize, replacement as isize);
    }
}

/// A list view in report mode with its rows in it, which a dialog's
/// template declares, such as Settings > Keyboard's commands.
#[derive(Clone, Copy)]
pub(crate) struct ReportList(Hwnd);

impl ReportList {
    /// The list view `id` of a dialog whose template declares one in
    /// report mode.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<ReportList> {
        super::dialog::item(dialog, id).map(ReportList)
    }

    /// Gives the list File Explorer's look and `columns`, each a header,
    /// its share of the list's width in percent, and whether its cells are
    /// right-aligned, as numbers are.
    pub(crate) fn set_columns(self, columns: &[(&str, i32, bool)]) {
        let extended = LVS_EX_DOUBLEBUFFER | LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP;
        send(
            self.0,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(self.0.0, w!("Explorer"), PCWSTR::null()) };
        // The vertical scroll bar takes its width when the rows fill the
        // list.
        let width = self.0.client_size().width.0 - scroll_bar_width();
        for (index, (header, share, right_aligned)) in columns.iter().enumerate() {
            let mut header = wide(header);
            let column = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if *right_aligned {
                    LVCFMT_RIGHT
                } else {
                    LVCFMT_LEFT
                },
                cx: width * share / 100,
                pszText: PWSTR(header.as_mut_ptr()),
                ..Default::default()
            };
            // SAFETY: `column` and its header outlive the call, which copies
            // them.
            unsafe { send_pointer(self.0, LVM_INSERTCOLUMNW, index, &raw const column) };
        }
    }

    /// Replaces the rows with `rows`, each its cells in the columns' order,
    /// and selects `selected`.
    pub(crate) fn set_rows<R: AsRef<[String]>>(self, rows: &[R], selected: Option<usize>) {
        send(self.0, LVM_DELETEALLITEMS, 0, 0);
        for (index, row) in rows.iter().enumerate() {
            for (column, text) in row.as_ref().iter().enumerate() {
                let mut text = wide(text);
                let item = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: index as i32,
                    iSubItem: column as i32,
                    pszText: PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                };
                let message = if column == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                };
                // SAFETY: `item` and its text outlive the call, which copies
                // them.
                unsafe { send_pointer(self.0, message, index, &raw const item) };
            }
        }
        if let Some(index) = selected.filter(|i| *i < rows.len()) {
            let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
            let item = LVITEMW {
                stateMask: both,
                state: both,
                ..Default::default()
            };
            // SAFETY: `item` outlives the call, which reads it.
            unsafe { send_pointer(self.0, LVM_SETITEMSTATE, index, &raw const item) };
            send(self.0, LVM_ENSUREVISIBLE, index, 0);
        }
    }

    pub(crate) fn selected(self) -> Option<usize> {
        usize::try_from(send(
            self.0,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        ))
        .ok()
    }
}

/// The width of a vertical scroll bar at the system's DPI.
fn scroll_bar_width() -> i32 {
    // SAFETY: plain values.
    unsafe { GetSystemMetrics(SM_CXVSCROLL) }
}
