//! Paths compared by what they name on disk (D12), and the trie that tests
//! whether one path is inside another.

use std::collections::HashMap;

const BACKSLASH: u16 = b'\\' as u16;

/// What a path names: its final path, which Windows gives once for every
/// spelling of one entry (`gozler_winfs::Entry::final_path`), so equal
/// identities are one entry. Compared exactly, because the final path
/// already carries the names as the disk stores them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Identity(Vec<u16>);

impl Identity {
    pub(crate) fn new(mut final_path: Vec<u16>) -> Self {
        // A volume root ends with a backslash; no other path does.
        if final_path.last() == Some(&BACKSLASH) {
            final_path.pop();
        }
        Identity(final_path)
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = &[u16]> {
        self.0.split(|&unit| unit == BACKSLASH)
    }

    /// The folder that holds the entry; `None` for a volume root.
    pub(crate) fn parent(&self) -> Option<Identity> {
        let at = self.0.iter().rposition(|&unit| unit == BACKSLASH)?;
        let parent = Identity(self.0[..at].to_vec());
        (parent.0.len() >= self.volume().len()).then_some(parent)
    }

    /// The entry `name` in this folder.
    pub(crate) fn child(&self, name: &[u16]) -> Identity {
        let mut path = self.0.clone();
        path.push(BACKSLASH);
        path.extend_from_slice(name);
        Identity(path)
    }

    /// The entry's name, its last component.
    pub(crate) fn name(&self) -> &[u16] {
        self.components()
            .last()
            .expect("split gives at least one part")
    }

    /// The volume part: `\\?\Volume{...}` or `\\?\UNC\server\share`.
    pub(crate) fn volume(&self) -> &[u16] {
        let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        let separators = if self.0.starts_with(&unc) { 6 } else { 4 };
        let end = (self.0.iter().enumerate())
            .filter(|&(_, &unit)| unit == BACKSLASH)
            .nth(separators - 1)
            .map_or(self.0.len(), |(i, _)| i);
        &self.0[..end]
    }

    /// The path below the volume part, without a leading backslash; empty
    /// for a volume root.
    pub(crate) fn below_volume(&self) -> &[u16] {
        let rest = &self.0[self.volume().len()..];
        rest.strip_prefix(&[BACKSLASH]).unwrap_or(rest)
    }
}

/// A set of paths, by component, that answers whether a path equals or is
/// below one of them in one walk down the path.
#[derive(Debug, Default)]
pub(crate) struct PathTrie {
    children: HashMap<Vec<u16>, PathTrie>,
    /// A path of the set ends here.
    end: bool,
}

impl PathTrie {
    pub(crate) fn insert(&mut self, path: &Identity) {
        let mut node = self;
        for part in path.components() {
            node = node.children.entry(part.to_vec()).or_default();
        }
        node.end = true;
    }

    /// Whether `path` equals or is below a path of the set.
    pub(crate) fn covers(&self, path: &Identity) -> bool {
        let mut node = self;
        for part in path.components() {
            if node.end {
                return true;
            }
            match node.children.get(part) {
                Some(child) => node = child,
                None => return false,
            }
        }
        node.end
    }
}

/// The selected entries to act on, as indices in selection order. An entry
/// below another selected folder is dropped, because it goes with that
/// folder, and so is a second spelling of one entry. Returns them with the
/// trie of what is kept.
pub(crate) fn outermost(sources: &[Identity]) -> (Vec<usize>, PathTrie) {
    let mut order: Vec<usize> = (0..sources.len()).collect();
    // An outer folder has fewer components than anything below it, so it is
    // in the trie before them.
    order.sort_by_key(|&i| sources[i].components().count());
    let mut trie = PathTrie::default();
    let mut kept = vec![false; sources.len()];
    for i in order {
        if !trie.covers(&sources[i]) {
            trie.insert(&sources[i]);
            kept[i] = true;
        }
    }
    let kept = (0..sources.len()).filter(|&i| kept[i]).collect();
    (kept, trie)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &str) -> Identity {
        Identity::new(path.encode_utf16().collect())
    }

    /// An entry inside a selected folder, and a second spelling of an
    /// entry, are acted on once; a name that only starts like a selected
    /// folder is not inside it.
    #[test]
    fn nested_and_repeated_entries_are_dropped() {
        let sources = [
            id(r"\\?\Volume{1}\proj\src\main.rs"),
            id(r"\\?\Volume{1}\proj"),
            id(r"\\?\Volume{1}\project"),
            id(r"\\?\Volume{1}\proj"),
            id(r"\\?\Volume{2}\proj\src"),
        ];
        let (kept, trie) = outermost(&sources);
        assert_eq!(kept, [1, 2, 4]);
        // A destination inside a source is refused through the same trie.
        assert!(trie.covers(&id(r"\\?\Volume{1}\proj\out")));
        assert!(trie.covers(&id(r"\\?\Volume{1}\proj")));
        assert!(!trie.covers(&id(r"\\?\Volume{1}")));
        assert!(!trie.covers(&id(r"\\?\Volume{1}\pro")));
    }
}
