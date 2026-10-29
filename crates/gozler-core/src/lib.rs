//! gozler-core: the pure, OS-agnostic heart of gozler.
//!
//! Everything that interprets bytes lives here: USN records, MFT enumeration
//! buffers, FILE_LAYOUT entries, the in-memory index and the steps that bring
//! it up to date after each journal batch, the folder walk, the query engine,
//! the wire protocol, CSV/JSON output, and the es.exe-compatible argv rules.
//! This crate has zero `cfg(windows)` code and zero platform dependencies:
//! its entire test suite runs on any OS from fixture buffers and synthetic
//! trees. The Win32 layer (`gozler-winfs`) produces and consumes raw buffers, plain typed
//! structs, and opaque handles; it never parses ioctl record payloads (USN
//! records, MFT enum output, file-layout entries).

pub mod attributes;
pub mod escompat;
pub mod fold;
pub mod include;
pub mod index;
pub mod layout;
pub mod links;
pub mod output;
pub mod proto;
pub mod query;
pub mod scope;
pub mod search;
pub mod types;
pub mod usn;
pub mod winpath;
pub mod wtf8;
