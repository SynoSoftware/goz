use super::search::query_error;
use super::{failure_text, joined, size_with_unknown, too_many};
use crate::core::commands::{self, Key, Keystroke};
use crate::core::dates::Dates;
use crate::core::model::{Column, Model};
use crate::core::panel::{
    Counts, LookIn, Panel, PanelColumn, PanelSearch, Row, Side, View, relative_folder,
};
use crate::core::results::{ListMessage, Shown};
use crate::core::selection::Selection;
use crate::core::text::{self, Numbers, SizeFormat, Strings};

/// What a panel shows besides its rows (DESIGN.md: A panel).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PanelCaptions {
    pub(crate) drive: String,
    pub(crate) path: String,
    /// The status line's counts, or the quick search box's text while it
    /// is open.
    pub(crate) counts: String,
    pub(crate) quick: bool,
    /// The free space of the panel's drive, at the status line's right end.
    pub(crate) free: String,
    /// A search panel's focused result's full folder, at the status line's
    /// right end in place of the free space while it searches all drives.
    pub(crate) folder: String,
}

impl PanelCaptions {
    pub(crate) fn new(model: &Model, side: Side, numbers: &dyn Numbers) -> PanelCaptions {
        let strings = model.strings();
        let panel = model.panels().panel(side);
        let quick = panel.quick_text();
        let (counts, free) = Self::status(model, side, numbers);
        let counts = quick.map_or(counts, |typed| strings.format("panel.find", &[typed]));
        let all_drives = panel
            .search()
            .is_some_and(|search| search.look_in == LookIn::AllDrives);
        let focused = panel
            .focused()
            .and_then(|row| model.panels().row(side, row));
        let folder = match focused {
            Some(Row::Result(item)) if all_drives => {
                let path =
                    text::path_part(Column::Path, &item.path).expect("Path is a part of the path");
                item.path[path].to_owned()
            }
            _ => String::new(),
        };
        PanelCaptions {
            drive: panel.drive(),
            path: panel.path_box().to_owned(),
            counts,
            quick: quick.is_some(),
            free,
            folder,
        }
    }

    pub(super) fn status(model: &Model, side: Side, numbers: &dyn Numbers) -> (String, String) {
        let strings = model.strings();
        let panel = model.panels().panel(side);
        let counts = match panel.view() {
            View::Folder => panel_counts(strings, numbers, panel.counts()),
            View::Search(search) => search_counts(strings, numbers, panel, search),
            View::Reading | View::Cancelled | View::NotReady { .. } => String::new(),
        };
        let all_drives = panel
            .search()
            .is_some_and(|search| search.look_in == LookIn::AllDrives);
        let free = match (panel.view(), panel.free()) {
            (View::Folder | View::Search(_), Some(free)) if !all_drives => {
                strings.format("panel.free", &[&text::bytes(strings, numbers, free)])
            }
            _ => String::new(),
        };
        (counts, free)
    }
}

pub(crate) fn panel_message(model: &Model, side: Side) -> (Option<String>, bool) {
    let strings = model.strings();
    match model.panels().panel(side).view() {
        View::Reading => (Some(strings.text("panel.reading").to_owned()), false),
        View::Cancelled => (Some(strings.text("work.cancelled").to_owned()), true),
        View::Folder => (None, false),
        View::NotReady { drive } => (Some(strings.format("panel.not_ready", &[drive])), true),
        View::Search(_) => (panel_list_message(model, side), false),
    }
}

