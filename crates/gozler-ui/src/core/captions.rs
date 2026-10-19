//! Every caption of the main window, built from the model's facts and its
//! strings when it is shown (design rule 12). [`WindowCaptions::new`] calls
//! every caption function: `app` sets its result at creation and after a
//! change to the regional format, and `layout::CaptionWidths` measures the
//! same value. A handler that updates one caption calls the same function.
//!
//! A catalogue key is written whole, as a literal, so the test below finds
//! every key the code uses in `en.json`.

use super::commands::{self, MenuNode};
use super::fetch::DaemonFailure;
use super::layout::WIDEST_TOTAL;
use super::model::{Column, Export, Model};
use super::query::MatchMode;
use super::results::{LIST_LIMIT, ListMessage, Shown};
use super::selection::{Selection, sums};
use super::text::{self, Numbers, Strings};
use gozler_client::NotRunning;
use gozler_core::query::{Malformed, QueryError};
use gozler_core::types::VolumePhase;

/// The main window's captions, in the language and regional format of the
/// moment they are built.
pub(crate) struct WindowCaptions {
    pub(crate) title: String,
    pub(crate) menu_bar: Vec<MenuNode>,
    pub(crate) search_cue: String,
    pub(crate) match_label: String,
    /// In [`MatchMode::ALL`]'s order.
    pub(crate) match_modes: [String; 5],
    pub(crate) match_case: String,
    pub(crate) match_path: String,
    /// In [`Model::columns`]' order.
    pub(crate) headers: Vec<String>,
    pub(crate) status: StatusCaptions,
    pub(crate) message: Option<ListCaption>,
    /// The texts each fixed status part can show at its widest, which the
    /// layout sizes the part for, so a part never moves with its text
    /// (spec: Look).
    pub(crate) status_widest: [Vec<String>; 3],
}

/// The status bar's four parts (spec: Status bar).
pub(crate) struct StatusCaptions {
    pub(crate) results: String,
    pub(crate) selection: String,
    pub(crate) export: String,
    pub(crate) index: String,
}

/// What the list's empty area says instead of rows: the message, and the
/// label of its fix's button.
pub(crate) type ListCaption = (String, Option<String>);

impl WindowCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> WindowCaptions {
        let strings = model.strings();
        WindowCaptions {
            title: title(model),
            menu_bar: commands::render(&commands::menu_bar(), strings),
            search_cue: strings.text("search.cue").to_owned(),
            match_label: strings.text("match.label").to_owned(),
            match_modes: MatchMode::ALL.map(|m| strings.text(m.label()).to_owned()),
            match_case: strings.text("match.case").to_owned(),
            match_path: strings.text("match.path").to_owned(),
            headers: model
                .columns()
                .iter()
                .map(|c| header(strings, *c))
                .collect(),
            status: StatusCaptions::new(model, numbers),
            message: list_message(model),
            status_widest: status_widest(strings, numbers),
        }
    }
}

impl StatusCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> StatusCaptions {
        StatusCaptions {
            results: results_status(model, numbers),
            selection: selection_status(model, numbers),
            export: export_status(model, numbers),
            index: index_status(model),
        }
    }

    pub(crate) fn parts(&self) -> [&str; 4] {
        [&self.results, &self.selection, &self.export, &self.index]
    }
}

fn status_widest(strings: &Strings, numbers: &dyn Numbers) -> [Vec<String>; 3] {
    let limit = numbers.integer(LIST_LIMIT);
    let showing = strings.format("status.showing", &[&limit, &numbers.integer(WIDEST_TOTAL)]);
    // Which size is widest depends on the font and the language, so every
    // unit's widest forms are measured: 1,023 bytes, then 9.99, 99.9 and 999
    // of each unit.
    let sizes = std::iter::once(1023).chain(
        (1..=4)
            .flat_map(|unit| [9.99, 99.9, 999.0].map(|value| (value * 1024f64.powi(unit)) as u64)),
    );
    let mut selected: Vec<String> = sizes
        .map(|n| {
            strings.format(
                "status.selected",
                &[&limit, &text::bytes(strings, numbers, n)],
            )
        })
        .collect();
    selected.push(strings.text("status.reading").to_owned());
    [
        vec![
            strings.format("status.filtered", &[&showing]),
            strings.text("status.searching").to_owned(),
        ],
        selected,
        vec![
            strings.count("status.exporting", WIDEST_TOTAL, numbers, &[]),
            strings.count("status.exported", WIDEST_TOTAL, numbers, &["results.csv"]),
        ],
    ]
}

