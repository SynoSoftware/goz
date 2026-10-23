//! Comparison and Mirror validation use relative opens under retained roots.
//! Ordinary copy keeps its existing executor; Mirror separately verifies its
//! required output before it removes any preview-approved entry.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_core::layout::FolderEntry;
use gozler_winfs::{Access, Entry, FileIdentity, Reparse, WinError, open_entry};

use super::{Comparison, ComparisonEntry, ComparisonSummary, SyncAction};
use crate::delete::{Disposition, remove};
use crate::identity::{Identity, PathTrie};
use crate::listing::{self, BACKSLASH, LISTING_WORDS, join};
use crate::names::fold;
use crate::operation::{Destination, End, Listed, Pause, Reason, Refusal, Request, Total};
use crate::receive::{plain, verbatim};
use crate::totals::Index;
use crate::tree::same_entry;
use crate::volumes::read_volume;
use crate::waits::Plan;
use crate::win32::{
    ERROR_FILE_NOT_FOUND, ERROR_OPERATION_ABORTED, ERROR_REPARSE_POINT_ENCOUNTERED,
    FILE_ATTRIBUTE_DIRECTORY, is_gone, is_name_surrogate,
};

const ERROR_DIRECTORY: u32 = 267;
const ERROR_FILE_INVALID: u32 = 1006;

pub(crate) struct Roots {
    source: Root,
    destination: Root,
    mirror: bool,
}

struct Root {
    path: Vec<u16>,
    resolved: Identity,
    identity: FileIdentity,
    entry: Entry,
}

pub(crate) struct Review {
    pub(crate) comparison: Arc<Comparison>,
    roots: Roots,
    source: Vec<Node>,
    extras: Vec<Candidate>,
    total: Total,
    totals: HashMap<Identity, Total>,
}

struct Node {
    path: Vec<u16>,
    folder: bool,
    size: u64,
    last_write: i64,
}

struct Candidate {
    node: Node,
    identity: FileIdentity,
    attributes: u32,
}

impl Review {
    pub(crate) fn current(&self) -> Result<(), End> {
        self.roots
            .current()
            .map_err(|_| End::Refused(Refusal::SynchronizationChanged))
    }

    pub(crate) fn mirrors(&self) -> bool {
        self.roots.mirror
    }

    pub(crate) fn total(&self) -> Total {
        self.total
    }

    pub(crate) fn copy_request(&self, cancel: &AtomicBool, pause: &Pause) -> Result<Request, End> {
        let mut sources = Vec::new();
        for node in &self.source {
            stopped(cancel, pause).map_err(|_| End::Cancelled)?;
            if !node.path.contains(&BACKSLASH) {
                sources.push(plain(&join(&self.roots.source.path, &node.path)));
            }
        }
        Ok(Request::Copy {
            policy: crate::Policy::UpdateChanged,
            sources,
            destination: Destination::Folder(plain(&self.roots.destination.path)),
        })
    }

    pub(crate) fn index<'a>(&'a self, pause: &'a Pause) -> impl Index + 'a {
        ComparedTotals {
            totals: &self.totals,
            pause,
        }
    }

    pub(crate) fn verify_output(&self, cancel: &AtomicBool, pause: &Pause) -> Result<(), End> {
        let verify = || {
            self.roots.current()?;
            for node in &self.source {
                stopped(cancel, pause)?;
                let source = self
                    .roots
                    .source
                    .lookup(&node.path, Access::Inspect)?
                    .ok_or(ERROR_FILE_INVALID)?;
                node.check(&source)?;
                let destination = self
                    .roots
                    .destination
                    .lookup(&node.path, Access::Inspect)?
                    .ok_or(ERROR_FILE_INVALID)?;
                node.check(&destination)?;
            }
            self.roots.current()
        };
        verify().map_err(|code| {
            if cancel.load(Ordering::Relaxed) {
                End::Cancelled
            } else if code == ERROR_FILE_INVALID || code == ERROR_REPARSE_POINT_ENCOUNTERED {
                End::Refused(Refusal::SynchronizationChanged)
            } else {
                End::Refused(Refusal::ComparisonFailed(code))
            }
        })
    }

