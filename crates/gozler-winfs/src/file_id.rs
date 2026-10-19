//! Opening files and folders by file ID, and listing a folder.
//!
//! Every open uses `ExtendedFileIdType`, the 128-bit ID: ReFS needs it, and
//! an NTFS ID zero-extended to 128 bits opens the same file. The listing
//! returns raw `FILE_ID_EXTD_DIR_INFO` bytes; `gozler-core` parses them.

use core::mem::{size_of, zeroed};
use core::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER,
    ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ExtendedFileIdType, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_128, FILE_ID_DESCRIPTOR, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdExtdDirectoryInfo, FileIdInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING, OpenFileById,
};

use crate::error::{WinError, last_error};
use crate::util::to_wide_nul;
use crate::volume::VolumeHandle;

/// An open file or folder; `CloseHandle` on drop.
pub(crate) struct FileHandle(HANDLE);

impl FileHandle {
    pub(crate) fn as_handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful open and is owned by this
        // value; it is closed exactly once, here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Opens the file or folder `id` on `volume` with `access`, whatever its
/// permissions allow, because the daemon holds `SeBackupPrivilege`. A
/// symbolic link or a junction opens as itself, which is what the index
/// holds. Without `FILE_FLAG_OPEN_REPARSE_POINT` its open by ID fails with
/// `ERROR_INVALID_PARAMETER`, which [`is_gone`] reads as gone.
pub(crate) fn open_by_id(
    volume: &VolumeHandle,
    id: u128,
    access: u32,
) -> Result<FileHandle, WinError> {
    // SAFETY: `desc` is fully initialized before use, and OpenFileById takes
    // the live volume-hint handle.
    let handle = unsafe {
        let mut desc: FILE_ID_DESCRIPTOR = zeroed();
        desc.dwSize = size_of::<FILE_ID_DESCRIPTOR>() as u32;
        desc.Type = ExtendedFileIdType;
        desc.Anonymous.ExtendedFileId = FILE_ID_128 {
            Identifier: id.to_le_bytes(),
        };
        OpenFileById(
            volume.as_handle(),
            &desc,
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            // Backup semantics also open folders.
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("OpenFileById"));
    }
    Ok(FileHandle(handle))
}

/// Whether an [`open_by_id`] failure means that no file has the ID any more.
/// NTFS answers `ERROR_INVALID_PARAMETER`, whether the record is free or
/// holds another file; `ERROR_FILE_NOT_FOUND` says the same.
pub(crate) fn is_gone(error: &WinError) -> bool {
    matches!(error.code, ERROR_INVALID_PARAMETER | ERROR_FILE_NOT_FOUND)
}

/// The file ID of the root folder of the volume at `guid_path`
/// (`\\?\Volume{GUID}\`). NTFS gives record 5; ReFS gives its own.
pub fn root_folder_id(guid_path: &str) -> Result<u128, WinError> {
    let root = if guid_path.ends_with('\\') {
        to_wide_nul(guid_path)
    } else {
        to_wide_nul(&format!("{guid_path}\\"))
    };
    path_file_id(&root)
}

/// The file ID of the file or folder at `path`, a NUL-terminated UTF-16 path.
pub(crate) fn path_file_id(path: &[u16]) -> Result<u128, WinError> {
    assert_eq!(path.last(), Some(&0), "path is NUL-terminated");
    // SAFETY: `path` is a NUL-terminated path; no security attributes and no
    // template handle are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (path_file_id)"));
    }
    let file = FileHandle(handle);
    // SAFETY: `info` is a plain struct the call fills, and its size is passed.
    let info = unsafe {
        let mut info: FILE_ID_INFO = zeroed();
        let ok = GetFileInformationByHandleEx(
            file.as_handle(),
            FileIdInfo,
            ptr::from_mut(&mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        );
        if ok == 0 {
            return Err(last_error("GetFileInformationByHandleEx (FileIdInfo)"));
        }
        info
    };
    Ok(u128::from_le_bytes(info.FileId.Identifier))
}

/// A folder opened for listing.
pub struct Folder(FileHandle);

/// Opens folder `id` on `volume` for listing. `Ok(None)` means the folder is
/// gone or being deleted, so there is nothing to list.
pub fn open_folder(volume: &VolumeHandle, id: u128) -> Result<Option<Folder>, WinError> {
    match open_by_id(volume, id, FILE_LIST_DIRECTORY) {
        Ok(handle) => Ok(Some(Folder(handle))),
        Err(error) if is_gone(&error) || is_being_deleted(volume, id, &error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether a folder that refused to open has a delete pending: it was deleted
/// while another process still has it open. Windows deletes only an empty
/// folder and creates nothing in one with a delete pending, so it holds
/// nothing to list. It refuses every open with `ERROR_ACCESS_DENIED`, even one
/// that asks for no access, which a folder whose access list refuses listing
/// still allows.
fn is_being_deleted(volume: &VolumeHandle, id: u128, error: &WinError) -> bool {
    error.code == ERROR_ACCESS_DENIED
        && matches!(open_by_id(volume, id, 0), Err(e) if e.code == ERROR_ACCESS_DENIED || is_gone(&e))
}

impl Folder {
    /// Reads the next page of the listing into `buf`: a chain of
    /// `FILE_ID_EXTD_DIR_INFO` entries, as bytes. `Ok(None)` when the
    /// listing is done. The buffer is `u64`s because the entries are 8-byte
    /// aligned.
    pub fn read_listing<'b>(&self, buf: &'b mut [u64]) -> Result<Option<&'b [u8]>, WinError> {
        let len = size_of_val(buf);
        let bytes = buf.as_mut_ptr().cast::<u8>();
        // SAFETY: `bytes` is the start of `buf`, which is `len` writable bytes
        // that outlive the call.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileIdExtdDirectoryInfo,
                bytes.cast(),
                len as u32,
            )
        };
        if ok == 0 {
            let error = last_error("GetFileInformationByHandleEx (FileIdExtdDirectoryInfo)");
            return if error.code == ERROR_NO_MORE_FILES {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // SAFETY: the same `len` bytes of `buf`, now borrowed for reading for
        // as long as `buf` is.
        Ok(Some(unsafe { core::slice::from_raw_parts(bytes, len) }))
    }
}
