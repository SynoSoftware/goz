//! The Viewer's decisions (DESIGN.md: The Viewer) that need neither a window
//! nor a disk: Text or Hex, the encoding, decoding, where a line starts, the
//! text of a hex row and the scroll position of a byte.
//!
//! Every function takes the bytes and the file offsets that the caller has
//! read, so the reading thread and the window own the file, the font, the
//! wrapping and the status bar. The bytes are untrusted: no function panics
//! on any input, and each allocates in proportion to its input.
//!
//! The code page for text without a byte order mark that is not UTF-8 is
//! Windows' ANSI code page, which only Win32 can decode; the caller passes
//! that decoding to [`decode_piece`]. The line functions count a byte of
//! that code page as a character.

/// How many bytes from the start of the file choose Text or Hex and the
/// encoding.
pub(crate) const SNIFF_BYTES: usize = 64 * 1024;

/// A line longer than this many characters is shown in pieces of this
/// length, so that finding a line's start never reads back further.
pub(crate) const PIECE_CHARS: usize = 10_000;

/// A piece also ends after this many bytes, which [`PIECE_CHARS`] valid
/// characters never exceed. Bytes that are no character, such as a run of
/// UTF-8 continuation bytes, would otherwise make a piece unbounded.
const PIECE_BYTES: usize = PIECE_CHARS * 4;

/// Reading this many bytes before or after a line start always decides the
/// other end of a line piece: [`PIECE_BYTES`] and a following CR LF.
pub(crate) const LINE_BYTES: usize = PIECE_BYTES + 4;

/// The bytes in one hex row.
pub(crate) const HEX_ROW_BYTES: usize = 16;

/// A tab moves to the next multiple of this many columns.
const TAB_COLUMNS: usize = 8;

/// The largest value of a Win32 scroll bar's position.
const SCROLL_MAX: u64 = i32::MAX as u64;

const REPLACEMENT: char = '\u{FFFD}';

/// What the Viewer shows a file as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Text,
    Hex,
}

/// How the bytes of a text file spell its characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Windows' ANSI code page.
    Ansi,
}

impl Encoding {
    /// The name the status bar shows.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Ansi => "ANSI",
        }
    }

    /// The bytes in one code unit.
    fn width(self) -> usize {
        match self {
            Encoding::Utf8 | Encoding::Ansi => 1,
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
        }
    }

    /// What the code unit in `unit`, exactly [`Encoding::width`] bytes,
    /// means for finding lines.
    fn kind(self, unit: &[u8]) -> Unit {
        let value = match self {
            Encoding::Utf8 | Encoding::Ansi => u16::from(unit[0]),
            Encoding::Utf16Le => u16::from_le_bytes([unit[0], unit[1]]),
            Encoding::Utf16Be => u16::from_be_bytes([unit[0], unit[1]]),
        };
        match value {
            0x0D => Unit::Cr,
            0x0A => Unit::Lf,
            0x80..=0xBF if self == Encoding::Utf8 => Unit::Continuation,
            0xDC00..=0xDFFF => Unit::Continuation,
            _ => Unit::Char,
        }
    }
}

/// A code unit as far as finding lines is concerned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Cr,
    Lf,
    /// The second part of a character: a UTF-8 continuation byte or the
    /// low half of a UTF-16 surrogate pair.
    Continuation,
    /// A unit that starts a character.
    Char,
}

/// A file's encoding and where its text starts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Detected {
    pub(crate) encoding: Encoding,
    /// The offset of the first character: after the byte order mark, else 0.
    pub(crate) data_start: u64,
}

fn byte_order_mark(first: &[u8]) -> Option<Detected> {
    let (encoding, data_start) = match first {
        [0xEF, 0xBB, 0xBF, ..] => (Encoding::Utf8, 3),
        [0xFF, 0xFE, ..] => (Encoding::Utf16Le, 2),
        [0xFE, 0xFF, ..] => (Encoding::Utf16Be, 2),
        _ => return None,
    };
    Some(Detected {
        encoding,
        data_start,
    })
}

/// Hex when the first [`SNIFF_BYTES`] of the file hold a zero byte and no
/// UTF-16 byte order mark, otherwise Text. UTF-16 without a byte order mark
/// holds zero bytes, so it shows as Hex.
pub(crate) fn choose_mode(first: &[u8]) -> Mode {
    let first = &first[..first.len().min(SNIFF_BYTES)];
    let utf16 = matches!(
        byte_order_mark(first),
        Some(Detected {
            encoding: Encoding::Utf16Le | Encoding::Utf16Be,
            ..
        })
    );
    if first.contains(&0) && !utf16 {
        Mode::Hex
    } else {
        Mode::Text
    }
}

