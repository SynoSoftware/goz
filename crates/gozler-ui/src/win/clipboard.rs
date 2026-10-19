//! The clipboard: one `GMEM_MOVEABLE` block per copy, which
//! `SetClipboardData` requires and which Windows owns once it is set
//! (docs/plans/ui-design.md: Copy).

use super::Hwnd;
use crate::core::clipboard::{ClipboardData, ClipboardFormat};
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// `CF_HDROP` and `CF_UNICODETEXT`, which the windows crate has only in a
/// feature for OLE.
const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;

/// How often a copy asks for the clipboard that another program holds, and
/// how long it waits between, so it gives up after 100 ms (design: Copy).
const OPEN_TRIES: u32 = 5;
const OPEN_WAIT: Duration = Duration::from_millis(20);

/// One format's bytes in a block of their own size. Dropping a block that
/// the clipboard did not take frees it.
pub(crate) struct ClipboardBlock {
    memory: HGLOBAL,
    format: ClipboardFormat,
}

/// Another program holds the clipboard.
pub(crate) struct ClipboardBusy;

impl ClipboardBlock {
    /// A block holding `data`, or `None` when Windows has no memory for it.
    /// It makes no call that sends a message, so the model may be borrowed
    /// while `data` writes into it (design rule 1).
    pub(crate) fn new(data: &ClipboardData) -> Option<ClipboardBlock> {
        let len = data.len();
        // SAFETY: plain values; the block is this value's until the
        // clipboard takes it.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, len) }.ok()?;
        let block = ClipboardBlock {
            memory,
            format: data.format(),
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

    /// Puts the block on the clipboard, as `owner`'s, in place of what the
    /// clipboard held. It sends `WM_DESTROYCLIPBOARD` to the clipboard's
    /// last owner, which can be this window, so no model borrow is held.
    pub(crate) fn set(self, owner: Hwnd) -> Result<(), ClipboardBusy> {
        let mut tries = 0;
        // SAFETY: plain values.
        while unsafe { OpenClipboard(Some(owner.0)) }.is_err() {
            tries += 1;
            if tries == OPEN_TRIES {
                return Err(ClipboardBusy);
            }
            std::thread::sleep(OPEN_WAIT);
        }
        let format = match self.format {
            ClipboardFormat::Files => CF_HDROP,
            ClipboardFormat::Paths | ClipboardFormat::Names => CF_UNICODETEXT,
        };
        // SAFETY: the clipboard is open on this thread. Once SetClipboardData
        // succeeds the clipboard owns the block, so it is not freed here.
        unsafe {
            let _ = EmptyClipboard();
            if SetClipboardData(format, Some(HANDLE(self.memory.0))).is_ok() {
                std::mem::forget(self);
            }
            let _ = CloseClipboard();
        }
        Ok(())
    }
}

impl Drop for ClipboardBlock {
    fn drop(&mut self) {
        // SAFETY: the block is this value's, because the clipboard did not
        // take it.
        let _ = unsafe { GlobalFree(Some(self.memory)) };
    }
}
