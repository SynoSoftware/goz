//! What controls tell their parent: `WM_COMMAND` as a [`Command`], and
//! `WM_NOTIFY` as a [`Notification`] that the control which sent it decodes.
//! Each decoded notification borrows the structure that the message carries,
//! so no handler can keep it past the message.

use super::Hwnd;
use super::controls::{Button, ListView, StatusBar};
use super::gdi::{
    Font, ImageList, disabled_color, draw_lone_glyph, draw_text, inactive_bar_color, system_color,
};
use crate::core::glyph::Glyph;
use crate::core::layout::{Px, cell_text_inset, icon_glyph_height};
use std::marker::PhantomData;
use std::ops::Range;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_WINDOW, COLOR_WINDOWTEXT, DC_BRUSH, DFC_BUTTON, DFCS_BUTTONPUSH, DFCS_INACTIVE,
    DFCS_PUSHED, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DrawFocusRect, DrawFrameControl, FillRect, FrameRect,
    GetStockObject, GetSysColor, GetSysColorBrush, HBRUSH, HGDIOBJ, InflateRect, SelectObject,
    SetBkMode, SetDCBrushColor, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    BCN_DROPDOWN, BP_PUSHBUTTON, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDIS_DISABLED,
    CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED, CDIS_SHOWKEYBOARDCUES, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYSUBITEMDRAW, CDRF_SKIPDEFAULT, CloseThemeData,
    DrawThemeBackground, DrawThemeParentBackground, HDN_DIVIDERDBLCLICKW, LVIF_IMAGE, LVIF_STATE,
    LVIF_TEXT, LVIR_BOUNDS, LVIR_ICON, LVIR_LABEL, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_GETITEMRECT, LVM_GETITEMTEXTW, LVM_GETSUBITEMRECT, LVN_BEGINDRAG, LVN_BEGINRDRAG,
    LVN_COLUMNCLICK, LVN_GETDISPINFOW, LVN_ITEMACTIVATE, LVN_ITEMCHANGED, LVN_ODCACHEHINT,
    LVN_ODSTATECHANGED, NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_KILLFOCUS, NM_RCLICK, NM_SETFOCUS,
    NMCUSTOMDRAW, NMHDR, NMHEADERW, NMLISTVIEW, NMLVCACHEHINT, NMLVCUSTOMDRAW, NMLVDISPINFOW,
    NMLVODSTATECHANGE, NMMOUSE, OpenThemeData, PBS_DISABLED, PBS_HOT, PBS_NORMAL, PBS_PRESSED,
    TP_BUTTON, TS_HOT, TS_PRESSED,
};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{PWSTR, w};

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

/// The background a flat button stands on, which it fills.
#[derive(Clone, Copy)]
pub(crate) enum FlatGround {
    /// The window's background, as a pane's.
    Window,
    /// The dialog face, as a status bar's.
    Face,
}

impl Button {
    /// Draws this button, if `notification` is its `NM_CUSTOMDRAW`, as a
    /// flat icon button, as a toolbar's: `glyph` centred on `ground`, on the
    /// toolbar's hot or pressed background while the mouse is on it or it
    /// is pressed. A focus rectangle shows while the window shows keyboard
    /// cues. The button's caption stays the name that screen readers say.
    /// Returns the answer to the notification.
    pub(crate) fn draw_flat(
        self,
        notification: &Notification,
        glyph: Glyph,
        ground: FlatGround,
    ) -> Option<LRESULT> {
        let header = notification.header();
        if header.hwndFrom != self.hwnd().0 || header.code != NM_CUSTOMDRAW {
            return None;
        }
        // SAFETY: a button sends NM_CUSTOMDRAW with an NMCUSTOMDRAW, valid
        // for the notification's lifetime.
        let draw = unsafe { &*notification.header.cast::<NMCUSTOMDRAW>() };
        if draw.dwDrawStage != CDDS_PREPAINT {
            return Some(LRESULT(CDRF_DODEFAULT as isize));
        }
        let state = draw.uItemState;
        let dc = draw.hdc;
        let mut rect = draw.rc;
        let hot = if state.contains(CDIS_SELECTED) {
            Some(TS_PRESSED.0)
        } else if state.contains(CDIS_HOT) {
            Some(TS_HOT.0)
        } else {
            None
        };
        let ground = match ground {
            FlatGround::Window => COLOR_WINDOW,
            FlatGround::Face => COLOR_BTNFACE,
        };
        let color = if state.contains(CDIS_DISABLED) {
            disabled_color(ground)
        } else {
            system_color(COLOR_WINDOWTEXT)
        };
        // SAFETY: the DC is the notification's, which the button holds for
        // it; the theme is closed before the answer.
        unsafe {
            FillRect(dc, &rect, GetSysColorBrush(ground));
            if let Some(hot) = hot {
                // High contrast has no theme, and its button face shows the
                // state instead.
                let theme = OpenThemeData(Some(self.hwnd().0), w!("Toolbar"));
                if theme.is_invalid() {
                    FillRect(dc, &rect, GetSysColorBrush(COLOR_BTNFACE));
                } else {
                    let _ = DrawThemeBackground(theme, dc, TP_BUTTON.0, hot, &rect, None);
                    let _ = CloseThemeData(theme);
                }
            }
            let size = icon_glyph_height(self.hwnd().dpi());
            draw_lone_glyph(dc, glyph, rect, size, color);
            if state.contains(CDIS_FOCUS) && state.contains(CDIS_SHOWKEYBOARDCUES) {
                let _ = InflateRect(&mut rect, -2, -2);
                let _ = DrawFocusRect(dc, &rect);
            }
        }
        Some(LRESULT(CDRF_SKIPDEFAULT as isize))
    }
}

