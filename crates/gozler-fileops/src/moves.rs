//! A move on one volume: each selected entry is renamed, a whole folder in
//! one call, so 100 GB moves in milliseconds (D16). No walk runs, except
//! into folders that merge: a folder of that name at the destination makes
//! the move rename each child into it, merge child folders the same way,
//! and delete each emptied source folder.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Entry, Reparse, open_entry, rename};

use crate::conflicts::{Decision, Rule, Wanted, decide};
use crate::copy::Source;
use crate::delete::{Disposition, remove};
use crate::listing::{LISTING_WORDS, join, kind, list, read_names};
use crate::names::Kind;
use crate::operation::{Listed, Pause, Progress, Reason};
use crate::place::{Placed, Placing, Target, place};
use crate::receive::listed;
use crate::tree::same_entry;
use crate::win32::{ERROR_ALREADY_EXISTS, FILE_ATTRIBUTE_DIRECTORY, is_gone};

pub(crate) struct Moving<'a> {
    pub(crate) rule: Rule,
    pub(crate) cancel: &'a AtomicBool,
    pub(crate) pause: &'a Pause,
    pub(crate) progress: &'a Progress,
}

/// Moves `sources` into `destination` by renaming them, and returns what
/// it lists, in the order it happened. `Source::own_folder` marks an entry
/// already in the destination, which is left alone.
pub(crate) fn move_by_rename(
    sources: Vec<Source>,
    destination: &Target,
    moving: &Moving<'_>,
) -> Vec<Listed> {
    let mut run = Run {
        moving,
        listed: Vec::new(),
        page: vec![0u64; LISTING_WORDS],
        disposition: Disposition::default(),
    };
    for source in sources {
        moving.pause.wait(moving.cancel);
        if run.cancelled() {
            break;
        }
        let selected = source.selected;
        if source.own_folder {
            moving.progress.add_items(1);
            continue;
        }
        let opened = open_entry(&selected.verbatim, Access::Inspect, Reparse::Open)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
        let (tag, entry) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                run.fail(&selected.path, error.code);
                continue;
            }
        };
        let moved = Moved {
            name: &source.name,
            path: &selected.path,
            from: &selected.verbatim,
            entry: &entry,
            attributes: tag.attributes,
            reparse_tag: tag.reparse_tag,
            shared: source.shared,
        };
        run.place_entry(destination, &moved);
    }
    run.listed
}

/// An entry to move, opened without following links.
struct Moved<'a> {
    name: &'a [u16],
    /// As the window gave it, for the result.
    path: &'a [u16],
    /// With the `\\?\` or `\\?\UNC\` prefix.
    from: &'a [u16],
    entry: &'a Entry,
    attributes: u32,
    reparse_tag: u32,
    /// [`Wanted::shared`].
    shared: bool,
}

struct Run<'a> {
    moving: &'a Moving<'a>,
    listed: Vec<Listed>,
    page: Vec<u64>,
    disposition: Disposition,
}

