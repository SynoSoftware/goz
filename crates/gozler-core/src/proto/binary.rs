//! Compact binary encoding for `Results` pages.
//!
//! JSON stays fine for the tiny control messages (Hello / Status / Error /
//! every request), but a large result set ships millions of names, and JSON
//! would escape, re-scan and allocate each one. A page is instead encoded as
//! length-prefixed raw name bytes and flag bytes.
//!
//! No item carries its full path. A reply sends each folder its items are in
//! once, as its parent and its name, before the first item that needs it,
//! and an item names its folder and carries only its own name. So the bytes
//! of a reply grow with the names, not with the folder depth: on a volume of
//! 2.15 million entries the pipe carries 73 MB instead of 251 MB of full
//! paths, and a tree 200 folders deep costs no more per item than a flat one.
//!
//! One frame payload is `[tag: u8][body]`. The tag lets the client dispatch a
//! frame without guessing: [`RESP_TAG_JSON`] bodies are JSON `Response`s (all
//! the control messages), [`RESP_TAG_RESULTS`] bodies are one results page:
//!
//! ```text
//! total:      u64 LE    match count less the rows left out so far; the last page's holds
//! generation: u64 LE
//! page_flags: u8        bit0 more, bit1 volumes_incomplete, bit2 metadata_pending,
//!                       bit3 has_sums
//! folder_count: u32 LE  folders this page adds to the reply's folders
//! folder_count folders, each:
//!   parent:   u32 LE    the reply's id of its parent folder; u32::MAX for a
//!                       volume's root
//!   name_len: u32 LE
//!   name:     name_len bytes   WTF-8; a root's name is its mount prefix
//!                              without the last separator
//! count:      u32 LE    items in this page
//! count items, each:
//!   folder:   u32 LE    the reply's id of the item's folder
//!   name_len: u32 LE
//!   name:     name_len bytes   WTF-8
//!   iflags:   u8        bit0 is_dir, bit1 has_size, bit2 has_mtime,
//!                       bit3 has_attributes
//!   size:     u64 LE    iff has_size
//!   mtime:    i64 LE    iff has_mtime
//!   attributes: u32 LE  iff has_attributes: `FILE_ATTRIBUTE_*` bits
//! sums, iff has_sums, after the items:
//!   files:      u64 LE
//!   folders:    u64 LE
//!   known:      u8      bit0 file_bytes known, bit1 folder_bytes known
//!   file_bytes: u64 LE  0 when unknown
//!   folder_bytes: u64 LE  0 when unknown
//! ```
//!
//! A folder's id is its place among the folders the reply has sent, from 0,
//! across all its pages, so the id itself is not sent. The decoder refuses a
//! parent or an item folder that the reply has not sent yet, so a malformed
//! page is an error and can never make a loop of folders.
//!
//! Names travel as raw WTF-8, exactly the bytes the daemon stored. The decoder
//! joins them into the full path, lossy-decodes it to `ResultItem::path` and,
//! when it was not valid UTF-8, reconstructs the exact UTF-16 into `path_u16`
//! from the same bytes. A separator always sits between two names, so the
//! join never splits a surrogate pair.

use smallvec::SmallVec;

use super::frame::{begin_frame, end_frame};
use super::types::{MAX_SERVER_FRAME, PAGE_ROWS, QueryResults, ResultItem};
use crate::attributes::Attributes;
use crate::query::{FolderId, MatchSums, ResultNames};
use crate::wtf8;

/// Response frame tag: the body is a JSON [`super::Response`].
pub const RESP_TAG_JSON: u8 = 0;
/// Response frame tag: the body is a binary results page (this module).
pub const RESP_TAG_RESULTS: u8 = 1;

const FLAG_MORE: u8 = 1;
const FLAG_VOLUMES_INCOMPLETE: u8 = 1 << 1;
const FLAG_METADATA_PENDING: u8 = 1 << 2;
const FLAG_SUMS: u8 = 1 << 3;

const SUMS_FILE_BYTES: u8 = 1;
const SUMS_FOLDER_BYTES: u8 = 1 << 1;
/// The sums' bytes after the items.
const SUMS_LEN: usize = 8 + 8 + 1 + 8 + 8;

