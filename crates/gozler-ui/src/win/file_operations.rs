//! The Win32 parts of the File operations window that the other windows
//! lack (file-manager-spec.md: File operations, step 5): a path line that
//! loses its middle, the queue list in tile view with the line where a
//! dragged row goes, a progress bar's paused state, the card that the
//! window paints behind the running operation, and the progress on the
//! taskbar button.

use super::controls::{ProgressBar, child, send, send_pointer};
use super::gdi::{Icon, IconFont, draw_lone_glyph, glyph_coverage, high_contrast, system_color};
use super::notify::Notification;
use super::{Hwnd, to_rect, wide};
use crate::core::glyph::Glyph;
use crate::core::layout::{Dpi, Px, Rect, Size};
use windows::Win32::Foundation::{LRESULT, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_3DLIGHT, COLOR_BTNFACE, COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetSysColorBrush, HDC, HGDIOBJ,
    MapWindowPoints, PAINTSTRUCT, PS_SOLID, RoundRect, SelectObject,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDIS_HOT, CDIS_SELECTED, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CloseThemeData, DrawThemeBackground, EMF_CENTERED,
    HIMAGELIST, ILC_COLOR32, ImageList_Create, ImageList_Destroy, ImageList_ReplaceIcon,
    LISS_HOTSELECTED, LISS_SELECTED, LIST_VIEW_ITEM_STATE_FLAGS, LV_VIEW_TILE, LVCF_WIDTH,
    LVCOLUMNW, LVHITTESTINFO, LVIF_COLUMNS, LVIF_IMAGE, LVIF_TEXT, LVIR_BOUNDS, LVIR_ICON,
    LVIR_LABEL, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEITEM, LVM_ENSUREVISIBLE,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVM_HITTEST,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SCROLL, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST,
    LVM_SETITEMSTATE, LVM_SETITEMW, LVM_SETTILEVIEWINFO, LVM_SETVIEW, LVN_BEGINDRAG,
    LVN_GETEMPTYMARKUP, LVN_GETINFOTIPW, LVN_ITEMCHANGED, LVN_KEYDOWN, LVNI_SELECTED, LVP_LISTITEM,
    LVS_EX_DOUBLEBUFFER, LVS_EX_INFOTIP, LVS_SHOWSELALWAYS, LVS_SINGLESEL, LVSIL_NORMAL,
    LVTILEVIEWINFO, LVTVIF_FIXEDWIDTH, LVTVIM_COLUMNS, LVTVIM_TILESIZE, NM_CUSTOMDRAW,
    NMCUSTOMDRAW_DRAW_STATE_FLAGS, NMLISTVIEW, NMLVCUSTOMDRAW, NMLVEMPTYMARKUP, NMLVGETINFOTIPW,
    NMLVKEYDOWN, OpenThemeData, PBM_SETSTATE, PBST_NORMAL, PBST_PAUSED, SetWindowTheme,
    WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_DELETE;
use windows::Win32::UI::Shell::{
    ITaskbarList3, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDC_ARROW, IDC_NO, LoadCursorW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetCursor,
    SetWindowPos, WINDOW_EX_STYLE, WM_ERASEBKGND, WS_EX_CLIENTEDGE, WS_TABSTOP,
};
use windows::core::{PCWSTR, PWSTR, w};

/// `WM_ERASEBKGND`, which the window answers itself: it paints its whole
/// client area on `WM_PAINT`, so Windows' erase in the class colour would
/// only flicker.
pub(crate) const ERASE_BACKGROUND: u32 = WM_ERASEBKGND;

/// The message the File operations window posts to its owner after the
/// entries that deletes and moves take changed, or an operation ended.
pub(crate) const TAKEN_CHANGED: u32 = super::wake::WAKE + 5;

/// The message the File operations window posts to its owner after the
/// operation that the owner's status bar shows changed.
pub(crate) const OPERATION_CHANGED: u32 = super::wake::WAKE + 8;

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

/// Paints the window's client area in the dialog colour and `card` on it in
/// the window colour, with a one-pixel border and corners rounded by
/// `corner`, and `icon` in the text colour.
pub(crate) fn paint_card(window: Hwnd, card: Rect, corner: Px, icon: Option<(Glyph, Rect, Px)>) {
    let mut paint = PAINTSTRUCT::default();
    let border = system_color(if high_contrast() {
        COLOR_WINDOWTEXT
    } else {
        COLOR_3DLIGHT
    });
    // SAFETY: BeginPaint and EndPaint pair on the same window and
    // structure. The pen and the brush are this block's own, selected out
    // of the DC before they are deleted; system brushes are never freed.
    unsafe {
        let dc = BeginPaint(window.0, &mut paint);
        FillRect(dc, &paint.rcPaint, GetSysColorBrush(COLOR_BTNFACE));
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
        if let Some((glyph, area, size)) = icon {
            draw_lone_glyph(
                dc,
                glyph,
                to_rect(area),
                size,
                system_color(COLOR_WINDOWTEXT),
            );
        }
        let _ = EndPaint(window.0, &paint);
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
    /// The list paints, and [`QueueList::draw`] answers.
    CustomDraw(ListDraw<'a>),
    Other,
}

/// `NM_CUSTOMDRAW` from the queue list.
pub(crate) struct ListDraw<'a>(&'a mut NMLVCUSTOMDRAW);

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
            WS_TABSTOP.0 | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
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
    /// the text colour, in their order; [`Tile::icon`] indexes them. An
    /// icon the font lacks shows no icon.
    pub(crate) fn set_icons(self, glyphs: &[Glyph], side: Px) {
        let color = system_color(COLOR_WINDOWTEXT);
        // SAFETY: plain values; the list copies each icon, and the list
        // view owns the image list it is given, and destroys it with
        // itself, so only the list it gives back is destroyed here.
        unsafe {
            let images = ImageList_Create(side.0, side.0, ILC_COLOR32, glyphs.len() as i32, 0);
            if images.is_invalid() {
                return;
            }
            IconFont::with(|icons| {
                for &glyph in glyphs {
                    let icon = glyph_coverage(icons, glyph, side)
                        .and_then(|coverage| Icon::colored(&coverage, side, color));
                    match icon {
                        Some(icon) => ImageList_ReplaceIcon(images, -1, icon.0),
                        // A blank image keeps the indexes of the others.
                        None => ImageList_ReplaceIcon(images, -1, Default::default()),
                    };
                }
            });
            let old = send(self.0, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, images.0);
            if old != 0 {
                let _ = ImageList_Destroy(Some(HIMAGELIST(old)));
            }
        }
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

    fn is_selected(self, index: usize) -> bool {
        send(self.0, LVM_GETITEMSTATE, index, LVIS_SELECTED.0 as isize) != 0
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
    /// list's own insertion mark serves tiles side by side, so it would
    /// choose by `point`'s x in this one column.
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

    /// Answers the list's custom draw: the selected row in File Explorer's
    /// light blue across its whole tile, also while the list lacks the
    /// focus, where the list itself would draw it gray; and a line across
    /// the gap `gap`, as [`QueueList::gap_at`] numbers them, over the rows.
    pub(crate) fn draw(self, draw: ListDraw, gap: Option<usize>) -> LRESULT {
        let draw = &mut draw.0.nmcd;
        let stage = draw.dwDrawStage;
        let answer = match (stage, gap) {
            (CDDS_PREPAINT, None) => CDRF_NOTIFYITEMDRAW,
            (CDDS_PREPAINT, Some(_)) => CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT,
            (CDDS_ITEMPREPAINT, _) => {
                let state = draw.uItemState;
                // The item state the list gives here can call an unselected
                // tile selected, so the list is asked.
                if self.is_selected(draw.dwItemSpec)
                    && let Some(rect) = self.item_rect(draw.dwItemSpec, LVIR_BOUNDS)
                    && self.draw_selected(draw.hdc, &rect, state.contains(CDIS_HOT))
                {
                    // Drawn as not selected, the row keeps the fill.
                    draw.uItemState =
                        NMCUSTOMDRAW_DRAW_STATE_FLAGS(state.0 & !(CDIS_SELECTED.0 | CDIS_HOT.0));
                }
                CDRF_DODEFAULT
            }
            (CDDS_POSTPAINT, Some(gap)) => {
                let count = self.count();
                let edge = if gap < count {
                    self.item_rect(gap, LVIR_BOUNDS)
                        .map(|rect| (rect, rect.top))
                } else {
                    count
                        .checked_sub(1)
                        .and_then(|last| self.item_rect(last, LVIR_BOUNDS))
                        .map(|rect| (rect, rect.bottom))
                };
                if let Some((rect, y)) = edge {
                    let thickness = Px(2)
                        .rescale(Dpi::new(96).expect("96 is not zero"), self.0.dpi())
                        .0;
                    let client = self.0.client_size().height.0;
                    let top = (y - thickness / 2).clamp(0, (client - thickness).max(0));
                    let line = RECT {
                        left: rect.left,
                        top,
                        right: rect.right,
                        bottom: top + thickness,
                    };
                    // SAFETY: the list's DC for this paint, and a system
                    // brush, which is never deleted.
                    unsafe { FillRect(draw.hdc, &line, GetSysColorBrush(COLOR_HIGHLIGHT)) };
                }
                CDRF_DODEFAULT
            }
            _ => CDRF_DODEFAULT,
        };
        LRESULT(answer as isize)
    }

    /// Fills `rect` on `dc` as File Explorer's list fills a selected row,
    /// or a selected row under the pointer when `hot`. Returns `false`
    /// without the theme, as in high contrast, where the list's own
    /// selection colours serve.
    fn draw_selected(self, dc: HDC, rect: &RECT, hot: bool) -> bool {
        let state = if hot { LISS_HOTSELECTED } else { LISS_SELECTED };
        // SAFETY: the DC is the paint's, which the list holds for the
        // notification; the theme is closed before the answer.
        unsafe {
            let theme = OpenThemeData(Some(self.0.0), w!("ListView"));
            if theme.is_invalid() {
                return false;
            }
            let drawn = DrawThemeBackground(theme, dc, LVP_LISTITEM.0, state.0, rect, None);
            let _ = CloseThemeData(theme);
            drawn.is_ok()
        }
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
                NM_CUSTOMDRAW => QueueNotify::CustomDraw(ListDraw(&mut *pointer.cast())),
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
