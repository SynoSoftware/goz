//! Hard-link set enumeration: every name a file record currently has.
//!
//! A USN `HARD_LINK_CHANGE` record names exactly one link and may name a DEAD
//! one, so the index can never learn a file's real link set from the journal
//! alone. This asks the filesystem instead, and the daemon feeds the answer to
//! `gozler_core::index::VolumeIndex::reconcile_links`.
//!
//! Cost is paid only when a hard-link change actually arrives, which is rare,
//! and the caller runs it with the index write lock RELEASED: like `stat_file`,
//! these are blocking file opens and must never sit inside the lock every query
//! contends on.
//!
//! Returns raw UTF-16 exactly as Win32 gave it. NTFS names can contain unpaired
//! surrogates, so decoding is left to `gozler-core`'s WTF-8 layer rather than being
//! lossily flattened here.

use core::mem::{size_of, zeroed};
use core::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER,
    ERROR_MORE_DATA, ERROR_TOO_MANY_LINKS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_DESCRIPTOR, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FindClose, FindFirstFileNameW,
    FindNextFileNameW, GetFinalPathNameByHandleW, OpenFileById, VOLUME_NAME_GUID,
};

use crate::error::{WinError, last_error};
use crate::volume::VolumeHandle;

/// `FileIdType`: a 64-bit NTFS file reference number in `FILE_ID_DESCRIPTOR`.
const FILE_ID_TYPE_FILE: i32 = 0;
/// Starting buffer for a link name, in UTF-16 units. Grown on `ERROR_MORE_DATA`.
const NAME_BUF_UNITS: usize = 512;
/// Refuse to walk a pathological link set rather than spin. NTFS caps hard links
/// at 1024 per file.
const MAX_LINKS: usize = 4096;

/// Every current hard-link name of `frn`, each as a volume-relative UTF-16 path
/// (`\dir\name`, no drive letter), exactly as `FindNextFileNameW` reports them.
///
/// `Ok(None)` means the file is gone: its file ID no longer opens. There is
/// nothing to reconcile, because the journal's delete record removes the file,
/// and the caller must NOT treat it as an empty link set.
///
/// # Errors
/// Returns [`WinError`] for every other failure, including a walk that fails
/// part way. The caller makes the index hold exactly the links it gets, so a
/// partial set would delete the links the walk did not reach.
pub fn link_paths(volume: &VolumeHandle, frn: u64) -> Result<Option<Vec<Vec<u16>>>, WinError> {
    // SAFETY: `desc` is fully initialized before use; OpenFileById takes the
    // live volume-hint handle; the returned handle is closed on every path
    // below, including the early returns.
    let file = unsafe {
        let mut desc: FILE_ID_DESCRIPTOR = zeroed();
        desc.dwSize = size_of::<FILE_ID_DESCRIPTOR>() as u32;
        desc.Type = FILE_ID_TYPE_FILE;
        desc.Anonymous.FileId = frn as i64;

        OpenFileById(
            volume.as_handle(),
            &desc,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            FILE_FLAG_BACKUP_SEMANTICS,
        )
    };
    if file == INVALID_HANDLE_VALUE {
        let error = last_error("OpenFileById");
        // A file ID whose file is gone opens with ERROR_INVALID_PARAMETER,
        // whether its record is free or already holds another file.
        return if error.code == ERROR_INVALID_PARAMETER {
            Ok(None)
        } else {
            Err(error)
        };
    }

    // FindFirstFileNameW needs an openable path, and the index's own path for
    // this FRN is exactly what may be stale (it is why we are reconciling), so
    // ask the filesystem. The GUID form works on a volume with no drive letter.
    let anchor = final_path(file, VOLUME_NAME_GUID | FILE_NAME_NORMALIZED);
    // SAFETY: `file` is a live handle this function opened and has not closed.
    unsafe {
        CloseHandle(file);
    }

    walk_links(&anchor?).map(Some)
}

/// `GetFinalPathNameByHandleW` into an owned NUL-terminated UTF-16 buffer.
fn final_path(file: HANDLE, flags: u32) -> Result<Vec<u16>, WinError> {
    // SAFETY: `file` is live for this call. The first call passes a zero-length
    // buffer, which the API documents as "return the required length".
    let needed = unsafe { GetFinalPathNameByHandleW(file, ptr::null_mut(), 0, flags) };
    if needed == 0 {
        return Err(last_error("GetFinalPathNameByHandleW"));
    }
    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` holds `needed` units, which is the length the call above
    // asked for; the API writes at most that many including the NUL.
    let written = unsafe { GetFinalPathNameByHandleW(file, buf.as_mut_ptr(), needed, flags) };
    if written == 0 {
        return Err(last_error("GetFinalPathNameByHandleW"));
    }
    if written >= needed {
        // The path grew between the calls (a rename), and the API returned
        // the new required length instead of the path.
        return Err(WinError {
            code: ERROR_INSUFFICIENT_BUFFER,
            context: "GetFinalPathNameByHandleW",
        });
    }
    buf.truncate(written as usize + 1); // keep the NUL for PCWSTR
    Ok(buf)
}

/// Walks `FindFirstFileNameW`/`FindNextFileNameW` over an anchor path. Returns
/// the whole link set or an error, never part of it.
fn walk_links(anchor: &[u16]) -> Result<Vec<Vec<u16>>, WinError> {
    let mut buf = vec![0u16; NAME_BUF_UNITS];

    let find = loop {
        let mut len = buf.len() as u32;
        // SAFETY: `anchor` is NUL-terminated; `len` names `buf`'s capacity and
        // the API writes at most that many units, updating `len` to what it
        // needs.
        let find = unsafe { FindFirstFileNameW(anchor.as_ptr(), 0, &mut len, buf.as_mut_ptr()) };
        if find != INVALID_HANDLE_VALUE {
            break find;
        }
        let error = last_error("FindFirstFileNameW");
        if error.code == ERROR_MORE_DATA && len as usize > buf.len() {
            buf = vec![0u16; len as usize];
        } else {
            // Also a file deleted or renamed since it was opened. Only the
            // file ID proves a file gone; a path that stopped resolving may
            // belong to a live file with an unreconciled link.
            return Err(error);
        }
    };

    let mut links = vec![take_name(&buf)];
    let walked = loop {
        if links.len() > MAX_LINKS {
            break Err(WinError {
                code: ERROR_TOO_MANY_LINKS,
                context: "FindNextFileNameW",
            });
        }
        let mut len = buf.len() as u32;
        // SAFETY: `find` is the live search handle; `len` names `buf`'s capacity.
        if unsafe { FindNextFileNameW(find, &mut len, buf.as_mut_ptr()) } != 0 {
            links.push(take_name(&buf));
            continue;
        }
        let error = last_error("FindNextFileNameW");
        match error.code {
            ERROR_HANDLE_EOF => break Ok(links),
            ERROR_MORE_DATA if len as usize > buf.len() => buf = vec![0u16; len as usize],
            _ => break Err(error),
        }
    };

    // SAFETY: `find` came from FindFirstFileNameW and is closed exactly once.
    unsafe {
        FindClose(find);
    }
    walked
}

/// Copies the NUL-terminated name out of the scratch buffer.
fn take_name(buf: &[u16]) -> Vec<u16> {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf[..end].to_vec()
}
