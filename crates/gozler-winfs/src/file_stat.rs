//! Read one file's current size + last-write time by its file ID, for the live
//! enricher. USN change records carry neither size nor timestamps, so a file
//! created or modified after bootstrap needs an out-of-band stat to refresh
//! them (otherwise its size stays "unknown", which downstream renders as a
//! directory).

use core::mem::zeroed;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
};

use crate::error::{WinError, last_error};
use crate::file_id::open_by_id;
use crate::volume::VolumeHandle;

/// `FILE_ATTRIBUTE_DIRECTORY`, defined locally per this crate's convention.
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

/// The live size/mtime of one file.
pub struct FileStat {
    pub size: u64,
    /// Last-write time as a Windows `FILETIME` packed into `i64` (100 ns ticks
    /// since 1601), the same representation the FILE_LAYOUT path produces.
    pub mtime_ft: i64,
    pub is_dir: bool,
}

/// Opens the file identified by `id` on `volume` and reads its size and
/// last-write time. Returns `Ok(None)` when the file cannot be opened (already
/// deleted, or transiently locked): a non-fatal "nothing to refresh", since
/// the structural create/delete was already applied from the journal.
pub fn stat_file(volume: &VolumeHandle, id: u128) -> Result<Option<FileStat>, WinError> {
    let Ok(file) = open_by_id(volume, id, FILE_READ_ATTRIBUTES) else {
        return Ok(None);
    };
    // SAFETY: `info` is a plain struct the call fills, and `file` is live.
    // The size/mtime arithmetic below stays in safe code.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        if GetFileInformationByHandle(file.as_handle(), &mut info) == 0 {
            return Err(last_error("GetFileInformationByHandle"));
        }
        info
    };

    let size = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    let mtime_ft = ((info.ftLastWriteTime.dwHighDateTime as i64) << 32)
        | info.ftLastWriteTime.dwLowDateTime as i64;
    let is_dir = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    Ok(Some(FileStat {
        size,
        mtime_ft,
        is_dir,
    }))
}
