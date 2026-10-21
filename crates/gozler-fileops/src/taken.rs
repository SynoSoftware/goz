//! Taken entries: the paths that the window leaves out of the panels and
//! shows blank in the search results, because a delete or a move has them.
//!
//! This is a display rule on the path text the window gave; the safety rule
//! is D14 on identities. The window asks for every row it paints, so a row
//! is never compared with every taken path, which a large delete would make
//! millions of compares per paint: a row is taken when its folded path or
//! one of its parent paths is in a hash map, one lookup per component.
//!
//! A waiting or running operation takes its sources from every list. An
//! ended one takes them only from the lists that keep it, because each
//! list learns at its own time that the disk has the change: a folder panel
//! when it has read its folder again, a search list at its next search. So
//! one map holds both, with the operations that take each path, and each
//! list names the ended operations it keeps.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::names::fold;
use crate::operation::OperationId;

const BACKSLASH: u16 = b'\\' as u16;

/// The folded full paths of the sources of every waiting, running or ended
/// delete or move that is still in the queue. The window takes a snapshot
/// of it at each wake, so a paint never waits for the engine's lock.
#[derive(Debug, Default, Clone)]
pub struct Taken {
    /// Each taken path and the operations that take it: one, or several
    /// when an operation waited for an earlier one with the same source.
    paths: HashMap<Vec<u16>, Vec<OperationId>>,
    /// The operations in `paths` that have ended.
    ended: HashSet<OperationId>,
    /// The folded folder of each taken path, so a folder panel compares
    /// its entries' paths only in a folder that holds a taken entry.
    folders: HashSet<Vec<u16>>,
}

impl Taken {
    /// Adds a source of `id`. Only the queue builds the snapshot; the
    /// window's tests build one without a queue.
    #[doc(hidden)]
    pub fn insert(&mut self, id: OperationId, path: &Path, ended: bool) {
        let path = folded(&units(path));
        if let Some(at) = path.iter().rposition(|&unit| unit == BACKSLASH) {
            self.folders.insert(path[..at].to_vec());
        }
        self.paths.entry(path).or_default().push(id);
        if ended {
            self.ended.insert(id);
        }
    }

    /// Whether no path is taken, which a caller checks before it encodes a
    /// path for [`contains`](Self::contains).
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Whether a taken path is directly in the folder `folder`, as UTF-16,
    /// whichever operation takes it.
    pub fn holds_in(&self, folder: &[u16]) -> bool {
        // A drive's root, such as `C:\`, is kept without its backslash.
        let folder = folder.strip_suffix(&[BACKSLASH]).unwrap_or(folder);
        !self.folders.is_empty() && self.folders.contains(&folded(folder))
    }

    /// The folded names of the taken paths directly in the folder `folder`,
    /// as UTF-16, whichever operation takes them. It walks every taken path,
    /// so a caller asks once for a folder, not once for each entry.
    pub fn names_in<'a>(&'a self, folder: &[u16]) -> impl Iterator<Item = &'a [u16]> + 'a {
        let mut prefix = folded(folder);
        prefix.push(BACKSLASH);
        self.paths.keys().filter_map(move |path| {
            path.strip_prefix(prefix.as_slice())
                .filter(|name| !name.contains(&BACKSLASH))
        })
    }

    /// Whether `path`, as UTF-16, equals or is below a path that the
    /// operation `id` takes.
    pub fn takes(&self, path: &[u16], id: OperationId) -> bool {
        self.contains_where(path, |ids| ids.contains(&id))
    }

    /// Whether `path`, as UTF-16, equals or is below a path that a waiting
    /// or running operation takes, or one that an ended operation takes
    /// that the list `keeps`, compared as Windows compares names, ignoring
    /// case. `keeps` answers from the list's own set of the ended
    /// operations whose rows it still leaves out.
    pub fn contains(&self, path: &[u16], keeps: impl Fn(OperationId) -> bool) -> bool {
        let takes = |id: &OperationId| !self.ended.contains(id) || keeps(*id);
        self.contains_where(path, |ids| ids.iter().any(takes))
    }

    /// Whether `path` or a folder above it is taken by operations that
    /// `taken` accepts.
    fn contains_where(&self, path: &[u16], taken: impl Fn(&[OperationId]) -> bool) -> bool {
        if self.is_empty() {
            return false;
        }
        let mut path = folded(path);
        loop {
            if self.paths.get(&path).is_some_and(|ids| taken(ids)) {
                return true;
            }
            match path.iter().rposition(|&unit| unit == BACKSLASH) {
                Some(at) => path.truncate(at),
                None => return false,
            }
        }
    }

    /// The ended operations whose sources it holds. The window decides
    /// which lists keep each one from the same snapshot that the lists then
    /// use, so no list shows an ended operation's rows before it has
    /// decided.
    pub fn ended(&self) -> impl Iterator<Item = OperationId> + '_ {
        self.ended.iter().copied()
    }
}

fn folded(path: &[u16]) -> Vec<u16> {
    let mut units = fold(path);
    // A volume root ends with a backslash; no other path does.
    while units.last() == Some(&BACKSLASH) {
        units.pop();
    }
    units
}

#[cfg(windows)]
fn units(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().collect()
}

#[cfg(not(windows))]
fn units(path: &Path) -> Vec<u16> {
    path.to_string_lossy().encode_utf16().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(path: &str) -> Vec<u16> {
        path.encode_utf16().collect()
    }

    /// A row equal to a taken path, or below it in another case, is taken;
    /// a row beside it whose name only starts like it is not.
    #[test]
    fn rows_below_a_taken_path_are_taken() {
        let mut taken = Taken::default();
        taken.insert(OperationId(0), Path::new(r"C:\a"), false);
        let none = |_| false;
        assert!(taken.contains(&wide(r"C:\a"), none));
        assert!(taken.contains(&wide(r"c:\A\b\c.txt"), none));
        assert!(!taken.contains(&wide(r"C:\ab"), none));
        assert!(!taken.contains(&wide(r"C:\"), none));
    }

    /// An ended operation takes its rows only from a list that keeps it,
    /// also when an earlier operation with the same source has ended and
    /// that list released it; a running one takes them from every list.
    #[test]
    fn an_ended_operation_takes_rows_only_from_the_lists_that_keep_it() {
        let (second, running) = (OperationId(1), OperationId(2));
        let mut taken = Taken::default();
        taken.insert(OperationId(0), Path::new(r"C:\a"), true);
        taken.insert(second, Path::new(r"C:\a"), true);
        taken.insert(running, Path::new(r"C:\b"), false);
        let row = wide(r"C:\a\c.txt");
        assert!(!taken.contains(&row, |_| false));
        assert!(taken.contains(&row, |id| id == second));
        assert!(taken.contains(&wide(r"C:\b"), |_| false));
    }
}
