//! Full paths for one query.
//!
//! The index stores no paths, so a full path is the mount prefix plus the
//! names on the walk up to the root. Walking to the root for every candidate
//! cost a path-term query over a whole volume about 1.4 s of index read lock
//! on a 1.95M-entry `C:`. [`DirPaths`] instead builds each folder's path once
//! per query, from its parent's, and an entry's path is its folder's path,
//! a separator and its name. Its cache is bounded, because the folders a local
//! user creates decide how large it would grow.

use crate::index::VolumeIndex;
use crate::index::volume::MAX_PATH_HOPS;
use crate::types::{EntryIdx, NIL};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

/// Which spelling of each name a path is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameForm {
    /// As stored: what gozler prints and what a case-sensitive query matches.
    Raw,
    /// Case-folded: what a case-insensitive query matches and the path sort
    /// compares.
    Folded,
}

/// A cached folder: its path's place in [`DirPaths`]' arena, and how many
/// entries its walk to the root visits, itself and the root included.
#[derive(Clone, Copy, Debug)]
struct Folder {
    start: usize,
    end: usize,
    depth: usize,
}

/// Folder paths longer than this are rebuilt from their nearest cached
/// ancestor on each use instead of cached. Caching every folder of a deep
/// chain stores each ancestor's path again inside every descendant's, which
/// grows with the square of the depth: a 4096-folder chain whose full path is
/// near the 32,767-character limit would cache about 200 MB. Real folder
/// paths are far shorter, so they all stay cached.
const CACHED_PATH_MAX: usize = 1024;

/// Arena size past which [`DirPaths`] starts its cache over. A real volume's
/// folder paths fit (about 40 MB for every folder of a 1.95M-entry `C:`),
/// but a local user can create enough folders to add up to gigabytes, and the
/// cache must not hold that under the index read lock.
const CACHE_LIMIT: usize = 64 << 20;

/// Builds the full paths of entries, caching each folder's path the first
/// time a query needs it.
///
/// An entry has a full path exactly when
/// [`crate::index::VolumeIndex::path_of`] gives one: not when its ancestry
/// ends in lost+found (an orphan whose parent has not arrived yet), and not
/// when its walk to the root visits more than [`MAX_PATH_HOPS`] entries
/// (corrupt parent links, or a cycle).
pub(crate) struct DirPaths {
    form: NameForm,
    /// Every cached folder path, back to back, after the root's.
    arena: Vec<u8>,
    root: (EntryIdx, Folder),
    /// Cached folders, and `None` for folders known to have no path. A folder
    /// missing here is not built yet, or has a path too long to cache.
    memo: FxHashMap<EntryIdx, Option<Folder>>,
    scratch: Vec<u8>,
}

impl DirPaths {
    /// Paths of `index`'s entries under `mount_prefix` (`C:\`; empty for a
    /// path within the volume).
    pub(crate) fn new(index: &VolumeIndex, form: NameForm, mount_prefix: &str) -> Self {
        let root_path = mount_prefix.trim_end_matches(['\\', '/']).as_bytes();
        let arena = match form {
            NameForm::Raw => root_path.to_vec(),
            NameForm::Folded => crate::fold::fold(root_path),
        };
        let root = Folder {
            start: 0,
            end: arena.len(),
            depth: 1,
        };
        let mut memo = FxHashMap::default();
        memo.insert(index.root(), Some(root));
        Self {
            form,
            arena,
            root: (index.root(), root),
            memo,
            scratch: Vec::new(),
        }
    }

    /// Writes the full path of entry `idx` into `out` (cleared first).
    /// Returns `false`, leaving `out` empty, when the entry has no path.
    pub(crate) fn full_path_into(
        &mut self,
        index: &VolumeIndex,
        idx: EntryIdx,
        out: &mut Vec<u8>,
    ) -> bool {
        match self.folder_path_into(index, index.entry(idx).parent(), out) {
            // The entry's own walk is one entry longer than its folder's.
            Some(depth) if depth < MAX_PATH_HOPS => {
                if !out.is_empty() {
                    out.push(b'\\');
                }
                out.extend_from_slice(self.name(index, idx));
                true
            }
            _ => {
                out.clear();
                false
            }
        }
    }

    /// Whether entry `idx` has a full path: exactly when
    /// [`Self::full_path_into`] would build one.
    pub(crate) fn entry_has_path(&mut self, index: &VolumeIndex, idx: EntryIdx) -> bool {
        let mut scratch = std::mem::take(&mut self.scratch);
        let has = self.full_path_into(index, idx, &mut scratch);
        self.scratch = scratch;
        has
    }

