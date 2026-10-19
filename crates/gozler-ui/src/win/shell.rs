//! COM, the known folders, the save dialog, the folder and program
//! pickers, the shell's verbs: open, show in File Explorer, Properties and
//! run as administrator, and what Settings > General keeps in the registry.

use super::Hwnd;
use std::io;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, IBindCtx,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOTESTFILECREATE,
    FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog,
    IFileSaveDialog, ILCreateFromPathW, ILFindLastID, ILFree, IShellItem, KF_FLAG_DEFAULT,
    SEE_MASK_FLAG_NO_UI, SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SHGetKnownFolderPath,
    SHOP_FILEPATH, SHObjectProperties, SHOpenFolderAndSelectItems, SIGDN_FILESYSPATH,
    ShellExecuteExW, ShellExecuteW,
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

/// Opens the file or folder `path`, UTF-16 without a NUL, with its default
/// command, as File Explorer's double-click does.
pub(crate) fn open_entry(owner: Hwnd, path: &[u16]) {
    // SAFETY: the path outlives the call; a null verb is the default one.
    unsafe {
        ShellExecuteW(
            Some(owner.0),
            PCWSTR::null(),
            &HSTRING::from_wide(path),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Opens the folder `folder` in File Explorer with `entries`, which are in
/// it, selected. Each path is UTF-16 without a NUL.
pub(crate) fn show_in_folder(folder: &[u16], entries: &[Vec<u16>]) {
    let item_list = |path: &[u16]| {
        // SAFETY: the path outlives the call; null is a path the shell
        // cannot name.
        let list = unsafe { ILCreateFromPathW(&HSTRING::from_wide(path)) };
        (!list.is_null()).then_some(list)
    };
    let Some(folder) = item_list(folder) else {
        return;
    };
    let entries: Vec<*mut ITEMIDLIST> = entries.iter().filter_map(|e| item_list(e)).collect();
    // SAFETY: each list is this function's own until it is freed below, and
    // the last item of an entry's list names it inside its folder.
    unsafe {
        let children: Vec<*const ITEMIDLIST> = entries
            .iter()
            .map(|list| ILFindLastID(*list).cast_const())
            .collect();
        let _ = SHOpenFolderAndSelectItems(folder, Some(children.as_slice()), 0);
        for list in entries {
            ILFree(Some(list));
        }
        ILFree(Some(folder));
    }
}

/// Shows the shell's Properties dialog of `path`, UTF-16 without a NUL.
/// The dialog runs in its own window, so the call returns at once.
pub(crate) fn properties(owner: Hwnd, path: &[u16]) {
    // SAFETY: the path outlives the call; no page is named.
    unsafe {
        let _ = SHObjectProperties(
            Some(owner.0),
            SHOP_FILEPATH,
            &HSTRING::from_wide(path),
            PCWSTR::null(),
        );
    }
}

/// One file type of the save dialog: its name and its extension.
pub(crate) struct FileType<'a> {
    pub(crate) name: &'a str,
    pub(crate) extension: &'a str,
}

/// Shows the Windows save dialog, which asks before it replaces a file, and
/// returns the chosen file and the index of the type it showed, or `None`
/// when the person cancels. It runs its own message loop, so the caller
/// holds no borrow across it (design rule 1).
pub(crate) fn save_dialog(
    owner: Hwnd,
    title: &str,
    file_name: &str,
    types: &[FileType],
) -> Option<(PathBuf, usize)> {
    let names: Vec<HSTRING> = types.iter().map(|t| HSTRING::from(t.name)).collect();
    let specs: Vec<HSTRING> = types
        .iter()
        .map(|t| HSTRING::from(format!("*.{}", t.extension)))
        .collect();
    let filters: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .zip(&specs)
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    // SAFETY: every string outlives the dialog, which copies them; the
    // chosen path is copied, then freed as the call requires.
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        // The export creates the file itself, and only a file on disk can
        // be written.
        dialog
            .SetOptions(options | FOS_OVERWRITEPROMPT | FOS_NOTESTFILECREATE | FOS_FORCEFILESYSTEM)
            .ok()?;
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetFileTypeIndex(1).ok()?;
        if let Some(first) = types.first() {
            dialog
                .SetDefaultExtension(&HSTRING::from(first.extension))
                .ok()?;
        }
        dialog.SetTitle(&HSTRING::from(title)).ok()?;
        dialog.SetFileName(&HSTRING::from(file_name)).ok()?;
        dialog.Show(Some(owner.0)).ok()?;
        let index = dialog.GetFileTypeIndex().ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        let index = usize::try_from(index).ok()?.checked_sub(1)?;
        Some((PathBuf::from(text.ok()?), index))
    }
}

/// Shows the Windows folder picker, open at `start` when that folder
/// exists, and returns the chosen folder, or `None` when the person
/// cancels. It runs its own message loop, so the caller holds no borrow
/// across it (design rule 1).
pub(crate) fn pick_folder(owner: Hwnd, start: Option<&str>) -> Option<String> {
    // SAFETY: every string outlives the call that reads it; the chosen
    // path is copied, then freed as the call requires.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        // Only a folder on disk can be searched.
        dialog
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
            .ok()?;
        if let Some(start) = start
            && let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(start),
                None::<&IBindCtx>,
            )
        {
            let _ = dialog.SetFolder(&folder);
        }
        dialog.Show(Some(owner.0)).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        text.ok()
    }
}

