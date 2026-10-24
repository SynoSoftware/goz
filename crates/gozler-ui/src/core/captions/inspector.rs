use super::too_many;
use crate::core::model::Model;
use crate::core::results::Shown;
use crate::core::selection::{FolderContents, Selection, Sums};
use crate::core::text::{self, Numbers, Strings};
use gozler_core::proto::ResultItem;

/// The labels of the inspector's fields: one entry's, in the pane's order,
/// then the several entries' Total size.
pub(super) const FIELD_LABELS: [&str; 5] = [
    "inspector.folder",
    "inspector.size",
    "inspector.date_modified",
    "inspector.attributes",
    "inspector.total_size",
];

/// The row of [`FIELD_LABELS`] that shows more than a text: the age after
/// Date modified.
pub(crate) const MODIFIED_ROW: usize = 2;

/// What the inspector shows for the selection (DESIGN.md: Inspector pane).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InspectorCaptions {
    pub(crate) heading: String,
    /// Each field's label and value, in the pane's order.
    pub(crate) fields: Vec<(String, String)>,
    pub(crate) inspected: Inspected,
}

/// What the inspector shows: the parts of the pane that only some
/// selections have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Inspected {
    /// No results show.
    #[default]
    Nothing,
    /// Results show and none is selected; the gray note under the heading
    /// says how to see an entry's details.
    NothingSelected { note: String },
    /// The selection is being read.
    Reading,
    /// One entry: its name and whether it is a folder, whose icon the
    /// heading shows, and its modification time, a FILETIME, whose [`age`]
    /// the window shows after it and keeps current.
    One {
        name: String,
        is_folder: bool,
        modified: Option<i64>,
    },
    /// Several entries, or more than the selection reads.
    Several,
}

impl InspectorCaptions {
    /// The gray hint under the heading.
    pub(crate) fn note(&self) -> Option<&str> {
        match &self.inspected {
            Inspected::NothingSelected { note } => Some(note),
            _ => None,
        }
    }

    /// The one selected entry's modification time.
    pub(crate) fn modified(&self) -> Option<i64> {
        match self.inspected {
            Inspected::One { modified, .. } => modified,
            _ => None,
        }
    }
}

impl InspectorCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> InspectorCaptions {
        let strings = model.strings();
        let label = |key: &'static str| strings.text(key).to_owned();
        match model.search_list().selection() {
            // The number of matches, and how to see an entry's details.
            Selection::None => match model.search_list().results().shown() {
                Shown::Rows(_) => InspectorCaptions {
                    heading: strings.count(
                        "status.results",
                        model.search_list().results().total(),
                        numbers,
                        &[],
                    ),
                    fields: Vec::new(),
                    inspected: Inspected::NothingSelected {
                        note: label("inspector.nothing_selected"),
                    },
                },
                Shown::Nothing | Shown::Message(_) => InspectorCaptions::default(),
            },
            Selection::TooMany(count) => InspectorCaptions {
                heading: too_many(strings, numbers, *count),
                fields: Vec::new(),
                inspected: Inspected::Several,
            },
            Selection::Reading { .. } | Selection::Filtering { .. } => {
                let reading = label("status.reading");
                InspectorCaptions {
                    heading: reading.clone(),
                    fields: vec![(label("inspector.total_size"), reading)],
                    inspected: Inspected::Reading,
                }
            }
            Selection::Files(files) => match files.as_ref() {
                [file] => entry(model, numbers, file, None),
                _ => several(strings, numbers, model.search_list().selection_sums()),
            },
            Selection::Folder { folder, contents } => entry(model, numbers, folder, Some(contents)),
        }
    }
}

/// One selected entry; `contents` is a folder's.
fn entry(
    model: &Model,
    numbers: &dyn Numbers,
    entry: &ResultItem,
    contents: Option<&FolderContents>,
) -> InspectorCaptions {
    let strings = model.strings();
    let unknown = || strings.text("cell.unknown").to_owned();
    let exact = |bytes: u64| exact_size(strings, numbers, bytes);
    let name = text::name_of(&entry.path);
    let size = match contents {
        Some(FolderContents::Calculating(_)) => strings.text("inspector.calculating").to_owned(),
        Some(FolderContents::Counted(totals)) => {
            let key = if totals.volumes_incomplete || totals.metadata_pending {
                "inspector.folder_size_at_least"
            } else {
                "inspector.folder_size"
            };
            strings.count(key, totals.count, numbers, &[&exact(totals.size)])
        }
        None => entry.size.map_or_else(unknown, exact),
    };
    let attributes = entry.attributes.map_or_else(unknown, |attributes| {
        let words: Vec<&str> = attributes
            .letters()
            .chars()
            .filter_map(attribute_word)
            .map(|key| strings.text(key))
            .collect();
        if words.is_empty() {
            strings.text("inspector.no_attributes").to_owned()
        } else {
            words.join(", ")
        }
    });
    let values = [
        gozler_core::winpath::parent_of(&entry.path),
        size,
        entry
            .mtime_ft
            .map_or_else(unknown, |ft| numbers.date_time_seconds(ft)),
        attributes,
    ];
    InspectorCaptions {
        heading: name.to_owned(),
        fields: FIELD_LABELS
            .iter()
            .zip(values)
            .map(|(key, value)| (strings.text(key).to_owned(), value))
            .collect(),
        inspected: Inspected::One {
            name: name.to_owned(),
            is_folder: entry.is_dir,
            modified: entry.mtime_ft,
        },
    }
}

