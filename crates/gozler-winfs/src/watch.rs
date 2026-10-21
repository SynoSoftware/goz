//! Watching one folder for changes of its names, which the window's panels
//! follow (`ReadDirectoryChangesW`).
//!
//! The watch reads overlapped, so it is armed the moment it opens: Windows
//! keeps every change from the first read on, also while the caller lists
//! the folder, and the caller then applies them on its listing. Its thread
//! waits for the changes or for a [`Signal`], so stopping it never depends
//! on a cancel reaching the thread in time. Another thread can also close
//! the watch's handle at once through its [`WatchRelease`], as Windows asks
//! before it removes the folder's drive. The changes come back as raw
//! `FILE_NOTIFY_INFORMATION` bytes; `gozler-core` parses them.

use core::mem::zeroed;
use core::ptr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::error::{WinError, last_error};

/// A kernel handle that this module closes once.
struct Handle(HANDLE);

// SAFETY: a kernel object reference, which any thread of the process may
// use; the watch's handle is used under its mutex, and an event's calls are
// safe from any thread.
unsafe impl Send for Handle {}
// SAFETY: as above; a shared `Handle` is only an event, whose `SetEvent`
// and waits are safe from several threads at once.
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and owns; closed once, here.
        unsafe { CloseHandle(self.0) };
    }
}

fn event() -> Result<Handle, WinError> {
    // SAFETY: a manual-reset event with no name or security attributes.
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(last_error("CreateEventW"));
    }
    Ok(Handle(event))
}

/// A signal that a watching thread waits for beside the changes, so that
/// another thread can stop it at once; once set, it stays set.
pub struct Signal(Handle);

impl Signal {
    pub fn new() -> Result<Signal, WinError> {
        event().map(Signal)
    }

    pub fn set(&self) {
        // SAFETY: a live event handle.
        unsafe { SetEvent(self.0.0) };
    }
}

/// The watch's handle, which its thread reads with and another thread may
/// close first.
type Shared = Arc<Mutex<Option<Handle>>>;

/// An open watch on one folder, armed from the moment it opened.
pub struct FolderWatch {
    handle: Shared,
    /// Set when a read completes.
    done: Handle,
    /// Boxed so that its address stays fixed while a read is pending.
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u64]>,
    pending: bool,
}

/// What [`FolderWatch::next`] found.
pub enum WatchEvent<'a> {
    /// Names in the folder were added, removed, renamed or written, as a
    /// chain of `FILE_NOTIFY_INFORMATION` entries. An empty slice means
    /// Windows lost changes, because they did not fit, so the caller reads
    /// the folder again.
    Changes(&'a [u8]),
    /// Nothing changed for the time the caller gave; the read stays armed.
    Quiet,
    /// The caller's stop signal was set.
    Stopped,
}

/// Closes a watch's handle from another thread, which ends its pending read.
pub struct WatchRelease(Shared);

impl WatchRelease {
    /// Closes the handle at once, without waiting for the watching thread,
    /// which then ends its wait. Closing it again does nothing.
    pub fn release(&self) {
        drop(self.0.lock().unwrap_or_else(PoisonError::into_inner).take());
    }
}

/// Opens a watch on the folder at `path`, a full path with the `\\?\` or
/// `\\?\UNC\` prefix and without a NUL, and arms it, so it keeps every
/// change from now on. `words` sizes its buffer; a network share takes at
/// most 64 KiB. Every open shares read, write and delete, so the watch never
/// stops another program, or a delete of the folder.
pub fn watch_folder(path: &[u16], words: usize) -> Result<FolderWatch, WinError> {
    let path: Vec<u16> = path.iter().copied().chain(Some(0)).collect();
    // SAFETY: `path` is NUL-terminated and outlives the call; no security
    // attributes and no template are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (watch_folder)"));
    }
    let handle = Handle(handle);
    let mut watch = FolderWatch {
        handle: Arc::new(Mutex::new(Some(handle))),
        done: event()?,
        // SAFETY: OVERLAPPED is plain data, valid when zeroed.
        overlapped: Box::new(unsafe { zeroed() }),
        buffer: vec![0u64; words].into_boxed_slice(),
        pending: false,
    };
    watch.arm()?;
    Ok(watch)
}

