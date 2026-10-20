//! The ring engine's Win32 calls: the unbuffered, overlapped opens, the
//! aligned buffer with its completion port, a file's sparse ranges, the
//! block clone, and the backup streams that carry its other streams.
//!
//! An overlapped read or write owns its part of the buffer until Windows
//! reports it complete, because Windows writes into the buffer until then.
//! [`IoRing`] keeps that rule: it holds the file of every operation in
//! flight, refuses an operation on a part of the buffer that one in flight
//! uses, and on drop cancels and waits for every operation before it frees
//! the buffer.

use core::ffi::c_void;
use core::mem::{size_of, zeroed};
use core::ptr;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_MORE_DATA, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    BackupRead, BackupSeek, BackupWrite, CREATE_NEW, CreateFileW, DELETE, FILE_END_OF_FILE_INFO,
    FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STORAGE_INFO,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, FileEndOfFileInfo, FileStorageInfo,
    GetDiskFreeSpaceW, GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
    ReOpenFile, ReadFile, SYNCHRONIZE, WriteFile,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_BASIC_INFO, FileBasicInfo};
use windows_sys::Win32::System::IO::{
    CancelIoEx, CreateIoCompletionPort, DeviceIoControl, GetOverlappedResult,
    GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus,
};
use windows_sys::Win32::System::Ioctl::{
    DUPLICATE_EXTENTS_DATA, FILE_ALLOCATED_RANGE_BUFFER, FILE_SET_SPARSE_BUFFER,
    FSCTL_DUPLICATE_EXTENTS_TO_FILE, FSCTL_QUERY_ALLOCATED_RANGES, FSCTL_SET_SPARSE,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE};

use crate::entry::Entry;
use crate::error::{WinError, last_error};
use crate::file_id::FileHandle;

/// How the ring opens a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingOpen {
    /// `FILE_FLAG_NO_BUFFERING`: every read and write is then aligned to the
    /// volume's sector size, at an aligned address.
    pub unbuffered: bool,
    /// `FILE_FLAG_OVERLAPPED`, for an [`IoRing`].
    pub overlapped: bool,
}

impl RingOpen {
    fn flags(self) -> u32 {
        let mut flags = 0;
        if self.unbuffered {
            flags |= FILE_FLAG_NO_BUFFERING;
        }
        if self.overlapped {
            flags |= FILE_FLAG_OVERLAPPED;
        }
        flags
    }
}

fn create(
    path: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    flags: u32,
) -> Result<Entry, WinError> {
    let path: Vec<u16> = path.iter().copied().chain(Some(0)).collect();
    // SAFETY: `path` is NUL-terminated and outlives the call; no security
    // attributes and no template are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            share,
            ptr::null(),
            disposition,
            flags,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (ring)"));
    }
    Ok(Entry(FileHandle(handle)))
}

/// Opens the file at `path` to read it. It shares reading and deleting but
/// not writing, so a file that another program has open for writing fails
/// with `ERROR_SHARING_VIOLATION` instead of being read while it changes.
pub fn open_ring_source(path: &[u16], open: RingOpen) -> Result<Entry, WinError> {
    let share = FILE_SHARE_READ | FILE_SHARE_DELETE;
    let flags = open.flags() | FILE_FLAG_SEQUENTIAL_SCAN;
    create(path, FILE_GENERIC_READ, share, OPEN_EXISTING, flags)
}

/// Not `FILE_GENERIC_WRITE`: it holds `FILE_APPEND_DATA`, which an
/// unbuffered open refuses with `ERROR_INVALID_PARAMETER`.
const DESTINATION_ACCESS: u32 = FILE_READ_ATTRIBUTES
    | FILE_WRITE_DATA
    | FILE_WRITE_ATTRIBUTES
    | FILE_WRITE_EA
    | DELETE
    | SYNCHRONIZE;

/// Shares everything, so the ring's own second handle for the backup
/// streams can open the destination.
const DESTINATION_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// Creates the file `path`, which must not exist, to write it. The handle
/// can also delete it.
pub fn create_ring_destination(path: &[u16], open: RingOpen) -> Result<Entry, WinError> {
    create(
        path,
        DESTINATION_ACCESS,
        DESTINATION_SHARE,
        CREATE_NEW,
        open.flags(),
    )
}

