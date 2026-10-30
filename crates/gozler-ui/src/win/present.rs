//! The presenter, the one owner of when the pixels of the thread's windows
//! appear: the main window, the Viewers and the dialogs. Each control
//! paints into its window's surface on its own, and the screen shows that
//! surface at every frame, so controls that paint one by one show the
//! window in pieces. So a change only marks the windows it changes: the
//! setters through [`mark_after`], and placements through
//! [`super::place_children`]. When the outermost update ends, the presenter
//! paints what was marked in one pass. A pass of several of the main
//! window's controls takes longer than a frame, so the cover, a copy of the
//! old picture in a window of its own, hides the pass until the main
//! window's surface is finished. `core::present` decides where an
//! update starts and ends; [`enter`] runs it for each message of the window
//! procedures of this thread.
//!
//! Each update that marked a window goes to `ui-frames.log` through the
//! watchdog's thread ([`log_frames`]), so the window's thread never writes
//! the file.

use super::window::DEFERRED;
use super::{Hwnd, from_rect};
use crate::core::layout::{Dpi, Px};
use crate::core::present::{Call, Cause, Cover, Frame, Marked, Start, Updates};
use std::cell::{Cell, RefCell};
use std::sync::mpsc::{SyncSender, TrySendError};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CombineRgn, CreateCompatibleDC,
    CreateDIBSection, CreateRectRgn, CreateRoundRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GdiFlush, GetDC, GetUpdateRect, GetUpdateRgn, HBITMAP, HGDIOBJ, InvalidateRect,
    MapWindowPoints, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
    REDRAW_WINDOW_FLAGS, RGN_DIFF, RedrawWindow, ReleaseDC, SRCCOPY, SelectObject, SetWindowRgn,
    UpdateWindow,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::{
    CDDS_PREPAINT, LVN_GETDISPINFOW, LVN_GETEMPTYMARKUP, LVN_ODCACHEHINT, LVN_ODFINDITEMW,
    NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, STATE_SYSTEM_INVISIBLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, EnumThreadWindows, GA_ROOT, GW_HWNDPREV, GetAncestor, GetClientRect,
    GetQueueStatus, GetScrollBarInfo, GetWindow, GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST,
    IMAGE_BITMAP, OBJID_HSCROLL, OBJID_VSCROLL, QS_INPUT, QS_PAINT, QS_POSTMESSAGE,
    QUEUE_STATUS_FLAGS, SCROLLBARINFO, SET_WINDOW_POS_FLAGS, STM_SETIMAGE, SWP_HIDEWINDOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOREDRAW, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SendMessageW, SetWindowPos, WINDOW_STYLE, WM_CTLCOLORMSGBOX, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITMENULOOP, WM_EXITSIZEMOVE,
    WM_GETMINMAXINFO, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCPAINT, WM_NOTIFY,
    WM_PRINTCLIENT, WM_SETCURSOR, WM_SETREDRAW, WM_SYNCPAINT, WM_WINDOWPOSCHANGING, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_HSCROLL, WS_POPUP, WS_VISIBLE,
    WS_VSCROLL,
};
use windows::core::{BOOL, PCWSTR, w};

thread_local! {
    static UPDATES: RefCell<Updates> = RefCell::new(Updates::default());
    /// The window whose updates are published, until its `WM_DESTROY`.
    static MAIN: Cell<Option<Main>> = const { Cell::new(None) };
    static FRAMES: RefCell<Option<FrameLog>> = const { RefCell::new(None) };
    static DEFERRED_PAINT: Cell<DeferredPaint> = const {
        Cell::new(DeferredPaint { since: None, present: Present::Idle })
    };
    /// The buffer of `find_marked`, kept so that a message allocates
    /// nothing for it.
    static WINDOWS: RefCell<Vec<Hwnd>> = const { RefCell::new(Vec::new()) };
    static OPEN: RefCell<OpenUpdate> = const {
        RefCell::new(OpenUpdate { early: Vec::new(), raised: None, held: Vec::new() })
    };
}

#[derive(Clone, Copy)]
struct Main {
    window: Hwnd,
    /// The cover ([`create_cover`]), when Windows made it.
    cover: Option<Hwnd>,
    /// The client's size at the last pass that wanted the cover.
    covered_size: Option<(i32, i32)>,
}

struct FrameLog {
    frames: SyncSender<Frame>,
    /// The records that found the channel full since the last one sent.
    dropped: usize,
}

/// What the open update did before its paint pass, which [`finish`] takes.
#[derive(Default)]
struct OpenUpdate {
    /// The windows that painted during the update's changes.
    early: Vec<Marked>,
    /// The cover, when [`cover_now`] showed it.
    raised: Option<Hwnd>,
    /// The controls whose drawing [`hold_after`] turned off.
    held: Vec<Held>,
}

#[derive(Clone, Copy)]
struct Held {
    control: Hwnd,
    repaint: Repaint,
}

