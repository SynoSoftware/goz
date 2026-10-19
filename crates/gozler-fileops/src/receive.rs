//! What an operation receives: the selected entries, each read from the
//! disk with its identity (D12), and the destination.

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Reparse, open_entry};

use crate::identity::{Identity, PathTrie, outermost};
use crate::operation::{Failure, Refusal};
use crate::win32::is_gone;

/// A selected entry that exists and whose identity was read.
#[derive(Debug)]
pub(crate) struct Selected {
    /// The path as the window gave it, for the result.
    pub(crate) path: Vec<u16>,
    /// The path to open: with the `\\?\` or `\\?\UNC\` prefix.
    pub(crate) verbatim: Vec<u16>,
    pub(crate) identity: Identity,
}

#[derive(Debug, Default)]
pub(crate) struct Received {
    /// The outermost selected entries, in selection order.
    pub(crate) entries: Vec<Selected>,
    /// Selected entries that no longer exist.
    pub(crate) missing: u64,
    /// Selected entries whose identity cannot be read. The operation does
    /// not touch them.
    pub(crate) failures: Vec<Failure>,
}

/// Reads each selected entry and the destination. An entry below another
/// selected folder, or a second spelling of one, is dropped; a destination
/// inside a selected folder is refused, as File Explorer refuses it. Stops
/// early, with what it has read, when `cancel` is set.
pub(crate) fn receive(
    sources: &[PathBuf],
    destination: Option<&Path>,
    cancel: &AtomicBool,
) -> Result<Received, Refusal> {
    let mut received = Received::default();
    let mut selected = Vec::new();
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let path: Vec<u16> = source.as_os_str().encode_wide().collect();
        let verbatim = verbatim(&path);
        let identity =
            open_entry(&verbatim, Access::Inspect, Reparse::Open).and_then(|e| e.final_path());
        match identity {
            Ok(identity) => selected.push(Selected {
                path,
                verbatim,
                identity: Identity::new(identity),
            }),
            Err(error) if is_gone(error.code) => received.missing += 1,
            Err(error) => received.failures.push(failure(&path, error.code)),
        }
    }
    let identities: Vec<Identity> = selected.iter().map(|s| s.identity.clone()).collect();
    let (kept, trie) = outermost(&identities);
    if let Some(destination) = destination {
        check_destination(destination, &trie)?;
    }
    let mut selected: Vec<Option<Selected>> = selected.into_iter().map(Some).collect();
    received.entries = kept
        .into_iter()
        .map(|i| selected[i].take().expect("each kept index is distinct"))
        .collect();
    Ok(received)
}

/// The destination is a folder the person chose, so it is opened following
/// links, and its identity is its target's.
fn check_destination(destination: &Path, sources: &PathTrie) -> Result<(), Refusal> {
    let path: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let identity = open_entry(&verbatim(&path), Access::Inspect, Reparse::Follow)
        .and_then(|e| e.final_path())
        .map_err(|error| {
            if is_gone(error.code) {
                Refusal::DestinationMissing
            } else {
                Refusal::DestinationUnreadable(error.code)
            }
        })?;
    if sources.covers(&Identity::new(identity)) {
        return Err(Refusal::DestinationInsideSource);
    }
    Ok(())
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

pub(crate) fn failure(path: &[u16], error: u32) -> Failure {
    Failure {
        path: PathBuf::from(std::ffi::OsString::from_wide(path)),
        error,
    }
}