const IFLAG_IS_DIR: u8 = 1;
const IFLAG_HAS_SIZE: u8 = 1 << 1;
const IFLAG_HAS_MTIME: u8 = 1 << 2;
const IFLAG_HAS_ATTRIBUTES: u8 = 1 << 3;

/// A folder's parent field for a volume's root.
const NO_PARENT: u32 = u32::MAX;

/// A truncated or malformed binary results page.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("malformed binary results page: {0}")]
pub struct BinError(&'static str);

/// The folders of a reply's [`ResultNames`] that its pages have sent, with
/// the id each was sent under, so each folder travels once per reply. The
/// table may grow between pages.
#[derive(Default)]
pub struct SentFolders {
    ids: Vec<Option<u32>>,
    next: u32,
}

/// What a page's header says that is known only once its items are read.
#[derive(Clone, Copy, Debug)]
pub struct PageEnd {
    /// The full match count, less the rows left out so far.
    pub total: u64,
    /// Another page of the same query follows.
    pub more: bool,
    pub volumes_incomplete: bool,
}

/// One item of a results page.
#[derive(Clone, Copy, Debug)]
pub struct PageItem<'a> {
    /// The folder that holds the item, in the reply's [`ResultNames`].
    pub folder: FolderId,
    /// The item's own name (WTF-8).
    pub name: &'a [u8],
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub attributes: Option<Attributes>,
}

/// One results page, built as a complete frame: the daemon
/// [`push`](Self::push)es items until the page refuses one, then
/// [`finish`](Self::finish)es it and writes it to the pipe.
pub struct ResultsPage {
    /// The frame up to and including the folders.
    frame: Vec<u8>,
    /// The items, which follow the folders and their count.
    items: Vec<u8>,
    /// Where the payload starts in `frame`, after the length prefix.
    payload_at: usize,
    /// Where the header's total, flags byte and folder count sit in `frame`;
    /// [`finish`](Self::finish) fills them in.
    total_at: usize,
    flags_at: usize,
    folder_count_at: usize,
    folder_count: u32,
    count: u32,
    /// Written after the items by [`finish`](Self::finish).
    sums: Option<MatchSums>,
}

impl ResultsPage {
    /// Starts a page whose header carries these fields; [`Self::finish`]
    /// writes the rest. `rows_left`, the rows still to send, sizes the buffer.
    pub fn new(generation: u64, metadata_pending: bool, rows_left: usize) -> Self {
        // ~48 bytes per item covers a typical name and its fields without a
        // regrow mid-page.
        let rows = rows_left.min(PAGE_ROWS as usize);
        let mut frame = Vec::with_capacity(4096);
        // The buffer is empty, so the frame starts at 0, where `finish` ends it.
        begin_frame(&mut frame);
        let payload_at = frame.len();
        frame.push(RESP_TAG_RESULTS);
        let total_at = frame.len();
        frame.extend_from_slice(&0u64.to_le_bytes());
        frame.extend_from_slice(&generation.to_le_bytes());
        let mut flags = 0u8;
        if metadata_pending {
            flags |= FLAG_METADATA_PENDING;
        }
        let flags_at = frame.len();
        frame.push(flags);
        let folder_count_at = frame.len();
        frame.extend_from_slice(&0u32.to_le_bytes());
        Self {
            frame,
            items: Vec::with_capacity(rows * 48),
            payload_at,
            total_at,
            flags_at,
            folder_count_at,
            folder_count: 0,
            count: 0,
            sums: None,
        }
    }

    /// Makes the page carry `sums` after its items. Called before the first
    /// [`push`](Self::push), which keeps room for them.
    pub fn with_sums(mut self, sums: Option<MatchSums>) -> Self {
        if sums.is_some() {
            self.frame[self.flags_at] |= FLAG_SUMS;
        }
        self.sums = sums;
        self
    }

