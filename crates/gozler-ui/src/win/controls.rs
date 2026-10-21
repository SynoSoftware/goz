//! The common controls of the main window, which are created here, and the
//! wrappers of the dialogs' controls, which their templates create.

use super::gdi::{
    Font, Icon, ImageList, TextMeter, button_images, glyph_icon, glyph_list, icons_shown,
    lone_button_images, system_color,
};
use super::keys::{is_extended, key_of, virtual_key};
use super::notify::ControlEvent;
use super::{Hwnd, wide};
use crate::core::captions::ButtonIcon;
use crate::core::commands::Keystroke;
use crate::core::glyph::Glyph;
use crate::core::layout::column_sizing::{self, ColumnFit};
use crate::core::layout::{self, Dpi, Px, Size, glyph_gap, icon_glyph_height};
use crate::core::panel::difference;
use gozler_core::include::FolderCheck;
use std::ops::Range;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNTEXT, COLOR_WINDOWTEXT, ClientToScreen, MapWindowPoints, ScreenToClient,
};
use windows::Win32::UI::Controls::{
    BCM_GETIMAGELIST, BCM_SETIMAGELIST, BCM_SETSHIELD, BCM_SETSPLITINFO, BCSIF_STYLE, BCSS_NOSPLIT,
    BS_SPLITBUTTON, BST_CHECKED, BUTTON_IMAGELIST, BUTTON_IMAGELIST_ALIGN,
    BUTTON_IMAGELIST_ALIGN_CENTER, BUTTON_IMAGELIST_ALIGN_LEFT, BUTTON_SPLITINFO,
    DTM_GETSYSTEMTIME, DTM_SETSYSTEMTIME, EM_GETMARGINS, EM_GETSEL, EM_SCROLLCARET,
    EM_SETCUEBANNER, EM_SETSEL, GDT_NONE, GDT_VALID, HDF_SORTDOWN, HDF_SORTUP, HDHITTESTINFO,
    HDI_FORMAT, HDITEMW, HDM_GETITEMRECT, HDM_GETITEMW, HDM_HITTEST, HDM_SETITEMW, HIMAGELIST,
    HKM_GETHOTKEY, HKM_SETHOTKEY, HKM_SETRULES, HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_EXT,
    HOTKEYF_SHIFT, HTREEITEM, ICC_BAR_CLASSES, ICC_DATE_CLASSES, ICC_HOTKEY_CLASS, ICC_LINK_CLASS,
    ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, LIST_VIEW_ITEM_STATE_FLAGS, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT,
    LVCFMT_RIGHT, LVCOLUMNW, LVHITTESTINFO, LVIF_IMAGE, LVIF_TEXT, LVIR_BOUNDS, LVIR_LABEL,
    LVIS_DROPHILITED, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS, LVM_DELETECOLUMN,
    LVM_ENSUREVISIBLE, LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE,
    LVM_GETHEADER, LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETITEMTEXTW,
    LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETTOPINDEX, LVM_HITTEST, LVM_INSERTCOLUMNW,
    LVM_INSERTITEMW, LVM_REDRAWITEMS, LVM_SETCALLBACKMASK, LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNW,
    LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SETITEMCOUNT,
    LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVM_SETSELECTIONMARK, LVNI_FOCUSED, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP, LVS_EX_INFOTIP,
    LVS_EX_LABELTIP, LVS_NOCOLUMNHEADER, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, LVSCW_AUTOSIZE, LVSICF_NOSCROLL, LVSIL_SMALL, NMHDR,
    PBM_SETMARQUEE, PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE, PROGRESS_CLASSW, SB_GETICON,
    SB_GETPARTS, SB_SETICON, SB_SETMINHEIGHT, SB_SETPARTS, SB_SETTEXTW, SB_SETTIPTEXTW,
    SBARS_SIZEGRIP, SBARS_TOOLTIPS, STATUSCLASSNAMEW, SetWindowTheme, TOOLTIPS_CLASSW,
    TREE_VIEW_ITEM_STATE_FLAGS, TTF_ABSOLUTE, TTF_IDISHWND, TTF_SUBCLASS, TTF_TRACK, TTM_ADDTOOLW,
    TTM_NEWTOOLRECTW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW,
    TTS_BALLOON, TTS_NOPREFIX, TTTOOLINFOW, TVE_EXPAND, TVGN_CARET, TVGN_CHILD, TVGN_NEXT,
    TVGN_ROOT, TVI_LAST, TVI_ROOT, TVIF_CHILDREN, TVIF_PARAM, TVIF_STATE, TVIF_TEXT,
    TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVIS_STATEIMAGEMASK, TVITEMEXW_CHILDREN, TVITEMW,
    TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMW, TVM_GETNEXTITEM, TVM_INSERTITEMW, TVM_SELECTITEM,
    TVM_SETEXTENDEDSTYLE, TVM_SETITEMW, TVS_CHECKBOXES, TVS_EX_DOUBLEBUFFER,
    TVS_EX_PARTIALCHECKBOXES, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_ICON, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_GETCOUNT, CB_GETCURSEL, CB_GETDROPPEDSTATE, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
    CB_SETCURSEL, CBN_CLOSEUP, CBN_SELCHANGE, CBN_SELENDCANCEL, CBN_SELENDOK, CBS_DROPDOWNLIST,
    CreateWindowExW, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, ES_AUTOHSCROLL, ES_READONLY, GWL_STYLE,
    GetParent, GetWindowLongPtrW, GetWindowRect, HICON, HMENU, IMAGE_ICON, IsChild, LB_ADDSTRING,
    LB_GETCOUNT, LB_GETCURSEL, LB_GETTEXT, LB_GETTEXTLEN, LB_RESETCONTENT, LB_SETCURSEL,
    SB_LINEDOWN, SB_LINEUP, SM_CXVSCROLL, STM_GETIMAGE, STM_SETIMAGE, SendMessageW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_COPY, WM_NCDESTROY, WM_NOTIFY,
    WM_SIZE, WM_VSCROLL, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
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
            | ICC_HOTKEY_CLASS
            | ICC_TREEVIEW_CLASSES,
    };
    // SAFETY: `classes` is complete.
    let _ = unsafe { InitCommonControlsEx(&classes) };
}

