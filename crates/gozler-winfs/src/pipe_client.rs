//! What the daemon learns of a pipe client and the ids it hands out: the
//! user the client runs as, by which the daemon caps the memory each user's
//! result snapshots take, and random ids that another user cannot guess.

use core::ffi::c_void;
use core::ptr;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::error::{WinError, last_error};

/// The bytes of the user SID of the process at the other end of the pipe
/// `handle`, a server end.
///
/// # Errors
///
/// [`WinError`] when the client's process or token cannot be read, as for a
/// client that has already exited.
pub fn pipe_client_user(handle: BorrowedHandle<'_>) -> Result<Vec<u8>, WinError> {
    let mut pid = 0u32;
    // SAFETY: `handle` is a live, borrowed pipe handle; `pid` is a live out
    // param written on success.
    if unsafe { GetNamedPipeClientProcessId(handle.as_raw_handle(), &mut pid) } == 0 {
        return Err(last_error("GetNamedPipeClientProcessId"));
    }
    // SAFETY: no handle is inherited; the call returns null on failure.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(last_error("OpenProcess"));
    }
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a valid process handle; `token` is a live out
    // param written on success.
    let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    let opened = if opened == 0 {
        Err(last_error("OpenProcessToken"))
    } else {
        Ok(token)
    };
    // SAFETY: `process` was opened above and is owned here; closed once.
    unsafe { CloseHandle(process) };
    let token = opened?;
    let user = token_user(token);
    // SAFETY: `token` was opened above and is owned here; closed once.
    unsafe { CloseHandle(token) };
    user
}

/// The bytes of the user SID of `token`.
fn token_user(token: HANDLE) -> Result<Vec<u8>, WinError> {
    // A TOKEN_USER and the largest SID (SECURITY_MAX_SID_SIZE, 68 bytes),
    // in u64s so the buffer has the struct's alignment.
    let mut buf = [0u64; (size_of::<TOKEN_USER>() + 68).div_ceil(8)];
    let mut len = 0u32;
    // SAFETY: `token` is a valid token opened with TOKEN_QUERY; `buf` is a
    // live buffer of the size passed; `len` is a live out param.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            size_of_val(&buf) as u32,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER, aligned as u64
    // is, whose SID points into the same buffer and is valid while it lives;
    // GetLengthSid reads only that SID, and the copy is taken before `buf`
    // ends.
    let sid = unsafe {
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        let sid = user.User.Sid;
        core::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize).to_vec()
    };
    Ok(sid)
}

/// 128 bits from the system's preferred random number generator.
///
/// # Errors
///
/// [`WinError`] with the `NTSTATUS` when the generator fails.
pub fn random_u128() -> Result<u128, WinError> {
    let mut bytes = [0u8; 16];
    // SAFETY: no algorithm handle is passed, as BCRYPT_USE_SYSTEM_PREFERRED_RNG
    // requires; `bytes` is a live buffer of the length passed.
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(WinError {
            code: status as u32,
            context: "BCryptGenRandom",
        });
    }
    Ok(u128::from_le_bytes(bytes))
}
