//! The names of a destination folder, and Keep both's numbered names. Names
//! are raw UTF-16, as Windows gives them, because they may hold unpaired
//! surrogates.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

mod upper;

/// `name` as Windows compares names: each UTF-16 unit upper-cased one to
/// one, as `CharUpperW` does, with no normalization. Neither
/// `gozler_core::fold`, which normalizes, nor `char::to_uppercase`, which
/// turns `ß` into `SS` and differs from `CharUpperW` in other units, matches
/// NTFS, and a false match with Skip would leave a file uncopied.
pub(crate) fn fold(name: &[u16]) -> Vec<u16> {
    let mut folded = Vec::with_capacity(name.len());
    fold_into(name, &mut folded);
    folded
}

/// [`fold`] into `folded`, a buffer that the caller reuses for many names.
pub(crate) fn fold_into(name: &[u16], folded: &mut Vec<u16>) {
    folded.clear();
    folded.extend(name.iter().map(|&unit| match unit {
        // The table changes no ASCII unit but a–z, which most paths hold.
        0x61..=0x7a => unit - 0x20,
        0..0x80 => unit,
        _ => match upper::UPPER.binary_search_by_key(&unit, |&(lower, _)| lower) {
            Ok(i) => upper::UPPER[i].1,
            Err(_) => unit,
        },
    }));
}

/// What a destination entry is, for the conflict rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Folder,
    /// A folder whose reparse tag names another entry, such as a junction:
    /// never merged into, because writing through it would write outside
    /// the destination.
    FolderLink,
}

/// An entry that a destination folder held when it was listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) kind: Kind,
    pub(crate) size: u64,
    /// As `FILETIME`s, in 100 ns units.
    pub(crate) created: i64,
    pub(crate) last_write: i64,
    /// It has a reparse tag, such as a file symbolic link's, which a copy
    /// would write through to wherever it points.
    pub(crate) reparse_point: bool,
}

/// What holds a name in a destination folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder {
    Found(Found),
    /// This operation gave the name to one of its entries.
    Ours,
}

/// The names of one destination folder, compared as Windows compares them.
#[derive(Debug, Default)]
pub(crate) struct Names(HashMap<Vec<u16>, Holder>);

impl Names {
    pub(crate) fn insert(&mut self, name: &[u16], holder: Holder) {
        self.0.insert(fold(name), holder);
    }

    pub(crate) fn get(&self, name: &[u16]) -> Option<Holder> {
        self.0.get(&fold(name)).copied()
    }

    /// Keep both's name for `name`: the first free numbered name from 2,
    /// `report (2).pdf`, or `photos (2)` for a folder, which keeps its dots.
    /// The name becomes ours, so no second entry gets it. A numbered name
    /// longer than Windows takes fails when it is created.
    pub(crate) fn keep_both(&mut self, name: &[u16], is_folder: bool) -> Vec<u16> {
        let (stem, extension) = split(name, is_folder);
        for number in 2u32.. {
            let numbered = numbered(stem, extension, number);
            if let Entry::Vacant(free) = self.0.entry(fold(&numbered)) {
                free.insert(Holder::Ours);
                return numbered;
            }
        }
        unreachable!("a folder cannot hold every numbered name")
    }

    /// `name`'s numbered names and their holders, from 2 up to the first
    /// free one, which is where Keep both stops.
    pub(crate) fn numbered(
        &self,
        name: &[u16],
        is_folder: bool,
    ) -> impl Iterator<Item = (Vec<u16>, Holder)> {
        let (stem, extension) = split(name, is_folder);
        (2u32..).map_while(move |number| {
            let name = numbered(stem, extension, number);
            let holder = self.get(&name)?;
            Some((name, holder))
        })
    }
}

/// Whether the folded name `candidate` is one of Keep both's numbered names
/// for the folded name `name`.
#[cfg(test)]
pub(crate) fn is_numbered(candidate: &[u16], name: &[u16], is_folder: bool) -> bool {
    let (stem, extension) = split(name, is_folder);
    let Some(rest) = candidate.strip_prefix(stem) else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(extension) else {
        return false;
    };
    let Some(digits) = rest
        .strip_prefix(&[u16::from(b' '), u16::from(b'(')][..])
        .and_then(|rest| rest.strip_suffix(&[u16::from(b')')][..]))
    else {
        return false;
    };
    !digits.is_empty()
        && digits[0] != u16::from(b'0')
        && digits
            .iter()
            .all(|&unit| (u16::from(b'0')..=u16::from(b'9')).contains(&unit))
}

/// The stem and the extension that Keep both puts the number between. The
/// extension starts at the last dot, but not at a leading one: `.gitignore`
/// has none, and nor has a folder.
fn split(name: &[u16], is_folder: bool) -> (&[u16], &[u16]) {
    let at = match name.iter().rposition(|&unit| unit == u16::from(b'.')) {
        Some(dot) if dot > 0 && !is_folder => dot,
        _ => name.len(),
    };
    name.split_at(at)
}

fn numbered(stem: &[u16], extension: &[u16], number: u32) -> Vec<u16> {
    let mut name = stem.to_vec();
    name.extend(format!(" ({number})").encode_utf16());
    name.extend_from_slice(extension);
    name
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
        let mut names = Names::default();
        for name in ["report.pdf", "report (2).pdf", "REPORT (3).PDF"] {
            names.insert(&utf16(name), Holder::Ours);
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
        assert_eq!(names.numbered(&utf16("report.pdf"), false).count(), 4);
        let report = fold(&utf16("report.pdf"));
        assert!(is_numbered(
            &fold(&utf16("Report (12).PDF")),
            &report,
            false
        ));
        assert!(!is_numbered(
            &fold(&utf16("report (x).pdf")),
            &report,
            false
        ));
        assert!(!is_numbered(&fold(&utf16("report ().pdf")), &report, false));
        // Windows matches only one-to-one capitals, as CharUpperW gives them:
        // ß matches only itself, and dotless ı is not I.
        names.insert(&utf16("straße"), Holder::Ours);
        assert_eq!(names.get(&utf16("STRASSE")), None);
        assert_eq!(names.get(&utf16("STRAßE")), Some(Holder::Ours));
        names.insert(&utf16("ı"), Holder::Ours);
        assert_eq!(names.get(&utf16("I")), None);
    }
}
