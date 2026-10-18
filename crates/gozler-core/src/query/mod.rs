//! The query engine: case folding, the query grammar, and the executor that
//! scans a [`crate::index::VolumeIndex`] and returns a sorted page of hits.
//!
//! [`crate::fold`] provides case folding; [`parse`] turns a query string into a
//! [`parse::ParsedQuery`] of AND-ed clauses (refusing what it cannot search,
//! such as an unknown function); [`engine`] compiles it and runs it against the index with
//! scope, sort, and a limit; [`order`] owns the result order the engine and
//! the daemon both sort by.

pub mod engine;
pub mod order;
pub mod parse;
mod paths;

pub use engine::{
    CompiledQuery, QueryHit, QueryOutcome, SizeOverflow, Totals, extension, resolve_scope,
    run_query, run_query_deferrable, run_totals,
};
pub use order::{VolumeRun, order_hits};
pub use parse::{
    Clause, Kind, ParseOptions, ParsedQuery, QueryError, SizeRange, Term, Test, parse_query,
    parse_query_with, parse_query_with_case,
};
