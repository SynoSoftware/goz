//! The Viewer's Win32 parts (file-manager-spec.md: The Viewer): the area
//! that shows the file's rows, and the thread that reads the file. The
//! thread reads the blocks the window asks for, one at a time, and the
//! window never joins or waits for it, because a network share that stopped
//! answering can hold a read for a minute: closing the Viewer ends the
//! thread with its stop signal and `CancelSynchronousIo`, as a panel's
//! reading is ended ([`super::folder`]), and the thread wakes the window
//! only while the window is open.
//!
//! The file's handle is registered for the removal of its drive, so Windows
//! asks before Safely Remove takes the drive, and the window then closes
//! the handle at once ([`FileReader::release`]).

use super::Hwnd;
use super::folder::{Stop, reason, register, unregister, verbatim_wide};
use super::gdi::{Font, muted_text, system_color};
use super::wake::OpenWaker;
use super::window::offer_key;
use crate::core::layout::{Dpi, Px};
use crate::core::viewer_window::{BLOCK_BYTES, ScrollBar};
use gozler_winfs::{Access, Entry, Reparse, WinError, open_entry};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, COLOR_WINDOWTEXT, ETO_CLIPPED, ETO_OPAQUE, EndPaint, ExtTextOutW,
    FillRect, GetSysColorBrush, HBRUSH, HGDIOBJ, PAINTSTRUCT, SelectObject, SetBkColor,
    SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DLGC_WANTARROWS, DLGC_WANTCHARS, DefWindowProcW, GetParent, HMENU, IDC_ARROW,
    LoadCursorW, RegisterClassExW, SB_VERT, SCROLLINFO, SIF_DISABLENOSCROLL, SIF_PAGE, SIF_POS,
    SIF_RANGE, SM_CYCAPTION, SendMessageW, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETDLGCODE,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_PAINT, WM_VSCROLL, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_LEFT, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

const AREA_CLASS: PCWSTR = w!("gozler.ui.viewer_area");

/// The private message with which the area asks its Viewer to paint it;
/// the Viewer answers 1 when it painted.
pub(crate) const AREA_PAINT: u32 = super::wake::WAKE + 9;

/// The message that ends a move or a resize of the Viewer.
pub(crate) const MOVE_ENDED: u32 = WM_EXITSIZEMOVE;

/// Registers the area's window class; once, before the first Viewer opens.
pub(crate) fn register_area() -> windows::core::Result<()> {
    // SAFETY: a null name asks for the executable's own module.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(area_procedure),
        hInstance: instance.into(),
        // SAFETY: IDC_ARROW is a predefined cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        // The documented way to name a system colour as a class brush.
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
        lpszClassName: AREA_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is complete.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Creates the area in `parent`. `Tab` reaches it, and it keeps the arrow
/// keys, which `IsDialogMessageW` would otherwise take. It clips the
/// controls created before it, which lie above it, such as the message.
pub(crate) fn create_area(parent: Hwnd, id: u16) -> windows::core::Result<Hwnd> {
    // SAFETY: a child window with an ID in place of a menu, as Windows
    // documents it.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LEFT,
            AREA_CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP | WS_CLIPSIBLINGS,
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

/// Has the Viewer paint the area, and passes the scroll bar's requests to
/// it with the area as their sender, as a scroll bar control sends them. A
/// click gives the area the focus.
unsafe extern "system" fn area_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: plain values; an area without a parent gives an error.
    let parent = || unsafe { GetParent(hwnd) }.ok();
    let to_parent = |msg: u32, wparam: WPARAM| -> Option<LRESULT> {
        let parent = parent()?;
        // SAFETY: the parameters are the ones Windows passed, or plain
        // values, with the area's own handle, which the Viewer decodes as
        // its own controls'.
        Some(unsafe { SendMessageW(parent, msg, Some(wparam), Some(LPARAM(hwnd.0 as isize))) })
    };
    match msg {
        WM_PAINT if to_parent(AREA_PAINT, WPARAM(0)).is_some_and(|painted| painted.0 != 0) => {
            LRESULT(0)
        }
        // The Viewer paints every pixel, so erasing first would flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_VSCROLL => to_parent(msg, wparam).unwrap_or_default(),
        // A modal dialog's message loop dispatches the area's keys without
        // offering them to the Viewer, as `run_message_loop` does first.
        WM_KEYDOWN if parent().is_some_and(|parent| offer_key(parent, hwnd, wparam, lparam)) => {
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS) as isize),
        WM_LBUTTONDOWN => {
            // SAFETY: plain values.
            let _ = unsafe { SetFocus(Some(hwnd)) };
            LRESULT(0)
        }
        // SAFETY: the parameters are the ones Windows passed.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The rows the area shows, and how to draw them.
pub(crate) struct AreaText<'a> {
    pub(crate) font: &'a Font,
    /// Each row's text, with how many of its first characters show in the
    /// muted text colour.
    pub(crate) rows: &'a [(&'a str, usize)],
    pub(crate) line_height: Px,
    pub(crate) char_width: Px,
    /// The space before the text and above its first row.
    pub(crate) inset: (Px, Px),
}