/// A search panel's status line: "Searching…" once its search has run long
/// enough to be named (DESIGN.md: Searching) until its first answer, then
/// its total and where it searched, also "0 results in …" beside the
/// list's no-results message, or how many of them are selected and their
/// size once read; nothing while the list shows another message
/// (DESIGN.md: Search in a panel). The count is gozlerd's total, as Search's
/// is, also past the rows the list reaches.
fn search_counts(
    strings: &Strings,
    numbers: &dyn Numbers,
    panel: &Panel,
    search: &PanelSearch,
) -> String {
    let results = search.list.results();
    if results.search_named() {
        return strings.text("status.searching").to_owned();
    }
    let total = results.total();
    if !matches!(results.shown(), Shown::Rows(_)) {
        return String::new();
    }
    let selected =
        |count: u64| strings.count("panel.selected", count, numbers, &[&numbers.integer(total)]);
    let nothing_selected = match search.list.selection() {
        Selection::None => !panel.has_selection(),
        _ => search.list.chosen_by_focus(),
    };
    if nothing_selected {
        return match search.look_in {
            LookIn::ThisFolder => {
                strings.count("panel.results_in", total, numbers, &[panel.folder()])
            }
            LookIn::AllDrives => strings.count("panel.results_everywhere", total, numbers, &[]),
        };
    }
    match search.list.selection() {
        Selection::None | Selection::Reading { .. } | Selection::Filtering { .. } => {
            selected(panel.selected_rows())
        }
        Selection::TooMany(count) => too_many(strings, numbers, *count),
        Selection::Files(_) | Selection::Folder { .. } => {
            let files = search.list.selection().files().unwrap_or_default();
            let size = (search.list.chosen_size())
                .map(|size| counted_size(strings, numbers, size.known, size.unknown))
                .unwrap_or_default();
            joined(strings, selected(files.len() as u64), size)
        }
    }
}

/// What a search panel's list area says instead of rows, if anything: why
/// it shows no results and how to search on (DESIGN.md: Search in a panel).
/// Retry never shows in a search panel.
pub(crate) fn panel_list_message(model: &Model, side: Side) -> Option<String> {
    let strings = model.strings();
    let panel = model.panels().panel(side);
    let search = panel.search()?;
    let results = search.list.results();
    let this_folder = search.look_in == LookIn::ThisFolder;
    Some(match results.shown() {
        Shown::Rows(_) if results.total() == 0 => {
            let none = strings.text("list.no_results");
            if this_folder {
                format!("{none}\n{}", strings.text("panel.try_all_drives"))
            } else {
                none.to_owned()
            }
        }
        // Only Search's list has a Location folder that can be missing.
        Shown::Nothing | Shown::Rows(_) | Shown::Message(ListMessage::MissingFolder(_)) => {
            return None;
        }
        Shown::Message(ListMessage::Parse(error)) => query_error(strings, error),
        Shown::Message(ListMessage::Failure(failure)) => failure_text(strings, failure),
        Shown::Message(ListMessage::TypeToSearch) if this_folder => {
            strings.format("panel.type_here", &[panel.folder()])
        }
        Shown::Message(ListMessage::TypeToSearch) => {
            strings.text("panel.type_everywhere").to_owned()
        }
        Shown::Message(ListMessage::NotIndexed { root }) => {
            strings.format("panel.search_not_indexed", &[root])
        }
    })
}

/// The status line's counts: "14 items · 5.2 GB" or "3 of 14 selected ·
/// 4.2 GB". A panel's files always have their size, so only folders are
/// counted as unknown.
fn panel_counts(strings: &Strings, numbers: &dyn Numbers, counts: Counts) -> String {
    let count = match counts.selected {
        None => strings.count("panel.items", counts.items, numbers, &[]),
        Some(selected) => strings.count(
            "panel.selected",
            selected,
            numbers,
            &[&numbers.integer(counts.items)],
        ),
    };
    let counted = counts.selected.unwrap_or(counts.items);
    let size = (counts.size)
        .map(|size| {
            let known = (size.unknown < counted).then_some(size.bytes);
            counted_size(strings, numbers, known, size.unknown)
        })
        .unwrap_or_default();
    joined(strings, count, size)
}

/// The size of a panel's counted entries, `known` the sum of the known
/// sizes and `unknown` how many have none: "4.2 GB", "(3.4 GB, 12
/// unknown)", or "(12 unknown)" when no size is known.
fn counted_size(
    strings: &Strings,
    numbers: &dyn Numbers,
    known: Option<u64>,
    unknown: u64,
) -> String {
    match known {
        Some(bytes) if unknown == 0 => text::bytes(strings, numbers, bytes),
        None if unknown == 0 => String::new(),
        _ => {
            let size = size_with_unknown(strings, numbers, known, unknown);
            strings.format("panel.size_with_unknown", &[&size])
        }
    }
}

