//! COM, the known folders, the save dialog, the folder picker and the
//! shell's verbs: open, show in File Explorer, Properties and run as
//! administrator.

use super::Hwnd;
use std::path::PathBuf;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, IBindCtx,
};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOS_FORCEFILESYSTEM, FOS_NOTESTFILECREATE, FOS_OVERWRITEPROMPT,
    FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog,
    ILCreateFromPathW, ILFindLastID, ILFree, IShellItem, KF_FLAG_DEFAULT,
    SHCreateItemFromParsingName, SHGetKnownFolderPath, SHOP_FILEPATH, SHObjectProperties,
    SHOpenFolderAndSelectItems, SIGDN_FILESYSPATH, ShellExecuteW,
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
