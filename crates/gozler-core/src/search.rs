//! The search that typed text and its options ask for. `gozler` builds it from
//! its command line and the window from its search box and match controls,
//! so the same search sends the same request from both.

use crate::proto::{Returns, SearchRequest};
use crate::query::{
    Clause, ParseOptions, ParsedQuery, QueryError, QueryStart, TermMatch, Test, parse_query_with,
    regex_test,
};

/// How the typed text matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMatch {
    /// The text is a query, whose name terms match as the [`TermMatch`]
    /// says.
    Terms(TermMatch),
    /// The whole text is one regular expression, as `-r` takes its argument.
    Regex,
}

/// What a person asks for, before it is parsed.
#[derive(Clone, Copy, Debug)]
pub struct SearchInput<'a> {
    pub text: &'a str,
    pub text_match: TextMatch,
    pub match_case: bool,
    /// Name terms and regular expressions test the full path instead of
    /// the name.
    pub match_path: bool,
    /// More patterns, each tested as `regex:PATTERN` is, as `-r` adds them.
    pub regexes: &'a [String],
    /// The absolute folder the search is limited to (`-path`).
    pub scope: Option<&'a str>,
    /// The absolute folder whose direct entries alone match (`-parent`).
    /// Without a scope it is also the scope, so the daemon walks the folder
    /// instead of every volume and refuses a folder on no indexed volume.
    pub parent: Option<&'a str>,
    /// The absolute folder a relative path in the text is below; `None`
    /// refuses a relative path with [`QueryError::RelativePath`].
    pub working_dir: Option<&'a str>,
    /// What `dm:` values resolve against.
    pub query_start: &'a QueryStart,
}

/// What a search looks for, and where; [`Search::request`] adds what it
/// returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub query: ParsedQuery,
    pub scope: Option<String>,
}

impl Search {
    /// Parses `input`, or says why its text cannot run.
    pub fn new(input: &SearchInput) -> Result<Search, QueryError> {
        let (text, term_match, pattern) = match input.text_match {
            TextMatch::Terms(term_match) => (input.text, term_match, None),
            TextMatch::Regex => ("", TermMatch::Anywhere, Some(input.text)),
        };
        let mut query = parse_query_with(
            text,
            ParseOptions {
                match_case: input.match_case,
                match_path: input.match_path,
                term_match,
                working_dir: input.working_dir,
                query_start: Some(input.query_start),
            },
        )?;
        let patterns = pattern
            .into_iter()
            .chain(input.regexes.iter().map(String::as_str));
        query.clauses.extend(
            patterns
                .filter_map(|pattern| regex_test(pattern, input.match_path))
                .map(Clause::single),
        );
        let mut scope = input.scope.map(str::to_owned);
        if let Some(folder) = input.parent {
            query
                .clauses
                .push(Clause::single(Test::Parent(folder.to_owned())));
            scope.get_or_insert_with(|| folder.to_owned());
        }
        Ok(Search { query, scope })
    }

    /// The request that asks for `returns` of this search.
    pub fn request(self, returns: Returns) -> SearchRequest {
        SearchRequest {
            query: self.query,
            scope: self.scope,
            returns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read as terms, the pattern would split at its spaces and lose its
    /// `\\`, which is what makes it test the full path.
    #[test]
    fn a_regular_expression_text_is_one_pattern() {
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        let search = |text| {
            Search::new(&SearchInput {
                text,
                text_match: TextMatch::Regex,
                match_case: true,
                match_path: false,
                regexes: &[],
                scope: None,
                parent: None,
                working_dir: None,
                query_start: &start,
            })
            .expect("a pattern always parses")
            .query
        };
        let pattern = r"docs\\report .*\.pdf$";
        assert_eq!(
            search(pattern),
            ParsedQuery {
                clauses: vec![Clause::single(Test::PathRegex(pattern.into()))],
                match_case: true,
            }
        );
        assert_eq!(search("").clauses, Vec::new(), "an empty pattern finds all");
    }
}
