//! Saved operations (D19): before a running copy, move or delete changes
//! its first file, the queue writes it to the saved file, and it removes it
//! when the operation ends. At the next start each saved operation is
//! offered as interrupted, for Continue or Dismiss.
//! Synchronization saves only its roots and Mirror intent; Continue requires
//! a fresh comparison and explicit commitment, never stale deletion approval.
//!
//! The keys are a saved format: an older entry that lacks a key keeps its
//! old meaning, as `AGENTS.md` asks of the wire, so a new key gets
//! `#[serde(default)]` with that meaning.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conflicts::Policy;
use crate::operation::{Destination, Request};
use crate::settings::{
    CopyWorkers, Count, DeleteWorkers, Engine, LargeFileLimit, RingBlock, RingBuffer, RingHardDisk,
    RingSsd, SettingError, SpeedSettings,
};

/// One saved operation, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    kind: Kind,
    sources: Vec<SavedPath>,
    /// The folder a copy or move keeps the entries' names in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<SavedPath>,
    /// The one entry's folder and new name, instead of `destination`, so
    /// v0.4.0, which reads no `named` and needs `destination`, leaves the
    /// operation out instead of continuing it under the entry's old name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    named: Option<SavedNamed>,
    /// Legacy entries without a choice use the current default after recovery checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    choice: Option<SavedChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mirror: Option<bool>,
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
    pub(crate) started: Option<i64>,
    pub(crate) settings: SpeedSettings,
}

impl Saved {
    /// `None` for a new folder, which is one call, so nothing of it is left
    /// to continue, and for a typed destination, which prepare decides
    /// before any run. A rename is a move to a named destination, and is
    /// saved.
    pub(crate) fn new(request: &Request, started: i64, settings: &SpeedSettings) -> Option<Saved> {
        let paths = |paths: &[PathBuf]| paths.iter().map(|path| SavedPath::new(path)).collect();
        let (kind, sources, destination) = match request {
            Request::Synchronize {
                source,
                destination,
                mirror,
            } => {
                return Some(Saved {
                    kind: Kind::Synchronize,
                    sources: vec![SavedPath::new(source)],
                    destination: Some(SavedPath::new(destination)),
                    named: None,
                    choice: None,
                    mirror: Some(*mirror),
                    started: Some(started),
                    settings: SavedSettings::from(settings),
                });
            }
            Request::Copy {
                sources,
                destination,
                ..
            } => (Kind::Copy, paths(sources), Some(destination)),
            Request::Move {
                sources,
                destination,
                ..
            } => (Kind::Move, paths(sources), Some(destination)),
            Request::Delete { sources } => (Kind::Delete, paths(sources), None),
            Request::NewFolder { .. } => return None,
        };
        let (destination, named) = match destination {
            Some(Destination::Folder(folder)) => (Some(SavedPath::new(folder)), None),
            Some(Destination::Named { folder, name }) => (
                None,
                Some(SavedNamed {
                    folder: SavedPath::new(folder),
                    name: SavedPath::new(Path::new(name)),
                }),
            ),
            Some(Destination::Typed(_)) => return None,
            None => (None, None),
        };
        Some(Saved {
            kind,
            sources,
            destination,
            named,
            choice: request.policy().map(SavedChoice::from),
            mirror: None,
            started: Some(started),
            settings: SavedSettings::from(settings),
        })
    }

