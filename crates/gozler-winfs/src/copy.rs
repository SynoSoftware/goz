//! The calls the copy engine makes by path: `CopyFile2` with a progress
//! closure that can cancel, the rename, the folder create, and a path's
//! attributes, allocated size and free space.
//!
//! Every path has the `\\?\` or `\\?\UNC\` prefix and no NUL; this module
//! adds the NUL.

use core::ffi::c_void;
use core::mem::{ManuallyDrop, size_of, zeroed};
use core::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_REQUEST_PAUSED, GetLastError, NO_ERROR,
};
use windows_sys::Win32::Storage::FileSystem::{
    COPY_FILE_COPY_SYMLINK, COPY_FILE_DIRECTORY, COPY_FILE_ENABLE_SPARSE_COPY,
    COPY_FILE_FAIL_IF_EXISTS, COPY_FILE_NO_BUFFERING, COPY_FILE_OPEN_AND_COPY_REPARSE_POINT,
    COPY_FILE_RESUME_FROM_PAUSE, COPYFILE2_CALLBACK_CHUNK_FINISHED, COPYFILE2_EXTENDED_PARAMETERS,
    COPYFILE2_EXTENDED_PARAMETERS_V2, COPYFILE2_MESSAGE, COPYFILE2_MESSAGE_ACTION,
    COPYFILE2_PROGRESS_CANCEL, COPYFILE2_PROGRESS_CONTINUE, COPYFILE2_PROGRESS_PAUSE, CopyFile2,
    CreateDirectoryExW, CreateDirectoryW, GetCompressedFileSizeW, GetDiskFreeSpaceExW,
    GetFileAttributesExW, GetFileExInfoStandard, INVALID_FILE_SIZE, MOVEFILE_REPLACE_EXISTING,
    MoveFileExW, SetFileAttributesW, WIN32_FILE_ATTRIBUTE_DATA,
};

use crate::entry::{Access, Entry};
use crate::error::{WinError, last_error};
use crate::file_id::FileHandle;

fn nul(path: &[u16]) -> Vec<u16> {
    path.iter().copied().chain(Some(0)).collect()
}

/// How `CopyFile2` treats the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySource {
    /// A file, read through any file system filter.
    File,
    /// A symbolic link, copied as a link (`COPY_FILE_COPY_SYMLINK`). Without
    /// the right to create symbolic links this fails with
    /// `ERROR_PRIVILEGE_NOT_HELD`.
    SymbolicLink { folder: bool },
    /// Any other link, such as a junction, copied as its reparse point
    /// (`COPY_FILE_OPEN_AND_COPY_REPARSE_POINT`). A symbolic link must not
    /// take this way: without the right, `CopyFile2` then copies a file
    /// link's target's data and makes a folder link an empty folder.
    ReparsePoint { folder: bool },
}

/// What `copy_file` asks of `CopyFile2` besides the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyOptions {
    pub source: CopySource,
    /// `COPY_FILE_NO_BUFFERING`.
    pub unbuffered: bool,
    /// `COPY_FILE_ENABLE_SPARSE_COPY`, which only Windows build 22621 and
    /// later knows.
    pub sparse: bool,
    /// `COPYFILE2_EXTENDED_PARAMETERS_V2::ioDesiredSize` in bytes, which
    /// only Windows build 22621 and later knows; `None` leaves the size to
    /// `CopyFile2`.
    pub io_desired_size: Option<u32>,
    /// Resumes only the restart header left by an earlier paused call.
    pub resume: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyAction {
    Continue,
    Cancel,
    Pause,
}

pub enum CopyState {
    Complete,
    /// Owns the actual destination even if its name changes while paused.
    Paused(Entry),
}

struct Progress<'a> {
    callback: &'a mut dyn FnMut(u64) -> CopyAction,
    destination: Option<Entry>,
    capture_error: Option<WinError>,
}

