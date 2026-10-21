//! The Win32 parts of the File operations window that the other windows
//! lack (file-manager-spec.md: File operations, step 5): a path line that
//! loses its middle, the queue list in tile view with the line where a
//! dragged row goes, a progress bar's paused state, the card that the
//! window paints behind the running operation, and the progress on the
//! taskbar button.

use super::controls::{ProgressBar, child, send, send_pointer};
use super::gdi::{EraseDc, glyph_list, high_contrast, system_color};
use super::notify::Notification;
use super::{Hwnd, to_rect, wide};
use crate::core::glyph::Glyph;
use crate::core::layout::{Px, Rect, Size};
use windows::Win32::Foundation::{LRESULT, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    COLOR_3DLIGHT, COLOR_BTNFACE, COLOR_WINDOW, COLOR_WINDOWTEXT, CreatePen, CreateSolidBrush,
    DeleteObject, FillRect, GetSysColorBrush, HGDIOBJ, MapWindowPoints, PS_SOLID, RoundRect,
    SelectObject,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Controls::{
    EMF_CENTERED, LIST_VIEW_ITEM_STATE_FLAGS, LV_VIEW_TILE, LVCF_WIDTH, LVCOLUMNW, LVHITTESTINFO,
    LVIF_COLUMNS, LVIF_IMAGE, LVIF_TEXT, LVIM_AFTER, LVINSERTMARK, LVIR_BOUNDS, LVIR_ICON,
    LVIR_LABEL, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEITEM, LVM_ENSUREVISIBLE,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETNEXTITEM, LVM_HITTEST, LVM_INSERTCOLUMNW,
    LVM_INSERTITEMW, LVM_SCROLL, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SETINSERTMARK,
    LVM_SETITEMSTATE, LVM_SETITEMW, LVM_SETTILEVIEWINFO, LVM_SETVIEW, LVN_BEGINDRAG,
    LVN_GETEMPTYMARKUP, LVN_GETINFOTIPW, LVN_ITEMCHANGED, LVN_KEYDOWN, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_INFOTIP, LVS_SHAREIMAGELISTS, LVS_SHOWSELALWAYS, LVS_SINGLESEL,
    LVSIL_NORMAL, LVTILEVIEWINFO, LVTVIF_FIXEDWIDTH, LVTVIM_COLUMNS, LVTVIM_TILESIZE, NMLISTVIEW,
    NMLVEMPTYMARKUP, NMLVGETINFOTIPW, NMLVKEYDOWN, PBM_SETSTATE, PBST_NORMAL, PBST_PAUSED,
    SetWindowTheme, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_DELETE;
use windows::Win32::UI::Shell::{
    ITaskbarList3, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDC_ARROW, IDC_NO, LoadCursorW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetCursor,
    SetWindowPos, WINDOW_EX_STYLE, WS_EX_CLIENTEDGE, WS_TABSTOP,
};
use windows::core::{PCWSTR, PWSTR, w};

/// The message the File operations window posts to its owner after the
/// entries that deletes and moves take changed.
pub(crate) const TAKEN_CHANGED: u32 = super::wake::WAKE + 5;

/// One line of a path that loses its middle when it is too long
/// (`SS_PATHELLIPSIS`), so its drive and its last part stay. It notifies its
/// parent of clicks (`SS_NOTIFY`), without which it lets the mouse through
/// and its tooltip never shows.
#[derive(Clone, Copy)]
pub(crate) struct PathLabel(Hwnd);

impl PathLabel {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<PathLabel> {
        const SS_NOTIFY: u32 = 0x100;
        const SS_NOPREFIX: u32 = 0x80;
        const SS_PATHELLIPSIS: u32 = 0x8000;
        child(
            parent,
            w!("STATIC"),
            SS_NOTIFY | SS_NOPREFIX | SS_PATHELLIPSIS,
            WINDOW_EX_STYLE(0),
            id,
        )
        .map(PathLabel)
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }
}

/// Shows a measuring bar yellow while its operation is paused, as File
/// Explorer's copy shows a paused copy.
pub(crate) fn show_paused(bar: ProgressBar, paused: bool) {
    let state = if paused { PBST_PAUSED } else { PBST_NORMAL };
    send(bar.hwnd(), PBM_SETSTATE, state as usize, 0);
}

/// Paints the background of `window` on `dc`, as `WM_ERASEBKGND` asks: the
/// dialog colour, and `card` on it in the window colour, with a one-pixel
/// border and corners rounded by `corner`. A themed button asks for it too,
/// to paint its corners in the colour it stands on.
pub(crate) fn paint_card(window: Hwnd, dc: EraseDc, card: Rect, corner: Px) {
    let EraseDc(dc) = dc;
    let client = window.client_size();
    let ground = to_rect(Rect {
        x: Px(0),
        y: Px(0),
        width: client.width,
        height: client.height,
    });
    let border = system_color(if high_contrast() {
        COLOR_WINDOWTEXT
    } else {
        COLOR_3DLIGHT
    });
    // SAFETY: the DC is the message's, which Windows holds for it. The pen
    // and the brush are this block's own, selected out of the DC before
    // they are deleted; system brushes are never freed.
    unsafe {
        FillRect(dc, &ground, GetSysColorBrush(COLOR_BTNFACE));
        let pen = CreatePen(PS_SOLID, 1, border);
        let brush = CreateSolidBrush(system_color(COLOR_WINDOW));
        let old_pen = SelectObject(dc, HGDIOBJ(pen.0));
        let old_brush = SelectObject(dc, HGDIOBJ(brush.0));
        let rect = to_rect(card);
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            corner.0,
            corner.0,
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// The size of `window`, a top-level window without a menu bar, whose
/// client area is `client`.
pub(crate) fn window_size(window: Hwnd, client: Size) -> Size {
    let mut rect = to_rect(Rect {
        x: Px(0),
        y: Px(0),
        width: client.width,
        height: client.height,
    });
    let (style, ex_style) = window.styles();
    // SAFETY: `rect` is a local the call adjusts in place.
    if unsafe { AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, window.dpi().get()) }
        .is_err()
    {
        return client;
    }
    Size {
        width: Px(rect.right - rect.left),
        height: Px(rect.bottom - rect.top),
    }
}

/// Sizes `window`, a top-level window without a menu bar, so its client
/// area is `client`, and keeps its position.
pub(crate) fn set_client_size(window: Hwnd, client: Size) {
    let size = window_size(window, client);
    // SAFETY: plain values; a stale handle only fails.
    let _ = unsafe {
        SetWindowPos(
            window.0,
            None,
            0,
            0,
            size.width.0,
            size.height.0,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Shows the cursor of a drag: the arrow, or the "not allowed" cursor over
/// a place that refuses the drop.
pub(crate) fn show_drag_cursor(refused: bool) {
    // SAFETY: both are predefined cursors, which are never destroyed.
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, if refused { IDC_NO } else { IDC_ARROW }) {
            SetCursor(Some(cursor));
        }
    }
}

/// The queue: a list view in tile view, whose rows show an icon, a title
/// and one line of detail, and where a line between rows shows where a
/// dragged row would go. Only [`QueueList::create`] makes one, so its
/// handle is a list view's, which decoding its notifications relies on.
#[derive(Clone, Copy)]
pub(crate) struct QueueList(Hwnd);

/// One row of the queue list.
pub(crate) struct Tile<'a> {
    /// The index of its icon among those [`QueueList::set_icons`] gave.
    pub(crate) icon: usize,
    pub(crate) title: &'a str,
    pub(crate) detail: &'a str,
}

/// What the queue list told its parent.
pub(crate) enum QueueNotify<'a> {
    /// The selected row changed.
    SelectionChanged,
    /// The person started dragging a row with the left button.
    BeginDrag,
    /// `Delete` was pressed in the list.
    DeleteKey,
    /// The list is empty and asks what to say instead.
    EmptyText(EmptyText<'a>),
    /// The pointer rests on a row, and the list asks for its tooltip.
    InfoTip(InfoTip<'a>),
    Other,
}

/// `LVN_GETINFOTIPW`: the tooltip of the row under the pointer.
pub(crate) struct InfoTip<'a>(&'a mut NMLVGETINFOTIPW);

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

/// `LVN_GETEMPTYMARKUP`: the text an empty list shows.
pub(crate) struct EmptyText<'a>(&'a mut NMLVEMPTYMARKUP);

impl EmptyText<'_> {
    /// Answers with `text`, centred, and returns the notification's answer.
    pub(crate) fn set(self, text: &str) -> LRESULT {
        let units: Vec<u16> = text.encode_utf16().collect();
        let room = self.0.szMarkup.len() - 1;
        let kept = units.len().min(room);
        self.0.szMarkup[..kept].copy_from_slice(&units[..kept]);
        self.0.szMarkup[kept] = 0;
        self.0.dwFlags = EMF_CENTERED;
        LRESULT(1)
    }
}

impl QueueList {
    pub(crate) fn create(parent: Hwnd, id: u16) -> windows::core::Result<QueueList> {
        let list = child(
            parent,
            WC_LISTVIEWW,
            WS_TABSTOP.0 | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS,
            WS_EX_CLIENTEDGE,
            id,
        )?;
        let extended = LVS_EX_DOUBLEBUFFER | LVS_EX_INFOTIP;
        send(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(list.0, w!("Explorer"), PCWSTR::null()) };
        send(list, LVM_SETVIEW, LV_VIEW_TILE as usize, 0);
        // A tile shows its item's text as its title and the columns its
        // item names as its further lines, so the list needs a second
        // column to hold the detail.
        for index in 0..2 {
            let column = LVCOLUMNW {
                mask: LVCF_WIDTH,
                cx: 100,
                ..Default::default()
            };
            // SAFETY: `column` outlives the call, which copies it.
            unsafe { send_pointer(list, LVM_INSERTCOLUMNW, index, &raw const column) };
        }
        Ok(QueueList(list))
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Makes each tile `width` wide, the list's width less its scroll bar,
    /// so the tiles stack in one column, with one line of detail under the
    /// title.
    pub(crate) fn set_tile_width(self, width: Px) {
        let info = LVTILEVIEWINFO {
            cbSize: size_of::<LVTILEVIEWINFO>() as u32,
            dwMask: LVTVIM_TILESIZE | LVTVIM_COLUMNS,
            dwFlags: LVTVIF_FIXEDWIDTH,
            sizeTile: SIZE {
                cx: width.0.max(1),
                cy: 0,
            },
            cLines: 1,
            ..Default::default()
        };
        // SAFETY: `info` outlives the call, which copies it.
        unsafe { send_pointer(self.0, LVM_SETTILEVIEWINFO, 0, &raw const info) };
    }

    /// How wide a tile's lines of text can be: from where the first tile's
    /// text starts to its end, less the room before its icon. `None` while
    /// the list holds no tile.
    pub(crate) fn text_width(self) -> Option<Px> {
        let part = |part| self.item_rect(0, part);
        let (bounds, icon, label) = (part(LVIR_BOUNDS)?, part(LVIR_ICON)?, part(LVIR_LABEL)?);
        Some(Px(bounds.right - (icon.left - bounds.left) - label.left))
    }

    /// The part `part`, an `LVIR_` value, of the row `index`, in the list's
    /// client coordinates.
    fn item_rect(self, index: usize, part: u32) -> Option<RECT> {
        let mut rect = RECT {
            left: part as i32,
            ..Default::default()
        };
        // SAFETY: `rect` is a local the call reads and fills in.
        let found = unsafe { send_pointer(self.0, LVM_GETITEMRECT, index, &raw mut rect) } != 0;
        found.then_some(rect)
    }

    fn count(self) -> usize {
        usize::try_from(send(self.0, LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0)
    }

    /// Gives the tiles `glyphs` as their icons, `side` pixels square, in
    /// the text colour of now, in their order; [`Tile::icon`] indexes them.
    /// An icon the font lacks shows no icon.
    pub(crate) fn set_icons(self, glyphs: &[Glyph], side: Px) {
        let images = glyph_list(glyphs, side, system_color(COLOR_WINDOWTEXT)).unwrap_or_default();
        // The list shares the image list, which `glyph_list` keeps alive.
        send(self.0, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, images.0);
    }

    /// Shows `tiles` in their order, with the row `selected` selected and
    /// focused. Rows are changed in place, so the list keeps its scroll
    /// position.
    pub(crate) fn set_tiles(self, tiles: &[Tile], selected: Option<usize>) {
        let count = usize::try_from(send(self.0, LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0);
        for index in (tiles.len()..count).rev() {
            send(self.0, LVM_DELETEITEM, index, 0);
        }
        let detail_column = [1u32];
        for (index, tile) in tiles.iter().enumerate() {
            let mut title = wide(tile.title);
            let mut item = LVITEMW {
                mask: LVIF_TEXT | LVIF_IMAGE | LVIF_COLUMNS,
                iItem: index as i32,
                pszText: PWSTR(title.as_mut_ptr()),
                iImage: tile.icon as i32,
                cColumns: 1,
                puColumns: detail_column.as_ptr().cast_mut(),
                ..Default::default()
            };
            let message = if index < count {
                LVM_SETITEMW
            } else {
                LVM_INSERTITEMW
            };
            // SAFETY: `item`, its text and its column list outlive the
            // call, which copies them.
            unsafe { send_pointer(self.0, message, 0, &raw const item) };
            let mut detail = wide(tile.detail);
            item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: index as i32,
                iSubItem: 1,
                pszText: PWSTR(detail.as_mut_ptr()),
                ..Default::default()
            };
            // SAFETY: as above.
            unsafe { send_pointer(self.0, LVM_SETITEMW, 0, &raw const item) };
        }
        for index in 0..tiles.len() {
            self.set_selected(index, Some(index) == selected);
        }
    }

    fn set_selected(self, index: usize, selected: bool) {
        let mask = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        let item = LVITEMW {
            stateMask: mask,
            state: if selected {
                mask
            } else {
                LIST_VIEW_ITEM_STATE_FLAGS(0)
            },
            ..Default::default()
        };
        // SAFETY: `item` outlives the call, which reads it.
        unsafe { send_pointer(self.0, LVM_SETITEMSTATE, index, &raw const item) };
    }

    /// The selected row, if any.
    pub(crate) fn selected(self) -> Option<usize> {
        usize::try_from(send(
            self.0,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        ))
        .ok()
    }

    /// Scrolls the row `index` into view.
    pub(crate) fn ensure_visible(self, index: usize) {
        send(self.0, LVM_ENSUREVISIBLE, index, 0);
    }

    /// The row at `point`, in screen coordinates, as a context menu gives
    /// it.
    pub(crate) fn tile_at(self, (x, y): (i32, i32)) -> Option<usize> {
        let mut point = [POINT { x, y }];
        // SAFETY: `point` is a local the call converts in place.
        unsafe { MapWindowPoints(None, Some(self.0.0), &mut point) };
        let mut info = LVHITTESTINFO {
            pt: point[0],
            ..Default::default()
        };
        // SAFETY: `info` is a local the call fills in.
        let hit = unsafe { send_pointer(self.0, LVM_HITTEST, 0, &raw mut info) };
        usize::try_from(hit).ok()
    }

    /// The middle of the row `index`'s left part, in screen coordinates,
    /// where a context menu opened with the keyboard appears.
    pub(crate) fn menu_point(self, index: usize) -> Option<(i32, i32)> {
        let rect = self.item_rect(index, LVIR_BOUNDS)?;
        let mut point = [POINT {
            x: rect.left + (rect.bottom - rect.top) / 2,
            y: (rect.top + rect.bottom) / 2,
        }];
        // SAFETY: `point` is a local the call converts in place.
        unsafe { MapWindowPoints(Some(self.0.0), None, &mut point) };
        Some((point[0].x, point[0].y))
    }

    /// The gap nearest `point`, in the client coordinates of the list's
    /// parent: 0 before the first row, the row count after the last. The
    /// list's own hit test for its insertion mark serves tiles side by
    /// side, so it would choose by `point`'s x in this one column.
    pub(crate) fn gap_at(self, point: (i32, i32)) -> Option<usize> {
        let y = self.client_point(point).y;
        let count = self.count();
        (count > 0).then(|| {
            (0..count)
                .find(|&row| {
                    self.item_rect(row, LVIR_BOUNDS)
                        .is_some_and(|rect| y < (rect.top + rect.bottom) / 2)
                })
                .unwrap_or(count)
        })
    }

    /// Shows the list's insertion mark at the gap `gap`, as
    /// [`QueueList::gap_at`] numbers them, or none.
    pub(crate) fn set_insert_mark(self, gap: Option<usize>) {
        let count = self.count();
        let (item, flags) = match gap {
            Some(gap) if gap < count => (gap as i32, 0),
            // After the last row.
            Some(_) if count > 0 => ((count - 1) as i32, LVIM_AFTER),
            _ => (-1, 0),
        };
        let mark = LVINSERTMARK {
            cbSize: size_of::<LVINSERTMARK>() as u32,
            dwFlags: flags,
            iItem: item,
            dwReserved: 0,
        };
        // SAFETY: `mark` outlives the call, which reads it.
        unsafe { send_pointer(self.0, LVM_SETINSERTMARK, 0, &raw const mark) };
    }

    /// Scrolls the list one row toward `point`, in the client coordinates
    /// of the list's parent, while it is above or below the list, as a list
    /// scrolls while something is dragged past its edge. Returns whether it
    /// is.
    pub(crate) fn scroll_toward(self, point: (i32, i32)) -> bool {
        let y = self.client_point(point).y;
        let row = self
            .item_rect(0, LVIR_BOUNDS)
            .map_or(0, |rect| rect.bottom - rect.top);
        let dy = if y < 0 {
            -row
        } else if y >= self.0.client_size().height.0 {
            row
        } else {
            return false;
        };
        // In pixels in tile view.
        send(self.0, LVM_SCROLL, 0, dy as isize);
        true
    }

    /// `point`, in its parent's client coordinates, in the list's own.
    fn client_point(self, (x, y): (i32, i32)) -> POINT {
        let mut point = [POINT { x, y }];
        // SAFETY: `point` is a local the call converts in place; the list's
        // parent is the window whose client coordinates `point` is in.
        unsafe { MapWindowPoints(Some(self.0.parent().0), Some(self.0.0), &mut point) };
        point[0]
    }

    /// `notification` decoded, if this list sent it.
    pub(crate) fn notification<'a>(
        self,
        notification: Notification<'a>,
    ) -> Option<QueueNotify<'a>> {
        let header = notification.header();
        if header.hwndFrom != self.0.0 {
            return None;
        }
        let pointer = notification.header;
        // SAFETY: the sender is this list view, which `QueueList::create`
        // made with the list view class, and each structure is read only
        // for the code with which a list view sends it, valid for the
        // notification's lifetime.
        let decoded = unsafe {
            match header.code {
                LVN_ITEMCHANGED => {
                    let change = &*pointer.cast::<NMLISTVIEW>();
                    if (change.uNewState ^ change.uOldState) & LVIS_SELECTED.0 != 0 {
                        QueueNotify::SelectionChanged
                    } else {
                        QueueNotify::Other
                    }
                }
                LVN_BEGINDRAG => QueueNotify::BeginDrag,
                LVN_KEYDOWN => {
                    let key = &*pointer.cast::<NMLVKEYDOWN>();
                    if key.wVKey == VK_DELETE.0 {
                        QueueNotify::DeleteKey
                    } else {
                        QueueNotify::Other
                    }
                }
                LVN_GETEMPTYMARKUP => QueueNotify::EmptyText(EmptyText(&mut *pointer.cast())),
                LVN_GETINFOTIPW => QueueNotify::InfoTip(InfoTip(&mut *pointer.cast())),
                _ => QueueNotify::Other,
            }
        };
        Some(decoded)
    }
}

/// The progress of every file operation on the taskbar button of a window,
/// as File Explorer shows its copies. Without the taskbar's interface,
/// such as before Explorer runs, it shows nothing.
pub(crate) struct TaskbarProgress {
    window: Hwnd,
    taskbar: Option<ITaskbarList3>,
}

/// What the taskbar button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskbarState {
    None,
    /// `done` of `total`.
    Measured {
        done: u64,
        total: u64,
    },
    /// Moving, with no total, as a delete runs.
    Moving,
    Paused {
        done: u64,
        total: u64,
    },
}

