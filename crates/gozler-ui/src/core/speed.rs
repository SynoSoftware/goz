//! The speed settings that Settings > File operations edits and `ui.json`
//! keeps (file-manager-spec.md: Speed settings). The engine's setting types
//! own every range, so a damaged value in `ui.json` falls back to the
//! default instead of reaching an operation.

use gozler_fileops::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, Preset, RingBlock, RingBuffer,
    RingHardDisk, RingSsd, SettingError, SpeedSettings,
};
use serde::{Deserialize, Serialize};

/// The Speed settings list's choices, in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpeedPreset {
    #[default]
    Automatic,
    ParallelCopy,
    RingCopy,
    RingForLargeFiles,
    WindowsUnbuffered,
    /// The person's own values.
    Custom,
}

impl SpeedPreset {
    pub(crate) const ALL: [SpeedPreset; 6] = [
        SpeedPreset::Automatic,
        SpeedPreset::ParallelCopy,
        SpeedPreset::RingCopy,
        SpeedPreset::RingForLargeFiles,
        SpeedPreset::WindowsUnbuffered,
        SpeedPreset::Custom,
    ];

    /// The catalogue keys of its name in the list and of the sentence under
    /// the list that says what it does.
    pub(crate) fn texts(self) -> (&'static str, &'static str) {
        match self {
            SpeedPreset::Automatic => (
                "settings_file_operations.automatic",
                "settings_file_operations.automatic_description",
            ),
            SpeedPreset::ParallelCopy => (
                "settings_file_operations.parallel_copy",
                "settings_file_operations.parallel_copy_description",
            ),
            SpeedPreset::RingCopy => (
                "settings_file_operations.ring_copy",
                "settings_file_operations.ring_copy_description",
            ),
            SpeedPreset::RingForLargeFiles => (
                "settings_file_operations.ring_for_large_files",
                "settings_file_operations.ring_for_large_files_description",
            ),
            SpeedPreset::WindowsUnbuffered => (
                "settings_file_operations.windows_unbuffered",
                "settings_file_operations.windows_unbuffered_description",
            ),
            SpeedPreset::Custom => (
                "settings_file_operations.custom",
                "settings_file_operations.custom_description",
            ),
        }
    }

    fn preset(self) -> Option<Preset> {
        match self {
            SpeedPreset::Automatic => Some(Preset::Automatic),
            SpeedPreset::ParallelCopy => Some(Preset::ParallelCopy),
            SpeedPreset::RingCopy => Some(Preset::RingCopy),
            SpeedPreset::RingForLargeFiles => Some(Preset::RingForLargeFiles),
            SpeedPreset::WindowsUnbuffered => Some(Preset::WindowsUnbufferedFrom256MiB),
            SpeedPreset::Custom => None,
        }
    }
}

/// The chosen preset and the person's own values, which Custom keeps while
/// another preset is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedSpeed", into = "SavedSpeed")]
pub(crate) struct Speed {
    pub(crate) preset: SpeedPreset,
    pub(crate) custom: SpeedSettings,
}

impl Default for Speed {
    fn default() -> Speed {
        Speed {
            preset: SpeedPreset::Automatic,
            custom: SpeedSettings::automatic(),
        }
    }
}

impl Speed {
    /// The settings that operations run with, and that the page shows.
    pub(crate) fn settings(&self) -> SpeedSettings {
        self.preset.preset().map_or(self.custom, Preset::settings)
    }

    /// Changes one of the shown values, which makes the choice Custom.
    pub(crate) fn change(&mut self, change: impl FnOnce(&mut SpeedSettings)) {
        self.custom = self.settings();
        self.preset = SpeedPreset::Custom;
        change(&mut self.custom);
    }
}

/// Which engine copies one of the two cases where the engines differ, in
/// the order of the lists that show it.
pub(crate) const ENGINES: [Engine; 2] = [Engine::Windows, Engine::Ring];

/// The ring's block sizes, in the order of the lists that show them: each
/// power of two that [`RingBlock`] accepts, 64 KiB to 64 MiB.
pub(crate) fn ring_blocks() -> impl Iterator<Item = RingBlock> {
    (16..=26)
        .map(|power| RingBlock::from_bytes(1 << power).expect("64 KiB to 64 MiB are ring blocks"))
}

/// The range that refused a number box's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutOfRange {
    pub(crate) min: u32,
    pub(crate) max: u32,
}

