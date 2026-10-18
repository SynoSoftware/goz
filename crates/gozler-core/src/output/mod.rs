//! Output formatting: the writer of result rows and FILETIME conversion
//! helpers.
//!
//! [`table`] writes plain text and es.exe's CSV, TSV and TXT byte streams
//! (spec extracted from the es source), with goz v0.2.0's column layout.
//! [`filetime`] converts between FILETIME ticks and Unix milliseconds.
//! [`style`] colors the help and the overview.

pub mod filetime;
pub mod style;
pub mod table;

pub use filetime::{FILETIME_UNIX_EPOCH, filetime_to_unix_ms, unix_ms_to_filetime};
pub use table::{Format, TableOptions, TableWriter};
