//! Parsers for one folder listing: the output buffer of
//! `GetFileInformationByHandleEx`, a chain of `FILE_ID_EXTD_DIR_INFO`
//! entries for gozlerd's ReFS walk, or of `FILE_FULL_DIR_INFO` entries for
//! the copy, move and delete engine, which every file system answers.
//!
//! <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info>
//! <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_full_dir_info>
//!
//! `NextEntryOffset` is relative to the entry that holds it, and 0 ends the
//! chain. The call does not report how many bytes it wrote, so the chain is
//! the only end marker.

use super::parse::{
    LayoutFile, LayoutName, LayoutWalkError, follow_offset, read_bytes, read_struct,
};
use crate::types::{Frn, MAX_NAME_UTF16_BYTES};
use crate::wtf8;
use zerocopy::little_endian::{I64, U32, U128};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// `FILE_ID_EXTD_DIR_INFO` fixed part (88 bytes); `FileName` (UTF-16LE,
/// `FileNameLength` bytes, not null-terminated) follows.
#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawDirEntry {
    next_entry_offset: U32,
    // Layout-bearing only; never read individually.
    #[allow(dead_code)]
    file_index: U32,
    #[allow(dead_code)]
    creation_time: I64,
    #[allow(dead_code)]
    last_access_time: I64,
    last_write_time: I64,
    #[allow(dead_code)]
    change_time: I64,
    end_of_file: I64,
    #[allow(dead_code)]
    allocation_size: I64,
    file_attributes: U32,
    file_name_length: U32,
    #[allow(dead_code)]
    ea_size: U32,
    #[allow(dead_code)]
    reparse_point_tag: U32,
    file_id: U128,
}

/// Parses one listing page of `folder` into one [`LayoutFile`] per name, each
/// with that single name in `folder`, its size (`None` for a folder) and its
/// last-write time. The `.` and `..` entries are skipped. An empty buffer
/// yields no entries.
pub fn walk_listing_buffer(folder: Frn, buf: &[u8]) -> Result<Vec<LayoutFile>, LayoutWalkError> {
    let mut files = Vec::new();
    if buf.is_empty() {
        return Ok(files);
    }
    let mut entry_off = 0;
    loop {
        let raw: RawDirEntry = read_struct(buf, entry_off, "FILE_ID_EXTD_DIR_INFO")?;
        let name_len = raw.file_name_length.get();
        // Cap before allocating, as the layout parser does.
        if name_len as usize > MAX_NAME_UTF16_BYTES {
            return Err(LayoutWalkError::NameTooLong {
                what: "FILE_ID_EXTD_DIR_INFO.FileName",
                offset: entry_off,
                len: name_len,
                max: MAX_NAME_UTF16_BYTES as u32,
            });
        }
        if !name_len.is_multiple_of(2) {
            return Err(LayoutWalkError::OddUtf16Length {
                what: "FILE_ID_EXTD_DIR_INFO.FileNameLength",
                offset: entry_off,
                len: name_len,
            });
        }
        let name_bytes = read_bytes(
            buf,
            entry_off + size_of::<RawDirEntry>(),
            name_len as usize,
            "FILE_ID_EXTD_DIR_INFO.FileName",
        )?;
        let name = wtf8::from_utf16le(name_bytes);
        if name != b"." && name != b".." {
            let attributes = raw.file_attributes.get();
            files.push(LayoutFile {
                frn: Frn(raw.file_id.get()),
                attributes,
                // EndOfFile is a byte count; a negative one is impossible
                // from the filesystem, so clamp it as the layout parser does.
                size: (attributes & FILE_ATTRIBUTE_DIRECTORY == 0)
                    .then(|| raw.end_of_file.get().max(0) as u64),
                mtime_ft: Some(raw.last_write_time.get()),
                names: vec![LayoutName {
                    parent_frn: folder,
                    name,
                    dos_only: false,
                }],
            });
        }
        let next = raw.next_entry_offset.get();
        if next == 0 {
            return Ok(files);
        }
        // Each hop clears its own entry and name, so the names parsed never
        // add up to more bytes than the buffer holds.
        entry_off = follow_offset(
            buf,
            entry_off,
            next,
            size_of::<RawDirEntry>() + name_len as usize,
            "FILE_ID_EXTD_DIR_INFO.NextEntryOffset",
        )?;
    }
}