/// The encoding of a text file from its first bytes: the byte order mark if
/// there is one, else UTF-8 when the first [`SNIFF_BYTES`] are valid UTF-8,
/// else ANSI. `first` is as much of the file as the caller read, up to
/// [`SNIFF_BYTES`]; a character cut off by the end of exactly
/// [`SNIFF_BYTES`] bytes does not make the text invalid, because the file
/// goes on.
pub(crate) fn detect_encoding(first: &[u8]) -> Detected {
    let first = &first[..first.len().min(SNIFF_BYTES)];
    if let Some(detected) = byte_order_mark(first) {
        return detected;
    }
    let utf8 = match std::str::from_utf8(first) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && first.len() == SNIFF_BYTES,
    };
    Detected {
        encoding: if utf8 { Encoding::Utf8 } else { Encoding::Ansi },
        data_start: 0,
    }
}

/// Decodes `bytes`, a line piece that starts at a character, with � for
/// each byte sequence the encoding cannot read, as Notepad shows them. A
/// character that the piece's end cuts off, such as at the byte limit of a
/// long line's piece, also shows as �.
///
/// `ansi` decodes bytes in the ANSI code page. It is only called for
/// [`Encoding::Ansi`].
pub(crate) fn decode_piece(
    encoding: Encoding,
    bytes: &[u8],
    ansi: &dyn Fn(&[u8]) -> String,
) -> String {
    match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Utf16Le => decode_utf16(bytes, u16::from_le_bytes),
        Encoding::Utf16Be => decode_utf16(bytes, u16::from_be_bytes),
        Encoding::Ansi => ansi(bytes),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let (units, rest) = bytes.as_chunks::<2>();
    let mut text: String = char::decode_utf16(units.iter().map(|&pair| unit(pair)))
        .map(|c| c.unwrap_or(REPLACEMENT))
        .collect();
    // A single byte after the last whole unit can never become a character.
    if !rest.is_empty() {
        text.push(REPLACEMENT);
    }
    text
}

/// `line` without its terminator: one CR LF, LF or CR.
pub(crate) fn without_terminator(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .or_else(|| line.strip_suffix('\r'))
        .unwrap_or(line)
}

/// `line` with each tab replaced by the spaces that reach the next multiple
/// of 8 columns. A column is one character.
pub(crate) fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let spaces = TAB_COLUMNS - column % TAB_COLUMNS;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += 1;
        }
    }
    out
}

/// The offset where the line, or the piece of a long line, that holds
/// `position` starts, with `position` itself at the start of a line. A
/// `position` at the end of a piece belongs to that piece, so Up from a
/// piece's start reaches the piece before it. `position` is at least
/// `data_start`.
///
/// `before` are the bytes of the file that end at `position`: at least the
/// smaller of [`LINE_BYTES`] and `position - data_start`; the bytes before
/// `data_start` and the bytes beyond [`PIECE_BYTES`] are ignored, so the
/// answer does not depend on how many more the caller passes. Where the line
/// starts within those bytes, at a terminator or at `data_start`, the pieces
/// are the ones [`next_line_start`] makes from there. Otherwise the line
/// started too far back to know, and the piece starts [`PIECE_CHARS`]
/// characters before `position`, or [`PIECE_BYTES`] bytes when they are no
/// characters. A `position` inside a character starts the line of that
/// character.
pub(crate) fn line_start(encoding: Encoding, data_start: u64, before: &[u8], position: u64) -> u64 {
    let width = encoding.width();
    let available = position.saturating_sub(data_start);
    let usable = before
        .len()
        .min(usize::try_from(available).unwrap_or(usize::MAX))
        .min(PIECE_BYTES);
    let before = &before[before.len() - usable..];
    let base = position.saturating_sub(usable as u64);
    // Units line up with `data_start`, so a `position` inside a unit starts
    // counting at the unit before it.
    let end = usable.saturating_sub((available % width as u64) as usize);
    let mut front = end;
    let mut chars = 0;
    let mut capped = None;
    let mut terminated = false;
    while front >= width {
        let start = front - width;
        match encoding.kind(&before[start..front]) {
            Unit::Cr | Unit::Lf => {
                terminated = true;
                break;
            }
            Unit::Continuation => {}
            Unit::Char => {
                chars += 1;
                if chars == PIECE_CHARS {
                    capped = Some(start);
                }
            }
        }
        front = start;
    }
    // `front` is where the terminator ends, or the front of `before`.
    if !terminated && usable as u64 != available {
        return base + capped.unwrap_or(front) as u64;
    }
    let mut piece = front;
    while piece < end {
        let rest = &before[piece..end];
        let len = piece_len(encoding, rest, true).unwrap_or(rest.len());
        if piece + len >= end {
            break;
        }
        piece += len;
    }
    base + piece as u64
}

