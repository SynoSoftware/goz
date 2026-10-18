//! File attributes as es writes them: one letter for each `FILE_ATTRIBUTE_*`
//! bit that `attrib:`, `/a` and the Attributes column know.
//!
//! The letters, their bits and their order are those of es's
//! `_es_format_attributes`, so `/ah`, `attrib:H` and the Attributes column
//! read as in es. Everything's search syntax page and es's README name some
//! letters differently (V device, P sparse file); es itself prints V for an
//! integrity stream and P for a pinned file, and has no letter for a sparse
//! file.

use serde::{Deserialize, Serialize};

/// One attribute letter: the `FILE_ATTRIBUTE_*` bit it stands for and what
/// the help says about it.
pub(crate) struct Letter {
    pub(crate) letter: char,
    bit: u32,
    pub(crate) meaning: &'static str,
}

const DIRECTORY: u32 = 0x10;
const NORMAL: u32 = 0x80;

/// Every attribute letter, in the order es prints them.
pub(crate) const LETTERS: &[Letter] = &[
    letter('R', 0x1, "read-only"),
    letter('H', 0x2, "hidden"),
    letter('S', 0x4, "system"),
    letter('D', DIRECTORY, "folder"),
    letter('A', 0x20, "archive"),
    letter('V', 0x8000, "integrity stream"),
    letter('X', 0x2_0000, "no scrub data"),
    letter('N', NORMAL, "normal: no other attribute"),
    letter('T', 0x100, "temporary"),
    letter(
        'L',
        0x400,
        "reparse point, such as a symbolic link or a junction",
    ),
    letter('C', 0x800, "compressed"),
    letter('O', 0x1000, "offline"),
    letter('I', 0x2000, "not content indexed"),
    letter('E', 0x4000, "encrypted"),
    letter('U', 0x10_0000, "unpinned"),
    letter('P', 0x8_0000, "pinned"),
    letter('M', 0x40_0000, "recall on data access"),
];

const fn letter(letter: char, bit: u32, meaning: &'static str) -> Letter {
    Letter {
        letter,
        bit,
        meaning,
    }
}

/// Every bit of [`LETTERS`].
const LETTERED: u32 = {
    let mut bits = 0;
    let mut i = 0;
    while i < LETTERS.len() {
        bits |= LETTERS[i].bit;
        i += 1;
    }
    bits
};

/// The letters but D, which [`PackedAttributes`] stores, in [`LETTERS`]
/// order: bit `i` of a packed value is `PACKED[i]`.
const PACKED: [u32; 16] = {
    let mut packed = [0; 16];
    let (mut i, mut at) = (0, 0);
    while i < LETTERS.len() {
        if LETTERS[i].bit != DIRECTORY {
            packed[at] = LETTERS[i].bit;
            at += 1;
        }
        i += 1;
    }
    assert!(at == packed.len(), "every letter but D has a packed bit");
    packed
};

/// A set of attributes: the `FILE_ATTRIBUTE_*` bits of the attribute letters
/// and no other, so every value prints as letters and the index can test
/// every bit. On the wire it is the bits as a number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Attributes(u32);

/// A number that holds a `FILE_ATTRIBUTE_*` bit without an attribute letter,
/// which gozler does not index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:#x} holds attributes that gozler does not index")]
pub struct UnindexedAttributes(pub u32);

impl Attributes {
    /// The attributes that `letters` name, in either case, or the first
    /// character that is no attribute letter.
    pub fn from_letters(letters: &str) -> Result<Self, char> {
        letters.chars().try_fold(Attributes(0), |set, c| {
            let upper = c.to_ascii_uppercase();
            let row = LETTERS.iter().find(|row| row.letter == upper).ok_or(c)?;
            Ok(Attributes(set.0 | row.bit))
        })
    }

    /// The `FILE_ATTRIBUTE_*` bits.
    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every attribute of `other` is in `self`.
    pub fn contains(self, other: Attributes) -> bool {
        self.0 & other.0 == other.0
    }

