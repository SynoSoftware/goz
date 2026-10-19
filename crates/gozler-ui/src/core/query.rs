//! What the person asks for besides the typed text: the match controls, the
//! sort, and the search that the text and the match controls make.

use gozler_core::query::{QueryError, QueryStart, TermMatch};
use gozler_core::search::{Search, SearchInput, TextMatch};
use gozler_core::types::{SortDir, SortKey};
use serde::{Deserialize, Serialize};

/// The search that `text` and the match controls ask for. The window parses
/// with no working folder, so a relative path is the parser's
/// [`QueryError::RelativePath`], which asks for the full path.
pub(crate) fn search(
    text: &str,
    matching: Matching,
    query_start: &QueryStart,
) -> Result<Search, QueryError> {
    let text_match = match matching.mode {
        MatchMode::StartOfWords => TextMatch::Terms(TermMatch::WordStart),
        MatchMode::StartOfName => TextMatch::Terms(TermMatch::NameStart),
        MatchMode::Anywhere => TextMatch::Terms(TermMatch::Anywhere),
        MatchMode::WholeWords => TextMatch::Terms(TermMatch::WholeWords),
        MatchMode::RegularExpression => TextMatch::Regex,
    };
    Search::new(&SearchInput {
        text,
        text_match,
        match_case: matching.case,
        match_path: matching.path,
        regexes: &[],
        scope: None,
        parent: None,
        working_dir: None,
        query_start,
    })
}

/// How the typed name terms match; one choice, because the modes exclude
/// each other (spec: Search box). Its names are `ui.json`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchMode {
    #[default]
    StartOfWords,
    StartOfName,
    Anywhere,
    WholeWords,
    RegularExpression,
}

impl MatchMode {
    /// In the Match list's order.
    pub(crate) const ALL: [MatchMode; 5] = [
        MatchMode::StartOfWords,
        MatchMode::StartOfName,
        MatchMode::Anywhere,
        MatchMode::WholeWords,
        MatchMode::RegularExpression,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            MatchMode::StartOfWords => "match.start_of_words",
            MatchMode::StartOfName => "match.start_of_name",
            MatchMode::Anywhere => "match.anywhere",
            MatchMode::WholeWords => "match.whole_words",
            MatchMode::RegularExpression => "match.regular_expression",
        }
    }
}

/// The match controls beside the search box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Matching {
    pub(crate) mode: MatchMode,
    pub(crate) case: bool,
    pub(crate) path: bool,
}

/// The sort keys in View > Sort by's order.
pub(crate) const SORT_KEYS: [SortKey; 5] = [
    SortKey::Name,
    SortKey::Path,
    SortKey::Size,
    SortKey::DateModified,
    SortKey::Extension,
];

pub(crate) const SORT_DIRECTIONS: [SortDir; 2] = [SortDir::Asc, SortDir::Desc];

pub(crate) fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "sort.name",
        SortKey::Path => "sort.path",
        SortKey::Size => "sort.size",
        SortKey::DateModified => "sort.date_modified",
        SortKey::Extension => "sort.extension",
    }
}

pub(crate) fn sort_direction_label(direction: SortDir) -> &'static str {
    match direction {
        SortDir::Asc => "sort.ascending",
        SortDir::Desc => "sort.descending",
    }
}
