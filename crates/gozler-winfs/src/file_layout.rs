//! `FSCTL_QUERY_FILE_LAYOUT`: the documented bulk path for sizes/timestamps/
//! extra hard-link names on NTFS. Like the journal FSCTLs, the raw output page
//! is handed back untouched for `gozler-core` to parse.

use windows_sys::Win32::Foundation::ERROR_HANDLE_EOF;

use crate::error::WinError;
use crate::ioctl::{FSCTL_QUERY_FILE_LAYOUT, QueryFileLayoutInput, ioctl_in_bytes};
use crate::volume::VolumeHandle;

/// `QUERY_FILE_LAYOUT_RESTART`: begins a fresh pass. It lives in the INPUT
/// struct's `Flags`, not in the per-record flags.
const QUERY_FILE_LAYOUT_RESTART: u32 = 0x0000_0001;

/// `QUERY_FILE_LAYOUT_FILTER_TYPE_NONE`: no filtering, enumerate the whole
/// volume. Requires `FilterEntryCount == 0` (the canonical whole-volume form,
/// used by wimlib).
const FILTER_TYPE_NONE: u32 = 0;

/// `FSCTL_QUERY_FILE_LAYOUT`, one page, whole-volume (`NONE` filter). `flags`
/// is the caller's `QUERY_FILE_LAYOUT_INPUT` flags (e.g.
/// `gozler-core::layout::RECOMMENDED_LAYOUT_FLAGS`); `restart` OR's in
/// `QUERY_FILE_LAYOUT_RESTART` on the first call of a pass. Returns
/// `Some(bytes_written)` or `None` at end-of-scan (`ERROR_HANDLE_EOF`).
pub fn query_file_layout(
    h: &VolumeHandle,
    flags: u32,
    restart: bool,
    out: &mut [u8],
) -> Result<Option<usize>, WinError> {
    let input = QueryFileLayoutInput {
        filter_entry_count: 0,
        flags: flags
            | if restart {
                QUERY_FILE_LAYOUT_RESTART
            } else {
                0
            },
        filter_type: FILTER_TYPE_NONE,
        reserved: 0,
        filter: [0, 0],
    };
    match ioctl_in_bytes(h.as_handle(), FSCTL_QUERY_FILE_LAYOUT, &input, out) {
        Ok(bytes) => Ok(Some(bytes as usize)),
        Err(ERROR_HANDLE_EOF) => Ok(None),
        Err(code) => Err(WinError {
            code,
            context: "FSCTL_QUERY_FILE_LAYOUT",
        }),
    }
}
