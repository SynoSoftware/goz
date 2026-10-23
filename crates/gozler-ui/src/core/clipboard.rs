//! What Copy, Cut, Copy name, Copy folder and Copy path put on the
//! clipboard (DESIGN.md: C8, C9, C10, C25). [`ClipboardData`] knows each
//! format's exact size before it writes a byte, so `win::clipboard`
//! allocates one block of that size and the files are written into it once
//! (DESIGN.md: Copy).

use gozler_core::proto::ResultItem;
use std::ops::Range;
use std::time::Duration;

pub(crate) const RETRY_AFTER: Duration = Duration::from_millis(20);

pub(crate) struct Publication {
    sequence: u32,
    attempts: u8,
}

impl Publication {
    pub(crate) fn new(sequence: u32) -> Self {
        Self {
            sequence,
            attempts: 0,
        }
    }

    /// Our preceding publication may have completed after this Copy was accepted.
    pub(crate) fn is_current(&self, sequence: u32, preceding: u32) -> bool {
        sequence == self.sequence || (preceding != 0 && sequence == preceding)
    }

    pub(crate) fn retry(&mut self) -> bool {
        self.attempts += 1;
        self.attempts < 5
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardFormat {
    /// `CF_HDROP` with the copy effect, as File Explorer's Copy puts
    /// files there.
    Files,
    /// `CF_HDROP` with the move effect, as File Explorer's Cut puts files
    /// there.
    CutFiles,
    /// `CF_UNICODETEXT`: each full path in double quotes, as File Explorer's
    /// Copy as path.
    Paths,
    /// `CF_UNICODETEXT`: each name.
    Names,
    /// `CF_UNICODETEXT`: the folder that holds each entry, a drive's root
    /// with its backslash, as `C:\`.
    Folders,
}

impl ClipboardFormat {
    /// Whether it is a file list, `CF_HDROP`.
    pub(crate) fn is_files(self) -> bool {
        matches!(self, ClipboardFormat::Files | ClipboardFormat::CutFiles)
    }
}

/// The part of a file's path that a text format writes.
#[derive(Clone, Copy)]
enum Part {
    Full,
    Name,
    Folder,
}

/// The size of `DROPFILES`: `pFiles`, `pt`, `fNC` and `fWide`.
const DROPFILES_SIZE: usize = 20;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const COLON: u16 = b':' as u16;
const LINE_BREAK: [u16; 2] = [b'\r' as u16, b'\n' as u16];

pub(crate) struct ClipboardData<'a> {
    format: ClipboardFormat,
    files: &'a [ResultItem],
}

impl<'a> ClipboardData<'a> {
    pub(crate) fn new(format: ClipboardFormat, files: &'a [ResultItem]) -> ClipboardData<'a> {
        ClipboardData { format, files }
    }

    pub(crate) fn format(&self) -> ClipboardFormat {
        self.format
    }

    pub(crate) fn items(&self) -> usize {
        self.files.len()
    }

    /// The bytes [`write_into`](Self::write_into) writes.
    pub(crate) fn len(&self) -> usize {
        let mut units = 0;
        self.each_unit(|_| units += 1);
        DROPFILES_SIZE * usize::from(self.format.is_files()) + 2 * units
    }

    /// Writes the format into `block`, which is [`len`](Self::len) bytes
    /// long. The text formats end in a NUL, and the file list in two.
    pub(crate) fn write_into(&self, block: &mut [u8]) {
        // Only in debug builds, because it walks every file once more. In
        // release, a shorter block panics at the write past its end, and a
        // longer one at the assert after the last write.
        debug_assert_eq!(block.len(), self.len(), "the block is len() bytes long");
        let mut at = 0;
        if self.format.is_files() {
            let pfiles = DROPFILES_SIZE as u32;
            let fwide = 1u32;
            // pFiles, then pt and fNC, which stay 0, then fWide.
            block[..4].copy_from_slice(&pfiles.to_le_bytes());
            block[4..16].fill(0);
            block[16..20].copy_from_slice(&fwide.to_le_bytes());
            at = DROPFILES_SIZE;
        }
        self.each_unit(|unit| {
            block[at..at + 2].copy_from_slice(&unit.to_le_bytes());
            at += 2;
        });
        assert_eq!(at, block.len(), "len() counts every unit written");
    }

    /// Calls `unit` with each UTF-16 unit of the format after the header.
    fn each_unit(&self, mut unit: impl FnMut(u16)) {
        for (i, file) in self.files.iter().enumerate() {
            match self.format {
                ClipboardFormat::Files | ClipboardFormat::CutFiles => {
                    path_units(file, Part::Full, &mut unit);
                    unit(0);
                }
                ClipboardFormat::Paths => {
                    if i > 0 {
                        LINE_BREAK.into_iter().for_each(&mut unit);
                    }
                    unit(QUOTE);
                    path_units(file, Part::Full, &mut unit);
                    unit(QUOTE);
                }
                ClipboardFormat::Names | ClipboardFormat::Folders => {
                    if i > 0 {
                        LINE_BREAK.into_iter().for_each(&mut unit);
                    }
                    let part = if self.format == ClipboardFormat::Names {
                        Part::Name
                    } else {
                        Part::Folder
                    };
                    path_units(file, part, &mut unit);
                }
            }
        }
        unit(0);
    }
}

/// Calls `unit` with each UTF-16 unit of `part` of `file`'s path, from its
/// exact units when its UTF-8 path lost an unpaired surrogate.
fn path_units(file: &ResultItem, part: Part, unit: &mut impl FnMut(u16)) {
    match &file.path_u16 {
        Some(units) => units[part_of(units, part)].iter().copied().for_each(unit),
        None => {
            let path = file.path.as_str();
            path[part_of(path.as_bytes(), part)]
                .encode_utf16()
                .for_each(unit);
        }
    }
}

/// Where `part` lies in `path`, whose units are UTF-16 or UTF-8: a
/// backslash and a colon are one unit in both, and in UTF-8 no other
/// character holds their byte, so the range ends between characters.
fn part_of<T: Copy + Into<u32>>(path: &[T], part: Part) -> Range<usize> {
    let last = path.iter().rposition(|&u| u.into() == u32::from(BACKSLASH));
    match (part, last) {
        (Part::Full, _) | (Part::Name, None) => 0..path.len(),
        (Part::Name, Some(i)) => i + 1..path.len(),
        (Part::Folder, _) => 0..folder_of(path).len(),
    }
}

/// The folder that holds `path`, a drive's root with its backslash, as
/// `C:\`; empty when `path` has no backslash.
pub(crate) fn folder_of<T: Copy + Into<u32>>(path: &[T]) -> &[T] {
    match path.iter().rposition(|&u| u.into() == u32::from(BACKSLASH)) {
        None => &[],
        Some(i) if i > 0 && path[i - 1].into() == u32::from(COLON) => &path[..=i],
        Some(i) => &path[..i],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_copy_stops_retrying_and_never_replaces_an_unrelated_copy() {
        let mut publication = Publication::new(10);
        for _ in 0..4 {
            assert!(publication.retry());
        }
        assert!(!publication.retry());
        assert!(publication.is_current(10, 0));
        assert!(
            publication.is_current(11, 11),
            "our preceding copy finished"
        );
        assert!(!publication.is_current(12, 11), "another copy replaced it");
        assert!(
            !publication.is_current(0, 0),
            "an unknown preceding copy is no permission"
        );
    }

    fn file(path: &str) -> ResultItem {
        ResultItem {
            path: path.to_owned(),
            path_u16: None,
            size: None,
            mtime_ft: None,
            created_ft: None,
            is_dir: false,
            attributes: None,
        }
    }

    fn written(format: ClipboardFormat, files: &[ResultItem]) -> Vec<u8> {
        let data = ClipboardData::new(format, files);
        let mut block = vec![0xAA; data.len()];
        data.write_into(&mut block);
        block
    }

    fn text(bytes: &[u8]) -> String {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16(&units).expect("the test paths are UTF-16")
    }

    fn three() -> Vec<ResultItem> {
        vec![
            file(r"C:\repo\report.txt"),
            file(r"C:\repo\docs"),
            file(r"D:\a b.pdf"),
        ]
    }

    /// File Explorer and editors read these bytes; a wrong length leaves
    /// garbage after the text, or a write past the block.
    #[test]
    fn copy_as_path_and_copy_name_give_one_line_each() {
        assert_eq!(
            text(&written(ClipboardFormat::Paths, &three())),
            "\"C:\\repo\\report.txt\"\r\n\"C:\\repo\\docs\"\r\n\"D:\\a b.pdf\"\0"
        );
        assert_eq!(
            text(&written(ClipboardFormat::Names, &three())),
            "report.txt\r\ndocs\r\na b.pdf\0"
        );
    }

    /// Copy folder slices each path where its last backslash is, also in
    /// UTF-8, and a drive's root keeps its backslash, because `D:` alone
    /// names the current folder of drive D.
    #[test]
    fn copy_folder_gives_the_folder_of_each_entry() {
        let files = [file(r"C:\Ünï\é.txt"), file(r"D:\a b.pdf")];
        assert_eq!(
            text(&written(ClipboardFormat::Folders, &files)),
            "C:\\Ünï\r\nD:\\\0"
        );
    }

    /// File Explorer reads the header to find the list and its encoding; a
    /// wrong `pFiles` or `fWide` pastes nothing, or garbage names.
    #[test]
    fn copy_gives_the_dropfiles_header_and_the_double_nul_list() {
        let files = three();
        let block = written(ClipboardFormat::Files, &files[..2]);
        let mut expected = Vec::new();
        expected.extend_from_slice(&20u32.to_le_bytes());
        expected.extend_from_slice(&[0; 12]);
        expected.extend_from_slice(&1u32.to_le_bytes());
        for unit in "C:\\repo\\report.txt\0C:\\repo\\docs\0\0".encode_utf16() {
            expected.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(block, expected);
    }

    /// A name with an unpaired surrogate must reach the clipboard as Windows
    /// names it, or the paste names another file or none.
    #[test]
    fn a_lossy_path_is_copied_from_its_exact_units() {
        let mut lossy = file("C:\\x\u{FFFD}");
        lossy.path_u16 = Some(vec![
            u16::from(b'C'),
            u16::from(b':'),
            BACKSLASH,
            0x78,
            0xD800,
        ]);
        let block = written(ClipboardFormat::Names, &[lossy]);
        assert_eq!(block, [0x78, 0, 0x00, 0xD8, 0, 0]);
    }
}
