//! The Windows error codes that the engine's decisions read, here so that
//! the decisions build and are tested on every OS.

pub(crate) const ERROR_INVALID_FUNCTION: u32 = 1;
pub(crate) const ERROR_FILE_NOT_FOUND: u32 = 2;
pub(crate) const ERROR_PATH_NOT_FOUND: u32 = 3;
pub(crate) const ERROR_ACCESS_DENIED: u32 = 5;
pub(crate) const ERROR_INVALID_DATA: u32 = 13;
pub(crate) const ERROR_SHARING_VIOLATION: u32 = 32;
pub(crate) const ERROR_LOCK_VIOLATION: u32 = 33;
pub(crate) const ERROR_HANDLE_DISK_FULL: u32 = 39;
pub(crate) const ERROR_NOT_SUPPORTED: u32 = 50;
pub(crate) const ERROR_FILE_EXISTS: u32 = 80;
pub(crate) const ERROR_INVALID_PARAMETER: u32 = 87;
pub(crate) const ERROR_DISK_FULL: u32 = 112;
pub(crate) const ERROR_DIR_NOT_EMPTY: u32 = 145;
pub(crate) const ERROR_ALREADY_EXISTS: u32 = 183;
pub(crate) const ERROR_OPERATION_ABORTED: u32 = 995;
pub(crate) const ERROR_REQUEST_ABORTED: u32 = 1235;
pub(crate) const ERROR_REPARSE_POINT_ENCOUNTERED: u32 = 4395;

/// Whether a failed open or delete means that the entry is not there.
pub(crate) fn is_gone(code: u32) -> bool {
    matches!(code, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
}

pub(crate) const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
pub(crate) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub(crate) const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
pub(crate) const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;

pub(crate) const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

pub(crate) const FILE_SUPPORTS_SPARSE_FILES: u32 = 0x40;
pub(crate) const FILE_SUPPORTS_BLOCK_REFCOUNTING: u32 = 0x0800_0000;

/// The first Windows build whose `CopyFile2` takes
/// `COPY_FILE_ENABLE_SPARSE_COPY`.
pub(crate) const SPARSE_COPY_BUILD: u32 = 22621;

/// `IsReparseTagNameSurrogate`: the tag of a junction, a symbolic link or a
/// mount point, which name another entry. A cloud placeholder's tag is not
/// one.
pub(crate) fn is_name_surrogate(reparse_tag: u32) -> bool {
    reparse_tag & 0x2000_0000 != 0
}