impl TaskbarProgress {
    pub(crate) fn new(window: Hwnd) -> TaskbarProgress {
        // SAFETY: COM is initialized on this thread at start; the instance
        // is released when it drops.
        let taskbar: Option<ITaskbarList3> =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok();
        // SAFETY: HrInit is the call the interface needs before any other.
        let taskbar = taskbar.filter(|taskbar| unsafe { taskbar.HrInit() }.is_ok());
        TaskbarProgress { window, taskbar }
    }

    pub(crate) fn show(&self, state: TaskbarState) {
        let Some(taskbar) = &self.taskbar else {
            return;
        };
        let hwnd = self.window.0;
        // SAFETY: plain values on a window of this process; a failure
        // leaves the button as it was.
        unsafe {
            let _ = match state {
                TaskbarState::None => taskbar.SetProgressState(hwnd, TBPF_NOPROGRESS),
                TaskbarState::Moving => taskbar.SetProgressState(hwnd, TBPF_INDETERMINATE),
                TaskbarState::Measured { done, total } => taskbar
                    .SetProgressState(hwnd, TBPF_NORMAL)
                    .and_then(|()| taskbar.SetProgressValue(hwnd, done, total.max(1))),
                TaskbarState::Paused { done, total } => taskbar
                    .SetProgressState(hwnd, TBPF_PAUSED)
                    .and_then(|()| taskbar.SetProgressValue(hwnd, done, total.max(1))),
            };
        }
    }
}
