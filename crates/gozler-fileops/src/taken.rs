//! Taken entries: the paths that the window leaves out of the panels and
//! shows blank in the search results, because a delete or a move has them.
//!
//! This is a display rule on the path text the window gave; the safety rule
//! is D14 on identities. The window asks for every row it paints, so a row
//! is never compared with every taken path, which a large delete would make
//! millions of compares per paint: a row is taken when its folded path or
//! one of its parent paths is in a hash set, one lookup per component.

use std::collections::HashSet;
use std::path::Path;

use crate::names::fold;

const BACKSLASH: u16 = b'\\' as u16;

/// The folded full paths of the sources of every waiting, running or ended
/// delete or move. The window takes a snapshot of it at each wake, so a
/// paint never waits for the engine's lock.
#[derive(Debug, Default, Clone)]
pub struct Taken(HashSet<Vec<u16>>);

impl Taken {
    pub(crate) fn insert(&mut self, path: &Path) {
        self.0.insert(key(path));
    }

    /// Whether `path` equals or is below a taken path, compared as Windows
    /// compares names, ignoring case.
    pub fn contains(&self, path: &Path) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let mut path = key(path);
        loop {
            if self.0.contains(&path) {
                return true;
            }
            match path.iter().rposition(|&unit| unit == BACKSLASH) {
                Some(at) => path.truncate(at),
                None => return false,
            }
        }
    }
}

fn key(path: &Path) -> Vec<u16> {
    let mut units = fold(&units(path));
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

    /// A row equal to a taken path, or below it in another case, is taken;
    /// a row beside it whose name only starts like it is not.
    #[test]
    fn rows_below_a_taken_path_are_taken() {
        let mut taken = Taken::default();
        taken.insert(Path::new(r"C:\a"));
        assert!(taken.contains(Path::new(r"C:\a")));
        assert!(taken.contains(Path::new(r"c:\A\b\c.txt")));
        assert!(!taken.contains(Path::new(r"C:\ab")));
        assert!(!taken.contains(Path::new(r"C:\")));
    }
}
