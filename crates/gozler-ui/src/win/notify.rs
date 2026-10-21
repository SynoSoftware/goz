//! What controls tell their parent: `WM_COMMAND` as a [`Command`], and
//! `WM_NOTIFY` as a [`Notification`] that the control which sent it decodes.
//! Each decoded notification borrows the structure that the message carries,
//! so no handler can keep it past the message.

use super::Hwnd;
use super::controls::{Button, IconList, ListView, StatusBar};
use super::gdi::{Font, ImageList, draw_glyph, draw_text, system_color};
use crate::core::glyph::Glyph;
use crate::core::layout::Px;
use std::marker::PhantomData;
use std::ops::Range;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_WINDOWTEXT, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, GetSysColor, HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    BCN_DROPDOWN, CDDS_ITEMPOSTPAINT, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM,
    CDRF_DODEFAULT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT,
    CDRF_NOTIFYSUBITEMDRAW, CDRF_SKIPDEFAULT, HDN_DIVIDERDBLCLICKW, LVIF_IMAGE, LVIF_STATE,
    LVIF_TEXT, LVIR_ICON, LVIR_LABEL, LVIS_DROPHILITED, LVIS_FOCUSED, LVIS_SELECTED,
    LVM_GETITEMRECT, LVM_GETSUBITEMRECT, LVN_BEGINDRAG, LVN_BEGINRDRAG, LVN_COLUMNCLICK,
    LVN_GETDISPINFOW, LVN_GETINFOTIPW, LVN_ITEMACTIVATE, LVN_ITEMCHANGED, LVN_ODCACHEHINT,
    LVN_ODSTATECHANGED, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_KILLFOCUS, NM_RCLICK, NM_SETFOCUS,
    NMHDR, NMHEADERW, NMLISTVIEW, NMLVCACHEHINT, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVGETINFOTIPW,
    NMLVODSTATECHANGE, NMMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

/// A `WM_COMMAND`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Command {
    /// A menu item or an accelerator, by its ID.
    Menu(u16),
    Control(ControlEvent),
}

/// A control's notification in `WM_COMMAND`, such as `EN_CHANGE`. The
/// control's wrapper says which it is, by the control's handle, so an ID
/// that two windows share never confuses them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlEvent {
    pub(super) code: u32,
    pub(super) from: Hwnd,
    /// The sender's ID, or for a toolbar the clicked button's.
    pub(super) id: u16,
}

impl ControlEvent {
    /// The control that sent it.
    pub(crate) fn sender(self) -> Hwnd {
        self.from
    }
}

pub(super) fn command(wparam: WPARAM, lparam: LPARAM) -> Command {
    let id = (wparam.0 & 0xffff) as u16;
    if lparam.0 == 0 {
        Command::Menu(id)
    } else {
        Command::Control(ControlEvent {
            code: ((wparam.0 >> 16) & 0xffff) as u32,
            from: Hwnd(HWND(lparam.0 as *mut _)),
            id,
        })
    }
}

/// A `WM_NOTIFY`, which only the control that sent it can decode, because
/// each control class sends its own structures behind the `NMHDR`.
pub(crate) struct Notification<'a> {
    pub(super) header: *mut NMHDR,
    message: PhantomData<&'a mut NMHDR>,
}

impl Notification<'_> {
    /// # Safety
    /// `lparam` is the `NMHDR` pointer of a `WM_NOTIFY` that Windows sent,
    /// valid and unaliased for the notification's lifetime.
    pub(super) unsafe fn new(lparam: LPARAM) -> Self {
        Notification {
            header: lparam.0 as *mut NMHDR,
            message: PhantomData,
        }
    }

    pub(super) fn header(&self) -> &NMHDR {
        // SAFETY: `new`'s contract.
        unsafe { &*self.header }
    }

    /// The control that sent it.
    pub(crate) fn sender(&self) -> Hwnd {
        Hwnd(self.header().hwndFrom)
    }
}