    /// Appends `item`, after each folder above it that `sent` says the reply
    /// has not sent yet, or returns `false` and leaves the page and `sent`
    /// unchanged when the page is full: it holds [`PAGE_ROWS`] items, or the
    /// item and its new folders would take the payload past
    /// [`MAX_SERVER_FRAME`], the cap a client decodes with. An empty page
    /// takes any item, so every page holds at least one; the hop limit keeps
    /// an item's folders far below the cap.
    pub fn push(
        &mut self,
        item: PageItem<'_>,
        names: &ResultNames,
        sent: &mut SentFolders,
    ) -> bool {
        if self.count == PAGE_ROWS {
            return false;
        }
        let (frame_before, items_before) = (self.frame.len(), self.items.len());
        let (folders_before, next_before) = (self.folder_count, sent.next);
        if sent.ids.len() < names.folder_count() {
            sent.ids.resize(names.folder_count(), None);
        }

        // The folders this item needs that the reply has not sent, nearest
        // first; they are sent from the root down.
        let mut unsent: SmallVec<[FolderId; 32]> = SmallVec::new();
        let mut cur = Some(item.folder);
        while let Some(folder) = cur
            && sent.ids[folder.index()].is_none()
        {
            unsent.push(folder);
            cur = names.parent(folder);
        }
        for &folder in unsent.iter().rev() {
            let parent = names.parent(folder).map_or(NO_PARENT, |parent| {
                sent.ids[parent.index()].expect("a folder's parent is sent before it")
            });
            push_named(&mut self.frame, parent, names.folder_name(folder));
            sent.ids[folder.index()] = Some(sent.next);
            sent.next += 1;
            self.folder_count += 1;
        }

        let folder = sent.ids[item.folder.index()].expect("the item's folder is sent");
        push_named(&mut self.items, folder, item.name);
        let mut iflags = 0u8;
        if item.is_dir {
            iflags |= IFLAG_IS_DIR;
        }
        if item.size.is_some() {
            iflags |= IFLAG_HAS_SIZE;
        }
        if item.mtime.is_some() {
            iflags |= IFLAG_HAS_MTIME;
        }
        if item.attributes.is_some() {
            iflags |= IFLAG_HAS_ATTRIBUTES;
        }
        self.items.push(iflags);
        if let Some(s) = item.size {
            self.items.extend_from_slice(&s.to_le_bytes());
        }
        if let Some(m) = item.mtime {
            self.items.extend_from_slice(&m.to_le_bytes());
        }
        if let Some(a) = item.attributes {
            self.items.extend_from_slice(&a.bits().to_le_bytes());
        }

        let trailer = if self.sums.is_some() { SUMS_LEN } else { 0 };
        let payload = self.frame.len() - self.payload_at + 4 + self.items.len() + trailer;
        if self.count > 0 && payload > MAX_SERVER_FRAME as usize {
            self.frame.truncate(frame_before);
            self.items.truncate(items_before);
            self.folder_count = folders_before;
            for &folder in &unsent {
                sent.ids[folder.index()] = None;
            }
            sent.next = next_before;
            return false;
        }
        self.count += 1;
        true
    }

    /// Completes the frame with the header fields of `end`.
    pub fn finish(mut self, end: PageEnd) -> Vec<u8> {
        self.frame[self.total_at..self.total_at + size_of::<u64>()]
            .copy_from_slice(&end.total.to_le_bytes());
        if end.more {
            self.frame[self.flags_at] |= FLAG_MORE;
        }
        if end.volumes_incomplete {
            self.frame[self.flags_at] |= FLAG_VOLUMES_INCOMPLETE;
        }
        self.frame[self.folder_count_at..self.folder_count_at + size_of::<u32>()]
            .copy_from_slice(&self.folder_count.to_le_bytes());
        self.frame.extend_from_slice(&self.count.to_le_bytes());
        self.frame.extend_from_slice(&self.items);
        if let Some(sums) = self.sums {
            let mut known = 0u8;
            if sums.file_bytes.is_some() {
                known |= SUMS_FILE_BYTES;
            }
            if sums.folder_bytes.is_some() {
                known |= SUMS_FOLDER_BYTES;
            }
            self.frame.extend_from_slice(&sums.files.to_le_bytes());
            self.frame.extend_from_slice(&sums.folders.to_le_bytes());
            self.frame.push(known);
            self.frame
                .extend_from_slice(&sums.file_bytes.unwrap_or(0).to_le_bytes());
            self.frame
                .extend_from_slice(&sums.folder_bytes.unwrap_or(0).to_le_bytes());
        }
        end_frame(&mut self.frame, 0);
        self.frame
    }
}

