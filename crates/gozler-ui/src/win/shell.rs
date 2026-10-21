//! COM and OLE, the known folders, the save dialog, the folder and program
//! pickers, the shell's verbs: open, show in File Explorer, Properties and
//! run as administrator, dragging entries out, and what Settings > General
//! keeps in the registry.

use super::Hwnd;
use crate::core::selection::wide_path;
use gozler_core::proto::ResultItem;
use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{
    CloseHandle, E_POINTER, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, FILETIME, HANDLE,
    HWND, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, GetVolumeInformationW, WIN32_FIND_DATAW,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CreateBindCtx, IBindCtx, IDataObject,
};
use windows::Win32::System::Ole::{
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, IDropSource, OleInitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY,
    RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, FOS_NOTESTFILECREATE, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS,
    FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, IFileSystemBindData,
    IFileSystemBindData_Impl, ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILIsParent,
    ILRemoveLastID, IShellItem, KF_FLAG_DEFAULT, KF_FLAG_DONT_VERIFY, KNOWN_FOLDER_FLAG,
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHCreateDataObject, SHCreateItemFromParsingName,
    SHDoDragDrop, SHELLEXECUTEINFOW, SHGetKnownFolderPath, SHOP_FILEPATH, SHObjectProperties,
    SHOpenFolderAndSelectItems, SHParseDisplayName, SIGDN_FILESYSPATH, STR_FILE_SYS_BIND_DATA,
    ShellExecuteExW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWNORMAL};
use windows::core::{HSTRING, IUnknown, Interface, PCWSTR, w};

/// Starts COM on this thread in the apartment model, which the shell's
/// dialogs and `ShellExecuteW` need.
pub(crate) fn initialize_com() {
    // SAFETY: called once on each thread that makes COM or shell calls,
    // before the first.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
}

/// Starts OLE on the window's thread, which a drag needs; the thread's COM
/// apartment stays as [`initialize_com`] made it.
pub(crate) fn initialize_ole() {
    // SAFETY: called once on the window's thread, after `initialize_com`.
    let _ = unsafe { OleInitialize(None) };
}

/// One entry as the index holds it, which the shell reads through
/// `STR_FILE_SYS_BIND_DATA` instead of reading the entry from the disk.
#[windows::core::implement(IFileSystemBindData)]
struct FindData(Cell<WIN32_FIND_DATAW>);

impl IFileSystemBindData_Impl for FindData_Impl {
    fn SetFindData(&self, data: *const WIN32_FIND_DATAW) -> windows::core::Result<()> {
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: the caller passes one find data, checked not to be null.
        self.0.set(unsafe { *data });
        Ok(())
    }

    fn GetFindData(&self, data: *mut WIN32_FIND_DATAW) -> windows::core::Result<()> {
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: the caller passes a buffer for one find data, checked not
        // to be null.
        unsafe { *data = self.0.get() };
        Ok(())
    }
}

/// What `FindFirstFileW` would report for `file`, from what its row holds;
/// a size or a date the row lacks is 0.
fn find_data(file: &ResultItem, path: &[u16]) -> WIN32_FIND_DATAW {
    let mut attributes = file.attributes.map_or(0, |a| a.bits());
    if file.is_dir {
        attributes |= FILE_ATTRIBUTE_DIRECTORY.0;
    }
    if attributes == 0 {
        attributes = FILE_ATTRIBUTE_NORMAL.0;
    }
    // A folder's size in the row is the sum of its files; Windows gives 0.
    let size = if file.is_dir {
        0
    } else {
        file.size.unwrap_or(0)
    };
    let modified = file.mtime_ft.unwrap_or(0) as u64;
    let mut data = WIN32_FIND_DATAW {
        dwFileAttributes: attributes,
        ftLastWriteTime: FILETIME {
            dwLowDateTime: modified as u32,
            dwHighDateTime: (modified >> 32) as u32,
        },
        nFileSizeHigh: (size >> 32) as u32,
        nFileSizeLow: size as u32,
        ..Default::default()
    };
    let last = path.iter().rposition(|&u| u == u16::from(b'\\'));
    let name = &path[last.map_or(0, |i| i + 1)..];
    // The last unit stays the NUL; an NTFS or ReFS name always fits.
    let length = name.len().min(data.cFileName.len() - 1);
    data.cFileName[..length].copy_from_slice(&name[..length]);
    data
}

