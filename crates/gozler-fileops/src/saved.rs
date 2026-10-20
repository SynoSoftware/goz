//! Saved operations (D19): before a running copy, move or delete changes
//! its first file, the queue writes it to the saved file, and it removes it
//! when the operation ends. At the next start each saved operation is
//! offered as interrupted, for Continue or Dismiss.
//!
//! The keys are a saved format: an older entry that lacks a key keeps its
//! old meaning, as `AGENTS.md` asks of the wire, so a new key gets
//! `#[serde(default)]` with that meaning.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conflicts::Choice;
use crate::operation::Request;
use crate::settings::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, RingBlock, RingBuffer, RingHardDisk,
    RingSsd, SettingError, SpeedSettings,
};

/// One saved operation, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    kind: Kind,
    sources: Vec<SavedPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<SavedPath>,
    /// No key: the person was not asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    choice: Option<SavedChoice>,
    /// When the operation first ran, as a `FILETIME`, kept across Continue.
    /// No key: saved before it was recorded, so Continue finds no file
    /// unfinished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<i64>,
    settings: SavedSettings,
}

/// What a saved operation restores to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Restored {
    pub(crate) request: Request,
    pub(crate) choice: Option<Choice>,
    pub(crate) started: Option<i64>,
    pub(crate) settings: SpeedSettings,
}

impl Saved {
    /// `None` for a rename or a new folder, which are one call each, so
    /// nothing of them is left to continue.
    pub(crate) fn new(
        request: &Request,
        choice: Option<Choice>,
        started: i64,
        settings: &SpeedSettings,
    ) -> Option<Saved> {
        let paths = |paths: &[PathBuf]| paths.iter().map(|path| SavedPath::new(path)).collect();
        let (kind, sources, destination) = match request {
            Request::Copy {
                sources,
                destination,
            } => (
                Kind::Copy,
                paths(sources),
                Some(SavedPath::new(destination)),
            ),
            Request::Move {
                sources,
                destination,
            } => (
                Kind::Move,
                paths(sources),
                Some(SavedPath::new(destination)),
            ),
            Request::Delete { sources } => (Kind::Delete, paths(sources), None),
            Request::Rename { .. } | Request::NewFolder { .. } => return None,
        };
        Some(Saved {
            kind,
            sources,
            destination,
            choice: choice.map(SavedChoice::from),
            started: Some(started),
            settings: SavedSettings::from(settings),
        })
    }