/// The start of the line before the one that starts at `position`.
/// `before` are the bytes that end at `position`, as for [`line_start`].
pub(crate) fn previous_line_start(
    encoding: Encoding,
    data_start: u64,
    before: &[u8],
    position: u64,
) -> u64 {
    let width = encoding.width();
    let back = |n: usize| -> Option<Unit> {
        let end = before.len().checked_sub((n - 1) * width)?;
        let start = end.checked_sub(width)?;
        Some(encoding.kind(&before[start..end]))
    };
    let units = match (back(1), back(2)) {
        (Some(Unit::Lf), Some(Unit::Cr)) => 2,
        (Some(Unit::Lf | Unit::Cr), _) => 1,
        _ => 0,
    };
    let cut = units * width;
    line_start(
        encoding,
        data_start,
        &before[..before.len() - cut],
        position.saturating_sub(cut as u64),
    )
}

/// The start of the line after the one that starts at `position`, or `None`
/// when `ahead` ends before that is known. `ahead` are the bytes of the file
/// from `position` on: [`LINE_BYTES`] of them always decide, or all that
/// remain, with `last` set, when the file ends sooner. With `last`, a line
/// that runs to the end of `ahead` ends at the end of the file, and the
/// result is the file's length.
///
/// A line ends after its terminator, after [`PIECE_CHARS`] characters or
/// after [`PIECE_BYTES`] bytes. A terminator that follows exactly one of
/// those belongs to that piece.
pub(crate) fn next_line_start(
    encoding: Encoding,
    ahead: &[u8],
    position: u64,
    last: bool,
) -> Option<u64> {
    piece_len(encoding, ahead, last).map(|len| position + len as u64)
}

/// The bytes of the line piece that starts `ahead`, as [`next_line_start`]
/// decides them.
fn piece_len(encoding: Encoding, ahead: &[u8], last: bool) -> Option<usize> {
    let width = encoding.width();
    let units = ahead.len() / width;
    let mut chars = 0;
    for i in 0..units {
        let offset = i * width;
        let kind = encoding.kind(&ahead[offset..offset + width]);
        match kind {
            Unit::Lf => return Some(offset + width),
            Unit::Cr if i + 1 < units => {
                let next = &ahead[offset + width..offset + 2 * width];
                let lf = encoding.kind(next) == Unit::Lf;
                return Some(offset + width * (1 + usize::from(lf)));
            }
            // Whether an LF follows is not known yet.
            Unit::Cr if last => return Some(offset + width),
            Unit::Cr => return None,
            Unit::Continuation | Unit::Char => {
                let full = kind == Unit::Char && chars == PIECE_CHARS;
                if full || offset >= PIECE_BYTES {
                    return Some(offset);
                }
                chars += usize::from(kind == Unit::Char);
            }
        }
    }
    last.then_some(ahead.len())
}

/// The first line that starts at or after `position`, for a jump of the
/// scroll bar: `position` itself when a line starts there, else the line
/// after the one that holds it. `None` when `window` does not decide it.
///
/// `window` are bytes of the file from `window_offset`: at least
/// [`LINE_BYTES`] before `position` (or all back to `data_start`) and the
/// bytes after it that [`next_line_start`] asks for. `last` says that
/// `window` reaches the end of the file.
pub(crate) fn line_at_or_after(
    encoding: Encoding,
    data_start: u64,
    window: &[u8],
    window_offset: u64,
    position: u64,
    last: bool,
) -> Option<u64> {
    let position = position.max(data_start);
    let reach = position.saturating_sub(LINE_BYTES as u64).max(data_start);
    if window_offset > reach {
        return None;
    }
    let split = usize::try_from(position - window_offset).ok()?;
    let before = window.get(..split)?;
    let start = line_start(encoding, data_start, before, position);
    if start != position {
        let from = usize::try_from(start - window_offset).ok()?;
        return next_line_start(encoding, window.get(from..)?, start, last);
    }
    // A position between a CR and its LF is inside one terminator, so the
    // line starts after the LF.
    let width = encoding.width();
    let after_cr = split >= width
        && position - data_start >= width as u64
        && encoding.kind(&before[split - width..]) == Unit::Cr;
    if !after_cr {
        return Some(position);
    }
    match window.get(split..split + width) {
        Some(unit) if encoding.kind(unit) == Unit::Lf => Some(position + width as u64),
        Some(_) => Some(position),
        None if last => Some(position),
        None => None,
    }
}