    /// `None` when the settings are out of their ranges, as a hand-edited
    /// file can make them, a copy or move has no destination, or a new name
    /// is not for exactly one copied or moved entry.
    pub(crate) fn restore(&self) -> Option<Restored> {
        let settings = SpeedSettings::try_from(&self.settings).ok()?;
        let sources: Vec<PathBuf> = self.sources.iter().map(SavedPath::path).collect();
        let destination = match (&self.destination, &self.named) {
            (Some(folder), None) => Some(Destination::Folder(folder.path())),
            (None, Some(named)) if sources.len() == 1 => Some(Destination::Named {
                folder: named.folder.path(),
                name: named.name.path().into_os_string(),
            }),
            (Some(_), Some(_)) | (None, Some(_)) => return None,
            (None, None) => None,
        };
        let request = match (self.kind, destination) {
            (Kind::Synchronize, Some(Destination::Folder(destination))) if sources.len() == 1 => {
                Request::Synchronize {
                    source: sources.into_iter().next()?,
                    destination,
                    mirror: self.mirror.unwrap_or_default(),
                }
            }
            (Kind::Synchronize, _) => return None,
            (Kind::Copy, Some(destination)) => Request::Copy {
                policy: self.choice.map(Policy::from).unwrap_or_default(),
                sources,
                destination,
            },
            (Kind::Move, Some(destination)) => Request::Move {
                policy: self.choice.map(Policy::from).unwrap_or_default(),
                sources,
                destination,
            },
            (Kind::Delete, _) if self.named.is_some() => return None,
            (Kind::Delete, _) => Request::Delete { sources },
            (Kind::Copy | Kind::Move, None) => return None,
        };
        Some(Restored {
            request,
            started: self.started,
            settings,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Synchronize,
    Copy,
    Move,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SavedChoice {
    UpdateChanged,
    UpdateNewer,
    Replace,
    Skip,
    KeepBoth,
}

impl From<Policy> for SavedChoice {
    fn from(choice: Policy) -> Self {
        match choice {
            Policy::UpdateChanged => SavedChoice::UpdateChanged,
            Policy::UpdateNewer => SavedChoice::UpdateNewer,
            Policy::Replace => SavedChoice::Replace,
            Policy::Skip => SavedChoice::Skip,
            Policy::KeepBoth => SavedChoice::KeepBoth,
        }
    }
}

impl From<SavedChoice> for Policy {
    fn from(choice: SavedChoice) -> Self {
        match choice {
            SavedChoice::UpdateChanged => Policy::UpdateChanged,
            SavedChoice::UpdateNewer => Policy::UpdateNewer,
            SavedChoice::Replace => Policy::Replace,
            SavedChoice::Skip => Policy::Skip,
            SavedChoice::KeepBoth => Policy::KeepBoth,
        }
    }
}

/// A copy or move of one entry to a new name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedNamed {
    folder: SavedPath,
    name: SavedPath,
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
            policy: Policy::KeepBoth,
            sources: vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b c")],
            destination: Destination::Folder(PathBuf::from(r"D:\out")),
        };
        const STARTED: i64 = 134_000_000_000_000_000;
        let saved = Saved::new(&request, STARTED, &settings).expect("saved");
        let text = serde_json::to_string(&saved).expect("serialize");
        let read: Saved = serde_json::from_str(&text).expect("parse");
        let restored = read.restore().expect("restore");
        assert_eq!(restored.settings, settings);
        assert_eq!(restored.request, request);
        assert_eq!(restored.request.policy(), Some(Policy::KeepBoth));
        assert_eq!(restored.started, Some(STARTED));

        let mut damaged = read.clone();
        damaged.settings.workers_ssd = 0;
        assert_eq!(damaged.restore(), None);

        let older = text.replace(&format!(r#""started":{STARTED},"#), "");
        assert_ne!(older, text);
        let older: Saved = serde_json::from_str(&older).expect("parse the older entry");
        assert_eq!(older.restore().expect("restore").started, None);
    }

    #[test]
    fn legacy_policies_keep_their_recovery_meaning() {
        let settings = SpeedSettings::automatic();
        let request = Request::Copy {
            sources: vec![PathBuf::from(r"C:\a.txt")],
            destination: Destination::Folder(PathBuf::from(r"D:\out")),
            policy: Policy::UpdateChanged,
        };
        let saved = Saved::new(&request, 1, &settings).unwrap();
        let value = serde_json::to_value(saved).unwrap();
        for (legacy, expected) in [
            (None, Policy::UpdateChanged),
            (Some("replace"), Policy::Replace),
            (Some("skip"), Policy::Skip),
            (Some("keep-both"), Policy::KeepBoth),
        ] {
            let mut value = value.clone();
            let fields = value.as_object_mut().unwrap();
            fields.remove("choice");
            if let Some(legacy) = legacy {
                fields.insert("choice".into(), legacy.into());
            }
            let saved: Saved = serde_json::from_value(value).unwrap();
            assert_eq!(saved.restore().unwrap().request.policy(), Some(expected));
        }
    }

    /// One entry's new name reads back, so Continue writes the file it
    /// wrote; a new name for two entries, as a hand-edited file can hold,
    /// is refused. It has no `destination`, so v0.4.0, which ignores the new
    /// name, leaves it out instead of writing the entry's old name (#111).
    #[test]
    fn a_new_name_is_saved() {
        let settings = SpeedSettings::automatic();
        let request = Request::Copy {
            policy: crate::Policy::default(),
            sources: vec![PathBuf::from(r"C:\report.pdf")],
            destination: Destination::Named {
                folder: PathBuf::from(r"D:\out"),
                name: "report-old.pdf".into(),
            },
        };
        let saved = Saved::new(&request, 1, &settings).expect("saved");
        let text = serde_json::to_string(&saved).expect("serialize");
        assert!(!text.contains(r#""destination""#), "{text}");
        let read: Saved = serde_json::from_str(&text).expect("parse");
        assert_eq!(read.restore().expect("restore").request, request);

        let two = text.replace(r#""C:\\report.pdf""#, r#""C:\\a","C:\\b""#);
        assert_ne!(two, text);
        let two: Saved = serde_json::from_str(&two).expect("parse");
        assert_eq!(two.restore(), None);
    }
}