/// A list view's notification.
pub(crate) enum ListNotify<'a> {
    /// The list asks for a cell's text or a row's icon.
    GetDispInfo(DispInfo<'a>),
    /// The rows the list is about to show.
    CacheHint(Range<u64>),
    /// A click on the header of the column at this index.
    ColumnClick(usize),
    CustomDraw(CustomDraw<'a>),
    /// Rows were selected or deselected: `LVN_ITEMCHANGED` with a change of
    /// `LVIS_SELECTED`, item -1 (every row) included, and
    /// `LVN_ODSTATECHANGED`, which a virtual list sends for a range such as
    /// a Shift+click. The handler reads the selection from the list once
    /// per user action (DESIGN.md: The selection).
    SelectionChanged,
    /// The focused row changed, and the selection did not.
    FocusChanged,
    /// A double-click on a row.
    Activated,
    /// The person starts dragging the selected rows, with either button.
    BeginDrag,
    /// The list got the focus.
    Focused,
    /// The list lost the focus.
    Unfocused,
    /// A click in the list, on a row or on its empty area, with either
    /// button.
    Clicked,
    /// A double-click on the divider after the column at this index, which
    /// the header sends and the list passes on after sizing the column
    /// itself.
    DividerDoubleClick(usize),
    Other,
}

impl ListView {
    /// `notification` decoded, if this list sent it.
    pub(crate) fn notification<'a>(self, notification: Notification<'a>) -> Option<ListNotify<'a>> {
        let header = notification.header();
        let pointer = notification.header;
        if header.hwndFrom == self.header().0 && header.code == HDN_DIVIDERDBLCLICKW {
            // SAFETY: a header sends HDN_DIVIDERDBLCLICKW with an NMHEADERW,
            // valid for the notification's lifetime.
            let item = unsafe { &*pointer.cast::<NMHEADERW>() }.iItem;
            return usize::try_from(item)
                .ok()
                .map(ListNotify::DividerDoubleClick);
        }
        if header.hwndFrom != self.hwnd().0 {
            return None;
        }
        let selection_changed = |new: u32, old: u32| (new ^ old) & LVIS_SELECTED.0 != 0;
        // SAFETY: the sender is this list view, which `ListView::create`
        // made with the list view class, and each structure is read only
        // for the code with which a list view sends it, so the memory behind
        // the NMHDR is that whole structure for the notification's lifetime.
        // Other controls send NM_CUSTOMDRAW with the smaller NMCUSTOMDRAW,
        // which is why the sender is checked first.
        let list = unsafe {
            match header.code {
                LVN_GETDISPINFOW => ListNotify::GetDispInfo(DispInfo(&mut *pointer.cast())),
                LVN_ODCACHEHINT => {
                    let hint = &*pointer.cast::<NMLVCACHEHINT>();
                    let from = u64::try_from(hint.iFrom).unwrap_or(0);
                    let to = u64::try_from(hint.iTo).unwrap_or(0);
                    ListNotify::CacheHint(from..to.max(from) + 1)
                }
                LVN_COLUMNCLICK => {
                    let list = &*pointer.cast::<NMLISTVIEW>();
                    match usize::try_from(list.iSubItem) {
                        Ok(column) => ListNotify::ColumnClick(column),
                        Err(_) => ListNotify::Other,
                    }
                }
                LVN_ITEMCHANGED => {
                    let change = &*pointer.cast::<NMLISTVIEW>();
                    let changed = change.uNewState ^ change.uOldState;
                    if !change.uChanged.contains(LVIF_STATE) {
                        ListNotify::Other
                    } else if selection_changed(change.uNewState, change.uOldState) {
                        ListNotify::SelectionChanged
                    } else if changed & LVIS_FOCUSED.0 != 0 {
                        ListNotify::FocusChanged
                    } else {
                        ListNotify::Other
                    }
                }
                LVN_ODSTATECHANGED => {
                    let change = &*pointer.cast::<NMLVODSTATECHANGE>();
                    if selection_changed(change.uNewState.0, change.uOldState.0) {
                        ListNotify::SelectionChanged
                    } else {
                        ListNotify::Other
                    }
                }
                LVN_ITEMACTIVATE => ListNotify::Activated,
                LVN_BEGINDRAG | LVN_BEGINRDRAG => ListNotify::BeginDrag,
                NM_SETFOCUS => ListNotify::Focused,
                NM_KILLFOCUS => ListNotify::Unfocused,
                NM_CLICK | NM_RCLICK | NM_DBLCLK => ListNotify::Clicked,
                NM_CUSTOMDRAW => ListNotify::CustomDraw(CustomDraw {
                    draw: &mut *pointer.cast(),
                    list: self.hwnd(),
                }),
                _ => ListNotify::Other,
            }
        };
        Some(list)
    }
}

impl IconList {
    /// Whether `notification` says that this list selected another item,
    /// which the person or the window chose.
    pub(crate) fn selection_changed(self, notification: &Notification) -> bool {
        let header = notification.header();
        if header.hwndFrom != self.hwnd().0 || header.code != LVN_ITEMCHANGED {
            return false;
        }
        // SAFETY: a list view sends LVN_ITEMCHANGED with an NMLISTVIEW,
        // valid for the notification's lifetime.
        let change = unsafe { &*notification.header.cast::<NMLISTVIEW>() };
        change.uChanged.contains(LVIF_STATE)
            && change.uNewState & !change.uOldState & LVIS_SELECTED.0 != 0
    }

