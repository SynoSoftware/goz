//! Prepare's counts of a copy or a move, so the window can ask the person:
//! the names that the selected entries find taken in the destination
//! folder, the total from gozlerd's index, and the bytes the destination
//! needs. Prepare reads one folder, the destination, and walks none (D5).
//! Measure, which counts a folder outside the queue, is the one walk that
//! counts.
//!
//! The conflicts come from the same rules the run applies ([`classify`]),
//! and a file that Continue finds done needs no space: otherwise a 100 GB
//! copy that crashed at 90 GB on a drive with 120 GB free could not
//! continue.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Entry, Reparse, allocated_size, open_entry};

use crate::conflicts::{Clash, Rule, Wanted, classify};
use crate::copy::Source;
use crate::listing::{LISTING_WORDS, is_folder, join, kind, list};
use crate::names::Holder;
use crate::operation::{Conflicts, Total};
use crate::receive::{Destination, Selected, verbatim};
use crate::totals::{Index, totals};
use crate::tree::same_entry;
use crate::win32::FILE_ATTRIBUTE_SPARSE_FILE;

/// What prepare counts.
pub(crate) struct Surveyed {
    pub(crate) conflicts: Conflicts,
    pub(crate) total: Option<Total>,
    /// The bytes the destination needs; `None` without a total, and when a
    /// selected folder merges into one whose files may already be there. A
    /// sparse file that stays sparse needs its allocated size, because one
    /// copied where it cannot stay sparse takes its full size.
    pub(crate) needed: Option<u64>,
}

/// How prepare counts.
pub(crate) struct Surveying<'a> {
    pub(crate) rule: Rule,
    pub(crate) sparse_stays: bool,
    pub(crate) index: &'a dyn Index,
    pub(crate) cancel: &'a Arc<AtomicBool>,
}

/// Counts `sources` against the names of `destination`. Each source says
/// whether its bytes are copied, as for a copy or a move across volumes; a
/// move by rename moves only its entries.
pub(crate) fn survey(
    sources: &[(&Source, bool)],
    destination: &Destination,
    surveying: &Surveying<'_>,
) -> Result<Surveyed, u32> {
    let mut names = destination.names(&mut vec![0u64; LISTING_WORDS])?;
    let copied: Vec<&Selected> = (sources.iter())
        .filter(|(_, copies)| *copies)
        .map(|(source, _)| &source.selected)
        .collect();
    let mut copied_totals = totals(&copied, surveying.index, surveying.cancel).map(Vec::into_iter);
    let mut total = Some(Total::default());
    let mut needed = Some(0);
    let mut conflicts = Conflicts::default();
    for &(source, copies) in sources {
        let selected = &source.selected;
        let entry_total = if copies {
            (copied_totals.as_mut())
                .map(|each| each.next().expect("one total for each copied entry"))
        } else {
            Some(Total { items: 1, bytes: 0 })
        };
        total = total.zip(entry_total).map(|(sum, entry)| sum + entry);
        // What it needs when it is written under its name or a new one.
        let sparse = copies
            && surveying.sparse_stays
            && !selected.folder()
            && selected.tag.attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0;
        let written = || {
            if sparse {
                Some(allocated_size(&selected.verbatim).unwrap_or(selected.stat.size))
            } else {
                entry_total.map(|entry| entry.bytes)
            }
        };
        let name = selected.identity.name();
        let wanted = Wanted {
            kind: kind(selected.tag.attributes, selected.tag.reparse_tag),
            size: selected.stat.size,
            last_write: selected.stat.last_write,
            own_folder: source.own_folder,
            copied: copies,
            shared: source.shared,
        };
        let clash = classify(&names, name, wanted, surveying.rule);
        let entry_needs = match clash {
            Clash::Done { name } => {
                names.insert(&name, Holder::Ours);
                Some(0)
            }
            Clash::Mismatch => Some(0),
            // Its bytes at the destination already take their space.
            Clash::Unfinished { name, size } => {
                names.insert(&name, Holder::Ours);
                Some(selected.stat.size.saturating_sub(size))
            }
            Clash::Merge => {
                conflicts.merges += 1;
                if copies { None } else { Some(0) }
            }
            // The name becomes the operation's, as the run takes it, so a
            // second selected entry of this name counts as one with the same
            // name.
            Clash::Free => {
                names.insert(name, Holder::Ours);
                written()
            }
            Clash::Conflict => {
                conflicts.files += 1;
                names.insert(name, Holder::Ours);
                written()
            }
            Clash::SameName => {
                conflicts.same_name += 1;
                written()
            }
            Clash::OwnFolder => written(),
        };
        needed = needed.zip(entry_needs).map(|(sum, entry)| sum + entry);
    }
    Ok(Surveyed {
        conflicts,
        total,
        needed,
    })
}

