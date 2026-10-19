//! One window per user session: a named mutex decides which process is
//! first, and a second start hands over to the first window (spec: Window
//! lifetime).

use super::Hwnd;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, RegisterWindowMessageW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::{HSTRING, PCWSTR};

/// The mutex that the first process holds until it exits.
pub(crate) struct FirstInstance(HANDLE);

impl Drop for FirstInstance {
    fn drop(&mut self) {
        // SAFETY: the handle is this value's own.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The mutex `name` if this process is the first to create it; `None` when
/// another process holds it.
pub(crate) fn claim(name: &str) -> windows::core::Result<Option<FirstInstance>> {
    // SAFETY: a named mutex with default security, not owned at creation.
    let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }?;
    // SAFETY: no parameters; it reads the result of the call above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: the handle is this function's own.
        let _ = unsafe { CloseHandle(handle) };
        return Ok(None);
    }
    Ok(Some(FirstInstance(handle)))
}

/// The message ID that `name` stands for in every process of the session.
pub(crate) fn register_message(name: &str) -> u32 {
    // SAFETY: the name outlives the call.
    unsafe { RegisterWindowMessageW(&HSTRING::from(name)) }
}

/// How long a second start waits for the first window to take a folder.
const HAND_OVER_TIMEOUT_MS: u32 = 5_000;

/// Lets the first window of class `class` come to the front, and posts it
/// `message`; with a `folder`, from `--location`, it sends `message`'s ID
/// and the folder with `WM_COPYDATA` instead. Does nothing while the first
/// process has no window yet, because it shows its window by itself.
pub(crate) fn hand_over(class: &str, message: u32, folder: Option<&str>) {
    // SAFETY: the class name outlives the call.
    let Ok(hwnd) = (unsafe { FindWindowW(&HSTRING::from(class), PCWSTR::null()) }) else {
        return;
    };
    let mut process = 0;
    // SAFETY: `process` is a local the call fills in.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
    // SAFETY: plain values; Windows refuses it when this process may not
    // give the foreground away, and the first window then flashes instead.
    let _ = unsafe { AllowSetForegroundWindow(process) };
    let Some(folder) = folder else {
        Hwnd(hwnd).post(message);
        return;
    };
    let text: Vec<u16> = folder.encode_utf16().collect();
    let data = COPYDATASTRUCT {
        dwData: message as usize,
        cbData: (text.len() * 2) as u32,
        lpData: text.as_ptr().cast_mut().cast(),
    };
    // SAFETY: `data` and its text outlive the call, which Windows copies
    // into the first process; a window that does not answer in time is
    // left alone.
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM((&raw const data) as isize),
            SMTO_ABORTIFHUNG,
            HAND_OVER_TIMEOUT_MS,
            None,
        )
    };
}
