//! A volume's class and disks, and the method for a pair of volumes ("The
//! Windows engine" and "The ring engine" in the spec). The choices build and
//! are tested on every OS; reading a volume is Windows only.

use crate::ring_plan::{Layout, RingPlan};
use crate::settings::{Count, Engine, SpeedSettings};

/// The four classes that decide copy plans. Ordered
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
}

/// A physical device counted once in a transfer's admission.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// How a copy runs between a pair of volumes.
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
    /// The ring's profile for this pair: the files the settings give it, and
    /// the files Continue resumes. A share is never the ring's: buffered
    /// reads from a share read ahead, and AngelCOPY measured its ring 30
    /// times slower between shares.
    pub(crate) ring: Option<RingPlan>,
}

impl CopyPlan {
    /// `pair` is the slower class of the two ends, and `layout` says whether
    /// they share a physical disk.
    pub(crate) fn new(pair: VolumeClass, settings: &SpeedSettings, layout: Layout) -> CopyPlan {
        let workers = |count: Count| count.get();
        let limit = settings.large_file_limit.bytes();
        let w = &settings.workers;
        match pair {
            VolumeClass::Share => CopyPlan {
                workers: workers(w.share),
                large_from: None,
                lane_beside: false,
                ring: None,
            },
            VolumeClass::HardDisk => CopyPlan {
                workers: workers(w.hard_disk),
                large_from: Some(limit),
                lane_beside: false,
                ring: Some(RingPlan::hard_disk(
                    settings.ring_hard_disk,
                    layout,
                    settings.hard_disk == Engine::Ring,
                )),
            },
            VolumeClass::Usb | VolumeClass::Ssd => CopyPlan {
                workers: workers(match pair {
                    VolumeClass::Usb => w.usb,
                    _ => w.ssd,
                }),
                large_from: Some(limit),
                lane_beside: true,
                ring: Some(RingPlan::ssd(
                    settings.ring_ssd,
                    (settings.large_between_ssds == Engine::Ring).then_some(limit),
                )),
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
    use crate::settings::Preset;

    /// The plan for each class: a share buffered on its workers, a hard
    /// disk one file at a time, and USB and SSD with large files beside
    /// their workers; with the ring chosen, a hard disk's every file in
    /// FastCopy's profile, an SSD's large files in AngelCOPY's, and a
    /// share's none. Without the ring chosen, the ring's profile stays for
    /// the files Continue resumes, and gets no other file. A wrong plan
    /// makes a hard disk seek between files, copies a share's large files
    /// one at a time unbuffered, reads a share unbuffered through the ring,
    /// or gives the ring files the settings give the Windows engine.
    #[test]
    fn each_volume_class_gets_its_method() {
        let settings = SpeedSettings::automatic();
        let plan = |class| CopyPlan::new(class, &settings, Layout::TwoDisks);
        let limit = 32 * 1024 * 1024;
        assert_eq!(
            plan(VolumeClass::Share),
            CopyPlan {
                workers: 2,
                large_from: None,
                lane_beside: false,
                ring: None,
            }
        );
        assert_eq!(
            plan(VolumeClass::HardDisk),
            CopyPlan {
                workers: 1,
                large_from: Some(limit),
                lane_beside: false,
                ring: Some(RingPlan::hard_disk(
                    settings.ring_hard_disk,
                    Layout::TwoDisks,
                    false
                )),
            }
        );
        let resumes_only = Some(RingPlan::ssd(settings.ring_ssd, None));
        assert_eq!(
            plan(VolumeClass::Usb),
            CopyPlan {
                workers: 2,
                large_from: Some(limit),
                lane_beside: true,
                ring: resumes_only,
            }
        );
        assert_eq!(
            plan(VolumeClass::Ssd),
            CopyPlan {
                workers: 16,
                large_from: Some(limit),
                lane_beside: true,
                ring: resumes_only,
            }
        );
        let ring = Preset::RingCopy.settings();
        let ring_plan = |class, layout| CopyPlan::new(class, &ring, layout).ring;
        assert_eq!(ring_plan(VolumeClass::Share, Layout::OneDisk), None);
        let hard_disk = ring_plan(VolumeClass::HardDisk, Layout::OneDisk).expect("the ring");
        assert_eq!(
            (hard_disk.from, hard_disk.one_disk, hard_disk.buffer),
            (Some(0), true, 256 * 1024 * 1024)
        );
        let ssd = ring_plan(VolumeClass::Ssd, Layout::OneDisk).expect("the ring");
        assert_eq!(
            (ssd.from, ssd.one_disk, ssd.buffer, ssd.open_files),
            (Some(limit), false, 64 * 1024 * 1024, 1)
        );
        assert_eq!(VolumeClass::of_local(None, false), VolumeClass::HardDisk);
        assert_eq!(VolumeClass::of_local(Some(false), true), VolumeClass::Usb);
        assert_eq!(VolumeClass::Ssd.max(VolumeClass::Share), VolumeClass::Share);
    }
}