/// What Measure counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measured {
    /// Files and folders below the folder.
    pub items: u64,
    pub bytes: u64,
}

/// Counts the items and bytes below the folder `path`, without following
/// links, as Space does on a folder in a volume that gozlerd does not
/// index. Stops early with what it counted when `cancel` is set. Fails
/// when the folder cannot be opened.
pub fn measure(path: &Path, cancel: &AtomicBool) -> Result<Measured, u32> {
    let path: Vec<u16> = path.as_os_str().encode_wide().collect();
    let path = verbatim(&path);
    let mut walk = Walk {
        cancel,
        page: vec![0u64; LISTING_WORDS],
        measured: Measured::default(),
    };
    let entry = open_entry(&path, Access::Inspect, Reparse::Follow).map_err(|error| error.code)?;
    let tag = entry.attribute_tag().map_err(|error| error.code)?;
    walk.count_folder(&entry, &path, tag.reparse_tag);
    Ok(walk.measured)
}

struct Walk<'a> {
    cancel: &'a AtomicBool,
    page: Vec<u64>,
    measured: Measured,
}

impl Walk<'_> {
    /// Counts everything below a folder.
    fn count_folder(&mut self, entry: &Entry, verbatim: &[u16], reparse_tag: u32) {
        // A subfolder opens when it is taken from the stack, so only the
        // folders above it stay open, through their children's handles.
        let mut stack = self
            .children(entry, verbatim, reparse_tag)
            .unwrap_or_default();
        while let Some((child, verbatim)) = stack.pop() {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            self.measured.items += 1;
            let tag = child.listed.reparse_tag.unwrap_or(0);
            if !is_folder(child.listed.attributes, tag) {
                self.measured.bytes += child.listed.size;
                continue;
            }
            let children =
                (child.entry().ok()).and_then(|folder| self.children(&folder, &verbatim, tag));
            stack.extend(children.into_iter().flatten());
        }
    }

    /// The children of a folder, each with its path. A folder whose reparse
    /// tag is not a name surrogate, such as a OneDrive folder, is listed
    /// through its filter, as the run lists it. `None` when it cannot be
    /// listed.
    fn children(
        &mut self,
        folder: &Entry,
        verbatim: &[u16],
        reparse_tag: u32,
    ) -> Option<Vec<(Child, Vec<u16>)>> {
        let source = folder.reopen(Access::List).ok()?;
        let view = if reparse_tag == 0 {
            None
        } else {
            let view = open_entry(verbatim, Access::List, Reparse::Follow).ok()?;
            same_entry(&source, &view).ok()?;
            Some(view)
        };
        let parent = Arc::new(view.unwrap_or(source));
        let mut children = Vec::new();
        let cancel = self.cancel;
        let listed = list(
            &parent,
            &mut self.page,
            || cancel.load(Ordering::Relaxed),
            |child| {
                if let Ok(child) = child {
                    let path = join(verbatim, &child.name);
                    children.push((
                        Child {
                            listed: child,
                            parent: Arc::clone(&parent),
                        },
                        path,
                    ));
                }
            },
        );
        listed.ok()?;
        Some(children)
    }
}

/// A listed child, which opens relative to its folder's handle.
struct Child {
    listed: gozler_core::layout::FolderEntry,
    parent: Arc<Entry>,
}

impl Child {
    fn entry(&self) -> Result<Entry, gozler_winfs::WinError> {
        (self.parent).open_child(&self.listed.name, Access::Inspect, Reparse::Open)
    }
}
