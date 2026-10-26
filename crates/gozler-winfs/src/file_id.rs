//! Opening files and folders by file ID, listing a folder, and reading a
//! folder's security descriptor.
//!
//! Every open uses `ExtendedFileIdType`, the 128-bit ID: ReFS needs it, and
//! an NTFS ID zero-extended to 128 bits opens the same file. The listing
//! returns raw `FILE_ID_EXTD_DIR_INFO` bytes; `gozler-core` parses them.
//!
//! A descriptor is returned as the bytes of a self-relative security
//! descriptor with its owner, group, DACL and mandatory label: `AccessCheck`
//! needs the owner and the group, and the label carries the integrity level.
//! The SACL is not read, because it does not decide who may list a folder.

use core::mem::{size_of, zeroed};
use core::ptr;

use windows_sys::Wdk::System::SystemServices::RtlValidRelativeSecurityDescriptor;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_PARAMETER, ERROR_INVALID_SECURITY_DESCR, ERROR_NO_MORE_FILES, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetKernelObjectSecurity,
    GetSecurityDescriptorLength, LABEL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ExtendedFileIdType, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_128, FILE_ID_DESCRIPTOR, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdExtdDirectoryInfo, FileIdInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING, OpenFileById, READ_CONTROL,
};

use crate::error::{WinError, last_error};
use crate::ioctl::{FSCTL_READ_FILE_USN_DATA, ReadFileUsnData, ioctl_in_bytes};
use crate::util::to_wide_nul;
use crate::volume::VolumeHandle;

/// The largest descriptor read, so a bad answer cannot make the daemon
/// allocate without bound.
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;

/// The smallest self-relative descriptor: its fixed header.
const DESCRIPTOR_HEADER_BYTES: usize = 20;

/// The parts of a descriptor that decide who may list a folder.
const DESCRIPTOR_PARTS: u32 = OWNER_SECURITY_INFORMATION
    | GROUP_SECURITY_INFORMATION
    | DACL_SECURITY_INFORMATION
    | LABEL_SECURITY_INFORMATION;

/// An open file, folder, volume or event; `CloseHandle` on drop.
pub(crate) struct FileHandle(pub(crate) HANDLE);

