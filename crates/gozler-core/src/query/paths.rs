//! Full paths for one query.
//!
//! The index stores no paths, so a full path is the mount prefix plus the
//! names on the walk up to the root. Walking to the root for every candidate
//! would hold the index read lock for seconds on a large volume, so
//! [`DirPaths`] builds each folder's path once per query, from its parent's,
//! and an entry's path is its folder's path, a separator and its name. Its
//! cache is bounded, because the folders a local user creates decide how large
//! it would grow.
//!
//! A query's results hold no full paths at all: each hit names its folder in
//! a [`ResultNames`] table, which holds each folder once, as its parent and its
//! name. So the results grow with the names, not with the folder depth, and
//! the pipe sends each folder once in the same way.

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
/// folder paths fit (about 40 MB for every folder of a two-million-entry
/// system volume), but a local user can create enough folders to add up to
/// gigabytes, and the cache must not hold that under the index read lock.
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

/// A folder in a [`ResultNames`] table: its place in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FolderId(u32);

impl FolderId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// An entry's own name in a [`ResultNames`] table: where its bytes lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRef {
    start: usize,
    end: usize,
}

/// The names of a set of results, so that no result holds its full path:
/// each folder the results are in, held once with every folder above it as
/// its parent and its name, and each result's own name. All of them share one
/// buffer, so a result costs no allocation of its own.
///
/// A volume's root has no parent, and its name is the mount prefix without
/// the last separator. A folder always comes after its parent, so every walk
/// up the table ends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultNames {
    folders: Vec<TableFolder>,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TableFolder {
    parent: Option<FolderId>,
    name: NameRef,
}

/// What the ids of one [`ResultNames`] become after
/// [`ResultNames::append`] moves them into another.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Moved {
    folders: u32,
    bytes: usize,
}

impl Moved {
    pub(crate) fn folder(self, id: FolderId) -> FolderId {
        FolderId(id.0 + self.folders)
    }

    pub(crate) fn name(self, name: NameRef) -> NameRef {
        NameRef {
            start: name.start + self.bytes,
            end: name.end + self.bytes,
        }
    }
}

impl ResultNames {
    /// How many folders the table holds.
    pub fn folder_count(&self) -> usize {
        self.folders.len()
    }

    /// The folder at `position`, when the table holds one there.
    pub(crate) fn folder_at(&self, position: u32) -> Option<FolderId> {
        ((position as usize) < self.folders.len()).then_some(FolderId(position))
    }

    /// The folder that holds `folder`; `None` for a volume's root.
    pub fn parent(&self, folder: FolderId) -> Option<FolderId> {
        self.folders[folder.index()].parent
    }

    /// The folder's own name (WTF-8); for a volume's root, its mount prefix
    /// without the last separator.
    pub fn folder_name(&self, folder: FolderId) -> &[u8] {
        self.name(self.folders[folder.index()].name)
    }

    /// An entry's own name (WTF-8).
    pub fn name(&self, name: NameRef) -> &[u8] {
        &self.bytes[name.start..name.end]
    }

    /// Writes the full path of `folder` into `out` (cleared first).
    pub fn folder_path_into(&self, folder: FolderId, out: &mut Vec<u8>) {
        out.clear();
        let mut chain: SmallVec<[FolderId; 32]> = SmallVec::new();
        let mut cur = Some(folder);
        while let Some(f) = cur {
            chain.push(f);
            cur = self.parent(f);
        }
        for &f in chain.iter().rev() {
            join_name(out, self.folder_name(f));
        }
    }

    /// Writes the full path of the entry `name` in `folder` into `out`
    /// (cleared first).
    pub fn full_path_into(&self, folder: FolderId, name: &[u8], out: &mut Vec<u8>) {
        self.folder_path_into(folder, out);
        join_name(out, name);
    }

    /// Adds a folder in `parent`, which must already be in the table.
    pub(crate) fn push_folder(&mut self, parent: Option<FolderId>, name: &[u8]) -> FolderId {
        assert!(
            parent.is_none_or(|p| p.index() < self.folders.len()),
            "a folder's parent comes before it"
        );
        let name = self.push_name(name);
        self.folders.push(TableFolder { parent, name });
        FolderId(
            u32::try_from(self.folders.len() - 1)
                .expect("fewer folders than entries, and an EntryIdx is a u32"),
        )
    }

    /// Adds an entry's own name.
    pub(crate) fn push_name(&mut self, name: &[u8]) -> NameRef {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(name);
        NameRef {
            start,
            end: self.bytes.len(),
        }
    }

    /// Moves every folder and name of `other` to the end of this table.
    pub(crate) fn append(&mut self, other: ResultNames) -> Moved {
        let moved = Moved {
            folders: u32::try_from(self.folders.len()).expect("a folder count fits in u32"),
            bytes: self.bytes.len(),
        };
        self.bytes.extend_from_slice(&other.bytes);
        self.folders
            .extend(other.folders.into_iter().map(|f| TableFolder {
                parent: f.parent.map(|p| moved.folder(p)),
                name: moved.name(f.name),
            }));
        moved
    }
}