/// Opens the entry at `path` with its bytes, to read and write it, with the
/// rights of [`create_ring_destination`]'s handle and read access. A reparse
/// point at `path` opens itself, not its target, so a caller that checks
/// the handle's reparse tag never writes through a link.
pub fn open_ring_destination(path: &[u16], open: RingOpen) -> Result<Entry, WinError> {
    let access = DESTINATION_ACCESS | FILE_READ_DATA;
    let flags = open.flags() | FILE_FLAG_OPEN_REPARSE_POINT;
    create(path, access, DESTINATION_SHARE, OPEN_EXISTING, flags)
}

/// Page-aligned memory, for unbuffered reads outside an [`IoRing`].
pub struct AlignedBuffer {
    memory: *mut u8,
    len: usize,
}

// SAFETY: the memory belongs to this value alone.
unsafe impl Send for AlignedBuffer {}

impl AlignedBuffer {
    pub fn new(len: usize) -> Result<AlignedBuffer, WinError> {
        // SAFETY: a fresh committed region; null means failure.
        let memory = unsafe {
            VirtualAlloc(
                ptr::null(),
                len.max(1),
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if memory.is_null() {
            return Err(last_error("VirtualAlloc"));
        }
        Ok(AlignedBuffer {
            memory: memory.cast(),
            len,
        })
    }
}

impl core::ops::Deref for AlignedBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: `len` committed bytes, which VirtualAlloc zeroed, live
        // until drop.
        unsafe { core::slice::from_raw_parts(self.memory, self.len) }
    }
}

impl core::ops::DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `deref`, borrowed mutably through `self`.
        unsafe { core::slice::from_raw_parts_mut(self.memory, self.len) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        // SAFETY: the region VirtualAlloc returned, released once.
        unsafe { VirtualFree(self.memory.cast(), 0, MEM_RELEASE) };
    }
}

/// The bytes in a cluster of the volume whose root is `root`, such as
/// `\\?\Volume{...}\`, with its trailing backslash and without a NUL.
pub fn cluster_size(root: &[u16]) -> Result<u32, WinError> {
    let root: Vec<u16> = root.iter().copied().chain(Some(0)).collect();
    let (mut sectors, mut bytes, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: `root` is NUL-terminated; the out-params are live locals.
    let ok = unsafe {
        GetDiskFreeSpaceW(
            root.as_ptr(),
            &mut sectors,
            &mut bytes,
            &mut free,
            &mut total,
        )
    };
    if ok == 0 {
        return Err(last_error("GetDiskFreeSpaceW"));
    }
    Ok(sectors * bytes)
}

/// A file's attributes and times (`FILE_BASIC_INFO`), each time as a
/// `FILETIME` in 100 ns units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicInfo {
    pub creation: i64,
    pub last_access: i64,
    pub last_write: i64,
    pub attributes: u32,
}

/// Runs a control code on `handle` and waits for it, whether the handle is
/// overlapped or not. The event's low bit keeps the completion out of any
/// completion port that the handle is tied to.
fn control(
    handle: HANDLE,
    code: u32,
    input: *const c_void,
    input_len: u32,
    output: *mut c_void,
    output_len: u32,
    context: &'static str,
) -> Result<u32, WinError> {
    // SAFETY: a manual-reset event with no name or security attributes.
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(last_error("CreateEventW"));
    }
    let event = FileHandle(event);
    // SAFETY: `overlapped` and the caller's buffers outlive the call, which
    // `GetOverlappedResult` waits for before they go out of scope.
    unsafe {
        let mut overlapped: OVERLAPPED = zeroed();
        overlapped.hEvent = (event.as_handle() as usize | 1) as HANDLE;
        let mut bytes = 0u32;
        let ok = DeviceIoControl(
            handle,
            code,
            input,
            input_len,
            output,
            output_len,
            &mut bytes,
            &mut overlapped,
        );
        if ok == 0 && GetLastError() != ERROR_IO_PENDING {
            return Err(last_error(context));
        }
        if GetOverlappedResult(handle, &overlapped, &mut bytes, 1) == 0 {
            return Err(last_error(context));
        }
        Ok(bytes)
    }
}

