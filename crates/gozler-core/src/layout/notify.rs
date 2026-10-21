//! Parser for the output buffer of `ReadDirectoryChangesW`: a chain of
//! `FILE_NOTIFY_INFORMATION` entries, one for each change of a name in the
//! watched folder, which the window's panels apply by name.
//!
//! <https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-file_notify_information>
//!
//! `NextEntryOffset` is relative to the entry that holds it, and 0 ends the
//! chain, as in a folder listing.

use super::parse::{LayoutWalkError, follow_offset, read_bytes, read_struct};
use crate::types::MAX_NAME_UTF16_BYTES;
use zerocopy::little_endian::U32;
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

/// `FILE_NOTIFY_INFORMATION` fixed part (12 bytes); `FileName` (UTF-16LE,
/// `FileNameLength` bytes, not null-terminated) follows.
#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawNotify {
    next_entry_offset: U32,
    action: U32,
    file_name_length: U32,
}

/// What happened to a name in the watched folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyAction {
    Added,
    Removed,
    Modified,
    /// The name an entry had before a rename; its new name follows.
    RenamedFrom,
    RenamedTo,
}

impl NotifyAction {
    fn of(action: u32) -> Option<NotifyAction> {
        Some(match action {
            1 => NotifyAction::Added,
            2 => NotifyAction::Removed,
            3 => NotifyAction::Modified,
            4 => NotifyAction::RenamedFrom,
            5 => NotifyAction::RenamedTo,
            _ => return None,
        })
    }
}

/// One change of a name in the watched folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notified {
    pub action: NotifyAction,
    /// The name as raw UTF-16, which may hold unpaired surrogates.
    pub name: Vec<u16>,
}

/// Parses one `ReadDirectoryChangesW` buffer into its changes, in the order
/// they happened. An empty buffer yields none; an action that a later
/// Windows adds is left out.
pub fn walk_notify_buffer(buf: &[u8]) -> Result<Vec<Notified>, LayoutWalkError> {
    let mut changes = Vec::new();
    if buf.is_empty() {
        return Ok(changes);
    }
    let mut entry_off = 0;
    loop {
        let raw: RawNotify = read_struct(buf, entry_off, "FILE_NOTIFY_INFORMATION")?;
        let name_len = raw.file_name_length.get();
        if name_len as usize > MAX_NAME_UTF16_BYTES {
            return Err(LayoutWalkError::NameTooLong {
                what: "FILE_NOTIFY_INFORMATION.FileName",
                offset: entry_off,
                len: name_len,
                max: MAX_NAME_UTF16_BYTES as u32,
            });
        }
        if !name_len.is_multiple_of(2) {
            return Err(LayoutWalkError::OddUtf16Length {
                what: "FILE_NOTIFY_INFORMATION.FileNameLength",
                offset: entry_off,
                len: name_len,
            });
        }
        let name_bytes = read_bytes(
            buf,
            entry_off + size_of::<RawNotify>(),
            name_len as usize,
            "FILE_NOTIFY_INFORMATION.FileName",
        )?;
        if let Some(action) = NotifyAction::of(raw.action.get()) {
            let (units, _) = name_bytes.as_chunks::<2>();
            changes.push(Notified {
                action,
                name: units.iter().map(|&unit| u16::from_le_bytes(unit)).collect(),
            });
        }
        let next = raw.next_entry_offset.get();
        if next == 0 {
            return Ok(changes);
        }
        entry_off = follow_offset(
            buf,
            entry_off,
            next,
            size_of::<RawNotify>() + name_len as usize,
            "FILE_NOTIFY_INFORMATION.NextEntryOffset",
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Lays the changes out back to back on 4-byte boundaries, as Windows
    /// does, with the last entry's `NextEntryOffset` 0.
    fn build(changes: &[(u32, Vec<u16>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (i, (action, units)) in changes.iter().enumerate() {
            let start = buf.len();
            let len = (12 + 2 * units.len()).next_multiple_of(4);
            let next = if i + 1 == changes.len() {
                0
            } else {
                len as u32
            };
            buf.extend_from_slice(&next.to_le_bytes());
            buf.extend_from_slice(&action.to_le_bytes());
            buf.extend_from_slice(&(2 * units.len() as u32).to_le_bytes());
            for unit in units {
                buf.extend_from_slice(&unit.to_le_bytes());
            }
            buf.resize(start + len, 0);
        }
        buf
    }

    /// A rename arrives as its two names in order, and an action this
    /// parser does not know is left out instead of failing the buffer.
    #[test]
    fn a_rename_reads_as_its_two_names() {
        let wide = |name: &str| name.encode_utf16().collect::<Vec<u16>>();
        let buf = build(&[(4, wide("a.txt")), (9, wide("x")), (5, wide("b.txt"))]);
        assert_eq!(
            walk_notify_buffer(&buf).expect("a valid buffer"),
            [
                Notified {
                    action: NotifyAction::RenamedFrom,
                    name: wide("a.txt"),
                },
                Notified {
                    action: NotifyAction::RenamedTo,
                    name: wide("b.txt"),
                },
            ]
        );
    }

    proptest! {
        /// The buffer comes from the file system's driver, and a network
        /// redirector fills it too, so a corrupt one must give an error, not
        /// a panic that ends the window, and never more name bytes than the
        /// buffer holds.
        #[test]
        fn corrupted_buffers_error_instead_of_panicking(
            changes in prop::collection::vec(
                (1u32..6, prop::collection::vec(any::<u16>(), 0..20)),
                1..5,
            ),
            patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
            cut in any::<prop::sample::Index>(),
        ) {
            let mut buf = build(&changes);
            let len = buf.len();
            for (at, byte) in &patches {
                buf[at.index(len)] = *byte;
            }
            buf.truncate(cut.index(len + 1));
            if let Ok(parsed) = walk_notify_buffer(&buf) {
                let units: usize = parsed.iter().map(|change| change.name.len()).sum();
                prop_assert!(units * 2 <= buf.len(), "{units} name units in {} bytes", buf.len());
            }
        }
    }
}
