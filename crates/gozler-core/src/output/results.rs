//! The one writer of a result set, page by page, for `gozler`'s stdout and
//! `-export-*` files and for the window's Export, so both write the same
//! bytes for the same rows.

use super::{TableOptions, TableWriter};
use crate::proto::QueryResults;
use jiff::tz::TimeZone;
use std::io::{self, Write};

#[cfg(test)]
mod corpus;

/// How a [`ResultWriter`] writes the rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultFormat {
    /// Plain text, CSV, TSV or TXT, through [`TableWriter`].
    Table(TableOptions),
    /// One JSON object at the end: every page, folded with
    /// [`QueryResults::add_page`].
    Json,
}

/// Writes one result set as its pages arrive, so a large export never waits
/// for or holds the whole set. JSON is the exception: it is one object, so it
/// is written when the last page has arrived.
pub struct ResultWriter<W: Write> {
    out: W,
    rows: Rows,
}

enum Rows {
    Table(TableWriter),
    /// The pages so far.
    Json(Option<QueryResults>),
}

impl<W: Write> ResultWriter<W> {
    /// Dates are local time in `time_zone`.
    pub fn new(format: ResultFormat, time_zone: TimeZone, out: W) -> Self {
        let rows = match format {
            ResultFormat::Table(options) => Rows::Table(TableWriter::new(options, time_zone)),
            ResultFormat::Json => Rows::Json(None),
        };
        Self { out, rows }
    }

    /// Writes one page, the first with the byte order mark and the header
    /// that the format asks for.
    ///
    /// # Errors
    ///
    /// The error of writing to `out`.
    pub fn page(&mut self, page: QueryResults) -> io::Result<()> {
        match &mut self.rows {
            Rows::Table(table) => {
                let mut bytes = Vec::new();
                table.write_page(&page.items, &mut bytes);
                self.out.write_all(&bytes)
            }
            Rows::Json(pages) => {
                match pages {
                    None => *pages = Some(page),
                    Some(all) => all.add_page(page),
                }
                Ok(())
            }
        }
    }

    /// Writes what the pages left to write, and flushes.
    ///
    /// # Errors
    ///
    /// The error of writing to `out`.
    pub fn finish(mut self) -> io::Result<()> {
        if let Rows::Json(Some(all)) = &self.rows {
            let json = serde_json::to_string_pretty(all)
                .expect("QueryResults holds only numbers, strings and flags, which serialize");
            writeln!(self.out, "{json}")?;
        }
        self.out.flush()
    }
}
