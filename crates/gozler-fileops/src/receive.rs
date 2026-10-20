//! What an operation receives: the selected entries, each read from the
//! disk with its identity (D12), and the destination.

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Entry, Reparse, open_entry};

use crate::identity::{Identity, PathTrie, outermost};
use crate::listing::read_names;
use crate::names::Names;
use crate::operation::{Listed, Reason, Refusal};
use crate::win32::{FILE_ATTRIBUTE_DIRECTORY, is_gone};

/// A selected entry that exists and whose identity was read.
#[derive(Debug)]
pub(crate) struct Selected {
    /// The path as the window gave it, for the result.
    pub(crate) path: Vec<u16>,
    /// The path to open: with the `\\?\` or `\\?\UNC\` prefix.
    pub(crate) verbatim: Vec<u16>,
    pub(crate) identity: Identity,
    /// It had the folder attribute when it was received; the walks read
    /// what it is again when they reach it.
    pub(crate) folder: bool,
}

/// The folder a copy or a move writes into, opened following links.
pub(crate) struct Destination {
    pub(crate) verbatim: Vec<u16>,
    /// The target's identity, when the destination is a link.
    pub(crate) identity: Identity,
    pub(crate) entry: Entry,
}

impl Destination {
    /// Its names now. Each call lists it through a handle of its own,
    /// because a handle's listing goes on where its last read stopped, so a
    /// second listing of one handle would find the folder empty.
    pub(crate) fn names(&self, page: &mut [u64]) -> Result<Names, u32> {
        let folder = self
            .entry
            .reopen(Access::List)
            .map_err(|error| error.code)?;
        read_names(&folder, page)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Received {
    /// The outermost selected entries, in selection order.
    pub(crate) entries: Vec<Selected>,
    /// Selected entries that no longer exist, as the window gave them.
    pub(crate) missing: Vec<Vec<u16>>,
    /// Selected entries whose identity cannot be read. The operation does
    /// not touch them.
    pub(crate) failures: Vec<Listed>,
}

/// Reads each selected entry and the destination. An entry below another
/// selected folder, or a second spelling of one, is dropped; a destination
/// inside a selected folder is refused, as File Explorer refuses it. Stops
/// early, with what it has read, when `cancel` is set.
pub(crate) fn receive(
    sources: &[PathBuf],
    destination: Option<&Path>,
    cancel: &AtomicBool,
) -> Result<(Received, Option<Destination>), Refusal> {
    let mut received = Received::default();
    let mut selected = Vec::new();
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let path: Vec<u16> = source.as_os_str().encode_wide().collect();
        let verbatim = verbatim(&path);
        let read = open_entry(&verbatim, Access::Inspect, Reparse::Open)
            .and_then(|entry| Ok((entry.final_path()?, entry.attribute_tag()?)));
        match read {
            Ok((identity, tag)) => selected.push(Selected {
                path,
                verbatim,
                identity: Identity::new(identity),
                folder: tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            }),
            Err(error) if is_gone(error.code) => received.missing.push(path),
            Err(error) => received.failures.push(failure(&path, error.code)),
        }
    }
    let identities: Vec<Identity> = selected.iter().map(|s| s.identity.clone()).collect();
    let (kept, trie) = outermost(&identities);
    let destination = match destination {
        Some(destination) => Some(open_destination(destination, &trie)?),
        None => None,
    };
    let mut selected: Vec<Option<Selected>> = selected.into_iter().map(Some).collect();
    received.entries = kept
        .into_iter()
        .map(|i| selected[i].take().expect("each kept index is distinct"))
        .collect();
    Ok((received, destination))
}

/// The destination is a folder the person chose, so it is opened following
/// links, and its identity is its target's.
fn open_destination(destination: &Path, sources: &PathTrie) -> Result<Destination, Refusal> {
    let path: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let verbatim = verbatim(&path);
    let unreadable = |code| {
        if is_gone(code) {
            Refusal::DestinationMissing
        } else {
            Refusal::DestinationUnreadable(code)
        }
    };
    let entry = open_entry(&verbatim, Access::List, Reparse::Follow)
        .map_err(|error| unreadable(error.code))?;
    let identity = Identity::new(entry.final_path().map_err(|error| unreadable(error.code))?);
    if sources.covers(&identity) {
        return Err(Refusal::DestinationInsideSource);
    }
    Ok(Destination {
        verbatim,
        identity,
        entry,
    })
}

/// `path` with the prefix that lets Windows take it longer than `MAX_PATH`.
/// The prefix also turns off Windows' rewriting of the path, so a name that
/// ends with a dot or a space reaches the file system as it is; a path that
/// is not a full path then fails to open, and the entry is a failure.
pub(crate) fn verbatim(path: &[u16]) -> Vec<u16> {
    let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    if path.starts_with(&units(r"\\?\")) {
        path.to_vec()
    } else if let Some(share) = path.strip_prefix(&units(r"\\")[..]) {
        units(r"\\?\UNC\")
            .into_iter()
            .chain(share.iter().copied())
            .collect()
    } else {
        units(r"\\?\")
            .into_iter()
            .chain(path.iter().copied())
            .collect()
    }
}

/// A path that [`verbatim`] made, as the person writes it.
pub(crate) fn plain(path: &[u16]) -> PathBuf {
    let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let plain = if let Some(share) = path.strip_prefix(&units(r"\\?\UNC\")[..]) {
        units(r"\\")
            .into_iter()
            .chain(share.iter().copied())
            .collect()
    } else {
        path.strip_prefix(&units(r"\\?\")[..])
            .unwrap_or(path)
            .to_vec()
    };
    PathBuf::from(std::ffi::OsString::from_wide(&plain))
}

pub(crate) fn listed(path: &[u16], reason: Reason) -> Listed {
    Listed {
        path: PathBuf::from(std::ffi::OsString::from_wide(path)),
        reason,
    }
}

pub(crate) fn failure(path: &[u16], error: u32) -> Listed {
    listed(path, Reason::Failed(error))
}