/// Creates a visible child of `parent`; its place comes later from the
/// layout.
pub(super) fn child(
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
pub(super) fn send(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize {
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
pub(super) unsafe fn send_pointer<T>(
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: *const T,
) -> isize {
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

    /// A box that shows what keys typed elsewhere make, such as a panel's
    /// quick search box: it takes no focus from `Tab`, and its text cannot
    /// be edited.
    pub(crate) fn create_echo(parent: Hwnd, id: u16) -> windows::core::Result<Edit> {
        child(
            parent,
            w!("EDIT"),
            (ES_AUTOHSCROLL | ES_READONLY) as u32,
            WS_EX_CLIENTEDGE,
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

    /// Whether `event` says that the box lost the focus.
    pub(crate) fn unfocused(self, event: ControlEvent) -> bool {
        event.is(self.0, EN_KILLFOCUS)
    }

    /// The room the box keeps beside its text: its left and right margins.
    pub(crate) fn margins(self) -> Px {
        let margins = send(self.0, EM_GETMARGINS, 0, 0) as usize;
        Px(((margins & 0xffff) + ((margins >> 16) & 0xffff)) as i32)
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

    /// Sets the text with the caret after it, as a hotkey control shows the
    /// keys pressed.
    pub(crate) fn set_text_caret_at_end(self, text: &str) {
        self.0.set_text(text);
        let end = text.encode_utf16().count();
        send(self.0, EM_SETSEL, end, end as isize);
    }

    /// Copies the selected text, as `Ctrl+C` does in a text box.
    pub(crate) fn copy(self) {
        send(self.0, WM_COPY, 0, 0);
    }

    pub(crate) fn text(self) -> String {
        self.0.text()
    }

    /// Selects the text and puts the caret at its end.
    pub(crate) fn select_to_end(self) {
        send(self.0, EM_SETSEL, 0, -1);
        send(self.0, EM_SCROLLCARET, 0, 0);
    }

    /// Selects the UTF-16 units `start..end` of the text, with the caret at
    /// `end`.
    pub(crate) fn select(self, start: usize, end: usize) {
        send(self.0, EM_SETSEL, start, end as isize);
        send(self.0, EM_SCROLLCARET, 0, 0);
    }

    /// The selected UTF-16 units of the text; the caret is at its end.
    pub(crate) fn selection(self) -> (usize, usize) {
        let selection = send(self.0, EM_GETSEL, 0, 0) as usize;
        (selection & 0xffff, (selection >> 16) & 0xffff)
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

    /// One line that shows `&` as itself and ends in an ellipsis when it is
    /// too long, such as a note after a value.
    pub(crate) fn create_line(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        child(
            parent,
            w!("STATIC"),
            SS_NOPREFIX | SS_ENDELLIPSIS,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Label)
    }

    /// A line as [`Label::create_line`] makes, against its right edge, such
    /// as a count that ends under the end of a bar.
    pub(crate) fn create_right_line(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        const SS_RIGHT: u32 = 0x2;
        child(
            parent,
            w!("STATIC"),
            SS_RIGHT | SS_NOPREFIX | SS_ENDELLIPSIS,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Label)
    }

    /// A path against its right edge that loses its middle when it is too
    /// long, so the drive and the last folder stay, such as a search
    /// panel's focused folder.
    pub(crate) fn create_right_path(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        const SS_RIGHT: u32 = 0x2;
        const SS_PATHELLIPSIS: u32 = 0x8000;
        child(
            parent,
            w!("STATIC"),
            SS_RIGHT | SS_NOPREFIX | SS_PATHELLIPSIS,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Label)
    }

    /// A centred text that wraps at word ends and shows `&` as itself, such
    /// as a panel's message in its empty list.
    pub(crate) fn create_centred(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
        const SS_CENTER: u32 = 0x1;
        child(
            parent,
            w!("STATIC"),
            SS_CENTER | SS_NOPREFIX,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Label)
    }

    /// One line, centred in its height, that shows `&` as itself and ends
    /// in an ellipsis when it is too long, such as a file's name.
    pub(crate) fn create_heading(parent: Hwnd, id: u16) -> windows::core::Result<Label> {
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

/// A one-pixel line between the main window's panes (DESIGN.md: Look): an
/// empty static control, which fills itself with the brush its parent gives
/// it ([`super::gdi::StaticColors::divider`]).
#[derive(Clone, Copy)]
pub(crate) struct Divider(Hwnd);

impl Divider {
    pub(crate) fn create(parent: Hwnd) -> windows::core::Result<Divider> {
        child(parent, w!("STATIC"), 0, WINDOW_EX_STYLE(0), 0).map(Divider)
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }
}

/// The static control styles, which the windows crate has only in a feature
/// for system services.
const SS_NOPREFIX: u32 = 0x80;
const SS_ENDELLIPSIS: u32 = 0x4000;
const SS_CENTERIMAGE: u32 = 0x200;

/// A static control that shows an icon in its middle, at the icon's own
/// size.
fn icon_static(parent: Hwnd, id: u16) -> windows::core::Result<Hwnd> {
    const SS_ICON: u32 = 0x3;
    child(
        parent,
        w!("STATIC"),
        SS_ICON | SS_CENTERIMAGE,
        WINDOW_EX_STYLE(0),
        id,
    )
}

/// A static control that shows a file's icon, such as the inspector's: a
/// copy that the control destroys when another replaces it.
#[derive(Clone, Copy)]
pub(crate) struct FileIconLabel(Hwnd);

impl FileIconLabel {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<FileIconLabel> {
        icon_static(parent, id).map(FileIconLabel)
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Shows the icon `index` of the system image list `images`, or none.
    pub(crate) fn show_file_icon(self, icon: Option<(ImageList, i32)>) {
        // The control holds the new copy, and the copy it gives back is this
        // method's earlier one.
        let handle = icon
            .and_then(|(images, index)| images.icon(index))
            .map(Icon::into_handle)
            .unwrap_or_default();
        let previous = send(
            self.0,
            STM_SETIMAGE,
            IMAGE_ICON.0 as usize,
            handle.0 as isize,
        );
        if previous != 0 {
            drop(Icon(HICON(previous as *mut _)));
        }
    }
}

/// A static control that shows a glyph, such as the File operations card's
/// kind icon. The icon is the one [`glyph_icon`] keeps for the thread, so
/// the control never destroys it.
#[derive(Clone, Copy)]
pub(crate) struct GlyphLabel(Hwnd);

impl GlyphLabel {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<GlyphLabel> {
        icon_static(parent, id).map(GlyphLabel)
    }

    /// The static icon `id` of a dialog whose template declares one with
    /// `SS_ICON | SS_CENTERIMAGE`, as [`GlyphLabel::create`] makes it.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<GlyphLabel> {
        super::dialog::item(dialog, id).map(GlyphLabel)
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Shows `glyph`, `side` pixels square, in the text colour of now, so
    /// the owner calls this again when it changes. The same icon is not set
    /// again, which would draw the control again for nothing.
    pub(crate) fn show_glyph(self, glyph: Glyph, side: Px) {
        let icon = glyph_icon(glyph, side, system_color(COLOR_WINDOWTEXT)).unwrap_or_default();
        if send(self.0, STM_GETIMAGE, IMAGE_ICON.0 as usize, 0) != icon.0 as isize {
            send(self.0, STM_SETIMAGE, IMAGE_ICON.0 as usize, icon.0 as isize);
        }
    }
}

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

    /// The button `id` of a dialog.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<Button> {
        super::dialog::item(dialog, id).map(Button)
    }

    /// Shows `icon` before the caption: a glyph at the button's DPI, in the
    /// system colours of now, and only while icons show, so the owner calls
    /// this again when any of them changes; or the administrator shield.
    pub(crate) fn set_icon(self, icon: Option<ButtonIcon>) {
        let glyph = match icon {
            Some(ButtonIcon::Glyph(glyph)) => Some(glyph),
            Some(ButtonIcon::Shield) | None => None,
        };
        // The shield is the button's image too, so each is cleared before
        // the other is set.
        let shield = icon == Some(ButtonIcon::Shield);
        if !shield {
            send(self.0, BCM_SETSHIELD, 0, 0);
        }
        self.set_glyph(glyph);
        if shield {
            send(self.0, BCM_SETSHIELD, 0, 1);
        }
    }

    /// Shows `glyph` before the caption, or no image for `None` and while
    /// icons are hidden.
    fn set_glyph(self, glyph: Option<Glyph>) {
        let dpi = self.0.dpi();
        let images = glyph.and_then(|glyph| button_images(glyph, icon_glyph_height(dpi)));
        let gap = glyph_gap(dpi).0;
        self.set_images(images, BUTTON_IMAGELIST_ALIGN_LEFT, gap);
    }

    /// Shows only `glyph`, in the button's middle, also while icons are
    /// hidden, or its caption for `None`: a button such as File operations'
    /// Pause, whose caption is then only its name for screen readers, at the
    /// button's DPI and in the system colours of now, so the owner calls
    /// this again when either changes.
    pub(crate) fn set_lone_icon(self, glyph: Option<Glyph>) {
        let side = icon_glyph_height(self.0.dpi());
        let images = glyph.and_then(|glyph| lone_button_images(glyph, side));
        // A centred image alone does not hide the caption: Windows draws
        // both, one over the other. `BS_ICON` has it draw only the image.
        // SAFETY: plain values on a window of this thread.
        unsafe {
            let style = GetWindowLongPtrW(self.0.0, GWL_STYLE);
            let changed = if images.is_some() {
                style | BS_ICON as isize
            } else {
                style & !(BS_ICON as isize)
            };
            if changed != style {
                SetWindowLongPtrW(self.0.0, GWL_STYLE, changed);
                self.0.invalidate();
            }
        }
        self.set_images(images, BUTTON_IMAGELIST_ALIGN_CENTER, 0);
    }

    /// Shows `images`, a six-state list, aligned as `align` says, `gap`
    /// pixels from the caption; `None` removes the image. The image the
    /// button already shows is not set again, which would draw the button
    /// again for nothing.
    fn set_images(self, images: Option<HIMAGELIST>, align: BUTTON_IMAGELIST_ALIGN, gap: i32) {
        let list = BUTTON_IMAGELIST {
            himl: images.unwrap_or_default(),
            margin: RECT {
                left: gap,
                right: gap,
                ..RECT::default()
            },
            uAlign: align,
        };
        let mut shown = BUTTON_IMAGELIST::default();
        // SAFETY: `shown` is a local the call fills in.
        unsafe { send_pointer(self.0, BCM_GETIMAGELIST, 0, &raw mut shown) };
        if shown.himl == list.himl && shown.margin == list.margin && shown.uAlign == list.uAlign {
            return;
        }
        // SAFETY: `list` outlives the call; the button keeps the image list
        // without owning it, and `win::gdi` keeps it alive. A null list
        // removes the image.
        unsafe { send_pointer(self.0, BCM_SETIMAGELIST, 0, &raw const list) };
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

    /// A key bar button, which `Tab` does not reach, because `Tab` switches
    /// panels (DESIGN.md: The key bar).
    pub(crate) fn create_key(parent: Hwnd, id: u16) -> windows::core::Result<Button> {
        child(
            parent,
            w!("BUTTON"),
            BS_PUSHBUTTON as u32,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(Button)
    }

    /// A button with its caption and a drop-down arrow and no split, so a
    /// click anywhere on it asks for its menu ([`Button::dropped_down`]),
    /// as a panel's drive button does.
    pub(crate) fn create_drop_down(parent: Hwnd, id: u16) -> windows::core::Result<Button> {
        let button = child(
            parent,
            w!("BUTTON"),
            WS_TABSTOP.0 | BS_SPLITBUTTON as u32,
            WINDOW_EX_STYLE(0),
            id,
        )?;
        let info = BUTTON_SPLITINFO {
            mask: BCSIF_STYLE,
            uSplitStyle: BCSS_NOSPLIT,
            ..BUTTON_SPLITINFO::default()
        };
        // SAFETY: `info` outlives the call, which copies it.
        unsafe { send_pointer(button, BCM_SETSPLITINFO, 0, &raw const info) };
        Ok(Button(button))
    }
}

/// Whether the push button `button` shows an image list's icon before its
/// caption.
pub(super) fn has_image(button: Hwnd) -> bool {
    let mut list = BUTTON_IMAGELIST::default();
    // SAFETY: `list` is a local the call fills in.
    unsafe { send_pointer(button, BCM_GETIMAGELIST, 0, &raw mut list) };
    !list.himl.is_invalid()
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

/// A list box that a dialog's template declares, such as Settings >
/// Keyboard's keys.
#[derive(Clone, Copy)]
pub(crate) struct ListBox(Hwnd);

impl ListBox {
    /// The list box `id` of a dialog whose template declares one.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<ListBox> {
        super::dialog::item(dialog, id).map(ListBox)
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

/// A list of items, each an icon and its text, in one column without a
/// header, with one selection that looks like File Explorer's: the filter
/// pane's Kind list and Settings' page list, and the filter pane's Location
/// list, whose paths have no icon.
#[derive(Clone, Copy)]
pub(crate) struct IconList(Hwnd);

impl IconList {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Its height is the layout's, which [`IconList::row_size`] gives a
    /// row of.
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<IconList> {
        let style = WS_TABSTOP.0
            | LVS_REPORT
            | LVS_NOCOLUMNHEADER
            | LVS_SINGLESEL
            | LVS_SHOWSELALWAYS
            | LVS_SHAREIMAGELISTS;
        let list = IconList(child(parent, WC_LISTVIEWW, style, WS_EX_CLIENTEDGE, id)?);
        list.set_up();
        Ok(list)
    }

    /// The list `id` of a dialog whose template declares one with
    /// [`IconList::create`]'s styles, which [`IconList::set_up`] readies.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<IconList> {
        super::dialog::item(dialog, id).map(IconList)
    }

    /// Gives the list its one column and File Explorer's look; once, before
    /// its first items.
    pub(crate) fn set_up(self) {
        let extended = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        send(
            self.0,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(self.0.0, w!("Explorer"), PCWSTR::null()) };
        let column = LVCOLUMNW {
            mask: LVCF_WIDTH,
            ..Default::default()
        };
        // SAFETY: `column` outlives the call, which copies it.
        unsafe { send_pointer(self.0, LVM_INSERTCOLUMNW, 0, &raw const column) };
    }

    /// Gives the list `glyphs` as the icons its items show, at its DPI, in
    /// the system colours of now, and only while icons show, so the owner
    /// calls this again when any of them changes.
    pub(crate) fn set_icons(self, glyphs: &[Glyph]) {
        let side = icon_glyph_height(self.0.dpi());
        let images = glyph_list(glyphs, side, system_color(COLOR_WINDOWTEXT))
            .filter(|_| icons_shown())
            .unwrap_or_default();
        // The list shares the image list, which `glyph_list` keeps alive.
        send(self.0, LVM_SETIMAGELIST, LVSIL_SMALL as usize, images.0);
    }

    /// Gives the list `items`, each a text and the index of its icon among
    /// those [`IconList::set_icons`] gave, unless it has them, and selects
    /// `selected`, so a list that does not change keeps its scroll position.
    pub(crate) fn set_items<S: AsRef<str>>(self, items: &[(S, usize)], selected: usize) {
        let count = usize::try_from(send(self.0, LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0);
        let shown: Vec<String> = (0..count).map(|index| self.text(index)).collect();
        if !shown
            .iter()
            .map(String::as_str)
            .eq(items.iter().map(|(text, _)| text.as_ref()))
        {
            send(self.0, LVM_DELETEALLITEMS, 0, 0);
            for (index, (text, icon)) in items.iter().enumerate() {
                let mut text = wide(text.as_ref());
                let item = LVITEMW {
                    mask: LVIF_TEXT | LVIF_IMAGE,
                    iItem: index as i32,
                    pszText: PWSTR(text.as_mut_ptr()),
                    iImage: *icon as i32,
                    ..Default::default()
                };
                // SAFETY: `item` and its text outlive the call, which copies
                // them.
                unsafe { send_pointer(self.0, LVM_INSERTITEMW, 0, &raw const item) };
            }
        }
        if self.selected() != Some(selected) {
            let mask = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
            let item = LVITEMW {
                stateMask: mask,
                state: mask,
                ..Default::default()
            };
            // SAFETY: `item` outlives the call, which reads it.
            unsafe { send_pointer(self.0, LVM_SETITEMSTATE, selected, &raw const item) };
        }
    }

    /// Shows the tooltip that the parent gives for the row under the pointer
    /// (`LVN_GETINFOTIPW`, [`IconList::info_tip`]).
    pub(crate) fn show_info_tips(self) {
        send(
            self.0,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            LVS_EX_INFOTIP as usize,
            LVS_EX_INFOTIP as isize,
        );
    }

    /// Gives the list `paths` as [`IconList::set_items`] does, each one that
    /// is wider than its row without its middle, as Windows shortens a path,
    /// so its drive and its last folder stay; then fills the column. The
    /// owner shows a shortened path whole in the row's info tip.
    pub(crate) fn set_paths(self, paths: &[String], selected: usize) {
        let room = self.text_width(paths.len());
        // The list has no icons, so the items' icon index shows nothing.
        let items: Vec<(String, usize)> = match Font::of(self.0) {
            Ok(font) => {
                let meter = TextMeter::new(&font);
                paths
                    .iter()
                    .map(|path| (meter.compact_path(path, room), 0))
                    .collect()
            }
            Err(_) => paths.iter().map(|path| (path.clone(), 0)).collect(),
        };
        self.set_items(&items, selected);
        self.fill_column();
    }

    /// The room for a row's text once the list holds `count` rows: the
    /// list's inside, less the vertical scroll bar when it shows fewer rows,
    /// and less the room the list keeps on either side of a text.
    fn text_width(self, count: usize) -> Px {
        let dpi = self.0.dpi();
        let bar = scroll_bar_width(dpi);
        // The client area leaves out the scroll bar that shows now.
        let mut width = self.0.client_size().width;
        if self.0.styles().0.0 & WS_VSCROLL.0 != 0 {
            width = width + bar;
        }
        let shown = usize::try_from(send(self.0, LVM_GETCOUNTPERPAGE, 0, 0)).unwrap_or(0);
        if count > shown {
            width = width - bar;
        }
        width - column_sizing::cell_width(Px(0), Px(0), true, dpi)
    }

    /// The text of the item `index`.
    pub(crate) fn text(self, index: usize) -> String {
        let mut buffer = [0u16; 260];
        let mut item = LVITEMW {
            pszText: PWSTR(buffer.as_mut_ptr()),
            cchTextMax: buffer.len() as i32,
            ..Default::default()
        };
        // SAFETY: `item` and the buffer it points to outlive the call, which
        // writes at most `cchTextMax` characters.
        let length = unsafe { send_pointer(self.0, LVM_GETITEMTEXTW, index, &raw mut item) };
        String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
    }

    pub(crate) fn selected(self) -> Option<usize> {
        let found = send(self.0, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
        usize::try_from(found).ok()
    }

    /// The height of a row and the width of the widest, its icon and its
    /// text, in the list's font and icons of now, as the layout sizes the
    /// list for them.
    pub(crate) fn row_size(self) -> Size {
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..Default::default()
        };
        // SAFETY: `rect` is a local the call reads and fills in.
        unsafe { send_pointer(self.0, LVM_GETITEMRECT, 0, &raw mut rect) };
        let filled = send(self.0, LVM_GETCOLUMNWIDTH, 0, 0);
        send(self.0, LVM_SETCOLUMNWIDTH, 0, LVSCW_AUTOSIZE as isize);
        let widest = send(self.0, LVM_GETCOLUMNWIDTH, 0, 0);
        send(self.0, LVM_SETCOLUMNWIDTH, 0, filled);
        Size {
            width: Px(widest as i32),
            height: Px(rect.bottom - rect.top),
        }
    }

    /// Makes the column as wide as the list, so a selected row's highlight
    /// spans it; after the list's width changes.
    pub(crate) fn fill_column(self) {
        let width = self.0.client_size().width.0 as isize;
        if send(self.0, LVM_GETCOLUMNWIDTH, 0, 0) != width {
            send(self.0, LVM_SETCOLUMNWIDTH, 0, width);
        }
    }
}

/// Shows `glyph` in `icon` before `title`, the heading of a group of a
/// dialog's page, and places the title after it; while icons are hidden
/// (DESIGN.md: Look), it hides the icon and the title takes its place. The
/// owner calls this again when icons are shown or hidden.
pub(crate) fn show_heading_icon(icon: GlyphLabel, title: Hwnd, glyph: Glyph) {
    let parent = title.parent();
    let dpi = title.dpi();
    let side = icon_glyph_height(dpi);
    let shown = icons_shown();
    icon.show_glyph(glyph, side);
    icon.0.set_visible(shown);
    let place = icon.0.rect_in(parent);
    let heading = title.rect_in(parent);
    let x = if shown {
        place.x + side + glyph_gap(dpi)
    } else {
        place.x
    };
    super::move_children(&[
        (
            icon.0,
            layout::Rect {
                width: side,
                ..place
            },
        ),
        (
            title,
            layout::Rect {
                x,
                width: heading.right() - x,
                ..heading
            },
        ),
    ]);
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
/// (DESIGN.md: Dialogs). Windows destroys it with its owner.
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

/// A tooltip over some of a window's child controls, which shows a
/// control's text while the mouse rests on it. Windows destroys it with its
/// owner, so a copy is only another handle.
#[derive(Clone, Copy)]
pub(crate) struct Tooltip {
    tip: Hwnd,
    parent: Hwnd,
}

impl Tooltip {
    /// A tooltip for `controls`, children of `parent`, which show no tip
    /// until [`Tooltip::set`] gives them text.
    pub(crate) fn create(parent: Hwnd, controls: &[Hwnd]) -> windows::core::Result<Tooltip> {
        // SAFETY: a tooltip window, as Windows documents it. When `parent`
        // is a child window, Windows makes its top-level window the tip's
        // owner, which destroys it.
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
                Some(parent.0),
                None,
                None,
                None,
            )
        }?;
        let tooltip = Tooltip {
            tip: Hwnd(tip),
            parent,
        };
        for &control in controls {
            let mut empty = wide("");
            let tool = tooltip.tool(control, &mut empty);
            // SAFETY: `tool` and its text outlive the call, which copies them.
            unsafe { send_pointer(tooltip.tip, TTM_ADDTOOLW, 0, &raw const tool) };
        }
        // A width makes the tip start a new line at each "\r\n"; no path
        // is this wide, so a path stays on its line.
        send(tooltip.tip, TTM_SETMAXTIPWIDTH, 0, isize::from(i16::MAX));
        Ok(tooltip)
    }

    /// Shows `text` while the mouse rests on `control`, one of the
    /// tooltip's controls; an empty text shows no tip.
    pub(crate) fn set(&self, control: Hwnd, text: &str) {
        let mut text = wide(text);
        let tool = self.tool(control, &mut text);
        // SAFETY: `tool` and its text outlive the call, which copies them.
        unsafe { send_pointer(self.tip, TTM_UPDATETIPTEXTW, 0, &raw const tool) };
    }

    /// A tooltip for `areas` places in `parent`, numbered from 0, such as
    /// the parts of what `parent` draws itself, which show no tip until
    /// [`Tooltip::place_area`] places them and [`Tooltip::set_area`] gives
    /// them text.
    pub(crate) fn create_areas(parent: Hwnd, areas: usize) -> windows::core::Result<Tooltip> {
        let tooltip = Tooltip::create(parent, &[])?;
        for area in 0..areas {
            let mut empty = wide("");
            let tool = tooltip.area_tool(area, RECT::default(), &mut empty);
            // SAFETY: `tool` and its text outlive the call, which copies them.
            unsafe { send_pointer(tooltip.tip, TTM_ADDTOOLW, 0, &raw const tool) };
        }
        Ok(tooltip)
    }

    /// Moves area `area` to `rect`, in its parent's client coordinates.
    pub(crate) fn place_area(&self, area: usize, rect: layout::Rect) {
        let mut text = wide("");
        let tool = self.area_tool(area, super::to_rect(rect), &mut text);
        // SAFETY: `tool` outlives the call, which reads only its rectangle.
        unsafe { send_pointer(self.tip, TTM_NEWTOOLRECTW, 0, &raw const tool) };
    }

    /// Shows `text` while the mouse rests on area `area`; an empty text
    /// shows no tip.
    pub(crate) fn set_area(&self, area: usize, text: &str) {
        let mut text = wide(text);
        let tool = self.area_tool(area, RECT::default(), &mut text);
        // SAFETY: `tool` and its text outlive the call, which copies them.
        unsafe { send_pointer(self.tip, TTM_UPDATETIPTEXTW, 0, &raw const tool) };
    }

    /// The tool of area `area`, which the tooltip knows by its number.
    fn area_tool(&self, area: usize, rect: RECT, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: self.parent.0,
            uId: area,
            rect,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..TTTOOLINFOW::default()
        }
    }

    /// The tool of `control`, which the tooltip knows by its handle.
    fn tool(&self, control: Hwnd, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: self.parent.0,
            uId: control.0.0 as usize,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..TTTOOLINFOW::default()
        }
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
/// Explorer draws its list (DESIGN.md: Look). Only [`ListView::create`] makes
/// one, so its handle is a list view's, which decoding its notifications
/// relies on.
#[derive(Clone, Copy)]
pub(crate) struct ListView(Hwnd);

/// The edge of the results list that it scrolls toward during a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollEdge {
    Top,
    Bottom,
}

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
        // A virtual list keeps only the selection and the focus, so it asks
        // the window which row a drop would go to (DispInfo::set_drop_target).
        send(list, LVM_SETCALLBACKMASK, LVIS_DROPHILITED.0 as usize, 0);
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

    /// Selects every row (DESIGN.md: C11).
    pub(crate) fn select_all(self) {
        self.set_state(usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
    }

    pub(crate) fn deselect(self, row: u64) {
        self.set_state(row as usize, LVIS_SELECTED, LIST_VIEW_ITEM_STATE_FLAGS(0));
    }

    /// The focused row, which a panel moves without the selection
    /// (DESIGN.md: Selecting entries).
    pub(crate) fn focused(self) -> Option<u64> {
        u64::try_from(send(
            self.0,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok()
    }

    /// Focuses `row` and keeps the selection, or takes the focus from every
    /// row for `None`, and scrolls the row into view. The row is also where
    /// a `Shift`+click starts.
    pub(crate) fn set_focused(self, row: Option<u64>) {
        self.focus_in_place(row);
        if let Some(row) = row {
            self.ensure_visible(row);
        }
    }

    /// As [`ListView::set_focused`], without scrolling, so the rows stay
    /// where the person sees them.
    pub(crate) fn focus_in_place(self, row: Option<u64>) {
        match row {
            Some(row) => {
                self.set_state(row as usize, LVIS_FOCUSED, LVIS_FOCUSED);
                send(self.0, LVM_SETSELECTIONMARK, 0, row as isize);
            }
            None => self.set_state(usize::MAX, LVIS_FOCUSED, LIST_VIEW_ITEM_STATE_FLAGS(0)),
        }
    }

    /// Whether `row` is focused, and whether it is selected.
    pub(crate) fn row_state(self, row: u64) -> (bool, bool) {
        let mask = LVIS_FOCUSED.0 | LVIS_SELECTED.0;
        let state = send(self.0, LVM_GETITEMSTATE, row as usize, mask as isize) as u32;
        (state & LVIS_FOCUSED.0 != 0, state & LVIS_SELECTED.0 != 0)
    }

    pub(crate) fn select(self, row: u64, selected: bool) {
        let state = if selected {
            LVIS_SELECTED
        } else {
            LIST_VIEW_ITEM_STATE_FLAGS(0)
        };
        self.set_state(row as usize, LVIS_SELECTED, state);
    }

    /// Selects exactly the rows of `ranges`, sorted ranges that do not
    /// touch, and keeps the focus.
    pub(crate) fn set_selected(self, ranges: &[Range<u64>]) {
        let none = LIST_VIEW_ITEM_STATE_FLAGS(0);
        self.set_state(usize::MAX, LVIS_SELECTED, none);
        let count = self.count();
        if let [all] = ranges
            && *all == (0..count)
        {
            return self.select_all();
        }
        for row in ranges.iter().flat_map(Clone::clone) {
            self.select(row, true);
        }
    }

    /// Changes the selection from `shown`, the rows the list selects, to
    /// `wanted`, both sorted ranges that do not touch, writing only the rows
    /// that differ, because each write notifies the parent.
    pub(crate) fn change_selected(self, shown: &[Range<u64>], wanted: &[Range<u64>]) {
        let count = self.count();
        if let [all] = wanted
            && all.end == count
            && all.start < 2
            && shown != wanted
        {
            self.select_all();
            for row in 0..all.start {
                self.select(row, false);
            }
            return;
        }
        for row in difference(shown, wanted).into_iter().flatten() {
            self.select(row, false);
        }
        for row in difference(wanted, shown).into_iter().flatten() {
            self.select(row, true);
        }
    }

    /// How many whole rows the list shows at a time.
    pub(crate) fn page_rows(self) -> u64 {
        send(self.0, LVM_GETCOUNTPERPAGE, 0, 0).max(1) as u64
    }

    /// The selected rows, as sorted ranges that do not touch. A virtual
    /// list knows them only row by row, so a selection of every row from
    /// the first selected one on, which Select all makes, also without a
    /// panel's `..`, is read without asking for each.
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
        let first = send(self.0, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
        if let Ok(first) = u64::try_from(first)
            && selected == count - first
        {
            return vec![first..count];
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

    /// The edge that the list scrolls toward while dragged files rest at
    /// `point`, in screen coordinates, as File Explorer's list does: the
    /// header and the row height below it, or the last row height above the
    /// bottom.
    pub(crate) fn drag_scroll_edge(self, point: (i32, i32)) -> Option<ScrollEdge> {
        let mut client = POINT {
            x: point.0,
            y: point.1,
        };
        // SAFETY: `client` is a local the call converts in place.
        if !unsafe { ScreenToClient(self.0.0, &mut client) }.as_bool() {
            return None;
        }
        let size = self.0.client_size();
        if client.x < 0 || client.x >= size.width.0 || client.y < 0 || client.y >= size.height.0 {
            return None;
        }
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        // SAFETY: `rect` is a local that the list reads the part from and
        // fills in; a list without rows fails, and nothing scrolls.
        if unsafe { send_pointer(self.0, LVM_GETITEMRECT, 0, &raw mut rect) } == 0 {
            return None;
        }
        let row = rect.bottom - rect.top;
        if client.y < self.header().client_size().height.0 + row {
            Some(ScrollEdge::Top)
        } else if client.y >= size.height.0 - row {
            Some(ScrollEdge::Bottom)
        } else {
            None
        }
    }

    /// Scrolls the list one row toward `edge`.
    pub(crate) fn scroll_row(self, edge: ScrollEdge) {
        let code = match edge {
            ScrollEdge::Top => SB_LINEUP,
            ScrollEdge::Bottom => SB_LINEDOWN,
        };
        send(self.0, WM_VSCROLL, code.0 as usize, 0);
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

    /// Whether the last of [`Self::visible_rows`] shows in part, and is not
    /// the row below the whole rows.
    pub(crate) fn shows_part_row(self) -> bool {
        let row = self.visible_rows().end - 1;
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        // SAFETY: `rect` is a local that the list reads the part from and
        // fills in.
        let held = unsafe { send_pointer(self.0, LVM_GETITEMRECT, row as usize, &raw mut rect) };
        held != 0 && rect.top < self.0.client_size().height.0
    }

    /// The width the columns fill: the list's inside less a vertical scroll
    /// bar, whether the bar shows or not, so rows that bring the bar never
    /// push the columns past the right edge.
    pub(crate) fn fill_width(self) -> Px {
        let inside = self.0.client_size().width;
        // SAFETY: plain values.
        let style = unsafe { GetWindowLongPtrW(self.0.0, GWL_STYLE) } as u32;
        if style & WS_VSCROLL.0 != 0 {
            inside
        } else {
            inside - scroll_bar_width(self.0.dpi())
        }
    }

    /// How far the first column's text area starts after the column's left
    /// edge, past the row's icon, measured on `row`; `None` when the list
    /// does not hold it.
    pub(crate) fn first_label_offset(self, row: u64) -> Option<Px> {
        let mut label = RECT {
            left: LVIR_LABEL as i32,
            ..RECT::default()
        };
        // SAFETY: `label` is a local that the list reads the part from and
        // fills in.
        if unsafe { send_pointer(self.0, LVM_GETITEMRECT, row as usize, &raw mut label) } == 0 {
            return None;
        }
        let header = self.header();
        let mut column = RECT::default();
        // SAFETY: `column` is a local that the header fills in.
        if unsafe { send_pointer(header, HDM_GETITEMRECT, 0, &raw mut column) } == 0 {
            return None;
        }
        // The header's coordinates differ from the list's while the list
        // scrolls sideways.
        let mut left = [POINT {
            x: column.left,
            y: 0,
        }];
        // SAFETY: `left` is a local the call converts in place.
        unsafe { MapWindowPoints(Some(header.0), Some(self.0.0), &mut left) };
        Some(Px(label.left - left[0].x))
    }

    /// The column whose header is at `point`, in screen coordinates.
    pub(crate) fn column_at(self, point: (i32, i32)) -> Option<usize> {
        let header = self.header();
        let mut hit = HDHITTESTINFO {
            pt: POINT {
                x: point.0,
                y: point.1,
            },
            ..Default::default()
        };
        // SAFETY: `hit.pt` is a local the call converts in place.
        if !unsafe { ScreenToClient(header.0, &mut hit.pt) }.as_bool() {
            return None;
        }
        // SAFETY: `hit` is a local that the header reads and fills in.
        let column = unsafe { send_pointer(header, HDM_HITTEST, 0, &raw mut hit) };
        usize::try_from(column).ok()
    }
}

/// The status bar, which sizes itself to its font at the window's bottom.
#[derive(Clone, Copy)]
pub(crate) struct StatusBar(Hwnd);

impl StatusBar {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Creates the status bar as the parent of the controls in its parts,
    /// whose notifications it passes to `parent`. `Tab` and the access keys
    /// reach them, as `WS_EX_CONTROLPARENT` lets `IsDialogMessageW` look
    /// inside it. A parent that the person resizes gets the size grip. A
    /// part's tooltip shows only while its text is cut off.
    pub(crate) fn create(
        parent: Hwnd,
        id: u16,
        size_grip: bool,
    ) -> windows::core::Result<StatusBar> {
        let grip = if size_grip { SBARS_SIZEGRIP } else { 0 };
        let bar = child(
            parent,
            STATUSCLASSNAMEW,
            grip | SBARS_TOOLTIPS | WS_CLIPCHILDREN.0,
            WS_EX_CONTROLPARENT,
            id,
        )?;
        // SAFETY: the procedure uses no data, and removes itself on
        // WM_NCDESTROY, the status bar's last message.
        if !unsafe { SetWindowSubclass(bar.0, Some(status_bar_procedure), 0, 0) }.as_bool() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(StatusBar(bar))
    }

    /// Sets the height of the drawing area, which Windows makes at least
    /// as tall as the font; [`Self::fit`] applies it.
    pub(crate) fn set_min_height(self, height: Px) {
        send(self.0, SB_SETMINHEIGHT, height.0.max(0) as usize, 0);
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
    /// A part keeps the text and icon of the part that had its index.
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
        let mut current = vec![0; edges.len()];
        // SAFETY: `current` has room for the `current.len()` integers the
        // message writes at most, and outlives the call.
        let count =
            unsafe { send_pointer(self.0, SB_GETPARTS, current.len(), current.as_mut_ptr()) };
        // Setting the parts draws the whole bar again, which the status
        // updates would make flicker.
        if count == edges.len() as isize && current == edges {
            return;
        }
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

    /// Shows `glyph` before the text of `part`, at the bar's DPI, in the
    /// system colours of now and only while icons show, or no icon for
    /// `None`, so the owner calls this again when any of them changes.
    pub(crate) fn set_icon(self, part: usize, glyph: Option<Glyph>) {
        let side = icon_glyph_height(self.0.dpi());
        let icon = glyph
            .filter(|_| icons_shown())
            .and_then(|glyph| glyph_icon(glyph, side, system_color(COLOR_BTNTEXT)))
            .unwrap_or_default();
        // The bar keeps the icon without owning it, and `glyph_icon` keeps
        // it alive; one that it shows already is not set again, because each
        // setting draws the part again.
        if send(self.0, SB_GETICON, part, 0) != icon.0 as isize {
            send(self.0, SB_SETICON, part, icon.0 as isize);
        }
    }

    /// The width of a part that shows `text` whole, after `glyph`'s icon
    /// while icons show, as [`Self::set_icon`] shows it.
    pub(crate) fn part_width(self, text: &str, glyph: Option<Glyph>) -> Px {
        let text = Font::of(self.0).map_or(Px(0), |font| TextMeter::new(&font).width(text));
        layout::status_part_width(text, glyph.is_some() && icons_shown(), self.0.dpi())
    }

    /// Sets the tooltip of `part`, which shows while its text is cut off.
    pub(crate) fn set_tip(self, part: usize, text: &str) {
        let text = wide(text);
        // SAFETY: `text` is NUL-terminated and outlives the call, which
        // copies it.
        unsafe { send_pointer(self.0, SB_SETTIPTEXTW, part, text.as_ptr()) };
    }
}

/// The procedure of the status bar, before its own: it passes the
/// notifications of the controls in its parts to its parent, which handles
/// them as its own controls'.
unsafe extern "system" fn status_bar_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let forwarded = match msg {
        WM_COMMAND => true,
        // SAFETY: a WM_NOTIFY's lparam points to an NMHDR. The bar's own
        // tooltips are popups, not children, and their notifications stay
        // the bar's, which answers them.
        WM_NOTIFY => unsafe { IsChild(hwnd, (*(lparam.0 as *const NMHDR)).hwndFrom) }.as_bool(),
        _ => false,
    };
    match msg {
        _ if forwarded => {
            // SAFETY: the parameters are the ones Windows passed, and the
            // parent decodes them as its own controls'.
            unsafe {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, msg, Some(wparam), Some(lparam));
                }
            }
        }
        WM_NCDESTROY => {
            // SAFETY: the status bar gets no message after this one.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(status_bar_procedure), 0) };
        }
        _ => {}
    }
    // SAFETY: the parameters are the ones Windows passed.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// A progress bar that shows that work runs, without saying how much of it
/// is done.
#[derive(Clone, Copy)]
pub(crate) struct ProgressBar(Hwnd);

impl ProgressBar {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<ProgressBar> {
        child(parent, PROGRESS_CLASSW, PBS_MARQUEE, WINDOW_EX_STYLE(0), id).map(ProgressBar)
    }

    /// Starts or stops the movement that measures nothing.
    pub(crate) fn set_moving(self, moving: bool) {
        if moving {
            self.set_marquee(true);
        }
        // A period of 0 keeps Windows' default of 30 ms.
        send(self.0, PBM_SETMARQUEE, usize::from(moving), 0);
    }

    /// Fills the bar to `done` of `total`.
    pub(crate) fn set_measured(self, done: u64, total: u64) {
        /// The bar's positions, so a size of any number of bytes fits.
        const STEPS: u64 = 1000;
        send(self.0, PBM_SETMARQUEE, 0, 0);
        self.set_marquee(false);
        let position = done.min(total).saturating_mul(STEPS) / total.max(1);
        // A themed bar animates each increase, so at one update in 500 ms
        // the fill trails the percent beside it; a decrease draws at once,
        // so the bar goes one step past the position and back.
        send(self.0, PBM_SETRANGE32, 0, (STEPS + 1) as isize);
        send(self.0, PBM_SETPOS, position as usize + 1, 0);
        send(self.0, PBM_SETPOS, position as usize, 0);
        send(self.0, PBM_SETRANGE32, 0, STEPS as isize);
    }

    /// Only a bar without `PBS_MARQUEE` measures, and the bar takes the
    /// style's change while it exists.
    fn set_marquee(self, marquee: bool) {
        // SAFETY: plain values on a window of this thread.
        unsafe {
            let style = GetWindowLongPtrW(self.0.0, GWL_STYLE);
            let changed = if marquee {
                style | PBS_MARQUEE as isize
            } else {
                style & !(PBS_MARQUEE as isize)
            };
            if changed != style {
                SetWindowLongPtrW(self.0.0, GWL_STYLE, changed);
            }
        }
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

    /// Makes the control take any key as it is pressed, instead of turning
    /// some keys into others by its own rules.
    pub(crate) fn take_any_key(self) {
        send(self.0, HKM_SETRULES, 0, 0);
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

    /// Shows the tooltip that the dialog's `DialogProc::info_tip` gives for
    /// the row under the pointer, instead of the cut-off text in full. Call
    /// it after [`ReportList::set_columns`].
    pub(crate) fn show_info_tips(self) {
        send(
            self.0,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            (LVS_EX_INFOTIP | LVS_EX_LABELTIP) as usize,
            LVS_EX_INFOTIP as isize,
        );
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
        let width = (self.0.client_size().width - scroll_bar_width(self.0.dpi())).0;
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
                let message = if column == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                };
                self.put_cell(message, index, column, text);
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

    /// Changes the rows from `shown`, which the list holds, to `rows`. With
    /// as many rows, it sets only the cells that differ, so the list redraws
    /// only those and keeps its selection; otherwise it replaces the rows as
    /// [`ReportList::set_rows`] does.
    pub(crate) fn update_rows<R: AsRef<[String]>>(self, shown: &[R], rows: &[R]) {
        if shown.len() != rows.len() {
            self.set_rows(rows, None);
            return;
        }
        for (index, (old, new)) in shown.iter().zip(rows).enumerate() {
            for (column, (old, new)) in old.as_ref().iter().zip(new.as_ref()).enumerate() {
                if old != new {
                    self.put_cell(LVM_SETITEMTEXTW, index, column, new);
                }
            }
        }
    }

    /// Sends `message`, which inserts an item or sets a cell's text, for
    /// cell `column` of row `index`.
    fn put_cell(self, message: u32, index: usize, column: usize, text: &str) {
        let mut text = wide(text);
        let item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: index as i32,
            iSubItem: column as i32,
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: `item` and its text outlive the call, which copies them.
        unsafe { send_pointer(self.0, message, index, &raw const item) };
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

    /// Fits each column to `headers` and to the cells of `rows`, and fills
    /// the list's width with them, as the results list does
    /// ([`column_sizing::fill_widths`]). The list's label tips show a cell that is
    /// still cut off.
    pub(crate) fn fit_columns<R: AsRef<[String]>>(self, headers: &[&str], rows: &[R]) {
        let Ok(font) = Font::of(self.0) else {
            return;
        };
        let meter = TextMeter::new(&font);
        let dpi = self.0.dpi();
        let fits: Vec<ColumnFit> = headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                let texts = (rows.iter())
                    .filter_map(|row| row.as_ref().get(column))
                    .filter(|text| !text.is_empty())
                    .map(|text| meter.plain_width(text));
                ColumnFit::of(meter.plain_width(header), texts, Px(0), column == 0, dpi)
            })
            .collect();
        // The vertical scroll bar takes its width when the rows fill the
        // list.
        let width = self.0.client_size().width - scroll_bar_width(dpi);
        for (column, width) in column_sizing::fill_widths(&fits, width, dpi)
            .into_iter()
            .enumerate()
        {
            send(self.0, LVM_SETCOLUMNWIDTH, column, width.0 as isize);
        }
    }

    /// Shows each path of `rows` in `column` that is wider than the column
    /// without its middle, as Windows shortens a path, so its drive and its
    /// name stay. Call it once the column has its width.
    pub(crate) fn show_paths<R: AsRef<[String]>>(self, column: usize, rows: &[R]) {
        let Ok(font) = Font::of(self.0) else {
            return;
        };
        let meter = TextMeter::new(&font);
        let width = Px(send(self.0, LVM_GETCOLUMNWIDTH, column, 0) as i32);
        let room = width - column_sizing::cell_width(Px(0), Px(0), column == 0, self.0.dpi());
        for (index, row) in rows.iter().enumerate() {
            if let Some(path) = row.as_ref().get(column) {
                let shown = meter.compact_path(path, room);
                if shown != *path {
                    self.put_cell(LVM_SETITEMTEXTW, index, column, &shown);
                }
            }
        }
    }
}

/// The width of a vertical scroll bar at `dpi`.
fn scroll_bar_width(dpi: Dpi) -> Px {
    // SAFETY: plain values.
    Px(unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, dpi.get()) })
}

/// An item of a [`FolderTree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeItem(pub(crate) HTREEITEM);

/// A tree view with a check box on each item, which a dialog's template
/// declares, such as Indexed folders' folders. Each item carries a number,
/// which the dialog gives it, and a check box that shows a [`FolderCheck`].
#[derive(Clone, Copy)]
pub(crate) struct FolderTree(Hwnd);

impl FolderTree {
    /// The tree view `id` of a dialog.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<FolderTree> {
        super::dialog::item(dialog, id).map(FolderTree)
    }

    /// Gives the tree check boxes that can show [`FolderCheck::Partial`],
    /// and File Explorer's look. Call it before the first item is added.
    pub(crate) fn show_check_boxes(self) {
        // Windows makes the check boxes' images only when the style is set
        // after the control exists and before it holds an item.
        // SAFETY: plain values on a window of this thread.
        unsafe {
            let style = GetWindowLongPtrW(self.0.0, GWL_STYLE);
            SetWindowLongPtrW(self.0.0, GWL_STYLE, style | TVS_CHECKBOXES as isize);
        }
        let extended = TVS_EX_PARTIALCHECKBOXES | TVS_EX_DOUBLEBUFFER;
        send(
            self.0,
            TVM_SETEXTENDEDSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(self.0.0, w!("Explorer"), PCWSTR::null()) };
    }

    /// Adds the item `text`, carrying `data`, as the last child of
    /// `parent`, or as the last top item. With `may_have_children`, it shows
    /// the button that expands it, before its children are added.
    pub(crate) fn insert(
        self,
        parent: Option<TreeItem>,
        text: &str,
        data: usize,
        may_have_children: bool,
    ) -> Option<TreeItem> {
        let mut text = wide(text);
        let insert = TVINSERTSTRUCTW {
            hParent: parent.map_or(TVI_ROOT, |p| p.0),
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN,
                    pszText: PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(i32::from(may_have_children)),
                    lParam: LPARAM(data as isize),
                    ..Default::default()
                },
            },
        };
        // SAFETY: `insert` and its text outlive the call, which copies them.
        let item = unsafe { send_pointer(self.0, TVM_INSERTITEMW, 0, &raw const insert) };
        (item != 0).then_some(TreeItem(HTREEITEM(item)))
    }

    /// Shows that `item` has no children, after a listing found none.
    pub(crate) fn set_childless(self, item: TreeItem) {
        let change = TVITEMW {
            mask: TVIF_CHILDREN,
            hItem: item.0,
            ..Default::default()
        };
        // SAFETY: `change` outlives the call, which reads it.
        unsafe { send_pointer(self.0, TVM_SETITEMW, 0, &raw const change) };
    }

    pub(crate) fn set_check(self, item: TreeItem, check: FolderCheck) {
        // The state images of TVS_EX_PARTIALCHECKBOXES, from 1.
        let image: u32 = match check {
            FolderCheck::Unchecked => 1,
            FolderCheck::Checked => 2,
            FolderCheck::Partial => 3,
        };
        let change = TVITEMW {
            mask: TVIF_STATE,
            hItem: item.0,
            state: TREE_VIEW_ITEM_STATE_FLAGS(image << 12),
            stateMask: TVIS_STATEIMAGEMASK,
            ..Default::default()
        };
        // SAFETY: `change` outlives the call, which reads it.
        unsafe { send_pointer(self.0, TVM_SETITEMW, 0, &raw const change) };
    }

    /// The number the item carries.
    pub(crate) fn data(self, item: TreeItem) -> usize {
        let mut read = TVITEMW {
            mask: TVIF_PARAM,
            hItem: item.0,
            ..Default::default()
        };
        // SAFETY: `read` outlives the call, which fills it in.
        unsafe { send_pointer(self.0, TVM_GETITEMW, 0, &raw mut read) };
        read.lParam.0 as usize
    }

    /// The children of `parent`, or the top items, in their order.
    pub(crate) fn children(self, parent: Option<TreeItem>) -> Vec<TreeItem> {
        let mut children = Vec::new();
        let mut next = match parent {
            Some(parent) => send(self.0, TVM_GETNEXTITEM, TVGN_CHILD as usize, parent.0.0),
            None => send(self.0, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0),
        };
        while next != 0 {
            children.push(TreeItem(HTREEITEM(next)));
            next = send(self.0, TVM_GETNEXTITEM, TVGN_NEXT as usize, next);
        }
        children
    }

    pub(crate) fn expand(self, item: TreeItem) {
        send(self.0, TVM_EXPAND, TVE_EXPAND.0 as usize, item.0.0);
    }

    /// Selects `item` and scrolls it into view.
    pub(crate) fn select(self, item: TreeItem) {
        send(self.0, TVM_SELECTITEM, TVGN_CARET as usize, item.0.0);
        send(self.0, TVM_ENSUREVISIBLE, 0, item.0.0);
    }
}
