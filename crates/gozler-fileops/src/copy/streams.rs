//! What the ring copies after a file's data, as FastCopy does: every
//! alternate data stream and the extended attributes through `BackupRead`
//! and `BackupWrite`, then the attributes and times. Security descriptors
//! are not copied, as `CopyFile2` does not copy them by default.
//! AngelCOPY's ring copies only times and attributes, and so loses
//! `Zone.Identifier`, the mark that makes Windows warn before it runs a
//! downloaded file.

use gozler_core::layout::{BACKUP_HEADER_LEN, BackupStream, parse_backup_header};
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

fn copy_streams(from: &Entry, to: &Entry) -> Result<(), u32> {
    let code = |error: gozler_winfs::WinError| error.code;
    let mut reader = BackupReader::new(from);
    let mut writer = BackupWriter::new(to);
    let mut header = [0u8; BACKUP_HEADER_LEN];
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = read_full(&mut reader, &mut header)?;
        if read == 0 {
            return Ok(());
        }
        if read < BACKUP_HEADER_LEN {
            return Err(ERROR_INVALID_DATA);
        }
        let stream = parse_backup_header(&header).map_err(|_| ERROR_INVALID_DATA)?;
        let name = &mut buffer[..stream.name_bytes];
        if read_full(&mut reader, name)? < name.len() {
            return Err(ERROR_INVALID_DATA);
        }
        let copied = matches!(
            stream.stream,
            BackupStream::AlternateData | BackupStream::ExtendedAttributes
        );
        if copied {
            writer.write(&header).map_err(code)?;
            writer.write(&buffer[..stream.name_bytes]).map_err(code)?;
        }
        let mut left = stream.size;
        if !copied {
            left -= reader.skip(left).min(left);
        }
        while left > 0 {
            let chunk = left.min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..chunk]).map_err(code)?;
            if read == 0 {
                return Err(ERROR_INVALID_DATA);
            }
            if copied {
                writer.write(&buffer[..read]).map_err(code)?;
            }
            left -= read as u64;
        }
    }
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