    /// Writes the full path of folder `dir` into `out` (cleared first) and
    /// returns the folder's depth, or `None` when it has no path.
    pub(crate) fn folder_path_into(
        &mut self,
        index: &VolumeIndex,
        dir: EntryIdx,
        out: &mut Vec<u8>,
    ) -> Option<usize> {
        out.clear();
        if self.arena.len() > CACHE_LIMIT {
            let (root_idx, root) = self.root;
            self.arena.truncate(root.end);
            self.memo.clear();
            self.memo.insert(root_idx, Some(root));
        }
        // Climb to the nearest cached folder (at worst the root), then build
        // each folder on the way back down from its parent's path.
        let mut chain: SmallVec<[EntryIdx; 32]> = SmallVec::new();
        let mut cur = dir;
        let base = loop {
            if let Some(&known) = self.memo.get(&cur) {
                break known;
            }
            let parent = index.entry(cur).parent();
            if parent == NIL {
                break None; // lost+found
            }
            if chain.len() == MAX_PATH_HOPS {
                // Longer than any walk that has a path, or a cycle. Nothing
                // is cached: the depths on this chain are unknown.
                return None;
            }
            chain.push(cur);
            cur = parent;
        };
        let Some(base) = base else {
            for &folder in &chain {
                self.memo.insert(folder, None);
            }
            return None;
        };
        out.extend_from_slice(&self.arena[base.start..base.end]);
        let mut depth = base.depth;
        for (i, &folder) in chain.iter().rev().enumerate() {
            depth += 1;
            if depth > MAX_PATH_HOPS {
                // This folder and every one below it walk too far.
                for &deeper in chain.iter().rev().skip(i) {
                    self.memo.insert(deeper, None);
                }
                out.clear();
                return None;
            }
            if !out.is_empty() {
                out.push(b'\\');
            }
            out.extend_from_slice(self.name(index, folder));
            if out.len() <= CACHED_PATH_MAX {
                let start = self.arena.len();
                self.arena.extend_from_slice(out);
                let end = self.arena.len();
                self.memo.insert(folder, Some(Folder { start, end, depth }));
            }
        }
        Some(depth)
    }

    fn name<'i>(&self, index: &'i VolumeIndex, idx: EntryIdx) -> &'i [u8] {
        match self.form {
            NameForm::Raw => index.entry(idx).name(),
            NameForm::Folded => index.folded_name(idx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{NTFS_ROOT_FRN, PathStatus};
    use crate::types::Frn;
    use crate::usn::record::{FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE};

    /// One chain of folders, deeper than [`MAX_PATH_HOPS`], with a file at
    /// every level.
    fn deep_index() -> (VolumeIndex, Vec<EntryIdx>) {
        let mut index = VolumeIndex::new();
        let mut parent = NTFS_ROOT_FRN;
        let mut entries = Vec::new();
        let rec = |frn: u64, parent: Frn, name: String, dir: bool| ParsedUsnRecord {
            major_version: 2,
            frn: Frn(frn | (1 << 48)),
            parent_frn: parent,
            usn: 0,
            timestamp_ft: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.into_bytes(),
            name_lossy: false,
        };
        for level in 0..(MAX_PATH_HOPS as u64 + 4) {
            let dir = rec(100 + 2 * level, parent, format!("folder{level:06}"), true);
            let file = rec(101 + 2 * level, dir.frn, format!("file{level:08}"), false);
            index.insert_enum(&dir);
            index.insert_enum(&file);
            parent = dir.frn;
            entries.push(index.head_of(dir.frn).unwrap());
            entries.push(index.head_of(file.frn).unwrap());
        }
        (index, entries)
    }

    /// An entry has a full path exactly when `path_of` gives one, whatever
    /// order the query visits entries in, and a deep chain does not fill the
    /// cache.
    #[test]
    fn full_paths_match_path_of_in_any_order() {
        let (index, entries) = deep_index();
        // The top of the chain and its last levels, around the depth limit.
        let entries: Vec<EntryIdx> = entries[..8]
            .iter()
            .chain(&entries[entries.len() - 24..])
            .copied()
            .collect();
        let expected = |idx| {
            let mut path = b"C:".to_vec(); // path_of adds the separator
            (index.path_of(idx, &mut path) == PathStatus::Ok).then_some(path)
        };
        let mut shallow_first = DirPaths::new(&index, NameForm::Raw, r"C:\");
        let mut deep_first = DirPaths::new(&index, NameForm::Raw, r"C:\");
        let mut out = Vec::new();
        for (dirs, order) in [
            (&mut shallow_first, entries.clone()),
            (&mut deep_first, entries.iter().rev().copied().collect()),
        ] {
            for idx in order {
                let built = dirs
                    .full_path_into(&index, idx, &mut out)
                    .then(|| out.clone());
                assert_eq!(built, expected(idx), "entry {idx}");
                assert_eq!(dirs.entry_has_path(&index, idx), built.is_some());
            }
        }
        // Caching every folder of the chain would hold about 100 MB.
        for dirs in [shallow_first, deep_first] {
            assert!(
                dirs.arena.len() < 1 << 20,
                "{} bytes cached",
                dirs.arena.len()
            );
        }
    }
}
