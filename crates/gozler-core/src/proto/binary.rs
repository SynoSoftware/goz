//! Compact binary encoding for `Results` pages.
//!
//! JSON stays fine for the tiny control messages (Hello / Status / Error /
//! every request), but a large result set ships millions of Windows paths and
//! JSON charges for each one three times: escaping every `\` on encode,
//! re-scanning every byte on parse, and a `String` allocation per item. A page
//! is instead encoded as length-prefixed raw path bytes plus a flag byte, so
//! the daemon encodes with `memcpy`s straight from its stored path bytes and the
//! client reconstructs (or streams) paths without any JSON work.
//!
//! One frame payload is `[tag: u8][body]`. The tag lets the client dispatch a
//! frame without guessing: [`RESP_TAG_JSON`] bodies are JSON `Response`s (all
//! the control messages), [`RESP_TAG_RESULTS`] bodies are one results page:
//!
//! ```text
//! total:      u64 LE    full match count (es totitems), repeated per page
//! generation: u64 LE
//! page_flags: u8        bit0 more, bit1 volumes_incomplete, bit2 metadata_pending
//! count:      u32 LE    items in this page
//! count items, each:
//!   path_len: u32 LE    raw path byte length (WTF-8, mount prefix included)
//!   path:     path_len bytes
//!   iflags:   u8        bit0 is_dir, bit1 has_size, bit2 has_mtime,
//!                       bit3 has_attributes
//!   size:     u64 LE    iff has_size
//!   mtime:    i64 LE    iff has_mtime
//!   attributes: u32 LE  iff has_attributes: `FILE_ATTRIBUTE_*` bits
//! ```
//!
//! Items are positional, so a client that does not know a field misreads
//! every item after the first that holds it. The daemon therefore sets
//! has_attributes only when the request asks for attributes, which no client
//! before protocol 6 does.
//!
//! Paths travel as raw WTF-8, exactly the bytes the daemon stored. The decoder
//! lossy-decodes them to `ResultItem::path` and, when they were not valid UTF-8,
//! reconstructs the exact UTF-16 into `path_u16` from the same bytes, so a page
//! round-trips to the identical [`ResultItem`]s the JSON path produced.

use super::frame::{begin_frame, end_frame};
use super::types::{MAX_SERVER_FRAME, PAGE_ROWS, QueryResults, ResultItem};
use crate::attributes::Attributes;
use crate::wtf8;

/// Response frame tag: the body is a JSON [`super::Response`].
pub const RESP_TAG_JSON: u8 = 0;
/// Response frame tag: the body is a binary results page (this module).
pub const RESP_TAG_RESULTS: u8 = 1;

const FLAG_MORE: u8 = 1;
const FLAG_VOLUMES_INCOMPLETE: u8 = 1 << 1;
const FLAG_METADATA_PENDING: u8 = 1 << 2;

const IFLAG_IS_DIR: u8 = 1;
const IFLAG_HAS_SIZE: u8 = 1 << 1;
const IFLAG_HAS_MTIME: u8 = 1 << 2;
const IFLAG_HAS_ATTRIBUTES: u8 = 1 << 3;

