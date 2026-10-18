//! The one writer of result rows, for every format but JSON: plain text, and
//! es's CSV, TSV and TXT.
//!
//! The CSV bytes are those of voidtools es.exe's `-export-csv`, with gozler
//! v0.2.0's layout: the Filename column comes last, not first, and an export
//! file starts with a UTF-8 byte order mark unless `-no-utf8-bom`. TSV and TXT
//! follow es too. Format facts, from the es source:
//!
//! - The columns follow their switches. Filename, the full path, is added
//!   last unless a Name, Path or Filename column is already there. TXT prints
//!   only the Filename column, whatever the switches ask for.
//! - CSV and TSV start with a header row of es's column names (`Name`,
//!   `Path`, `Filename`, `Extension`, `Size`, `Date Modified`, `Attributes`),
//!   never quoted; `-no-header` leaves it out. Plain text and TXT have none.
//! - Every line ends with CRLF, the last one included, except in plain text,
//!   which ends lines with LF, as goz v0.2.0 did.
//! - A CSV text column is always wrapped in `"` with interior `"` doubled;
//!   with `-no-double-quote` only when it contains `,`, `"`, CR or LF (es's
//!   `_es_should_quote`). TSV quotes a text column only when it contains a
//!   tab, `"`, CR or LF, as es does without `-double-quote`. Plain text and
//!   TXT never quote.
//! - Size is empty for a folder and for an unknown size, and Date Modified for
//!   an unknown date; neither is quoted. CSV and TSV print the size in bytes.
//!   Plain text separates columns with a space, as es's console does: it
//!   groups the size's digits, right-aligns the size and pads the date and
//!   the attributes to es's widths, except after the last value of a line.
//! - Attributes print as letters in plain text (`HA`) and as their
//!   `FILE_ATTRIBUTE_*` number in CSV and TSV (`34`), as es prints them;
//!   empty when the daemon did not send them.
//! - A folder's full path carries no trailing backslash.

use crate::proto::ResultItem;
use crate::query::extension;
use crate::types::EsColumn;
use std::borrow::Cow;

/// How the rows are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The columns separated by spaces, one result per line: gozler's output
    /// when no format switch is given.
    Plain,
    Csv,
    Tsv,
    /// One full path per line.
    Txt,
}

/// What a [`TableWriter`] writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableOptions {
    pub format: Format,
    /// The columns the column switches asked for, in their order; see the
    /// module documentation for the Filename column the writer adds.
    pub columns: Vec<EsColumn>,
    /// Start with the UTF-8 byte order mark (`EF BB BF`).
    pub bom: bool,
    /// Start a CSV or TSV with the header row.
    pub header: bool,
    /// Quote every CSV text column, not only those that need quotes.
    pub double_quote: bool,
}

/// Writes one result set, page by page, so a large export never waits for or
/// holds the whole set. The first page starts with the byte order mark and
/// the header.
pub struct TableWriter {
    format: Format,
    columns: Vec<EsColumn>,
    /// The byte order mark and the header, which the first page writes.
    preamble: Vec<u8>,
    double_quote: bool,
    /// Formats a FILETIME for the Date Modified column; the caller chooses
    /// the time zone.
    format_date: Box<dyn Fn(i64) -> String>,
}

/// Width of the plain-text Size column: `99,999,999,999` fits, as in es.
const PLAIN_SIZE_WIDTH: usize = 14;
/// Width of the plain-text Date Modified column, es's width for a date:
/// `2026-10-17T12:34:56` fits, so an unknown date is as wide as a known one.
const PLAIN_DATE_WIDTH: usize = 19;
/// Width of the plain-text Attributes column, es's: `RASH` fits, and more
/// letters run past it.
const PLAIN_ATTRIBUTES_WIDTH: usize = 4;

impl TableWriter {
    pub fn new(options: TableOptions, format_date: Box<dyn Fn(i64) -> String>) -> Self {
        let mut columns = options.columns;
        if options.format == Format::Txt {
            columns = vec![EsColumn::FullPath];
        } else if !columns
            .iter()
            .any(|c| matches!(c, EsColumn::Name | EsColumn::Path | EsColumn::FullPath))
        {
            columns.push(EsColumn::FullPath);
        }

        let mut preamble = Vec::new();
        if options.bom {
            preamble.extend_from_slice(b"\xEF\xBB\xBF");
        }
        if options.header && matches!(options.format, Format::Csv | Format::Tsv) {
            let names: Vec<&str> = columns.iter().map(|&c| column_name(c)).collect();
            preamble.extend_from_slice(names.join(separator(options.format)).as_bytes());
            preamble.extend_from_slice(b"\r\n");
        }

        Self {
            format: options.format,
            columns,
            preamble,
            double_quote: options.double_quote,
            format_date,
        }
    }