    pub(crate) fn remove_extras(
        &mut self,
        cancel: &AtomicBool,
        pause: &Pause,
        progress: &crate::operation::Progress,
    ) -> (Vec<Listed>, End) {
        self.extras.sort_unstable_by_key(|candidate| {
            std::cmp::Reverse(
                candidate
                    .node
                    .path
                    .iter()
                    .filter(|&&c| c == BACKSLASH)
                    .count(),
            )
        });
        let disposition = Disposition::default();
        let mut listed = Vec::new();
        for candidate in &self.extras {
            if stopped(cancel, pause).is_err() {
                return (listed, End::Cancelled);
            }
            if self.roots.current().is_err() {
                return (listed, End::Refused(Refusal::SynchronizationChanged));
            }
            let path = plain(&join(&self.roots.destination.path, &candidate.node.path));
            let shown: Vec<u16> = path.as_os_str().encode_wide().collect();
            progress.start_file(&shown, None, 0);
            let root_changed = Cell::new(false);
            let result = remove(cancel, &disposition, || {
                let checked = || {
                    stopped(cancel, pause)?;
                    if self.roots.current().is_err() {
                        root_changed.set(true);
                        return Err(ERROR_FILE_INVALID);
                    }
                    if self
                        .roots
                        .source
                        .lookup(&candidate.node.path, Access::Inspect)?
                        .is_some()
                    {
                        return Err(ERROR_FILE_INVALID);
                    }
                    let entry = self
                        .roots
                        .destination
                        .lookup(&candidate.node.path, Access::delete(candidate.attributes))?
                        .ok_or(ERROR_FILE_NOT_FOUND)?;
                    if entry.identity().map_err(|e| e.code)? != candidate.identity {
                        return Err(ERROR_FILE_INVALID);
                    }
                    candidate.node.check(&entry)?;
                    Ok(entry)
                };
                checked().map_err(|code| WinError {
                    code,
                    context: "Mirror revalidation",
                })
            });
            if root_changed.get() {
                return (listed, End::Refused(Refusal::SynchronizationChanged));
            }
            progress.add_items(1);
            if let Err(code) = result {
                let reason =
                    if code == ERROR_FILE_INVALID || code == ERROR_REPARSE_POINT_ENCOUNTERED {
                        Reason::SynchronizationChanged
                    } else {
                        Reason::Failed(code)
                    };
                listed.push(Listed { path, reason });
            }
        }
        let end = if cancel.load(Ordering::Relaxed) {
            End::Cancelled
        } else {
            End::Complete
        };
        (listed, end)
    }
}

impl Root {
    fn open(path: &Path) -> Result<Self, u32> {
        let path = verbatim(&path.as_os_str().encode_wide().collect::<Vec<_>>());
        let entry = open_entry(&path, Access::List, Reparse::Follow).map_err(|e| e.code)?;
        if entry.attribute_tag().map_err(|e| e.code)?.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(ERROR_DIRECTORY);
        }
        Ok(Self {
            path,
            resolved: Identity::new(entry.final_path().map_err(|e| e.code)?),
            identity: entry.identity().map_err(|e| e.code)?,
            entry,
        })
    }

    fn current(&self) -> Result<(), u32> {
        let entry = open_entry(&self.path, Access::Inspect, Reparse::Follow).map_err(|e| e.code)?;
        if entry.identity().map_err(|e| e.code)? != self.identity {
            return Err(ERROR_FILE_INVALID);
        }
        Ok(())
    }

    fn lookup(&self, path: &[u16], access: Access) -> Result<Option<Entry>, u32> {
        let mut parent = None;
        let mut components = path.split(|&c| c == BACKSLASH).peekable();
        while let Some(name) = components.next() {
            let last = components.peek().is_none();
            let folder = parent.as_ref().unwrap_or(&self.entry);
            let entry = match folder.open_child(
                name,
                if last { access } else { Access::List },
                Reparse::Open,
            ) {
                Ok(entry) => entry,
                Err(error) if is_gone(error.code) => return Ok(None),
                Err(error) => return Err(error.code),
            };
            if last {
                return Ok(Some(entry));
            }
            let tag = entry.attribute_tag().map_err(|e| e.code)?;
            if tag.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || is_name_surrogate(tag.reparse_tag)
            {
                return Err(ERROR_REPARSE_POINT_ENCOUNTERED);
            }
            parent = Some(if tag.reparse_tag == 0 {
                entry
            } else {
                let view = folder
                    .open_child(name, Access::List, Reparse::Follow)
                    .map_err(|e| e.code)?;
                same_entry(&entry, &view)?;
                view
            });
        }
        Err(ERROR_DIRECTORY)
    }

    fn listing(&self) -> Result<Entry, u32> {
        let entry = open_entry(&self.path, Access::List, Reparse::Follow).map_err(|e| e.code)?;
        if entry.identity().map_err(|e| e.code)? != self.identity {
            return Err(ERROR_FILE_INVALID);
        }
        Ok(entry)
    }
}

