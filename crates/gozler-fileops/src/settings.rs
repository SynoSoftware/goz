//! The speed settings: the values every copy, move and delete runs with.
//!
//! Each value is its own type whose constructor refuses a value out of its
//! range, so a bad value from the Settings page or from `ui.json` never
//! reaches an operation.

/// A setting's value that its range refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SettingError {
    #[error("{setting} is {value}; it must be from {min} to {max}")]
    OutOfRange {
        setting: &'static str,
        value: u32,
        min: u32,
        max: u32,
    },
    /// A power of two keeps every block aligned to any sector size.
    #[error("a ring block of {0} bytes is not a power of two")]
    BlockNotPowerOfTwo(u32),
}

fn in_range(setting: &'static str, value: u32, min: u32, max: u32) -> Result<u32, SettingError> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(SettingError::OutOfRange {
            setting,
            value,
            min,
            max,
        })
    }
}

const MIB: u32 = 1024 * 1024;

/// From this size, a file between two local volumes is large.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeFileLimit(u32);

impl LargeFileLimit {
    pub fn from_mib(mib: u32) -> Result<Self, SettingError> {
        in_range("the large-file limit in MiB", mib, 1, 1024).map(Self)
    }

    pub fn bytes(self) -> u64 {
        u64::from(self.0) * u64::from(MIB)
    }
}

/// Which engine copies one of the two cases where the engines differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `CopyFile2`.
    Windows,
    /// Gozler's own unbuffered ring.
    Ring,
}

/// How many workers, or how many blocks in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count(u32);

impl Count {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 64;

    pub fn new(count: u32) -> Result<Self, SettingError> {
        in_range("a worker or block count", count, Self::MIN, Self::MAX).map(Self)
    }

    pub fn get(self) -> usize {
        self.0 as usize
    }
}

/// How many files the Windows copy function copies at once, for files that
/// are not large, by the volume class at either end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyWorkers {
    pub ssd: Count,
    pub usb: Count,
    pub hard_disk: Count,
    pub share: Count,
}

/// The ring's block size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingBlock(u32);

impl RingBlock {
    pub const MIN_BYTES: u32 = 64 * 1024;
    pub const MAX_BYTES: u32 = 64 * MIB;

    pub fn from_bytes(bytes: u32) -> Result<Self, SettingError> {
        let bytes = in_range(
            "a ring block in bytes",
            bytes,
            Self::MIN_BYTES,
            Self::MAX_BYTES,
        )?;
        if !bytes.is_power_of_two() {
            return Err(SettingError::BlockNotPowerOfTwo(bytes));
        }
        Ok(Self(bytes))
    }

    pub fn bytes(self) -> u32 {
        self.0
    }
}

/// The hard-disk profile's buffer, shared by all files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingBuffer(u32);

impl RingBuffer {
    pub fn from_mib(mib: u32) -> Result<Self, SettingError> {
        in_range("the ring buffer in MiB", mib, 16, 1024).map(Self)
    }

    pub fn bytes(self) -> u64 {
        u64::from(self.0) * u64::from(MIB)
    }
}

/// The ring's SSD profile: blocks in flight, each reading or writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingSsd {
    pub block: RingBlock,
    pub in_flight: Count,
}

/// The ring's hard-disk profile: `in_flight` reads and as many writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingHardDisk {
    pub block: RingBlock,
    pub in_flight: Count,
    pub buffer: RingBuffer,
}

/// How many workers delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteWorkers {
    PerProcessor,
    Fixed(Count),
}

impl DeleteWorkers {
    pub fn get(self) -> usize {
        match self {
            DeleteWorkers::PerProcessor => {
                std::thread::available_parallelism().map_or(1, usize::from)
            }
            DeleteWorkers::Fixed(count) => count.get(),
        }
    }
}

/// Every speed setting. A saved operation keeps its settings, so Continue
/// runs with the same ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeedSettings {
    pub large_file_limit: LargeFileLimit,
    pub large_between_ssds: Engine,
    pub hard_disk: Engine,
    pub workers: CopyWorkers,
    pub ring_ssd: RingSsd,
    pub ring_hard_disk: RingHardDisk,
    pub delete_workers: DeleteWorkers,
}