/// Where the status bar's position part stands in Text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Top,
    /// 1 to 99.
    Percent(u8),
    End,
}

/// The place of the first shown line, which starts at `first_line`, in a
/// file of `file_len` bytes. `end_shown` says that the last line shows.
/// The top wins when the whole file shows.
pub(crate) fn text_place(
    data_start: u64,
    file_len: u64,
    first_line: u64,
    end_shown: bool,
) -> Place {
    if first_line <= data_start {
        Place::Top
    } else if end_shown {
        Place::End
    } else {
        let percent = u128::from(first_line) * 100 / u128::from(file_len.max(1));
        Place::Percent(percent.clamp(1, 99) as u8)
    }
}

/// How many hex digits the offset of a row has: 8, or 16 for a file of 4 GB
/// or more.
fn offset_digits(file_len: u64) -> usize {
    if file_len >= 1 << 32 { 16 } else { 8 }
}

/// An offset as a hex row and the status bar show it.
pub(crate) fn offset_text(file_len: u64, offset: u64) -> String {
    format!("{offset:0width$X}", width = offset_digits(file_len))
}

/// The offset of the hex row that holds `offset`.
pub(crate) fn hex_row_start(offset: u64) -> u64 {
    offset & !(HEX_ROW_BYTES as u64 - 1)
}

/// The text of the hex row that starts at `offset`: the offset, the bytes in
/// hex with a gap after the eighth, then the bytes as characters, with `.`
/// for a byte outside the printable ASCII range. A row of fewer than
/// [`HEX_ROW_BYTES`] bytes, such as the last, is padded so that its
/// characters line up with the other rows. `bytes` beyond the row are
/// ignored.
pub(crate) fn hex_row(file_len: u64, offset: u64, bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let bytes = &bytes[..bytes.len().min(HEX_ROW_BYTES)];
    let mut row = offset_text(file_len, offset);
    row.push_str("  ");
    for i in 0..HEX_ROW_BYTES {
        if i > 0 {
            row.push(' ');
        }
        if i == HEX_ROW_BYTES / 2 {
            row.push(' ');
        }
        match bytes.get(i) {
            Some(&b) => {
                row.push(DIGITS[usize::from(b >> 4)] as char);
                row.push(DIGITS[usize::from(b & 15)] as char);
            }
            None => row.push_str("  "),
        }
    }
    row.push_str("  ");
    row.extend(bytes.iter().map(|&b| {
        if (0x20..=0x7E).contains(&b) {
            b as char
        } else {
            '.'
        }
    }));
    row
}

/// The text that Copy puts on the clipboard for `bytes`, a whole file shown
/// as `mode`: in Text the decoded text after the byte order mark, with the
/// file's own line ends and tabs; in Hex each row as the Viewer shows it,
/// with CR LF between the rows.
pub(crate) fn copy_text(
    mode: Mode,
    detected: Detected,
    bytes: &[u8],
    ansi: &dyn Fn(&[u8]) -> String,
) -> String {
    match mode {
        Mode::Text => {
            let start = usize::try_from(detected.data_start)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            decode_piece(detected.encoding, &bytes[start..], ansi)
        }
        Mode::Hex => {
            let len = bytes.len() as u64;
            let mut text = String::new();
            for (row, offset) in bytes
                .chunks(HEX_ROW_BYTES)
                .zip((0u64..).step_by(HEX_ROW_BYTES))
            {
                if offset > 0 {
                    text.push_str("\r\n");
                }
                text.push_str(&hex_row(len, offset, row));
            }
            text
        }
    }
}

/// The largest position of a scroll bar for a file of `file_len` bytes: a
/// byte each, until the 32-bit range of a Win32 scroll bar ends.
pub(crate) fn scroll_max(file_len: u64) -> i32 {
    i32::try_from(file_len.min(SCROLL_MAX)).expect("the minimum is at most i32::MAX")
}

/// The scroll bar position of the byte at `offset`. A file larger than the
/// scroll bar's range maps its whole length onto the range, so a position
/// in the middle is the middle of the file.
pub(crate) fn scroll_position(file_len: u64, offset: u64) -> i32 {
    let offset = offset.min(file_len);
    if file_len <= SCROLL_MAX {
        return i32::try_from(offset).expect("offset is at most file_len, which fits");
    }
    let position = u128::from(offset) * u128::from(SCROLL_MAX) / u128::from(file_len);
    i32::try_from(position).expect("offset is at most file_len, so position is at most the range")
}