/// How a held control repaints when its hold ends.
#[derive(Clone, Copy)]
pub(crate) enum Repaint {
    /// Over its own buffer, as a list view with `LVS_EX_DOUBLEBUFFER`
    /// does, so erasing first would flicker.
    Buffered,
    /// After its background is erased.
    Erased,
}

/// The message the presenter posts to the main window when an update
/// leaves marks unpainted; the update it opens paints them ([`finish`]).
pub(crate) const PRESENT: u32 = super::wake::WAKE + 11;

/// Names the main window, whose marked windows the presenter publishes,
/// and makes its cover.
pub(crate) fn set_main_window(main: Hwnd) {
    MAIN.set(Some(Main {
        window: main,
        cover: create_cover(main),
        covered_size: None,
    }));
}

/// Makes the cover: a static control as a popup that `main` owns, which
/// shows a copy of the main window's client while a paint pass runs
/// ([`raise_cover`]). It stays hidden and cloaked between passes, so the
/// watchdog does not count it and it needs nothing when the main window
/// hides. It never takes the activation, and the mouse goes through it,
/// because a static control without `SS_NOTIFY` answers `WM_NCHITTEST`
/// with `HTTRANSPARENT`. Without a cover, each pass paints in sight.
fn create_cover(main: Hwnd) -> Option<Hwnd> {
    const SS_BITMAP: u32 = 0xE;
    // SAFETY: a top-level window of the system's static class, owned by
    // `main`, which destroys it.
    let cover = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("STATIC"),
            PCWSTR::null(),
            WS_POPUP | WINDOW_STYLE(SS_BITMAP),
            0,
            0,
            0,
            0,
            Some(main.0),
            None,
            None,
            None,
        )
    }
    .ok()
    .map(Hwnd)?;
    // A transition would fade the cover in and out.
    set_flag(cover, DWMWA_TRANSITIONS_FORCEDISABLED, true)?;
    set_flag(cover, DWMWA_CLOAK, true)?;
    // Windows 11 rounds a popup's corners and then draws a border and a
    // shadow around it. The cover's top edge lies just under the menu bar,
    // so they showed as a line there each time the cover went up. The
    // cover's region rounds its bottom corners instead ([`shape_cover`]).
    // Windows 10 knows no such attribute, and draws neither.
    let square = DWMWCP_DONOTROUND;
    // SAFETY: the value is the attribute's enum and outlives the call.
    let _ = unsafe {
        DwmSetWindowAttribute(
            cover.0,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const square).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    Some(cover)
}

/// Sets the DWM attribute `attribute`, a `BOOL`, of `window`.
fn set_flag(window: Hwnd, attribute: DWMWINDOWATTRIBUTE, on: bool) -> Option<()> {
    let value = BOOL::from(on);
    // SAFETY: the value is a BOOL that outlives the call, as the attribute
    // asks.
    unsafe {
        DwmSetWindowAttribute(
            window.0,
            attribute,
            (&raw const value).cast(),
            size_of::<BOOL>() as u32,
        )
    }
    .ok()
}

/// Sends a record of each update that marked a window to `frames`, whose
/// receiver writes `ui-frames.log`. A send never waits: a full channel
/// counts the record as dropped, and the next record carries the count.
pub(crate) fn log_frames(frames: SyncSender<Frame>) {
    FRAMES.set(Some(FrameLog { frames, dropped: 0 }));
}

/// How many [`Frame`] ticks make a second.
pub(crate) fn ticks_per_second() -> i64 {
    let mut frequency: i64 = 0;
    // SAFETY: the call writes one integer through a valid pointer.
    let _ = unsafe { QueryPerformanceFrequency(&mut frequency) };
    frequency
}

fn ticks() -> i64 {
    let mut ticks: i64 = 0;
    // SAFETY: the call writes one integer through a valid pointer.
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    ticks
}

fn start(cause: Cause) -> Start {
    Start {
        cause,
        ticks: ticks(),
        cycles: gozler_winfs::thread_cycles(),
    }
}

/// One message of a window procedure, from [`enter`] until the procedure
/// returns and this drops, which ends the update the message opened.
#[must_use = "retain the scope until the procedure returns"]
pub(crate) struct Scope(Option<Call>);