impl Entry {
    /// The alignment of an unbuffered read or write: the volume's physical
    /// sector size, or its logical one when that is larger.
    pub fn sector_size(&self) -> Result<u32, WinError> {
        // SAFETY: as in `attribute_tag`.
        let info = unsafe {
            let mut info: FILE_STORAGE_INFO = zeroed();
            if GetFileInformationByHandleEx(
                self.0.as_handle(),
                FileStorageInfo,
                ptr::from_mut(&mut info).cast(),
                size_of::<FILE_STORAGE_INFO>() as u32,
            ) == 0
            {
                return Err(last_error("GetFileInformationByHandleEx (FileStorageInfo)"));
            }
            info
        };
        Ok(info
            .PhysicalBytesPerSectorForPerformance
            .max(info.LogicalBytesPerSector))
    }

    /// The name of the file system that holds the entry, such as `NTFS` or
    /// `FAT32`.
    pub fn file_system_name(&self) -> Result<String, WinError> {
        let mut name = [0u16; 32];
        // SAFETY: the handle is live; `name` is a live buffer of the length
        // passed, and the unused out-params are null, which the API permits.
        let ok = unsafe {
            GetVolumeInformationByHandleW(
                self.0.as_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        };
        if ok == 0 {
            return Err(last_error("GetVolumeInformationByHandleW"));
        }
        let len = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        Ok(String::from_utf16_lossy(&name[..len]))
    }

    pub fn basic_info(&self) -> Result<BasicInfo, WinError> {
        // SAFETY: as in `attribute_tag`.
        let info = unsafe {
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
        Ok(BasicInfo {
            creation: info.CreationTime,
            last_access: info.LastAccessTime,
            last_write: info.LastWriteTime,
            attributes: info.FileAttributes,
        })
    }

    /// Sets the attributes and the three times. Windows keeps the times as
    /// set: later writes through this handle no longer change them.
    pub fn set_basic_info(&self, info: BasicInfo) -> Result<(), WinError> {
        let info = FILE_BASIC_INFO {
            CreationTime: info.creation,
            LastAccessTime: info.last_access,
            LastWriteTime: info.last_write,
            // Zero leaves the change time as it is.
            ChangeTime: 0,
            FileAttributes: info.attributes,
        };
        self.set_info(FileBasicInfo, &info, "FileBasicInfo")
    }

    /// Reads up to `buf.len()` bytes at `offset` and waits for them, whether
    /// the handle is overlapped or not; returns the bytes read, fewer at the
    /// end of the file. On an unbuffered handle the offset, the buffer and
    /// its length are aligned to the sector size. The event's low bit keeps
    /// the completion out of any completion port that the handle is tied to.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, WinError> {
        // SAFETY: a manual-reset event with no name or security attributes.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(last_error("CreateEventW"));
        }
        let event = FileHandle(event);
        let handle = self.0.as_handle();
        // SAFETY: `buf` and `overlapped` outlive the read, which
        // `GetOverlappedResult` waits for before they go out of scope.
        unsafe {
            let mut overlapped: OVERLAPPED = zeroed();
            overlapped.Anonymous.Anonymous.Offset = offset as u32;
            overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
            overlapped.hEvent = (event.as_handle() as usize | 1) as HANDLE;
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ok = ReadFile(
                handle,
                buf.as_mut_ptr(),
                len,
                ptr::null_mut(),
                &mut overlapped,
            );
            if ok == 0 {
                match GetLastError() {
                    ERROR_HANDLE_EOF => return Ok(0),
                    ERROR_IO_PENDING => {}
                    _ => return Err(last_error("ReadFile")),
                }
            }
            let mut read = 0u32;
            if GetOverlappedResult(handle, &overlapped, &mut read, 1) == 0 {
                let error = last_error("ReadFile");
                return if error.code == ERROR_HANDLE_EOF {
                    Ok(0)
                } else {
                    Err(error)
                };
            }
            Ok(read as usize)
        }
    }

    pub fn set_end_of_file(&self, size: u64) -> Result<(), WinError> {
        let info = FILE_END_OF_FILE_INFO {
            EndOfFile: size as i64,
        };
        self.set_info(FileEndOfFileInfo, &info, "FileEndOfFileInfo")
    }