/// Drags `files` as File Explorer drags its selection: the shell's own data
/// object, so a drop target gets the files and the shell's item list, with
/// Explorer's drag image, and the target copies, moves or links them.
/// Returns when the drop ends, with the first path that the shell could not
/// name, such as one on a drive that is gone; the others are dragged.
///
/// Each path is named from what its row holds, without reading the disk, so
/// an entry deleted since the search is dragged too, and the drop target
/// reports it, as it does for any stale item.
pub(crate) fn drag(files: &[ResultItem]) -> Option<Vec<u16>> {
    let found: IFileSystemBindData = FindData(Cell::new(WIN32_FIND_DATAW::default())).into();
    // SAFETY: plain COM calls on the window's thread, whose apartment
    // `initialize_com` started; the context holds its own reference.
    let context = unsafe {
        CreateBindCtx(0).and_then(|context| {
            context.RegisterObjectParam(STR_FILE_SYS_BIND_DATA, &found.cast::<IUnknown>()?)?;
            Ok(context)
        })
    };
    let Ok(context) = context else {
        return None;
    };
    let mut missing = None;
    let mut lists = Vec::with_capacity(files.len());
    for file in files {
        let path = wide_path(file);
        let mut list = std::ptr::null_mut();
        // SAFETY: the find data and the path outlive the calls; the second
        // writes `list`.
        let named = unsafe {
            found.SetFindData(&find_data(file, &path)).and_then(|()| {
                SHParseDisplayName(&HSTRING::from_wide(&path), &context, &mut list, 0, None)
            })
        };
        if named.is_ok() && !list.is_null() {
            lists.push(list);
        } else {
            missing.get_or_insert(path);
        }
    }
    let Some(&first) = lists.first() else {
        return missing;
    };
    // SAFETY: every list is this function's own until it is freed below, and
    // the last item of a list names its entry inside its folder.
    unsafe {
        let parent = ILClone(first);
        let _ = ILRemoveLastID(Some(parent));
        let shared = lists
            .iter()
            .all(|list| ILIsParent(parent, *list, true).as_bool());
        let data: windows::core::Result<IDataObject> = if shared {
            let children: Vec<*const ITEMIDLIST> = lists
                .iter()
                .map(|list| ILFindLastID(*list).cast_const())
                .collect();
            SHCreateDataObject(
                Some(parent),
                Some(children.as_slice()),
                None::<&IDataObject>,
            )
        } else {
            // Each list names its entry from the desktop.
            let absolute: Vec<*const ITEMIDLIST> =
                lists.iter().map(|list| list.cast_const()).collect();
            SHCreateDataObject(None, Some(absolute.as_slice()), None::<&IDataObject>)
        };
        if let Ok(data) = data {
            // A move needs nothing here: the index follows the journal.
            let _ = SHDoDragDrop(
                None,
                &data,
                None::<&IDropSource>,
                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
            );
        }
        ILFree(Some(parent));
        for list in lists {
            ILFree(Some(list));
        }
    }
    missing
}

/// The user's local application data folder, such as
/// `C:\Users\ana\AppData\Local`.
pub(crate) fn local_app_data() -> windows::core::Result<PathBuf> {
    known_folder(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT).map(PathBuf::from)
}

/// The user's Documents folder, which the left panel shows the first time
/// (DESIGN.md: A panel). It is not checked, because a folder redirected to a
/// server that does not answer would hold the window; the panel's reading finds
/// out.
pub(crate) fn documents() -> windows::core::Result<String> {
    known_folder(&FOLDERID_Documents, KF_FLAG_DONT_VERIFY)
}

/// The user's Downloads folder, which the right panel shows the first time,
/// not checked, as [`documents`].
pub(crate) fn downloads() -> windows::core::Result<String> {
    known_folder(&FOLDERID_Downloads, KF_FLAG_DONT_VERIFY)
}