/// Starts `message` of `window` among the updates, and starts or ends the
/// level of a modal loop that `DefWindowProc` runs: the menu bar's, the
/// window menu's and a drag of the window's frame.
///
/// # Safety
/// `lparam` is the message's own; a notification's header remains readable
/// while its handler runs.
pub(super) unsafe fn enter(window: HWND, message: u32, lparam: LPARAM) -> Scope {
    // SAFETY: the caller's promise.
    let opens = unsafe { opens_update(message, lparam) };
    let call =
        UPDATES.with_borrow_mut(|updates| updates.enter(opens, || start(Cause::Message(message))));
    // The cover asks its owner for colours too, and paints only itself.
    // SAFETY: the caller's promise.
    if let Some(control) = unsafe { painting(message, lparam) }
        && MAIN.get().and_then(|main| main.cover) != Some(control)
        && UPDATES.with_borrow(Updates::is_open)
    {
        OPEN.with_borrow_mut(|OpenUpdate { early, .. }| {
            if !early
                .iter()
                .any(|marked| marked.window == control.0.0 as isize)
            {
                early.push(Marked {
                    window: control.0.0 as isize,
                    rect: control.screen_rect(),
                });
            }
        });
    }
    match message {
        WM_ENTERMENULOOP | WM_ENTERSIZEMOVE => {
            finish(UPDATES.with_borrow_mut(Updates::native_loop_started));
        }
        WM_EXITMENULOOP | WM_EXITSIZEMOVE => {
            let ended = UPDATES
                .with_borrow_mut(|updates| updates.native_loop_ended(|| start(Cause::LoopEnded)));
            finish(ended);
        }
        DEFERRED => UPDATES.with_borrow_mut(Updates::deferred_arrived),
        PRESENT => DEFERRED_PAINT.set(DeferredPaint {
            present: Present::Arrived,
            ..DEFERRED_PAINT.get()
        }),
        WM_DESTROY if MAIN.get().is_some_and(|main| main.window == Hwnd(window)) => {
            MAIN.set(None);
            UPDATES.with_borrow_mut(Updates::forget);
        }
        _ => {}
    }
    Scope(Some(call))
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(call) = self.0.take() {
            finish(UPDATES.with_borrow_mut(|updates| updates.exit(call)));
        }
    }
}

/// The control that `message` shows to be painting now: a custom draw's
/// first stage, or a control asking its parent for colours.
///
/// # Safety
/// As [`enter`].
unsafe fn painting(message: u32, lparam: LPARAM) -> Option<Hwnd> {
    match message {
        WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC => Some(Hwnd(HWND(lparam.0 as *mut _))),
        WM_NOTIFY if lparam.0 != 0 => {
            // SAFETY: a custom draw's notification is an NMCUSTOMDRAW,
            // which starts with its NMHDR.
            let draw = unsafe { &*(lparam.0 as *const NMCUSTOMDRAW) };
            (draw.hdr.code == NM_CUSTOMDRAW && draw.dwDrawStage == CDDS_PREPAINT)
                .then_some(Hwnd(draw.hdr.hwndFrom))
        }
        _ => None,
    }
}

/// Whether `message` opens an update: all but the messages that Windows
/// sends while it paints, hit-tests or activates, which change nothing.
///
/// # Safety
/// As [`enter`].
unsafe fn opens_update(message: u32, lparam: LPARAM) -> bool {
    match message {
        WM_ERASEBKGND | WM_NCPAINT | WM_SYNCPAINT | WM_PRINTCLIENT | WM_NCHITTEST
        | WM_SETCURSOR | WM_GETMINMAXINFO | WM_NCCALCSIZE | WM_NCACTIVATE
        | WM_WINDOWPOSCHANGING => false,
        WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC => false,
        WM_NOTIFY if lparam.0 != 0 => {
            // SAFETY: WM_NOTIFY carries the live NMHDR that the caller
            // promised.
            let code = unsafe { (*(lparam.0 as *const NMHDR)).code };
            !matches!(
                code,
                NM_CUSTOMDRAW
                    | LVN_GETDISPINFOW
                    | LVN_ODCACHEHINT
                    | LVN_ODFINDITEMW
                    | LVN_GETEMPTYMARKUP
            )
        }
        _ => true,
    }
}

/// A modal loop of Gozler's, which runs while this lives: a dialog, a
/// message box, the file dialog, a popup menu or a drag. The update open
/// when it starts ends then, each message inside the loop is an update of
/// its own, and what the handler changes after the loop is a new update.
#[must_use = "retain the loop's level until the loop returns"]
pub(crate) struct ModalLoop {
    menu: bool,
}

pub(crate) fn modal_loop() -> ModalLoop {
    finish(UPDATES.with_borrow_mut(|updates| updates.own_loop_started(false)));
    ModalLoop { menu: false }
}

/// [`modal_loop`] for a popup menu, whose `WM_ENTERMENULOOP` then adds no
/// level of its own.
pub(crate) fn menu_loop() -> ModalLoop {
    finish(UPDATES.with_borrow_mut(|updates| updates.own_loop_started(true)));
    ModalLoop { menu: true }
}

impl Drop for ModalLoop {
    fn drop(&mut self) {
        finish(UPDATES.with_borrow_mut(|updates| {
            updates.own_loop_ended(self.menu, || start(Cause::LoopEnded))
        }));
    }
}

/// The current message posted `DEFERRED` ([`super::Hwnd::post_deferred`]),
/// so the open update goes on until that message ends.
/// Windows delivers a posted message before input and `WM_PAINT`, and the
/// messages posted before it arrive first and join the update too.
pub(super) fn extend_to_deferred() {
    UPDATES.with_borrow_mut(Updates::extend_to_deferred);
}

