//! Gozler's network requests: GitHub's latest release, for Check for updates
//! (DESIGN.md: The General page), and its setup, for Download and install
//! Gozler, with the setup's SHA-256 (DESIGN.md: Service fixes).

use crate::core::update::{LATEST_RELEASE_HOST, LATEST_RELEASE_PATH};
use std::io::{self, Read};
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData,
};
use windows::core::{HSTRING, PCWSTR, w};

/// The most of a reply that is read, far more than a release's
/// description, so a wrong server cannot fill the memory.
const MOST_BYTES: usize = 1 << 20;

/// How long each step of the request may wait, so the thread ends soon on
/// a network that does not answer.
const STEP_TIMEOUT_MS: i32 = 15_000;

/// A WinHTTP handle, closed on drop.
struct Internet(*mut core::ffi::c_void);

impl Internet {
    fn new(handle: *mut core::ffi::c_void) -> Option<Internet> {
        (!handle.is_null()).then_some(Internet(handle))
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        // SAFETY: the handle is this value's own; a request closes before
        // its connection, which closes before its session, because each is
        // dropped in the reverse order of its creation.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// GitHub's description of Gozler's latest release; `None` for any
/// failure, which Check for updates ignores.
pub(crate) fn latest_release() -> Option<String> {
    let mut body = Vec::new();
    read(LATEST_RELEASE_HOST, LATEST_RELEASE_PATH, &mut |piece| {
        body.extend_from_slice(piece);
        body.len() <= MOST_BYTES
    })?;
    String::from_utf8(body).ok()
}

/// An HTTPS GET of `path` on `host`, which follows redirects, giving the
/// body to `piece` as it arrives; `piece` returns false to stop. Returns
/// `Some` when the server answered 200 OK and every piece was taken. It
/// sends the user agent GitHub asks every client to send.
pub(crate) fn read(host: &str, path: &str, piece: &mut dyn FnMut(&[u8]) -> bool) -> Option<()> {
    let agent = format!("Gozler/{}", gozler_core::escompat::version());
    // SAFETY: every string outlives the call that reads it; each handle is
    // used only while its owner lives, and the buffer is written only up
    // to its length.
    unsafe {
        let session = Internet::new(WinHttpOpen(
            &HSTRING::from(agent.as_str()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(
            session.0,
            STEP_TIMEOUT_MS,
            STEP_TIMEOUT_MS,
            STEP_TIMEOUT_MS,
            STEP_TIMEOUT_MS,
        )
        .ok()?;
        let connection = Internet::new(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = Internet::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;
        let mut status = 0u32;
        let mut length = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut length,
            std::ptr::null_mut(),
        )
        .ok()?;
        if status != 200 {
            return None;
        }
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            )
            .ok()?;
            if read == 0 {
                return Some(());
            }
            if !piece(&buffer[..read as usize]) {
                return None;
            }
        }
    }
}

/// The SHA-256 of everything `file` holds from where it is read.
pub(crate) fn sha256(file: &mut impl Read) -> io::Result<[u8; 32]> {
    /// A hash object, destroyed on drop.
    struct Hash(BCRYPT_HASH_HANDLE);
    impl Drop for Hash {
        fn drop(&mut self) {
            // SAFETY: the handle is this value's own.
            let _ = unsafe { BCryptDestroyHash(self.0) };
        }
    }
    let mut handle = BCRYPT_HASH_HANDLE::default();
    // SAFETY: the pseudo-handle names SHA-256 on every Windows that Gozler
    // runs on; with no buffer, Windows keeps the object's memory itself.
    unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut handle, None, None, 0) }
        .ok()
        .map_err(io::Error::other)?;
    let hash = Hash(handle);
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        // SAFETY: the handle is open and the slice is valid for the call.
        unsafe { BCryptHashData(hash.0, &buffer[..read], 0) }
            .ok()
            .map_err(io::Error::other)?;
    }
    let mut digest = [0u8; 32];
    // SAFETY: SHA-256's digest is 32 bytes, the output's length.
    unsafe { BCryptFinishHash(hash.0, &mut digest, 0) }
        .ok()
        .map_err(io::Error::other)?;
    Ok(digest)
}
