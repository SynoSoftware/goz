//! Which operations wait for each other (D14). An operation waits for an
//! earlier one that has not ended when one of them writes or deletes a path
//! that the other reads or writes, a path counting as the entry and
//! everything below it; copies that move bytes on one disk that seeks wait
//! for each other; nothing else waits.
//!
//! Paths are compared through a trie of their identity components (D12),
//! folded as Windows compares names, so each path of one operation walks
//! the other's trie once, and two large selections cost steps in
//! proportion to their paths, not to their pairs.

use std::collections::HashMap;

use crate::identity::Identity;
use crate::names::{fold, is_numbered};
use crate::operation::Wait;
use crate::volumes::Disk;

/// A set of paths, and of the numbered names that Keep both may give.
#[derive(Debug, Default)]
pub(crate) struct PathSet {
    root: Node,
    paths: Vec<Vec<Vec<u16>>>,
    /// A folder, and a folded name whose numbered names may be written in it.
    numbered: Vec<(Vec<Vec<u16>>, Vec<u16>, bool)>,
}

#[derive(Debug, Default)]
struct Node {
    children: HashMap<Vec<u16>, Node>,
    /// A path of the set ends here.
    end: bool,
    /// Folded names whose numbered names may be written in this folder,
    /// each with whether it names a folder.
    numbered: Vec<(Vec<u16>, bool)>,
}

fn folded(path: &Identity) -> Vec<Vec<u16>> {
    path.components().map(fold).collect()
}

impl PathSet {
    pub(crate) fn insert(&mut self, path: &Identity) {
        let path = folded(path);
        self.node(&path).end = true;
        self.paths.push(path);
    }

    /// `folder\name`, and the numbered names Keep both may give it, which a
    /// copy also writes.
    pub(crate) fn insert_named(&mut self, folder: &Identity, name: &[u16], is_folder: bool) {
        let folder = folded(folder);
        let name = fold(name);
        self.node(&folder).numbered.push((name.clone(), is_folder));
        let mut path = folder.clone();
        path.push(name.clone());
        self.node(&path).end = true;
        self.paths.push(path);
        self.numbered.push((folder, name, is_folder));
    }

    fn node(&mut self, path: &[Vec<u16>]) -> &mut Node {
        let mut node = &mut self.root;
        for part in path {
            node = node.children.entry(part.clone()).or_default();
        }
        node
    }

    /// Whether a path of the set equals `path`, is above it or is below it.
    fn meets(&self, path: &[Vec<u16>]) -> bool {
        let mut node = &self.root;
        for part in path {
            if node.end
                || node
                    .numbered
                    .iter()
                    .any(|(name, f)| is_numbered(part, name, *f))
            {
                return true;
            }
            match node.children.get(part) {
                Some(child) => node = child,
                None => return false,
            }
        }
        node.end || !node.children.is_empty() || !node.numbered.is_empty()
    }

    /// Whether a path of the set meets one of `name`'s numbered names in
    /// `folder`.
    fn meets_numbered(&self, folder: &[Vec<u16>], name: &[u16], is_folder: bool) -> bool {
        let mut node = &self.root;
        for part in folder {
            if node.end {
                return true;
            }
            match node.children.get(part) {
                Some(child) => node = child,
                None => return false,
            }
        }
        let numbered_by = |other: &[u16], f: bool| {
            other == name || is_numbered(other, name, is_folder) || is_numbered(name, other, f)
        };
        node.end
            || node.children.keys().any(|child| numbered_by(child, false))
            || node
                .numbered
                .iter()
                .any(|(other, f)| numbered_by(other, *f))
    }

    fn meets_set(&self, other: &PathSet) -> bool {
        other.paths.iter().any(|path| self.meets(path))
            || (other.numbered.iter())
                .any(|(folder, name, f)| self.meets_numbered(folder, name, *f))
    }
}

/// What a submitted operation holds for the waits.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    pub(crate) reads: PathSet,
    /// The paths it writes or deletes.
    pub(crate) writes: PathSet,
    /// A copy, or a move across volumes.
    pub(crate) moves_bytes: bool,
    /// The disks of every volume it reads or writes, each with whether it
    /// seeks.
    pub(crate) disks: Vec<(Disk, bool)>,
}

/// Why `later` waits for `earlier`, or `None` when it does not.
pub(crate) fn wait(later: &Plan, earlier: &Plan) -> Option<Wait> {
    let paths = later.writes.meets_set(&earlier.reads)
        || later.writes.meets_set(&earlier.writes)
        || earlier.writes.meets_set(&later.reads);
    if paths {
        return Some(Wait::Paths);
    }
    let shared_disk = || {
        (later.disks.iter())
            .any(|(disk, seeks)| *seeks && earlier.disks.iter().any(|(other, _)| other == disk))
    };
    (later.moves_bytes && earlier.moves_bytes && shared_disk()).then_some(Wait::Disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &str) -> Identity {
        Identity::new(path.encode_utf16().collect())
    }

    fn copy(source: &str, folder: &str, disks: &[(Disk, bool)]) -> Plan {
        let source = id(source);
        let mut plan = Plan {
            moves_bytes: true,
            disks: disks.to_vec(),
            ..Plan::default()
        };
        plan.writes.insert_named(&id(folder), source.name(), false);
        plan.reads.insert(&source);
        plan
    }

    fn delete(path: &str) -> Plan {
        let mut plan = Plan::default();
        plan.writes.insert(&id(path));
        plan
    }

    /// Overlapping paths wait in any case and through numbered names; two
    /// copies on one hard disk wait, on one SSD they do not; a delete
    /// beside what a copy writes, or on its disk, does not.
    #[test]
    fn operations_wait_only_for_what_they_share() {
        let hdd = [(Disk::Number(1), true)];
        let ssd = [(Disk::Number(2), false)];
        let a = copy(
            r"\\?\Volume{1}\src\report.pdf",
            r"\\?\Volume{1}\backup",
            &hdd,
        );
        assert_eq!(wait(&delete(r"\\?\VOLUME{1}\SRC"), &a), Some(Wait::Paths));
        assert_eq!(
            wait(&delete(r"\\?\Volume{1}\backup\Report (3).pdf"), &a),
            Some(Wait::Paths)
        );
        assert_eq!(
            wait(&delete(r"\\?\Volume{1}\backup\report.pdf\x"), &a),
            Some(Wait::Paths)
        );
        assert_eq!(wait(&delete(r"\\?\Volume{1}\backup\old.txt"), &a), None);
        assert_eq!(
            wait(&delete(r"\\?\Volume{1}\backup"), &a),
            Some(Wait::Paths)
        );
        assert_eq!(wait(&delete(r"\\?\Volume{1}\srcs"), &a), None);

        let b = copy(r"\\?\Volume{1}\other\a.txt", r"\\?\Volume{1}\out", &hdd);
        assert_eq!(wait(&b, &a), Some(Wait::Disk));
        let c = copy(r"\\?\Volume{2}\x\a.txt", r"\\?\Volume{2}\out", &ssd);
        let d = copy(r"\\?\Volume{2}\y\b.txt", r"\\?\Volume{2}\out2", &ssd);
        assert_eq!(wait(&d, &c), None);
        // Two copies whose numbered names can meet in one folder.
        let e = copy(r"\\?\Volume{2}\z\a (2).txt", r"\\?\Volume{2}\out", &ssd);
        assert_eq!(wait(&e, &c), Some(Wait::Paths));
        let mut deleting = delete(r"\\?\Volume{1}\elsewhere");
        deleting.disks = hdd.to_vec();
        assert_eq!(wait(&deleting, &a), None);
    }
}