/// `FILE_FULL_DIR_INFO` fixed part (68 bytes); `FileName` (UTF-16LE,
/// `FileNameLength` bytes, not null-terminated) follows.
#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawFullDirEntry {
    next_entry_offset: U32,
    // Layout-bearing only; never read individually.
    #[allow(dead_code)]
    file_index: U32,
    creation_time: I64,
    #[allow(dead_code)]
    last_access_time: I64,
    last_write_time: I64,
    #[allow(dead_code)]
    change_time: I64,
    end_of_file: I64,
    #[allow(dead_code)]
    allocation_size: I64,
    file_attributes: U32,
    file_name_length: U32,
    ea_size: U32,
}

/// One entry of a `FILE_FULL_DIR_INFO` listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderEntry {
    /// The name as raw UTF-16, which may hold unpaired surrogates, because
    /// the engine opens the entry by it.
    pub name: Vec<u16>,
    pub attributes: u32,
    /// The data stream's size in bytes; 0 for a folder.
    pub size: u64,
    /// The creation time as a `FILETIME` value.
    pub created: i64,
    /// The last write time as a `FILETIME` value.
    pub last_write: i64,
    /// The reparse tag of a reparse point. MS-FSCC puts it in `EaSize` when
    /// `FILE_ATTRIBUTE_REPARSE_POINT` is set.
    pub reparse_tag: Option<u32>,
}