/// Copies `from` to `to`, which must not exist unless `options.resume`
/// names a previous paused copy. `progress` receives bytes copied so far,
/// including alternate streams. Cancel returns `ERROR_REQUEST_ABORTED`
/// after deleting partial output; Pause retains the destination after writing
/// its restart header, or reports Complete if the copy already finished.
pub fn copy_file(
    from: &[u16],
    to: &[u16],
    options: CopyOptions,
    progress: &mut dyn FnMut(u64) -> CopyAction,
) -> Result<CopyState, WinError> {
    let mut flags = COPY_FILE_FAIL_IF_EXISTS;
    if options.resume {
        flags |= COPY_FILE_RESUME_FROM_PAUSE;
    }
    match options.source {
        CopySource::File => {}
        CopySource::SymbolicLink { folder } => {
            flags |= COPY_FILE_COPY_SYMLINK;
            if folder {
                flags |= COPY_FILE_DIRECTORY;
            }
        }
        CopySource::ReparsePoint { folder } => {
            flags |= COPY_FILE_OPEN_AND_COPY_REPARSE_POINT;
            if folder {
                flags |= COPY_FILE_DIRECTORY;
            }
        }
    }
    if options.unbuffered {
        flags |= COPY_FILE_NO_BUFFERING;
    }
    if options.sparse {
        flags |= COPY_FILE_ENABLE_SPARSE_COPY;
    }
    let (from, to) = (nul(from), nul(to));
    let mut progress = Progress {
        callback: progress,
        destination: None,
        capture_error: None,
    };
    // The version-2 parameters begin with the version-1 fields, and
    // `CopyFile2` reads the version from `dwSize`.
    let parameters = COPYFILE2_EXTENDED_PARAMETERS_V2 {
        dwSize: match options.io_desired_size {
            Some(_) => size_of::<COPYFILE2_EXTENDED_PARAMETERS_V2>(),
            None => size_of::<COPYFILE2_EXTENDED_PARAMETERS>(),
        } as u32,
        dwCopyFlags: flags,
        pfCancel: ptr::null_mut(),
        pProgressRoutine: Some(on_progress),
        pvCallbackContext: ptr::from_mut(&mut progress).cast(),
        ioDesiredSize: options.io_desired_size.unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: both paths are NUL-terminated and outlive the call; the
    // parameters are a version-2 struct whose `dwSize` names its version
    // or the version-1 prefix; the context points at `progress`, which
    // outlives the call, and only `on_progress` reads it, on this thread,
    // while the call runs.
    let result = unsafe {
        CopyFile2(
            from.as_ptr(),
            to.as_ptr(),
            ptr::from_ref(&parameters).cast(),
        )
    };
    if result < 0 {
        // A Win32 error comes as HRESULT_FROM_WIN32, its code in the low
        // 16 bits under facility 7; anything else keeps its HRESULT.
        let result = result as u32;
        let code = if result >> 16 == 0x8007 {
            result & 0xFFFF
        } else {
            result
        };
        if let Some(error) = progress.capture_error {
            return Err(error);
        }
        if code == ERROR_REQUEST_PAUSED
            && let Some(destination) = progress.destination
        {
            return Ok(CopyState::Paused(destination));
        }
        return Err(WinError {
            code,
            context: "CopyFile2",
        });
    }
    Ok(CopyState::Complete)
}

unsafe extern "system" fn on_progress(
    message: *const COPYFILE2_MESSAGE,
    context: *const c_void,
) -> COPYFILE2_MESSAGE_ACTION {
    // SAFETY: `CopyFile2` passes a valid message for the call's duration,
    // and `context` is the `Progress` that `copy_file` passed,
    // still borrowed by that call.
    unsafe {
        let message = &*message;
        if message.Type != COPYFILE2_CALLBACK_CHUNK_FINISHED {
            return COPYFILE2_PROGRESS_CONTINUE;
        }
        let progress = &mut *context.cast::<Progress<'_>>().cast_mut();
        match (progress.callback)(message.Info.ChunkFinished.uliTotalBytesTransferred) {
            CopyAction::Continue => COPYFILE2_PROGRESS_CONTINUE,
            CopyAction::Cancel => COPYFILE2_PROGRESS_CANCEL,
            CopyAction::Pause => {
                // Windows owns this handle through the callback. Reopening
                // by identity shares delete, unlike duplicating its file object;
                // ManuallyDrop leaves the borrowed handle for Windows to close.
                let borrowed = ManuallyDrop::new(Entry(FileHandle(
                    message.Info.ChunkFinished.hDestinationFile,
                )));
                match borrowed.reopen(Access::Inspect) {
                    Ok(destination) => {
                        progress.destination = Some(destination);
                        COPYFILE2_PROGRESS_PAUSE
                    }
                    Err(error) => {
                        progress.capture_error = Some(error);
                        COPYFILE2_PROGRESS_CANCEL
                    }
                }
            }
        }
    }
}

/// What a rename does when its new name exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Fails with `ERROR_ALREADY_EXISTS` or `ERROR_FILE_EXISTS`, or with
    /// `ERROR_ACCESS_DENIED` for a hidden or read-only file.
    Refuse,
    /// Replaces a file; a folder or a read-only file still refuses.
    Replace,
}

