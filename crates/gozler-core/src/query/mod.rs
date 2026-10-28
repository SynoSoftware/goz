//! The query engine: case folding, the query grammar, and the executor that
//! scans a [`crate::index::VolumeIndex`] and returns its matches' rows.
//!
//! [`crate::fold`] provides case folding; [`parse`] turns a query string into a
//! [`parse::ParsedQuery`] of AND-ed clauses (refusing what it cannot search,
//! such as an unknown function), with `dates` resolving each `dm:` value;
//! [`engine`] compiles it and runs it against the index with
//! scope, sort, and a limit; [`order`] owns the result order the engine and
//! the page builder both sort by; `rows` runs a search over one or more
//! volumes, through [`IndexedVolume`], and reads its rows a page at a time.

mod dates;
pub mod engine;
pub mod mask;
pub mod order;
pub mod parse;
mod paths;
mod rows;
pub(crate) mod wildcard;

pub use dates::{DateField, DateRange, QueryStart};
pub use engine::{CompiledQuery, MatchSums, MatchedParts, SizeOverflow, Totals, extension};
pub use mask::{EmptyMask, MaskScratch, NameMask};
pub use parse::{
    Clause, Kind, Malformed, ParseOptions, ParsedQuery, QueryError, SIZE_WORDS, SizeRange,
    SizeWord, Term, TermMatch, Test, parse_query, parse_query_with, regex_test,
};
pub use paths::{FolderId, ResultNames};
pub use rows::{
    HeldVolume, IndexedVolume, NotIndexed, PathRow, QueryOutcome, ResultRows, ResultSnapshot,
    ResultSnapshotGone, RowView, RowsSpec, SearchTotals, SnapshotRow, SnapshotRows, TooManyVolumes,
    TotalsError, Viewer, run_query, search_totals,
};