/// Paints the area for its `WM_PAINT`: `text`'s rows from the top, and the
/// window colour around them.
pub(crate) fn paint_area(area: Hwnd, text: &AreaText) {
    let size = area.client_size();
    let (x, y) = (text.inset.0.0, text.inset.1.0);
    let height = text.line_height.0;
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: BeginPaint and EndPaint pair on the same window and
    // structure, and the font is selected back out before EndPaint; each
    // string outlives its call, and system brushes are never freed.
    unsafe {
        let dc = BeginPaint(area.0, &mut paint);
        let previous = SelectObject(dc, HGDIOBJ(text.font.handle().0));
        let window = GetSysColorBrush(COLOR_WINDOW);
        SetBkColor(dc, system_color(COLOR_WINDOW));
        let fill = |top: i32, bottom: i32| {
            if top < bottom {
                let rect = RECT {
                    left: 0,
                    top,
                    right: size.width.0,
                    bottom,
                };
                FillRect(dc, &rect, window);
            }
        };
        fill(0, y);
        let mut top = y;
        for (row, muted) in text.rows {
            if top >= size.height.0 {
                break;
            }
            let line = RECT {
                left: 0,
                top,
                right: size.width.0,
                bottom: top + height,
            };
            let units: Vec<u16> = row.encode_utf16().collect();
            let split = row.char_indices().nth(*muted).map_or(row.len(), |(i, _)| i);
            let muted_units = row[..split].encode_utf16().count();
            // The muted part, and the row's ground over the whole line.
            SetTextColor(dc, system_color(muted_text()));
            let _ = ExtTextOutW(
                dc,
                x,
                top,
                ETO_OPAQUE | ETO_CLIPPED,
                Some(&raw const line),
                PCWSTR(units.as_ptr()),
                muted_units as u32,
                None,
            );
            SetTextColor(dc, system_color(COLOR_WINDOWTEXT));
            let rest = &units[muted_units..];
            let _ = ExtTextOutW(
                dc,
                x + text.char_width.0 * *muted as i32,
                top,
                ETO_CLIPPED,
                Some(&raw const line),
                PCWSTR(rest.as_ptr()),
                rest.len() as u32,
                None,
            );
            top += height;
        }
        fill(top, size.height.0);
        SelectObject(dc, previous);
        let _ = EndPaint(area.0, &paint);
    }
}

/// The height of a window's caption at `dpi`, by which a Viewer opens
/// below and to the right of the one opened before it.
pub(crate) fn caption_height(dpi: Dpi) -> Px {
    // SAFETY: plain values.
    Px(unsafe { GetSystemMetricsForDpi(SM_CYCAPTION, dpi.get()) })
}

/// Sets the area's scroll bar to `bar`. Its page holds the whole range
/// while the whole file shows, which disables it, as Notepad's is, instead
/// of hiding it, so the text's width never changes with it.
pub(crate) fn set_area_scroll(area: Hwnd, bar: ScrollBar) {
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: bar.max,
        nPage: bar.page,
        nPos: bar.position,
        nTrackPos: 0,
    };
    // SAFETY: `info` is complete and outlives the call.
    unsafe { SetScrollInfo(area.0, SB_VERT, &info, true) };
}

/// What the reading thread found.
pub(crate) enum ViewerEvent {
    /// The file opened: its length, and its first block.
    Opened { len: u64, first: Vec<u8> },
    /// The block `index`, fewer bytes than asked when the file shrank.
    Block { index: u64, bytes: Vec<u8> },
    /// Windows could not open the file, or, when `opened`, read it; the
    /// thread has ended.
    Failed { opened: bool, reason: String },
}

/// What the window and the reading thread share: the open file, and its
/// registration for the drive's removal.
#[derive(Default)]
struct Held {
    file: Option<Arc<Entry>>,
    notification: Option<isize>,
}

/// One opening of a file, and the thread that reads it.
pub(crate) struct FileReader {
    stop: Arc<Stop>,
    held: Arc<Mutex<Held>>,
    /// The blocks the window asks for; `None` once released, which ends
    /// the thread's wait for the next.
    requests: Option<Sender<u64>>,
    events: Receiver<ViewerEvent>,
}