    /// The request for a row's tooltip that `notification` holds, if this
    /// list sent it ([`IconList::show_info_tips`]).
    pub(crate) fn info_tip<'a>(self, notification: &'a mut Notification) -> Option<InfoTip<'a>> {
        let header = notification.header();
        if header.hwndFrom != self.hwnd().0 || header.code != LVN_GETINFOTIPW {
            return None;
        }
        // SAFETY: a list view sends LVN_GETINFOTIPW with an NMLVGETINFOTIPW,
        // valid for the notification's lifetime, which the borrow of
        // `notification` keeps to one answer.
        Some(InfoTip(unsafe { &mut *notification.header.cast() }))
    }
}

impl Button {
    /// Whether `notification` is this drop-down button's request for its
    /// menu, which a click anywhere on it sends ([`Button::create_drop_down`]).
    pub(crate) fn dropped_down(self, notification: &Notification) -> bool {
        let header = notification.header();
        header.hwndFrom == self.hwnd().0 && header.code == BCN_DROPDOWN
    }
}

impl StatusBar {
    /// The index of the part that `notification` says was clicked, if this
    /// status bar sent it.
    pub(crate) fn clicked_part(self, notification: &Notification) -> Option<usize> {
        let header = notification.header();
        if header.hwndFrom != self.hwnd().0 || header.code != NM_CLICK {
            return None;
        }
        // SAFETY: a status bar sends NM_CLICK with an NMMOUSE, whose item
        // is the part clicked, for the notification's lifetime.
        let mouse = unsafe { &*notification.header.cast::<NMMOUSE>() };
        Some(mouse.dwItemSpec)
    }
}

/// `LVN_GETINFOTIPW`: the tooltip of the row under the pointer.
pub(crate) struct InfoTip<'a>(pub(super) &'a mut NMLVGETINFOTIPW);

impl InfoTip<'_> {
    pub(crate) fn row(&self) -> Option<usize> {
        usize::try_from(self.0.iItem).ok()
    }

    /// Answers with `text`, cut to the room the list gives, and returns
    /// the notification's answer.
    pub(crate) fn set(self, text: &str) -> LRESULT {
        let Ok(room) = usize::try_from(self.0.cchTextMax) else {
            return LRESULT(0);
        };
        if room == 0 {
            return LRESULT(0);
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let kept = units.len().min(room - 1);
        // SAFETY: the list gives pszText with room for cchTextMax
        // characters, valid while it waits for the answer.
        let buffer = unsafe { std::slice::from_raw_parts_mut(self.0.pszText.0, room) };
        buffer[..kept].copy_from_slice(&units[..kept]);
        buffer[kept] = 0;
        LRESULT(0)
    }
}

/// `LVN_GETDISPINFOW`: the list asks for a cell.
pub(crate) struct DispInfo<'a>(&'a mut NMLVDISPINFOW);

impl DispInfo<'_> {
    pub(crate) fn row(&self) -> u64 {
        u64::try_from(self.0.item.iItem).unwrap_or(0)
    }

    /// The column's index in the list.
    pub(crate) fn column(&self) -> usize {
        usize::try_from(self.0.item.iSubItem).unwrap_or(0)
    }

    pub(crate) fn wants_text(&self) -> bool {
        self.0.item.mask.contains(LVIF_TEXT)
    }

    pub(crate) fn wants_image(&self) -> bool {
        self.0.item.mask.contains(LVIF_IMAGE)
    }

    /// Copies `text` into the list's buffer, cut to the buffer's length.
    pub(crate) fn set_text(&mut self, text: &str) {
        let item = &mut self.0.item;
        let Ok(capacity) = usize::try_from(item.cchTextMax) else {
            return;
        };
        if capacity == 0 || item.pszText.is_null() {
            return;
        }
        // SAFETY: the list gives a buffer of `cchTextMax` UTF-16 units for
        // the length of the message.
        let buffer = unsafe { std::slice::from_raw_parts_mut(item.pszText.0, capacity) };
        let mut written = 0;
        for (slot, unit) in buffer[..capacity - 1].iter_mut().zip(text.encode_utf16()) {
            *slot = unit;
            written += 1;
        }
        buffer[written] = 0;
    }

    /// The row's icon, an index in the list's image list, or none.
    pub(crate) fn set_image(&mut self, image: Option<i32>) {
        // I_IMAGENONE
        self.0.item.iImage = image.unwrap_or(-2);
    }

    /// Whether the list asks for the row's state, which it does only for
    /// the states the window keeps (`ListView::create`).
    pub(crate) fn wants_state(&self) -> bool {
        self.0.item.mask.contains(LVIF_STATE)
    }

    /// Shows the row highlighted as the folder that dragged files would be
    /// dropped into, as File Explorer does.
    pub(crate) fn set_drop_target(&mut self, target: bool) {
        let item = &mut self.0.item;
        let bit = LVIS_DROPHILITED.0;
        item.stateMask.0 |= bit;
        item.state.0 = if target {
            item.state.0 | bit
        } else {
            item.state.0 & !bit
        };
    }
}

