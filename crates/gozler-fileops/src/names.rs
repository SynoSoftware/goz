//! The names the engine gives files: Keep both's numbered names, and the
//! temporary name a file is copied under (D10). Names are raw UTF-16, as
//! Windows gives them, because they may hold unpaired surrogates.

use std::collections::HashSet;

/// The longest name, in UTF-16 units, that Windows file systems take.
const MAX_NAME: usize = 255;

/// What a file being copied ends with until it is complete.
pub(crate) const PARTIAL_SUFFIX: &str = ".gozler-partial";

/// `name` as Windows compares names: each UTF-16 unit upper-cased one to
/// one, with no normalization. Neither `gozler_core::fold`, which
/// normalizes, nor `str::to_uppercase`, which turns `ß` into `SS`, matches
/// NTFS, and a false match with Skip would leave a file uncopied.
fn fold(name: &[u16]) -> Vec<u16> {
    name.iter()
        .map(|&unit| {
            let Some(c) = char::from_u32(u32::from(unit)) else {
                return unit; // a surrogate
            };
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) => u16::try_from(u32::from(u)).unwrap_or(unit),
                _ => unit,
            }
        })
        .collect()
}

/// The names of one destination folder, compared as Windows compares them.
#[derive(Debug, Default)]
pub(crate) struct NameSet(HashSet<Vec<u16>>);

impl NameSet {
    pub(crate) fn insert(&mut self, name: &[u16]) {
        self.0.insert(fold(name));
    }

    pub(crate) fn contains(&self, name: &[u16]) -> bool {
        self.0.contains(&fold(name))
    }

    /// Keep both's name for `name`: the first free numbered name from 2,
    /// `report (2).pdf`, or `photos (2)` for a folder, which keeps its dots.
    /// The name is added to the set, so no second file gets it. A numbered
    /// name longer than Windows takes fails when it is created.
    pub(crate) fn keep_both(&mut self, name: &[u16], is_folder: bool) -> Vec<u16> {
        // The extension starts at the last dot, but not at a leading one:
        // `.gitignore` has none.
        let split = match name.iter().rposition(|&u| u == u16::from(b'.')) {
            Some(dot) if dot > 0 && !is_folder => dot,
            _ => name.len(),
        };
        let (stem, extension) = name.split_at(split);
        for number in 2u32.. {
            let mut numbered = stem.to_vec();
            numbered.extend(format!(" ({number})").encode_utf16());
            numbered.extend_from_slice(extension);
            let folded = fold(&numbered);
            if !self.0.contains(&folded) {
                self.0.insert(folded);
                return numbered;
            }
        }
        unreachable!("a folder cannot hold every numbered name")
    }
}

/// The temporary name `name` is copied under: `<name>.gozler-partial`. A
/// name that would pass 255 units keeps the start of `name` that fits, then
/// 8 lowercase hexadecimal digits of the 32-bit FNV-1a hash of `name`'s
/// UTF-16 units, each hashed as its two little-endian bytes. Continue finds
/// an earlier run's partial file by this name, so it is a saved format.
pub(crate) fn partial_name(name: &[u16]) -> Vec<u16> {
    let suffix = PARTIAL_SUFFIX.encode_utf16();
    if name.len() + PARTIAL_SUFFIX.len() <= MAX_NAME {
        return name.iter().copied().chain(suffix).collect();
    }
    let mut keep = MAX_NAME - PARTIAL_SUFFIX.len() - 8;
    // A high surrogate without its low half would make an invalid name.
    if (0xD800..0xDC00).contains(&name[keep - 1]) {
        keep -= 1;
    }
    let hash = name
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
    name[..keep]
        .iter()
        .copied()
        .chain(format!("{hash:08x}").encode_utf16())
        .chain(suffix)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// The first free number skips taken ones in any case, two calls never
    /// give one number, and only a file's extension follows the number.
    #[test]
    fn keep_both_takes_the_first_free_number() {
        let mut names = NameSet::default();
        for name in ["report.pdf", "report (2).pdf", "REPORT (3).PDF"] {
            names.insert(&utf16(name));
        }
        assert_eq!(
            names.keep_both(&utf16("Report.pdf"), false),
            utf16("Report (4).pdf")
        );
        assert_eq!(
            names.keep_both(&utf16("report.pdf"), false),
            utf16("report (5).pdf")
        );
        assert_eq!(names.keep_both(&utf16("v1.2"), true), utf16("v1.2 (2)"));
        assert_eq!(
            names.keep_both(&utf16(".gitignore"), false),
            utf16(".gitignore (2)")
        );
        // ß has no one-unit capital, so it matches only itself.
        names.insert(&utf16("straße"));
        assert!(!names.contains(&utf16("STRASSE")));
        assert!(names.contains(&utf16("STRAßE")));
    }

    /// A name that leaves no room for the suffix still gets a temporary
    /// name Windows takes, and two long names that start alike get two.
    #[test]
    fn a_long_name_gets_a_short_hashed_temporary_name() {
        assert_eq!(partial_name(&utf16("a.txt")), utf16("a.txt.gozler-partial"));
        let one = utf16(&format!("{}1.txt", "x".repeat(245)));
        let two = utf16(&format!("{}2.txt", "x".repeat(245)));
        assert_eq!(one.len(), 250);
        // The hash is a saved format: Continue looks for this exact name.
        let expected = format!("{}e4652282{PARTIAL_SUFFIX}", "x".repeat(232));
        assert_eq!(partial_name(&one), utf16(&expected));
        assert_eq!(expected.len(), MAX_NAME);
        assert_ne!(partial_name(&one), partial_name(&two));
    }
}
