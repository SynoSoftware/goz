//! A volume's class and disks, and the Windows engine's method for a pair
//! of volumes ("The Windows engine" in the spec). The choices build and are
//! tested on every OS; reading a volume is Windows only.

use crate::settings::{Count, SpeedSettings};

/// The four classes that the worker counts and the disk waits read. Ordered
/// from fastest to slowest, so the slower end of a pair decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum VolumeClass {
    Ssd,
    /// A USB bulk-only device without a seek penalty, which serves one
    /// file at a time.
    Usb,
    /// A seek penalty, or one that cannot be read.
    HardDisk,
    /// A network share, by its path. Its seek penalty cannot be read.
    Share,
}

impl VolumeClass {
    /// BetterCopy's test (`profiler.rs`): a USB device that takes one
    /// command at a time is a bulk-only device.
    pub(crate) fn of_local(seek_penalty: Option<bool>, usb_bulk_only: bool) -> VolumeClass {
        match seek_penalty {
            Some(true) | None => VolumeClass::HardDisk,
            Some(false) if usb_bulk_only => VolumeClass::Usb,
            Some(false) => VolumeClass::Ssd,
        }
    }

    /// Whether two copies that move bytes on its disk wait for each other
    /// (D14): only a disk that seeks, and a share, which counts as one.
    pub(crate) fn seeks(self) -> bool {
        matches!(self, VolumeClass::HardDisk | VolumeClass::Share)
    }
}

/// A physical disk, for the disk waits (D14).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Disk {
    Number(u32),
    /// A share counts as one disk, named by its server, folded.
    Server(Vec<u16>),
    /// A volume whose disks cannot be read counts as a disk of its own.
    Volume(Vec<u16>),
}

/// What an operation reads about one volume at prepare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Volume {
    pub(crate) class: VolumeClass,
    pub(crate) disks: Vec<Disk>,
}

/// How the Windows engine copies between a pair of volumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CopyPlan {
    /// Workers for files that are not large.
    pub(crate) workers: usize,
    /// From this size a file is copied unbuffered, one at a time; `None` on
    /// a share, where every file is buffered so the round trips of one file
    /// overlap another's.
    pub(crate) large_from: Option<u64>,
    /// Large files run on their own lane beside the workers, as AngelCOPY's
    /// do. When a volume seeks they join the workers' queue instead, so one
    /// file moves at a time, because a second stream makes a hard disk seek.
    pub(crate) lane_beside: bool,
}

impl CopyPlan {
    /// `pair` is the slower class of the two ends.
    pub(crate) fn new(pair: VolumeClass, settings: &SpeedSettings) -> CopyPlan {
        let workers = |count: Count| count.get();
        let limit = Some(settings.large_file_limit.bytes());
        let w = &settings.workers;
        match pair {
            VolumeClass::Share => CopyPlan {
                workers: workers(w.share),
                large_from: None,
                lane_beside: false,
            },
            VolumeClass::HardDisk => CopyPlan {
                workers: workers(w.hard_disk),
                large_from: limit,
                lane_beside: false,
            },
            VolumeClass::Usb => CopyPlan {
                workers: workers(w.usb),
                large_from: limit,
                lane_beside: true,
            },
            VolumeClass::Ssd => CopyPlan {
                workers: workers(w.ssd),
                large_from: limit,
                lane_beside: true,
            },
        }
    }
}

#[cfg(windows)]
pub(crate) use read::read_volume;

#[cfg(windows)]
mod read {
    use super::{Disk, Volume, VolumeClass};
    use crate::names::fold;

    /// `STORAGE_BUS_TYPE`'s USB.
    const BUS_USB: i32 = 7;

    /// Reads a volume named by an identity's volume part: `\\?\Volume{...}`
    /// or `\\?\UNC\server\share`.
    pub(crate) fn read_volume(volume: &[u16]) -> Volume {
        let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        if let Some(share) = volume.strip_prefix(&unc[..]) {
            let server = share.split(|&unit| unit == u16::from(b'\\')).next();
            return Volume {
                class: VolumeClass::Share,
                disks: vec![Disk::Server(fold(server.unwrap_or(share)))],
            };
        }
        let Ok(storage) = gozler_winfs::volume_storage(volume) else {
            return Volume {
                class: VolumeClass::HardDisk,
                disks: vec![Disk::Volume(volume.to_vec())],
            };
        };
        let usb_bulk_only = storage
            .device
            .is_some_and(|device| device.bus_type == BUS_USB && !device.command_queueing);
        let disks = match storage.disks {
            Some(disks) if !disks.is_empty() => disks.into_iter().map(Disk::Number).collect(),
            _ => vec![Disk::Volume(volume.to_vec())],
        };
        Volume {
            class: VolumeClass::of_local(storage.seek_penalty, usb_bulk_only),
            disks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan for each class: a share buffered on its workers, a hard
    /// disk one file at a time, and USB and SSD with large files beside
    /// their workers. A wrong plan makes a hard disk seek between files or
    /// copies a share's large files one at a time unbuffered.
    #[test]
    fn each_volume_class_gets_its_method() {
        let settings = SpeedSettings::automatic();
        let plan = |class| CopyPlan::new(class, &settings);
        let limit = Some(32 * 1024 * 1024);
        assert_eq!(
            plan(VolumeClass::Share),
            CopyPlan {
                workers: 2,
                large_from: None,
                lane_beside: false
            }
        );
        assert_eq!(
            plan(VolumeClass::HardDisk),
            CopyPlan {
                workers: 1,
                large_from: limit,
                lane_beside: false
            }
        );
        assert_eq!(
            plan(VolumeClass::Usb),
            CopyPlan {
                workers: 2,
                large_from: limit,
                lane_beside: true
            }
        );
        assert_eq!(
            plan(VolumeClass::Ssd),
            CopyPlan {
                workers: 16,
                large_from: limit,
                lane_beside: true
            }
        );
        assert_eq!(VolumeClass::of_local(None, false), VolumeClass::HardDisk);
        assert_eq!(VolumeClass::of_local(Some(false), true), VolumeClass::Usb);
        assert_eq!(VolumeClass::Ssd.max(VolumeClass::Share), VolumeClass::Share);
    }
}