/// Parses one `FileFullDirectoryInfo` listing page into its entries, without
/// `.` and `..`. An empty buffer yields no entries.
pub fn walk_full_listing_buffer(buf: &[u8]) -> Result<Vec<FolderEntry>, LayoutWalkError> {
    let mut entries = Vec::new();
    if buf.is_empty() {
        return Ok(entries);
    }
    let mut entry_off = 0;
    loop {
        let raw: RawFullDirEntry = read_struct(buf, entry_off, "FILE_FULL_DIR_INFO")?;
        let name_len = raw.file_name_length.get();
        if name_len as usize > MAX_NAME_UTF16_BYTES {
            return Err(LayoutWalkError::NameTooLong {
                what: "FILE_FULL_DIR_INFO.FileName",
                offset: entry_off,
                len: name_len,
                max: MAX_NAME_UTF16_BYTES as u32,
            });
        }
        if !name_len.is_multiple_of(2) {
            return Err(LayoutWalkError::OddUtf16Length {
                what: "FILE_FULL_DIR_INFO.FileNameLength",
                offset: entry_off,
                len: name_len,
            });
        }
        let name_bytes = read_bytes(
            buf,
            entry_off + size_of::<RawFullDirEntry>(),
            name_len as usize,
            "FILE_FULL_DIR_INFO.FileName",
        )?;
        let (units, _) = name_bytes.as_chunks::<2>();
        let name: Vec<u16> = units.iter().map(|&unit| u16::from_le_bytes(unit)).collect();
        if name != [u16::from(b'.')] && name != [u16::from(b'.'); 2] {
            let attributes = raw.file_attributes.get();
            entries.push(FolderEntry {
                name,
                attributes,
                size: if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                    raw.end_of_file.get().max(0) as u64
                } else {
                    0
                },
                created: raw.creation_time.get(),
                last_write: raw.last_write_time.get(),
                reparse_tag: (attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
                    .then_some(raw.ea_size.get()),
            });
        }
        let next = raw.next_entry_offset.get();
        if next == 0 {
            return Ok(entries);
        }
        entry_off = follow_offset(
            buf,
            entry_off,
            next,
            size_of::<RawFullDirEntry>() + name_len as usize,
            "FILE_FULL_DIR_INFO.NextEntryOffset",
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One `FILE_ID_EXTD_DIR_INFO` entry, as the fixture builder writes it.
    #[derive(Clone, Debug)]
    struct EntryFixture {
        id: u128,
        attributes: u32,
        size: i64,
        mtime: i64,
        units: Vec<u16>,
        /// `EaSize` in a `FILE_FULL_DIR_INFO` entry, where a reparse point's
        /// tag goes.
        ea: u32,
    }

    fn entry(id: u128, attributes: u32, size: i64, name: &str) -> EntryFixture {
        EntryFixture {
            id,
            attributes,
            size,
            mtime: 133_800_000_000_000_000,
            units: name.encode_utf16().collect(),
            ea: 0,
        }
    }

    /// Lays the entries out back to back on 8-byte boundaries, as Windows
    /// does, with the last entry's `NextEntryOffset` 0.
    fn build_listing(entries: &[EntryFixture]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let start = buf.len();
            let len = (88 + 2 * e.units.len()).next_multiple_of(8);
            let next = if i + 1 == entries.len() {
                0
            } else {
                len as u32
            };
            buf.extend_from_slice(&next.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            for time in [0, 0, e.mtime, 0] {
                buf.extend_from_slice(&time.to_le_bytes());
            }
            buf.extend_from_slice(&e.size.to_le_bytes());
            buf.extend_from_slice(&e.size.to_le_bytes());
            buf.extend_from_slice(&e.attributes.to_le_bytes());
            buf.extend_from_slice(&(2 * e.units.len() as u32).to_le_bytes());
            buf.extend_from_slice(&[0; 8]);
            buf.extend_from_slice(&e.id.to_le_bytes());
            for unit in &e.units {
                buf.extend_from_slice(&unit.to_le_bytes());
            }
            buf.resize(start + len, 0);
        }
        buf
    }

    /// Lays out `FILE_FULL_DIR_INFO` entries as [`build_listing`] does.
    fn build_full_listing(entries: &[EntryFixture]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let start = buf.len();
            let len = (68 + 2 * e.units.len()).next_multiple_of(8);
            let next = if i + 1 == entries.len() {
                0
            } else {
                len as u32
            };
            buf.extend_from_slice(&next.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            for time in [e.mtime - 1, 0, e.mtime, 0] {
                buf.extend_from_slice(&time.to_le_bytes());
            }
            buf.extend_from_slice(&e.size.to_le_bytes());
            buf.extend_from_slice(&e.size.to_le_bytes());
            buf.extend_from_slice(&e.attributes.to_le_bytes());
            buf.extend_from_slice(&(2 * e.units.len() as u32).to_le_bytes());
            buf.extend_from_slice(&e.ea.to_le_bytes());
            for unit in &e.units {
                buf.extend_from_slice(&unit.to_le_bytes());
            }
            buf.resize(start + len, 0);
        }
        buf
    }

    #[test]
    fn raw_view_matches_on_wire_size() {
        assert_eq!(size_of::<RawDirEntry>(), 88);
        assert_eq!(size_of::<RawFullDirEntry>(), 68);
    }

    /// A junction's tag comes from `EaSize`, and only for a reparse point:
    /// a plain file's `EaSize` is the size of its extended attributes.
    #[test]
    fn full_listing_gives_names_sizes_dates_and_reparse_tags() {
        let junction = EntryFixture {
            ea: 0xA000_0003,
            ..entry(0, 0x410, 0, "link")
        };
        let file = EntryFixture {
            ea: 64,
            ..entry(0, 0x20, 4096, "a.txt")
        };
        let buf = build_full_listing(&[
            entry(0, 0x10, 0, "."),
            entry(0, 0x10, 0, ".."),
            junction,
            file,
        ]);
        let entries = walk_full_listing_buffer(&buf).expect("valid listing");
        assert_eq!(
            entries,
            [
                FolderEntry {
                    name: "link".encode_utf16().collect(),
                    attributes: 0x410,
                    size: 0,
                    created: 133_799_999_999_999_999,
                    last_write: 133_800_000_000_000_000,
                    reparse_tag: Some(0xA000_0003),
                },
                FolderEntry {
                    name: "a.txt".encode_utf16().collect(),
                    attributes: 0x20,
                    size: 4096,
                    created: 133_799_999_999_999_999,
                    last_write: 133_800_000_000_000_000,
                    reparse_tag: None,
                },
            ]
        );
    }

    /// A ReFS listing: the folder's own `.` and `..`, a folder and a file,
    /// with 128-bit IDs that use both halves.
    #[test]
    fn listing_gives_names_ids_sizes_and_dates() {
        let folder = Frn(0x702 << 64);
        let buf = build_listing(&[
            entry(0x702 << 64, 0x10, 0, "."),
            entry(0x600 << 64, 0x10, 0, ".."),
            entry(0x7c4 << 64, 0x10, 0, "src"),
            entry((0x702 << 64) | 0x84, 0x20, 4096, "a.txt"),
        ]);
        let files = walk_listing_buffer(folder, &buf).expect("valid listing");
        let name = |name: &str| LayoutName {
            parent_frn: folder,
            name: name.as_bytes().to_vec(),
            dos_only: false,
        };
        assert_eq!(
            files,
            [
                LayoutFile {
                    frn: Frn(0x7c4 << 64),
                    attributes: 0x10,
                    size: None,
                    mtime_ft: Some(133_800_000_000_000_000),
                    names: vec![name("src")],
                },
                LayoutFile {
                    frn: Frn((0x702 << 64) | 0x84),
                    attributes: 0x20,
                    size: Some(4096),
                    mtime_ft: Some(133_800_000_000_000_000),
                    names: vec![name("a.txt")],
                },
            ]
        );
    }

    /// Each check of a listing turns its malformed field into its own
    /// error, for both parsers: the proptests below only ask for no panic,
    /// so a check that stopped firing would pass them while a corrupt
    /// listing read as a shorter one. Both entries put `NextEntryOffset` at
    /// byte 0 and `FileNameLength` at byte 60.
    #[test]
    fn each_malformed_listing_field_is_its_own_error() {
        let entries = [entry(1, 0x20, 1, "ab"), entry(2, 0x20, 1, "cd")];
        type Parse<'a> = &'a dyn Fn(&[u8]) -> Result<(), LayoutWalkError>;
        let parsers: [(Vec<u8>, Parse<'_>); 2] = [
            (build_listing(&entries), &|buf| {
                walk_listing_buffer(Frn(0x600 << 64), buf).map(drop)
            }),
            (build_full_listing(&entries), &|buf| {
                walk_full_listing_buffer(buf).map(drop)
            }),
        ];
        for (valid, parse) in parsers {
            let with = |at: usize, value: u32| {
                let mut buf = valid.clone();
                buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
                parse(&buf)
            };
            assert!(parse(&valid).is_ok());
            assert!(matches!(
                parse(&valid[..50]),
                Err(LayoutWalkError::Truncated { offset: 0, .. })
            ));
            assert!(matches!(
                with(60, MAX_NAME_UTF16_BYTES as u32 + 2),
                Err(LayoutWalkError::NameTooLong { .. })
            ));
            assert!(matches!(
                with(60, 3),
                Err(LayoutWalkError::OddUtf16Length { len: 3, .. })
            ));
            assert!(matches!(
                with(60, 400),
                Err(LayoutWalkError::Truncated { need: 400, .. })
            ));
            assert!(matches!(
                with(0, 10_000),
                Err(LayoutWalkError::OffsetOutOfBounds { .. })
            ));
            assert!(matches!(
                with(0, 8),
                Err(LayoutWalkError::OffsetCycle { rel: 8, .. })
            ));
        }
    }

    fn arb_entry() -> impl Strategy<Value = EntryFixture> {
        (
            any::<u128>(),
            prop_oneof![Just(0x10u32), Just(0x20), Just(0x410)],
            any::<i64>(),
            any::<i64>(),
            prop::collection::vec(any::<u16>(), 0..12),
            any::<u32>(),
        )
            .prop_map(|(id, attributes, size, mtime, units, ea)| EntryFixture {
                id,
                attributes,
                size,
                mtime,
                units,
                ea,
            })
    }

    proptest! {
        /// A listing comes from the filesystem, so any bytes can arrive. A
        /// corrupt one must give an error, not a panic that ends the walk,
        /// and never more name bytes than the buffer holds.
        #[test]
        fn corrupted_listings_error_instead_of_panicking(
            entries in prop::collection::vec(arb_entry(), 1..5),
            patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let mut buf = build_listing(&entries);
            let len = buf.len();
            for (at, byte) in &patches {
                buf[at.index(len)] = *byte;
            }
            buf.truncate(cut.index(len + 1));
            if let Ok(files) = walk_listing_buffer(Frn(0x600 << 64), &buf) {
                let bytes: usize = files.iter().map(|f| f.names[0].name.len()).sum();
                // WTF-8 takes at most 3 bytes per UTF-16 code unit.
                prop_assert!(bytes <= buf.len() * 3 / 2, "{bytes} name bytes in {} bytes", buf.len());
            }
        }

        /// The same for the engine's listing, whose names stay UTF-16.
        #[test]
        fn corrupted_full_listings_error_instead_of_panicking(
            entries in prop::collection::vec(arb_entry(), 1..5),
            patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let mut buf = build_full_listing(&entries);
            let len = buf.len();
            for (at, byte) in &patches {
                buf[at.index(len)] = *byte;
            }
            buf.truncate(cut.index(len + 1));
            if let Ok(entries) = walk_full_listing_buffer(&buf) {
                let bytes: usize = entries.iter().map(|e| 2 * e.name.len()).sum();
                prop_assert!(bytes <= buf.len(), "{bytes} name bytes in {} bytes", buf.len());
            }
        }
    }
}