impl Run<'_> {
    fn cancelled(&self) -> bool {
        self.moving.cancel.load(Ordering::Relaxed)
    }

    fn list_entry(&mut self, path: &[u16], reason: Reason) {
        self.moving.progress.add_items(1);
        self.listed.push(listed(path, reason));
    }

    fn fail(&mut self, path: &[u16], error: u32) {
        self.list_entry(path, Reason::Failed(error));
    }

    /// Renames one entry into `target`, or merges a folder into the folder
    /// of its name. Returns whether the entry left its source.
    fn place_entry(&mut self, target: &Target, moved: &Moved<'_>) -> bool {
        let stat = match moved.entry.stat() {
            Ok(stat) => stat,
            Err(error) => {
                self.fail(moved.path, error.code);
                return false;
            }
        };
        let wanted = Wanted {
            kind: kind(moved.attributes, moved.reparse_tag),
            size: stat.size,
            last_write: stat.last_write,
            own_folder: false,
            copied: false,
            shared: moved.shared,
        };
        let decision = {
            let mut names = target.names.lock().expect("one thread");
            decide(&mut names, moved.name, wanted, self.moving.rule)
        };
        match decision {
            Decision::Write { name, replace } => {
                let placing = Placing {
                    base: moved.name,
                    name,
                    replace,
                    is_folder: moved.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    shared: moved.shared,
                };
                let renamed = |to: &[u16], existing| {
                    rename(moved.from, to, existing).map_err(|error| error.code)
                };
                match place(target, placing, self.moving.rule.choice, renamed) {
                    Placed::Named(()) => {
                        self.moving.progress.add_items(1);
                        true
                    }
                    Placed::Skipped => {
                        self.list_entry(moved.path, Reason::Skipped);
                        false
                    }
                    Placed::Failed(error) => {
                        self.fail(moved.path, error);
                        false
                    }
                }
            }
            Decision::Merge if wanted.kind == Kind::Folder => self.merge(target, moved),
            Decision::Merge | Decision::Mismatch => {
                self.fail(moved.path, ERROR_ALREADY_EXISTS);
                false
            }
            Decision::Resume { .. } => unreachable!("only a copied entry is unfinished"),
            Decision::Done => {
                // Equal size and time are a guess, and no hash checks it, so
                // a continued move never deletes a source on it.
                self.list_entry(moved.path, Reason::SourceKept);
                false
            }
            Decision::Skip => {
                self.list_entry(moved.path, Reason::Skipped);
                false
            }
        }
    }

    /// Moves the children of the source folder into the destination folder
    /// of its name in `target`, then deletes the source folder when it is
    /// empty. Returns whether it went.
    fn merge(&mut self, target: &Target, moved: &Moved<'_>) -> bool {
        let into = join(&target.verbatim, moved.name);
        let names = open_entry(&into, Access::List, Reparse::Open)
            .map_err(|error| error.code)
            .and_then(|folder| read_names(&folder, &mut self.page));
        let names = match names {
            Ok(names) => names,
            Err(error) => {
                self.fail(moved.path, error);
                return false;
            }
        };
        let into = Target {
            verbatim: into,
            names: Mutex::new(names),
            made: false,
        };
        // A folder whose reparse tag is not a name surrogate, such as a
        // OneDrive folder, is listed through its filter, as the delete does.
        let source = moved.entry.reopen(Access::List).and_then(|source| {
            if moved.reparse_tag == 0 {
                return Ok((source, None));
            }
            let view = open_entry(moved.from, Access::List, Reparse::Follow)?;
            Ok((source, Some(view)))
        });
        let (source, view) = match source {
            Ok(source) => source,
            Err(error) => {
                self.fail(moved.path, error.code);
                return false;
            }
        };
        if let Some(view) = &view
            && let Err(error) = same_entry(&source, view)
        {
            self.fail(moved.path, error);
            return false;
        }
        let children_of = view.as_ref().unwrap_or(&source);
        let mut children = Vec::new();
        let mut page = std::mem::take(&mut self.page);
        let listed_all = list(
            children_of,
            &mut page,
            || false,
            |child| children.push(child),
        );
        self.page = page;
        let mut emptied = true;
        if let Err(error) = listed_all {
            self.fail(moved.path, error);
            emptied = false;
        }
        for child in children {
            self.moving.pause.wait(self.moving.cancel);
            if self.cancelled() {
                return false;
            }
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    self.fail(moved.path, error);
                    emptied = false;
                    continue;
                }
            };
            let path = join(moved.path, &child.name);
            let opened = children_of
                .open_child(&child.name, Access::Inspect, Reparse::Open)
                .and_then(|entry| Ok((entry.attribute_tag()?, entry)));
            let (tag, entry) = match opened {
                Ok(opened) => opened,
                Err(error) if is_gone(error.code) => continue,
                Err(error) => {
                    self.fail(&path, error.code);
                    emptied = false;
                    continue;
                }
            };
            let child = Moved {
                name: &child.name,
                path: &path,
                from: &join(moved.from, &child.name),
                entry: &entry,
                attributes: tag.attributes,
                reparse_tag: tag.reparse_tag,
                shared: moved.shared,
            };
            emptied &= self.place_entry(&into, &child);
        }
        if !emptied {
            return false;
        }
        drop(view);
        let reopen = || source.reopen(Access::delete(moved.attributes));
        match remove(self.moving.cancel, &self.disposition, reopen) {
            Ok(()) => {
                self.moving.progress.add_items(1);
                true
            }
            Err(error) => {
                self.fail(moved.path, error);
                false
            }
        }
    }
}
