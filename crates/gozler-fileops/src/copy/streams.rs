//! What the ring copies after a file's data, as FastCopy does: every
//! alternate data stream and the extended attributes through `BackupRead`
//! and `BackupWrite`, then the attributes and times. Security descriptors
//! are not copied, as `CopyFile2` does not copy them by default.
//! AngelCOPY's ring copies only times and attributes, and so loses
//! `Zone.Identifier`, the mark that makes Windows warn before it runs a
//! downloaded file.

use gozler_core::layout::{
    BACKUP_HEADER_LEN, BackupStream, BackupStreamHeader, parse_backup_header,
};
use gozler_winfs::{BackupReader, BackupWriter, BasicInfo, Entry};

use crate::win32::{ERROR_INVALID_DATA, FILE_ATTRIBUTE_NORMAL};

/// The attributes that `FileBasicInfo` sets: read-only, hidden, system,
/// archive, temporary and not content indexed. The sparse attribute comes
/// from `FSCTL_SET_SPARSE`; compression, encryption and reparse points are
/// not the ring's.
const SETTABLE: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x100 | 0x2000;

/// Sets the destination's true end of file, copies the other streams of
/// `source` to `destination`, and sets `basic`'s attributes and times last,
/// so that no later write changes them. Both are the ring's handles, which
/// it reopens buffered and synchronous for this, as AngelCOPY reopens its
/// destination to set the end of file.
pub(super) fn finish_streams(
    source: &Entry,
    destination: &Entry,
    size: u64,
    basic: BasicInfo,
) -> Result<(), u32> {
    let code = |error: gozler_winfs::WinError| error.code;
    let from = source.reopen_for_streams(false).map_err(code)?;
    let to = destination.reopen_for_streams(true).map_err(code)?;
    to.set_end_of_file(size).map_err(code)?;
    copy_streams(&from, &to)?;
    let attributes = match basic.attributes & SETTABLE {
        // Zero would leave the new file's attributes as they are.
        0 => FILE_ATTRIBUTE_NORMAL,
        attributes => attributes,
    };
    to.set_basic_info(BasicInfo {
        attributes,
        ..basic
    })
    .map_err(code)
}

/// Whether `file`, one of the ring's handles, has an alternate data stream
/// or extended attributes, which a file the ring resumes would keep whether
/// or not the source has them.
pub(super) fn has_other_streams(file: &Entry) -> Result<bool, u32> {
    let file = file.reopen_for_streams(false).map_err(|error| error.code)?;
    let mut reader = BackupReader::new(&file);
    let mut header = [0u8; BACKUP_HEADER_LEN];
    let mut buffer = vec![0u8; 64 * 1024];
    while let Some(stream) = next_stream(&mut reader, &mut header, &mut buffer)? {
        if is_copied(stream.stream) {
            return Ok(true);
        }
        skip_data(&mut reader, stream.size, &mut buffer)?;
    }
    Ok(false)
}

fn copy_streams(from: &Entry, to: &Entry) -> Result<(), u32> {
    let code = |error: gozler_winfs::WinError| error.code;
    let mut reader = BackupReader::new(from);
    let mut writer = BackupWriter::new(to);
    let mut header = [0u8; BACKUP_HEADER_LEN];
    let mut buffer = vec![0u8; 64 * 1024];
    while let Some(stream) = next_stream(&mut reader, &mut header, &mut buffer)? {
        if !is_copied(stream.stream) {
            skip_data(&mut reader, stream.size, &mut buffer)?;
            continue;
        }
        writer.write(&header).map_err(code)?;
        writer.write(&buffer[..stream.name_bytes]).map_err(code)?;
        let mut left = stream.size;
        while left > 0 {
            let chunk = left.min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..chunk]).map_err(code)?;
            if read == 0 {
                return Err(ERROR_INVALID_DATA);
            }
            writer.write(&buffer[..read]).map_err(code)?;
            left -= read as u64;
        }
    }
    Ok(())
}

fn is_copied(stream: BackupStream) -> bool {
    matches!(
        stream,
        BackupStream::AlternateData | BackupStream::ExtendedAttributes
    )
}

/// Reads the next stream's header into `header` and its name into the start
/// of `buffer`; `None` after the last stream.
fn next_stream(
    reader: &mut BackupReader<'_>,
    header: &mut [u8; BACKUP_HEADER_LEN],
    buffer: &mut [u8],
) -> Result<Option<BackupStreamHeader>, u32> {
    let read = read_full(reader, header)?;
    if read == 0 {
        return Ok(None);
    }
    if read < BACKUP_HEADER_LEN {
        return Err(ERROR_INVALID_DATA);
    }
    let stream = parse_backup_header(header).map_err(|_| ERROR_INVALID_DATA)?;
    let name = &mut buffer[..stream.name_bytes];
    if read_full(reader, name)? < name.len() {
        return Err(ERROR_INVALID_DATA);
    }
    Ok(Some(stream))
}

/// Passes over a stream's `size` bytes of data.
fn skip_data(reader: &mut BackupReader<'_>, size: u64, buffer: &mut [u8]) -> Result<(), u32> {
    let mut left = size - reader.skip(size).min(size);
    while left > 0 {
        let chunk = left.min(buffer.len() as u64) as usize;
        let read = reader
            .read(&mut buffer[..chunk])
            .map_err(|error| error.code)?;
        if read == 0 {
            return Err(ERROR_INVALID_DATA);
        }
        left -= read as u64;
    }
    Ok(())
}

/// Reads until `buf` is full or the streams end; returns the bytes read.
fn read_full(reader: &mut BackupReader<'_>, buf: &mut [u8]) -> Result<usize, u32> {
    let mut filled = 0;
    while filled < buf.len() {
        let read = reader
            .read(&mut buf[filled..])
            .map_err(|error| error.code)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}
