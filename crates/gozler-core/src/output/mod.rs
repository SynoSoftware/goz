//! Output formatting: the writer of result rows and FILETIME conversion
//! helpers.
//!
//! [`table`] writes plain text and es.exe's CSV, TSV and TXT byte streams
//! (spec extracted from the es source), with goz v0.2.0's column layout.
//! [`filetime`] converts between FILETIME ticks and Unix milliseconds, and
//! formats the dates that results and exports print.
//! [`results`] writes a whole result set, page by page, in any format.
//! [`status`] writes the `--status` report.
//! [`style`] colors the help and the overview.

pub mod filetime;
pub mod results;
pub mod status;
pub mod style;
pub mod table;

pub use filetime::{
    FILETIME_UNIX_EPOCH, filetime_to_unix_ms, format_date_local, unix_ms_to_filetime,
};
pub use results::{ResultFormat, ResultWriter};
pub use status::{bytes, write_status};
pub use table::{Format, TableOptions, TableWriter};