/// Runs `change`, a native setter that would paint `control` at once,
/// inside the message, with drawing off on that one control, then marks
/// its client and frame, which paint when the update ends. Turning drawing
/// off drops the marks the control's children already had, such as the
/// filter pane's controls that the pane's placement marked, so those are
/// marked again; the others stay unmarked, so a status text does not
/// repaint the controls inside the status bar. A top-level window runs
/// `change` as it is, because DWM draws its title, and so does a hidden
/// control, because `WM_SETREDRAW` TRUE would show it.
pub(crate) fn mark_after<T>(control: Hwnd, change: impl FnOnce() -> T) -> T {
    mark_rect_after(control, None, change)
}

/// [`mark_after`], marking only `rect` of `control`'s client when it is
/// given, for a setter that changes only that part, as a status bar's text
/// changes one part. The status bar erases before it draws, so a part that
/// did not change and still repainted could show blank for a frame.
pub(crate) fn mark_rect_after<T>(
    control: Hwnd,
    rect: Option<RECT>,
    change: impl FnOnce() -> T,
) -> T {
    // The hold's release repaints the control whole.
    if is_held(control) || !control.styles().0.contains(WS_CHILD | WS_VISIBLE) {
        return change();
    }
    let mut children = Vec::new();
    super::dialog::add_descendants(control, &mut children);
    let marked: Vec<(Hwnd, RECT)> = update_rects(children).collect();
    // Drawing off drops the control's own marks too, such as a part that an
    // earlier status text of the same update marked, so marking one part
    // marks them again. A region, because the bounding box of two parts can
    // be the whole bar.
    let kept = rect
        // SAFETY: plain values; the region is checked before use, and
        // deleted below.
        .map(|_| unsafe { CreateRectRgn(0, 0, 0, 0) })
        .filter(|region| !region.is_invalid());
    if let Some(region) = kept {
        // SAFETY: `region` is a valid region the call fills in.
        unsafe { GetUpdateRgn(control.0, region, false) };
    }
    let result = with_drawing_off(control, change);
    // SAFETY: plain values; each rectangle and region outlives the call that
    // reads it, and the kept region is this function's to delete.
    unsafe {
        if let Some(region) = kept {
            let _ = RedrawWindow(
                Some(control.0),
                None,
                Some(region),
                RDW_INVALIDATE | RDW_ERASE,
            );
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
        let _ = match &rect {
            Some(rect) => RedrawWindow(
                Some(control.0),
                Some(rect),
                None,
                RDW_INVALIDATE | RDW_ERASE,
            ),
            None => RedrawWindow(
                Some(control.0),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME,
            ),
        };
        for (child, rect) in &marked {
            let _ = RedrawWindow(Some(child.0), Some(rect), None, RDW_INVALIDATE | RDW_ERASE);
        }
    }
    result
}

/// Runs `change` with `window`'s drawing off, so nothing it changes paints
/// until the caller invalidates what should.
fn with_drawing_off<T>(window: Hwnd, change: impl FnOnce() -> T) -> T {
    set_drawing(window, false);
    let result = change();
    set_drawing(window, true);
    result
}

/// Turns `window`'s drawing off or on (`WM_SETREDRAW`). Off clears its
/// `WS_VISIBLE`, and on sets it again.
fn set_drawing(window: Hwnd, on: bool) {
    // SAFETY: plain values.
    unsafe { SendMessageW(window.0, WM_SETREDRAW, Some(WPARAM(usize::from(on))), None) };
}

/// Runs `change`, a native change after which `control`, a list or a tree,
/// draws itself at once, such as a column's width or a tree's item, with
/// the control's drawing off until the open update ends. Several such
/// changes in one update then draw the control once, when [`finish`]
/// releases it, or earlier when a placement or a scroll needs its drawing
/// on ([`release`]). Outside an update no `finish` would release the hold, and
/// a hidden control would show at `WM_SETREDRAW` TRUE, so both run `change`
/// as [`mark_after`] does.
pub(crate) fn hold_after<T>(control: Hwnd, repaint: Repaint, change: impl FnOnce() -> T) -> T {
    if is_held(control) {
        return change();
    }
    if !UPDATES.with_borrow(Updates::is_open) || !control.styles().0.contains(WS_CHILD | WS_VISIBLE)
    {
        return mark_after(control, change);
    }
    set_drawing(control, false);
    OPEN.with_borrow_mut(|open| open.held.push(Held { control, repaint }));
    change()
}

/// Whether [`hold_after`] has `window`'s drawing off.
pub(super) fn is_held(window: Hwnd) -> bool {
    OPEN.with_borrow(|open| open.held.iter().any(|held| held.control == window))
}

/// Ends the hold of `control`, if it has one, before code that needs its
/// drawing on, such as a placement or a scroll.
pub(super) fn release(control: Hwnd) {
    let held = OPEN.with_borrow_mut(|open| {
        let index = open.held.iter().position(|held| held.control == control)?;
        Some(open.held.swap_remove(index))
    });
    if let Some(held) = held {
        repaint(held);
    }
}

/// Turns the held control's drawing on and marks it whole, with its frame
/// and its children, whose marks the hold dropped. A column or a tree
/// change repaints every row anyway.
fn repaint(held: Held) {
    set_drawing(held.control, true);
    let erase = match held.repaint {
        Repaint::Buffered => REDRAW_WINDOW_FLAGS(0),
        Repaint::Erased => RDW_ERASE,
    };
    // SAFETY: plain values.
    let _ = unsafe {
        RedrawWindow(
            Some(held.control.0),
            None,
            None,
            RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN | erase,
        )
    };
}

/// The windows among `windows` with a part waiting to paint, each with the
/// bounds of that part in its own client coordinates.
fn update_rects(windows: impl IntoIterator<Item = Hwnd>) -> impl Iterator<Item = (Hwnd, RECT)> {
    windows.into_iter().filter_map(|window| {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a local the call fills in.
        unsafe { GetUpdateRect(window.0, Some(&raw mut rect), false) }
            .as_bool()
            .then_some((window, rect))
    })
}

/// Shows the cover now, while the open update still makes its changes, for
/// a change of `area` pixels in `window`, the main window or one of its
/// controls, that paints at once inside native code; the update's paint
/// pass then runs under the same cover. Drawing stays on in the main
/// window itself: `WM_SETREDRAW` FALSE clears a top-level window's
/// `WS_VISIBLE`, and a recorded run with it lost "always on top".
pub(crate) fn cover_now(window: Hwnd, area: i64) {
    let Some(main) = MAIN.get() else {
        return;
    };
    let wanted = OPEN.with_borrow(|open| open.raised.is_none())
        && is_shown_in_main(window)
        && main.window.is_on_screen()
        && UPDATES.with_borrow(Updates::is_open);
    if let Some(cover) = main.cover.filter(|_| wanted)
        && raise_cover(main.window, cover, area) == Cover::Shown
    {
        OPEN.with_borrow_mut(|open| open.raised = Some(cover));
    }
}

/// Whether `window` is shown and is the main window or inside it.
fn is_shown_in_main(window: Hwnd) -> bool {
    let main = MAIN
        .get()
        .filter(|_| window.styles().0.contains(WS_VISIBLE));
    // SAFETY: plain values.
    main.is_some_and(|main| unsafe { GetAncestor(window.0, GA_ROOT) } == main.window.0)
}

/// Invalidates before scrolling to prevent corrupt text during repeated
/// partial-page scrolls, even with native double buffering.
pub(crate) fn scroll_now<T>(list: Hwnd, scroll: impl FnOnce() -> T) -> T {
    release(list);
    // SAFETY: plain values.
    let _ = unsafe { InvalidateRect(Some(list.0), None, false) };
    let result = scroll();
    // SAFETY: plain values.
    let _ = unsafe { UpdateWindow(list.0) };
    result
}

/// The marks that [`finish`] left for a later update to paint.
#[derive(Clone, Copy)]
struct DeferredPaint {
    /// The tick of the oldest change that no paint pass has shown yet;
    /// `None` while nothing waits.
    since: Option<i64>,
    present: Present,
}

/// Where the [`PRESENT`] message that paints left marks is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Present {
    /// None is on its way.
    Idle,
    /// Posted, and not arrived yet.
    Posted,
    /// Arrived since the last update ended. It can arrive inside an update
    /// that another message opened.
    Arrived,
}

