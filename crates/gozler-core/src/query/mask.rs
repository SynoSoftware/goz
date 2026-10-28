//! A name mask, such as `*.txt;*.md`, as the window's Select and Unselect
//! commands take it: wildcards apart by `;`, each matched against a whole
//! name without regard to case, as a search's name wildcard is.

use super::wildcard::{CodePointBuf, Wildcard};
use crate::fold::{fold, fold_into};

/// The names that one of its wildcards matches whole. `*.*` matches every
/// name, also one without a dot, as it does in Windows.
#[derive(Debug)]
pub struct NameMask {
    wildcards: Vec<Wildcard>,
}

/// A mask that holds no wildcard, such as an empty text or `;`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the mask holds no wildcard")]
pub struct EmptyMask;

/// What [`NameMask::matches`] reuses from one name to the next, so a pass
/// over many names allocates nothing for each.
#[derive(Debug, Default)]
pub struct MaskScratch {
    folded: Vec<u8>,
    code_points: CodePointBuf,
}

impl NameMask {
    /// The mask that `text` writes.
    ///
    /// # Errors
    ///
    /// [`EmptyMask`] when `text` holds no wildcard.
    pub fn parse(text: &str) -> Result<NameMask, EmptyMask> {
        let wildcards: Vec<Wildcard> = (text.split(';'))
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| if part == "*.*" { "*" } else { part })
            .map(|part| Wildcard::compile(&fold(part.as_bytes())))
            .collect();
        if wildcards.is_empty() {
            return Err(EmptyMask);
        }
        Ok(NameMask { wildcards })
    }

    /// Whether the mask matches `name`, in WTF-8.
    pub fn matches(&self, name: &[u8], scratch: &mut MaskScratch) -> bool {
        scratch.folded.clear();
        fold_into(name, &mut scratch.folded);
        (self.wildcards.iter())
            .any(|wildcard| wildcard.matches_into(&scratch.folded, &mut scratch.code_points))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(mask: &str, name: &str) -> bool {
        let mask = NameMask::parse(mask).expect("the mask holds a wildcard");
        mask.matches(name.as_bytes(), &mut MaskScratch::default())
    }

    /// Each part matches whole names without regard to case.
    #[test]
    fn a_mask_matches_whole_names_by_any_part() {
        assert!(matches("*.txt; *.md", "Notes.TXT"));
        assert!(matches("*.txt; *.md", "read.me.md"));
        assert!(!matches("*.txt; *.md", "notes.txt.bak"));
    }

    /// `*.*` selects every file, as in Windows and Total Commander.
    #[test]
    fn star_dot_star_matches_a_name_without_a_dot() {
        assert!(matches("*.*", "README"));
    }

    #[test]
    fn a_mask_without_a_wildcard_is_refused() {
        assert_eq!(NameMask::parse(" ; ").unwrap_err(), EmptyMask);
    }
}
