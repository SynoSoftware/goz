use super::{child, scroll_bar_width, send, send_pointer};
use crate::core::layout::{Px, column_sizing::cell_width};
use crate::core::text::Text;
use crate::win::gdi::ImageList;
use crate::win::present::{self, Repaint};
use crate::win::{Hwnd, wide};
use std::ops::Range;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, ScreenToClient};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDHITTESTINFO, HDI_FORMAT, HDITEMW, HDM_GETITEMRECT, HDM_GETITEMW,
    HDM_HITTEST, HDM_SETITEMW, LIST_VIEW_ITEM_STATE_FLAGS, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH,
    LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVHITTESTINFO, LVIR_BOUNDS, LVIR_LABEL, LVIS_DROPHILITED,
    LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETECOLUMN, LVM_ENSUREVISIBLE,
    LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNW, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE,
    LVM_GETHEADER, LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETNEXTITEM,
    LVM_GETSELECTEDCOUNT, LVM_GETSELECTIONMARK, LVM_GETSTRINGWIDTHW, LVM_GETTOPINDEX, LVM_HITTEST,
    LVM_INSERTCOLUMNW, LVM_REDRAWITEMS, LVM_SETCALLBACKMASK, LVM_SETCOLUMNORDERARRAY,
    LVM_SETCOLUMNW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST,
    LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVM_SETSELECTIONMARK, LVN_BEGINDRAG, LVN_ITEMACTIVATE,
    LVNI_FOCUSED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_HEADERDRAGDROP,
    LVS_EX_INFOTIP, LVS_EX_LABELTIP, LVS_OWNERDATA, LVS_REPORT, LVS_SHAREIMAGELISTS,
    LVS_SHOWSELALWAYS, LVSICF_NOSCROLL, LVSIL_SMALL, NM_CLICK, NMHDR, NMITEMACTIVATE, NMLISTVIEW,
    WC_LISTVIEWW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::DragDetect;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetDlgCtrlID, GetWindowLongPtrW, IsWindow, SB_LINEDOWN, SB_LINEUP, WINDOW_EX_STYLE,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NCDESTROY, WM_NOTIFY, WM_VSCROLL, WS_TABSTOP, WS_VSCROLL,
};
use windows::core::PWSTR;