/// Whether a message of `flags` waits in the thread's queue.
fn queue_holds(flags: QUEUE_STATUS_FLAGS) -> bool {
    // SAFETY: plain values. The high word says what waits in the queue.
    (unsafe { GetQueueStatus(flags) } >> 16) & flags.0 != 0
}

/// Ends the update `ended`, if one ended: publishes what it marked in the
/// thread's windows on screen, and logs it.
fn finish(ended: Option<Start>) {
    let Some(start) = ended else {
        return;
    };
    let mut deferred = DEFERRED_PAINT.get();
    let presented = deferred.present == Present::Arrived;
    if presented {
        deferred.present = Present::Idle;
        DEFERRED_PAINT.set(deferred);
    }
    let OpenUpdate {
        early,
        raised,
        held,
    } = OPEN.take();
    let raised = Raised(raised);
    if !held.is_empty() {
        // A message that a control sends while its drawing comes on cannot
        // end an update, as in the paint pass.
        let _pass = Scope(Some(UPDATES.with_borrow_mut(Updates::pass)));
        held.into_iter().for_each(repaint);
    }
    let changed = ticks();
    // A window that needs painting is a waiting WM_PAINT.
    if !queue_holds(QS_PAINT) {
        deferred.since = None;
        DEFERRED_PAINT.set(deferred);
        return;
    }
    let windows = shown_windows();
    if windows.is_empty() {
        return;
    }
    let main = MAIN
        .get()
        .map(|main| main.window)
        .filter(|main| windows.contains(main));
    let mut marked = Vec::new();
    let mut in_main = 0..0;
    let mut painted = Vec::new();
    for window in windows {
        let first = marked.len();
        find_marked(window, &mut marked);
        if Some(window) == main {
            in_main = first..marked.len();
        }
        if marked.len() > first || (Some(window) == main && raised.0.is_some()) {
            painted.push(window);
        }
    }
    // The marks paint when nothing else waits, or once the oldest is a
    // frame old. Input and posted messages go before WM_PAINT, so marks
    // left to it waited for seconds behind a stream of search results;
    // under load, keys still go first and the marks of several updates
    // paint in one pass. A cover raised during the changes paints now.
    // Messages to a native control, such as the mouse over a list, end no
    // update, so leaving marks posts PRESENT, whose update paints them
    // before WM_PAINT could, after the messages that wait before it.
    let since = deferred.since.unwrap_or(start.ticks);
    let due = raised.0.is_some()
        || presented
        || !queue_holds(QUEUE_STATUS_FLAGS(QS_INPUT.0 | QS_POSTMESSAGE.0))
        || changed - since >= ticks_per_second() / 60;
    if !due
        && deferred.present == Present::Idle
        && MAIN.get().is_some_and(|main| main.window.post(PRESENT))
    {
        deferred.present = Present::Posted;
    }
    deferred.since = (!due).then_some(since);
    DEFERRED_PAINT.set(deferred);
    let area = marked[in_main.clone()]
        .iter()
        .map(|marked| i64::from(marked.rect.width.0) * i64::from(marked.rect.height.0))
        .sum();
    // One window paints in one step: a list is double-buffered, and the
    // other controls paint in well under a frame. The other windows, such
    // as the Viewer and the dialogs, have few controls each, and paint
    // without the cover.
    let covered = in_main.len() >= 2 || raised.0.is_some();
    let (paint, raise, redraw, published) = if due {
        let before = ticks();
        let (cover, raise, redraw) = publish(main, &painted, area, raised, covered);
        (ticks() - before, raise, redraw, Some(cover))
    } else {
        (0, 0, 0, None)
    };
    if marked.is_empty() {
        return;
    }
    let end = ticks();
    FRAMES.with_borrow_mut(|log| {
        let Some(log) = log else {
            return;
        };
        let frame = Frame {
            start,
            end,
            change: changed - start.ticks,
            paint,
            raise,
            redraw,
            cycles: gozler_winfs::thread_cycles().saturating_sub(start.cycles),
            marked,
            published,
            early,
            dropped: log.dropped,
        };
        match log.frames.try_send(frame) {
            Ok(()) => log.dropped = 0,
            Err(TrySendError::Full(_)) => log.dropped += 1,
            Err(TrySendError::Disconnected(_)) => {}
        }
    });
}

