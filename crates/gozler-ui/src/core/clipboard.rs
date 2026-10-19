//! What Copy, Copy as path and Copy name put on the clipboard (DESIGN.md: C8,
//! C9, C10). [`ClipboardData`] knows each format's exact size before it
//! writes a byte, so `win::clipboard` allocates one block of that size and
//! the files are written into it once (DESIGN.md: Copy).

use gozler_core::proto::ResultItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardFormat {
    /// `CF_HDROP`, as File Explorer copies files.
    Files,
    /// `CF_UNICODETEXT`: each full path in double quotes, as File Explorer's
    /// Copy as path.
    Paths,
    /// `CF_UNICODETEXT`: each name.
    Names,
}

/// The size of `DROPFILES`: `pFiles`, `pt`, `fNC` and `fWide`.
const DROPFILES_SIZE: usize = 20;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
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

    /// The bytes [`write_into`](Self::write_into) writes.
    pub(crate) fn len(&self) -> usize {
        let mut units = 0;
        self.each_unit(|_| units += 1);
        DROPFILES_SIZE * usize::from(self.format == ClipboardFormat::Files) + 2 * units
    }

    /// Writes the format into `block`, which is [`len`](Self::len) bytes
    /// long. The text formats end in a NUL, and the file list in two.
    pub(crate) fn write_into(&self, block: &mut [u8]) {
        assert_eq!(block.len(), self.len(), "the block is len() bytes long");
        let mut at = 0;
        if self.format == ClipboardFormat::Files {
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
                ClipboardFormat::Files => {
                    path_units(file, false, &mut unit);
                    unit(0);
                }
                ClipboardFormat::Paths => {
                    if i > 0 {
                        LINE_BREAK.into_iter().for_each(&mut unit);
                    }
                    unit(QUOTE);
                    path_units(file, false, &mut unit);
                    unit(QUOTE);
                }
                ClipboardFormat::Names => {
                    if i > 0 {
                        LINE_BREAK.into_iter().for_each(&mut unit);
                    }
                    path_units(file, true, &mut unit);
                }
            }
        }
        unit(0);
    }
}

/// Calls `unit` with each UTF-16 unit of `file`'s full path, or of its name
/// alone, from its exact units when its UTF-8 path lost an unpaired
/// surrogate.
fn path_units(file: &ResultItem, name_only: bool, unit: &mut impl FnMut(u16)) {
    match &file.path_u16 {
        Some(units) => {
            let start = if name_only {
                units
                    .iter()
                    .rposition(|&u| u == BACKSLASH)
                    .map_or(0, |i| i + 1)
            } else {
                0
            };
            units[start..].iter().copied().for_each(unit);
        }
        None => {
            let start = if name_only {
                file.path.rfind('\\').map_or(0, |i| i + 1)
            } else {
                0
            };
            file.path[start..].encode_utf16().for_each(unit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> ResultItem {
        ResultItem {
            path: path.to_owned(),
            path_u16: None,
            size: None,
            mtime_ft: None,
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
