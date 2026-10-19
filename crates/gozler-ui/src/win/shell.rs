//! COM, the known folders and the shell's open and run-as-administrator
//! verbs.

use super::Hwnd;
use std::path::PathBuf;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};

/// Starts COM on this thread in the apartment model, which the shell's
/// dialogs and `ShellExecuteW` need.
pub(crate) fn initialize_com() {
    // SAFETY: called once, on the UI thread, before any COM or shell call.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
}

/// The user's local application data folder, such as
/// `C:\Users\ana\AppData\Local`.
pub(crate) fn local_app_data() -> windows::core::Result<PathBuf> {
    // SAFETY: the returned string is copied, then freed as the call requires.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        Ok(PathBuf::from(
            text.map_err(|_| windows::core::Error::from_thread())?,
        ))
    }
}

/// Opens `target` as a double-click would: an address in the default
/// browser, or a console such as `services.msc`.
pub(crate) fn open(target: &str) {
    // SAFETY: both strings outlive the call.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Runs `program` with `parameters` as administrator, without a console
/// window, after Windows asks the person for permission. The window never
/// sees or passes credentials (docs/plans/ui-design.md: Feedback and fixes).
pub(crate) fn run_as_administrator(owner: Hwnd, program: &str, parameters: &str) {
    // SAFETY: every string outlives the call; `owner` only owns the prompt.
    unsafe {
        ShellExecuteW(
            Some(owner.0),
            w!("runas"),
            &HSTRING::from(program),
            &HSTRING::from(parameters),
            PCWSTR::null(),
            SW_HIDE,
        );
    }
}