/// Appends `name` to the path in `out`, after a separator unless the path is
/// still empty: the root of a volume mounted nowhere has an empty name.
fn join_name(out: &mut Vec<u8>, name: &[u8]) {
    if !out.is_empty() {
        out.push(b'\\');
    }
    out.extend_from_slice(name);
}

/// Copies the folders that a query's hits are in out of the index, into a
/// [`ResultNames`] table, each once. It decides which entries have a full path
/// by the rule that [`DirPaths`] applies, so a hit it keeps is a hit that
/// has one.
pub(crate) struct FolderCopy {
    names: ResultNames,
    /// Copied folders with their depth, counted as [`DirPaths`] counts it,
    /// and `None` for folders known to have no path.
    copied: FxHashMap<EntryIdx, Option<(FolderId, usize)>>,
}

impl FolderCopy {
    /// A table for `index`, the volume mounted at `mount_prefix` (`C:\`; empty
    /// for a path within the volume).
    pub(crate) fn new(index: &VolumeIndex, mount_prefix: &str) -> Self {
        let mut names = ResultNames::default();
        let root = names.push_folder(None, mount_prefix.trim_end_matches(['\\', '/']).as_bytes());
        let mut copied = FxHashMap::default();
        copied.insert(index.root(), Some((root, 1)));
        Self { names, copied }
    }

    /// The folder of entry `idx`, copied with every folder above it, or
    /// `None` when the entry has no full path.
    pub(crate) fn entry_folder(&mut self, index: &VolumeIndex, idx: EntryIdx) -> Option<FolderId> {
        match self.folder(index, index.entry(idx).parent()) {
            // The entry's own walk is one entry longer than its folder's.
            Some((folder, depth)) if depth < MAX_PATH_HOPS => Some(folder),
            _ => None,
        }
    }

    /// Copies an entry's own name.
    pub(crate) fn push_name(&mut self, name: &[u8]) -> NameRef {
        self.names.push_name(name)
    }

    pub(crate) fn finish(self) -> ResultNames {
        self.names
    }

    /// Copies folder `dir` and returns it with its depth, or `None` when it
    /// has no path. The walk is [`DirPaths::folder_path_into`]'s.
    fn folder(&mut self, index: &VolumeIndex, dir: EntryIdx) -> Option<(FolderId, usize)> {
        let mut chain: SmallVec<[EntryIdx; 32]> = SmallVec::new();
        let mut cur = dir;
        let base = loop {
            if let Some(&known) = self.copied.get(&cur) {
                break known;
            }
            let parent = index.entry(cur).parent();
            if parent == NIL {
                break None; // lost+found
            }
            if chain.len() == MAX_PATH_HOPS {
                // Longer than any walk that has a path, or a cycle. Nothing
                // is copied: the depths on this chain are unknown.
                return None;
            }
            chain.push(cur);
            cur = parent;
        };
        let Some((mut folder, mut depth)) = base else {
            for &f in &chain {
                self.copied.insert(f, None);
            }
            return None;
        };
        for (i, &dir) in chain.iter().rev().enumerate() {
            depth += 1;
            if depth > MAX_PATH_HOPS {
                // This folder and every one below it walk too far.
                for &deeper in chain.iter().rev().skip(i) {
                    self.copied.insert(deeper, None);
                }
                return None;
            }
            folder = self
                .names
                .push_folder(Some(folder), index.entry(dir).name());
            self.copied.insert(dir, Some((folder, depth)));
        }
        Some((folder, depth))
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
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        let mut parent = NTFS_ROOT_FRN;
        let mut entries = Vec::new();
        let rec = |frn: u64, parent: Frn, name: String, dir: bool| ParsedUsnRecord {
            frn: Frn(u128::from(frn) | (1 << 48)),
            parent_frn: parent,
            reason: USN_REASON_FILE_CREATE,
            attributes: if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.into_bytes(),
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

    /// An entry has a full path exactly when `path_of` gives one, both as a
    /// query builds it and as its results' copied folders spell it, whatever
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
        let mut out = Vec::new();
        let mut built_dirs = Vec::new();
        for order in [entries.clone(), entries.iter().rev().copied().collect()] {
            let mut dirs = DirPaths::new(&index, NameForm::Raw, r"C:\");
            let mut copy = FolderCopy::new(&index, r"C:\");
            for idx in order {
                let built = dirs
                    .full_path_into(&index, idx, &mut out)
                    .then(|| out.clone());
                assert_eq!(built, expected(idx), "entry {idx}");
                let copied = copy.entry_folder(&index, idx).map(|folder| {
                    copy.names
                        .full_path_into(folder, index.entry(idx).name(), &mut out);
                    out.clone()
                });
                assert_eq!(copied, expected(idx), "copied entry {idx}");
            }
            built_dirs.push(dirs);
        }
        // Caching every folder of the chain would hold about 100 MB.
        for dirs in built_dirs {
            assert!(
                dirs.arena.len() < 1 << 20,
                "{} bytes cached",
                dirs.arena.len()
            );
        }
    }
}
