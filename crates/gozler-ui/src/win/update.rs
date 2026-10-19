//! The one network request Gozler makes: GitHub's latest release, for Check
//! for updates (DESIGN.md: The General page).

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
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

/// The body of an HTTPS GET of `path` on `host`, when it answers 200 OK;
/// `None` for any failure, which Check for updates ignores.
pub(crate) fn get(host: &str, path: &str, agent: &str) -> Option<String> {
    // SAFETY: every string outlives the call that reads it; each handle is
    // used only while its owner lives, and the buffer is written only up
    // to its length.
    unsafe {
        let session = Internet::new(WinHttpOpen(
            &HSTRING::from(agent),
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
        let mut body = Vec::new();
        let mut buffer = vec![0u8; 16 * 1024];
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
                break;
            }
            body.extend_from_slice(&buffer[..read as usize]);
            if body.len() > MOST_BYTES {
                return None;
            }
        }
        String::from_utf8(body).ok()
    }
}
