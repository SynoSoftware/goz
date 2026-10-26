//! USN journal byte-level machinery: record parsing, output-buffer walking,
//! and reason-bit normalization.
//!
//! Everything here is pure: the Win32 layer (`gozler-winfs`) hands over raw
//! `FSCTL_ENUM_USN_DATA` / `FSCTL_READ_USN_JOURNAL` output buffers and this
//! module turns them into typed records ([`record`]) and normalizes
//! accumulated reason bitmasks into idempotent index ops ([`ops`]). The whole
//! module is testable from fixture buffers on any OS ([`fixtures`]).

pub mod ops;
pub mod record;

// `doc` as well as `test` so the fixture story stays visible in rustdoc: the
// module-level claim above links to it, and that claim is the reason the crate
// is structured this way.
#[cfg(any(test, doc))]
pub mod fixtures;

pub use ops::{UsnOp, ops_for};
pub use record::{
    EnumWalk, JournalWalk, ParsedUsnRecord, WalkError, record_usn, walk_enum_buffer,
    walk_journal_buffer,
};
