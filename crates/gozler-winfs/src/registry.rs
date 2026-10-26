//! Reads and writes values of the machine's registry, where an administrator
//! saved the service's settings.

use core::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_SUCCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_MULTI_SZ, REG_ROUTINE_FLAGS, RRF_RT_REG_MULTI_SZ, RRF_RT_REG_SZ,
    RegGetValueW, RegSetKeyValueW,
};

use crate::error::WinError;
use crate::util::{from_wide_nul, to_wide_nul};

/// The longest string value read, so a damaged value cannot make the daemon
/// allocate without bound.
const MAX_STRING_BYTES: u32 = 1 << 20;

/// The longest list read or written. A list of SID strings needs far less.
const MAX_STRINGS_BYTES: u32 = 64 * 1024;

/// The string value `name` of the key `key` below `HKEY_LOCAL_MACHINE`, or
/// `None` when the key or the value does not exist. A value longer than 1 MiB
/// fails with `ERROR_MORE_DATA`.
pub fn machine_string(key: &str, name: &str) -> Result<Option<String>, WinError> {
    let value = read_value(key, name, RRF_RT_REG_SZ, MAX_STRING_BYTES)?;
    Ok(value.map(|units| from_wide_nul(&units)))
}

/// The `REG_MULTI_SZ` value `name` of the key `key` below
/// `HKEY_LOCAL_MACHINE`, one string per line, or `None` when the key or the
/// value does not exist. A value longer than 64 KiB fails with
/// `ERROR_MORE_DATA`.
pub fn machine_strings(key: &str, name: &str) -> Result<Option<Vec<String>>, WinError> {
    let value = read_value(key, name, RRF_RT_REG_MULTI_SZ, MAX_STRINGS_BYTES)?;
    Ok(value.map(|units| {
        units
            .split(|&unit| unit == 0)
            .filter(|line| !line.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }))
}

/// Stores `lines` as the `REG_MULTI_SZ` value `name` of the key `key` below
/// `HKEY_LOCAL_MACHINE`, creating the key when it is missing. Needs a caller
/// who may write the key, such as an administrator.
///
/// # Errors
///
/// `ERROR_INVALID_PARAMETER` for an empty line or one that holds a NUL,
/// which `REG_MULTI_SZ` cannot store, and for a list longer than
/// [`machine_strings`] reads; otherwise the registry's error, such as
/// `ERROR_ACCESS_DENIED`.
pub fn set_machine_strings(
    key: &str,
    name: &str,
    lines: &[impl AsRef<str>],
) -> Result<(), WinError> {
    let mut data: Vec<u16> = Vec::new();
    for line in lines {
        let line = line.as_ref();
        if line.is_empty() || line.contains('\0') {
            return Err(WinError {
                code: ERROR_INVALID_PARAMETER,
                context: "set_machine_strings: a line is empty or holds a NUL",
            });
        }
        data.extend(line.encode_utf16());
        data.push(0);
    }
    data.push(0);
    let bytes = size_of_val(data.as_slice());
    if bytes > MAX_STRINGS_BYTES as usize {
        return Err(WinError {
            code: ERROR_INVALID_PARAMETER,
            context: "set_machine_strings: the list is larger than 64 KiB",
        });
    }
    let key = to_wide_nul(key);
    let name = to_wide_nul(name);
    // SAFETY: `key` and `name` are NUL-terminated, and `data` holds `bytes`
    // bytes, ending in the two NULs REG_MULTI_SZ needs; all three outlive
    // the call, which only reads them. It creates a missing key, non-volatile
    // and with the default security descriptor.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            REG_MULTI_SZ,
            data.as_ptr().cast(),
            bytes as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(WinError {
            code: status,
            context: "RegSetKeyValueW",
        });
    }
    Ok(())
}

/// The UTF-16 data of the value `name` of `key` below `HKEY_LOCAL_MACHINE`
/// whose type `flags` allows, or `None` when the key or the value does not
/// exist. A value longer than `max_bytes` fails with `ERROR_MORE_DATA`.
fn read_value(
    key: &str,
    name: &str,
    flags: REG_ROUTINE_FLAGS,
    max_bytes: u32,
) -> Result<Option<Vec<u16>>, WinError> {
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
            flags,
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
    if bytes > max_bytes {
        return Err(WinError {
            code: ERROR_MORE_DATA,
            context: "RegGetValueW: the value is larger than its cap",
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
            flags,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut length,
        )
    };
    match status {
        ERROR_SUCCESS => {
            buf.truncate(length as usize / 2);
            Ok(Some(buf))
        }
        ERROR_FILE_NOT_FOUND => Ok(None),
        code => Err(WinError {
            code,
            context: "RegGetValueW",
        }),
    }
}