/// Shows the Windows file dialog for programs, titled `title`, whose file
/// type is named `programs`, and returns the chosen program, or `None` when
/// the person cancels. It runs its own message loop, so the caller holds no
/// borrow across it (design rule 1).
pub(crate) fn pick_program(owner: Hwnd, title: &str, programs: &str) -> Option<String> {
    let name = HSTRING::from(format!("{programs} (*.exe)"));
    let filter = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(name.as_ptr()),
        pszSpec: w!("*.exe"),
    }];
    // SAFETY: every string outlives the dialog, which copies them; the
    // chosen path is copied, then freed as the call requires.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)
            .ok()?;
        dialog.SetFileTypes(&filter).ok()?;
        dialog.SetTitle(&HSTRING::from(title)).ok()?;
        dialog.Show(Some(owner.0)).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        text.ok()
    }
}

/// Runs `program` with `parameters`, an open command of Settings > General
/// (spec: The General page). Windows shows no message of its own; a
/// program that cannot start gives Windows' reason.
pub(crate) fn run_command(owner: Hwnd, program: &str, parameters: &str) -> io::Result<()> {
    let program = HSTRING::from(program);
    let parameters = HSTRING::from(parameters);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        hwnd: owner.0,
        lpFile: PCWSTR(program.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is complete, with its size set, and its strings outlive
    // the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(io_error)
}

/// The label of the volume whose root is `root`, such as "Windows" for
/// `C:\`, or `None` when Windows cannot read it, as for an offline drive.
pub(crate) fn volume_label(root: &str) -> Option<String> {
    let mut label = [0u16; 261];
    // SAFETY: the root outlives the call, which writes at most the
    // buffer's length.
    unsafe {
        GetVolumeInformationW(
            &HSTRING::from(root),
            Some(&mut label),
            None,
            None,
            None,
            None,
        )
    }
    .ok()?;
    let length = label.iter().position(|&u| u == 0).unwrap_or(label.len());
    Some(String::from_utf16_lossy(&label[..length]))
}

/// An operating-system error as Rust's, so [`super::error_text`] gives
/// Windows' words for it.
fn io_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    // An HRESULT made from a Win32 error code holds the code in its low word.
    if code & 0xffff_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xffff) as i32)
    } else {
        io::Error::other(error.message())
    }
}

/// A registry call's result, where a missing key or value is no error,
/// because deleting one that is gone leaves what was asked.
fn registry(result: WIN32_ERROR) -> io::Result<()> {
    match result {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        error => Err(io::Error::from_raw_os_error(error.0 as i32)),
    }
}