/// `clrText` when the list gives the row no colour of its own.
const CLR_DEFAULT: COLORREF = COLORREF(0xFF00_0000);

/// `NM_CUSTOMDRAW` from the list, which lets the window draw a cell itself.
pub(crate) struct CustomDraw<'a> {
    draw: &'a mut NMLVCUSTOMDRAW,
    list: Hwnd,
}

/// The stage of drawing a custom-draw notification is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawStage {
    /// Before the list paints.
    Paint,
    /// Before `row`.
    Row {
        row: u64,
    },
    /// Before the cell of `row` in the column at index `column`.
    Cell {
        row: u64,
        column: usize,
    },
    /// After `row`, which [`DrawAnswer::NewFont`] or
    /// [`DrawAnswer::AfterRow`] answered.
    RowPainted {
        row: u64,
    },
    Other,
}

/// How a custom-draw notification is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawAnswer {
    /// The list draws as usual.
    Default,
    /// The list asks again before each row.
    EachRow,
    /// The list asks again before each cell.
    EachCell,
    /// The window drew the cell, and the list draws nothing in it.
    Drawn,
    /// The list draws the row in the colour that
    /// [`CustomDraw::draw_row_grey`] set.
    NewColour,
    /// The list draws the row in the font and colour that
    /// [`CustomDraw::draw_row_struck`] set, and asks again after it.
    NewFont,
    /// The list draws the row as usual, and asks again after it.
    AfterRow,
}

