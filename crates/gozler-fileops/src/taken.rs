//! Taken entries: the paths that the window leaves out of the panels and
//! shows blank in the search results, because a delete or a move has them.
//!
//! This is a display rule on the path text the window gave; the safety rule
//! is D14 on identities. The window asks for every row it paints, so a row
//! is never compared with every taken path, which a large delete would make
//! millions of compares per paint: a row is taken when its folded path or
//! one of its parent paths is in a hash map, looked up only where a taken
//! path of that length could end.
//!
//! A waiting or running operation takes its sources from every list. An
//! ended one takes them only from the lists that keep it, because each
//! list learns at its own time that the disk has the change: a folder panel
//! when it has read its folder again, a search list at its next search. So
//! one map holds both, with the operations that take each path, and each
//! list names the ended operations it keeps.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::names::fold_into;
use crate::operation::OperationId;

const BACKSLASH: u16 = b'\\' as u16;

/// The folded full paths of the sources of every waiting, running or ended
/// delete or move that is still in the queue. The window takes a snapshot
/// of it at each wake, so a paint never waits for the engine's lock.
#[derive(Debug, Default, Clone)]
pub struct Taken {
    /// The operations that take each taken path: one, or several when an
    /// operation waited for an earlier one with the same source.
    ids: Vec<Vec<OperationId>>,
    /// Each folded taken path, with its place in `ids`.
    paths: HashMap<Vec<u16>, usize>,
    /// The lengths of the paths in `paths`, sorted, so a row's parent
    /// paths of other lengths are not looked up.
    lengths: Vec<usize>,
    /// The operations in `ids` that have ended.
    ended: HashSet<OperationId>,
    /// Each folded folder that holds a taken path, with the name of each
    /// taken path in it as the operation gave it and its place in `ids`, so
    /// a folder panel finds its taken entries without folding a name.
    folders: HashMap<Vec<u16>, HashMap<Box<[u16]>, usize>>,
}

/// The taken paths directly in one folder ([`Taken::in_folder`]).
pub struct TakenFolder<'a> {
    taken: &'a Taken,
    names: &'a HashMap<Box<[u16]>, usize>,
}

impl<'a> TakenFolder<'a> {
    /// The names of the taken paths, as the operations gave them.
    pub fn names(&self) -> impl Iterator<Item = &'a [u16]> + 'a {
        self.names.keys().map(|name| &**name)
    }

    /// Whether the entry `name` is taken, as [`Taken::contains`] says, when
    /// an operation gave it spelled exactly so. `None` when none did, and
    /// the caller then compares its path with [`Taken::contains`], which
    /// ignores case.
    pub fn takes(&self, name: &[u16], keeps: impl Fn(OperationId) -> bool) -> Option<bool> {
        let &at = self.names.get(name)?;
        Some(self.taken.any_takes(&self.taken.ids[at], keeps))
    }
}

impl Taken {
    /// Adds a source of `id`. Only the queue builds the snapshot; the
    /// window's tests build one without a queue.
    #[doc(hidden)]
    pub fn insert(&mut self, id: OperationId, path: &Path, ended: bool) {
        let given = units(path);
        let path = folded(&given);
        let at = match self.paths.get(&path) {
            Some(&at) => at,
            None => {
                let at = self.ids.len();
                self.ids.push(Vec::new());
                if let Some(slash) = path.iter().rposition(|&unit| unit == BACKSLASH) {
                    // Folding keeps every unit in its place.
                    let name = &given[slash + 1..path.len()];
                    (self.folders.entry(path[..slash].to_vec()).or_default())
                        .insert(name.into(), at);
                }
                if let Err(place) = self.lengths.binary_search(&path.len()) {
                    self.lengths.insert(place, path.len());
                }
                self.paths.insert(path, at);
                at
            }
        };
        self.ids[at].push(id);
        if ended {
            self.ended.insert(id);
        }
    }

    /// Whether no path is taken, which a caller checks before it encodes a
    /// path for [`contains`](Self::contains).
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The taken paths directly in the folder `folder`, as UTF-16,
    /// whichever operation takes them; `None` when it holds none.
    pub fn in_folder(&self, folder: &[u16]) -> Option<TakenFolder<'_>> {
        if self.folders.is_empty() {
            return None;
        }
        let names = self.folders.get(&folded(folder))?;
        Some(TakenFolder { taken: self, names })
    }

    /// Gives `found` the operations that take each taken path that `path`,
    /// as UTF-16, equals or is below, ended or not; `folded` is a buffer
    /// that the caller reuses for many paths.
    pub fn takers(
        &self,
        path: &[u16],
        folded: &mut Vec<u16>,
        mut found: impl FnMut(&[OperationId]),
    ) {
        self.contains_where(path, folded, |ids| {
            found(ids);
            false
        });
    }

    /// Whether `path`, as UTF-16, equals or is below a path that a waiting
    /// or running operation takes, or one that an ended operation takes
    /// that the list `keeps`, compared as Windows compares names, ignoring
    /// case. `keeps` answers from the list's own set of the ended
    /// operations whose rows it still leaves out.
    pub fn contains(&self, path: &[u16], keeps: impl Fn(OperationId) -> bool) -> bool {
        self.contains_where(path, &mut Vec::new(), |ids| self.any_takes(ids, &keeps))
    }

    /// Whether one of `ids` takes rows from a list that `keeps` the ended
    /// operations it says.
    fn any_takes(&self, ids: &[OperationId], keeps: impl Fn(OperationId) -> bool) -> bool {
        (ids.iter()).any(|id| !self.ended.contains(id) || keeps(*id))
    }

    /// Whether `path` or a folder above it is taken by operations that
    /// `taken` accepts, folded into `folded`.
    fn contains_where(
        &self,
        path: &[u16],
        folded: &mut Vec<u16>,
        mut taken: impl FnMut(&[OperationId]) -> bool,
    ) -> bool {
        if self.is_empty() {
            return false;
        }
        fold_into(path, folded);
        trim_root(folded);
        let folded = &folded[..];
        let parents =
            (folded.iter().enumerate()).filter_map(|(at, &unit)| (unit == BACKSLASH).then_some(at));
        parents.chain([folded.len()]).any(|end| {
            self.lengths.binary_search(&end).is_ok()
                && (self.paths.get(&folded[..end])).is_some_and(|&at| taken(&self.ids[at]))
        })
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
    let mut units = Vec::with_capacity(path.len());
    fold_into(path, &mut units);
    trim_root(&mut units);
    units
}

/// A volume root ends with a backslash; no other path does.
fn trim_root(units: &mut Vec<u16>) {
    while units.last() == Some(&BACKSLASH) {
        units.pop();
    }
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

    /// A search list keeps each ended operation that takes one of its rows,
    /// so a row below two taken paths names the operations of both, also
    /// when the outer one is found first.
    #[test]
    fn a_row_names_the_takers_of_every_path_above_it() {
        let (outer, inner) = (OperationId(0), OperationId(1));
        let mut taken = Taken::default();
        taken.insert(outer, Path::new(r"C:\a"), true);
        taken.insert(inner, Path::new(r"C:\a\b"), true);
        taken.insert(OperationId(2), Path::new(r"C:\a\bc"), true);
        let mut found = Vec::new();
        taken.takers(&wide(r"c:\A\B\x.txt"), &mut Vec::new(), |ids| {
            found.extend_from_slice(ids);
        });
        found.sort();
        assert_eq!(found, [outer, inner]);
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