/// The window's title: the typed search and the product's name, as Windows
/// titles a document window, or the name alone while the search is empty.
pub(crate) fn title(model: &Model) -> String {
    let strings = model.strings();
    let text = model.text.trim();
    if text.is_empty() {
        strings.text("window.title").to_owned()
    } else {
        strings.format("window.title_search", &[text])
    }
}

/// A column's header.
pub(crate) fn header(strings: &Strings, column: Column) -> String {
    strings.text(column.header()).to_owned()
}

/// Status part 1: how many results, or that a search runs.
pub(crate) fn results_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    let results = model.results();
    if results.searching() {
        return strings.text("status.searching").to_owned();
    }
    match results.shown() {
        Shown::Rows(_) => {
            let total = results.total();
            if total > LIST_LIMIT {
                strings.format(
                    "status.showing",
                    &[&numbers.integer(LIST_LIMIT), &numbers.integer(total)],
                )
            } else {
                strings.count("status.results", total, numbers, &[])
            }
        }
        Shown::Nothing | Shown::Message(_) => String::new(),
    }
}

/// Status part 2: how many entries are selected and the size of the
/// selected files, or that the selection is read.
pub(crate) fn selection_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.selection() {
        Selection::None => String::new(),
        Selection::Reading { .. } => strings.text("status.reading").to_owned(),
        Selection::Files(files) => {
            let (count, bytes) = sums(files);
            strings.format(
                "status.selected",
                &[
                    &numbers.integer(count),
                    &text::bytes(strings, numbers, bytes),
                ],
            )
        }
    }
}

/// Status part 3: the export's progress, or the last export's result.
pub(crate) fn export_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.export() {
        Export::Idle => String::new(),
        Export::Running { rows, .. } => strings.count("status.exporting", *rows, numbers, &[]),
        Export::Done { path, rows } => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            strings.count("status.exported", *rows, numbers, &[&name])
        }
    }
}

/// Status part 4: the index state, only when it affects the answer (spec:
/// Status bar).
pub(crate) fn index_status(model: &Model) -> String {
    let strings = model.strings();
    let coverage = model.coverage();
    if let Some(volume) = coverage.incomplete() {
        let key = match volume.phase {
            VolumePhase::Live | VolumePhase::Snapshot => {
                unreachable!("an incomplete volume is neither live nor a snapshot")
            }
            VolumePhase::Bootstrapping | VolumePhase::Rescanning => "status.volume_indexing",
            VolumePhase::Offline => "status.volume_offline",
            VolumePhase::Failed { .. } => "status.volume_failed",
        };
        return strings.format(key, &[volume.mounted().display_prefix()]);
    }
    coverage
        .sizes_or_dates_unknown()
        .map(|volume| strings.format("status.metadata", &[volume.mounted().display_prefix()]))
        .unwrap_or_default()
}

/// What the list's empty area says instead of rows, if anything.
pub(crate) fn list_message(model: &Model) -> Option<ListCaption> {
    let strings = model.strings();
    let message = match model.results().shown() {
        Shown::Rows(_) if model.results().total() == 0 => {
            return Some((strings.text("list.no_results").to_owned(), None));
        }
        Shown::Nothing | Shown::Rows(_) => return None,
        Shown::Message(message) => message,
    };
    let text = match message {
        ListMessage::Parse(error) => query_error(strings, error),
        ListMessage::Failure(failure) => failure_text(strings, failure),
    };
    let button = message
        .fix()
        .map(|fix| strings.text(fix.label()).to_owned());
    Some((text, button))
}

/// What a failure of the service means for the person (spec: Daemon
/// failures).
pub(crate) fn failure_text(strings: &Strings, failure: &DaemonFailure) -> String {
    match failure {
        DaemonFailure::NotRunning(why) => match why {
            NotRunning::Stopped => strings.text("failure.stopped").to_owned(),
            NotRunning::Disabled => strings.text("failure.disabled").to_owned(),
            NotRunning::NotInstalled {
                gozlerd_beside: true,
                ..
            } => strings.text("failure.not_installed").to_owned(),
            NotRunning::NotInstalled {
                gozlerd_beside: false,
                ..
            } => strings.text("failure.missing").to_owned(),
            NotRunning::Unknown(detail) => strings.format("failure.unknown", &[detail]),
        },
        DaemonFailure::NotReady => strings.text("failure.not_ready").to_owned(),
        DaemonFailure::NotGozler => strings.text("failure.not_gozler").to_owned(),
        DaemonFailure::TooOld => strings.text("failure.too_old").to_owned(),
        DaemonFailure::Refused { detail } => strings.format("failure.refused", &[detail]),
        DaemonFailure::Lost => strings.text("failure.lost").to_owned(),
    }
}