/// The top-level windows of this thread that are on screen, other than the
/// cover: the main window, the Viewers and the dialogs, and the windows
/// that Windows shows for the thread, such as a message box or a tooltip.
fn shown_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn add(window: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector that `shown_windows` passed, alive
        // while EnumThreadWindows runs.
        unsafe { (*(found.0 as *mut Vec<Hwnd>)).push(Hwnd(window)) };
        BOOL(1)
    }
    let mut windows = Vec::new();
    // SAFETY: the callback writes only to `windows`, which outlives the call.
    let _ = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(add),
            LPARAM(std::ptr::from_mut(&mut windows) as isize),
        )
    };
    let cover = MAIN.get().and_then(|main| main.cover);
    windows.retain(|&window| Some(window) != cover && window.is_on_screen());
    windows
}

/// Adds to `marked` `window`'s client and its descendants that are shown
/// and need painting, each with the rectangle to paint, in screen
/// coordinates.
fn find_marked(window: Hwnd, marked: &mut Vec<Marked>) {
    let mut windows = WINDOWS.take();
    windows.clear();
    windows.push(window);
    super::dialog::add_descendants(window, &mut windows);
    let visible = windows.iter().copied().filter(|window| window.is_visible());
    marked.extend(update_rects(visible).map(|(window, mut rect)| {
        // SAFETY: `rect` is a local that holds the two points
        // MapWindowPoints converts.
        unsafe {
            let points = std::slice::from_raw_parts_mut((&raw mut rect).cast::<POINT>(), 2);
            MapWindowPoints(Some(window.0), None, points);
        }
        Marked {
            window: window.0.0 as isize,
            rect: from_rect(rect),
        }
    }));
    WINDOWS.set(windows);
}