    /// `self` without the attributes of `other`.
    #[must_use]
    pub fn without(self, other: Attributes) -> Attributes {
        Attributes(self.0 & !other.0)
    }

    /// The letters, upper case, in es's order: `HSA`.
    pub fn letters(self) -> String {
        LETTERS
            .iter()
            .filter(|row| self.0 & row.bit != 0)
            .map(|row| row.letter)
            .collect()
    }

    /// Every attribute letter, in es's order, for messages.
    pub fn all_letters() -> String {
        Attributes(LETTERED).letters()
    }

    /// The letters but D as the index stores them, and whether D is in
    /// `self`, which the entry's folder flag answers.
    pub(crate) fn pack(self) -> (PackedAttributes, bool) {
        let packed = PACKED
            .iter()
            .enumerate()
            .filter(|&(_, &bit)| self.0 & bit != 0)
            .fold(0u16, |packed, (i, _)| packed | (1 << i));
        (PackedAttributes(packed), self.0 & DIRECTORY != 0)
    }
}

impl TryFrom<u32> for Attributes {
    type Error = UnindexedAttributes;

    fn try_from(bits: u32) -> Result<Self, Self::Error> {
        if bits & !LETTERED == 0 {
            Ok(Attributes(bits))
        } else {
            Err(UnindexedAttributes(bits))
        }
    }
}

impl From<Attributes> for u32 {
    fn from(attributes: Attributes) -> u32 {
        attributes.0
    }
}

/// [`Attributes`] as the index stores them for each entry: one bit for each
/// letter but D, in [`LETTERS`] order, because the entry's folder flag
/// already says D. Two bytes an entry, where the record's `u32` would take
/// four, and the index holds millions of entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackedAttributes(u16);

impl PackedAttributes {
    /// The attributes of the `FILE_ATTRIBUTE_*` bits that a journal record,
    /// the MFT enumeration or FILE_LAYOUT reports for a file. N means that no
    /// other attribute is set, as Windows defines `FILE_ATTRIBUTE_NORMAL`,
    /// whether or not the record sets its bit; an attribute without a letter
    /// is dropped.
    pub(crate) fn from_record(bits: u32) -> Self {
        let normal = if bits & !NORMAL == 0 { NORMAL } else { 0 };
        let (packed, _) = Attributes((bits & LETTERED & !NORMAL) | normal).pack();
        packed
    }

    /// The attributes, with D when the entry is a folder.
    pub(crate) fn unpack(self, is_dir: bool) -> Attributes {
        // Visits only the set bits, one or two for most files, because every
        // result row of a broad query is unpacked.
        let mut bits = if is_dir { DIRECTORY } else { 0 };
        let mut packed = self.0;
        while packed != 0 {
            bits |= PACKED[packed.trailing_zeros() as usize];
            packed &= packed - 1;
        }
        Attributes(bits)
    }

    /// Whether every attribute of `other` is in `self`.
    pub(crate) fn contains(self, other: PackedAttributes) -> bool {
        self.0 & other.0 == other.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index keeps what a record reports for each letter, derives N from
    /// the other bits as Windows does, and drops what has no letter: here
    /// the sparse bit, 0x200.
    #[test]
    fn a_record_packs_and_unpacks_to_its_lettered_attributes() {
        let cases = [
            (0x2 | 0x4 | 0x20 | 0x200, false, "HSA"),
            (0, false, "N"),
            (NORMAL, false, "N"),
            (0x200, false, ""),
            (DIRECTORY, true, "D"),
            (DIRECTORY | 0x2 | 0x40_0000, true, "HDM"),
        ];
        for (bits, is_dir, letters) in cases {
            let attributes = PackedAttributes::from_record(bits).unpack(is_dir);
            assert_eq!(attributes.letters(), letters, "{bits:#x}");
        }
    }
}