    /// Marks the file sparse, or clears the mark (`FSCTL_SET_SPARSE`).
    pub fn set_sparse(&self, sparse: bool) -> Result<(), WinError> {
        let input = FILE_SET_SPARSE_BUFFER { SetSparse: sparse };
        control(
            self.0.as_handle(),
            FSCTL_SET_SPARSE,
            ptr::from_ref(&input).cast(),
            size_of::<FILE_SET_SPARSE_BUFFER>() as u32,
            ptr::null_mut(),
            0,
            "FSCTL_SET_SPARSE",
        )
        .map(drop)
    }

    /// The ranges of the file's first `size` bytes that hold data, as
    /// `(offset, length)` in order (`FSCTL_QUERY_ALLOCATED_RANGES`). The rest
    /// is holes, which read as zeros.
    pub fn allocated_ranges(&self, size: u64) -> Result<Vec<(u64, u64)>, WinError> {
        let mut ranges = Vec::new();
        let mut from = 0u64;
        let mut page = [FILE_ALLOCATED_RANGE_BUFFER::default(); 64];
        while from < size {
            let query = FILE_ALLOCATED_RANGE_BUFFER {
                FileOffset: from as i64,
                Length: (size - from) as i64,
            };
            let answered = control(
                self.0.as_handle(),
                FSCTL_QUERY_ALLOCATED_RANGES,
                ptr::from_ref(&query).cast(),
                size_of::<FILE_ALLOCATED_RANGE_BUFFER>() as u32,
                page.as_mut_ptr().cast(),
                size_of_val(&page) as u32,
                "FSCTL_QUERY_ALLOCATED_RANGES",
            );
            let (bytes, more) = match answered {
                Ok(bytes) => (bytes, false),
                Err(error) if error.code == ERROR_MORE_DATA => (size_of_val(&page) as u32, true),
                Err(error) => return Err(error),
            };
            let count = bytes as usize / size_of::<FILE_ALLOCATED_RANGE_BUFFER>();
            for range in &page[..count] {
                let (offset, length) = (range.FileOffset as u64, range.Length as u64);
                let end = (offset + length).min(size);
                if end > offset {
                    ranges.push((offset, end - offset));
                }
                from = from.max(end);
            }
            if !more || count == 0 {
                break;
            }
        }
        Ok(ranges)
    }

    /// Clones `len` bytes at `offset` of `source` into this file at the same
    /// offset (`FSCTL_DUPLICATE_EXTENTS_TO_FILE`), writing no data. The
    /// range is less than 4 GB and starts at a cluster boundary, and this
    /// file's end of file already covers it.
    pub fn clone_range(&self, source: &Entry, offset: u64, len: u64) -> Result<(), WinError> {
        let data = DUPLICATE_EXTENTS_DATA {
            FileHandle: source.0.as_handle(),
            SourceFileOffset: offset as i64,
            TargetFileOffset: offset as i64,
            ByteCount: len as i64,
        };
        control(
            self.0.as_handle(),
            FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            ptr::from_ref(&data).cast(),
            size_of::<DUPLICATE_EXTENTS_DATA>() as u32,
            ptr::null_mut(),
            0,
            "FSCTL_DUPLICATE_EXTENTS_TO_FILE",
        )
        .map(drop)
    }

    /// Opens this same file again, buffered and synchronous, to read its
    /// backup streams, or to write them when `write` is set.
    pub fn reopen_for_streams(&self, write: bool) -> Result<Entry, WinError> {
        let access = if write {
            FILE_GENERIC_WRITE
        } else {
            FILE_GENERIC_READ
        };
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        // SAFETY: the handle is live; ReOpenFile returns a new handle or
        // INVALID_HANDLE_VALUE.
        let handle = unsafe { ReOpenFile(self.0.as_handle(), access, share, 0) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("ReOpenFile"));
        }
        Ok(Entry(FileHandle(handle)))
    }
}

/// Reads a file's streams with `BackupRead`, without its security
/// descriptor. Ends its read context when dropped.
pub struct BackupReader<'a> {
    file: &'a Entry,
    context: *mut c_void,
}

impl<'a> BackupReader<'a> {
    /// `file` comes from [`Entry::reopen_for_streams`].
    pub fn new(file: &'a Entry) -> Self {
        BackupReader {
            file,
            context: ptr::null_mut(),
        }
    }