/// Paints the marked windows inside each of `windows` now, in one pass, the
/// main window's, `area` pixels in all, under the cover when `covered`. It
/// invalidates nothing, so a window that did not change does not paint, and
/// neither do the frame and the menu bar. Gives what the cover did, and the
/// ticks that raising it and the windows' paint took.
fn publish(
    main: Option<Hwnd>,
    windows: &[Hwnd],
    area: i64,
    mut raised: Raised,
    covered: bool,
) -> (Cover, i64, i64) {
    let _pass = Scope(Some(UPDATES.with_borrow_mut(Updates::pass)));
    let before = ticks();
    let cover = MAIN.get().and_then(|main| main.cover);
    let outcome = match (raised.0, main.zip(cover)) {
        (Some(_), _) => Cover::Early,
        (None, _) if !covered => Cover::Single,
        (None, Some((main, cover))) => {
            let outcome = raise_cover(main, cover, area);
            if outcome == Cover::Shown {
                raised.0 = Some(cover);
            }
            outcome
        }
        (None, None) => Cover::ShowFailed,
    };
    let raise = ticks() - before;
    let before = ticks();
    for window in windows {
        // SAFETY: plain values.
        let _ =
            unsafe { RedrawWindow(Some(window.0), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN) };
    }
    (outcome, raise, ticks() - before)
}

/// The cover while it shows over an update; dropping it takes the cover
/// away, so an update that ends without a paint pass leaves none behind.
struct Raised(Option<Hwnd>);

impl Drop for Raised {
    fn drop(&mut self) {
        if let Some(cover) = self.0.take() {
            lower_cover(cover);
        }
    }
}

/// Shows `cover` over `main`'s client, holding what the client shows now,
/// and says whether it shows. A pass that paints `area` pixels, less
/// than a quarter of the client, such as a list row and the status bar,
/// finishes well inside a frame, so it goes without the cover, which costs
/// a copy of the whole client. Each change of the update only marked, so
/// the window's surface still holds the old picture. The cover goes
/// directly above the main window, so a window above it, such as a dialog
/// or another program's window, stays above.
fn raise_cover(main: Hwnd, cover: Hwnd, area: i64) -> Cover {
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: `client` and `origin` are locals the calls fill in.
    let placed = unsafe {
        GetClientRect(main.0, &mut client).is_ok() && ClientToScreen(main.0, &mut origin).as_bool()
    };
    let (width, height) = (client.right, client.bottom);
    let Some(mut measured) = MAIN.get() else {
        return Cover::ShowFailed;
    };
    let covered_size = measured.covered_size.replace((width, height));
    MAIN.set(Some(measured));
    let small = area * 4 < i64::from(width) * i64::from(height);
    if !placed {
        return Cover::ShowFailed;
    }
    // After a resize, the part of the surface that the window gained holds
    // nothing yet, and a copy would show it so.
    match covered_size {
        None => return Cover::First,
        Some(size) if size != (width, height) => return Cover::Resized,
        Some(_) => {}
    }
    if small || width <= 0 || height <= 0 {
        return Cover::Small;
    }
    let Some(copy) = copy_client(main, width, height) else {
        return Cover::CopyFailed;
    };
    // The control keeps a bitmap without alpha as it is, and gives it back
    // when the next image replaces it.
    if let Some(previous) = send_image(cover, Some(copy)) {
        delete_bitmap(previous);
    }
    shape_cover(main, cover, origin, width, height);
    // SAFETY: plain values.
    let above = unsafe { GetWindow(main.0, GW_HWNDPREV) }.ok();
    // The cover takes the main window's band, topmost or not: a cover in
    // the other band would pull its owner, the main window, out of its own
    // band, such as out of always on top.
    let topmost = |window: HWND| Hwnd(window).styles().1.contains(WS_EX_TOPMOST);
    let main_topmost = topmost(main.0);
    let (after, order) = match above {
        Some(above) if above == cover.0 => (None, SWP_NOZORDER),
        Some(above) if topmost(above) == main_topmost => (Some(above), SET_WINDOW_POS_FLAGS(0)),
        // The main window is the top window of its band.
        _ if main_topmost => (Some(HWND_TOPMOST), SET_WINDOW_POS_FLAGS(0)),
        _ => (Some(HWND_NOTOPMOST), SET_WINDOW_POS_FLAGS(0)),
    };
    // SAFETY: plain values.
    let shown = unsafe {
        SetWindowPos(
            cover.0,
            after,
            origin.x,
            origin.y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | order,
        )
        .is_ok()
    };
    if !shown {
        return Cover::ShowFailed;
    }
    // The cover paints while still cloaked, so it shows whole.
    // SAFETY: plain values.
    unsafe {
        let _ = UpdateWindow(cover.0);
        let _ = GdiFlush();
    }
    if set_flag(cover, DWMWA_CLOAK, false).is_none() {
        hide_cover(cover);
        return Cover::ShowFailed;
    }
    Cover::Shown
}

/// Takes `cover` away from the window's finished surface. No `DwmFlush`
/// waits for DWM first: the changes lie under the opaque cover, so DWM
/// has nothing to show, and a flush waits until something else on the
/// screen changes, measured at up to most of a second.
fn lower_cover(cover: Hwnd) {
    // SAFETY: no parameters.
    let _ = unsafe { GdiFlush() };
    let _ = set_flag(cover, DWMWA_CLOAK, true);
    hide_cover(cover);
}