/// The first byte whose [`scroll_position`] is `position`. A `position`
/// outside the range counts as the nearest end.
pub(crate) fn scroll_offset(file_len: u64, position: i32) -> u64 {
    let position = u64::try_from(position).unwrap_or(0).min(SCROLL_MAX);
    if file_len <= SCROLL_MAX {
        return position.min(file_len);
    }
    // Rounded up, so that `scroll_position` of the result is `position`.
    let offset = (u128::from(position) * u128::from(file_len)).div_ceil(u128::from(SCROLL_MAX));
    u64::try_from(offset).map_or(file_len, |offset| offset.min(file_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| char::from(b)).collect()
    }

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn a_zero_byte_chooses_hex_unless_a_utf16_mark_explains_it() {
        assert_eq!(choose_mode(b"plain text\r\n"), Mode::Text);
        assert_eq!(choose_mode(b"MZ\x90\x00\x03"), Mode::Hex);
        assert_eq!(choose_mode(&[0xFF, 0xFE, b'a', 0]), Mode::Text);
        assert_eq!(choose_mode(&[0xFE, 0xFF, 0, b'a']), Mode::Text);
        // A UTF-8 mark explains no zero byte.
        assert_eq!(choose_mode(&[0xEF, 0xBB, 0xBF, 0]), Mode::Hex);
        // Only the first 64 KB decide.
        let mut late = vec![b'a'; SNIFF_BYTES];
        late.push(0);
        assert_eq!(choose_mode(&late), Mode::Text);
    }

    #[test]
    fn the_encoding_comes_from_the_mark_else_from_the_bytes() {
        let detect = |bytes: &[u8]| {
            let d = detect_encoding(bytes);
            (d.encoding, d.data_start)
        };
        assert_eq!(detect(&[0xEF, 0xBB, 0xBF, b'a']), (Encoding::Utf8, 3));
        assert_eq!(detect(&[0xFF, 0xFE, b'a', 0]), (Encoding::Utf16Le, 2));
        assert_eq!(detect(&[0xFE, 0xFF, 0, b'a']), (Encoding::Utf16Be, 2));
        assert_eq!(detect("h\u{e9}llo".as_bytes()), (Encoding::Utf8, 0));
        assert_eq!(detect(b"h\xE9llo"), (Encoding::Ansi, 0));
        assert_eq!(detect(b""), (Encoding::Utf8, 0));
    }

    #[test]
    fn a_character_cut_at_64_kb_is_valid_but_at_the_end_of_a_file_is_not() {
        let mut cut = vec![b'a'; SNIFF_BYTES - 1];
        cut.push(0xC3);
        assert_eq!(detect_encoding(&cut).encoding, Encoding::Utf8);
        assert_eq!(detect_encoding(b"ab\xC3").encoding, Encoding::Ansi);
        // Bytes after the first 64 KB are not read.
        let mut late = vec![b'a'; SNIFF_BYTES];
        late.push(0xFF);
        assert_eq!(detect_encoding(&late).encoding, Encoding::Utf8);
    }

    #[test]
    fn unreadable_bytes_decode_as_replacement_characters() {
        let decode = |encoding, bytes: &[u8]| decode_piece(encoding, bytes, &latin1);
        assert_eq!(decode(Encoding::Utf8, b"a\xFFb"), "a\u{FFFD}b");
        // A character that the piece's end cuts off.
        assert_eq!(decode(Encoding::Utf8, b"ab\xC3"), "ab\u{FFFD}");
        assert_eq!(
            decode(Encoding::Utf16Be, &[0, 0x41, 0xD8, 0x00, 0, 0x42]),
            "A\u{FFFD}B"
        );
        assert_eq!(
            decode(Encoding::Utf16Be, &[0, 0x41, 0xD8, 0x3D]),
            "A\u{FFFD}"
        );
        assert_eq!(decode(Encoding::Utf16Le, &[0x41, 0, 0x42]), "A\u{FFFD}");
        assert_eq!(
            decode(Encoding::Utf16Le, &[0x3D, 0xD8, 0x00, 0xDE]),
            "\u{1F600}"
        );
        assert_eq!(decode(Encoding::Ansi, b"caf\xE9"), "caf\u{e9}");
    }

    #[test]
    fn a_tab_moves_to_the_next_multiple_of_8_columns() {
        assert_eq!(expand_tabs("a\tb"), "a       b");
        assert_eq!(expand_tabs("12345678\tx"), "12345678        x");
        assert_eq!(without_terminator("end\r\n"), "end");
        assert_eq!(without_terminator("end\r"), "end");
        assert_eq!(without_terminator("end\n\n"), "end\n");
        assert_eq!(without_terminator("end"), "end");
    }

    // "one\n" 0..4, "two\r\n" 4..9, "three\r" 9..15, "four" 15..19.
    const LINES: &[u8] = b"one\ntwo\r\nthree\rfour";

    #[test]
    fn a_line_starts_after_the_terminator_before_any_byte() {
        let at =
            |position: usize| line_start(Encoding::Utf8, 0, &LINES[..position], position as u64);
        assert_eq!(at(0), 0);
        assert_eq!(at(2), 0);
        assert_eq!(at(6), 4);
        assert_eq!(at(9), 9);
        assert_eq!(at(12), 9);
        // After a lone CR a line starts.
        assert_eq!(at(15), 15);
        assert_eq!(at(18), 15);
    }

    #[test]
    fn the_previous_line_skips_one_terminator_whole() {
        let previous = |data: &[u8], position: usize| {
            previous_line_start(Encoding::Utf8, 0, &data[..position], position as u64)
        };
        assert_eq!(previous(LINES, 9), 4);
        assert_eq!(previous(LINES, 15), 9);
        assert_eq!(previous(LINES, 4), 0);
        // "a\n\nb": the empty line starts at 2.
        assert_eq!(previous(b"a\n\nb", 3), 2);
        assert_eq!(previous(b"a\n\nb", 2), 0);
    }

    #[test]
    fn the_next_line_waits_for_a_byte_that_may_complete_cr_lf() {
        let next =
            |from: usize, last| next_line_start(Encoding::Utf8, &LINES[from..], from as u64, last);
        assert_eq!(next(0, false), Some(4));
        assert_eq!(next(4, false), Some(9));
        assert_eq!(next(9, false), Some(15));
        assert_eq!(next(15, false), None);
        assert_eq!(next(15, true), Some(19));
        assert_eq!(next_line_start(Encoding::Utf8, b"ab\r", 0, false), None);
        assert_eq!(next_line_start(Encoding::Utf8, b"ab\r", 0, true), Some(3));
    }

    #[test]
    fn utf16_lines_line_up_with_the_start_of_the_text() {
        let mut file = vec![0xFF, 0xFE];
        file.extend(utf16le("ab\r\ncd"));
        // "c" is at 10, after the CR LF at 6..10.
        assert_eq!(line_start(Encoding::Utf16Le, 2, &file[..12], 12), 10);
        // A position inside the unit of "c" gives the same line.
        assert_eq!(line_start(Encoding::Utf16Le, 2, &file[..11], 11), 10);
        assert_eq!(
            previous_line_start(Encoding::Utf16Le, 2, &file[..10], 10),
            2
        );
        assert_eq!(
            next_line_start(Encoding::Utf16Le, &file[2..], 2, false),
            Some(10)
        );
        // 0x0A in the high byte of a unit is not an LF.
        let high = utf16le("\u{0A00}x");
        assert_eq!(next_line_start(Encoding::Utf16Le, &high, 0, true), Some(4));
    }

    #[test]
    fn a_long_line_is_cut_into_pieces_that_both_directions_agree_on() {
        let mut file = vec![b'a'; 25_000];
        file.extend(b"\nb");
        let next = |from: usize| next_line_start(Encoding::Utf8, &file[from..], from as u64, true);
        assert_eq!(next(0), Some(10_000));
        assert_eq!(next(10_000), Some(20_000));
        assert_eq!(next(20_000), Some(25_001));
        let previous = |at: usize| previous_line_start(Encoding::Utf8, 0, &file[..at], at as u64);
        assert_eq!(previous(20_000), 10_000);
        assert_eq!(previous(10_000), 0);
        // The last piece is the one that Down reached, not 10,000 back.
        assert_eq!(previous(25_001), 20_000);
        // From inside a piece, the piece's start.
        assert_eq!(
            line_start(Encoding::Utf8, 0, &file[..15_000], 15_000),
            10_000
        );
    }

    #[test]
    fn a_line_start_out_of_reach_is_a_piece_back_from_the_position() {
        let file = vec![b'a'; 100_000];
        assert_eq!(
            line_start(Encoding::Utf8, 0, &file[..70_000], 70_000),
            60_000
        );
        // The answer does not depend on how many bytes the caller passes
        // beyond LINE_BYTES.
        let enough = &file[70_000 - LINE_BYTES..70_000];
        assert_eq!(line_start(Encoding::Utf8, 0, enough, 70_000), 60_000);
    }

    #[test]
    fn bytes_that_are_no_characters_still_end_a_piece() {
        let continuation = vec![0x85; LINE_BYTES];
        assert_eq!(
            next_line_start(Encoding::Utf8, &continuation, 0, false),
            Some(40_000)
        );
        let low_surrogates: Vec<u8> = [0x00, 0xDC].repeat(LINE_BYTES / 2);
        assert_eq!(
            next_line_start(Encoding::Utf16Le, &low_surrogates, 0, false),
            Some(40_000)
        );
        let before = &continuation[..PIECE_BYTES];
        assert_eq!(line_start(Encoding::Utf8, 0, before, 50_000), 10_000);
    }

    #[test]
    fn a_terminator_after_exactly_one_piece_belongs_to_it() {
        let mut file = vec![b'a'; PIECE_CHARS];
        file.extend(b"\r\nb");
        assert_eq!(
            next_line_start(Encoding::Utf8, &file, 0, true),
            Some(10_002)
        );
    }

    #[test]
    fn pieces_count_characters_not_bytes() {
        let two_bytes = "\u{e9}".repeat(PIECE_CHARS + 1);
        assert_eq!(
            next_line_start(Encoding::Utf8, two_bytes.as_bytes(), 0, true),
            Some(20_000)
        );
        // 4 bytes each, and the reading limit still decides.
        let four_bytes = "\u{1F600}".repeat(PIECE_CHARS + 2);
        assert_eq!(
            next_line_start(
                Encoding::Utf8,
                &four_bytes.as_bytes()[..LINE_BYTES],
                0,
                false
            ),
            Some(40_000)
        );
        let pairs = utf16le(&"\u{1F600}".repeat(PIECE_CHARS + 1));
        assert_eq!(
            next_line_start(Encoding::Utf16Le, &pairs, 0, true),
            Some(40_000)
        );
    }

    #[test]
    fn a_jump_shows_the_first_line_that_starts_at_or_after_the_byte() {
        let file = b"one\ntwo\nthree";
        let jump =
            |position: u64, last| line_at_or_after(Encoding::Utf8, 0, file, 0, position, last);
        assert_eq!(jump(0, true), Some(0));
        assert_eq!(jump(4, true), Some(4));
        assert_eq!(jump(5, true), Some(8));
        // The last line's end is only known at the end of the file.
        assert_eq!(jump(10, true), Some(13));
        assert_eq!(jump(10, false), None);
        // A jump to the top lands after the byte order mark.
        let marked = b"\xEF\xBB\xBFx\ny";
        assert_eq!(
            line_at_or_after(Encoding::Utf8, 3, marked, 0, 0, true),
            Some(3)
        );
    }

    #[test]
    fn a_jump_between_cr_and_lf_lands_after_the_pair() {
        // The LF is at 8; Up from 9 reaches 4, so 8 must not start a line.
        let file = b"one\ntwo\r\nthree";
        let jump = |position, last| line_at_or_after(Encoding::Utf8, 0, file, 0, position, last);
        assert_eq!(jump(8, true), Some(9));
        assert_eq!(previous_line_start(Encoding::Utf8, 0, &file[..9], 9), 4);
        // A CR alone is a whole terminator.
        assert_eq!(
            line_at_or_after(Encoding::Utf8, 0, b"a\rb", 0, 2, true),
            Some(2)
        );
        // Whether an LF follows is not known at the end of the window.
        assert_eq!(
            line_at_or_after(Encoding::Utf8, 0, b"a\r", 0, 2, false),
            None
        );
        assert_eq!(
            line_at_or_after(Encoding::Utf8, 0, b"a\r", 0, 2, true),
            Some(2)
        );
        let utf16 = utf16le("ab\r\ncd");
        assert_eq!(
            line_at_or_after(Encoding::Utf16Le, 0, &utf16, 0, 6, true),
            Some(8)
        );
    }

    #[test]
    fn a_jump_needs_a_window_that_reaches_back_far_enough() {
        let file = vec![b'a'; 100_000];
        let jump = |window_offset: usize| {
            line_at_or_after(
                Encoding::Utf8,
                0,
                &file[window_offset..],
                window_offset as u64,
                70_000,
                true,
            )
        };
        assert_eq!(jump(69_900), None);
        assert_eq!(jump(70_000 - LINE_BYTES), Some(70_000));
    }

    #[test]
    fn a_hex_row_shows_offset_bytes_and_characters() {
        let row = hex_row(
            100,
            0,
            b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xFF\xFF\x00\x00",
        );
        assert_eq!(
            row,
            "00000000  4D 5A 90 00 03 00 00 00  04 00 00 00 FF FF 00 00  MZ.............."
        );
        // The last row keeps the character column where it is.
        let short = hex_row(100, 0x10, b"ABC");
        assert_eq!(short, format!("00000010  {:<48}  ABC", "41 42 43"));
        // A file of 4 GB or more has 16 digits.
        let wide = hex_row(1 << 32, 0x1_0000_0010, b"A");
        assert!(wide.starts_with("0000000100000010  41 "));
        assert_eq!(offset_text(u64::from(u32::MAX), 0xFF), "000000FF");
        assert_eq!(hex_row_start(0x1F), 0x10);
    }

    #[test]
    fn a_file_within_the_scroll_range_maps_byte_for_byte() {
        assert_eq!(scroll_max(1_000), 1_000);
        assert_eq!(scroll_position(1_000, 400), 400);
        assert_eq!(scroll_offset(1_000, 400), 400);
        // The range's last value is still a byte each.
        assert_eq!(scroll_position(SCROLL_MAX, SCROLL_MAX), i32::MAX);
        assert_eq!(scroll_offset(SCROLL_MAX, i32::MAX), SCROLL_MAX);
        assert_eq!(scroll_offset(1_000, -5), 0);
        assert_eq!(scroll_offset(1_000, 2_000), 1_000);
    }

    #[test]
    fn a_file_over_2_gb_maps_its_whole_length_onto_the_scroll_range() {
        let len: u64 = 50_000_000_000;
        assert_eq!(scroll_max(len), i32::MAX);
        assert_eq!(scroll_position(len, 0), 0);
        assert_eq!(scroll_position(len, len), i32::MAX);
        // Half of 2,147,483,647 is 1,073,741,823.5.
        assert_eq!(scroll_position(len, len / 2), 1_073_741_823);
        assert_eq!(scroll_offset(len, 0), 0);
        assert_eq!(scroll_offset(len, i32::MAX), len);
        // Dragging to any position and reading the byte there agrees.
        for position in [1, 12_345, 1_073_741_823, i32::MAX - 1] {
            let offset = scroll_offset(len, position);
            assert_eq!(scroll_position(len, offset), position);
        }
    }

    #[test]
    fn the_status_bar_names_top_a_percentage_or_end() {
        assert_eq!(text_place(0, 1_000, 0, false), Place::Top);
        assert_eq!(text_place(3, 1_000, 3, true), Place::Top);
        assert_eq!(text_place(0, 1_000, 370, false), Place::Percent(37));
        assert_eq!(text_place(0, 1_000, 5, false), Place::Percent(1));
        assert_eq!(text_place(0, 1_000, 999, false), Place::Percent(99));
        assert_eq!(text_place(0, 1_000, 900, true), Place::End);
    }

    mod untrusted {
        use super::*;
        use proptest::prelude::*;

        /// A piece always ends within [`LINE_BYTES`], and no function panics.
        fn check(
            bytes: &[u8],
            position: u64,
            back: usize,
            data_start: u64,
            last: bool,
        ) -> Result<(), TestCaseError> {
            for encoding in [
                Encoding::Utf8,
                Encoding::Utf16Le,
                Encoding::Utf16Be,
                Encoding::Ansi,
            ] {
                let next = next_line_start(encoding, bytes, position, false);
                prop_assert!(
                    next.is_some_and(|n| n > position && n - position <= LINE_BYTES as u64)
                );
                let _ = line_start(encoding, data_start, bytes, position);
                let _ = previous_line_start(encoding, data_start, bytes, position);
                let _ = line_at_or_after(
                    encoding,
                    data_start,
                    bytes,
                    position.saturating_sub(back as u64),
                    position,
                    last,
                );
                let _ = decode_piece(encoding, bytes, &latin1);
            }
            Ok(())
        }

        proptest! {
            /// Any bytes from the disk. Uniform bytes hold a terminator
            /// every few hundred bytes, so this reaches the short lines.
            #[test]
            fn any_bytes_panic_nowhere(
                bytes in proptest::collection::vec(any::<u8>(), LINE_BYTES),
                position in 0u64..(1 << 40),
                back in 0usize..=LINE_BYTES,
                data_start in 0u64..4,
                last in any::<bool>(),
            ) {
                check(&bytes, position, back, data_start, last)?;
            }

            /// Bytes with no terminator, in the units that matter to a
            /// piece: characters, continuation bytes, surrogates and zeros.
            /// This reaches the limit of 10,000 characters and the byte
            /// limit.
            #[test]
            fn bytes_without_terminators_panic_nowhere(
                bytes in proptest::collection::vec(
                    prop::sample::select(vec![b'a', 0x80, 0xC3, 0x00, 0xD8, 0xDC]),
                    LINE_BYTES,
                ),
                position in 0u64..(1 << 40),
                back in 0usize..=LINE_BYTES,
                data_start in 0u64..4,
                last in any::<bool>(),
            ) {
                check(&bytes, position, back, data_start, last)?;
            }
        }
    }
}