/// Renames `from` to `to` on one volume, a folder whole. Never copies: a
/// rename across volumes fails with `ERROR_NOT_SAME_DEVICE`.
pub fn rename(from: &[u16], to: &[u16], existing: Existing) -> Result<(), WinError> {
    let flags = match existing {
        Existing::Refuse => 0,
        Existing::Replace => MOVEFILE_REPLACE_EXISTING,
    };
    let (from, to) = (nul(from), nul(to));
    // SAFETY: both paths are NUL-terminated and outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        return Err(last_error("MoveFileExW"));
    }
    Ok(())
}

/// Creates the folder `path`, with the attributes of the folder `template`
/// when one is given. An existing name fails with `ERROR_ALREADY_EXISTS`.
pub fn create_folder(path: &[u16], template: Option<&[u16]>) -> Result<(), WinError> {
    let path = nul(path);
    let ok = match template {
        Some(template) => {
            let template = nul(template);
            // SAFETY: both paths are NUL-terminated and outlive the call; no
            // security attributes are supplied.
            unsafe { CreateDirectoryExW(template.as_ptr(), path.as_ptr(), ptr::null()) }
        }
        // SAFETY: as above.
        None => unsafe { CreateDirectoryW(path.as_ptr(), ptr::null()) },
    };
    if ok == 0 {
        return Err(last_error("CreateDirectoryExW"));
    }
    Ok(())
}

/// The attributes of what is at `path`, or `None` when nothing is.
pub fn path_attributes(path: &[u16]) -> Result<Option<u32>, WinError> {
    let path = nul(path);
    // SAFETY: `path` is NUL-terminated; `data` is the struct the standard
    // level fills.
    let data: WIN32_FILE_ATTRIBUTE_DATA = unsafe {
        let mut data: WIN32_FILE_ATTRIBUTE_DATA = zeroed();
        if GetFileAttributesExW(
            path.as_ptr(),
            GetFileExInfoStandard,
            ptr::from_mut(&mut data).cast(),
        ) == 0
        {
            let error = last_error("GetFileAttributesExW");
            return match error.code {
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
                _ => Err(error),
            };
        }
        data
    };
    Ok(Some(data.dwFileAttributes))
}

pub fn set_path_attributes(path: &[u16], attributes: u32) -> Result<(), WinError> {
    let path = nul(path);
    // SAFETY: `path` is NUL-terminated and outlives the call.
    if unsafe { SetFileAttributesW(path.as_ptr(), attributes) } == 0 {
        return Err(last_error("SetFileAttributesW"));
    }
    Ok(())
}

/// The bytes a file takes on its volume: less than its size for a sparse
/// or compressed file.
pub fn allocated_size(path: &[u16]) -> Result<u64, WinError> {
    let path = nul(path);
    let mut high = 0u32;
    // SAFETY: `path` is NUL-terminated; `high` is a live out-param.
    let low = unsafe { GetCompressedFileSizeW(path.as_ptr(), &mut high) };
    // INVALID_FILE_SIZE is also a valid low half, so the error decides.
    // SAFETY: GetLastError has no preconditions.
    if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != NO_ERROR {
        return Err(last_error("GetCompressedFileSizeW"));
    }
    Ok(u64::from(high) << 32 | u64::from(low))
}

/// The bytes the person may still write to the volume that holds the
/// folder `path`.
pub fn free_space(path: &[u16]) -> Result<u64, WinError> {
    // A share's root is taken only with its trailing backslash.
    let mut path = path.to_vec();
    if path.last() != Some(&u16::from(b'\\')) {
        path.push(u16::from(b'\\'));
    }
    let path = nul(&path);
    let mut free = 0u64;
    // SAFETY: `path` is NUL-terminated; the unused out-params are null, which
    // the API permits, and `free` outlives the call.
    let ok =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, ptr::null_mut(), ptr::null_mut()) };
    if ok == 0 {
        return Err(last_error("GetDiskFreeSpaceExW"));
    }
    Ok(free)
}