/// A value of the page that a number box shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpeedNumber {
    LargeFileLimit,
    WorkersSsd,
    WorkersUsb,
    WorkersHardDisk,
    WorkersShare,
    RingSsdInFlight,
    RingHardDiskInFlight,
    RingHardDiskBuffer,
    /// A fixed count; one per logical processor has no number.
    DeleteWorkers,
}

const MIB: u64 = 1024 * 1024;

impl SpeedNumber {
    /// The number that `settings` holds, in the unit the box shows; for
    /// one delete worker per logical processor, that many.
    pub(crate) fn get(self, settings: &SpeedSettings) -> u32 {
        let count = |count: Count| count.get() as u32;
        let mib = |bytes: u64| (bytes / MIB) as u32;
        match self {
            SpeedNumber::LargeFileLimit => mib(settings.large_file_limit.bytes()),
            SpeedNumber::WorkersSsd => count(settings.workers.ssd),
            SpeedNumber::WorkersUsb => count(settings.workers.usb),
            SpeedNumber::WorkersHardDisk => count(settings.workers.hard_disk),
            SpeedNumber::WorkersShare => count(settings.workers.share),
            SpeedNumber::RingSsdInFlight => count(settings.ring_ssd.in_flight),
            SpeedNumber::RingHardDiskInFlight => count(settings.ring_hard_disk.in_flight),
            SpeedNumber::RingHardDiskBuffer => mib(settings.ring_hard_disk.buffer.bytes()),
            SpeedNumber::DeleteWorkers => settings.delete_workers.get() as u32,
        }
    }

    /// Sets the number that `text` holds in `settings`, unless its range
    /// refuses it; text that is not a number is out of every range.
    pub(crate) fn set_text(
        self,
        settings: &mut SpeedSettings,
        text: &str,
    ) -> Result<(), OutOfRange> {
        let value = text.trim().parse().unwrap_or(u32::MAX);
        self.set(settings, value).map_err(|error| match error {
            SettingError::OutOfRange { min, max, .. } => OutOfRange { min, max },
            SettingError::BlockNotPowerOfTwo(_) => {
                unreachable!("no number box holds a ring block")
            }
        })
    }

    /// Sets the number in `settings`, unless its range refuses it.
    pub(crate) fn set(self, settings: &mut SpeedSettings, value: u32) -> Result<(), SettingError> {
        match self {
            SpeedNumber::LargeFileLimit => {
                settings.large_file_limit = LargeFileLimit::from_mib(value)?;
            }
            SpeedNumber::WorkersSsd => settings.workers.ssd = Count::new(value)?,
            SpeedNumber::WorkersUsb => settings.workers.usb = Count::new(value)?,
            SpeedNumber::WorkersHardDisk => settings.workers.hard_disk = Count::new(value)?,
            SpeedNumber::WorkersShare => settings.workers.share = Count::new(value)?,
            SpeedNumber::RingSsdInFlight => settings.ring_ssd.in_flight = Count::new(value)?,
            SpeedNumber::RingHardDiskInFlight => {
                settings.ring_hard_disk.in_flight = Count::new(value)?;
            }
            SpeedNumber::RingHardDiskBuffer => {
                settings.ring_hard_disk.buffer = RingBuffer::from_mib(value)?;
            }
            SpeedNumber::DeleteWorkers => {
                settings.delete_workers = DeleteWorkers::Fixed(Count::new(value)?);
            }
        }
        Ok(())
    }
}

