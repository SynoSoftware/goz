//! The query engine: case folding, the query grammar, and the executor that
//! scans a [`crate::index::VolumeIndex`] and returns a sorted page of hits.
//!
//! [`crate::fold`] provides case folding; [`parse`] turns a query string into a
//! [`parse::ParsedQuery`] of AND-ed clauses (refusing what it cannot search,
//! such as an unknown function), with `dates` resolving each `dm:` value;
//! [`engine`] compiles it and runs it against the index with
//! scope, sort, and a limit; [`order`] owns the result order the engine and
//! the daemon both sort by.

mod dates;
pub mod engine;
pub mod order;
pub mod parse;
mod paths;
pub(crate) mod wildcard;

pub use dates::{DateRange, QueryStart};
pub use engine::{
    CompiledQuery, IndexChanged, MatchSums, MatchedParts, Matches, OrderedMatches, QueryHit,
    QueryOutcome, SizeOverflow, Totals, extension, find_matches, resolve_scope, run_query,
    run_totals,
};
pub use order::{OrderedHits, VolumeRun, order_hits};
pub use parse::{
    Clause, Kind, Malformed, ParseOptions, ParsedQuery, QueryError, SIZE_WORDS, SizeRange,
    SizeWord, Term, TermMatch, Test, parse_query, parse_query_with, regex_test,
};
pub use paths::{FolderId, NameRef, ResultNames};