impl Roots {
    pub(crate) fn open(
        source: &Path,
        destination: &Path,
        mirror: bool,
    ) -> Result<(Plan, Self), Refusal> {
        let read = || {
            let source = Root::open(source)?;
            let destination = Root::open(destination)?;
            Ok::<_, u32>((source, destination))
        };
        let (source, destination) = read().map_err(Refusal::ComparisonFailed)?;
        let mut sources = PathTrie::default();
        sources.insert(&source.resolved);
        let mut destinations = PathTrie::default();
        destinations.insert(&destination.resolved);
        if sources.covers(&destination.resolved) || destinations.covers(&source.resolved) {
            return Err(Refusal::SynchronizationRootsOverlap);
        }
        let disks = read_volume(source.resolved.volume())
            .disks
            .into_iter()
            .chain(read_volume(destination.resolved.volume()).disks)
            .collect();
        Ok((
            Plan::new(disks),
            Self {
                source,
                destination,
                mirror,
            },
        ))
    }

    fn current(&self) -> Result<(), u32> {
        self.source.current()?;
        self.destination.current()
    }

    pub(crate) fn compare(self, cancel: &AtomicBool, pause: &Pause) -> Result<Review, End> {
        let compare = || {
            self.current()?;
            let source = enumerate(&self.source, cancel, pause)?;
            let mut destination = enumerate(&self.destination, cancel, pause)?;
            let mut entries = Vec::with_capacity(source.len() + destination.len());
            let mut summary = ComparisonSummary::default();
            let mut required = Vec::with_capacity(source.len());
            for (key, node) in source {
                stopped(cancel, pause)?;
                let action = match destination.remove(&key) {
                    None => {
                        summary.additions += 1;
                        SyncAction::Add
                    }
                    Some(other) if node.folder != other.folder => {
                        summary.conflicts += 1;
                        SyncAction::Conflict
                    }
                    Some(other) if node.matches(&other) => {
                        summary.unchanged += 1;
                        SyncAction::Unchanged
                    }
                    Some(_) => {
                        summary.replacements += 1;
                        SyncAction::Replace
                    }
                };
                entries.push(ComparisonEntry {
                    path: String::from_utf16_lossy(&node.path),
                    action,
                });
                required.push(node);
            }
            let mut extras = Vec::new();
            for node in destination.into_values() {
                stopped(cancel, pause)?;
                summary.extras += 1;
                let action = if self.mirror {
                    SyncAction::Delete
                } else {
                    SyncAction::Keep
                };
                entries.push(ComparisonEntry {
                    path: String::from_utf16_lossy(&node.path),
                    action,
                });
                if self.mirror {
                    let entry = self
                        .destination
                        .lookup(&node.path, Access::Inspect)?
                        .ok_or(ERROR_FILE_INVALID)?;
                    node.check(&entry)?;
                    extras.push(Candidate {
                        node,
                        identity: entry.identity().map_err(|e| e.code)?,
                        attributes: entry.attribute_tag().map_err(|e| e.code)?.attributes,
                    });
                    summary.deletions += 1;
                }
            }
            self.current()?;
            Ok::<_, u32>((entries, summary, required, extras))
        };
        let (entries, summary, source, extras) = compare().map_err(|code| {
            if cancel.load(Ordering::Relaxed) {
                End::Cancelled
            } else {
                End::Refused(Refusal::ComparisonFailed(code))
            }
        })?;
        let mut totals = HashMap::<Identity, Total>::new();
        for node in &source {
            stopped(cancel, pause).map_err(|_| End::Cancelled)?;
            let name = node
                .path
                .split(|&c| c == BACKSLASH)
                .next()
                .expect("a relative path has a name");
            let total = totals.entry(self.source.resolved.child(name)).or_default();
            total.items += 1;
            if !node.folder {
                total.bytes += node.size;
            }
        }
        let total = Total {
            items: source.len() as u64 + extras.len() as u64,
            bytes: source
                .iter()
                .filter(|node| !node.folder)
                .map(|node| node.size)
                .sum(),
        };
        Ok(Review {
            comparison: Arc::new(Comparison { entries, summary }),
            roots: self,
            source,
            extras,
            total,
            totals,
        })
    }
}

struct ComparedTotals<'a> {
    totals: &'a HashMap<Identity, Total>,
    pause: &'a Pause,
}

impl Index for ComparedTotals<'_> {
    fn below(&self, folders: &[&Identity], cancel: &Arc<AtomicBool>) -> Option<Vec<Total>> {
        folders
            .iter()
            .map(|folder| {
                self.pause.wait(cancel);
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let total = self.totals.get(folder)?;
                Some(Total {
                    items: total.items - 1,
                    bytes: total.bytes,
                })
            })
            .collect()
    }
}

impl Node {
    fn matches(&self, other: &Self) -> bool {
        self.folder == other.folder
            && (self.folder || (self.size == other.size && self.last_write == other.last_write))
    }

