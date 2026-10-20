//! Reads a string value of the machine's registry, where an administrator
//! saved the service's settings.

use core::ptr;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

use crate::error::WinError;
use crate::util::{from_wide_nul, to_wide_nul};

/// The longest value read, so a damaged value cannot make the daemon
/// allocate without bound.
const MAX_VALUE_BYTES: u32 = 1 << 20;

/// The string value `name` of the key `key` below `HKEY_LOCAL_MACHINE`, or
/// `None` when the key or the value does not exist. A value longer than 1 MiB
/// fails with `ERROR_MORE_DATA`.
pub fn machine_string(key: &str, name: &str) -> Result<Option<String>, WinError> {
    let key = to_wide_nul(key);
    let name = to_wide_nul(name);
    let mut bytes = 0u32;
    // SAFETY: the names are NUL-terminated and outlive the call; a null
    // buffer asks only for the value's size, which goes to `bytes`.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    match status {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        code => {
            return Err(WinError {
                code,
                context: "RegGetValueW",
            });
        }
    }
    if bytes > MAX_VALUE_BYTES {
        return Err(WinError {
            code: ERROR_MORE_DATA,
            context: "RegGetValueW: the value is larger than 1 MiB",
        });
    }
    let mut buf = vec![0u16; bytes.div_ceil(2) as usize];
    let mut length = bytes;
    // SAFETY: as above; the call writes at most `length` bytes into `buf`,
    // which holds that many.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut length,
        )
    };
    match status {
        ERROR_SUCCESS => Ok(Some(from_wide_nul(&buf))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        code => Err(WinError {
            code,
            context: "RegGetValueW",
        }),
    }
}
