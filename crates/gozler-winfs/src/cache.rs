//! Emptying the standby list, so a measured copy reads from the disk and not
//! from memory. It needs an elevated token.

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Security::TOKEN_ADJUST_PRIVILEGES;

use crate::elevation::{enable_privilege, open_current_process_token};
use crate::error::WinError;

/// `SYSTEM_INFORMATION_CLASS::SystemMemoryListInformation`.
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
/// `SYSTEM_MEMORY_LIST_COMMAND::MemoryPurgeStandbyList`.
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSetSystemInformation(class: i32, information: *const i32, length: u32) -> i32;
}

/// Drops the file pages that memory holds unmodified, so the next read of
/// any file comes from the disk. Enables `SeProfileSingleProcessPrivilege`.
pub fn empty_standby_list() -> Result<(), WinError> {
    let token = open_current_process_token(TOKEN_ADJUST_PRIVILEGES)?;
    let enabled = enable_privilege(token, "SeProfileSingleProcessPrivilege");
    // SAFETY: `token` is owned here and closed exactly once.
    unsafe { CloseHandle(token) };
    enabled?;
    let command = MEMORY_PURGE_STANDBY_LIST;
    // SAFETY: the class takes one 4-byte command, which `command` holds for
    // the call.
    let status = unsafe {
        NtSetSystemInformation(
            SYSTEM_MEMORY_LIST_INFORMATION,
            &command,
            size_of::<i32>() as u32,
        )
    };
    if status < 0 {
        return Err(WinError {
            code: status as u32,
            context: "NtSetSystemInformation (purge the standby list)",
        });
    }
    Ok(())
}