    /// Appends one page of results to `out`.
    pub fn write_page(&mut self, items: &[ResultItem], out: &mut Vec<u8>) {
        out.append(&mut self.preamble);
        for item in items {
            self.write_row(item, out);
        }
    }

    fn write_row(&self, item: &ResultItem, out: &mut Vec<u8>) {
        let plain = self.format == Format::Plain;
        let (folder, name) = item.path.rsplit_once('\\').unwrap_or(("", &item.path));
        for (i, &column) in self.columns.iter().enumerate() {
            let last = i + 1 == self.columns.len();
            let cell: Cow<str> = match column {
                EsColumn::Name => name.into(),
                EsColumn::Path => folder.into(),
                EsColumn::FullPath => item.path.as_str().into(),
                EsColumn::Extension => extension(name.as_bytes(), item.is_dir)
                    .map(|ext| {
                        std::str::from_utf8(ext).expect("a split at an ASCII dot keeps UTF-8 whole")
                    })
                    .unwrap_or("")
                    .into(),
                EsColumn::Size => match item.size.filter(|_| !item.is_dir) {
                    Some(size) if plain => grouped(size).into(),
                    Some(size) => size.to_string().into(),
                    None => "".into(),
                },
                EsColumn::DateModified => item
                    .mtime_ft
                    .map(&self.format_date)
                    .unwrap_or_default()
                    .into(),
                EsColumn::Attributes => match item.attributes {
                    Some(attributes) if plain => attributes.letters().into(),
                    Some(attributes) => attributes.bits().to_string().into(),
                    None => "".into(),
                },
            };
            if i > 0 {
                // Plain text ends a line at its last value, without padding.
                if plain && last && cell.is_empty() {
                    break;
                }
                out.extend_from_slice(separator(self.format).as_bytes());
            }
            match column {
                EsColumn::Name | EsColumn::Path | EsColumn::FullPath | EsColumn::Extension => {
                    self.write_text(&cell, out);
                }
                EsColumn::Size if plain => {
                    out.extend_from_slice(format!("{cell:>PLAIN_SIZE_WIDTH$}").as_bytes());
                }
                EsColumn::DateModified if plain && !last => {
                    out.extend_from_slice(format!("{cell:<PLAIN_DATE_WIDTH$}").as_bytes());
                }
                EsColumn::Attributes if plain && !last => {
                    out.extend_from_slice(format!("{cell:<PLAIN_ATTRIBUTES_WIDTH$}").as_bytes());
                }
                EsColumn::Size | EsColumn::DateModified | EsColumn::Attributes => {
                    out.extend_from_slice(cell.as_bytes());
                }
            }
        }
        out.extend_from_slice(match self.format {
            Format::Plain => b"\n",
            Format::Csv | Format::Tsv | Format::Txt => b"\r\n",
        });
    }

    /// Writes a text column, quoted as the format asks.
    fn write_text(&self, text: &str, out: &mut Vec<u8>) {
        let quoted = match self.format {
            Format::Csv => self.double_quote || needs_quoting(text, b','),
            Format::Tsv => needs_quoting(text, b'\t'),
            Format::Plain | Format::Txt => false,
        };
        if quoted {
            write_quoted(text, out);
        } else {
            out.extend_from_slice(text.as_bytes());
        }
    }
}

/// What separates two columns.
fn separator(format: Format) -> &'static str {
    match format {
        Format::Plain | Format::Txt => " ",
        Format::Csv => ",",
        Format::Tsv => "\t",
    }
}

/// es's header name for a column.
fn column_name(column: EsColumn) -> &'static str {
    match column {
        EsColumn::Name => "Name",
        EsColumn::Path => "Path",
        EsColumn::FullPath => "Filename",
        EsColumn::Extension => "Extension",
        EsColumn::Size => "Size",
        EsColumn::DateModified => "Date Modified",
        EsColumn::Attributes => "Attributes",
    }
}

/// es's `_es_should_quote`: quote when the field contains the separator, a
/// double quote, CR or LF.
fn needs_quoting(field: &str, separator: u8) -> bool {
    field
        .bytes()
        .any(|b| b == separator || matches!(b, b'"' | b'\r' | b'\n'))
}

/// Wraps `field` in double quotes, doubling interior `"` (RFC 4180, as es
/// does).
fn write_quoted(field: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in field.as_bytes() {
        if b == b'"' {
            out.push(b'"');
        }
        out.push(b);
    }
    out.push(b'"');
}