    /// Reads up to `buf.len()` bytes; 0 at the end of the last stream.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, WinError> {
        let mut read = 0u32;
        // SAFETY: `buf` is writable for the length passed; `context` is the
        // one this reader carries between calls.
        let ok = unsafe {
            BackupRead(
                self.file.0.as_handle(),
                buf.as_mut_ptr(),
                buf.len().min(u32::MAX as usize) as u32,
                &mut read,
                0,
                0,
                &mut self.context,
            )
        };
        if ok == 0 {
            return Err(last_error("BackupRead"));
        }
        Ok(read as usize)
    }

    /// Skips up to `bytes` of the current stream's data; returns how many
    /// it skipped, which is fewer at the end of the stream.
    pub fn skip(&mut self, bytes: u64) -> u64 {
        let (mut low, mut high) = (0u32, 0u32);
        // SAFETY: the out-params are live locals; `context` as in `read`.
        unsafe {
            BackupSeek(
                self.file.0.as_handle(),
                bytes as u32,
                (bytes >> 32) as u32,
                &mut low,
                &mut high,
                &mut self.context,
            )
        };
        u64::from(high) << 32 | u64::from(low)
    }
}

impl Drop for BackupReader<'_> {
    fn drop(&mut self) {
        if self.context.is_null() {
            return;
        }
        let mut read = 0u32;
        // SAFETY: an abort with no buffer frees the context.
        unsafe {
            BackupRead(
                self.file.0.as_handle(),
                ptr::null_mut(),
                0,
                &mut read,
                1,
                0,
                &mut self.context,
            )
        };
    }
}

/// Writes streams in `BackupRead`'s format with `BackupWrite`, without a
/// security descriptor. Ends its write context when dropped.
pub struct BackupWriter<'a> {
    file: &'a Entry,
    context: *mut c_void,
}

impl<'a> BackupWriter<'a> {
    /// `file` comes from [`Entry::reopen_for_streams`] with `write`.
    pub fn new(file: &'a Entry) -> Self {
        BackupWriter {
            file,
            context: ptr::null_mut(),
        }
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), WinError> {
        let mut written = 0u32;
        // SAFETY: `bytes` is readable for the length passed; `context` is
        // the one this writer carries between calls.
        let ok = unsafe {
            BackupWrite(
                self.file.0.as_handle(),
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                0,
                0,
                &mut self.context,
            )
        };
        if ok == 0 {
            return Err(last_error("BackupWrite"));
        }
        Ok(())
    }
}

impl Drop for BackupWriter<'_> {
    fn drop(&mut self) {
        if self.context.is_null() {
            return;
        }
        let mut written = 0u32;
        // SAFETY: an abort with no buffer frees the context.
        unsafe {
            BackupWrite(
                self.file.0.as_handle(),
                ptr::null(),
                0,
                &mut written,
                1,
                0,
                &mut self.context,
            )
        };
    }
}

/// One operation's place. `overlapped` comes first, so the pointer that the
/// completion port returns is the slot's.
#[repr(C)]
struct Slot {
    overlapped: OVERLAPPED,
    /// The file of the operation in flight; `None` when the slot is free.
    file: Option<Arc<Entry>>,
    at: usize,
    len: usize,
}

/// What [`IoRing::wait`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// The operation in `slot` ended, with the bytes it moved or Windows'
    /// error. A read at the end of the file moved 0 bytes.
    Done {
        slot: usize,
        result: Result<usize, u32>,
    },
    /// A [`RingWaker`] woke the ring.
    Woken,
}

/// A completion port, closed when the ring and its wakers are gone.
struct Port(FileHandle);

// SAFETY: a completion port is a kernel object that any thread may wait on
// or post to.
unsafe impl Send for Port {}
// SAFETY: as above.
unsafe impl Sync for Port {}

impl Port {
    fn as_handle(&self) -> HANDLE {
        self.0.as_handle()
    }
}

/// Wakes a waiting [`IoRing`] from another thread.
#[derive(Clone)]
pub struct RingWaker(Arc<Port>);

impl RingWaker {
    pub fn wake(&self) {
        // SAFETY: the port is live while this waker holds it.
        unsafe { PostQueuedCompletionStatus(self.0.as_handle(), 0, 0, ptr::null()) };
    }
}

/// An aligned buffer and the overlapped reads and writes into it, on one
/// completion port.
pub struct IoRing {
    port: Arc<Port>,
    memory: *mut u8,
    capacity: usize,
    slots: Box<[Slot]>,
    busy: usize,
}