fn known_folder(
    id: &windows::core::GUID,
    flags: KNOWN_FOLDER_FLAG,
) -> windows::core::Result<String> {
    // SAFETY: the returned string is copied, then freed as the call requires.
    unsafe {
        let path = SHGetKnownFolderPath(id, flags, None)?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        text.map_err(|_| windows::core::Error::from_thread())
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

/// Opens the folder `folder`, UTF-16 without a NUL, in File Explorer,
/// whatever program opens folders on this computer.
pub(crate) fn explore(owner: Hwnd, folder: &[u16]) {
    // A root's last backslash is doubled, so it cannot escape the closing
    // quote.
    let quote = u16::from(b'"');
    let mut parameters = vec![quote];
    parameters.extend_from_slice(folder);
    if folder.last() == Some(&u16::from(b'\\')) {
        parameters.push(u16::from(b'\\'));
    }
    parameters.push(quote);
    // SAFETY: the parameters outlive the call; a null verb is the default
    // one.
    unsafe {
        ShellExecuteW(
            Some(owner.0),
            PCWSTR::null(),
            w!("explorer.exe"),
            &HSTRING::from_wide(&parameters),
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
/// holds no borrow across it (DESIGN.md: window rule 1).
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
/// across it (DESIGN.md: window rule 1).
pub(crate) fn pick_folder(owner: Hwnd, start: Option<&str>) -> Option<String> {
    pick_folder_for(owner, start, None)
}

/// [`pick_folder`] for a destination, with the title `title` and `ok` as
/// its OK button's caption, such as "Copy to folder" and "Copy"
/// (DESIGN.md: File operations).
pub(crate) fn pick_destination(
    owner: Hwnd,
    start: Option<&str>,
    title: &str,
    ok: &str,
) -> Option<String> {
    pick_folder_for(owner, start, Some((title, ok)))
}

fn pick_folder_for(
    owner: Hwnd,
    start: Option<&str>,
    labels: Option<(&str, &str)>,
) -> Option<String> {
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
        if let Some((title, ok)) = labels {
            let _ = dialog.SetTitle(&HSTRING::from(title));
            let _ = dialog.SetOkButtonLabel(&HSTRING::from(ok));
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
/// borrow across it (DESIGN.md: window rule 1).
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

/// Runs `program` with `parameters`, an open command of Settings > Context
/// menu (DESIGN.md: The Context menu page). Windows shows no message of its own; a
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
/// The name of Gozler's values.
const GOZLER: &str = "gozler";
/// The keys of File Explorer's "Search with Gozler" on a folder and on a
/// folder's background, which earlier versions added.
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
    set_string_in(HKEY_CURRENT_USER, key, name, value)
}

/// Writes the string `value` as the value `name` of the machine's key `key`,
/// which it creates when it is missing. Only an administrator can.
pub(crate) fn set_machine_string(key: &str, name: &str, value: &str) -> io::Result<()> {
    set_string_in(HKEY_LOCAL_MACHINE, key, Some(name), value)
}

fn set_string_in(root: HKEY, key: &str, name: Option<&str>, value: &str) -> io::Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the names and the data outlive the call, and the length is
    // the data's, with its NUL, in bytes.
    registry(unsafe {
        RegSetKeyValueW(
            root,
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
/// `StartupApproved` value is off (DESIGN.md: The General page).
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

/// Removes "Search with Gozler" from File Explorer's menus, which an
/// earlier version added, so no item that Gozler no longer serves stays
/// there (DESIGN.md: The Context menu page). Keys that are gone stay gone.
pub(crate) fn remove_search_with() {
    for key in VERB_KEYS {
        // SAFETY: the name outlives the call.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    }
}

/// The window that owns Windows' permission prompt, which another thread
/// passes to [`start_as_administrator`].
#[derive(Clone, Copy)]
pub(crate) struct PromptOwner(isize);

// SAFETY: ShellExecuteExW only reads the handle to place its prompt, and
// the window joins the thread that holds it in WM_DESTROY, while the handle
// is valid.
unsafe impl Send for PromptOwner {}

impl PromptOwner {
    pub(crate) fn new(window: Hwnd) -> PromptOwner {
        PromptOwner(window.0.0 as isize)
    }
}

/// A program that runs as administrator. Dropping it closes the handle,
/// not the program.
pub(crate) struct ElevatedProcess(HANDLE);

// SAFETY: any thread may wait on and close a process handle.
unsafe impl Send for ElevatedProcess {}

impl ElevatedProcess {
    /// Waits up to `timeout_ms` for the program to end, and gives its exit
    /// code; `None` while it still runs.
    pub(crate) fn wait(&self, timeout_ms: u32) -> io::Result<Option<u32>> {
        // SAFETY: the handle is open until `drop`.
        unsafe {
            match WaitForSingleObject(self.0, timeout_ms) {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    GetExitCodeProcess(self.0, &mut code).map_err(io_error)?;
                    Ok(Some(code))
                }
                WAIT_TIMEOUT => Ok(None),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
}

impl Drop for ElevatedProcess {
    fn drop(&mut self) {
        // SAFETY: ShellExecuteExW opened the handle, which this closes once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Starts `program` with `parameters` as administrator, without a console
/// window, after Windows asks the person for permission; the call returns
/// once the person answers. A person who says no gives the error
/// `ERROR_CANCELLED`. The window never sees or passes credentials
/// (DESIGN.md: Daemon failures).
pub(crate) fn start_as_administrator(
    owner: PromptOwner,
    program: &str,
    parameters: &str,
) -> io::Result<ElevatedProcess> {
    let process = execute(
        HWND(owner.0 as *mut _),
        w!("runas"),
        program,
        parameters,
        SEE_MASK_NOCLOSEPROCESS,
        SW_HIDE,
    )?;
    // An executable always starts a process, so SEE_MASK_NOCLOSEPROCESS
    // gives its handle.
    Ok(ElevatedProcess(process))
}

/// Starts `program`, as a double-click on it does.
pub(crate) fn start_program(owner: Hwnd, program: &Path) -> io::Result<()> {
    let program = program.display().to_string();
    execute(owner.0, w!("open"), &program, "", 0, SW_SHOWNORMAL).map(|_| ())
}

/// Whether `error` says that the person said no to Windows' permission
/// prompt.
pub(crate) fn refused(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_CANCELLED.0 as i32)
}

/// `ShellExecuteExW` with `verb`, without Windows' own error messages,
/// because the caller says what failed. Gives the process handle that
/// `SEE_MASK_NOCLOSEPROCESS` in `mask` asks for.
fn execute(
    owner: HWND,
    verb: PCWSTR,
    program: &str,
    parameters: &str,
    mask: u32,
    show: SHOW_WINDOW_CMD,
) -> io::Result<HANDLE> {
    let program = HSTRING::from(program);
    let parameters = HSTRING::from(parameters);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: mask | SEE_MASK_FLAG_NO_UI,
        hwnd: owner,
        lpVerb: verb,
        lpFile: PCWSTR(program.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: show.0,
        ..Default::default()
    };
    // SAFETY: `info` is complete, with its size set, and its strings outlive
    // the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(io_error)?;
    Ok(info.hProcess)
}

/// How long [`run_as_administrator_and_wait`] waits for the program to end.
const ELEVATED_WAIT_MS: u32 = 30_000;

/// Runs `program` with `parameters` as administrator, as
/// [`start_as_administrator`] does, waits for it to end, and gives its exit
/// code. This thread waits for the person's answer and then for the
/// program, at most [`ELEVATED_WAIT_MS`], so it is not the window's.
pub(crate) fn run_as_administrator_and_wait(
    owner: PromptOwner,
    program: &str,
    parameters: &str,
) -> io::Result<u32> {
    start_as_administrator(owner, program, parameters)?
        .wait(ELEVATED_WAIT_MS)?
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drop target copies what the find data says: a folder without
    /// the folder attribute would be copied as a file, and a folder's
    /// summed size would be taken for its own.
    #[test]
    fn find_data_names_a_folder_as_a_folder_and_a_file_with_its_row() {
        let row = |path: &str, is_dir| ResultItem {
            path: path.to_owned(),
            path_u16: None,
            size: Some(0x1_0000_0002),
            mtime_ft: Some(0x0123_4567_89AB_CDEF),
            created_ft: None,
            is_dir,
            attributes: None,
        };
        let data = |file: &ResultItem| find_data(file, &wide_path(file));
        let folder = data(&row(r"C:\docs", true));
        assert_eq!(folder.dwFileAttributes, FILE_ATTRIBUTE_DIRECTORY.0);
        assert_eq!((folder.nFileSizeHigh, folder.nFileSizeLow), (0, 0));
        let file = data(&row(r"C:\docs\Ünï.txt", false));
        assert_eq!(file.dwFileAttributes, FILE_ATTRIBUTE_NORMAL.0);
        assert_eq!((file.nFileSizeHigh, file.nFileSizeLow), (1, 2));
        assert_eq!(
            (
                file.ftLastWriteTime.dwHighDateTime,
                file.ftLastWriteTime.dwLowDateTime
            ),
            (0x0123_4567, 0x89AB_CDEF)
        );
        let name: Vec<u16> = "Ünï.txt\0".encode_utf16().collect();
        assert_eq!(file.cFileName[..name.len()], name[..]);
    }
}
