//! The window's facts: the loaded language, the saved settings, and what the
//! controls show. Captions are built from these facts when they are shown,
//! never stored (design rule 12).

use super::layout::{Dip, Panes};
use super::query::{Matching, default_sort};
use super::settings::Settings;
use super::text::Strings;
use gozler_core::types::SortSpec;

/// The results list reaches the first 100,000 rows of a search (spec:
/// Searching).
pub(crate) const LIST_LIMIT: u64 = 100_000;

pub(crate) struct Model {
    pub(crate) strings: Strings,
    pub(crate) settings: Settings,
    pub(crate) matching: Matching,
    pub(crate) sort: SortSpec,
    /// The columns the list shows, in their order.
    pub(crate) columns: Vec<Column>,
    pub(crate) panes: Panes,
}

impl Model {
    pub(crate) fn new(strings: Strings, settings: Settings) -> Model {
        Model {
            strings,
            settings,
            matching: Matching::default(),
            sort: default_sort(),
            columns: Column::ALL
                .into_iter()
                .filter(|c| c.shown_by_default())
                .collect(),
            panes: Panes::default(),
        }
    }
}

/// A column of the results list (spec: Results list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
    Name,
    Path,
    Size,
    DateModified,
    Extension,
    Attributes,
}

impl Column {
    /// In View > Columns' order.
    pub(crate) const ALL: [Column; 6] = [
        Column::Name,
        Column::Path,
        Column::Size,
        Column::DateModified,
        Column::Extension,
        Column::Attributes,
    ];

    pub(crate) fn header(self) -> &'static str {
        match self {
            Column::Name => "header.name",
            Column::Path => "header.path",
            Column::Size => "header.size",
            Column::DateModified => "header.date_modified",
            Column::Extension => "header.extension",
            Column::Attributes => "header.attributes",
        }
    }

    /// Its item in View > Columns.
    pub(crate) fn menu_label(self) -> &'static str {
        match self {
            Column::Name => "column.name",
            Column::Path => "column.path",
            Column::Size => "column.size",
            Column::DateModified => "column.date_modified",
            Column::Extension => "column.extension",
            Column::Attributes => "column.attributes",
        }
    }

    pub(crate) fn default_width(self) -> Dip {
        Dip(match self {
            Column::Name => 320,
            Column::Path => 380,
            Column::Size => 90,
            Column::DateModified => 130,
            Column::Extension | Column::Attributes => 80,
        })
    }

    fn shown_by_default(self) -> bool {
        !matches!(self, Column::Extension | Column::Attributes)
    }

    /// Sizes are right-aligned, so their digits line up.
    pub(crate) fn right_aligned(self) -> bool {
        self == Column::Size
    }
}
