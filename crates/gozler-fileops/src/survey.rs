//! Prepare's counts of a copy or a move, so the window can ask the person
//! (items, bytes, conflicts, selected entries with one name), and Measure,
//! which counts a folder outside the queue.
//!
//! The counts come from the same conflict rules the run applies
//! ([`classify`]), and a file that Continue finds done counts no bytes:
//! otherwise a 100 GB copy that crashed at 90 GB on a drive with 120 GB
//! free could not continue.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Entry, Reparse, allocated_size, open_entry};

use crate::conflicts::{Clash, Rule, Run, Wanted, classify};
use crate::copy::Source;
use crate::listing::{LISTING_WORDS, is_folder, join, kind, list, read_names};
use crate::names::{Holder, Names};
use crate::operation::Survey;
use crate::receive::{Destination, verbatim};
use crate::tree::same_entry;
use crate::win32::FILE_ATTRIBUTE_SPARSE_FILE;

/// What prepare counts.
pub(crate) struct Surveyed {
    pub(crate) survey: Survey,
    /// The bytes the destination needs: a sparse file that stays sparse at
    /// its allocated size, because one copied where it cannot stay sparse
    /// takes its full size.
    pub(crate) needed: u64,
}

/// How prepare walks.
pub(crate) struct Surveying<'a> {
    pub(crate) rule: Rule,
    pub(crate) sparse_stays: bool,
    pub(crate) cancel: &'a AtomicBool,
}

/// Counts `sources` against the names of `destination`. Each source says
/// whether its bytes are copied, as for a copy or a move across volumes; a
/// move by rename counts only its entries.
pub(crate) fn survey<'s>(
    sources: impl IntoIterator<Item = (&'s Source, bool)>,
    destination: &Destination,
    surveying: &Surveying<'_>,
) -> Result<Surveyed, u32> {
    let mut walk = Walk {
        surveying,
        page: vec![0u64; LISTING_WORDS],
        counted: Surveyed {
            survey: Survey::default(),
            needed: 0,
        },
    };
    let mut names = destination.names(&mut walk.page)?;
    for (source, copies) in sources {
        if walk.cancelled() {
            break;
        }
        let selected = &source.selected;
        let opened = open_entry(&selected.verbatim, Access::Inspect, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry.stat()?, entry)));
        // The run lists an entry that cannot be read.
        let Ok((tag, stat, entry)) = opened else {
            continue;
        };
        let item = Item {
            name: selected.identity.name(),
            verbatim: &selected.verbatim,
            entry: &entry,
            attributes: tag.attributes,
            reparse_tag: tag.reparse_tag,
            size: stat.size,
            last_write: stat.last_write,
        };
        walk.item(
            &mut names,
            &destination.verbatim,
            &item,
            source.own_folder,
            copies,
            source.shared,
        );
    }
    Ok(walk.counted)
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
    let surveying = Surveying {
        rule: Rule {
            choice: None,
            run: Run::First { started: None },
        },
        sparse_stays: false,
        cancel,
    };
    let mut walk = Walk {
        surveying: &surveying,
        page: vec![0u64; LISTING_WORDS],
        counted: Surveyed {
            survey: Survey::default(),
            needed: 0,
        },
    };
    let entry = open_entry(&path, Access::Inspect, Reparse::Follow).map_err(|error| error.code)?;
    let tag = entry.attribute_tag().map_err(|error| error.code)?;
    walk.count_folder(&entry, &path, tag.reparse_tag);
    Ok(Measured {
        items: walk.counted.survey.items,
        bytes: walk.counted.survey.bytes,
    })
}

/// An entry the walk meets, opened without following links.
struct Item<'a> {
    name: &'a [u16],
    verbatim: &'a [u16],
    entry: &'a Entry,
    attributes: u32,
    reparse_tag: u32,
    size: u64,
    last_write: i64,
}

struct Walk<'a> {
    surveying: &'a Surveying<'a>,
    page: Vec<u64>,
    counted: Surveyed,
}