impl FileReader {
    /// Opens the file at `path` on a thread of its own, which then reads
    /// the blocks that [`FileReader::read`] asks for, and wakes `window`
    /// with what it found.
    pub(crate) fn open(path: &[u16], window: Hwnd, waker: OpenWaker) -> FileReader {
        let stop = Arc::new(Stop::new());
        let held = Arc::new(Mutex::new(Held::default()));
        let (requests, asked) = channel::<u64>();
        let (sender, events) = channel();
        let path = verbatim_wide(path);
        let window = window.0.0 as isize;
        let reader = FileReader {
            stop: Arc::clone(&stop),
            held: Arc::clone(&held),
            requests: Some(requests),
            events,
        };
        std::thread::spawn(move || {
            if stop.enter() {
                return;
            }
            let deliver = |event: ViewerEvent| {
                waker.deliver(|| !stop.stopped() && sender.send(event).is_ok());
            };
            let failed = |opened: bool, error: &WinError| {
                deliver(ViewerEvent::Failed {
                    opened,
                    reason: reason(error),
                });
            };
            let opened = open_entry(&path, Access::Read, Reparse::Follow)
                .and_then(|file| file.stat().map(|stat| (stat.size, file)));
            let (len, file) = match opened {
                Ok((len, file)) => (len, Arc::new(file)),
                Err(error) => return failed(false, &error),
            };
            {
                let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
                if stop.stopped() {
                    return;
                }
                held.file = Some(Arc::clone(&file));
            }
            // Outside the lock, because Windows may hold the registration
            // while it asks the window about a removal, and the window takes
            // the lock to answer.
            let notification = register(window, file.handle_value());
            {
                let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
                if stop.stopped() {
                    // The window ended the reading meanwhile and found no
                    // registration to end.
                    unregister(notification);
                    return;
                }
                held.notification = notification;
            }
            // Only the window's copy stays open between reads, so a release
            // closes the file at once.
            drop(file);
            // The block the window asked for; `None` for the opening's read
            // of block 0, the only read that is `Opened`. A later read of
            // block 0, after the window dropped it, is a `Block`, because
            // `Opened` starts the page anew.
            let mut asked_for = None;
            loop {
                let file = held
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .file
                    .clone();
                let Some(file) = file else {
                    return;
                };
                let bytes = match read_block(&file, asked_for.unwrap_or(0), len) {
                    Ok(bytes) => bytes,
                    Err(error) => return failed(true, &error),
                };
                drop(file);
                deliver(match asked_for {
                    None => ViewerEvent::Opened { len, first: bytes },
                    Some(index) => ViewerEvent::Block { index, bytes },
                });
                let Some(index) = asked.recv().ok().filter(|_| !stop.stopped()) else {
                    return;
                };
                asked_for = Some(index);
            }
        });
        reader
    }

    /// Asks the thread for the block `index`.
    pub(crate) fn read(&self, index: u64) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(index);
        }
    }

    /// Everything the thread found, without waiting.
    pub(crate) fn drain(&self) -> Vec<ViewerEvent> {
        self.events.try_iter().collect()
    }

    /// Whether Windows names this reading's file with the registration
    /// `notification`.
    pub(crate) fn is_registration(&self, notification: isize) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .notification
            == Some(notification)
    }

    /// Closes the file at once, for a removal of its drive that Windows
    /// asks about, and ends the thread; a read that runs is cancelled, and
    /// its copy of the file closes as the read ends. The registration stays,
    /// so the window hears whether the drive went.
    pub(crate) fn release(&mut self) {
        self.stop.stop();
        self.requests = None;
        let file = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .file
            .take();
        drop(file);
    }
}

impl Drop for FileReader {
    /// Ends the thread and the registration. The file closes on a thread
    /// of its own, because closing a file on a network share that stopped
    /// answering waits for the share.
    fn drop(&mut self) {
        self.stop.stop();
        let (file, notification) = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            (held.file.take(), held.notification.take())
        };
        unregister(notification);
        if let Some(file) = file {
            std::thread::spawn(move || drop(file));
        }
    }
}

/// The block `index` of a file of `len` bytes: [`BLOCK_BYTES`] bytes, the
/// last block fewer, and fewer still when the file shrank since it opened.
fn read_block(file: &Entry, index: u64, len: u64) -> Result<Vec<u8>, WinError> {
    let start = index.saturating_mul(BLOCK_BYTES as u64);
    let want = len.saturating_sub(start).min(BLOCK_BYTES as u64) as usize;
    let mut bytes = vec![0; want];
    let mut filled = 0;
    while filled < want {
        match file.read_at(start + filled as u64, &mut bytes[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    bytes.truncate(filled);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows::Win32::Foundation::HWND;

    /// `Opened` starts the page anew, so a second read of block 0, after
    /// the page dropped it, must come back as a `Block`: as `Opened` it
    /// lost the page's place and left the block awaited for ever.
    #[test]
    fn only_the_opening_reads_block_0_as_opened() {
        let path = std::env::temp_dir().join(format!("gozler-viewer-{}.bin", std::process::id()));
        std::fs::write(&path, vec![b'a'; BLOCK_BYTES + 1]).expect("the temp folder is writable");
        let wide: Vec<u16> = path
            .to_str()
            .expect("the temp path is Unicode")
            .encode_utf16()
            .collect();
        // No window: the registration for the drive's removal fails, and
        // the wake goes to the reading thread's own queue.
        let window = Hwnd(HWND::default());
        let reader = FileReader::open(&wide, window, OpenWaker::new(window));
        let next = || {
            reader
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("the thread answers each read")
        };
        assert!(matches!(next(), ViewerEvent::Opened { len, .. } if len == BLOCK_BYTES as u64 + 1));
        for index in [1, 0] {
            reader.read(index);
            assert!(matches!(next(), ViewerEvent::Block { index: got, .. } if got == index));
        }
        drop(reader);
        let _ = std::fs::remove_file(&path);
    }
}