/// A truncated or malformed binary results page.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("malformed binary results page: {0}")]
pub struct BinError(&'static str);

/// One results page, built in place as a complete frame: the daemon
/// [`push`](Self::push)es matched hits straight from their path bytes until
/// the page refuses one, then [`finish`](Self::finish)es it and writes it to
/// the pipe.
pub struct ResultsPage {
    frame: Vec<u8>,
    /// Where the payload starts in `frame`, after the length prefix.
    payload_at: usize,
    /// Where the header's flags byte and item count sit in `frame`;
    /// [`finish`](Self::finish) fills in both.
    flags_at: usize,
    count_at: usize,
    count: u32,
}

impl ResultsPage {
    /// Starts a page whose header carries these fields. `rows_left`, the rows
    /// still to send, sizes the buffer.
    pub fn new(
        total: u64,
        generation: u64,
        volumes_incomplete: bool,
        metadata_pending: bool,
        rows_left: usize,
    ) -> Self {
        // ~96 bytes per row covers a typical path and its fields without a
        // regrow mid-page.
        let rows = rows_left.min(PAGE_ROWS as usize);
        let mut frame = Vec::with_capacity(rows * 96 + 64);
        // The buffer is empty, so the frame starts at 0, where `finish` ends it.
        begin_frame(&mut frame);
        let payload_at = frame.len();
        frame.push(RESP_TAG_RESULTS);
        frame.extend_from_slice(&total.to_le_bytes());
        frame.extend_from_slice(&generation.to_le_bytes());
        let mut flags = 0u8;
        if volumes_incomplete {
            flags |= FLAG_VOLUMES_INCOMPLETE;
        }
        if metadata_pending {
            flags |= FLAG_METADATA_PENDING;
        }
        let flags_at = frame.len();
        frame.push(flags);
        let count_at = frame.len();
        frame.extend_from_slice(&0u32.to_le_bytes());
        Self {
            frame,
            payload_at,
            flags_at,
            count_at,
            count: 0,
        }
    }

    /// Appends one item, whose `raw_path` is its WTF-8 path bytes with the
    /// mount prefix, or returns `false` and leaves the page unchanged when the
    /// page is full: it holds [`PAGE_ROWS`] items, or the item would take its
    /// payload past [`MAX_SERVER_FRAME`], the cap a client decodes with. An
    /// empty page takes any item, so every page holds at least one; NTFS
    /// limits a full path to far less than the cap.
    pub fn push(
        &mut self,
        raw_path: &[u8],
        is_dir: bool,
        size: Option<u64>,
        mtime: Option<i64>,
        attributes: Option<Attributes>,
    ) -> bool {
        if self.count == PAGE_ROWS {
            return false;
        }
        let before = self.frame.len();
        self.frame
            .extend_from_slice(&(raw_path.len() as u32).to_le_bytes());
        self.frame.extend_from_slice(raw_path);
        let mut iflags = 0u8;
        if is_dir {
            iflags |= IFLAG_IS_DIR;
        }
        if size.is_some() {
            iflags |= IFLAG_HAS_SIZE;
        }
        if mtime.is_some() {
            iflags |= IFLAG_HAS_MTIME;
        }
        if attributes.is_some() {
            iflags |= IFLAG_HAS_ATTRIBUTES;
        }
        self.frame.push(iflags);
        if let Some(s) = size {
            self.frame.extend_from_slice(&s.to_le_bytes());
        }
        if let Some(m) = mtime {
            self.frame.extend_from_slice(&m.to_le_bytes());
        }
        if let Some(a) = attributes {
            self.frame.extend_from_slice(&a.bits().to_le_bytes());
        }
        if self.count > 0 && self.frame.len() - self.payload_at > MAX_SERVER_FRAME as usize {
            self.frame.truncate(before);
            return false;
        }
        self.count += 1;
        true
    }

    /// Completes the frame. `more` tells the client that another page of the
    /// same query follows.
    pub fn finish(mut self, more: bool) -> Vec<u8> {
        if more {
            self.frame[self.flags_at] |= FLAG_MORE;
        }
        self.frame[self.count_at..self.count_at + size_of::<u32>()]
            .copy_from_slice(&self.count.to_le_bytes());
        end_frame(&mut self.frame, 0);
        self.frame
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, BinError> {
        let b = *self.buf.get(self.pos).ok_or(BinError("truncated"))?;
        self.pos += 1;
        Ok(b)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], BinError> {
        let end = self.pos.checked_add(N).ok_or(BinError("overflow"))?;
        let slice = self.buf.get(self.pos..end).ok_or(BinError("truncated"))?;
        self.pos = end;
        Ok(slice.try_into().expect("slice len N"))
    }
    fn u32(&mut self) -> Result<u32, BinError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64, BinError> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn i64(&mut self) -> Result<i64, BinError> {
        Ok(i64::from_le_bytes(self.arr()?))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], BinError> {
        let end = self.pos.checked_add(n).ok_or(BinError("overflow"))?;
        let slice = self.buf.get(self.pos..end).ok_or(BinError("truncated"))?;
        self.pos = end;
        Ok(slice)
    }
}

/// Decodes a results-page frame payload (the bytes AFTER the tag byte) into a
/// [`QueryResults`]. Each path is lossy-decoded to `path`, with `path_u16`
/// reconstructed from the same bytes when they were not valid UTF-8, matching
/// the paths the daemon encoded via [`ResultsPage`]. Bounds-checked against truncation.
pub fn decode_results_payload(body: &[u8]) -> Result<QueryResults, BinError> {
    let mut r = Reader { buf: body, pos: 0 };
    let total = r.u64()?;
    let generation = r.u64()?;
    let flags = r.u8()?;
    let count = r.u32()? as usize;
    let mut items = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let plen = r.u32()? as usize;
        let path = r.bytes(plen)?;
        let iflags = r.u8()?;
        let size = (iflags & IFLAG_HAS_SIZE != 0)
            .then(|| r.u64())
            .transpose()?;
        let mtime = (iflags & IFLAG_HAS_MTIME != 0)
            .then(|| r.i64())
            .transpose()?;
        let attributes = (iflags & IFLAG_HAS_ATTRIBUTES != 0)
            .then(|| {
                Attributes::try_from(r.u32()?).map_err(|_| BinError("unindexed attribute bits"))
            })
            .transpose()?;
        let lossy = core::str::from_utf8(path).is_err();
        items.push(ResultItem {
            path: wtf8::to_string_lossy(path),
            path_u16: lossy.then(|| wtf8::to_utf16(path)),
            size,
            mtime_ft: mtime,
            is_dir: iflags & IFLAG_IS_DIR != 0,
            attributes,
        });
    }
    Ok(QueryResults {
        total,
        items,
        more: flags & FLAG_MORE != 0,
        volumes_incomplete: flags & FLAG_VOLUMES_INCOMPLETE != 0,
        metadata_pending: flags & FLAG_METADATA_PENDING != 0,
        generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{FrameDecoder, Response, decode_response_frame};
    use proptest::prelude::*;

    fn hidden_archive() -> Attributes {
        Attributes::from_letters("ha").expect("H and A are attribute letters")
    }

    /// Decodes a finished page as a client does: through the frame decoder,
    /// which checks the length prefix, and the tagged response decoder.
    fn decode(frame: &[u8]) -> QueryResults {
        let mut decoder = FrameDecoder::new(MAX_SERVER_FRAME);
        decoder.feed(frame);
        let payload = decoder
            .next_frame()
            .expect("under the cap")
            .expect("one whole frame");
        assert_eq!(decoder.next_frame(), Ok(None), "nothing after the frame");
        match decode_response_frame(&payload).expect("decodes") {
            Response::Results(page) => page,
            other => panic!("expected a results page, got {other:?}"),
        }
    }

    /// [`ResultsPage`] fills in the more flag, the item count and the frame
    /// length after the items; a slip there makes every client misread the
    /// page. Every item field and page flag must come back as pushed.
    #[test]
    fn mixed_items_round_trip() {
        let mut page = ResultsPage::new(987_654, 42, true, true, 3);
        assert!(page.push(
            br"C:\Windows\System32\kernel32.dll",
            false,
            Some(1234),
            Some(133_500_000_000_000_000),
            Some(hidden_archive()),
        ));
        assert!(page.push(br"D:\folder", true, None, None, None));
        assert!(page.push(
            "E:\\ünïcode\\文件.txt".as_bytes(),
            false,
            Some(0),
            Some(-1),
            None,
        ));
        assert_eq!(
            decode(&page.finish(true)),
            QueryResults {
                total: 987_654,
                items: vec![
                    ResultItem {
                        path: r"C:\Windows\System32\kernel32.dll".into(),
                        path_u16: None,
                        size: Some(1234),
                        mtime_ft: Some(133_500_000_000_000_000),
                        is_dir: false,
                        attributes: Some(hidden_archive()),
                    },
                    ResultItem {
                        path: r"D:\folder".into(),
                        path_u16: None,
                        size: None,
                        mtime_ft: None,
                        is_dir: true,
                        attributes: None,
                    },
                    ResultItem {
                        path: "E:\\ünïcode\\文件.txt".into(),
                        path_u16: None,
                        size: Some(0),
                        mtime_ft: Some(-1),
                        is_dir: false,
                        attributes: None,
                    },
                ],
                more: true,
                volumes_incomplete: true,
                metadata_pending: true,
                generation: 42,
            }
        );
    }

    #[test]
    fn lossy_path_round_trips_with_exact_units() {
        // A name with an unpaired high surrogate: the encoder ships the raw
        // WTF-8 bytes, the decoder flags it lossy and rebuilds the exact units.
        let units: Vec<u16> = vec![b'C' as u16, b':' as u16, b'\\' as u16, 0xD800, b'x' as u16];
        let mut wtf8_bytes = Vec::new();
        wtf8::from_utf16(&units, &mut wtf8_bytes);
        assert!(core::str::from_utf8(&wtf8_bytes).is_err());
        let mut page = ResultsPage::new(1, 0, false, false, 1);
        assert!(page.push(&wtf8_bytes, false, Some(7), None, None));
        assert_eq!(
            decode(&page.finish(false)).items,
            [ResultItem {
                path: wtf8::to_string_lossy(&wtf8_bytes),
                path_u16: Some(units),
                size: Some(7),
                mtime_ft: None,
                is_dir: false,
                attributes: None,
            }]
        );
    }

    #[test]
    fn truncated_body_errors_not_panics() {
        let mut page = ResultsPage::new(1, 0, false, false, 1);
        assert!(page.push(br"C:\a.txt", false, Some(9), None, None));
        let frame = page.finish(false);
        // The body follows the length prefix and the tag byte. Chop bytes off
        // its end at every length; none should panic.
        let body = &frame[5..];
        for cut in 0..body.len() {
            let _ = decode_results_payload(&body[..cut]);
        }
    }

    proptest! {
        /// A results page arrives from the pipe, so any bytes can arrive, and
        /// a panic would end the client instead of reporting a bad page.
        #[test]
        fn corrupted_pages_error_instead_of_panicking(
            paths in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..16), 0..4),
            patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let mut page = ResultsPage::new(1, 2, false, false, paths.len());
            for path in &paths {
                prop_assert!(page.push(path, false, Some(3), Some(4), Some(hidden_archive())));
            }
            // The body follows the length prefix and the tag byte.
            let mut body = page.finish(true).split_off(5);
            let len = body.len();
            for (at, byte) in &patches {
                body[at.index(len)] = *byte;
            }
            body.truncate(cut.index(len + 1));
            let _ = decode_results_payload(&body);
        }
    }

    /// A client of protocol 5 or older reads an item as its path, flags, size
    /// and date, and the next item right after them. An item without
    /// attributes must keep exactly the bytes that client reads, or it
    /// misreads every item after the first.
    #[test]
    fn an_item_without_attributes_keeps_the_protocol_5_layout() {
        let mut page = ResultsPage::new(1, 0, false, false, 1);
        assert!(page.push(br"C:\a", false, Some(2), Some(3), None));
        let frame = page.finish(false);
        #[rustfmt::skip]
        let payload: &[u8] = &[
            RESP_TAG_RESULTS,
            1, 0, 0, 0, 0, 0, 0, 0, // total
            0, 0, 0, 0, 0, 0, 0, 0, // generation
            0, // page flags
            1, 0, 0, 0, // item count
            4, 0, 0, 0, b'C', b':', b'\\', b'a', // path
            0b110, // iflags: has_size, has_mtime
            2, 0, 0, 0, 0, 0, 0, 0, // size
            3, 0, 0, 0, 0, 0, 0, 0, // mtime
        ];
        assert_eq!(frame[4..], *payload);
    }
}