impl FileHandle {
    pub(crate) fn as_handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful open and is owned by this
        // value; it is closed exactly once, here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Opens the file or folder `id` on `volume` with `access`, whatever its
/// permissions allow, because the daemon holds `SeBackupPrivilege`. A
/// symbolic link or a junction opens as itself, which is what the index
/// holds; without `FILE_FLAG_OPEN_REPARSE_POINT` its open by ID would fail
/// as a gone file's does.
///
/// `Ok(None)` when no file has the ID any more. NTFS answers
/// `ERROR_INVALID_PARAMETER`, whether the record is free or holds another
/// file; `ERROR_FILE_NOT_FOUND` says the same.
pub(crate) fn open_by_id(
    volume: &VolumeHandle,
    id: u128,
    access: u32,
) -> Result<Option<FileHandle>, WinError> {
    // SAFETY: `desc` is fully initialized before use, and OpenFileById takes
    // the live volume-hint handle.
    let handle = unsafe {
        let mut desc: FILE_ID_DESCRIPTOR = zeroed();
        desc.dwSize = size_of::<FILE_ID_DESCRIPTOR>() as u32;
        desc.Type = ExtendedFileIdType;
        desc.Anonymous.ExtendedFileId = FILE_ID_128 {
            Identifier: id.to_le_bytes(),
        };
        OpenFileById(
            volume.as_handle(),
            &desc,
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            // Backup semantics also open folders.
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = last_error("OpenFileById");
        return match error.code {
            ERROR_INVALID_PARAMETER | ERROR_FILE_NOT_FOUND => Ok(None),
            _ => Err(error),
        };
    }
    Ok(Some(FileHandle(handle)))
}

/// The file ID of the root folder of the volume at `guid_path`
/// (`\\?\Volume{GUID}\`, with its trailing backslash, as
/// [`VolumeInfo::guid_path`](crate::VolumeInfo::guid_path) holds it). NTFS
/// gives record 5; ReFS gives its own.
pub fn root_folder_id(guid_path: &str) -> Result<u128, WinError> {
    path_file_id(&to_wide_nul(guid_path))
}

/// The file ID of the file or folder at `path`, a NUL-terminated UTF-16 path.
pub(crate) fn path_file_id(path: &[u16]) -> Result<u128, WinError> {
    assert_eq!(path.last(), Some(&0), "path is NUL-terminated");
    // SAFETY: `path` is a NUL-terminated path; no security attributes and no
    // template handle are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (path_file_id)"));
    }
    let file = FileHandle(handle);
    // SAFETY: `info` is a plain struct the call fills, and its size is passed.
    let info = unsafe {
        let mut info: FILE_ID_INFO = zeroed();
        let ok = GetFileInformationByHandleEx(
            file.as_handle(),
            FileIdInfo,
            ptr::from_mut(&mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        );
        if ok == 0 {
            return Err(last_error("GetFileInformationByHandleEx (FileIdInfo)"));
        }
        info
    };
    Ok(u128::from_le_bytes(info.FileId.Identifier))
}

/// A folder opened for listing and, when that was granted, for reading its
/// descriptor.
pub struct Folder {
    handle: FileHandle,
    /// `Err` with the refusal when the open was granted listing but not
    /// `READ_CONTROL`.
    read_control: Result<(), WinError>,
}

/// Opens folder `id` on `volume` for listing and for reading its descriptor.
/// `Ok(None)` means the folder is gone or being deleted, so there is nothing
/// to list.
///
/// When the open with `READ_CONTROL` is refused, as by a filter driver that
/// the backup privilege does not pass, the folder opens for listing alone,
/// and only [`Folder::descriptor`] fails: a descriptor that does not read
/// never stops a walk.
pub fn open_folder(volume: &VolumeHandle, id: u128) -> Result<Option<Folder>, WinError> {
    let opened = match open_by_id(volume, id, FILE_LIST_DIRECTORY | READ_CONTROL) {
        Ok(handle) => Ok(handle.map(|handle| Folder {
            handle,
            read_control: Ok(()),
        })),
        Err(refused) if refused.code == ERROR_ACCESS_DENIED => {
            open_by_id(volume, id, FILE_LIST_DIRECTORY).map(|handle| {
                handle.map(|handle| Folder {
                    handle,
                    read_control: Err(refused),
                })
            })
        }
        Err(error) => Err(error),
    };
    match opened {
        Err(error) if is_being_deleted(volume, id, &error) => Ok(None),
        opened => opened,
    }
}

/// The security descriptor of folder `id` on `volume`, as the module doc
/// describes it. `Ok(None)` means the folder is gone or being deleted, by the
/// rule [`open_folder`] uses.
///
/// # Errors
///
/// [`WinError`] when the folder cannot be opened or its descriptor read, and
/// `ERROR_INVALID_SECURITY_DESCR` for a descriptor larger than 64 KiB,
/// malformed, or without an owner or a group, which `AccessCheck` refuses.
pub fn read_folder_descriptor(
    volume: &VolumeHandle,
    id: u128,
) -> Result<Option<Vec<u8>>, WinError> {
    match open_by_id(volume, id, READ_CONTROL) {
        Ok(Some(folder)) => handle_descriptor(folder.as_handle()).map(Some),
        Ok(None) => Ok(None),
        Err(error) if is_being_deleted(volume, id, &error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// A folder's descriptor and the raw `USN_RECORD` of
/// `FSCTL_READ_FILE_USN_DATA`, read from one open in that order, so a
/// permission change after the descriptor read gives the record a later USN.
/// `gozler-core` reads the record.
#[derive(Clone, Debug)]
pub struct RawFolderSecurity {
    /// As [`read_folder_descriptor`] returns it.
    pub descriptor: Vec<u8>,
    /// One `USN_RECORD_V2` or `USN_RECORD_V3`, exactly as Windows wrote it.
    pub usn_record: Vec<u8>,
}

/// Reads folder `id`'s descriptor, then its USN record. `Ok(None)` means
/// the folder is gone or being deleted, by the rule [`open_folder`] uses.
///
/// # Errors
///
/// As [`read_folder_descriptor`], and [`WinError`] when
/// `FSCTL_READ_FILE_USN_DATA` fails.
pub fn read_folder_security(
    volume: &VolumeHandle,
    id: u128,
) -> Result<Option<RawFolderSecurity>, WinError> {
    let folder = match open_by_id(volume, id, READ_CONTROL | FILE_READ_ATTRIBUTES) {
        Ok(Some(folder)) => folder,
        Ok(None) => return Ok(None),
        Err(error) if is_being_deleted(volume, id, &error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let descriptor = handle_descriptor(folder.as_handle())?;
    let usn_record = usn_record(folder.as_handle())?;
    Ok(Some(RawFolderSecurity {
        descriptor,
        usn_record,
    }))
}

/// A buffer for one USN record: the largest header, 76 bytes for V3, and the
/// longest name, 510 bytes, with the 8-byte alignment of the records.
#[repr(C, align(8))]
struct UsnRecordBuffer([u8; 1024]);

/// The raw `USN_RECORD` of the open file `file`, V2 or V3.
fn usn_record(file: HANDLE) -> Result<Vec<u8>, WinError> {
    let input = ReadFileUsnData {
        min_major_version: 2,
        max_major_version: 3,
    };
    let mut out = UsnRecordBuffer([0; 1024]);
    let written =
        ioctl_in_bytes(file, FSCTL_READ_FILE_USN_DATA, &input, &mut out.0).map_err(|code| {
            WinError {
                code,
                context: "DeviceIoControl (FSCTL_READ_FILE_USN_DATA)",
            }
        })?;
    Ok(out.0[..(written as usize).min(out.0.len())].to_vec())
}

/// The descriptor of the file open as `file`, which has `READ_CONTROL`.
pub(crate) fn handle_descriptor(file: HANDLE) -> Result<Vec<u8>, WinError> {
    // Most folder descriptors take a few hundred bytes. The buffer is u64s
    // because a descriptor's parts are 4-byte aligned.
    let mut buf = vec![0u64; 1024 / 8];
    loop {
        let capacity = size_of_val(buf.as_slice());
        let mut needed = 0u32;
        // SAFETY: `file` is a live handle; `buf` is `capacity` writable bytes
        // and `needed` a live out param.
        let ok = unsafe {
            GetKernelObjectSecurity(
                file,
                DESCRIPTOR_PARTS,
                buf.as_mut_ptr().cast(),
                capacity as u32,
                &mut needed,
            )
        };
        if ok != 0 {
            break;
        }
        let error = last_error("GetKernelObjectSecurity");
        let needed = needed as usize;
        if error.code != ERROR_INSUFFICIENT_BUFFER || needed <= capacity {
            return Err(error);
        }
        if needed > MAX_DESCRIPTOR_BYTES {
            return Err(WinError {
                code: ERROR_INVALID_SECURITY_DESCR,
                context: "GetKernelObjectSecurity: the descriptor is larger than 64 KiB",
            });
        }
        buf = vec![0u64; needed.div_ceil(8)];
    }
    // SAFETY: GetKernelObjectSecurity succeeded, so `buf` holds a valid
    // self-relative descriptor, which GetSecurityDescriptorLength only reads.
    let len = unsafe { GetSecurityDescriptorLength(buf.as_mut_ptr().cast()) } as usize;
    let bytes: Vec<u8> = buf
        .iter()
        .flat_map(|word| word.to_ne_bytes())
        .take(len)
        .collect();
    check_descriptor(&bytes)?;
    Ok(bytes)
}

/// Refuses `descriptor` unless it is a well-formed self-relative descriptor
/// of at most 64 KiB with an owner and a group. `AccessCheck` refuses a
/// descriptor without an owner or a group, so without this rule its folders
/// would be hidden from every filtered user as an unexplained check error,
/// not counted as a failed read. It also keeps every pointer `AccessCheck`
/// follows inside the bytes.
///
/// # Errors
///
/// [`WinError`] with `ERROR_INVALID_SECURITY_DESCR`.
pub(crate) fn check_descriptor(descriptor: &[u8]) -> Result<(), WinError> {
    if descriptor.len() > MAX_DESCRIPTOR_BYTES {
        return Err(WinError {
            code: ERROR_INVALID_SECURITY_DESCR,
            context: "the security descriptor is larger than 64 KiB",
        });
    }
    // SAFETY: the call reads at most `descriptor.len()` bytes of
    // `descriptor`, which is that long; the header test keeps it from
    // reading a header that is not there.
    let valid = descriptor.len() >= DESCRIPTOR_HEADER_BYTES
        && unsafe {
            RtlValidRelativeSecurityDescriptor(
                descriptor.as_ptr().cast_mut().cast(),
                descriptor.len() as u32,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
            )
        };
    if !valid {
        return Err(WinError {
            code: ERROR_INVALID_SECURITY_DESCR,
            context: "the security descriptor is malformed or has no owner or group",
        });
    }
    Ok(())
}

/// Whether a folder that refused to open has a delete pending: it was deleted
/// while another process still has it open. Windows deletes only an empty
/// folder and creates nothing in one with a delete pending, so it holds
/// nothing to list. It refuses every open with `ERROR_ACCESS_DENIED`, even one
/// that asks for no access, which a folder whose access list refuses listing
/// still allows.
fn is_being_deleted(volume: &VolumeHandle, id: u128, error: &WinError) -> bool {
    error.code == ERROR_ACCESS_DENIED
        && match open_by_id(volume, id, 0) {
            Ok(handle) => handle.is_none(),
            Err(e) => e.code == ERROR_ACCESS_DENIED,
        }
}

impl Folder {
    /// The folder's security descriptor, as [`read_folder_descriptor`]
    /// returns it, read from this open handle.
    ///
    /// # Errors
    ///
    /// As [`read_folder_descriptor`], except that an open folder cannot be
    /// gone; and the refusal of `READ_CONTROL` when [`open_folder`] opened
    /// the folder for listing alone.
    pub fn descriptor(&self) -> Result<Vec<u8>, WinError> {
        self.read_control.clone()?;
        handle_descriptor(self.handle.as_handle())
    }

    /// Reads the next page of the listing into `buf`: a chain of
    /// `FILE_ID_EXTD_DIR_INFO` entries, as bytes. `Ok(None)` when the
    /// listing is done. The buffer is `u64`s because the entries are 8-byte
    /// aligned.
    pub fn read_listing<'b>(&self, buf: &'b mut [u64]) -> Result<Option<&'b [u8]>, WinError> {
        let len = size_of_val(buf);
        let bytes = buf.as_mut_ptr().cast::<u8>();
        // SAFETY: `bytes` is the start of `buf`, which is `len` writable bytes
        // that outlive the call.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.handle.as_handle(),
                FileIdExtdDirectoryInfo,
                bytes.cast(),
                len as u32,
            )
        };
        if ok == 0 {
            let error = last_error("GetFileInformationByHandleEx (FileIdExtdDirectoryInfo)");
            return if error.code == ERROR_NO_MORE_FILES {
                Ok(None)
            } else {
                Err(error)
            };
        }
        // SAFETY: the same `len` bytes of `buf`, now borrowed for reading for
        // as long as `buf` is.
        Ok(Some(unsafe { core::slice::from_raw_parts(bytes, len) }))
    }
}
