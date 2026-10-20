//! One window per user session: a named mutex decides which process is
//! first, and a second start hands over to the first window or asks it to
//! exit (DESIGN.md: Window lifetime).

use super::Hwnd;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::{
    CreateMutexW, GetProcessTimes, INFINITE, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, RegisterWindowMessageW,
    SMTO_ABORTIFHUNG, SMTO_NOTIMEOUTIFNOTHUNG, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

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

/// The first window of class `class`; `None` while the first process has
/// no window, as while it starts or ends.
pub(crate) fn find(class: &str) -> Option<Hwnd> {
    // SAFETY: the class name outlives the call.
    unsafe { FindWindowW(&HSTRING::from(class), PCWSTR::null()) }
        .ok()
        .map(Hwnd)
}

/// The program of a running window: its file, and when its process started.
pub(crate) struct Program {
    pub(crate) path: PathBuf,
    pub(crate) started: SystemTime,
}

/// The process of the first window, held open so this start can wait for it
/// to end after it agreed to exit.
pub(crate) struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is this value's own.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

impl Process {
    /// `window`'s process; `None` when Windows refuses it, as for a process
    /// this one may not query.
    pub(crate) fn of(window: Hwnd) -> Option<Process> {
        // SAFETY: plain values; the rights read only the name and the times,
        // and wait for the end.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                process_id(window),
            )
        };
        handle.ok().map(Process)
    }

    /// The process's program; `None` when Windows does not say.
    pub(crate) fn program(&self) -> Option<Program> {
        let mut name = vec![0u16; 32_768];
        let mut length = name.len() as u32;
        // SAFETY: `name` holds `length` units, which the call lowers to the
        // name's length.
        unsafe {
            QueryFullProcessImageNameW(
                self.0,
                PROCESS_NAME_WIN32,
                PWSTR(name.as_mut_ptr()),
                &mut length,
            )
        }
        .ok()?;
        let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
        // SAFETY: the four times are locals the call fills in.
        unsafe { GetProcessTimes(self.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .ok()?;
        Some(Program {
            path: PathBuf::from(std::ffi::OsString::from_wide(&name[..length as usize])),
            started: system_time(created)?,
        })
    }

    /// Waits, without a limit, until the process has ended.
    pub(crate) fn wait_for_end(&self) {
        // SAFETY: the handle is this value's own and has the right to wait.
        let _ = unsafe { WaitForSingleObject(self.0, INFINITE) };
    }
}

/// A `FILETIME`, in 100 ns units since 1601, as a [`SystemTime`].
fn system_time(time: FILETIME) -> Option<SystemTime> {
    const UNIX_EPOCH_IN_FILETIME: u64 = 116_444_736_000_000_000;
    let units = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let since_unix = units.checked_sub(UNIX_EPOCH_IN_FILETIME)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(since_unix.checked_mul(100)?))
}

fn process_id(window: Hwnd) -> u32 {
    let mut process = 0;
    // SAFETY: `process` is a local the call fills in.
    unsafe { GetWindowThreadProcessId(window.0, Some(&mut process)) };
    process
}

/// Lets `window`'s process come to the front. Windows refuses it when this
/// process may not give the foreground away, and that window then flashes
/// on the taskbar instead.
fn allow_foreground(window: Hwnd) {
    // SAFETY: plain values.
    let _ = unsafe { AllowSetForegroundWindow(process_id(window)) };
}

/// How long a second start waits for the first window to take a folder.
const HAND_OVER_TIMEOUT_MS: u32 = 5_000;

/// Lets the first window come to the front, and posts it `message`; with a
/// `folder`, from `--location`, it sends `message`'s ID and the folder with
/// `WM_COPYDATA` instead.
pub(crate) fn hand_over(window: Hwnd, message: u32, folder: Option<&str>) {
    allow_foreground(window);
    let Some(folder) = folder else {
        window.post(message);
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
            window.0,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM((&raw const data) as isize),
            SMTO_ABORTIFHUNG,
            HAND_OVER_TIMEOUT_MS,
            None,
        )
    };
}

/// Lets the first window come to the front, sends it `message` and returns
/// its answer, waiting as long as the window answers other messages, so
/// the person can take their time over a question it asks first. `None`
/// when the window does not respond.
pub(crate) fn ask(window: Hwnd, message: u32) -> Option<isize> {
    allow_foreground(window);
    let mut answer: usize = 0;
    // SAFETY: plain values; `answer` is a local the call fills in.
    let sent = unsafe {
        SendMessageTimeoutW(
            window.0,
            message,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_NOTIMEOUTIFNOTHUNG,
            HAND_OVER_TIMEOUT_MS,
            Some(&raw mut answer),
        )
    };
    (sent.0 != 0).then_some(answer as isize)
}