impl Walk<'_> {
    fn cancelled(&self) -> bool {
        self.surveying.cancel.load(Ordering::Relaxed)
    }

    /// Counts one entry that wants its name in the folder whose names are
    /// `names`, at `folder`.
    fn item(
        &mut self,
        names: &mut Names,
        folder: &[u16],
        item: &Item<'_>,
        own_folder: bool,
        copies: bool,
        shared: bool,
    ) {
        let wanted = Wanted {
            kind: kind(item.attributes, item.reparse_tag),
            size: item.size,
            last_write: item.last_write,
            own_folder,
            copied: copies,
            shared,
        };
        let entered = is_folder(item.attributes, item.reparse_tag);
        let survey = &mut self.counted.survey;
        let clash = classify(names, item.name, wanted, self.surveying.rule);
        match clash {
            Clash::Done { name } => {
                names.insert(&name, Holder::Ours);
                return;
            }
            Clash::Mismatch => return,
            Clash::Merge => {
                survey.items += 1;
                return self.merge(&join(folder, item.name), item, copies, shared);
            }
            // Its bytes at the destination already take their space.
            Clash::Unfinished { name, size } => {
                names.insert(&name, Holder::Ours);
                survey.items += 1;
                survey.bytes += item.size;
                self.counted.needed += item.size.saturating_sub(size);
                return;
            }
            Clash::Conflict => survey.conflicts += 1,
            Clash::SameName => survey.same_name += 1,
            Clash::Free | Clash::OwnFolder => {}
        }
        // The name becomes the operation's, as the run takes it, so a second
        // selected entry of this name counts as one with the same name.
        if matches!(clash, Clash::Free | Clash::Conflict) {
            names.insert(item.name, Holder::Ours);
        }
        if !copies {
            self.counted.survey.items += 1;
        } else if entered {
            self.counted.survey.items += 1;
            self.count_folder(item.entry, item.verbatim, item.reparse_tag);
        } else {
            self.count_file(item.verbatim, item.attributes, item.size);
        }
    }

    fn count_file(&mut self, verbatim: &[u16], attributes: u32, size: u64) {
        self.counted.survey.items += 1;
        self.counted.survey.bytes += size;
        let sparse = self.surveying.sparse_stays && attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0;
        self.counted.needed += match sparse {
            true => allocated_size(verbatim).unwrap_or(size),
            false => size,
        };
    }

    /// Counts the children of a folder whose names the destination folder
    /// `into` already holds.
    fn merge(&mut self, into: &[u16], item: &Item<'_>, copies: bool, shared: bool) {
        let names = open_entry(into, Access::List, Reparse::Open)
            .map_err(|error| error.code)
            .and_then(|folder| read_names(&folder, &mut self.page));
        // The run lists a folder it cannot read.
        let Ok(mut names) = names else {
            return;
        };
        let Some(children) = self.children(item.entry, item.verbatim, item.reparse_tag) else {
            return;
        };
        for (child, verbatim) in children {
            if self.cancelled() {
                return;
            }
            let opened = (child.entry())
                .and_then(|entry| Ok((entry.attribute_tag()?, entry.stat()?, entry)));
            let Ok((tag, stat, entry)) = opened else {
                continue;
            };
            let child = Item {
                name: &child.name,
                verbatim: &verbatim,
                entry: &entry,
                attributes: tag.attributes,
                reparse_tag: tag.reparse_tag,
                size: stat.size,
                last_write: stat.last_write,
            };
            self.item(&mut names, into, &child, false, copies, shared);
        }
    }

    /// Counts everything below a folder that is copied whole.
    fn count_folder(&mut self, entry: &Entry, verbatim: &[u16], reparse_tag: u32) {
        // A subfolder opens when it is taken from the stack, so only the
        // folders above it stay open, through their children's handles.
        let mut stack = self
            .children(entry, verbatim, reparse_tag)
            .unwrap_or_default();
        while let Some((child, verbatim)) = stack.pop() {
            if self.cancelled() {
                return;
            }
            let tag = child.listed.reparse_tag.unwrap_or(0);
            if !is_folder(child.listed.attributes, tag) {
                let (attributes, size) = (child.listed.attributes, child.listed.size);
                self.count_file(&verbatim, attributes, size);
                continue;
            }
            self.counted.survey.items += 1;
            let children =
                (child.entry().ok()).and_then(|folder| self.children(&folder, &verbatim, tag));
            stack.extend(children.into_iter().flatten());
        }
    }

    /// The children of a folder, each with its path. A folder whose reparse
    /// tag is not a name surrogate, such as a OneDrive folder, is listed
    /// through its filter, as the run lists it. `None` when it cannot be
    /// listed, which the run reports.
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
        let parent = std::sync::Arc::new(view.unwrap_or(source));
        let mut children = Vec::new();
        let cancel = self.surveying.cancel;
        let listed = list(
            &parent,
            &mut self.page,
            || cancel.load(Ordering::Relaxed),
            |child| {
                if let Ok(child) = child {
                    let path = join(verbatim, &child.name);
                    children.push((
                        Child {
                            name: child.name.clone(),
                            listed: child,
                            parent: std::sync::Arc::clone(&parent),
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
    name: Vec<u16>,
    listed: gozler_core::layout::FolderEntry,
    parent: std::sync::Arc<Entry>,
}

impl Child {
    fn entry(&self) -> Result<Entry, gozler_winfs::WinError> {
        self.parent
            .open_child(&self.name, Access::Inspect, Reparse::Open)
    }
}
