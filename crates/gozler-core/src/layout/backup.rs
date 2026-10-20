//! The header of one stream in `BackupRead`'s output (`WIN32_STREAM_ID`),
//! which the copy engine's ring reads to copy a file's alternate data
//! streams and extended attributes.
//!
//! <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-win32_stream_id>
//!
//! The fixed part is 20 bytes. The stream's name, `dwStreamNameSize` bytes
//! of UTF-16, follows it, then `Size` bytes of the stream's data, then the
//! next header.

use zerocopy::little_endian::{I64, U32};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

use crate::types::MAX_NAME_UTF16_BYTES;

/// The bytes of a stream header before its name.
pub const BACKUP_HEADER_LEN: usize = 20;

/// An alternate data stream's name is `:name:$DATA`.
const MAX_STREAM_NAME_BYTES: usize = MAX_NAME_UTF16_BYTES + 2 * ":".len() + 2 * ":$DATA".len();

#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawStreamId {
    stream_id: U32,
    stream_attributes: U32,
    size: I64,
    stream_name_size: U32,
}

/// What a stream holds, as far as the copy engine tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupStream {
    /// `BACKUP_EA_DATA`.
    ExtendedAttributes,
    /// `BACKUP_ALTERNATE_DATA`, such as `Zone.Identifier`.
    AlternateData,
    /// Any other stream, with its `dwStreamId`: the main data, its sparse
    /// blocks, the object ID and the rest, which the ring does not copy
    /// this way.
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupStreamHeader {
    pub stream: BackupStream,
    /// The bytes of the stream's name that follow the header.
    pub name_bytes: usize,
    /// The bytes of the stream's data that follow its name.
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BackupHeaderError {
    #[error("a stream header needs {BACKUP_HEADER_LEN} bytes, and {0} arrived")]
    Truncated(usize),
    #[error("a stream's size is negative: {0}")]
    NegativeSize(i64),
    #[error(
        "a stream's name of {0} bytes is not a UTF-16 name of at most {MAX_STREAM_NAME_BYTES} bytes"
    )]
    BadName(u32),
}

/// Parses the stream header at the start of `bytes`.
pub fn parse_backup_header(bytes: &[u8]) -> Result<BackupStreamHeader, BackupHeaderError> {
    let (raw, _) = RawStreamId::ref_from_prefix(bytes)
        .map_err(|_| BackupHeaderError::Truncated(bytes.len()))?;
    let size = raw.size.get();
    let size = u64::try_from(size).map_err(|_| BackupHeaderError::NegativeSize(size))?;
    let name_bytes = raw.stream_name_size.get();
    if name_bytes % 2 != 0 || name_bytes as usize > MAX_STREAM_NAME_BYTES {
        return Err(BackupHeaderError::BadName(name_bytes));
    }
    let stream = match raw.stream_id.get() {
        2 => BackupStream::ExtendedAttributes,
        4 => BackupStream::AlternateData,
        other => BackupStream::Other(other),
    };
    Ok(BackupStreamHeader {
        stream,
        name_bytes: name_bytes as usize,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(id: u32, size: i64, name_bytes: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&name_bytes.to_le_bytes());
        bytes
    }

    /// The header comes from the disk, so a wrong size or name length must
    /// be an error, because the ring would otherwise read a name of any
    /// length into memory.
    #[test]
    fn malformed_headers_are_errors() {
        let zone = ":Zone.Identifier:$DATA".encode_utf16().count() as u32 * 2;
        assert_eq!(
            parse_backup_header(&header(4, 26, zone)),
            Ok(BackupStreamHeader {
                stream: BackupStream::AlternateData,
                name_bytes: zone as usize,
                size: 26,
            })
        );
        assert_eq!(
            parse_backup_header(&header(4, 26, zone)[..19]),
            Err(BackupHeaderError::Truncated(19))
        );
        assert_eq!(
            parse_backup_header(&header(4, -1, zone)),
            Err(BackupHeaderError::NegativeSize(-1))
        );
        assert_eq!(
            parse_backup_header(&header(4, 26, 525)),
            Err(BackupHeaderError::BadName(525))
        );
        assert_eq!(
            parse_backup_header(&header(4, 26, 526)),
            Err(BackupHeaderError::BadName(526))
        );
    }
}