    fn check(&self, entry: &Entry) -> Result<(), u32> {
        let tag = entry.attribute_tag().map_err(|e| e.code)?;
        if is_name_surrogate(tag.reparse_tag)
            || self.folder != (tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0)
        {
            return Err(ERROR_REPARSE_POINT_ENCOUNTERED);
        }
        if !self.folder {
            let stat = entry.stat().map_err(|e| e.code)?;
            if stat.size != self.size || stat.last_write != self.last_write {
                return Err(ERROR_FILE_INVALID);
            }
        }
        Ok(())
    }
}

struct Frame {
    entry: Entry,
    path: Vec<u16>,
    children: std::vec::IntoIter<FolderEntry>,
}

fn frame(
    entry: Entry,
    path: Vec<u16>,
    cancel: &AtomicBool,
    pause: &Pause,
    page: &mut [u64],
) -> Result<Frame, u32> {
    let mut children = Vec::new();
    let mut failure = None;
    listing::list(
        &entry,
        page,
        || {
            pause.wait(cancel);
            cancel.load(Ordering::Relaxed)
        },
        |listed| match listed {
            Ok(child) => children.push(child),
            Err(code) => failure = Some(code),
        },
    )?;
    stopped(cancel, pause)?;
    if let Some(code) = failure {
        return Err(code);
    }
    Ok(Frame {
        entry,
        path,
        children: children.into_iter(),
    })
}

fn enumerate(
    root: &Root,
    cancel: &AtomicBool,
    pause: &Pause,
) -> Result<BTreeMap<Vec<u16>, Node>, u32> {
    let mut nodes = BTreeMap::new();
    let mut page = vec![0u64; LISTING_WORDS];
    let mut frames = vec![frame(
        root.listing()?,
        Vec::new(),
        cancel,
        pause,
        &mut page,
    )?];
    while let Some(parent) = frames.last_mut() {
        stopped(cancel, pause)?;
        let Some(child) = parent.children.next() else {
            frames.pop();
            continue;
        };
        let path = if parent.path.is_empty() {
            child.name.clone()
        } else {
            join(&parent.path, &child.name)
        };
        let folder = child.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if is_name_surrogate(child.reparse_tag.unwrap_or(0)) {
            return Err(ERROR_REPARSE_POINT_ENCOUNTERED);
        }
        let next = if folder {
            let entry = parent
                .entry
                .open_child(&child.name, Access::List, Reparse::Open)
                .map_err(|e| e.code)?;
            let tag = entry.attribute_tag().map_err(|e| e.code)?;
            if tag.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || is_name_surrogate(tag.reparse_tag)
            {
                return Err(ERROR_REPARSE_POINT_ENCOUNTERED);
            }
            Some(if tag.reparse_tag == 0 {
                entry
            } else {
                let view = parent
                    .entry
                    .open_child(&child.name, Access::List, Reparse::Follow)
                    .map_err(|e| e.code)?;
                same_entry(&entry, &view)?;
                view
            })
        } else {
            None
        };
        let key = fold(&path);
        if nodes
            .insert(
                key,
                Node {
                    path: path.clone(),
                    folder,
                    size: child.size,
                    last_write: child.last_write,
                },
            )
            .is_some()
        {
            return Err(ERROR_FILE_INVALID);
        }
        if let Some(entry) = next {
            frames.push(frame(entry, path, cancel, pause, &mut page)?);
        }
    }
    Ok(nodes)
}

fn stopped(cancel: &AtomicBool, pause: &Pause) -> Result<(), u32> {
    pause.wait(cancel);
    if cancel.load(Ordering::Relaxed) {
        Err(ERROR_OPERATION_ABORTED)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{Scratch, junction};

    #[test]
    fn synchronization_rejects_redirected_output_even_when_both_roots_are_unchanged() {
        let scratch = Scratch::new("sync-redirected-output");
        let source = scratch.write(r"source\a\file.txt", b"source");
        std::fs::create_dir_all(scratch.path(r"destination\a")).expect("destination tree");
        std::fs::create_dir(scratch.path("outside")).expect("outside folder");
        std::fs::copy(source, scratch.path(r"outside\file.txt"))
            .expect("matching file outside the tree");
        let cancel = AtomicBool::new(false);
        let pause = Pause::default();
        let _active = pause.enter(&cancel);
        let (_, roots) = Roots::open(&scratch.path("source"), &scratch.path("destination"), true)
            .expect("disjoint roots");
        let review = roots.compare(&cancel, &pause).expect("complete comparison");
        std::fs::remove_dir(scratch.path(r"destination\a"))
            .expect("replace an intermediate directory");
        junction(&scratch.path(r"destination\a"), &scratch.path("outside"));
        review
            .current()
            .expect("both root identities are unchanged");
        assert_eq!(
            review.verify_output(&cancel, &pause),
            Err(End::Refused(Refusal::SynchronizationChanged))
        );
    }
}