// SAFETY: the buffer and the slots belong to this value alone; the port and
// the files are kernel handles that any thread may use.
unsafe impl Send for IoRing {}

impl IoRing {
    /// A buffer of `capacity` bytes, aligned to a page, with room for
    /// `slots` operations in flight.
    pub fn new(capacity: usize, slots: usize) -> Result<IoRing, WinError> {
        // SAFETY: a new port, not tied to a file.
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1) };
        if port.is_null() {
            return Err(last_error("CreateIoCompletionPort"));
        }
        let port = Arc::new(Port(FileHandle(port)));
        // SAFETY: a fresh committed region; null means failure.
        let memory = unsafe {
            VirtualAlloc(
                ptr::null(),
                capacity,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if memory.is_null() {
            return Err(last_error("VirtualAlloc"));
        }
        let slots = (0..slots)
            .map(|_| Slot {
                // SAFETY: an all-zero OVERLAPPED is its initial state.
                overlapped: unsafe { zeroed() },
                file: None,
                at: 0,
                len: 0,
            })
            .collect();
        Ok(IoRing {
            port,
            memory: memory.cast(),
            capacity,
            slots,
            busy: 0,
        })
    }

    pub fn waker(&self) -> RingWaker {
        RingWaker(Arc::clone(&self.port))
    }

    /// Ties an overlapped file to this ring's port, once per file.
    pub fn associate(&self, file: &Entry) -> Result<(), WinError> {
        // SAFETY: both handles are live; the key is unused.
        let port =
            unsafe { CreateIoCompletionPort(file.0.as_handle(), self.port.as_handle(), 0, 0) };
        if port.is_null() {
            return Err(last_error("CreateIoCompletionPort (associate)"));
        }
        Ok(())
    }

    /// Operations in flight.
    pub fn in_flight(&self) -> usize {
        self.busy
    }

    /// The buffer range `at..at + len`, which no operation in flight uses.
    fn range(&self, at: usize, len: usize) -> *mut u8 {
        let end = at.checked_add(len).expect("a buffer range fits in memory");
        assert!(
            end <= self.capacity,
            "the range {at}..{end} is inside the buffer"
        );
        let overlaps = (self.slots.iter())
            .any(|slot| slot.file.is_some() && at < slot.at + slot.len && slot.at < end);
        assert!(!overlaps, "no operation in flight uses {at}..{end}");
        // SAFETY: `at` is inside the allocation, as just checked.
        unsafe { self.memory.add(at) }
    }

    /// Starts reading `len` bytes at `offset` of `file` into the buffer at
    /// `at`, and returns its slot.
    pub fn start_read(
        &mut self,
        file: &Arc<Entry>,
        offset: u64,
        at: usize,
        len: usize,
    ) -> Result<usize, WinError> {
        self.start(file, offset, at, len, false)
    }

    /// Starts writing the buffer's `len` bytes at `at` to `offset` of `file`.
    pub fn start_write(
        &mut self,
        file: &Arc<Entry>,
        offset: u64,
        at: usize,
        len: usize,
    ) -> Result<usize, WinError> {
        self.start(file, offset, at, len, true)
    }

    fn start(
        &mut self,
        file: &Arc<Entry>,
        offset: u64,
        at: usize,
        len: usize,
        write: bool,
    ) -> Result<usize, WinError> {
        let data = self.range(at, len);
        let slot = (self.slots.iter())
            .position(|slot| slot.file.is_none())
            .expect("the ring starts no more operations than it has slots");
        let entry = &mut self.slots[slot];
        // SAFETY: an all-zero OVERLAPPED with only the offset set.
        entry.overlapped = unsafe { zeroed() };
        entry.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        entry.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let handle = file.0.as_handle();
        let overlapped = &raw mut entry.overlapped;
        // SAFETY: `data` is `len` bytes of the buffer that no other operation
        // uses, and both it and the OVERLAPPED stay in place until the
        // completion, which `wait` or `drop` collects.
        let ok = unsafe {
            if write {
                WriteFile(handle, data, len as u32, ptr::null_mut(), overlapped)
            } else {
                ReadFile(handle, data, len as u32, ptr::null_mut(), overlapped)
            }
        };
        // SAFETY: GetLastError has no preconditions.
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(last_error(if write { "WriteFile" } else { "ReadFile" }));
        }
        // Started: the port reports its end, whether it ended at once or not.
        let entry = &mut self.slots[slot];
        entry.file = Some(Arc::clone(file));
        entry.at = at;
        entry.len = len;
        self.busy += 1;
        Ok(slot)
    }

    /// Reads synchronously from a file opened without `overlapped`; returns
    /// the bytes read, fewer at the end of the file.
    pub fn read_sync(
        &mut self,
        file: &Entry,
        offset: u64,
        at: usize,
        len: usize,
    ) -> Result<usize, WinError> {
        let data = self.range(at, len);
        // SAFETY: as in `start`, and the call returns only when done.
        unsafe {
            let mut overlapped: OVERLAPPED = zeroed();
            overlapped.Anonymous.Anonymous.Offset = offset as u32;
            overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
            let mut read = 0u32;
            if ReadFile(
                file.0.as_handle(),
                data,
                len as u32,
                &mut read,
                &mut overlapped,
            ) == 0
            {
                let error = last_error("ReadFile");
                return if error.code == ERROR_HANDLE_EOF {
                    Ok(0)
                } else {
                    Err(error)
                };
            }
            Ok(read as usize)
        }
    }

    /// Writes synchronously to a file opened without `overlapped`.
    pub fn write_sync(
        &mut self,
        file: &Entry,
        offset: u64,
        at: usize,
        len: usize,
    ) -> Result<(), WinError> {
        let data = self.range(at, len);
        // SAFETY: as in `read_sync`.
        unsafe {
            let mut overlapped: OVERLAPPED = zeroed();
            overlapped.Anonymous.Anonymous.Offset = offset as u32;
            overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
            let mut written = 0u32;
            if WriteFile(
                file.0.as_handle(),
                data,
                len as u32,
                &mut written,
                &mut overlapped,
            ) == 0
            {
                return Err(last_error("WriteFile"));
            }
        }
        Ok(())
    }

    /// Waits up to `timeout_ms` for an operation to end or for a waker;
    /// `None` when the time ran out.
    pub fn wait(&mut self, timeout_ms: u32) -> Result<Option<Completion>, WinError> {
        let mut bytes = 0u32;
        let mut key = 0usize;
        let mut overlapped: *mut OVERLAPPED = ptr::null_mut();
        // SAFETY: the out-params are live locals.
        let ok = unsafe {
            GetQueuedCompletionStatus(
                self.port.as_handle(),
                &mut bytes,
                &mut key,
                &mut overlapped,
                timeout_ms,
            )
        };
        if overlapped.is_null() {
            if ok != 0 {
                return Ok(Some(Completion::Woken));
            }
            let error = last_error("GetQueuedCompletionStatus");
            return if error.code == WAIT_TIMEOUT {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let result = if ok != 0 {
            Ok(bytes as usize)
        } else {
            // SAFETY: GetLastError has no preconditions.
            match unsafe { GetLastError() } {
                ERROR_HANDLE_EOF => Ok(0),
                code => Err(code),
            }
        };
        let base = self.slots.as_ptr() as usize;
        let slot = (overlapped as usize - base) / size_of::<Slot>();
        let entry = &mut self.slots[slot];
        assert!(
            entry.file.is_some(),
            "a completion belongs to a started slot"
        );
        entry.file = None;
        self.busy -= 1;
        Ok(Some(Completion::Done { slot, result }))
    }

    /// Asks Windows to end every operation in flight early; each still ends
    /// through [`IoRing::wait`].
    pub fn cancel(&self) {
        for slot in self.slots.iter() {
            if let Some(file) = &slot.file {
                // SAFETY: the file and the OVERLAPPED are live while the slot
                // is busy.
                unsafe { CancelIoEx(file.0.as_handle(), &slot.overlapped) };
            }
        }
    }
}

impl Drop for IoRing {
    fn drop(&mut self) {
        self.cancel();
        while self.busy > 0 {
            if self.wait(INFINITE).is_err() {
                break;
            }
        }
        // A buffer that Windows may still write into is leaked rather than
        // freed.
        if self.busy == 0 {
            // SAFETY: the region came from VirtualAlloc, and no operation uses
            // it any more.
            unsafe { VirtualFree(self.memory.cast(), 0, MEM_RELEASE) };
        }
    }
}
