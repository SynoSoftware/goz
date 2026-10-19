//! One window per user session: a named mutex decides which process is
//! first, and a second start hands over to the first window (spec: Window
//! lifetime).

use super::Hwnd;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, RegisterWindowMessageW,
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

/// Lets the first window of class `class` come to the front, and posts it
/// `message`. Does nothing while the first process has no window yet,
/// because it shows its window by itself.
pub(crate) fn hand_over(class: &str, message: u32) {
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
    Hwnd(hwnd).post(message);
}
