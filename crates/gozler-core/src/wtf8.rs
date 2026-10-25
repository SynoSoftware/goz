//! WTF-8 encoding for NTFS names.
//!
//! NTFS filenames are arbitrary sequences of `u16` code units. Unpaired
//! surrogates occur in the wild. WTF-8 is UTF-8 extended to also encode lone
//! surrogate code points (as their 3-byte sequences), which makes the
//! UTF-16 → WTF-8 conversion total and lossless: every possible NTFS name
//! round-trips exactly. Valid Unicode names produce plain UTF-8 bytes.

/// Converts potentially ill-formed UTF-16 to WTF-8, appending to `out`.
///
/// The bytes are valid UTF-8 exactly when every surrogate was paired, so a
/// consumer tells a lossy name by `str::from_utf8` failing.
pub fn from_utf16(units: &[u16], out: &mut Vec<u8>) {
    extend_from_units(units.iter().copied(), out);
}

/// Converts a UTF-16LE name read from a disk structure to WTF-8. The caller
/// rejects an odd byte length first, because a trailing odd byte is ignored.
pub(crate) fn from_utf16le(bytes: &[u8]) -> Vec<u8> {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair));
    let mut out = Vec::with_capacity(bytes.len());
    extend_from_units(units, &mut out);
    out
}

/// Appends the WTF-8 of `units` to `out`.
fn extend_from_units(units: impl IntoIterator<Item = u16>, out: &mut Vec<u8>) {
    for decoded in char::decode_utf16(units) {
        let cp = match decoded {
            Ok(c) => u32::from(c),
            Err(e) => u32::from(e.unpaired_surrogate()),
        };
        encode_code_point(cp, out);
    }
}

/// Encodes one code point (surrogates permitted, the WTF-8 generalization)
/// as 1-4 bytes.
pub(crate) fn encode_code_point(cp: u32, out: &mut Vec<u8>) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.push(0xC0 | (cp >> 6) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    } else if cp < 0x10000 {
        out.push(0xE0 | (cp >> 12) as u8);
        out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    } else {
        out.push(0xF0 | (cp >> 18) as u8);
        out.push(0x80 | ((cp >> 12) & 0x3F) as u8);
        out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    }
}

/// Iterator over the code points of well-formed WTF-8 bytes.
///
/// Total for bytes produced by [`from_utf16`]; malformed input (which we
/// never produce) yields U+FFFD per bogus byte rather than panicking, so the
/// decoder is safe on untrusted data too.
pub struct CodePoints<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> CodePoints<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, i: 0 }
    }
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let b = *self.bytes.get(self.i)?;
        let (len, init) = match b {
            0x00..=0x7F => (1, b as u32),
            0xC0..=0xDF => (2, (b & 0x1F) as u32),
            0xE0..=0xEF => (3, (b & 0x0F) as u32),
            0xF0..=0xF7 => (4, (b & 0x07) as u32),
            _ => {
                self.i += 1;
                return Some(0xFFFD);
            }
        };
        if self.i + len > self.bytes.len() {
            self.i += 1;
            return Some(0xFFFD);
        }
        let mut cp = init;
        for k in 1..len {
            let c = self.bytes[self.i + k];
            if c & 0xC0 != 0x80 {
                self.i += 1;
                return Some(0xFFFD);
            }
            cp = (cp << 6) | (c & 0x3F) as u32;
        }
        // Reject overlong encodings and out-of-range code points, matching
        // std's `from_utf8_lossy`. Surrogates (3-byte, cp in 0x800..=0x10FFFF)
        // are intentionally still accepted, which is WTF-8's whole purpose, so
        // this only rejects a shorter value smuggled in a longer form (e.g. an
        // overlong-encoded '/') or a 4-byte value above U+10FFFF.
        let min = [0u32, 0, 0x80, 0x800, 0x10000][len];
        if cp < min || cp > 0x10FFFF {
            self.i += 1; // not minimal / out of range: resync one byte at a time
            return Some(0xFFFD);
        }
        self.i += len;
        Some(cp)
    }
}