    /// `None` when the settings are out of their ranges, as a hand-edited
    /// file can make them, or a copy or move has no destination.
    pub(crate) fn restore(&self) -> Option<Restored> {
        let settings = SpeedSettings::try_from(&self.settings).ok()?;
        let sources: Vec<PathBuf> = self.sources.iter().map(SavedPath::path).collect();
        let destination = self.destination.as_ref().map(SavedPath::path);
        let request = match (self.kind, destination) {
            (Kind::Copy, Some(destination)) => Request::Copy {
                sources,
                destination,
            },
            (Kind::Move, Some(destination)) => Request::Move {
                sources,
                destination,
            },
            (Kind::Delete, _) => Request::Delete { sources },
            (Kind::Copy | Kind::Move, None) => return None,
        };
        Some(Restored {
            request,
            choice: self.choice.map(Choice::from),
            started: self.started,
            settings,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Copy,
    Move,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SavedChoice {
    Replace,
    Skip,
    KeepBoth,
}

impl From<Choice> for SavedChoice {
    fn from(choice: Choice) -> Self {
        match choice {
            Choice::Replace => SavedChoice::Replace,
            Choice::Skip => SavedChoice::Skip,
            Choice::KeepBoth => SavedChoice::KeepBoth,
        }
    }
}

impl From<SavedChoice> for Choice {
    fn from(choice: SavedChoice) -> Self {
        match choice {
            SavedChoice::Replace => Choice::Replace,
            SavedChoice::Skip => Choice::Skip,
            SavedChoice::KeepBoth => Choice::KeepBoth,
        }
    }
}

/// A path as text, or as its UTF-16 units when it holds an unpaired
/// surrogate, which a Windows name may and JSON text cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum SavedPath {
    Text(String),
    Units { utf16: Vec<u16> },
}

impl SavedPath {
    fn new(path: &Path) -> SavedPath {
        match path.to_str() {
            Some(text) => SavedPath::Text(text.to_owned()),
            None => SavedPath::Units { utf16: units(path) },
        }
    }

    fn path(&self) -> PathBuf {
        match self {
            SavedPath::Text(text) => PathBuf::from(text),
            SavedPath::Units { utf16 } => from_units(utf16),
        }
    }
}

#[cfg(windows)]
fn units(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().collect()
}

#[cfg(windows)]
fn from_units(units: &[u16]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_wide(units))
}

#[cfg(not(windows))]
fn units(path: &Path) -> Vec<u16> {
    path.to_string_lossy().encode_utf16().collect()
}

#[cfg(not(windows))]
fn from_units(units: &[u16]) -> PathBuf {
    PathBuf::from(String::from_utf16_lossy(units))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SavedEngine {
    Windows,
    Ring,
}

/// The speed settings as plain numbers, checked by their constructors when
/// they are read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SavedSettings {
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

const MIB: u64 = 1024 * 1024;

impl From<&SpeedSettings> for SavedSettings {
    fn from(settings: &SpeedSettings) -> Self {
        let engine = |engine| match engine {
            Engine::Windows => SavedEngine::Windows,
            Engine::Ring => SavedEngine::Ring,
        };
        let count = |count: Count| count.get() as u32;
        let mib = |bytes: u64| (bytes / MIB) as u32;
        SavedSettings {
            large_file_limit_mib: mib(settings.large_file_limit.bytes()),
            large_between_ssds: engine(settings.large_between_ssds),
            hard_disk: engine(settings.hard_disk),
            workers_ssd: count(settings.workers.ssd),
            workers_usb: count(settings.workers.usb),
            workers_hard_disk: count(settings.workers.hard_disk),
            workers_share: count(settings.workers.share),
            ring_ssd_block: settings.ring_ssd.block.bytes(),
            ring_ssd_in_flight: count(settings.ring_ssd.in_flight),
            ring_hard_disk_block: settings.ring_hard_disk.block.bytes(),
            ring_hard_disk_in_flight: count(settings.ring_hard_disk.in_flight),
            ring_hard_disk_buffer_mib: mib(settings.ring_hard_disk.buffer.bytes()),
            delete_workers: match settings.delete_workers {
                DeleteWorkers::PerProcessor => None,
                DeleteWorkers::Fixed(count) => Some(count.get() as u32),
            },
        }
    }
}

impl TryFrom<&SavedSettings> for SpeedSettings {
    type Error = SettingError;

    fn try_from(saved: &SavedSettings) -> Result<Self, SettingError> {
        let engine = |engine| match engine {
            SavedEngine::Windows => Engine::Windows,
            SavedEngine::Ring => Engine::Ring,
        };
        Ok(SpeedSettings {
            large_file_limit: LargeFileLimit::from_mib(saved.large_file_limit_mib)?,
            large_between_ssds: engine(saved.large_between_ssds),
            hard_disk: engine(saved.hard_disk),
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
}

#[derive(Serialize, Deserialize)]
struct SavedFile<T> {
    #[serde(default)]
    operations: Vec<T>,
}

/// Reads the saved file. A missing file holds nothing, and an entry that
/// does not parse or restore is left out, so one damaged entry does not
/// lose the others.
pub(crate) fn read(path: &Path) -> Vec<(Saved, Restored)> {
    let Ok(text) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_slice::<SavedFile<serde_json::Value>>(&text) else {
        return Vec::new();
    };
    (file.operations.into_iter())
        .filter_map(|value| serde_json::from_value::<Saved>(value).ok())
        .filter_map(|saved| {
            let restored = saved.restore()?;
            Some((saved, restored))
        })
        .collect()
}

/// Writes `operations` to a temporary file, then renames it over the saved
/// file, so a crash leaves the old file or the new one, never a part.
pub(crate) fn write<'a>(
    path: &Path,
    operations: impl Iterator<Item = &'a Saved>,
) -> io::Result<()> {
    let file = SavedFile {
        operations: operations.collect(),
    };
    let text = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved operation reads back with the settings and the start it ran
    /// with, so Continue runs with the same ones; a value out of range is
    /// refused, and an entry saved before the start was recorded still
    /// loads.
    #[test]
    fn a_saved_operation_keeps_its_settings() {
        let mut settings = SpeedSettings::automatic();
        settings.large_file_limit = LargeFileLimit::from_mib(256).expect("in range");
        settings.workers.usb = Count::new(5).expect("in range");
        settings.ring_hard_disk.buffer = RingBuffer::from_mib(512).expect("in range");
        settings.delete_workers = DeleteWorkers::Fixed(Count::new(8).expect("in range"));
        let request = Request::Move {
            sources: vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b c")],
            destination: PathBuf::from(r"D:\out"),
        };
        const STARTED: i64 = 134_000_000_000_000_000;
        let saved =
            Saved::new(&request, Some(Choice::KeepBoth), STARTED, &settings).expect("saved");
        let text = serde_json::to_string(&saved).expect("serialize");
        let read: Saved = serde_json::from_str(&text).expect("parse");
        let restored = read.restore().expect("restore");
        assert_eq!(restored.settings, settings);
        assert_eq!(restored.request, request);
        assert_eq!(restored.choice, Some(Choice::KeepBoth));
        assert_eq!(restored.started, Some(STARTED));

        let mut damaged = read.clone();
        damaged.settings.workers_ssd = 0;
        assert_eq!(damaged.restore(), None);

        let older = text.replace(&format!(r#""started":{STARTED},"#), "");
        assert_ne!(older, text);
        let older: Saved = serde_json::from_str(&older).expect("parse the older entry");
        assert_eq!(older.restore().expect("restore").started, None);
    }
}