/// How long ago the FILETIME `modified` was at `now`, or how long until it
/// comes, after a "·": in the largest whole unit, rounded down, or "just
/// now" under a minute (DESIGN.md: Inspector pane). A month is 30 days, so
/// 360 to 364 days read 11 months, under a year.
pub(crate) fn age(strings: &Strings, numbers: &dyn Numbers, modified: i64, now: i64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    let seconds = now.abs_diff(modified) / 10_000_000;
    let (ago, ahead, count) = match seconds {
        ..MINUTE => return strings.format("inspector.age", &[strings.text("age.just_now")]),
        MINUTE..HOUR => ("age.minutes_ago", "age.in_minutes", seconds / MINUTE),
        HOUR..DAY => ("age.hours_ago", "age.in_hours", seconds / HOUR),
        DAY..MONTH => ("age.days_ago", "age.in_days", seconds / DAY),
        MONTH..YEAR => ("age.months_ago", "age.in_months", (seconds / MONTH).min(11)),
        YEAR.. => ("age.years_ago", "age.in_years", seconds / YEAR),
    };
    let key = if modified > now { ahead } else { ago };
    strings.format("inspector.age", &[&strings.count(key, count, numbers, &[])])
}

/// Several selected entries: how many files and folders, and the size of
/// the files.
fn several(strings: &Strings, numbers: &dyn Numbers, sums: Sums) -> InspectorCaptions {
    let files = strings.count("inspector.files", sums.files, numbers, &[]);
    let folders = strings.count("inspector.folders", sums.folders, numbers, &[]);
    let heading = match (sums.files, sums.folders) {
        (_, 0) => files,
        (0, _) => folders,
        _ => strings.format("inspector.files_and_folders", &[&files, &folders]),
    };
    let total = if sums.files == 0 {
        strings.text("inspector.no_files").to_owned()
    } else {
        let size = exact_size(strings, numbers, sums.bytes);
        if sums.unknown == 0 {
            size
        } else {
            strings.count("inspector.unknown", sums.unknown, numbers, &[&size])
        }
    };
    InspectorCaptions {
        heading,
        fields: vec![(strings.text("inspector.total_size").to_owned(), total)],
        inspected: Inspected::Several,
    }
}

/// The inspector includes exact bytes after the compact size, as
/// "1 KB (1,234 bytes)", or only once when both read "16 bytes".
fn exact_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64) -> String {
    let size = text::bytes(strings, numbers, bytes);
    let exact = strings.count("size.bytes", bytes, numbers, &[]);
    if size == exact {
        size
    } else {
        strings.format("inspector.size_exact", &[&size, &exact])
    }
}

/// The catalogue key of an attribute letter's word, as `attrib:` names the
/// letters. D has none, because the icon already shows a folder.
fn attribute_word(letter: char) -> Option<&'static str> {
    Some(match letter {
        'R' => "attribute.r",
        'H' => "attribute.h",
        'S' => "attribute.s",
        'A' => "attribute.a",
        'V' => "attribute.v",
        'X' => "attribute.x",
        'N' => "attribute.n",
        'T' => "attribute.t",
        'L' => "attribute.l",
        'C' => "attribute.c",
        'O' => "attribute.o",
        'I' => "attribute.i",
        'E' => "attribute.e",
        'U' => "attribute.u",
        'P' => "attribute.p",
        'M' => "attribute.m",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::attribute_word;
    use crate::core::text::{LANGUAGES, Strings};

    /// An attribute letter without a word is left out of the inspector, and
    /// no compiler sees a letter that `gozler-core` adds to its table.
    #[test]
    fn every_attribute_letter_but_d_has_its_word() {
        let strings = Strings::load(&LANGUAGES[0]);
        for letter in gozler_core::attributes::Attributes::all_letters().chars() {
            let key = attribute_word(letter);
            assert!(
                letter == 'D' || key.is_some_and(|key| strings.text(key) != key),
                "{letter} has no word in en.json"
            );
        }
    }
}