/// The results list: a list view in report and virtual mode, drawn as File
/// Explorer draws its virtual list (DESIGN.md: Look).
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
    pub(crate) fn focus_on_click(self) -> windows::core::Result<()> {
        // SAFETY: this thread owns the control; the stateless subclass is
        // removed when the control is destroyed.
        if unsafe { SetWindowSubclass(self.0.0, Some(cursor_mouse), 0, 0) }.as_bool() {
            Ok(())
        } else {
            Err(windows::core::Error::from_thread())
        }
    }

    pub(crate) fn show_info_tips(self) {
        send(
            self.0,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            LVS_EX_INFOTIP as usize,
            LVS_EX_INFOTIP as isize,
        );
    }

    /// The controls created before the list lie above it, such as the
    /// message in its empty area. It shares the system image list, which it
    /// must not destroy.
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<ListView> {
        let list = child(
            parent,
            WC_LISTVIEWW,
            WS_TABSTOP.0 | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS,
            WINDOW_EX_STYLE(0),
            id,
        )?;
        Self::initialize(list);
        Ok(ListView(list))
    }

    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<ListView> {
        let list = crate::win::dialog::item(dialog, id)?;
        Self::initialize(list);
        Some(ListView(list))
    }

    fn initialize(list: Hwnd) {
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
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    pub(crate) fn insert_column(self, index: usize, column: &ListColumn) {
        self.column_message(LVM_INSERTCOLUMNW, index, column.header, Some(column));
    }

    /// Sets a column's header text and keeps its width and order.
    pub(crate) fn set_header(self, index: usize, header: &str) {
        let text = wide(header);
        let mut shown = vec![0u16; text.len() + 1];
        let mut column = LVCOLUMNW {
            mask: LVCF_TEXT,
            pszText: PWSTR(shown.as_mut_ptr()),
            cchTextMax: shown.len() as i32,
            ..Default::default()
        };
        // SAFETY: `column` points to `shown`, which holds cchTextMax units.
        let read = unsafe { send_pointer(self.0, LVM_GETCOLUMNW, index, &raw mut column) };
        if read != 0 && shown.starts_with(&text) {
            return;
        }
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
        self.hold_after(|| {
            // SAFETY: `item` and the header text it points to outlive the
            // call, which copies them.
            unsafe { send_pointer(self.0, message, index, &raw const item) };
        });
    }

    /// Runs `change`, a column change, after which the list draws its
    /// header and every row it shows at once, so that several column
    /// changes in one update draw the list once ([`present::hold_after`]).
    fn hold_after(self, change: impl FnOnce()) {
        present::hold_after(self.0, Repaint::Buffered, change);
    }

    pub(crate) fn column_width(self, index: usize) -> Px {
        Px(send(self.0, LVM_GETCOLUMNWIDTH, index, 0) as i32)
    }

    pub(crate) fn fitting_text(self, index: usize, text: &Text) -> &str {
        text.fitting(|full| self.text_fits(index, full))
    }

    fn text_fits(self, index: usize, text: &str) -> bool {
        let text = wide(text);
        // SAFETY: the NUL-terminated text outlives the synchronous measurement.
        let width = unsafe { send_pointer(self.0, LVM_GETSTRINGWIDTHW, 0, text.as_ptr()) };
        cell_width(Px(width as i32), Px(0), index == 0, self.0.dpi()) <= self.column_width(index)
    }

    pub(crate) fn set_column_width(self, index: usize, width: Px) {
        self.hold_after(|| {
            send(self.0, LVM_SETCOLUMNWIDTH, index, width.0 as isize);
        });
    }

    /// Gives each column at an index of `widths` its width. A list paints
    /// every row it shows for each width it takes, so a width the column
    /// has already is not set again.
    pub(crate) fn set_column_widths(self, widths: impl IntoIterator<Item = (usize, Px)>) {
        for (index, width) in widths {
            if self.column_width(index) != width {
                self.set_column_width(index, width);
            }
        }
    }

    pub(crate) fn delete_column(self, index: usize) {
        self.hold_after(|| {
            send(self.0, LVM_DELETECOLUMN, index, 0);
        });
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
        Hwnd(HWND(send(self.0, LVM_GETHEADER, 0, 0) as *mut _))
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
                crate::win::present::mark_after(header, || {
                    // SAFETY: as above.
                    unsafe { send_pointer(header, HDM_SETITEMW, index, &raw const item) }
                });
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
    /// position. An unchanged count redraws only the rows in view, because
    /// the full redraw also repaints the frame, and its scroll bars flash. An
    /// empty list redraws whole, because its empty-list text may change.
    pub(crate) fn set_count(self, count: u64) {
        if count != 0 && count == self.count() {
            self.redraw_visible();
            return;
        }
        crate::win::present::mark_after(self.0, || {
            send(
                self.0,
                LVM_SETITEMCOUNT,
                usize::try_from(count).unwrap_or(usize::MAX),
                LVSICF_NOSCROLL as isize,
            )
        });
    }

    pub(crate) fn count(self) -> u64 {
        send(self.0, LVM_GETITEMCOUNT, 0, 0).max(0) as u64
    }

    /// How many rows are selected, in one message.
    pub(crate) fn selected_count(self) -> u64 {
        send(self.0, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64
    }

    /// Scrolls `row` into view, unless it shows whole; a row already in
    /// view leaves the marks to the rows that changed.
    pub(crate) fn ensure_visible(self, row: u64) {
        if self.whole_rows().contains(&row) {
            return;
        }
        crate::win::present::scroll_now(self.0, || {
            send(self.0, LVM_ENSUREVISIBLE, row as usize, 0)
        });
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

    /// Deselects every row with one message, which notifies the window once.
    pub(crate) fn deselect_all(self) {
        self.set_state(usize::MAX, LVIS_SELECTED, LIST_VIEW_ITEM_STATE_FLAGS(0));
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
        self.set_selection_mark(row);
        if let Some(row) = row {
            self.ensure_visible(row);
        }
    }

    /// Restores focus without scrolling or changing the Shift-click anchor.
    pub(crate) fn focus_in_place(self, row: Option<u64>) {
        // A virtual list does not draw again the row that loses the focus
        // to another row, which keeps its focus rectangle on the screen, so
        // that row alone is drawn again. Taking the focus from every row
        // would draw every row again on each cursor key.
        if let Some(old) = self.focused().filter(|&old| Some(old) != row) {
            self.set_state(old as usize, LVIS_FOCUSED, LIST_VIEW_ITEM_STATE_FLAGS(0));
            self.redraw_rows(old..old + 1);
        }
        if let Some(row) = row {
            self.set_state(row as usize, LVIS_FOCUSED, LVIS_FOCUSED);
        }
    }

    pub(crate) fn selection_mark(self) -> Option<u64> {
        u64::try_from(send(self.0, LVM_GETSELECTIONMARK, 0, 0)).ok()
    }

    pub(crate) fn set_selection_mark(self, row: Option<u64>) {
        send(
            self.0,
            LVM_SETSELECTIONMARK,
            0,
            row.map_or(-1, |row| row as isize),
        );
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

    pub(crate) fn next_selected(self, after: Option<u64>) -> Option<u64> {
        u64::try_from(send(
            self.0,
            LVM_GETNEXTITEM,
            after.map_or(usize::MAX, |row| row as usize),
            LVNI_SELECTED as isize,
        ))
        .ok()
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
        crate::win::present::scroll_now(self.0, || send(self.0, WM_VSCROLL, code.0 as usize, 0));
    }

    /// Takes the focus for the row the person left in this list of `count`
    /// rows: the focused row, else the first selected one, else the first
    /// row. Only the focus moves there; the selection stays as the person
    /// made it (DESIGN.md: Selecting entries, window rule 8).
    pub(crate) fn focus_left_row(self, count: u64) {
        self.hwnd().focus();
        let row = self
            .focused_or_first_selected()
            .or((count > 0).then_some(0));
        if row.is_some() {
            self.set_focused(row);
        }
    }

    /// The focused row, or the first selected one without a focused row,
    /// because Select all selects rows without focusing one.
    pub(crate) fn focused_or_first_selected(self) -> Option<u64> {
        [LVNI_FOCUSED, LVNI_SELECTED].into_iter().find_map(|flag| {
            u64::try_from(send(self.0, LVM_GETNEXTITEM, usize::MAX, flag as isize)).ok()
        })
    }

    /// Where a context menu for the focused row
    /// ([`Self::focused_or_first_selected`]) opens, in screen coordinates:
    /// the bottom left of the row, as File Explorer opens it for Shift+F10.
    /// The row is scrolled into view first.
    pub(crate) fn focused_row_point(self) -> Option<(i32, i32)> {
        let row = self.focused_or_first_selected()?;
        self.ensure_visible(row);
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        // SAFETY: `rect` is a local that the list reads the part from and
        // fills in.
        if unsafe { send_pointer(self.0, LVM_GETITEMRECT, row as usize, &raw mut rect) } == 0 {
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

    /// Redraws the rows the list shows now.
    pub(crate) fn redraw_visible(self) {
        self.redraw_rows(self.visible_rows());
    }

    /// The rows the list shows now, a partly shown last row included.
    pub(crate) fn visible_rows(self) -> Range<u64> {
        let whole = self.whole_rows();
        whole.start..whole.end + 1
    }

    /// The rows the list shows whole, which a page key moves through.
    pub(crate) fn whole_rows(self) -> Range<u64> {
        let top = send(self.0, LVM_GETTOPINDEX, 0, 0).max(0) as u64;
        let page = send(self.0, LVM_GETCOUNTPERPAGE, 0, 0).max(0) as u64;
        top..top + page
    }

    /// The rows a column's fit measures: those the list shows and one row
    /// above and below them, which a small scroll brings; and how far the
    /// first column's text starts past its icon on the first of them
    /// (DESIGN.md: Results list).
    pub(crate) fn fit_rows(self) -> (Range<u64>, Px) {
        let shown = self.visible_rows();
        // The shown rows end with a part row, or with the row below them.
        let below = u64::from(self.shows_part_row());
        let rows = shown.start.saturating_sub(1)..(shown.end + below).min(self.count());
        let label = self.first_label_offset(rows.start).unwrap_or_default();
        (rows, label)
    }

    /// Whether the last of [`Self::visible_rows`] shows in part, and is not
    /// the row below the whole rows.
    fn shows_part_row(self) -> bool {
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

    /// The width the columns fill once a resize gives the list the outer
    /// width `width`: [`Self::fill_width`], changed by as much as the
    /// list's width changes.
    pub(crate) fn fill_width_at(self, width: Px) -> Px {
        self.fill_width() + width - self.0.screen_rect().width
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
    fn first_label_offset(self, row: u64) -> Option<Px> {
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

unsafe extern "system" fn cursor_mouse(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // MK_SHIFT and MK_CONTROL leave range selection and toggling to Windows.
    if matches!(message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK)
        && wparam.0 & (MK_SHIFT.0 | MK_CONTROL.0) as usize == 0
    {
        let point = POINT {
            x: lparam.0 as i16 as i32,
            y: (lparam.0 >> 16) as i16 as i32,
        };
        let mut screen = point;
        // SAFETY: the point is local and the message targets this live control.
        let _ = unsafe { ClientToScreen(hwnd, &mut screen) };
        let list = ListView(Hwnd(hwnd));
        list.hwnd().focus();
        list.hwnd().parent().show_focus_cues();
        let row = list.row_at((screen.x, screen.y));
        if let Some(row) = row {
            list.set_focused(Some(row));
        }
        // SAFETY: Windows tracks the gesture using its system drag threshold.
        let drag = row.is_some()
            && message == WM_LBUTTONDOWN
            && unsafe { DragDetect(hwnd, screen) }.as_bool();
        // Drag tracking dispatches messages, including destruction of the list.
        // SAFETY: IsWindow accepts a handle that may have been destroyed.
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return LRESULT(0);
        }
        // SAFETY: the control is still live on its owning thread.
        let id = unsafe { GetDlgCtrlID(hwnd) } as usize;
        let hdr = NMHDR {
            hwndFrom: hwnd,
            idFrom: id,
            code: if drag {
                LVN_BEGINDRAG
            } else if message == WM_LBUTTONDBLCLK && row.is_some() {
                LVN_ITEMACTIVATE
            } else {
                NM_CLICK
            },
        };
        if drag {
            let notification = NMLISTVIEW {
                hdr,
                iItem: row.map_or(-1, |row| row as i32),
                ptAction: point,
                ..Default::default()
            };
            // SAFETY: the notification outlives the synchronous WM_NOTIFY.
            unsafe {
                send_pointer(list.hwnd().parent(), WM_NOTIFY, id, &raw const notification);
            }
        } else {
            let notification = NMITEMACTIVATE {
                hdr,
                iItem: row.map_or(-1, |row| row as i32),
                ptAction: point,
                ..Default::default()
            };
            // SAFETY: the notification outlives the synchronous WM_NOTIFY.
            unsafe {
                send_pointer(list.hwnd().parent(), WM_NOTIFY, id, &raw const notification);
            }
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        // SAFETY: this control owns the stateless subclass until destruction.
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(cursor_mouse), 0) };
    }
    // SAFETY: the original parameters are forwarded unchanged.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::notify::ListNotify;
    use crate::win::window::{self, Background, Message, WindowProc};
    use std::cell::Cell;
    use std::rc::Rc;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Controls::HDI_WIDTH;

    struct HeaderProbe {
        list: Cell<Option<ListView>>,
        header: Text,
    }

    impl WindowProc for HeaderProbe {
        fn message(&self, _: Hwnd, message: Message<'_>) -> Option<LRESULT> {
            if let Message::Notify(notification) = message {
                let list = self.list.get()?;
                if let Some(ListNotify::ColumnWidthChanged(1)) = list.notification(notification) {
                    list.set_header(1, list.fitting_text(1, &self.header));
                }
            }
            None
        }
    }

    fn header_text(list: ListView) -> String {
        let mut buffer = [0u16; 64];
        let mut column = LVCOLUMNW {
            mask: LVCF_TEXT,
            pszText: PWSTR(buffer.as_mut_ptr()),
            cchTextMax: buffer.len() as i32,
            ..Default::default()
        };
        // SAFETY: the item points to the local buffer and gives its capacity.
        unsafe { send_pointer(list.hwnd(), LVM_GETCOLUMNW, 1, &raw mut column) };
        let length = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    }

    #[test]
    fn a_column_resize_reselects_the_full_or_short_header() {
        crate::win::controls::initialize();
        let class = "gozler.test.short-header";
        window::register_class::<HeaderProbe>(class, 0, Background::Window).unwrap();
        let probe = Rc::new(HeaderProbe {
            list: Cell::new(None),
            header: Text {
                full: "Date modified".to_owned(),
                short: Some("Modified".to_owned()),
            },
        });
        let hwnd = window::create_window(class, Rc::clone(&probe), None).unwrap();
        let list = ListView::create(hwnd, 1).unwrap();
        probe.list.set(Some(list));
        for index in 0..2 {
            list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: Px(100),
                    right_aligned: false,
                },
            );
        }
        let full = wide(&probe.header.full);
        // SAFETY: the NUL-terminated text outlives the synchronous measurement.
        let width = unsafe { send_pointer(list.hwnd(), LVM_GETSTRINGWIDTHW, 0, full.as_ptr()) };
        let needed = cell_width(Px(width as i32), Px(0), false, list.hwnd().dpi());
        let shown = [false, true].map(|from_header| {
            [needed, needed - Px(1), needed].map(|width| {
                if from_header {
                    let item = HDITEMW {
                        mask: HDI_WIDTH,
                        cxy: width.0,
                        ..Default::default()
                    };
                    // SAFETY: the header copies the local item's width.
                    unsafe { send_pointer(list.header(), HDM_SETITEMW, 1, &raw const item) };
                } else {
                    list.set_column_width(1, width);
                }
                header_text(list)
            })
        });
        hwnd.destroy();
        for shown in shown {
            assert_eq!(shown, ["Date modified", "Modified", "Date modified"]);
        }
    }
}
