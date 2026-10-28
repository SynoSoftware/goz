//! What controls tell their parent: `WM_COMMAND` as a [`Command`], and
//! `WM_NOTIFY` as a [`Notification`] that the control which sent it decodes.
//! Each decoded notification borrows the structure that the message carries,
//! so no handler can keep it past the message.

use super::Hwnd;
use super::controls::{Button, IconList, ListView, StatusBar};
use super::gdi::{Font, ImageList, draw_text, high_contrast, system_color};
use crate::core::layout::Px;
use crate::core::selection::RowsChange;
use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::Range;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_WINDOWTEXT, DC_BRUSH, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DrawFocusRect, FillRect, GetStockObject, GetSysColor, HBRUSH,
    HGDIOBJ, SelectObject, SetBkMode, SetDCBrushColor, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    BCN_DROPDOWN, CDDS_ITEMPOSTPAINT, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDIS_FOCUS,
    CDIS_SELECTED, CDIS_SHOWKEYBOARDCUES, CDRF_DODEFAULT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYPOSTPAINT, CDRF_NOTIFYSUBITEMDRAW, CDRF_SKIPDEFAULT, HDI_WIDTH,
    HDN_DIVIDERDBLCLICKW, HDN_ENDDRAG, HDN_ITEMCHANGEDW, LVIF_IMAGE, LVIF_STATE, LVIF_TEXT,
    LVIR_BOUNDS, LVIR_ICON, LVIR_LABEL, LVIS_DROPHILITED, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_GETITEMRECT, LVM_GETITEMW, LVM_GETSUBITEMRECT, LVN_BEGINDRAG, LVN_BEGINRDRAG,
    LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_GETINFOTIPW, LVN_ITEMACTIVATE, LVN_ITEMCHANGED,
    LVN_ODCACHEHINT, LVN_ODFINDITEMW, LVN_ODSTATECHANGED, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK,
    NM_KILLFOCUS, NM_RCLICK, NM_SETFOCUS, NMHDR, NMHEADERW, NMLISTVIEW, NMLVCACHEHINT,
    NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVGETINFOTIPW, NMLVODSTATECHANGE, NMMOUSE,
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
    /// a Shift+click. The handler reads the selection once per user action
    /// (DESIGN.md: The selection).
    SelectionChanged(RowsChange),
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
    /// The person dropped a header they dragged. The header takes its new
    /// order after this notification, so the handler reads it later.
    ColumnDragEnded,
    ColumnWidthChanged(usize),
    Other,
}

/// The answer to `LVN_ODFINDITEMW`, which an owner-data list sends to ask
/// for the row that starts with a typed character, or `None` for another
/// notification. Gozler's lists find rows by quick search or not at all,
/// so the answer is -1, no row: the list reads an unanswered 0 as row 0
/// and selects it.
///
/// # Safety
/// `lparam` is the NMHDR of a WM_NOTIFY, valid for the length of the
/// message.
pub(super) unsafe fn typed_row_answer(lparam: LPARAM) -> Option<isize> {
    // SAFETY: the caller's promise.
    let code = unsafe { (*(lparam.0 as *const NMHDR)).code };
    (code == LVN_ODFINDITEMW).then_some(-1)
}