/// Hides `cover` and lets go of its copy.
fn hide_cover(cover: Hwnd) {
    // SAFETY: plain values. Under DWM the windows below keep their pixels,
    // so nothing below needs painting.
    let _ = unsafe {
        SetWindowPos(
            cover.0,
            None,
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
        )
    };
    if let Some(shown) = send_image(cover, None) {
        delete_bitmap(shown);
    }
}

/// Gives `cover` the bitmap `image`, or none, and returns the one it showed.
fn send_image(cover: Hwnd, image: Option<HBITMAP>) -> Option<HBITMAP> {
    let image = image.map_or(0, |image| image.0 as isize);
    // SAFETY: plain values; the control only shows the bitmap.
    let previous = unsafe {
        SendMessageW(
            cover.0,
            STM_SETIMAGE,
            Some(WPARAM(IMAGE_BITMAP.0 as usize)),
            Some(LPARAM(image)),
        )
    };
    (previous.0 != 0).then_some(HBITMAP(previous.0 as *mut _))
}

fn delete_bitmap(bitmap: HBITMAP) {
    // SAFETY: the bitmap is this module's, and no DC holds it.
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
}

/// Gives `cover` the shape of `main`'s client, `width` by `height` at
/// `origin` on the screen, less each scroll bar shown in it, out to its
/// control's outer edge. Windows 11 draws a scroll bar's track above the
/// window's surface, so the copy has none, and a cover with the copy's
/// scroll bars made each track flash and the list's border show; the real
/// scroll bars show through instead. The bottom corners are cut as round
/// as Windows 11 rounds the main window's, so the cover's square corners
/// do not show at both ends of the status bar.
fn shape_cover(main: Hwnd, cover: Hwnd, origin: POINT, width: i32, height: i32) {
    let mut windows = Vec::new();
    super::dialog::add_descendants(main, &mut windows);
    // SAFETY: each region is checked before use; `info` and `outer` are
    // locals the calls fill in; SetWindowRgn takes the shape, and frees it
    // and the shape before it.
    unsafe {
        // Windows 11 rounds a window's corners by 8 pixels at 96 DPI; the
        // round rectangle's top corners lie above the cover.
        let corner = 2 * Px(8)
            .rescale(Dpi::new(96).expect("96 is not zero"), main.dpi())
            .0;
        let shape = CreateRoundRectRgn(0, -corner, width + 1, height + 1, corner, corner);
        if shape.is_invalid() {
            return;
        }
        for window in windows.into_iter().filter(|window| window.is_visible()) {
            let style = window.styles().0;
            for (bar, id) in [(WS_VSCROLL, OBJID_VSCROLL), (WS_HSCROLL, OBJID_HSCROLL)] {
                let mut info = SCROLLBARINFO {
                    cbSize: size_of::<SCROLLBARINFO>() as u32,
                    ..SCROLLBARINFO::default()
                };
                let mut outer = RECT::default();
                if !style.contains(bar)
                    || GetScrollBarInfo(window.0, id, &mut info).is_err()
                    || info.rgstate[0] & STATE_SYSTEM_INVISIBLE.0 != 0
                    || GetWindowRect(window.0, &mut outer).is_err()
                {
                    continue;
                }
                let bar = info.rcScrollBar;
                let (right, bottom) = if id == OBJID_VSCROLL {
                    (outer.right, bar.bottom)
                } else {
                    (bar.right, outer.bottom)
                };
                let hole = CreateRectRgn(
                    bar.left - origin.x,
                    bar.top - origin.y,
                    right - origin.x,
                    bottom - origin.y,
                );
                if !hole.is_invalid() {
                    CombineRgn(Some(shape), Some(shape), Some(hole), RGN_DIFF);
                    let _ = DeleteObject(HGDIOBJ(hole.0));
                }
            }
        }
        SetWindowRgn(cover.0, Some(shape), false);
    }
}

/// A bitmap of what `main`'s client, `width` by `height`, shows now. The
/// controls are in it too, because they paint into the main window's
/// surface. It has no alpha channel: GDI leaves the alpha of the text it
/// draws at zero, and the static control would show such pixels as
/// transparent, so every text would be missing from the cover.
fn copy_client(main: Hwnd, width: i32, height: i32) -> Option<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    // SAFETY: each handle is checked before use and released here; the
    // bitmap leaves the memory DC before the DC is deleted.
    unsafe {
        let window = GetDC(Some(main.0));
        if window.is_invalid() {
            return None;
        }
        let memory = CreateCompatibleDC(Some(window));
        let mut bits = std::ptr::null_mut();
        let copy = CreateDIBSection(Some(window), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        let copied = !memory.is_invalid() && !copy.is_invalid() && {
            let previous = SelectObject(memory, HGDIOBJ(copy.0));
            let copied = BitBlt(memory, 0, 0, width, height, Some(window), 0, 0, SRCCOPY).is_ok();
            SelectObject(memory, previous);
            copied
        };
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        ReleaseDC(Some(main.0), window);
        if copied {
            Some(copy)
        } else {
            if !copy.is_invalid() {
                delete_bitmap(copy);
            }
            None
        }
    }
}