/// The current user's key that starts programs at sign-in.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Windows Settings record that they turned a `Run`
/// entry off.
const STARTUP_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The name of Gozler's values and verbs.
const GOZLER: &str = "gozler";
/// The keys of File Explorer's "Search with Gozler" on a folder and on a
/// folder's background.
const VERB_KEYS: [&str; 2] = [
    r"Software\Classes\Directory\shell\gozler",
    r"Software\Classes\Directory\Background\shell\gozler",
];

/// A value's name, where `None` is the key's default value.
fn value_name(name: Option<&str>) -> HSTRING {
    HSTRING::from(name.unwrap_or_default())
}

/// Writes the string `value` as the value `name` of the current user's key
/// `key`, which it creates when it is missing.
fn set_string(key: &str, name: Option<&str>, value: &str) -> io::Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the names and the data outlive the call, and the length is
    // the data's, with its NUL, in bytes.
    registry(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &value_name(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    })
}

/// Reads the value `name` of the current user's key `key` into `data`, of
/// the type `kind`, and returns its length in bytes; with no `data`, it
/// only asks whether the value exists.
fn get_value(
    key: &str,
    name: Option<&str>,
    kind: REG_ROUTINE_FLAGS,
    data: Option<&mut [u8]>,
) -> Option<u32> {
    let mut length = data.as_ref().map_or(0, |d| d.len() as u32);
    let buffer = data.map(|d| d.as_mut_ptr().cast());
    // SAFETY: the names outlive the call, which writes at most `length`
    // bytes into the buffer.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &value_name(name),
            kind,
            None,
            buffer,
            buffer.map(|_| &raw mut length),
        )
    };
    (result == ERROR_SUCCESS).then_some(length)
}

fn delete_value(key: &str, name: &str) -> io::Result<()> {
    // SAFETY: the names outlive the call.
    registry(unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name))
    })
}

/// Whether Windows starts Gozler at sign-in: the `Run` value exists, and
/// Task Manager did not turn it off. An odd first byte of the
/// `StartupApproved` value is off (design: Start at sign-in).
pub(crate) fn starts_at_sign_in() -> bool {
    if get_value(RUN_KEY, Some(GOZLER), RRF_RT_REG_SZ, None).is_none() {
        return false;
    }
    let mut approved = [0u8; 12];
    match get_value(
        STARTUP_APPROVED_KEY,
        Some(GOZLER),
        RRF_RT_REG_BINARY,
        Some(&mut approved),
    ) {
        Some(length) if length > 0 => approved[0] % 2 == 0,
        Some(_) | None => true,
    }
}

/// Starts `program` at sign-in with `--sign-in`, or no longer does. Turning
/// it on also removes Task Manager's off mark, so the check box and Task
/// Manager agree.
pub(crate) fn set_start_at_sign_in(on: bool, program: &Path) -> io::Result<()> {
    if on {
        let command = format!("\"{}\" --sign-in", program.display());
        set_string(RUN_KEY, Some(GOZLER), &command)?;
    } else {
        delete_value(RUN_KEY, GOZLER)?;
    }
    delete_value(STARTUP_APPROVED_KEY, GOZLER)
}

/// Whether File Explorer shows "Search with Gozler" on folders.
pub(crate) fn shows_search_with() -> bool {
    VERB_KEYS
        .iter()
        .all(|key| get_value(&format!(r"{key}\command"), None, RRF_RT_REG_SZ, None).is_some())
}

/// Adds "Search with Gozler", labelled `label`, to File Explorer's menu
/// for folders and for a folder's background, which starts `program` with
/// `--location` and the folder; or removes it.
pub(crate) fn set_search_with(on: bool, program: &Path, label: &str) -> io::Result<()> {
    let program = program.display().to_string();
    for key in VERB_KEYS {
        if on {
            set_string(key, None, label)?;
            set_string(key, Some("Icon"), &program)?;
            set_string(
                &format!(r"{key}\command"),
                None,
                &format!("\"{program}\" --location \"%V\""),
            )?;
        } else {
            // SAFETY: the name outlives the call.
            registry(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) })?;
        }
    }
    Ok(())
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
