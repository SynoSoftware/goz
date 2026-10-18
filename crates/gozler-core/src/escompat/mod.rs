//! es.exe-compatible command-line parsing, the request a command line sends,
//! gozler's exit codes, the warnings a finished search ends with, and the
//! help.
//!
//! A pure translation of an argv slice into a [`Command`] (what to search,
//! how to sort, what to output) or an [`EsFatal`] carrying the process
//! [`ExitCode`]. The switch-matching rules are lifted from the voidtools es
//! source: case-sensitive names with every interior dash optional, and
//! `/`-prefixed fallthrough to search text.
//!
//! goz-native extensions live behind a `--` prefix (`--json`, `--status`,
//! `--insecure-no-server-check`) so they can never collide with es's
//! single-dash switch namespace.
//!
//! The help is generated from the tables the parsers read: the switches and
//! sort keys in `argv`, the search functions in `query::parse`, and the exit
//! codes in [`ExitCode`]. A later switch, function or sort key is one row of
//! its table, and the help lists it.

mod answer;
mod argv;
mod exit;
mod help;
mod request;

pub use answer::{Answer, Warning};
pub use argv::{Command, Count, DEFAULT_LIMIT, EsFatal, EsPlan, Limit, parse_argv};
pub use exit::ExitCode;
pub use help::{help, version};
pub use request::{build_search, is_drive_relative};
