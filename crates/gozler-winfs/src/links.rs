//! Hard-link set enumeration: every name a file record currently has.
//!
//! A USN `HARD_LINK_CHANGE` record names exactly one link and may name a DEAD
//! one, so the index can never learn a file's real link set from the journal
//! alone. This asks the filesystem instead, and the daemon feeds the answer to
//! `gozler_core::index::VolumeIndex::reconcile_links`.
//!
//! Cost is paid only when a hard-link change arrives, which is rare, or for a
//! renamed file while the journal replays a folder walk. Each name costs one
//! more open, to read its folder's file ID. The caller runs it with the index
//! write lock RELEASED: like `stat_file`, these are blocking file opens and
//! must never sit inside the lock every query contends on.
//!
//! Returns raw UTF-16 exactly as Win32 gave it. NTFS names can contain unpaired
//! surrogates, so decoding is left to `gozler-core`'s WTF-8 layer rather than being
//! lossily flattened here.

use core::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_NAME, ERROR_MORE_DATA,
    ERROR_TOO_MANY_LINKS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FindClose, FindFirstFileNameW, FindNextFileNameW,
    GetFinalPathNameByHandleW, VOLUME_NAME_GUID,
};

use crate::error::{WinError, last_error};
use crate::file_id::{is_gone, open_by_id, path_file_id};
use crate::volume::VolumeHandle;

const BACKSLASH: u16 = b'\\' as u16;

/// Starting buffer for a link name, in UTF-16 units. Grown on `ERROR_MORE_DATA`.
const NAME_BUF_UNITS: usize = 512;
/// Refuse to walk a pathological link set rather than spin. NTFS caps hard links
/// at 1024 per file.
const MAX_LINKS: usize = 4096;

/// One hard-link name of a file.
pub struct Link {
    /// The file ID of the folder that holds the name.
    pub folder: u128,
    /// The name, as raw UTF-16.
    pub name: Vec<u16>,
}

/// Every current hard-link name of file `id`, each with its folder's file ID.
///
/// The folder comes as an ID, not a path, because the caller's index may not
/// yet know the folder's current name: while the journal replays a build, a
/// path can name a folder that a later record renames or swaps.
///
/// `Ok(None)` means the file is gone: its file ID no longer opens. There is
/// nothing to reconcile, because the journal's delete record removes the file,
/// and the caller must NOT treat it as an empty link set.
///
/// # Errors
/// Returns [`WinError`] for every other failure, including a walk that fails
/// part way. The caller makes the index hold exactly the links it gets, so a
/// partial set would delete the links the walk did not reach.
pub fn file_links(volume: &VolumeHandle, id: u128) -> Result<Option<Vec<Link>>, WinError> {
    let file = match open_by_id(volume, id, FILE_READ_ATTRIBUTES) {
        Ok(file) => file,
        Err(error) if is_gone(&error) => return Ok(None),
        Err(error) => return Err(error),
    };

    // FindFirstFileNameW needs an openable path, and the index's own path for
    // this file is exactly what may be stale (it is why we are reconciling), so
    // ask the filesystem. The GUID form works on a volume with no drive letter.
    let anchor = final_path(file.as_handle(), VOLUME_NAME_GUID | FILE_NAME_NORMALIZED);
    drop(file);
    let anchor = anchor?;

    // `\\?\Volume{GUID}`: the anchor up to the backslash after the `\\?\`
    // prefix. Every link path continues from there.
    let unexpected = |context| WinError {
        code: ERROR_INVALID_NAME,
        context,
    };
    let volume_len = anchor
        .iter()
        .skip(4)
        .position(|&c| c == BACKSLASH)
        .ok_or(unexpected("GetFinalPathNameByHandleW"))?
        + 4;
    walk_links(&anchor)?
        .into_iter()
        .map(|path| {
            let split = path
                .iter()
                .rposition(|&c| c == BACKSLASH)
                .ok_or(unexpected("FindNextFileNameW"))?;
            // The root keeps its backslash; any other folder opens without.
            let mut folder = anchor[..volume_len].to_vec();
            folder.extend_from_slice(&path[..split.max(1)]);
            folder.push(0);
            Ok(Link {
                folder: path_file_id(&folder)?,
                name: path[split + 1..].to_vec(),
            })
        })
        .collect::<Result<_, _>>()
        .map(Some)
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
