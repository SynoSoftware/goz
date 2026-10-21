//! What a finished search tells its caller beyond the rows: the warnings
//! gozler writes to stderr and the code it exits with.

use super::{Count, EsPlan, ExitCode};
use crate::output::ResultFormat;
use crate::output::table::grouped;
use crate::proto::{QueryResults, QueryTotals};
use crate::query::ParsedQuery;
use crate::scope::leaves_matches_out;
use crate::types::{EsColumn, SortKey};
use std::fmt;

/// What the pages of one result set say beyond their rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Every match, shown or not.
    pub total: u64,
    /// The rows the pages carried.
    pub shown: u64,
    /// A volume in scope is not ready: indexing, rescanning, offline or failed.
    pub volumes_incomplete: bool,
    /// The query tests the size or the date modified, or totals the size.
    pub reads_size_or_date: bool,
    /// The rows show sizes, in a Size column or JSON, or are sorted by size.
    pub shows_sizes: bool,
    /// The size and date pass failed or stopped on a volume in scope, so
    /// some sizes and dates are unknown.
    pub metadata_pending: bool,
}

impl Answer {
    /// The answer to `query`, parsed for `plan`, before any reply arrives.
    pub fn new(plan: &EsPlan, query: &ParsedQuery) -> Self {
        Answer {
            total: 0,
            shown: 0,
            volumes_incomplete: false,
            reads_size_or_date: query.tests_size_or_date() || plan.count == Some(Count::TotalSize),
            shows_sizes: plan.count.is_none()
                && (plan.columns.contains(&EsColumn::Size)
                    || matches!(plan.result_format(), ResultFormat::Json)
                    || plan.sort.key == SortKey::Size),
            metadata_pending: false,
        }
    }

    /// Counts one results page in.
    pub fn add_page(&mut self, page: &QueryResults) {
        self.total = page.total;
        self.shown += page.items.len() as u64;
        self.volumes_incomplete |= page.volumes_incomplete;
        self.metadata_pending |= page.metadata_pending;
    }

    /// Counts in the totals of a counting search, whose output covers every
    /// match, so the limit leaves none out.
    pub fn add_totals(&mut self, totals: &QueryTotals) {
        self.total = totals.count;
        self.shown = totals.count;
        self.volumes_incomplete |= totals.volumes_incomplete;
        self.metadata_pending |= totals.metadata_pending;
    }

    /// The warnings to write after the rows, in order.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        if self.volumes_incomplete {
            warnings.push(Warning::Incomplete);
        }
        if leaves_matches_out(self.metadata_pending, self.reads_size_or_date) {
            warnings.push(Warning::SizesOrDatesUnknown);
        } else if self.metadata_pending && self.shows_sizes {
            warnings.push(Warning::SizesShort);
        }
        // The daemon counts in `total` only matches a page can carry, so more
        // matches than rows means the limit cut the list.
        if self.shown < self.total {
            warnings.push(Warning::Truncated {
                shown: self.shown,
                total: self.total,
            });
        }
        warnings
    }

    /// The code the search exits with; `no_result_error` is
    /// `-no-result-error`. The first code that applies wins. 2 comes before
    /// 10 because 10 promises that the rows shown are correct, and before 9
    /// because the missing matches may exist.
    pub fn exit_code(&self, no_result_error: bool) -> ExitCode {
        let codes: Vec<ExitCode> = self
            .warnings()
            .into_iter()
            .map(Warning::exit_code)
            .collect();
        if codes.contains(&ExitCode::Incomplete) {
            ExitCode::Incomplete
        } else if codes.contains(&ExitCode::Truncated) {
            ExitCode::Truncated
        } else if no_result_error && self.total == 0 {
            ExitCode::NoMatches
        } else {
            ExitCode::Complete
        }
    }
}

/// A warning gozler writes to stderr after the rows, as
/// `gozler: warning: {warning}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// A volume in scope is not ready: indexing, rescanning, offline or failed.
    Incomplete,
    /// The query tests or totals sizes or dates, and some of them on a volume
    /// in scope are unknown.
    SizesOrDatesUnknown,
    /// The rows show sizes, and some of them on a volume in scope are
    /// unknown, so a folder's size counts only the files whose size is
    /// known. The rows are all there, so the exit code stays 0.
    SizesShort,
    /// The limit showed `shown` of `total` matches.
    Truncated { shown: u64, total: u64 },
}

impl Warning {
    /// The code a search with this warning exits with.
    pub fn exit_code(self) -> ExitCode {
        match self {
            Warning::Incomplete | Warning::SizesOrDatesUnknown => ExitCode::Incomplete,
            Warning::Truncated { .. } => ExitCode::Truncated,
            Warning::SizesShort => ExitCode::Complete,
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Incomplete => f.write_str(
                "results may be incomplete: a volume in scope is not ready; gozler --status \
                 shows why",
            ),
            Warning::SizesOrDatesUnknown => f.write_str(
                "results may be incomplete: gozler could not read the sizes and dates on a \
                 volume in scope, so size and date tests and size totals leave those files \
                 out",
            ),
            Warning::SizesShort => f.write_str(
                "some sizes are unknown: gozler could not read the sizes on a volume in \
                 scope, so those files show no size and a folder's size counts only the \
                 files it read",
            ),
            Warning::Truncated { shown, total } => write!(
                f,
                "showing {} of {} matches; narrow the query or pass -n all",
                grouped(*shown),
                grouped(*total)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(total: u64, shown: u64, volumes_incomplete: bool) -> Answer {
        Answer {
            total,
            shown,
            volumes_incomplete,
            reads_size_or_date: false,
            shows_sizes: false,
            metadata_pending: false,
        }
    }

    /// Bug 4 and the default limit: a list the limit cut, `-n 0` included,
    /// exits 10 with the shown and total counts on stderr, and an incomplete
    /// volume outranks it.
    #[test]
    fn a_cut_list_warns_and_exits_10() {
        let cut = answer(1_234_567, 20, false);
        assert_eq!(
            cut.warnings()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["showing 20 of 1,234,567 matches; narrow the query or pass -n all"]
        );
        assert_eq!(cut.exit_code(false).code(), 10);
        assert_eq!(answer(3, 0, false).exit_code(false).code(), 10);
        assert_eq!(answer(3, 3, false).exit_code(false).code(), 0);
        assert_eq!(answer(300, 20, true).exit_code(false).code(), 2);
    }

    /// A count asks for no rows, yet it covers every match, so it never warns
    /// that the limit cut the list or exits 10.
    #[test]
    fn a_count_is_never_cut() {
        let mut count = answer(0, 0, false);
        count.add_totals(&QueryTotals {
            count: 3,
            size: 0,
            volumes_incomplete: false,
            metadata_pending: false,
        });
        assert!(count.warnings().is_empty());
        assert_eq!(count.exit_code(false).code(), 0);
    }

    /// `-no-result-error` exits 9 only when nothing matched: not when `-n 0`
    /// hid the matches, and not when an incomplete volume may hold some.
    #[test]
    fn no_matches_exits_9_only_with_the_switch() {
        assert_eq!(answer(0, 0, false).exit_code(true).code(), 9);
        assert_eq!(answer(0, 0, false).exit_code(false).code(), 0);
        assert_eq!(answer(3, 0, false).exit_code(true).code(), 10);
        assert_eq!(answer(0, 0, true).exit_code(true).code(), 2);
    }
}