/// Converts WTF-8 back to the exact UTF-16 code units it came from.
pub fn to_utf16(bytes: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(bytes.len());
    for cp in CodePoints::new(bytes) {
        if cp < 0x10000 {
            out.push(cp as u16);
        } else {
            let v = cp - 0x10000;
            out.push(0xD800 + (v >> 10) as u16);
            out.push(0xDC00 + (v & 0x3FF) as u16);
        }
    }
    out
}

/// Converts WTF-8 to a `String`, replacing each lone surrogate with U+FFFD.
pub fn to_string_lossy(bytes: &[u8]) -> String {
    // Fast path: bytes with no encoded surrogates are already valid UTF-8.
    if let Ok(s) = core::str::from_utf8(bytes) {
        return s.to_owned();
    }
    let mut s = String::with_capacity(bytes.len());
    for cp in CodePoints::new(bytes) {
        s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WTF-8 of `units`, and whether a consumer sees it as lossy.
    fn wtf8(units: &[u16]) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        from_utf16(units, &mut out);
        let lossy = core::str::from_utf8(&out).is_err();
        (out, lossy)
    }

    #[test]
    fn bmp_and_supplementary_match_utf8() {
        for s in ["ağaç.pdf", "文件名.txt", "emoji-🎉-😀.png", "ß.md"] {
            let units: Vec<u16> = s.encode_utf16().collect();
            let (bytes, lone) = wtf8(&units);
            assert_eq!(bytes, s.as_bytes());
            assert!(!lone);
            assert_eq!(to_string_lossy(&bytes), s);
        }
    }

    #[test]
    fn lone_surrogates_round_trip_exactly() {
        // high-lone, low-lone, high-at-end
        for units in [
            vec![0x0061, 0xD800, 0x0062],
            vec![0xDC00, 0x0041],
            vec![0x0041, 0xD9FF],
        ] {
            let (bytes, lone) = wtf8(&units);
            assert!(lone);
            assert_eq!(to_utf16(&bytes), units, "units {units:X?}");
            // Lossy form replaces the surrogate but never fails.
            assert!(to_string_lossy(&bytes).contains('\u{FFFD}'));
        }
    }

    #[test]
    fn decoder_survives_malformed_bytes() {
        // Truncated multi-byte seq + stray continuation: FFFDs, no panic.
        let got: Vec<u32> = CodePoints::new(&[0xE0, 0x80, b'a', 0xFF]).collect();
        assert!(got.contains(&(b'a' as u32)));
        assert!(got.contains(&0xFFFD));
    }

    #[test]
    fn overlong_and_over_range_decode_to_replacement() {
        // Overlong '/' (2- and 3-byte forms) must NOT canonicalize to a real
        // slash: that would let a corrupt blob smuggle a path separator.
        assert!(!to_string_lossy(&[0xC0, 0xAF]).contains('/'));
        assert!(to_string_lossy(&[0xC0, 0xAF]).contains('\u{FFFD}'));
        assert!(!to_string_lossy(&[0xE0, 0x80, 0xAF]).contains('/'));
        // 4-byte value above U+10FFFF (0x1FFFFF) is rejected (resynced one byte
        // at a time to U+FFFD), not turned into a garbage surrogate pair.
        let over = to_utf16(&[0xF7, 0xBF, 0xBF, 0xBF]);
        assert!(!over.is_empty() && over.iter().all(|&u| u == 0xFFFD));
        // A genuine lone surrogate (3-byte, cp >= 0x800) is still accepted.
        let mut sur = Vec::new();
        from_utf16(&[0xD800], &mut sur);
        assert_eq!(to_utf16(&sur), vec![0xD800]);
    }

    use proptest::prelude::*;

    proptest! {
        /// Arbitrary UTF-16 (unpaired surrogates included) round-trips
        /// byte-for-byte through WTF-8: the module's total/lossless guarantee
        /// over the full u16 space.
        #[test]
        fn utf16_round_trips_through_wtf8(units in proptest::collection::vec(any::<u16>(), 0..64)) {
            let mut bytes = Vec::new();
            from_utf16(&units, &mut bytes);
            prop_assert_eq!(to_utf16(&bytes), units);
        }

        /// The decoder and `to_string_lossy` are total on any bytes (no panic,
        /// no infinite loop).
        #[test]
        fn decoder_total_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
            let _ = CodePoints::new(&bytes).count();
            let _ = to_string_lossy(&bytes);
        }
    }
}