impl Button {
    /// Whether `notification` is this drop-down button's request for its
    /// menu, which a click anywhere on it sends ([`Button::create_drop_down`]).
    pub(crate) fn dropped_down(self, notification: &Notification) -> bool {
        let header = notification.header();
        header.hwndFrom == self.hwnd().0 && header.code == BCN_DROPDOWN
    }

    /// Draws this key bar button, if `notification` is its
    /// `NM_CUSTOMDRAW`, as a push button with `key` in `fonts.1`, bold,
    /// then `word` in `fonts.0`, centred, as Total Commander's key bar
    /// shows them (file-manager-spec.md: The key bar). The button's caption
    /// stays the name that screen readers say. Returns the answer to the
    /// notification.
    pub(crate) fn draw_key(
        self,
        notification: &Notification,
        key: &str,
        word: &str,
        fonts: (&Font, &Font),
    ) -> Option<LRESULT> {
        let header = notification.header();
        if header.hwndFrom != self.hwnd().0 || header.code != NM_CUSTOMDRAW {
            return None;
        }
        // SAFETY: a button sends NM_CUSTOMDRAW with an NMCUSTOMDRAW, valid
        // for the notification's lifetime.
        let draw = unsafe { &*notification.header.cast::<NMCUSTOMDRAW>() };
        if draw.dwDrawStage != CDDS_PREPAINT {
            return Some(LRESULT(CDRF_DODEFAULT as isize));
        }
        let state = draw.uItemState;
        let disabled = state.contains(CDIS_DISABLED);
        let pressed = state.contains(CDIS_SELECTED);
        let dc = draw.hdc;
        let mut rect = draw.rc;
        let color = if disabled {
            disabled_color(COLOR_BTNFACE)
        } else {
            system_color(COLOR_BTNTEXT)
        };
        let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
        let word = format!(" {word}");
        // SAFETY: the DC is the notification's, which the button holds for
        // it; the theme is closed and the font restored before the answer.
        unsafe {
            // High contrast has no theme, and the classic frame shows the
            // state instead.
            let theme = OpenThemeData(Some(self.hwnd().0), w!("Button"));
            if theme.is_invalid() {
                let mut look = DFCS_BUTTONPUSH;
                if pressed {
                    look |= DFCS_PUSHED;
                }
                if disabled {
                    look |= DFCS_INACTIVE;
                }
                let _ = DrawFrameControl(dc, &mut rect, DFC_BUTTON, look);
            } else {
                let look = if disabled {
                    PBS_DISABLED
                } else if pressed {
                    PBS_PRESSED
                } else if state.contains(CDIS_HOT) {
                    PBS_HOT
                } else {
                    PBS_NORMAL
                };
                let _ = DrawThemeParentBackground(self.hwnd().0, dc, Some(&rect));
                let _ = DrawThemeBackground(theme, dc, BP_PUSHBUTTON.0, look.0, &rect, None);
                let _ = CloseThemeData(theme);
            }
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, color);
            let previous = SelectObject(dc, HGDIOBJ(fonts.1.handle().0));
            let mut key_size = RECT::default();
            draw_text(dc, key, &mut key_size, flags | DT_CALCRECT);
            SelectObject(dc, HGDIOBJ(fonts.0.handle().0));
            let mut word_size = RECT::default();
            draw_text(dc, &word, &mut word_size, flags | DT_CALCRECT);
            let width = key_size.right + word_size.right;
            let left = rect.left + ((rect.right - rect.left - width) / 2).max(0);
            let mut area = RECT {
                left: left + key_size.right,
                ..rect
            };
            draw_text(dc, &word, &mut area, flags | DT_END_ELLIPSIS);
            SelectObject(dc, HGDIOBJ(fonts.1.handle().0));
            let mut area = RECT { left, ..rect };
            draw_text(dc, key, &mut area, flags);
            SelectObject(dc, previous);
        }
        Some(LRESULT(CDRF_SKIPDEFAULT as isize))
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

/// The bar on a panel's focused row (file-manager-spec.md: Selecting
/// entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowBar {
    None,
    /// The active panel's focused row.
    Active,
    /// The other panel's focused row, gray as Windows draws a list's
    /// selection while the list does not have the focus.
    Inactive,
}