impl FolderWatch {
    /// What another thread closes the handle with.
    pub fn releaser(&self) -> WatchRelease {
        WatchRelease(Arc::clone(&self.handle))
    }

    /// The handle's value, which `RegisterDeviceNotificationW` names so
    /// Windows asks before it removes the folder's drive; `None` once it is
    /// released.
    pub fn handle_value(&self) -> Option<isize> {
        let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        handle.as_ref().map(|handle| handle.0 as isize)
    }

    /// Starts the next read.
    fn arm(&mut self) -> Result<(), WinError> {
        let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(handle) = handle.as_ref() else {
            return Err(WinError {
                code: ERROR_OPERATION_ABORTED,
                context: "ReadDirectoryChangesW (released)",
            });
        };
        // SAFETY: OVERLAPPED is plain data, valid when zeroed.
        *self.overlapped = unsafe { zeroed() };
        self.overlapped.hEvent = self.done.0;
        // The last read's completion left the event set.
        // SAFETY: a live event handle.
        unsafe { ResetEvent(self.done.0) };
        let len = size_of_val(&*self.buffer) as u32;
        // SAFETY: the buffer and the OVERLAPPED are boxed, so they stay at
        // their addresses until the read completes, which `Drop` waits for.
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle.0,
                self.buffer.as_mut_ptr().cast(),
                len,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
                ptr::null_mut(),
                &mut *self.overlapped,
                None,
            )
        };
        // SAFETY: no parameters.
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(last_error("ReadDirectoryChangesW"));
        }
        self.pending = true;
        Ok(())
    }

    /// Waits until a name in the folder changes, `stop` is set, or `quiet`
    /// passes without a change. The changes that come meanwhile wait in
    /// Windows until the next call.
    pub fn next(&mut self, stop: &Signal, quiet: Duration) -> Result<WatchEvent<'_>, WinError> {
        if !self.pending {
            self.arm()?;
        }
        let waits = [self.done.0, stop.0.0];
        let quiet = u32::try_from(quiet.as_millis()).unwrap_or(INFINITE - 1);
        // SAFETY: two live event handles.
        let waited = unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, quiet) };
        if waited == WAIT_TIMEOUT {
            return Ok(WatchEvent::Quiet);
        }
        if waited != WAIT_OBJECT_0 {
            return Ok(WatchEvent::Stopped);
        }
        let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        self.pending = false;
        let Some(handle) = handle.as_ref() else {
            return Err(WinError {
                code: ERROR_OPERATION_ABORTED,
                context: "ReadDirectoryChangesW (released)",
            });
        };
        let mut returned = 0u32;
        // SAFETY: the read completed, as its event says, so the call only
        // reads the OVERLAPPED.
        let ok = unsafe { GetOverlappedResult(handle.0, &*self.overlapped, &mut returned, 0) };
        if ok == 0 {
            let error = last_error("ReadDirectoryChangesW");
            return if error.code == ERROR_NOTIFY_ENUM_DIR {
                Ok(WatchEvent::Changes(&[]))
            } else {
                Err(error)
            };
        }
        let len = (returned as usize).min(size_of_val(&*self.buffer));
        // SAFETY: the first `len` bytes of the buffer, which the read wrote,
        // borrowed for reading for as long as `self` is.
        Ok(WatchEvent::Changes(unsafe {
            core::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), len)
        }))
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if self.pending {
            if let Some(handle) = &handle {
                // SAFETY: the pending read of this handle and OVERLAPPED.
                unsafe { CancelIoEx(handle.0, &*self.overlapped) };
            }
            // A cancelled read, or one whose handle another thread closed,
            // still completes, and only then may its buffer go.
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(self.done.0, INFINITE) };
        }
        drop(handle);
    }
}