impl ListView {
    pub(crate) fn info_tip<'a>(self, notification: &'a mut Notification) -> Option<InfoTip<'a>> {
        InfoTip::of(self.hwnd(), notification)
    }

    /// `notification` decoded, if this list sent it.
    pub(crate) fn notification<'a>(self, notification: Notification<'a>) -> Option<ListNotify<'a>> {
        let header = notification.header();
        let pointer = notification.header;
        // The rows' notifications come for every cell a paint draws, so the
        // header is asked for only for the header's own codes.
        let from_header = matches!(
            header.code,
            HDN_ITEMCHANGEDW | HDN_ENDDRAG | HDN_DIVIDERDBLCLICKW
        ) && header.hwndFrom == self.header().0;
        if from_header && header.code == HDN_ITEMCHANGEDW {
            // SAFETY: HDN_ITEMCHANGEDW carries an NMHEADERW and its optional
            // item for the notification's lifetime.
            let index = unsafe {
                let change = &*pointer.cast::<NMHEADERW>();
                change
                    .pitem
                    .as_ref()
                    .filter(|item| item.mask.contains(HDI_WIDTH))
                    .and_then(|_| usize::try_from(change.iItem).ok())
            };
            return index.map(ListNotify::ColumnWidthChanged);
        }
        if from_header && header.code == HDN_ENDDRAG {
            return Some(ListNotify::ColumnDragEnded);
        }
        if from_header && header.code == HDN_DIVIDERDBLCLICKW {
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
                        let selected = change.uNewState & LVIS_SELECTED.0 != 0;
                        ListNotify::SelectionChanged(match u64::try_from(change.iItem) {
                            Ok(row) => RowsChange::Row { row, selected },
                            Err(_) => RowsChange::All { selected },
                        })
                    } else if changed & LVIS_FOCUSED.0 != 0 {
                        ListNotify::FocusChanged
                    } else {
                        ListNotify::Other
                    }
                }
                LVN_ODSTATECHANGED => {
                    let change = &*pointer.cast::<NMLVODSTATECHANGE>();
                    if selection_changed(change.uNewState.0, change.uOldState.0) {
                        let from = u64::try_from(change.iFrom).unwrap_or(0);
                        let to = u64::try_from(change.iTo).unwrap_or(0);
                        ListNotify::SelectionChanged(RowsChange::Rows {
                            rows: from..to.max(from) + 1,
                            selected: change.uNewState.0 & LVIS_SELECTED.0 != 0,
                        })
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
                    list: self,
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
        InfoTip::of(self.hwnd(), notification)
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

impl<'a> InfoTip<'a> {
    fn of(list: Hwnd, notification: &'a mut Notification) -> Option<Self> {
        let header = notification.header();
        if header.hwndFrom != list.0 || header.code != LVN_GETINFOTIPW {
            return None;
        }
        // SAFETY: a list view sends LVN_GETINFOTIPW with an NMLVGETINFOTIPW,
        // valid for the notification's lifetime, which the borrow of
        // `notification` keeps to one answer.
        Some(InfoTip(unsafe { &mut *notification.header.cast() }))
    }

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
pub(crate) struct DispInfo<'a>(pub(super) &'a mut NMLVDISPINFOW);

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

/// `clrTextBk` that has the list draw text without filling behind it.
const CLR_NONE: COLORREF = COLORREF(0xFFFF_FFFF);

/// The fill of a selected row in the list that has the focus: File
/// Explorer's light blue, behind the text and the icon, without the border
/// that the "Explorer" visual style draws around each row (DESIGN.md:
/// Selecting entries).
const SELECTED_FILL: COLORREF = COLORREF(0x00FF_E8CC);

/// The fill of the focused row when it is not selected: a light gray, so
/// the row that `Enter` opens shows without looking selected (DESIGN.md:
/// Selecting entries).
const FOCUSED_FILL: COLORREF = COLORREF(0x00F0_F0F0);

thread_local! {
    /// What [`CustomDraw::answer`] chose for the row the list draws now.
    static LOOK: Cell<Option<RowLook>> = const { Cell::new(None) };
}

/// The fill and the outline of a row, chosen before the list draws it.
#[derive(Clone, Copy)]
struct RowLook {
    list: isize,
    row: usize,
    fill: Option<COLORREF>,
    outlined: bool,
}

/// `NM_CUSTOMDRAW` from the list, which lets the window draw a cell itself.
pub(crate) struct CustomDraw<'a> {
    draw: &'a mut NMLVCUSTOMDRAW,
    list: ListView,
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
    /// After a row that [`DrawAnswer::NewFont`] answered, or that
    /// [`CustomDraw::answer`] filled as selected or outlined as focused.
    RowPainted,
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
            DrawStage::RowPainted
        } else {
            DrawStage::Other
        }
    }

    /// Answers the list with `answer`. In the focused list, also draws a
    /// selected row with [`SELECTED_FILL`] behind its text and its icon,
    /// the focused row that is not selected with [`FOCUSED_FILL`], and the
    /// focused row's focus rectangle over the whole row. The fills show
    /// also while `keys_here` says that another control's keys act on this
    /// list, as `Enter` in Search's search box opens its focused row,
    /// because Windows' gray for a list without the focus looks like
    /// [`FOCUSED_FILL`].
    pub(crate) fn answer(&mut self, answer: DrawAnswer, keys_here: bool) -> LRESULT {
        let mut flags = match answer {
            DrawAnswer::Default => CDRF_DODEFAULT,
            DrawAnswer::EachRow => CDRF_NOTIFYITEMDRAW,
            DrawAnswer::EachCell => CDRF_NOTIFYSUBITEMDRAW,
            DrawAnswer::Drawn => CDRF_SKIPDEFAULT,
            DrawAnswer::NewColour => CDRF_NEWFONT,
            DrawAnswer::NewFont => CDRF_NEWFONT | CDRF_NOTIFYPOSTPAINT,
        };
        let stage = self.draw.nmcd.dwDrawStage;
        let area = self.draw.nmcd.rc;
        // Before the label tip shows a row's full text, the list asks for
        // the row's font to measure it, with an empty rectangle and a DC on
        // the list itself; a fill there drew over the row's cells on screen.
        let measures = area.right <= area.left || area.bottom <= area.top;
        if measures || (stage != CDDS_ITEMPREPAINT && stage != CDDS_ITEMPOSTPAINT) {
            return LRESULT(flags as isize);
        }
        let row = self.draw.nmcd.dwItemSpec;
        let list = self.list.hwnd().0.0 as isize;
        if stage == CDDS_ITEMPOSTPAINT {
            // The stage after the row matches what was chosen before it,
            // because the list's state bits are this function's own edits by
            // then.
            let look = LOOK
                .take()
                .filter(|look| look.list == list && look.row == row);
            if let Some(look) = look {
                if let Some(colour) = look.fill {
                    self.draw_icon_on(colour);
                }
                if look.outlined {
                    self.draw_focus();
                }
            }
            return LRESULT(flags as isize);
        }
        // The list is asked, because `uItemState` says that every row is
        // selected. The list of another panel or a window in the background
        // shows no focus rectangle, and keeps Windows' grey selection, as
        // File Explorer's does; high contrast keeps the system colours.
        let (focused, selected) = self.list.row_state(row as u64);
        let in_focus = Hwnd::focused() == Some(self.list.hwnd());
        let fill = if high_contrast() {
            None
        } else if selected && (in_focus || keys_here) {
            Some(SELECTED_FILL)
        } else if focused && !selected && (in_focus || keys_here) {
            Some(FOCUSED_FILL)
        } else {
            None
        };
        // The list draws a focus rectangle only over the cells it drew, so
        // Search's cells with matches in bold would have none.
        let outlined = focused && in_focus;
        LOOK.set(Some(RowLook {
            list,
            row,
            fill,
            outlined,
        }));
        if let Some(colour) = fill {
            self.fill_row(colour);
        } else if self.draw.clrTextBk == CLR_NONE {
            // The list keeps the colours it was given for the rows after.
            self.draw.clrTextBk = CLR_DEFAULT;
        }
        if outlined {
            self.draw.nmcd.uItemState.0 &= !CDIS_FOCUS.0;
        }
        if fill.is_some() || outlined {
            flags |= CDRF_NEWFONT | CDRF_NOTIFYPOSTPAINT;
        }
        LRESULT(flags as isize)
    }

    /// Before the list draws the row: fills it with `colour`, and has the
    /// list draw the row as unselected, whose text it draws on that fill
    /// when the text has no background of its own.
    fn fill_row(&mut self, colour: COLORREF) {
        self.draw.nmcd.uItemState.0 &= !CDIS_SELECTED.0;
        self.draw.clrTextBk = CLR_NONE;
        self.fill(self.row_part(LVIR_BOUNDS), colour);
    }

    /// After the list drew the row that [`Self::fill_row`] filled: draws its
    /// icon again on `colour`, because the list draws an icon on its own
    /// white.
    fn draw_icon_on(&self, colour: COLORREF) {
        let icon = self.row_part(LVIR_ICON);
        self.fill(icon, colour);
        if let (Some(images), Some(index)) = (ImageList::system_small(), self.row_image()) {
            images.draw(self.draw.nmcd.hdc, index, icon);
        }
    }

    /// After the list drew the focused row without its focus rectangle:
    /// draws it over the whole row, where Windows shows focus rectangles.
    fn draw_focus(&self) {
        if self.draw.nmcd.uItemState.0 & CDIS_SHOWKEYBOARDCUES.0 != 0 {
            // SAFETY: the list's DC for the length of its paint.
            let _ = unsafe { DrawFocusRect(self.draw.nmcd.hdc, &self.row_part(LVIR_BOUNDS)) };
        }
    }

    fn fill(&self, area: RECT, colour: COLORREF) {
        let dc = self.draw.nmcd.hdc;
        // SAFETY: the list's DC for the length of its paint; its brush
        // colour is restored before the call returns.
        unsafe {
            let previous = SetDCBrushColor(dc, colour);
            FillRect(dc, &area, HBRUSH(GetStockObject(DC_BRUSH).0));
            SetDCBrushColor(dc, previous);
        }
    }

    /// The icon of the row being drawn, an index in the system image list,
    /// which every list that draws its selection here shows.
    fn row_image(&self) -> Option<i32> {
        let mut item = LVITEMW {
            mask: LVIF_IMAGE,
            iItem: i32::try_from(self.draw.nmcd.dwItemSpec).ok()?,
            ..LVITEMW::default()
        };
        // SAFETY: `item` is a local the list fills in.
        unsafe {
            SendMessageW(
                self.list.hwnd().0,
                LVM_GETITEMW,
                Some(WPARAM(0)),
                Some(LPARAM(&raw mut item as isize)),
            )
        };
        // I_IMAGENONE and I_IMAGECALLBACK are negative.
        (item.iImage >= 0).then_some(item.iImage)
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
        // The first column's label and icon are the item's own; the others
        // are its subitems'.
        match self.draw.iSubItem {
            0 => self.row_part(part),
            column => self.part(LVM_GETSUBITEMRECT, part, column),
        }
    }

    /// The part `part`, such as `LVIR_ICON`, of the row being drawn. The
    /// row's stages read it here, because `iSubItem` there can hold the
    /// last cell's column.
    fn row_part(&self, part: u32) -> RECT {
        self.part(LVM_GETITEMRECT, part, 0)
    }

    fn part(&self, message: u32, part: u32, column: i32) -> RECT {
        let mut rect = RECT {
            left: part as i32,
            top: column,
            ..RECT::default()
        };
        // SAFETY: `rect` is a local that the list reads its part from and
        // fills in.
        unsafe {
            SendMessageW(
                self.list.hwnd().0,
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