/// One of a panel's rows as the window draws it.
pub(crate) struct PanelRow<'a> {
    /// In the order of the list's columns.
    pub(crate) cells: &'a [String],
    pub(crate) right_aligned: &'a [bool],
    pub(crate) icon: Option<(ImageList, i32)>,
    /// `..`, whose folder icon carries an arrow up.
    pub(crate) up: bool,
    pub(crate) bar: RowBar,
    pub(crate) selected: bool,
    /// A hidden or system entry.
    pub(crate) dimmed: bool,
}

/// The fonts of a panel's rows: plain, semibold for a selected row, and
/// bold for a selected row under the bar.
pub(crate) struct RowFonts<'a> {
    pub(crate) plain: &'a Font,
    pub(crate) selected: &'a Font,
    pub(crate) selected_on_bar: &'a Font,
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
    /// # Safety
    /// `notification` is an `NM_CUSTOMDRAW` that a list view sent.
    pub(super) unsafe fn of_list(notification: &Notification) -> Self {
        CustomDraw {
            // SAFETY: a list view sends NM_CUSTOMDRAW with an
            // NMLVCUSTOMDRAW, valid for the notification's lifetime.
            draw: unsafe { &mut *notification.header.cast() },
            list: Hwnd(notification.header().hwndFrom),
        }
    }

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

    /// The row being drawn.
    pub(crate) fn row(&self) -> u64 {
        self.draw.nmcd.dwItemSpec as u64
    }

    /// Draws the whole row at `DrawStage::Row`, in system colors so that
    /// high contrast works (file-manager-spec.md: Selecting entries): the
    /// bar of the focused row, and a selected row's text in the highlight
    /// color, semibold, or bold under the bar. The answer is then
    /// [`DrawAnswer::Drawn`].
    pub(crate) fn draw_panel_row(&self, row: &PanelRow, fonts: &RowFonts) {
        let dc = self.draw.nmcd.hdc;
        let text = match row.bar {
            RowBar::Active => COLOR_HIGHLIGHTTEXT,
            _ if row.selected => COLOR_HIGHLIGHT,
            _ if row.dimmed => COLOR_GRAYTEXT,
            RowBar::Inactive | RowBar::None => COLOR_WINDOWTEXT,
        };
        // The inactive bar is a fill between the window and text colours,
        // or a frame in high contrast.
        let (ground, frame) = match row.bar {
            RowBar::Active => (system_color(COLOR_HIGHLIGHT), false),
            RowBar::Inactive => match inactive_bar_color() {
                Some(fill) => (fill, false),
                None => (system_color(COLOR_WINDOW), true),
            },
            RowBar::None => (system_color(COLOR_WINDOW), false),
        };
        let font = match (row.selected, row.bar) {
            (false, _) => fonts.plain,
            (true, RowBar::None) => fonts.selected,
            (true, RowBar::Active | RowBar::Inactive) => fonts.selected_on_bar,
        };
        let bounds = self.part(0, LVIR_BOUNDS);
        // SAFETY: the list's DC for the length of the message; the font
        // selected into it is restored before the message returns, and
        // system brushes are never freed.
        unsafe {
            SetDCBrushColor(dc, ground);
            FillRect(dc, &bounds, HBRUSH(GetStockObject(DC_BRUSH).0));
            if frame {
                FrameRect(dc, &bounds, GetSysColorBrush(COLOR_WINDOWTEXT));
            }
            if let Some((images, index)) = row.icon {
                let icon = self.part(0, LVIR_ICON);
                images.draw(dc, index, icon);
                if row.up {
                    // Over the folder's front, below its tab.
                    let height = icon.bottom - icon.top;
                    let arrow = RECT {
                        top: icon.top + height / 4,
                        ..icon
                    };
                    let size = Px(height * 5 / 8);
                    draw_lone_glyph(dc, Glyph::Up, arrow, size, system_color(COLOR_WINDOWTEXT));
                }
            }
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, system_color(text));
            let previous = SelectObject(dc, HGDIOBJ(font.handle().0));
            for (column, (cell, right)) in row.cells.iter().zip(row.right_aligned).enumerate() {
                let label = self.part(column as i32, LVIR_LABEL);
                let inset = cell_text_inset(column == 0, self.list.dpi());
                let mut area = RECT {
                    left: label.left + inset.0,
                    right: label.right - inset.0,
                    ..label
                };
                let mut flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
                if *right {
                    flags |= DT_RIGHT;
                }
                draw_text(dc, cell, &mut area, flags);
            }
            SelectObject(dc, previous);
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
        let label = self.cell_part(LVIR_LABEL);
        if let Some((images, index)) = icon.filter(|_| self.draw.iSubItem == 0) {
            let area = self.cell_part(LVIR_ICON);
            images.draw(dc, index, area);
        }
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

    /// Draws the cell at `DrawStage::Cell` as the list would, except that
    /// its text, a path, loses its middle where it does not fit
    /// (`DT_PATH_ELLIPSIS`), so the drive and the name stay. The cell keeps
    /// its whole text, which the list's label tip shows.
    pub(crate) fn draw_path(&self, inset: Px) {
        let label = self.cell_part(LVIR_LABEL);
        let mut area = RECT {
            left: label.left + inset.0,
            right: label.right - inset.0,
            ..label
        };
        let dc = self.draw.nmcd.hdc;
        // SAFETY: the list's DC for the length of the message, which holds
        // the list's font.
        unsafe {
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, self.text_colour());
        }
        draw_text(
            dc,
            &self.cell_text(),
            &mut area,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_PATH_ELLIPSIS,
        );
    }

    /// The part `part`, such as `LVIR_LABEL`, of the cell being drawn.
    fn cell_part(&self, part: u32) -> RECT {
        self.part(self.draw.iSubItem, part)
    }

    /// The part `part` of the drawn row's cell in the column at index
    /// `column`; the first column's `LVIR_BOUNDS` is the whole row.
    fn part(&self, column: i32, part: u32) -> RECT {
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
            // CLR_DEFAULT
            COLORREF(0xFF00_0000) => COLORREF(
                // SAFETY: no parameters.
                unsafe { GetSysColor(COLOR_WINDOWTEXT) },
            ),
            colour => colour,
        }
    }

    /// The text of the cell being drawn, which the list holds.
    fn cell_text(&self) -> String {
        let mut buffer = vec![0u16; 260];
        loop {
            let mut item = LVITEMW {
                iSubItem: self.draw.iSubItem,
                pszText: PWSTR(buffer.as_mut_ptr()),
                cchTextMax: buffer.len() as i32,
                ..Default::default()
            };
            // SAFETY: `item` and the buffer it points to outlive the call,
            // which writes at most `cchTextMax` characters.
            let length = unsafe {
                SendMessageW(
                    self.list.0,
                    LVM_GETITEMTEXTW,
                    Some(WPARAM(self.draw.nmcd.dwItemSpec)),
                    Some(LPARAM(&raw mut item as isize)),
                )
            }
            .0 as usize;
            // A text that fills the buffer may have been cut off.
            if length + 1 < buffer.len() {
                return String::from_utf16_lossy(&buffer[..length]);
            }
            buffer.resize(buffer.len() * 2, 0);
        }
    }
}