/// Appends a folder or an item as the page lays both out: an id, then a
/// length-prefixed name.
fn push_named(out: &mut Vec<u8>, id: u32, name: &[u8]) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name);
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
    fn name(&mut self) -> Result<&'a [u8], BinError> {
        let len = self.u32()? as usize;
        self.bytes(len)
    }
}

/// Decodes a results-page frame payload (the bytes AFTER the tag byte) into a
/// [`QueryResults`], adding the page's folders to `names`, which holds the
/// folders of the reply's earlier pages: one `names` serves every page of a
/// reply, in order, and a new reply starts with an empty one. Bounds-checked
/// against truncation.
pub fn decode_results_payload(
    body: &[u8],
    names: &mut ResultNames,
) -> Result<QueryResults, BinError> {
    let mut r = Reader { buf: body, pos: 0 };
    let total = r.u64()?;
    let generation = r.u64()?;
    let flags = r.u8()?;
    let folder_count = r.u32()?;
    for _ in 0..folder_count {
        let parent = match r.u32()? {
            NO_PARENT => None,
            parent => Some(
                names
                    .folder_at(parent)
                    .ok_or(BinError("a folder's parent was not sent before it"))?,
            ),
        };
        let name = r.name()?;
        // An id must stay below NO_PARENT, which names no folder.
        if names.folder_count() >= NO_PARENT as usize {
            return Err(BinError("too many folders"));
        }
        names.push_folder(parent, name);
    }
    let count = r.u32()? as usize;
    let mut items = Vec::with_capacity(count.min(PAGE_ROWS as usize));
    let mut path = Vec::new();
    for _ in 0..count {
        let folder = names
            .folder_at(r.u32()?)
            .ok_or(BinError("an item's folder was not sent before it"))?;
        let name = r.name()?;
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
        names.full_path_into(folder, name, &mut path);
        let lossy = core::str::from_utf8(&path).is_err();
        items.push(ResultItem {
            path: wtf8::to_string_lossy(&path),
            path_u16: lossy.then(|| wtf8::to_utf16(&path)),
            size,
            mtime_ft: mtime,
            is_dir: iflags & IFLAG_IS_DIR != 0,
            attributes,
        });
    }
    let sums = (flags & FLAG_SUMS != 0)
        .then(|| -> Result<MatchSums, BinError> {
            let files = r.u64()?;
            let folders = r.u64()?;
            let known = r.u8()?;
            let file_bytes = r.u64()?;
            let folder_bytes = r.u64()?;
            Ok(MatchSums {
                files,
                file_bytes: (known & SUMS_FILE_BYTES != 0).then_some(file_bytes),
                folders,
                folder_bytes: (known & SUMS_FOLDER_BYTES != 0).then_some(folder_bytes),
            })
        })
        .transpose()?;
    Ok(QueryResults {
        total,
        items,
        more: flags & FLAG_MORE != 0,
        volumes_incomplete: flags & FLAG_VOLUMES_INCOMPLETE != 0,
        metadata_pending: flags & FLAG_METADATA_PENDING != 0,
        generation,
        sums,
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

    /// `C:\Windows\System32` and `D:\` as the names a daemon sends from.
    struct Tree {
        names: ResultNames,
        c: FolderId,
        system32: FolderId,
        d: FolderId,
    }

    fn tree() -> Tree {
        let mut names = ResultNames::default();
        let c = names.push_folder(None, b"C:");
        let windows = names.push_folder(Some(c), b"Windows");
        let system32 = names.push_folder(Some(windows), b"System32");
        let d = names.push_folder(None, b"D:");
        Tree {
            names,
            c,
            system32,
            d,
        }
    }

    fn item(folder: FolderId, name: &[u8]) -> PageItem<'_> {
        PageItem {
            folder,
            name,
            is_dir: false,
            size: None,
            mtime: None,
            attributes: None,
        }
    }

    fn last_page(total: u64) -> PageEnd {
        PageEnd {
            total,
            more: false,
            volumes_incomplete: false,
        }
    }

    /// Decodes the pages of one reply as a client does: through the frame
    /// decoder, which checks the length prefix, and the tagged response
    /// decoder, with one table of folders for the whole reply.
    fn decode(frames: &[Vec<u8>]) -> Vec<QueryResults> {
        let mut names = ResultNames::default();
        frames
            .iter()
            .map(|frame| {
                let mut decoder = FrameDecoder::new(MAX_SERVER_FRAME);
                decoder.feed(frame);
                let payload = decoder
                    .next_frame()
                    .expect("under the cap")
                    .expect("one whole frame");
                assert_eq!(decoder.next_frame(), Ok(None), "nothing after the frame");
                match decode_response_frame(&payload, &mut names).expect("decodes") {
                    Response::Results(page) => page,
                    other => panic!("expected a results page, got {other:?}"),
                }
            })
            .collect()
    }

    fn paths(page: &QueryResults) -> Vec<&str> {
        page.items.iter().map(|item| item.path.as_str()).collect()
    }

    /// [`ResultsPage`] fills in the more flag, the counts and the frame
    /// length after the items; a slip there makes every client misread the
    /// page. Every item field and page flag must come back as pushed, with
    /// its full path.
    #[test]
    fn mixed_items_round_trip() {
        let t = tree();
        let mut sent = SentFolders::default();
        let mut page = ResultsPage::new(42, true, 3);
        assert!(page.push(
            PageItem {
                size: Some(1234),
                mtime: Some(133_500_000_000_000_000),
                attributes: Some(hidden_archive()),
                ..item(t.system32, b"kernel32.dll")
            },
            &t.names,
            &mut sent,
        ));
        assert!(page.push(
            PageItem {
                is_dir: true,
                ..item(t.d, b"folder")
            },
            &t.names,
            &mut sent,
        ));
        assert!(page.push(
            PageItem {
                size: Some(0),
                mtime: Some(-1),
                ..item(t.c, "ünïcode 文件.txt".as_bytes())
            },
            &t.names,
            &mut sent,
        ));
        assert_eq!(
            decode(&[page.finish(PageEnd {
                total: 987_654,
                more: true,
                volumes_incomplete: true,
            })]),
            [QueryResults {
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
                        path: "C:\\ünïcode 文件.txt".into(),
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
                sums: None,
            }]
        );
    }

    /// A folder travels once per reply: a later page names the folders an
    /// earlier page sent, and its items still decode to their full paths.
    /// Without the shared table a client would read every later page wrong.
    #[test]
    fn a_later_page_names_the_folders_an_earlier_page_sent() {
        let t = tree();
        let mut sent = SentFolders::default();
        let mut first = ResultsPage::new(0, false, 1);
        assert!(first.push(item(t.system32, b"a.dll"), &t.names, &mut sent));
        let mut second = ResultsPage::new(0, false, 1);
        assert!(second.push(item(t.system32, b"b.dll"), &t.names, &mut sent));
        let end = |more| PageEnd {
            total: 2,
            more,
            volumes_incomplete: false,
        };
        let (first, second) = (first.finish(end(true)), second.finish(end(false)));
        assert!(
            second.len() < first.len(),
            "the second page sends no folder"
        );
        let pages = decode(&[first, second]);
        assert_eq!(paths(&pages[0]), [r"C:\Windows\System32\a.dll"]);
        assert_eq!(paths(&pages[1]), [r"C:\Windows\System32\b.dll"]);
    }

    /// The bytes of a page with one root folder and one item, as a client
    /// reads them: a slip in the order of the fields misreads every page.
    #[test]
    fn a_page_lays_out_its_folders_then_its_items() {
        let mut names = ResultNames::default();
        let root = names.push_folder(None, b"C:");
        let mut sent = SentFolders::default();
        let mut page = ResultsPage::new(0, false, 1);
        assert!(page.push(
            PageItem {
                size: Some(2),
                mtime: Some(3),
                ..item(root, b"a")
            },
            &names,
            &mut sent,
        ));
        let frame = page.finish(last_page(1));
        #[rustfmt::skip]
        let payload: &[u8] = &[
            RESP_TAG_RESULTS,
            1, 0, 0, 0, 0, 0, 0, 0, // total
            0, 0, 0, 0, 0, 0, 0, 0, // generation
            0, // page flags
            1, 0, 0, 0, // folder count
            0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0, b'C', b':', // the root
            1, 0, 0, 0, // item count
            0, 0, 0, 0, 1, 0, 0, 0, b'a', // folder 0, name
            0b110, // iflags: has_size, has_mtime
            2, 0, 0, 0, 0, 0, 0, 0, // size
            3, 0, 0, 0, 0, 0, 0, 0, // mtime
        ];
        assert_eq!(frame[4..], *payload);
    }

    /// A page whose item or folder names a folder the reply has not sent is
    /// an error: decoding it would read an unrelated path, or a parent that
    /// points forward could make a loop that never reaches a root.
    #[test]
    fn a_folder_the_reply_has_not_sent_is_an_error() {
        let header = |folder_count: u32| {
            let mut body = vec![0; 17]; // total, generation, flags
            body.extend_from_slice(&folder_count.to_le_bytes());
            body
        };
        // A folder whose parent is itself.
        let mut looped = header(1);
        looped.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, b'x']);
        looped.extend_from_slice(&0u32.to_le_bytes());
        // An item in folder 0 of a reply that sent none.
        let mut orphan = header(0);
        orphan.extend_from_slice(&1u32.to_le_bytes());
        orphan.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, b'x', 0]);
        for body in [looped, orphan] {
            assert!(decode_results_payload(&body, &mut ResultNames::default()).is_err());
        }
    }

    #[test]
    fn lossy_path_round_trips_with_exact_units() {
        // A name with an unpaired high surrogate: the encoder ships the raw
        // WTF-8 bytes, the decoder flags it lossy and rebuilds the exact units.
        let units: Vec<u16> = vec![b'x' as u16, 0xD800];
        let mut name = Vec::new();
        wtf8::from_utf16(&units, &mut name);
        assert!(core::str::from_utf8(&name).is_err());
        let mut names = ResultNames::default();
        let root = names.push_folder(None, b"C:");
        let mut sent = SentFolders::default();
        let mut page = ResultsPage::new(0, false, 1);
        assert!(page.push(item(root, &name), &names, &mut sent));
        let mut full = b"C:\\".to_vec();
        full.extend_from_slice(&name);
        assert_eq!(
            decode(&[page.finish(last_page(1))])[0].items,
            [ResultItem {
                path: wtf8::to_string_lossy(&full),
                path_u16: Some([&[b'C' as u16, b':' as u16, b'\\' as u16][..], &units].concat()),
                size: None,
                mtime_ft: None,
                is_dir: false,
                attributes: None,
            }]
        );
    }

    proptest! {
        /// A results page arrives from the pipe, so any bytes can arrive, and
        /// a panic would end the client instead of reporting a bad page.
        #[test]
        fn corrupted_pages_error_instead_of_panicking(
            names in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..16), 0..4),
            patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let t = tree();
            let mut sent = SentFolders::default();
            let mut page = ResultsPage::new(2, false, names.len());
            for name in &names {
                let full = PageItem {
                    size: Some(3),
                    mtime: Some(4),
                    attributes: Some(hidden_archive()),
                    ..item(t.system32, name)
                };
                prop_assert!(page.push(full, &t.names, &mut sent));
            }
            // The body follows the length prefix and the tag byte.
            let mut body = page
                .finish(PageEnd {
                    total: 1,
                    more: true,
                    volumes_incomplete: false,
                })
                .split_off(5);
            let len = body.len();
            for (at, byte) in &patches {
                body[at.index(len)] = *byte;
            }
            body.truncate(cut.index(len + 1));
            let _ = decode_results_payload(&body, &mut ResultNames::default());
        }
    }

    /// The sums follow the items, and a client reads them back, an unknown
    /// size as `None`.
    #[test]
    fn sums_follow_the_items() {
        let sums = MatchSums {
            files: 3,
            file_bytes: Some(7),
            folders: 1,
            folder_bytes: None,
        };
        let t = tree();
        let page = |sums: Option<MatchSums>| {
            let mut sent = SentFolders::default();
            let mut page = ResultsPage::new(0, false, 1).with_sums(sums);
            assert!(page.push(item(t.c, b"a"), &t.names, &mut sent));
            page.finish(last_page(1))
        };
        let (plain, with) = (decode(&[page(None)]), decode(&[page(Some(sums))]));
        assert_eq!(with[0].sums, Some(sums));
        assert_eq!(with[0].items, plain[0].items);
    }
}
