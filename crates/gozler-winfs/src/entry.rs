//! The entries that the copy, move and delete engine walks: opening one by
//! its path or relative to its folder's handle, reading what it is, listing
//! a folder and deleting an entry.
//!
//! A relative open (`NtOpenFile` with `RootDirectory`) is Rust's fix for
//! CVE-2022-21658: once a walk holds a folder's handle, a folder above it
//! swapped for a link cannot redirect what the walk opens below it. The name
//! is one component, so [`Reparse::Open`] alone keeps the open on the entry
//! itself. Every open shares read, write and delete, so the engine's own
//! handles never make a delete fail.
//!
//! The listing returns raw `FILE_FULL_DIR_INFO` bytes; `gozler-core` parses
//! them.

use core::mem::{size_of, zeroed};
use core::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_FILENAME_EXCED_RANGE, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, RtlNtStatusToDosError,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_TAG_INFO,
    FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_LIST_DIRECTORY,
    FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FileAttributeTagInfo, FileBasicInfo,
    FileDispositionInfo, FileDispositionInfoEx, FileFullDirectoryInfo, FileIdInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
    VOLUME_NAME_DOS, VOLUME_NAME_GUID,
};

use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use crate::error::{WinError, last_error};
use crate::file_id::FileHandle;
use crate::links::final_path;

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// What an open of an [`Entry`] may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read its attributes, reparse tag, file ID and final path.
    Inspect,
    /// Also list it, when it is a folder.
    List,
    /// Also delete it. `DELETE` alone can come from the folder's
    /// `FILE_DELETE_CHILD`, so this deletes an entry whose own permissions
    /// grant the person nothing, as `del` does.
    Delete,
    /// Also delete it and clear its read-only attribute, which the classic
    /// delete of a read-only entry needs.
    DeleteReadOnly,
}

impl Access {
    /// The access that deletes an entry with these attributes: one without
    /// the read-only attribute is not asked for more than `DELETE`, because
    /// an open fails when it asks for a right the entry denies.
    pub fn delete(attributes: u32) -> Access {
        if attributes & FILE_ATTRIBUTE_READONLY == 0 {
            Access::Delete
        } else {
            Access::DeleteReadOnly
        }
    }

    /// The rights to ask for. Only a listing needs synchronous I/O, and so
    /// `SYNCHRONIZE`; the others leave it out, because a file whose own
    /// permissions grant nothing still grants `DELETE` and
    /// `FILE_READ_ATTRIBUTES` through its folder to an open relative to the
    /// folder, but not `SYNCHRONIZE`. An open by path, which `CreateFileW`
    /// makes, it refuses whatever the open asks for.
    fn rights(self) -> u32 {
        match self {
            Access::Inspect => FILE_READ_ATTRIBUTES,
            Access::List => FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | SYNCHRONIZE,
            Access::Delete => FILE_READ_ATTRIBUTES | DELETE,
            Access::DeleteReadOnly => FILE_READ_ATTRIBUTES | DELETE | FILE_WRITE_ATTRIBUTES,
        }
    }

    fn synchronous(self) -> bool {
        self == Access::List
    }
}

/// Whether an open of a link or junction opens the link or its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reparse {
    /// The link itself, so the engine never acts on a link's target by
    /// accident.
    Open,
    /// The target: for a destination the person chose, and for the filter's
    /// view of a folder whose reparse tag is not a name surrogate.
    Follow,
}

/// A file, folder or link's attributes and reparse tag
/// (`FILE_ATTRIBUTE_TAG_INFO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeTag {
    pub attributes: u32,
    /// 0 when the entry is not a reparse point.
    pub reparse_tag: u32,
}

/// What identifies a file within the machine (`FILE_ID_INFO`): a file ID is
/// unique only on its own volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume_serial: u64,
    pub file_id: u128,
}

/// An open file, folder or link.
pub struct Entry(FileHandle);

// SAFETY: a file handle is a kernel object reference that any thread of the
// process may use, and no method of `Entry` is tied to the thread that
// opened it.
unsafe impl Send for Entry {}
// SAFETY: as above; concurrent calls on one handle are sound. The engine's
// walk lists a folder on one thread while others open its entries.
unsafe impl Sync for Entry {}