impl SpeedSettings {
    /// The default preset: the Parallel copy preset's values, with the Ring
    /// copy preset's ring values, which stage D's measurements on an SSD
    /// confirmed (`docs/BENCHMARKS.md`).
    pub fn automatic() -> Self {
        SpeedSettings {
            large_file_limit: LargeFileLimit(32),
            large_between_ssds: Engine::Windows,
            hard_disk: Engine::Windows,
            workers: CopyWorkers {
                ssd: Count(16),
                usb: Count(2),
                hard_disk: Count(1),
                share: Count(2),
            },
            ring_ssd: RingSsd {
                block: RingBlock(8 * MIB),
                in_flight: Count(8),
            },
            ring_hard_disk: RingHardDisk {
                block: RingBlock(2 * MIB),
                in_flight: Count(8),
                buffer: RingBuffer(256),
            },
            delete_workers: DeleteWorkers::PerProcessor,
        }
    }
}

/// The presets that the Settings page offers. Each sets every value; a
/// value it does not use keeps the Automatic value. It is shown by what it
/// does, not by the copier it comes from, because Gozler uses none of their
/// code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Automatic,
    /// TinyTorrent2's `Payload::Move`, with BetterCopy's share workers and
    /// rmbrr's delete workers.
    ParallelCopy,
    /// AngelCOPY's ring for large files between SSDs and FastCopy v5's
    /// hard-disk mode for every file when a hard disk is at either end.
    RingCopy,
    /// AngelCOPY 1.6.0: its ring for large files between SSDs, and 16
    /// workers for the other files whatever the disks.
    RingForLargeFiles,
    /// BetterCopy's: the Windows copy function, unbuffered from 256 MiB.
    WindowsUnbufferedFrom256MiB,
}

impl Preset {
    pub fn settings(self) -> SpeedSettings {
        let parallel = CopyWorkers {
            ssd: Count(16),
            usb: Count(2),
            hard_disk: Count(1),
            share: Count(2),
        };
        let windows = |limit_mib| SpeedSettings {
            large_file_limit: LargeFileLimit(limit_mib),
            large_between_ssds: Engine::Windows,
            hard_disk: Engine::Windows,
            workers: parallel,
            delete_workers: DeleteWorkers::PerProcessor,
            ..SpeedSettings::automatic()
        };
        match self {
            Preset::Automatic => SpeedSettings::automatic(),
            Preset::ParallelCopy => windows(32),
            Preset::RingCopy => SpeedSettings {
                large_between_ssds: Engine::Ring,
                hard_disk: Engine::Ring,
                ..windows(32)
            },
            Preset::RingForLargeFiles => SpeedSettings {
                large_between_ssds: Engine::Ring,
                workers: CopyWorkers {
                    ssd: Count(16),
                    usb: Count(16),
                    hard_disk: Count(16),
                    share: Count(16),
                },
                delete_workers: DeleteWorkers::Fixed(Count(8)),
                ..windows(32)
            },
            Preset::WindowsUnbufferedFrom256MiB => windows(256),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each value just outside its range is refused, so no value read from
    /// `ui.json` can reach an operation unchecked.
    #[test]
    fn values_outside_their_ranges_are_refused() {
        assert!(LargeFileLimit::from_mib(0).is_err());
        assert!(LargeFileLimit::from_mib(1025).is_err());
        assert!(Count::new(0).is_err());
        assert!(Count::new(65).is_err());
        assert!(RingBlock::from_bytes(32 * 1024).is_err());
        assert!(RingBlock::from_bytes(128 * MIB).is_err());
        assert!(RingBuffer::from_mib(15).is_err());
        assert!(RingBuffer::from_mib(1025).is_err());
        assert_eq!(
            RingBlock::from_bytes(3 * MIB),
            Err(SettingError::BlockNotPowerOfTwo(3 * MIB))
        );
        assert!(RingBlock::from_bytes(64 * 1024).is_ok());
    }
}
