//! The clipboard: one `GMEM_MOVEABLE` block per copy, which
//! `SetClipboardData` requires and which Windows owns once it is set
//! (DESIGN.md: Copy), and the files that Paste reads from it.

use super::Hwnd;
use crate::core::clipboard::{ClipboardData, ClipboardFormat, Publication};
use gozler_client::CancelToken;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{E_OUTOFMEMORY, GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
};
use windows::core::w;

/// The message a window that [`listen`]s gets after the clipboard changed.
pub(crate) const CLIPBOARD_CHANGED: u32 = WM_CLIPBOARDUPDATE;

/// Has `window` get [`CLIPBOARD_CHANGED`] after each change of the
/// clipboard, until it is destroyed; without it, Paste is never enabled.
pub(crate) fn listen(window: Hwnd) {
    // SAFETY: plain values; Windows ends the listening with the window.
    let _ = unsafe { AddClipboardFormatListener(window.0) };
}

/// `CF_HDROP` and `CF_UNICODETEXT`, which the windows crate has only in a
/// feature for OLE.
const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;

/// `DROPEFFECT_COPY` and `DROPEFFECT_MOVE`, as the effect formats hold
/// them.
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// The format a file list's source names its effect in: copy for Copy,
/// move for Cut.
fn preferred_drop_effect() -> u32 {
    // SAFETY: a constant name; the call returns the format's number.
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

/// The format a program that pasted files tells their source in that it
/// moved them, so File Explorer clears its Cut.
fn paste_succeeded() -> u32 {
    // SAFETY: as above.
    unsafe { RegisterClipboardFormatW(w!("Paste Succeeded")) }
}

/// A `GMEM_MOVEABLE` block holding `value`, as the effect formats need.
fn effect_block(value: u32) -> Option<HGLOBAL> {
    // SAFETY: a new block of 4 bytes, locked only to write them.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, 4).ok()?;
        let bytes = GlobalLock(memory).cast::<u32>();
        if bytes.is_null() {
            let _ = GlobalFree(Some(memory));
            return None;
        }
        bytes.write_unaligned(value);
        let _ = GlobalUnlock(memory);
        Some(memory)
    }
}

/// One format's bytes in a block of their own size. Dropping a block that
/// the clipboard did not take frees it.
pub(crate) struct ClipboardBlock {
    memory: Memory,
    format: ClipboardFormat,
    items: usize,
}

struct Memory(HGLOBAL);

// SAFETY: this owns unlocked GMEM_MOVEABLE storage exclusively. No pointer into
// it survives construction, and transfer to Windows consumes the owning value.
unsafe impl Send for Memory {}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: only memory that was not transferred to Windows reaches Drop.
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}

pub(crate) enum CopyAttempt {
    Published,
    Busy(ClipboardBlock),
    Superseded,
    Failed(windows::core::Error),
}

struct Owner(Hwnd);

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: only a successful OpenClipboard constructs this thread-local guard.
        let _ = unsafe { CloseClipboard() };
    }
}

pub(crate) fn sequence() -> u32 {
    // SAFETY: this reads the version without asking its owner to render anything.
    unsafe { GetClipboardSequenceNumber() }
}

impl ClipboardBlock {
    /// A block holding `data`, or `None` when Windows has no memory for it.
    /// It makes no call that sends a message, so the model may be borrowed
    /// while `data` writes into it (DESIGN.md: window rule 1).
    pub(crate) fn new(data: &ClipboardData) -> Option<ClipboardBlock> {
        let len = data.len();
        // SAFETY: plain values; the block is this value's until the
        // clipboard takes it.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, len) }.ok()?;
        let block = ClipboardBlock {
            memory: Memory(memory),
            format: data.format(),
            items: data.items(),
        };
        // SAFETY: a locked moveable block of at least `len` bytes stays put
        // until it is unlocked, and nothing else holds it.
        unsafe {
            let bytes = GlobalLock(memory).cast::<u8>();
            if bytes.is_null() {
                return None;
            }
            data.write_into(std::slice::from_raw_parts_mut(bytes, len));
            let _ = GlobalUnlock(memory);
        }
        Some(block)
    }

