//! What the copy engine reads about a volume's disks, to choose its workers
//! and which operations wait for each other, and the running Windows build.
//!
//! The volume is opened with no access rights, as BetterCopy opens it, so
//! no administrator right is needed for these queries.

use core::mem::{offset_of, size_of, zeroed};
use core::ptr;

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DISK_EXTENT, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

use crate::error::{WinError, last_error};
use crate::file_id::FileHandle;
use crate::ioctl::{ioctl_in_bytes, ioctl_out};

/// What a volume's storage says about itself. Each field is `None` when the
/// storage does not answer its query, as a virtual or remote disk may not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeStorage {
    /// Whether reading at another place costs a seek, as on a hard disk.
    pub seek_penalty: Option<bool>,
    pub device: Option<StorageDevice>,
    /// The physical disks the volume lies on; a volume can span disks.
    pub disks: Option<Vec<u32>>,
}

/// From `STORAGE_DEVICE_DESCRIPTOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageDevice {
    /// `STORAGE_BUS_TYPE`: 7 is USB.
    pub bus_type: i32,
    /// Whether the device takes several commands at once. A USB device
    /// without it is a bulk-only device, which serves one file at a time.
    pub command_queueing: bool,
}

/// The most disks a volume's answer may name; a volume on more is taken as
/// one whose disks cannot be read.
const MAX_DISKS: usize = 32;

#[repr(C)]
struct DiskExtents {
    count: u32,
    extents: [DISK_EXTENT; MAX_DISKS],
}

/// Reads the storage under `volume`, a `\\?\Volume{GUID}` path without a
/// trailing backslash. Fails only when the volume cannot be opened.
pub fn volume_storage(volume: &[u16]) -> Result<VolumeStorage, WinError> {
    let path: Vec<u16> = volume.iter().copied().chain(Some(0)).collect();
    // SAFETY: `path` is NUL-terminated; no security attributes and no
    // template are supplied.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW (volume_storage)"));
    }
    let handle = FileHandle(handle);
    let query = |property| STORAGE_PROPERTY_QUERY {
        PropertyId: property,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };

    let mut penalty = [0u8; size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()];
    let seek_penalty = ioctl_in_bytes(
        handle.as_handle(),
        IOCTL_STORAGE_QUERY_PROPERTY,
        &query(StorageDeviceSeekPenaltyProperty),
        &mut penalty,
    )
    .ok()
    .filter(|&bytes| bytes as usize >= size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>())
    .map(|_| penalty[offset_of!(DEVICE_SEEK_PENALTY_DESCRIPTOR, IncursSeekPenalty)] != 0);

    // The descriptor's fixed part; the strings after it are not needed.
    let mut device = [0u8; size_of::<STORAGE_DEVICE_DESCRIPTOR>()];
    let device = ioctl_in_bytes(
        handle.as_handle(),
        IOCTL_STORAGE_QUERY_PROPERTY,
        &query(StorageDeviceProperty),
        &mut device,
    )
    .ok()
    .filter(|&bytes| bytes as usize >= size_of::<STORAGE_DEVICE_DESCRIPTOR>())
    .map(|_| {
        let bus = offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType);
        StorageDevice {
            bus_type: i32::from_le_bytes(device[bus..bus + 4].try_into().expect("4 bytes")),
            command_queueing: device[offset_of!(STORAGE_DEVICE_DESCRIPTOR, CommandQueueing)] != 0,
        }
    });

    // SAFETY: `DiskExtents` is plain integers, for which zero is valid.
    let mut extents: DiskExtents = unsafe { zeroed() };
    let disks = ioctl_out(
        handle.as_handle(),
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        &mut extents,
    )
    .ok()
    .filter(|_| extents.count as usize <= MAX_DISKS)
    .map(|_| {
        let mut disks: Vec<u32> = extents.extents[..extents.count as usize]
            .iter()
            .map(|extent| extent.DiskNumber)
            .collect();
        disks.sort_unstable();
        disks.dedup();
        disks
    });

    Ok(VolumeStorage {
        seek_penalty,
        device,
        disks,
    })
}

/// The running Windows build, such as 26200. `GetVersionEx` would report
/// the build that the executable's manifest declares instead.
pub fn windows_build() -> u32 {
    // SAFETY: `info` is a live struct whose size field is set, as the call
    // requires; RtlGetVersion always succeeds.
    unsafe {
        let mut info: OSVERSIONINFOW = zeroed();
        info.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
        RtlGetVersion(&mut info);
        info.dwBuildNumber
    }
}
