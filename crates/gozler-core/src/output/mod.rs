//! Output formatting: the writer of result rows and FILETIME
//! conversion/formatting helpers.
//!
//! [`table`] writes plain text and es.exe's CSV, TSV and TXT byte streams
//! (spec extracted from the es source), with goz v0.2.0's column layout.
//! [`filetime`] converts between FILETIME ticks and Unix milliseconds and
//! renders civil UTC timestamps without a timezone database (the CLI formats
//! local time itself).

pub mod filetime;
pub mod table;

pub use filetime::{
    FILETIME_UNIX_EPOCH, filetime_to_unix_ms, format_filetime_utc, unix_ms_to_filetime,
};
pub use table::{Format, TableOptions, TableWriter};