/// What is wrong with a search that does not parse, and how to fix it, in
/// the person's language; text from a library stays English after it.
fn query_error(strings: &Strings, error: &QueryError) -> String {
    let as_typed = |function: &str, value: &str| format!("{function}{value}");
    match error {
        QueryError::UnsupportedOperator(c) => {
            strings.format("query.unsupported_operator", &[&c.to_string()])
        }
        QueryError::UnknownFunction(name) => {
            strings.format("query.unknown_function", &[&format!("{name}:")])
        }
        QueryError::RelativePath(path) => strings.format("query.relative_path", &[path]),
        QueryError::NoQueryStart => strings.text("query.no_query_start").to_owned(),
        QueryError::TooManyTerms(n) => strings.format("query.too_many_terms", &[&n.to_string()]),
        QueryError::TooManyRegexes(n) => {
            strings.format("query.too_many_regexes", &[&n.to_string()])
        }
        QueryError::Malformed(reason) => match reason {
            Malformed::MisplacedNot => strings.text("query.misplaced_not").to_owned(),
            Malformed::NotWithoutTerm => strings.text("query.not_without_term").to_owned(),
            Malformed::OrWithoutTerms => strings.text("query.or_without_terms").to_owned(),
            Malformed::OperatorOnNothing(term) => {
                strings.format("query.operator_on_nothing", &[term])
            }
            Malformed::Size(value) => strings.format("query.size", &[&as_typed("size:", value)]),
            Malformed::ReversedDates(value) => {
                strings.format("query.reversed_dates", &[&as_typed("dm:", value)])
            }
            Malformed::DateOutOfRange(value) => {
                strings.format("query.date_out_of_range", &[&as_typed("dm:", value)])
            }
            Malformed::Days(value) => strings.format("query.days", &[&as_typed("dm:", value)]),
            Malformed::Date(value) => strings.format("query.date", &[&as_typed("dm:", value)]),
            Malformed::NoParentFolder => strings.text("query.no_parent_folder").to_owned(),
            Malformed::AttributeLetter(c) => {
                strings.format("query.attribute_letter", &[&c.to_string()])
            }
            Malformed::Regex(detail) => strings.format("query.regex", &[detail]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::text::{LANGUAGES, parse_catalogue};
    use std::collections::HashSet;
    use std::path::Path;

    /// A key that `en.json` lacks shows its dotted name on screen, and only
    /// the code that builds that caption shows it, so a typo in a rare
    /// message reaches a person before anyone sees it.
    #[test]
    fn every_catalogue_key_in_the_code_is_in_english() {
        let entries = parse_catalogue(LANGUAGES[0].catalogue).expect("en.json parses");
        let keys: HashSet<String> = entries
            .iter()
            .flat_map(|(key, _)| {
                let base = key
                    .strip_suffix("_one")
                    .or_else(|| key.strip_suffix("_other"));
                [Some(key.clone()), base.map(str::to_owned)]
            })
            .flatten()
            .collect();
        let groups: HashSet<&str> = keys
            .iter()
            .filter_map(|key| key.split_once('.').map(|(group, _)| group))
            .collect();
        let mut missing = Vec::new();
        let mut folders = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("src is readable") {
                let path = entry.expect("src is readable").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let code = std::fs::read_to_string(&path).expect("the source is UTF-8");
                // Every second piece between quotes is a string literal, or
                // close enough: a key has no quote, space or backslash.
                for literal in code.split('"').skip(1).step_by(2) {
                    let Some((group, name)) = literal.split_once('.') else {
                        continue;
                    };
                    let is_key = |part: &str| {
                        !part.is_empty()
                            && part.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                    };
                    if groups.contains(group)
                        && is_key(group)
                        && is_key(name)
                        && !keys.contains(literal)
                    {
                        missing.push(format!("{}: {literal}", path.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "en.json lacks: {missing:#?}");
    }
}
