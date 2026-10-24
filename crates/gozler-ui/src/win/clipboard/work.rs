//! Paste keeps its OLE apartment until its captured source has been released.

use super::{
    CF_HDROP, ClipboardFiles, DROPEFFECT_MOVE, effect_block, paste_succeeded,
    preferred_drop_effect, sequence,
};
use crate::win::{shell::Session, wake::OpenWaker, work::OffThread};
use std::ffi::OsString;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::mpsc;
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::AgileReference;

#[derive(Clone)]
pub(crate) struct CompletionSender {
    sender: mpsc::Sender<Completion>,
    waker: OpenWaker,
}

impl CompletionSender {
    pub(crate) fn new(waker: OpenWaker) -> (Self, mpsc::Receiver<Completion>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender, waker }, receiver)
    }

    fn send(&self, completion: Completion) {
        if self.sender.send(completion).is_ok() {
            self.waker.deliver(|| true);
        }
    }
}

pub(crate) struct Read {
    pub(crate) sequence: u32,
    pub(crate) completion: CompletionSender,
}

impl Read {
    pub(crate) fn run(self, session: &Session, worker: &OffThread) -> Option<ClipboardFiles> {
        if !session.ole(worker) || self.sequence == 0 || sequence() != self.sequence {
            return None;
        }
        // SAFETY: this retained worker owns an initialized OLE apartment.
        let data = unsafe { OleGetClipboard() }.ok()?;
        if sequence() != self.sequence {
            return None;
        }
        let paths = files_of(&data, worker);
        if paths.is_empty() {
            return None;
        }
        // SAFETY: each successfully acquired medium is released on this apartment.
        let cut = unsafe {
            read_format(&data, preferred_drop_effect()).is_some_and(|mut effect| {
                let cut = read_effect(&effect).is_some_and(|value| value & DROPEFFECT_MOVE != 0);
                ReleaseStgMedium(&mut effect);
                cut
            })
        };
        let data = AgileReference::new(&data).ok()?;
        Some(ClipboardFiles {
            paths,
            cut,
            source: Source {
                data: Some(data),
                sequence: self.sequence,
                completion: self.completion,
            },
        })
    }
}

pub(super) struct Source {
    data: Option<AgileReference<IDataObject>>,
    sequence: u32,
    completion: CompletionSender,
}

impl Source {
    pub(super) fn release(&mut self) {
        self.finish(false);
    }

    pub(super) fn moved(mut self) {
        self.finish(true);
    }

    fn finish(&mut self, moved: bool) {
        if let Some(data) = self.data.take() {
            self.completion.send(Completion {
                data,
                sequence: self.sequence,
                moved,
            });
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.finish(false);
    }
}

pub(crate) struct Completion {
    data: AgileReference<IDataObject>,
    sequence: u32,
    moved: bool,
}

impl Completion {
    pub(crate) fn run(self, session: &Session, worker: &OffThread) {
        if session.ole(worker)
            && self.moved
            && let Ok(data) = self.data.resolve()
        {
            tell_moved(&data, self.sequence, worker);
        }
    }
}

fn tell_moved(data: &IDataObject, captured: u32, worker: &OffThread) {
    if sequence() != captured {
        return;
    }
    set_effect(data, paste_succeeded(), DROPEFFECT_MOVE, worker);
    // SAFETY: this worker empties only the still-current captured publication,
    // holding the native clipboard open for the final sequence check.
    unsafe {
        if OpenClipboard(None).is_ok() {
            if sequence() == captured {
                let _ = EmptyClipboard();
            }
            let _ = CloseClipboard();
        }
    }
}
/// Sets the effect format `format` of `data`, the files' source, to
/// `value`.
pub(crate) fn set_effect(data: &IDataObject, format: u32, value: u32, _worker: &OffThread) {
    let Some(block) = effect_block(value) else {
        return;
    };
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: block },
        pUnkForRelease: ManuallyDrop::new(None),
    };
    // SAFETY: with fRelease set, the data object owns the medium once the
    // call succeeds; otherwise it stays this block's.
    if unsafe { data.SetData(&format_of(format), &medium, true) }.is_err() {
        // SAFETY: the block is this function's own.
        let _ = unsafe { GlobalFree(Some(block)) };
    }
}

/// The paths of the files that `data` holds as `CF_HDROP`, none when it
/// holds no files.
pub(crate) fn files_of(data: &IDataObject, _worker: &OffThread) -> Vec<PathBuf> {
    // SAFETY: the medium is released once its paths are read.
    unsafe {
        let Some(mut files) = read_format(data, CF_HDROP) else {
            return Vec::new();
        };
        let paths = drop_paths(&files);
        ReleaseStgMedium(&mut files);
        paths
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