/// Opens the entry at `path`, a full path with the `\\?\` or `\\?\UNC\`
/// prefix and without a NUL.
pub fn open_entry(path: &[u16], access: Access, reparse: Reparse) -> Result<Entry, WinError> {
    let path: Vec<u16> = path.iter().copied().chain(Some(0)).collect();
    // Backup semantics also open folders.
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if reparse == Reparse::Open {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    // SAFETY: `path` is NUL-terminated and outlives the call; no security
    // attributes and no template are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access.rights(),
            SHARE_ALL,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (open_entry)"));
    }
    Ok(Entry(FileHandle(handle)))
}

impl Entry {
    /// Opens `name`, one component without a separator, in this folder.
    pub fn open_child(
        &self,
        name: &[u16],
        access: Access,
        reparse: Reparse,
    ) -> Result<Entry, WinError> {
        let bytes = u16::try_from(2 * name.len()).map_err(|_| WinError {
            code: ERROR_FILENAME_EXCED_RANGE,
            context: "NtOpenFile",
        })?;
        let mut options = 0;
        if access.synchronous() {
            options |= FILE_SYNCHRONOUS_IO_NONALERT;
        }
        if reparse == Reparse::Open {
            options |= FILE_OPEN_REPARSE_POINT;
        }
        let mut handle = ptr::null_mut();
        // SAFETY: `object_name` points into `name` and `attributes` at
        // `object_name`; both outlive the call. The call writes the new
        // handle and the status block, both live stack slots.
        let status = unsafe {
            let object_name = UNICODE_STRING {
                Length: bytes,
                MaximumLength: bytes,
                Buffer: name.as_ptr().cast_mut(),
            };
            let attributes = OBJECT_ATTRIBUTES {
                Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
                RootDirectory: self.0.as_handle(),
                ObjectName: &object_name,
                ..Default::default()
            };
            let mut io: IO_STATUS_BLOCK = zeroed();
            NtOpenFile(
                &mut handle,
                access.rights(),
                &attributes,
                &mut io,
                SHARE_ALL,
                options,
            )
        };
        if status < 0 {
            return Err(WinError {
                // SAFETY: a pure status-code translation.
                code: unsafe { RtlNtStatusToDosError(status) },
                context: "NtOpenFile",
            });
        }
        Ok(Entry(FileHandle(handle)))
    }

    /// Opens this same entry again with `access`, so no swap of its name can
    /// put another file in its place. An open relative to this handle with
    /// an empty name does it; `ReOpenFile` refuses a folder with
    /// `ERROR_ACCESS_DENIED`.
    pub fn reopen(&self, access: Access) -> Result<Entry, WinError> {
        self.open_child(&[], access, Reparse::Open)
    }