impl CustomDraw<'_> {
    pub(crate) fn stage(&self) -> DrawStage {
        let stage = self.draw.nmcd.dwDrawStage;
        let row = self.draw.nmcd.dwItemSpec as u64;
        if stage == CDDS_PREPAINT {
            DrawStage::Paint
        } else if stage == CDDS_ITEMPREPAINT {
            DrawStage::Row { row }
        } else if stage.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 {
            DrawStage::Cell {
                row,
                column: usize::try_from(self.draw.iSubItem).unwrap_or(0),
            }
        } else if stage == CDDS_ITEMPOSTPAINT {
            DrawStage::RowPainted { row }
        } else {
            DrawStage::Other
        }
    }

    pub(crate) fn answer(answer: DrawAnswer) -> LRESULT {
        LRESULT(match answer {
            DrawAnswer::Default => CDRF_DODEFAULT,
            DrawAnswer::EachRow => CDRF_NOTIFYITEMDRAW,
            DrawAnswer::EachCell => CDRF_NOTIFYSUBITEMDRAW,
            DrawAnswer::Drawn => CDRF_SKIPDEFAULT,
            DrawAnswer::NewColour => CDRF_NEWFONT,
            DrawAnswer::NewFont => CDRF_NEWFONT | CDRF_NOTIFYPOSTPAINT,
            DrawAnswer::AfterRow => CDRF_NOTIFYPOSTPAINT,
        } as isize)
    }

    /// Has the list draw the row at `DrawStage::Row` in grey, unless the
    /// list gives the row a colour of its own, as a selected row has in high
    /// contrast, where grey on the selection would not read. The list then
    /// draws every cell, its icon, its alignment and its ellipsis as in any
    /// row. Answer [`DrawAnswer::NewColour`].
    pub(crate) fn draw_row_grey(&mut self) {
        if self.draw.clrText == CLR_DEFAULT {
            self.draw.clrText = system_color(COLOR_GRAYTEXT);
        }
    }

    /// Has the list draw the row at `DrawStage::Row` in `font` and in grey,
    /// as [`Self::draw_row_grey`] says. Answer [`DrawAnswer::NewFont`], and
    /// at [`DrawStage::RowPainted`] give the DC the list's font back with
    /// [`Self::restore_font`], because the list does not, and would draw the
    /// next rows in `font`.
    pub(crate) fn draw_row_struck(&mut self, font: &Font) {
        self.draw_row_grey();
        // SAFETY: the list's DC for the length of its paint; the list's own
        // font goes back into it at the row's post-paint stage.
        unsafe { SelectObject(self.draw.nmcd.hdc, HGDIOBJ(font.handle().0)) };
    }

    /// Selects `font`, the list's own, into the DC after a row that
    /// [`Self::draw_row_struck`] drew.
    pub(crate) fn restore_font(&self, font: &Font) {
        // SAFETY: the list's DC for the length of its paint.
        unsafe { SelectObject(self.draw.nmcd.hdc, HGDIOBJ(font.handle().0)) };
    }

    /// Draws `glyph` in the window text colour over the lower part of the
    /// icon of the row at [`DrawStage::RowPainted`], as a badge on it.
    pub(crate) fn draw_over_icon(&self, glyph: Glyph) {
        let icon = self.cell_part(LVIR_ICON);
        let height = icon.bottom - icon.top;
        let area = RECT {
            top: icon.top + height / 4,
            ..icon
        };
        let side = Px(height * 9 / 16);
        draw_glyph(
            self.draw.nmcd.hdc,
            glyph,
            side,
            system_color(COLOR_WINDOWTEXT),
            area,
        );
    }

    /// Draws the cell at `DrawStage::Cell` as the list would, with the
    /// parts `bold` of `text`, byte ranges, in `fonts.1`: the text where the
    /// list puts it, in the colour it gives the row's state, ending in an
    /// ellipsis where it does not fit, and in the first column `icon` from
    /// `images` before it. The row's background is the list's.
    pub(crate) fn draw_cell(
        &self,
        text: &str,
        bold: &[Range<usize>],
        fonts: (&Font, &Font),
        icon: Option<(ImageList, i32)>,
        inset: Px,
    ) {
        let dc = self.draw.nmcd.hdc;
        let label = self.cell_part(LVIR_LABEL);
        self.draw_icon(icon);
        let colour = self.text_colour();
        let mut x = label.left + inset.0;
        let right = label.right - inset.0;
        let mut parts = Vec::new();
        let mut at = 0;
        for range in bold {
            parts.push((at..range.start, false));
            parts.push((range.clone(), true));
            at = range.end;
        }
        parts.push((at..text.len(), false));
        // SAFETY: the list's DC for the length of the message; the font
        // selected into it is restored before the message returns.
        unsafe {
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, colour);
            let previous = SelectObject(dc, HGDIOBJ(fonts.0.handle().0));
            for (range, is_bold) in parts.into_iter().filter(|(r, _)| !r.is_empty()) {
                if x >= right {
                    break;
                }
                let font = if is_bold { fonts.1 } else { fonts.0 };
                SelectObject(dc, HGDIOBJ(font.handle().0));
                let part = &text[range];
                let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
                let mut measured = RECT::default();
                draw_text(dc, part, &mut measured, flags | DT_CALCRECT);
                let mut area = RECT {
                    left: x,
                    top: label.top,
                    right,
                    bottom: label.bottom,
                };
                draw_text(dc, part, &mut area, flags | DT_END_ELLIPSIS);
                x += measured.right - measured.left;
            }
            SelectObject(dc, previous);
        }
    }

    /// Draws `icon` from its image list where the list puts the first
    /// column's icon; other columns have none.
    fn draw_icon(&self, icon: Option<(ImageList, i32)>) {
        if let Some((images, index)) = icon.filter(|_| self.draw.iSubItem == 0) {
            images.draw(self.draw.nmcd.hdc, index, self.cell_part(LVIR_ICON));
        }
    }

    /// The part `part`, such as `LVIR_LABEL`, of the cell being drawn.
    fn cell_part(&self, part: u32) -> RECT {
        let column = self.draw.iSubItem;
        let mut rect = RECT {
            left: part as i32,
            top: column,
            ..RECT::default()
        };
        // The first column's label and icon are the item's own; the others
        // are its subitems'.
        let message = if column == 0 {
            LVM_GETITEMRECT
        } else {
            LVM_GETSUBITEMRECT
        };
        // SAFETY: `rect` is a local that the list reads its part from and
        // fills in.
        unsafe {
            SendMessageW(
                self.list.0,
                message,
                Some(WPARAM(self.draw.nmcd.dwItemSpec)),
                Some(LPARAM(&raw mut rect as isize)),
            )
        };
        rect
    }

    /// The colour the list gives the text of the row's state.
    fn text_colour(&self) -> COLORREF {
        match self.draw.clrText {
            CLR_DEFAULT => COLORREF(
                // SAFETY: no parameters.
                unsafe { GetSysColor(COLOR_WINDOWTEXT) },
            ),
            colour => colour,
        }
    }
}
