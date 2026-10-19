//! The results list's notifications, decoded from `WM_NOTIFY`. Each borrows
//! the structure that the message carries, so no handler can keep it past
//! the message.

use super::Hwnd;
use super::gdi::{Font, ImageList, draw_text};
use crate::core::layout::Px;
use std::ops::Range;
use windows::Win32::Foundation::{COLORREF, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOWTEXT, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    GetSysColor, HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYSUBITEMDRAW, CDRF_SKIPDEFAULT, LVIF_IMAGE, LVIF_TEXT, LVIR_ICON, LVIR_LABEL,
    LVM_GETITEMRECT, LVM_GETSUBITEMRECT, LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_ODCACHEHINT,
    NM_CUSTOMDRAW, NMHDR, NMLISTVIEW, NMLVCACHEHINT, NMLVCUSTOMDRAW, NMLVDISPINFOW, WC_LISTVIEWW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, SendMessageW};

/// A notification from a child control.
pub(crate) struct Notify<'a> {
    /// The control's ID.
    pub(crate) from: u16,
    pub(crate) list: ListNotify<'a>,
}

/// A list view's notification; [`ListNotify::Other`] for any other
/// control's.
pub(crate) enum ListNotify<'a> {
    /// The list asks for a cell's text or a row's icon.
    GetDispInfo(DispInfo<'a>),
    /// The rows the list is about to show.
    CacheHint(Range<u64>),
    /// A click on the header of the column at this index.
    ColumnClick(usize),
    CustomDraw(CustomDraw<'a>),
    Other,
}

/// # Safety
/// `lparam` is the `NMHDR` pointer of a `WM_NOTIFY` that Windows sent, valid
/// and unaliased for the length of the message.
pub(super) unsafe fn decode<'a>(lparam: LPARAM) -> Notify<'a> {
    // SAFETY: the caller's contract.
    let header = unsafe { &*(lparam.0 as *const NMHDR) };
    let from = header.idFrom as u16;
    if !is_list_view(Hwnd(header.hwndFrom)) {
        return Notify {
            from,
            list: ListNotify::Other,
        };
    }
    // SAFETY: the sender is a list view, and each structure is read only for
    // the code with which a list view sends it, so the memory behind the
    // NMHDR is that whole structure. Other controls send NM_CUSTOMDRAW with
    // the smaller NMCUSTOMDRAW, which is why the sender is checked first.
    let list = unsafe {
        match header.code {
            LVN_GETDISPINFOW => {
                ListNotify::GetDispInfo(DispInfo(&mut *(lparam.0 as *mut NMLVDISPINFOW)))
            }
            LVN_ODCACHEHINT => {
                let hint = &*(lparam.0 as *const NMLVCACHEHINT);
                let from = u64::try_from(hint.iFrom).unwrap_or(0);
                let to = u64::try_from(hint.iTo).unwrap_or(0);
                ListNotify::CacheHint(from..to.max(from) + 1)
            }
            LVN_COLUMNCLICK => {
                let list = &*(lparam.0 as *const NMLISTVIEW);
                match usize::try_from(list.iSubItem) {
                    Ok(column) => ListNotify::ColumnClick(column),
                    Err(_) => ListNotify::Other,
                }
            }
            NM_CUSTOMDRAW => ListNotify::CustomDraw(CustomDraw {
                draw: &mut *(lparam.0 as *mut NMLVCUSTOMDRAW),
                list: Hwnd(header.hwndFrom),
            }),
            _ => ListNotify::Other,
        }
    };
    Notify { from, list }
}

fn is_list_view(window: Hwnd) -> bool {
    let mut class = [0u16; 32];
    // SAFETY: `class` is a local buffer that the call fills in.
    let length = unsafe { GetClassNameW(window.0, &mut class) };
    let length = usize::try_from(length).unwrap_or(0);
    // SAFETY: WC_LISTVIEWW is a constant NUL-terminated string.
    class[..length] == *unsafe { WC_LISTVIEWW.as_wide() }
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
}

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
    /// Before a row.
    Row,
    /// Before the cell of `row` in the column at index `column`.
    Cell {
        row: u64,
        column: usize,
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
}

impl CustomDraw<'_> {
    pub(crate) fn stage(&self) -> DrawStage {
        let stage = self.draw.nmcd.dwDrawStage;
        if stage == CDDS_PREPAINT {
            DrawStage::Paint
        } else if stage == CDDS_ITEMPREPAINT {
            DrawStage::Row
        } else if stage.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 {
            DrawStage::Cell {
                row: self.draw.nmcd.dwItemSpec as u64,
                column: usize::try_from(self.draw.iSubItem).unwrap_or(0),
            }
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
        } as isize)
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
        let row = self.draw.nmcd.dwItemSpec;
        let column = self.draw.iSubItem;
        let rect = |part| {
            let mut rect = RECT {
                left: part as i32,
                top: column,
                ..RECT::default()
            };
            // The first column's label and icon are the item's own; the
            // others are its subitems'.
            let message = if column == 0 {
                LVM_GETITEMRECT
            } else {
                LVM_GETSUBITEMRECT
            };
            // SAFETY: `rect` is a local that the list reads its part from
            // and fills in.
            unsafe {
                SendMessageW(
                    self.list.0,
                    message,
                    Some(WPARAM(row)),
                    Some(LPARAM(&raw mut rect as isize)),
                )
            };
            rect
        };
        let label = rect(LVIR_LABEL);
        if let Some((images, index)) = icon.filter(|_| column == 0) {
            let area = rect(LVIR_ICON);
            images.draw(dc, index, area);
        }
        let colour = match self.draw.clrText {
            // CLR_DEFAULT
            COLORREF(0xFF00_0000) => COLORREF(
                // SAFETY: no parameters.
                unsafe { GetSysColor(COLOR_WINDOWTEXT) },
            ),
            colour => colour,
        };
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
}