    pub fn attribute_tag(&self) -> Result<AttributeTag, WinError> {
        // SAFETY: `info` is a plain struct the call fills, and its size is
        // passed.
        let info = unsafe {
            let mut info: FILE_ATTRIBUTE_TAG_INFO = zeroed();
            if GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileAttributeTagInfo,
                ptr::from_mut(&mut info).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            ) == 0
            {
                return Err(last_error(
                    "GetFileInformationByHandleEx (FileAttributeTagInfo)",
                ));
            }
            info
        };
        Ok(AttributeTag {
            attributes: info.FileAttributes,
            reparse_tag: info.ReparseTag,
        })
    }

    pub fn identity(&self) -> Result<FileIdentity, WinError> {
        // SAFETY: as in `attribute_tag`.
        let info = unsafe {
            let mut info: FILE_ID_INFO = zeroed();
            if GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileIdInfo,
                ptr::from_mut(&mut info).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            ) == 0
            {
                return Err(last_error("GetFileInformationByHandleEx (FileIdInfo)"));
            }
            info
        };
        Ok(FileIdentity {
            volume_serial: info.VolumeSerialNumber,
            file_id: u128::from_le_bytes(info.FileId.Identifier),
        })
    }

    /// The entry's normalized full path, without a NUL: from the volume's
    /// GUID path on a local volume (`\\?\Volume{...}\a\b`), or as
    /// `\\?\UNC\server\share\a\b` on a share, which has no volume GUID path.
    /// Two spellings of one entry, by case, short names, mount points or
    /// junctions above it, give one path.
    pub fn final_path(&self) -> Result<Vec<u16>, WinError> {
        let handle = self.0.as_handle();
        let guid = final_path(handle, VOLUME_NAME_GUID | FILE_NAME_NORMALIZED);
        let mut path = match guid {
            Ok(path) => path,
            Err(error) => {
                // Only a share answers without a GUID path; a local volume's
                // drive-letter path would compare unequal to its GUID path.
                let dos = final_path(handle, VOLUME_NAME_DOS | FILE_NAME_NORMALIZED)?;
                let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
                if !dos.starts_with(&unc) {
                    return Err(error);
                }
                dos
            }
        };
        path.pop(); // the NUL
        Ok(path)
    }

    /// Reads the next page of this folder's listing into `buf`: a chain of
    /// `FILE_FULL_DIR_INFO` entries, as bytes. `Ok(None)` when the listing is
    /// done. The buffer is `u64`s because the entries are 8-byte aligned.
    pub fn read_listing<'b>(&self, buf: &'b mut [u64]) -> Result<Option<&'b [u8]>, WinError> {
        let len = size_of_val(buf);
        let bytes = buf.as_mut_ptr().cast::<u8>();
        // SAFETY: `bytes` is the start of `buf`, which is `len` writable bytes
        // that outlive the call.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileFullDirectoryInfo,
                bytes.cast(),
                len as u32,
            )
        };
        if ok == 0 {
            let error = last_error("GetFileInformationByHandleEx (FileFullDirectoryInfo)");
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

    /// Deletes the entry with POSIX semantics, read-only or not: its name
    /// leaves when this handle closes. Needs [`Access::delete`]'s access. A
    /// file system that refuses the flags answers `ERROR_INVALID_PARAMETER`,
    /// `ERROR_NOT_SUPPORTED` or `ERROR_INVALID_FUNCTION`.
    pub fn delete_posix(&self) -> Result<(), WinError> {
        let info = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        self.set_info(FileDispositionInfoEx, &info, "FileDispositionInfoEx")
    }

    /// Deletes the entry with classic semantics, after clearing its
    /// read-only attribute, which the classic delete refuses. Needs
    /// [`Access::delete`]'s access, which for a read-only entry includes
    /// clearing the attribute. The name leaves when the last handle to the
    /// entry closes. A delete that then fails leaves the attribute cleared.
    pub fn delete_classic(&self) -> Result<(), WinError> {
        // SAFETY: as in `attribute_tag`.
        let basic = unsafe {
            let mut info: FILE_BASIC_INFO = zeroed();
            if GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileBasicInfo,
                ptr::from_mut(&mut info).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            ) == 0
            {
                return Err(last_error("GetFileInformationByHandleEx (FileBasicInfo)"));
            }
            info
        };
        if basic.FileAttributes & FILE_ATTRIBUTE_READONLY != 0 {
            let attributes = basic.FileAttributes & !FILE_ATTRIBUTE_READONLY;
            // Zero times leave the times as they are, and zero attributes
            // would too, so a file with no other attribute gets NORMAL.
            let cleared = FILE_BASIC_INFO {
                FileAttributes: if attributes == 0 {
                    FILE_ATTRIBUTE_NORMAL
                } else {
                    attributes
                },
                ..Default::default()
            };
            self.set_info(FileBasicInfo, &cleared, "FileBasicInfo")?;
        }
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        self.set_info(FileDispositionInfo, &info, "FileDispositionInfo")
    }

    fn set_info<T>(&self, class: i32, info: &T, context: &'static str) -> Result<(), WinError> {
        // SAFETY: `info` is the plain struct that `class` names, and its size
        // is passed.
        let ok = unsafe {
            SetFileInformationByHandle(
                self.0.as_handle(),
                class,
                ptr::from_ref(info).cast(),
                size_of::<T>() as u32,
            )
        };
        if ok == 0 {
            return Err(last_error(context));
        }
        Ok(())
    }
}