    pub(crate) fn items(&self) -> usize {
        self.items
    }

    /// One attempt on its worker's native owner. Cancellation is checked before
    /// EmptyClipboard; afterward every format and CloseClipboard must finish.
    pub(crate) fn set(
        self,
        publication: &Publication,
        preceding: &AtomicU32,
        cancel: &CancelToken,
        _worker: &super::work::OffThread,
    ) -> CopyAttempt {
        if cancel.is_cancelled() {
            return CopyAttempt::Superseded;
        }
        let (format, effect) = match self.format {
            ClipboardFormat::Files => (CF_HDROP, Some(DROPEFFECT_COPY)),
            ClipboardFormat::CutFiles => (CF_HDROP, Some(DROPEFFECT_MOVE)),
            ClipboardFormat::Paths | ClipboardFormat::Names | ClipboardFormat::Folders => {
                (CF_UNICODETEXT, None)
            }
        };
        let effect = match effect {
            Some(value) => match effect_block(value) {
                Some(memory) => Some(Memory(memory)),
                None => {
                    return CopyAttempt::Failed(windows::core::Error::from_hresult(E_OUTOFMEMORY));
                }
            },
            None => None,
        };
        // SAFETY: a built-in, invisible message-only window owned by this worker.
        // Eager formats need no rendering messages after this owner is destroyed.
        let owner = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        } {
            Ok(owner) => Owner(Hwnd(owner)),
            Err(error) => return CopyAttempt::Failed(error),
        };
        // SAFETY: the owner was created on this thread and outlives the open guard.
        if unsafe { OpenClipboard(Some(owner.0.0)) }.is_err() {
            return CopyAttempt::Busy(self);
        }
        let _open = Open;
        if cancel.is_cancelled()
            || !publication.is_current(sequence(), preceding.load(Ordering::Acquire))
        {
            return CopyAttempt::Superseded;
        }
        // SAFETY: this thread holds the clipboard open. The guards close it and
        // destroy its owner in that order on every return path.
        if let Err(error) = unsafe { EmptyClipboard() } {
            return CopyAttempt::Failed(error);
        }
        let result = (|| {
            // SAFETY: both blocks are unlocked and owned here. Only a successful
            // SetClipboardData consumes their ownership; errors leave Drop to free them.
            unsafe {
                SetClipboardData(format, Some(HANDLE(self.memory.0.0)))?;
                std::mem::forget(self);
                if let Some(effect) = effect {
                    SetClipboardData(preferred_drop_effect(), Some(HANDLE(effect.0.0)))?;
                    std::mem::forget(effect);
                }
            }
            Ok(())
        })();
        // Record partial mutations too, before Close permits another publisher.
        preceding.store(sequence(), Ordering::Release);
        match result {
            Ok(()) => CopyAttempt::Published,
            Err(error) => CopyAttempt::Failed(error),
        }
    }
}

/// Whether the clipboard holds files, which Paste copies or moves.
pub(crate) fn holds_files() -> bool {
    // SAFETY: plain values.
    unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_ok()
}

mod work;
pub(crate) use work::{Completion, CompletionSender, Read};
pub(super) use work::{files_of, set_effect};

/// Files read for one Paste; its source stays alive until feedback or release.
pub(crate) struct ClipboardFiles {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) cut: bool,
    source: work::Source,
}

impl ClipboardFiles {
    pub(crate) fn take_paths(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.paths)
    }

    pub(crate) fn release_source(&mut self) {
        self.source.release();
    }

    /// Reports a completed cut without telling the source to delete its files.
    pub(crate) fn moved(self) {
        self.source.moved();
    }
}

/// The format a drop target tells the files' source in what it did with
/// them, which a source that sees move takes as an order to delete them.
pub(super) fn performed_drop_effect() -> u32 {
    // SAFETY: a constant name; the call returns the format's number.
    unsafe { RegisterClipboardFormatW(w!("Performed DropEffect")) }
}
