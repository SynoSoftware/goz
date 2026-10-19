//! What the person asks for besides the typed text: the match controls and
//! the sort.

use gozler_core::types::{SortDir, SortKey, SortSpec};

/// How the typed name terms match; one choice, because the modes exclude
/// each other (spec: Search box).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// The sort a first start shows: by name, ascending.
pub(crate) fn default_sort() -> SortSpec {
    SortSpec::default_for(SortKey::Name)
}

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