/// [`Speed`] as `ui.json` keeps it: the custom values as plain numbers,
/// which the engine's types check when they are read back.
#[derive(Serialize, Deserialize)]
struct SavedSpeed {
    #[serde(default)]
    preset: SpeedPreset,
    custom: SpeedValues,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SavedEngine {
    Windows,
    Ring,
}

#[derive(Serialize, Deserialize)]
struct SpeedValues {
    large_file_limit_mib: u32,
    large_between_ssds: SavedEngine,
    hard_disk: SavedEngine,
    workers_ssd: u32,
    workers_usb: u32,
    workers_hard_disk: u32,
    workers_share: u32,
    ring_ssd_block: u32,
    ring_ssd_in_flight: u32,
    ring_hard_disk_block: u32,
    ring_hard_disk_in_flight: u32,
    ring_hard_disk_buffer_mib: u32,
    /// No key: one per logical processor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delete_workers: Option<u32>,
}

impl From<Speed> for SavedSpeed {
    fn from(speed: Speed) -> SavedSpeed {
        let settings = &speed.custom;
        let engine = |engine| match engine {
            Engine::Windows => SavedEngine::Windows,
            Engine::Ring => SavedEngine::Ring,
        };
        let number = |number: SpeedNumber| number.get(settings);
        SavedSpeed {
            preset: speed.preset,
            custom: SpeedValues {
                large_file_limit_mib: number(SpeedNumber::LargeFileLimit),
                large_between_ssds: engine(settings.large_between_ssds),
                hard_disk: engine(settings.hard_disk),
                workers_ssd: number(SpeedNumber::WorkersSsd),
                workers_usb: number(SpeedNumber::WorkersUsb),
                workers_hard_disk: number(SpeedNumber::WorkersHardDisk),
                workers_share: number(SpeedNumber::WorkersShare),
                ring_ssd_block: settings.ring_ssd.block.bytes(),
                ring_ssd_in_flight: number(SpeedNumber::RingSsdInFlight),
                ring_hard_disk_block: settings.ring_hard_disk.block.bytes(),
                ring_hard_disk_in_flight: number(SpeedNumber::RingHardDiskInFlight),
                ring_hard_disk_buffer_mib: number(SpeedNumber::RingHardDiskBuffer),
                delete_workers: match settings.delete_workers {
                    DeleteWorkers::PerProcessor => None,
                    DeleteWorkers::Fixed(_) => Some(number(SpeedNumber::DeleteWorkers)),
                },
            },
        }
    }
}

impl From<SavedSpeed> for Speed {
    /// Custom values out of their ranges, as a hand-edited `ui.json` can
    /// hold, are the Automatic values; the chosen preset stays.
    fn from(saved: SavedSpeed) -> Speed {
        Speed {
            preset: saved.preset,
            custom: settings(&saved.custom).unwrap_or_else(|_| SpeedSettings::automatic()),
        }
    }
}

fn settings(saved: &SpeedValues) -> Result<SpeedSettings, SettingError> {
    let engine = |engine: &SavedEngine| match engine {
        SavedEngine::Windows => Engine::Windows,
        SavedEngine::Ring => Engine::Ring,
    };
    Ok(SpeedSettings {
        large_file_limit: LargeFileLimit::from_mib(saved.large_file_limit_mib)?,
        large_between_ssds: engine(&saved.large_between_ssds),
        hard_disk: engine(&saved.hard_disk),
        workers: CopyWorkers {
            ssd: Count::new(saved.workers_ssd)?,
            usb: Count::new(saved.workers_usb)?,
            hard_disk: Count::new(saved.workers_hard_disk)?,
            share: Count::new(saved.workers_share)?,
        },
        ring_ssd: RingSsd {
            block: RingBlock::from_bytes(saved.ring_ssd_block)?,
            in_flight: Count::new(saved.ring_ssd_in_flight)?,
        },
        ring_hard_disk: RingHardDisk {
            block: RingBlock::from_bytes(saved.ring_hard_disk_block)?,
            in_flight: Count::new(saved.ring_hard_disk_in_flight)?,
            buffer: RingBuffer::from_mib(saved.ring_hard_disk_buffer_mib)?,
        },
        delete_workers: match saved.delete_workers {
            None => DeleteWorkers::PerProcessor,
            Some(count) => DeleteWorkers::Fixed(Count::new(count)?),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The person's values survive a restart, and a value out of its range
    /// in a hand-edited `ui.json` never reaches an operation.
    #[test]
    fn saved_values_read_back_and_damaged_ones_fall_back() {
        let mut speed = Speed::default();
        speed.change(|settings| {
            settings.workers.usb = Count::new(5).expect("in range");
            settings.delete_workers = DeleteWorkers::Fixed(Count::new(8).expect("in range"));
        });
        let json = serde_json::to_string(&speed).expect("serializes");
        let read: Speed = serde_json::from_str(&json).expect("reads back");
        assert_eq!(read, speed);

        let damaged = json.replace("\"workers_ssd\":16", "\"workers_ssd\":0");
        assert_ne!(damaged, json);
        let read: Speed = serde_json::from_str(&damaged).expect("reads back");
        assert_eq!(read.preset, SpeedPreset::Custom);
        assert_eq!(read.custom, SpeedSettings::automatic());
    }
}