/// A cell of `panel`'s `row` (DESIGN.md: A panel), with sizes in the format
/// `sizes`, as Search's Size column shows them. A search panel's result shows
/// as Search's results list shows it, and its Folder cell the folder below the
/// searched one, or the full folder while it searches all drives.
pub(crate) fn panel_cell(
    strings: &Strings,
    numbers: &dyn Numbers,
    sizes: SizeFormat,
    dates: &Dates,
    panel: &Panel,
    row: Row,
    column: PanelColumn,
) -> text::Text {
    let entry = match row {
        Row::Up if column == PanelColumn::Name => return "..".to_owned().into(),
        Row::Up => return text::Text::default(),
        Row::Result(item) => {
            let this_folder =
                panel.search().map(|search| search.look_in) == Some(LookIn::ThisFolder);
            if column == PanelColumn::Folder && this_folder {
                return relative_folder(panel.folder(), &item.path)
                    .to_owned()
                    .into();
            }
            return text::cell(strings, numbers, sizes, dates, column.search_column(), item);
        }
        Row::Entry(entry) => entry,
    };
    match column {
        PanelColumn::Name => entry.display_name(),
        PanelColumn::Ext => entry.extension(),
        PanelColumn::Folder => String::new(),
        PanelColumn::Size => match (entry.is_folder(), entry.folder_size()) {
            (true, None) => strings.text("panel.folder").to_owned(),
            (true, Some(size)) | (false, Some(size)) => sizes.text(strings, numbers, size),
            (false, None) => sizes.text(strings, numbers, entry.size),
        },
        PanelColumn::DateModified => return dates.text(strings, numbers, entry.modified),
    }
    .into()
}

/// A Look in list's item.
pub(crate) fn look_in_item(strings: &Strings, look_in: LookIn) -> String {
    let key = match look_in {
        LookIn::ThisFolder => "panel.this_folder",
        LookIn::AllDrives => "panel.all_drives",
    };
    strings.text(key).to_owned()
}

/// The headers of a panel's `columns`, in their order.
pub(crate) fn panel_headers(strings: &Strings, columns: &[PanelColumn]) -> Vec<text::Text> {
    (columns.iter())
        .map(|column| panel_header(strings, *column))
        .collect()
}

pub(crate) fn panel_header(strings: &Strings, column: PanelColumn) -> text::Text {
    match column {
        PanelColumn::Ext => strings.text("panel.ext").to_owned().into(),
        PanelColumn::Folder => strings.text("header.folder").to_owned().into(),
        _ => super::header(strings, column.search_column()),
    }
}

/// The key bar's buttons: each key and its word, and the tooltip where the
/// word alone cannot say what it does (DESIGN.md: The key bar).
pub(crate) fn key_bar(strings: &Strings, alt: bool) -> [(String, String, Option<String>); 6] {
    if alt {
        return std::array::from_fn(|index| {
            if let Some(item) = commands::key_bar_item(index, true) {
                (
                    Keystroke::alt(Key::Function(
                        u8::try_from(index + 3).expect("the key bar has six buttons"),
                    ))
                    .text(strings),
                    text::without_access_key(&item.label(strings)),
                    None,
                )
            } else {
                (String::new(), String::new(), None)
            }
        });
    }
    let tip = |key: &str| Some(strings.text(key).to_owned());
    let word = |key: &str| strings.text(key).to_owned();
    [
        ("F3".to_owned(), word("key_bar.view"), None),
        (
            "F4".to_owned(),
            word("key_bar.edit"),
            tip("key_bar.edit_tooltip"),
        ),
        ("F5".to_owned(), word("key_bar.copy"), None),
        (
            "F6".to_owned(),
            word("key_bar.move"),
            tip("key_bar.move_tooltip"),
        ),
        ("F7".to_owned(), word("key_bar.new_folder"), None),
        ("F8".to_owned(), word("key_bar.delete"), None),
    ]
}

#[cfg(test)]
mod tests {
    use super::panel_counts;
    use crate::core::panel::{Counts, Totals};
    use crate::core::text::tests::{English, english};

    /// A panel's status line gives the known part of its size and says how
    /// many sizes are unknown, never only "unknown" (DESIGN.md: A panel).
    #[test]
    fn a_panel_status_names_its_unknown_sizes() {
        let strings = english();
        let numbers = English;
        let status = |items, bytes, unknown| {
            let size = Some(Totals { bytes, unknown });
            let counts = Counts {
                items,
                selected: None,
                size,
            };
            panel_counts(&strings, &numbers, counts)
        };
        assert_eq!(status(20, 2 << 30, 12), "20 items · (2.00 GB, 12 unknown)");
        assert_eq!(status(12, 0, 12), "12 items · (12 unknown)");
        assert_eq!(status(20, 2 << 30, 0), "20 items · 2.00 GB");
    }
}