/// `n` with a comma between each group of three digits, as `1,234,567`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn item(path: &str, size: Option<u64>, mtime_ft: Option<i64>, is_dir: bool) -> ResultItem {
        ResultItem {
            path: path.to_string(),
            path_u16: None,
            size,
            mtime_ft,
            is_dir,
            attributes: None,
        }
    }

    fn csv(columns: Vec<EsColumn>, header: bool, double_quote: bool) -> TableOptions {
        TableOptions {
            format: Format::Csv,
            columns,
            bom: true,
            header,
            double_quote,
        }
    }

    fn write(options: TableOptions, items: &[ResultItem]) -> Vec<u8> {
        let mut out = Vec::new();
        TableWriter::new(options, Box::new(|ft: i64| ft.to_string())).write_page(items, &mut out);
        out
    }

    #[test]
    fn zero_rows_still_write_the_mark_and_the_header() {
        let out = write(
            csv(vec![EsColumn::Size, EsColumn::DateModified], true, true),
            &[],
        );
        assert_eq!(
            out.as_slice(),
            b"\xEF\xBB\xBFSize,Date Modified,Filename\r\n"
        );
    }

    #[test]
    fn a_unicode_path_is_written_as_utf8_verbatim() {
        // "C:\ağaç\文件名.txt": ğ=C4 9F, ç=C3 A7, 文=E6 96 87, 件=E4 BB B6,
        // 名=E5 90 8D.
        let out = write(
            csv(vec![], false, true),
            &[item(
                "C:\\a\u{11F}a\u{E7}\\\u{6587}\u{4EF6}\u{540D}.txt",
                Some(1),
                None,
                false,
            )],
        );
        assert_eq!(
            out.as_slice(),
            b"\xEF\xBB\xBF\"C:\\a\xC4\x9Fa\xC3\xA7\\\xE6\x96\x87\xE4\xBB\xB6\xE5\x90\x8D.txt\"\r\n"
        );
    }

    /// Minimal RFC-4180-style reader for the parse-back property: BOM strip,
    /// CRLF records, `"`-quoting with `""` unescape. Panics on malformed
    /// input: the property is that our writer never produces any.
    fn parse_back(mut bytes: &[u8], separator: u8) -> Vec<Vec<String>> {
        if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
            bytes = rest;
        }
        let mut records = Vec::new();
        let mut fields = Vec::new();
        let mut field = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'"' if field.is_empty() => {
                    i += 1;
                    loop {
                        match bytes[i] {
                            b'"' if bytes.get(i + 1) == Some(&b'"') => {
                                field.push(b'"');
                                i += 2;
                            }
                            b'"' => {
                                i += 1;
                                break;
                            }
                            other => {
                                field.push(other);
                                i += 1;
                            }
                        }
                    }
                }
                b if b == separator => {
                    fields.push(String::from_utf8(std::mem::take(&mut field)).unwrap());
                    i += 1;
                }
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                    fields.push(String::from_utf8(std::mem::take(&mut field)).unwrap());
                    records.push(std::mem::take(&mut fields));
                    i += 2;
                }
                other => {
                    field.push(other);
                    i += 1;
                }
            }
        }
        assert!(field.is_empty() && fields.is_empty(), "unterminated record");
        records
    }

    proptest! {
        /// Arbitrary printable paths (quotes, commas, tabs and unicode
        /// included) and sizes survive a write → naive-reader round trip
        /// field-exactly, in CSV with both quoting modes and in TSV, with and
        /// without header and BOM.
        #[test]
        fn parse_back_recovers_exact_fields(
            rows in prop::collection::vec(
                (
                    "[ -~\tçğı文🎉]{1,40}",
                    prop::option::of(proptest::num::u64::ANY),
                    prop::option::of(proptest::num::i64::ANY),
                    proptest::bool::ANY,
                ),
                0..8,
            ),
            tsv in proptest::bool::ANY,
            double_quote in proptest::bool::ANY,
            header in proptest::bool::ANY,
            bom in proptest::bool::ANY,
        ) {
            let options = TableOptions {
                format: if tsv { Format::Tsv } else { Format::Csv },
                columns: vec![EsColumn::Size, EsColumn::DateModified],
                bom,
                header,
                double_quote,
            };
            let items: Vec<ResultItem> = rows
                .iter()
                .map(|(path, size, mtime, is_dir)| item(path, *size, *mtime, *is_dir))
                .collect();
            let out = write(options, &items);

            let mut expected: Vec<Vec<String>> = Vec::new();
            if header {
                expected.push(vec![
                    "Size".into(),
                    "Date Modified".into(),
                    "Filename".into(),
                ]);
            }
            for (path, size, mtime, is_dir) in &rows {
                let size_field = match (is_dir, size) {
                    (false, Some(s)) => s.to_string(),
                    _ => String::new(),
                };
                expected.push(vec![
                    size_field,
                    mtime.map(|ft| ft.to_string()).unwrap_or_default(),
                    path.clone(),
                ]);
            }
            prop_assert_eq!(parse_back(&out, if tsv { b'\t' } else { b',' }), expected);
        }
    }
}
