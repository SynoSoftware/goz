//! The Windows error codes that the engine's decisions read, here so that
//! the decisions build and are tested on every OS.

pub(crate) const ERROR_INVALID_FUNCTION: u32 = 1;
pub(crate) const ERROR_FILE_NOT_FOUND: u32 = 2;
pub(crate) const ERROR_PATH_NOT_FOUND: u32 = 3;
pub(crate) const ERROR_INVALID_DATA: u32 = 13;
pub(crate) const ERROR_SHARING_VIOLATION: u32 = 32;
pub(crate) const ERROR_LOCK_VIOLATION: u32 = 33;
pub(crate) const ERROR_NOT_SUPPORTED: u32 = 50;
pub(crate) const ERROR_INVALID_PARAMETER: u32 = 87;
pub(crate) const ERROR_DIR_NOT_EMPTY: u32 = 145;
pub(crate) const ERROR_REPARSE_POINT_ENCOUNTERED: u32 = 4395;

/// Whether a failed open or delete means that the entry is not there.
pub(crate) fn is_gone(code: u32) -> bool {
    matches!(code, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
}

pub(crate) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// `IsReparseTagNameSurrogate`: the tag of a junction, a symbolic link or a
/// mount point, which name another entry. A cloud placeholder's tag is not
/// one.
pub(crate) fn is_name_surrogate(reparse_tag: u32) -> bool {
    reparse_tag & 0x2000_0000 != 0
}
