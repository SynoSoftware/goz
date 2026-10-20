//! The clipboard: one `GMEM_MOVEABLE` block per copy, which
//! `SetClipboardData` requires and which Windows owns once it is set
//! (DESIGN.md: Copy), and the files that Paste reads from it
//! (file-manager-spec.md: Search mode).

use super::Hwnd;
use crate::core::clipboard::{ClipboardData, ClipboardFormat};
use std::ffi::OsString;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE;
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

/// How often a copy asks for the clipboard that another program holds, and
/// how long it waits between, so it gives up after 100 ms (DESIGN.md: Copy).
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
    /// while `data` writes into it (DESIGN.md: window rule 1).
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
        let (format, effect) = match self.format {
            ClipboardFormat::Files => (CF_HDROP, Some(DROPEFFECT_COPY)),
            ClipboardFormat::CutFiles => (CF_HDROP, Some(DROPEFFECT_MOVE)),
            ClipboardFormat::Paths | ClipboardFormat::Names | ClipboardFormat::Folders => {
                (CF_UNICODETEXT, None)
            }
        };
        // SAFETY: the clipboard is open on this thread. Once SetClipboardData
        // succeeds the clipboard owns the block, so it is not freed here.
        unsafe {
            let _ = EmptyClipboard();
            if SetClipboardData(format, Some(HANDLE(self.memory.0))).is_ok() {
                std::mem::forget(self);
                // Without the effect, a program pasting the files copies
                // them, so a Cut would act as a Copy.
                if let Some(block) = effect.and_then(effect_block)
                    && SetClipboardData(preferred_drop_effect(), Some(HANDLE(block.0))).is_err()
                {
                    let _ = GlobalFree(Some(block));
                }
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

/// Whether the clipboard holds files, which Paste copies or moves.
pub(crate) fn holds_files() -> bool {
    // SAFETY: plain values.
    unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_ok()
}

/// Files that Paste read from the clipboard. It keeps the clipboard's data
/// object until the paste is submitted, so a move can tell the files'
/// source that it moved them.
pub(crate) struct ClipboardFiles {
    data: IDataObject,
    /// The clipboard's sequence number when Paste read it, which changes
    /// when anything else is put there.
    sequence: u32,
    pub(crate) paths: Vec<PathBuf>,
    /// The files were cut, so Paste moves them.
    pub(crate) cut: bool,
}

/// The files on the clipboard, and whether they were cut, or `None` when
/// it holds none or cannot be read.
pub(crate) fn read_files() -> Option<ClipboardFiles> {
    // SAFETY: OLE is initialized on this thread at start; the data object
    // is released when the value drops, and each medium after it is read.
    unsafe {
        let sequence = GetClipboardSequenceNumber();
        let data = OleGetClipboard().ok()?;
        let mut files = read_format(&data, CF_HDROP)?;
        let paths = drop_paths(&files);
        ReleaseStgMedium(&mut files);
        let cut = match read_format(&data, preferred_drop_effect()) {
            Some(mut effect) => {
                let value = read_effect(&effect);
                ReleaseStgMedium(&mut effect);
                value.is_some_and(|value| value & DROPEFFECT_MOVE != 0)
            }
            None => false,
        };
        (!paths.is_empty()).then_some(ClipboardFiles {
            data,
            sequence,
            paths,
            cut,
        })
    }
}

impl ClipboardFiles {
    /// After a move of these files was submitted: when the clipboard still
    /// holds them, tells their source that they were moved, and empties the
    /// clipboard, as File Explorer does after its own paste. It never sets
    /// the performed effect to move, which would tell the source to delete
    /// the files itself.
    pub(crate) fn moved(self, owner: Hwnd) {
        // SAFETY: plain values.
        if unsafe { GetClipboardSequenceNumber() } != self.sequence {
            return;
        }
        if let Some(block) = effect_block(DROPEFFECT_MOVE) {
            let medium = STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: block },
                pUnkForRelease: ManuallyDrop::new(None),
            };
            // SAFETY: with fRelease set, the data object owns the medium
            // once the call succeeds; otherwise it stays this block's.
            if unsafe {
                self.data
                    .SetData(&format_of(paste_succeeded()), &medium, true)
            }
            .is_err()
            {
                // SAFETY: the block is this function's own.
                let _ = unsafe { GlobalFree(Some(block)) };
            }
        }
        // SAFETY: plain values; a clipboard another program holds stays as
        // it is.
        unsafe {
            if OpenClipboard(Some(owner.0)).is_ok() {
                let _ = EmptyClipboard();
                let _ = CloseClipboard();
            }
        }
    }
}

/// A block of memory in the clipboard format `format`.
fn format_of(format: u32) -> FORMATETC {
    FORMATETC {
        cfFormat: format as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// The medium of `format` from `data`, a block of memory, if it has one.
///
/// # Safety
/// The caller releases the medium with `ReleaseStgMedium`.
unsafe fn read_format(data: &IDataObject, format: u32) -> Option<STGMEDIUM> {
    // SAFETY: the format asks for a block of memory.
    let mut medium = unsafe { data.GetData(&format_of(format)) }.ok()?;
    if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        return Some(medium);
    }
    // SAFETY: the medium is this function's own.
    unsafe { ReleaseStgMedium(&mut medium) };
    None
}

/// The paths of a `CF_HDROP` medium.
///
/// # Safety
/// `medium` holds a `CF_HDROP` block that `read_format` returned.
unsafe fn drop_paths(medium: &STGMEDIUM) -> Vec<PathBuf> {
    // SAFETY: the block is a DROPFILES, which DragQueryFileW reads.
    unsafe {
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        (0..count)
            .filter_map(|index| {
                let length = DragQueryFileW(drop, index, None) as usize;
                let mut buffer = vec![0u16; length + 1];
                let copied = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
                (copied > 0)
                    .then(|| PathBuf::from(OsString::from_wide(&buffer[..copied.min(length)])))
            })
            .collect()
    }
}

/// The effect that a 4-byte effect block holds.
///
/// # Safety
/// `medium` holds a block that `read_format` returned.
unsafe fn read_effect(medium: &STGMEDIUM) -> Option<u32> {
    // SAFETY: the block is locked only to read it, and its size is checked
    // first.
    unsafe {
        let block = medium.u.hGlobal;
        if GlobalSize(block) < 4 {
            return None;
        }
        let value = GlobalLock(block).cast::<u32>();
        if value.is_null() {
            return None;
        }
        let effect = value.read_unaligned();
        let _ = GlobalUnlock(block);
        Some(effect)
    }
}
