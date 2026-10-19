//! The window's facts: the loaded language, the saved settings, what the
//! controls ask for, what the list shows and what the service said last.
//! Captions are built from these facts when they are shown, never stored
//! (design rule 12). Every reply passes through [`Model::accept`], which says
//! what changed and which request that makes necessary.

use super::fetch::{Ask, DaemonFailure, Fetch, Fetched, Generation, Outcome, Purpose};
use super::layout::{Dip, Panes};
use super::query::{self, Matching};
use super::results::{ListMessage, Results, RowsChange, Shown};
use super::settings::Settings;
use super::text::Strings;
use gozler_core::proto::DaemonStatus;
use gozler_core::query::QueryStart;
use gozler_core::scope::{Coverage, leaves_matches_out};
use gozler_core::types::{SortDir, SortKey, SortSpec, VolumePhase};
use serde::{Deserialize, Serialize};
use std::ops::Range;

pub(crate) struct Model {
    pub(crate) strings: Strings,
    pub(crate) settings: Settings,
    pub(crate) matching: Matching,
    pub(crate) sort: SortSpec,
    /// The columns the list shows, in the list's column order: Name first,
    /// because only the first column carries the icon.
    pub(crate) columns: Vec<Column>,
    pub(crate) panes: Panes,
    /// The typed search text.
    pub(crate) text: String,
    pub(crate) results: Results,
    /// The latest status reply.
    pub(crate) status: Option<DaemonStatus>,
    /// How the latest request of any purpose failed; `None` once the
    /// service answers.
    failure: Option<DaemonFailure>,
    /// The status request in flight.
    status_asked: Option<Generation>,
    last_generation: u64,
}

/// What a reply or an input changed, and what it makes necessary.
#[derive(Debug, PartialEq)]
pub(crate) struct Accepted {
    pub(crate) change: Change,
    pub(crate) then: Option<Then>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Then {
    Fetch(Fetch),
    /// The search again, with the clock of the moment it starts: the
    /// service answers again, or every volume in scope became complete.
    SearchAgain,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Stale,
    /// A new search runs; the list keeps its rows.
    Searching,
    /// A new search's first rows replace the old ones.
    NewSearch {
        count: u64,
    },
    /// Rows of the shown search arrived, and the count is `count`.
    Rows {
        rows: Range<u64>,
        count: u64,
    },
    /// The list is empty and shows a message.
    Message,
    /// The service's state changed, which the index part of the status bar
    /// and the status poll follow.
    Status,
}

impl Accepted {
    fn only(change: Change) -> Accepted {
        Accepted { change, then: None }
    }
}

impl Model {
    pub(crate) fn new(strings: Strings, settings: Settings) -> Model {
        let window = &settings.window;
        let mut columns = vec![Column::Name];
        let saved: Vec<Column> = window.columns.iter().map(|c| c.column).collect();
        let shown = if saved.is_empty() {
            Column::ALL
                .into_iter()
                .filter(|c| c.shown_by_default())
                .collect()
        } else {
            saved
        };
        for column in shown {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        Model {
            strings,
            matching: window.matching,
            sort: window.sort,
            columns,
            panes: Panes::default(),
            settings,
            text: String::new(),
            results: Results::new(),
            status: None,
            failure: None,
            status_asked: None,
            last_generation: 0,
        }
    }

    fn generation(&mut self) -> Generation {
        self.last_generation += 1;
        Generation(self.last_generation)
    }

    /// The search that the typed text and the controls ask for, or the
    /// parser's message when the text does not parse, which starts no
    /// request. The shown rows stay until the new search's first rows
    /// arrive.
    pub(crate) fn start_search(&mut self, query_start: &QueryStart) -> Accepted {
        match query::search(&self.text, self.matching, query_start) {
            Err(error) => {
                self.results.show_message(ListMessage::Parse(error));
                Accepted::only(Change::Message)
            }
            Ok(search) => {
                let generation = self.generation();
                let ask = self.results.start_search(generation, search, self.sort);
                Accepted {
                    change: Change::Searching,
                    then: Some(Then::Fetch(Fetch {
                        generation,
                        ask: Ask::Rows(ask),
                    })),
                }
            }
        }
    }

    /// The chunk that the visible rows still lack, if no request for rows
    /// is in flight.
    pub(crate) fn rows_needed(&mut self, visible: Range<u64>) -> Option<Fetch> {
        let generation = self.generation();
        let ask = self.results.next_chunk(visible, generation)?;
        Some(Fetch {
            generation,
            ask: Ask::Rows(ask),
        })
    }

    /// The status request, unless one is in flight.
    pub(crate) fn status_needed(&mut self) -> Option<Fetch> {
        if self.status_asked.is_some() {
            return None;
        }
        let generation = self.generation();
        self.status_asked = Some(generation);
        Some(Fetch {
            generation,
            ask: Ask::Status,
        })
    }

    /// Whether the window must ask for status every 2 seconds: the service
    /// is not running, not ready or lost, or a volume in scope is indexing.
    /// A refused request, a failed owner check, an older service and an
    /// offline or failed volume end only by the person's action, so they
    /// start no poll.
    pub(crate) fn polls_status(&self) -> bool {
        self.failure.as_ref().is_some_and(DaemonFailure::recovers) || self.indexing_in_scope()
    }

    /// What the latest status says about the volumes the shown search
    /// reaches.
    pub(crate) fn coverage(&self) -> Coverage<'_> {
        let volumes = self.status.as_ref().map_or(&[][..], |s| &s.volumes);
        let scope = self
            .results
            .shown_search()
            .and_then(|search| search.scope.as_deref());
        Coverage::of(volumes, scope, self.reads_size_or_date())
    }

    fn reads_size_or_date(&self) -> bool {
        self.results
            .shown_search()
            .is_some_and(|search| search.query.tests_size_or_date())
    }

    /// Whether a volume the shown search reaches completes by itself once
    /// it is read.
    fn indexing_in_scope(&self) -> bool {
        self.coverage().volumes().any(|v| {
            matches!(
                v.phase,
                VolumePhase::Bootstrapping | VolumePhase::Rescanning
            )
        })
    }

    /// Applies a reply; a reply of an older generation changes nothing.
    pub(crate) fn accept(&mut self, fetched: Fetched) -> Accepted {
        match (fetched.purpose, fetched.outcome) {
            (Purpose::Results, _) if !self.results.awaits(fetched.generation) => {
                Accepted::only(Change::Stale)
            }
            (Purpose::Status, _) if self.status_asked != Some(fetched.generation) => {
                Accepted::only(Change::Stale)
            }
            (Purpose::Results, Outcome::Rows(chunk)) => {
                self.failure = None;
                let flags = (chunk.volumes_incomplete, chunk.metadata_pending);
                let change = match self.results.accept(fetched.generation, chunk) {
                    RowsChange::Stale => Change::Stale,
                    RowsChange::NewSearch { count } => Change::NewSearch { count },
                    RowsChange::Rows { rows, count } => Change::Rows { rows, count },
                };
                // A page's flags cover every volume in scope, and the status
                // names which. The scope is the shown search's, so it is read
                // after the rows are accepted.
                let coverage = self.coverage();
                let unexplained = flags.0 && coverage.incomplete().is_none()
                    || leaves_matches_out(flags.1, self.reads_size_or_date())
                        && coverage.sizes_or_dates_unknown().is_none();
                let then = if unexplained {
                    self.status_needed().map(Then::Fetch)
                } else {
                    None
                };
                Accepted { change, then }
            }
            (Purpose::Status, Outcome::Status(status)) => {
                self.status_asked = None;
                let answers_again = self.failure.as_ref().is_some_and(DaemonFailure::recovers);
                let was_indexing = self.indexing_in_scope();
                self.failure = None;
                self.status = Some(status);
                let completed = was_indexing && !self.indexing_in_scope();
                let search_again =
                    answers_again || completed && self.results.shown_search().is_some();
                Accepted {
                    change: Change::Status,
                    then: search_again.then_some(Then::SearchAgain),
                }
            }
            (Purpose::Status, Outcome::Failed(failure)) => {
                self.status_asked = None;
                // A failure the list shows follows the service, so its
                // message and button never offer the fix for a state the
                // service has left. A search in flight shows its own reply.
                let shows_another = matches!(
                    self.results.shown(),
                    Shown::Message(ListMessage::Failure(shown)) if *shown != failure
                );
                let change = if shows_another && !self.results.searching() {
                    self.results
                        .show_message(ListMessage::Failure(failure.clone()));
                    Change::Message
                } else {
                    Change::Status
                };
                self.failure = Some(failure);
                Accepted::only(change)
            }
            (Purpose::Results, Outcome::Failed(failure)) => {
                self.failure = Some(failure.clone());
                self.results.show_message(ListMessage::Failure(failure));
                Accepted::only(Change::Message)
            }
            (Purpose::Results, Outcome::Status(_)) | (Purpose::Status, Outcome::Rows(_)) => {
                unreachable!("a request's reply has its purpose's outcome")
            }
        }
    }

    /// The list's display order of [`Model::columns`], as indexes into it:
    /// the saved columns in their saved order, which the person sets by
    /// dragging a header, then any other shown column.
    pub(crate) fn column_order(&self) -> Vec<usize> {
        let saved = &self.settings.window.columns;
        let mut order: Vec<usize> = Vec::new();
        for index in saved
            .iter()
            .filter_map(|s| self.columns.iter().position(|c| *c == s.column))
            .chain(0..self.columns.len())
        {
            if !order.contains(&index) {
                order.push(index);
            }
        }
        order
    }

    /// The sort a click on `column`'s header asks for: the column's key,
    /// reversed on a second click, and largest or newest first on the first
    /// click on Size or Date modified, as gozler's sort keys do.
    pub(crate) fn header_sort(&self, column: Column) -> Option<SortSpec> {
        let key = column.sort_key()?;
        Some(if self.sort.key == key {
            SortSpec {
                key,
                dir: match self.sort.dir {
                    SortDir::Asc => SortDir::Desc,
                    SortDir::Desc => SortDir::Asc,
                },
            }
        } else {
            SortSpec::default_for(key)
        })
    }
}

/// A column of the results list (spec: Results list). Its names are
/// `ui.json`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

    /// What a click on its header sorts by; Attributes sorts by nothing,
    /// because gozler has no attribute sort.
    pub(crate) fn sort_key(self) -> Option<SortKey> {
        match self {
            Column::Name => Some(SortKey::Name),
            Column::Path => Some(SortKey::Path),
            Column::Size => Some(SortKey::Size),
            Column::DateModified => Some(SortKey::DateModified),
            Column::Extension => Some(SortKey::Extension),
            Column::Attributes => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::results::CHUNK_ROWS;
    use crate::core::results::tests::chunk;
    use crate::core::text::tests::{English, english};
    use crate::core::text::{self, list_message};
    use gozler_client::NotRunning;
    use gozler_core::proto::{DaemonStatus, VolumeStatus};

    fn model() -> Model {
        Model::new(english(), Settings::default())
    }

    /// Types `text` and starts its search; returns the first chunk's fetch.
    fn search(model: &mut Model, text: &str) -> Fetch {
        model.text = text.to_owned();
        let start = QueryStart {
            now: "2026-03-04T12:00:00Z".parse().expect("an instant"),
            time_zone: jiff::tz::TimeZone::UTC,
        };
        match model.start_search(&start) {
            Accepted {
                change: Change::Searching,
                then: Some(Then::Fetch(fetch)),
            } => fetch,
            other => panic!("the search starts a request: {other:?}"),
        }
    }

    /// The reply to `fetch`: its first rows of a search with `total` matches.
    fn rows(fetch: &Fetch, total: u64) -> Fetched {
        let start = match &fetch.ask {
            Ask::Rows(ask) => ask.start,
            Ask::Status => panic!("a rows fetch"),
        };
        Fetched {
            purpose: Purpose::Results,
            generation: fetch.generation,
            outcome: Outcome::Rows(chunk(start, CHUNK_ROWS.min(total - start), total)),
        }
    }

    fn failed(fetch: &Fetch, failure: DaemonFailure) -> Fetched {
        Fetched {
            purpose: fetch.purpose(),
            generation: fetch.generation,
            outcome: Outcome::Failed(failure),
        }
    }

    /// A status reply with one volume, C:\, in `phase`.
    fn status(fetch: &Fetch, phase: VolumePhase) -> Fetched {
        Fetched {
            purpose: Purpose::Status,
            generation: fetch.generation,
            outcome: Outcome::Status(DaemonStatus {
                volumes: vec![VolumeStatus {
                    guid: r"\\?\Volume{c}\".into(),
                    mounts: vec![r"C:\".into()],
                    phase,
                    entries: 0,
                    generation: 0,
                    metadata_pending: false,
                    journal_max_bytes: None,
                    placeholders_created: 0,
                    delete_of_unknown: 0,
                    stale_slots: 0,
                    link_reconciles_dropped: 0,
                    memory: None,
                }],
                volumes_incomplete: false,
                process_working_set: 0,
                process_private_bytes: 0,
            }),
        }
    }

    /// A model that shows the first rows of a search with `total` matches.
    fn showing(total: u64) -> Model {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(rows(&fetch, total));
        model
    }

    /// Emptying the list on every keystroke makes the rows flicker, and a
    /// reply to a search the person has typed past would show rows of text
    /// that is no longer in the box.
    #[test]
    fn t1_a_new_search_keeps_the_rows_and_drops_the_older_search() {
        let mut model = showing(10);
        let older = search(&mut model, "repo");
        assert_eq!(model.results.count(), 10, "the previous rows stay");
        assert_eq!(text::results_status(&model, &English), "Searching…");
        let _newest = search(&mut model, "report f");
        assert_eq!(model.accept(rows(&older, 3)).change, Change::Stale);
        assert_eq!(model.results.count(), 10);
    }

    #[test]
    fn t2_the_new_search_first_rows_replace_the_old_ones() {
        let mut model = showing(10);
        let fetch = search(&mut model, "repo");
        assert_eq!(
            model.accept(rows(&fetch, 3)).change,
            Change::NewSearch { count: 3 }
        );
        assert_eq!(text::results_status(&model, &English), "3 results");
    }

    #[test]
    fn t3_a_failure_while_rows_are_shown_removes_them() {
        let mut model = showing(50_000);
        let chunk = model
            .rows_needed(CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        assert_eq!(
            model.accept(failed(&chunk, DaemonFailure::Lost)).change,
            Change::Message
        );
        assert_eq!(model.results.count(), 0);
        let strings = english();
        assert_eq!(
            list_message(&model),
            Some((strings.text("failure.lost").to_owned(), None))
        );
    }

    /// Without it, the person must search again by hand after the service
    /// starts or a volume finishes indexing (spec: J8, J9).
    #[test]
    fn t4_the_search_runs_again_when_the_service_answers_or_the_index_completes() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(failed(
            &fetch,
            DaemonFailure::NotRunning(NotRunning::Stopped),
        ));
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)).then,
            Some(Then::SearchAgain)
        );

        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Bootstrapping)).then,
            None
        );
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)).then,
            Some(Then::SearchAgain)
        );
        assert!(!model.polls_status(), "nothing needs the poll any more");
    }

    #[test]
    fn t5_a_volume_that_starts_indexing_changes_part_4_and_keeps_the_rows() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        assert_eq!(text::index_status(&model), "");
        assert!(!model.polls_status());

        let ask = model.status_needed().expect("no status request runs");
        let accepted = model.accept(status(&ask, VolumePhase::Rescanning));
        assert_eq!(accepted, Accepted::only(Change::Status));
        assert_eq!(model.results.count(), 10);
        // S13.
        assert_eq!(
            text::index_status(&model),
            r"C:\ is indexing — results may be incomplete."
        );
        assert!(model.polls_status());
    }

    /// A results page carries only combined flags, so part 4 would never
    /// name a volume that became incomplete after the last status reply.
    #[test]
    fn a_page_flag_that_the_status_does_not_explain_asks_for_status() {
        let mut model = showing(50_000);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        let fetch = model
            .rows_needed(CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        let mut reply = rows(&fetch, 50_000);
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.volumes_incomplete = true;
        }
        assert!(matches!(
            model.accept(reply).then,
            Some(Then::Fetch(Fetch {
                ask: Ask::Status,
                ..
            }))
        ));
    }

    /// t1 shows the same for the results.
    #[test]
    fn a_status_reply_of_an_older_generation_changes_nothing() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        let older = Fetch {
            generation: Generation(ask.generation.0 - 1),
            ask: Ask::Status,
        };
        assert_eq!(
            model.accept(status(&older, VolumePhase::Rescanning)).change,
            Change::Stale
        );
        assert_eq!(model.status, None);
    }

    /// A status failure that emptied the list would hide rows that are
    /// still right; a results failure must replace them (design: Requests).
    #[test]
    fn a_status_failure_leaves_the_rows_and_a_results_failure_replaces_them() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(failed(&ask, DaemonFailure::Lost)).change,
            Change::Status
        );
        assert_eq!(model.results.count(), 10);
        assert_eq!(list_message(&model), None);

        let fetch = search(&mut model, "repo");
        let refused = DaemonFailure::Refused {
            detail: "regex parse error".into(),
        };
        assert_eq!(
            model.accept(failed(&fetch, refused)).change,
            Change::Message
        );
        assert_eq!(model.results.count(), 0);
    }

    /// The list would otherwise keep offering Start the service for a
    /// service that has since been turned off, and that command fails.
    #[test]
    fn a_status_failure_with_another_reason_replaces_the_shown_failure() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(failed(
            &fetch,
            DaemonFailure::NotRunning(NotRunning::Stopped),
        ));
        let ask = model.status_needed().expect("no status request runs");
        let disabled = DaemonFailure::NotRunning(NotRunning::Disabled);
        assert_eq!(model.accept(failed(&ask, disabled)).change, Change::Message);
        let strings = english();
        assert_eq!(
            list_message(&model),
            Some((
                strings.text("failure.disabled").to_owned(),
                Some(strings.text("fix.turn_on_service").to_owned())
            ))
        );
    }

    /// S7: the parser's message, and no request, so no poll either.
    #[test]
    fn a_search_that_does_not_parse_starts_no_request() {
        let mut model = model();
        model.text = "dz:x".into();
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(model.start_search(&start), Accepted::only(Change::Message));
        let strings = english();
        assert_eq!(
            list_message(&model),
            Some((strings.format("query.unknown_function", &["dz:"]), None))
        );
        assert!(!model.polls_status());
    }

    /// A failure that ends by itself is polled until the service answers; a
    /// poll after one that the same request repeats would ask forever.
    #[test]
    fn the_status_poll_runs_only_while_waiting_can_change_the_answer() {
        for (failure, polls) in [
            (DaemonFailure::NotRunning(NotRunning::Stopped), true),
            (DaemonFailure::NotReady, true),
            (DaemonFailure::Lost, true),
            (
                DaemonFailure::Refused {
                    detail: String::new(),
                },
                false,
            ),
        ] {
            let mut model = model();
            let fetch = search(&mut model, "report");
            model.accept(failed(&fetch, failure.clone()));
            assert_eq!(model.polls_status(), polls, "{failure:?}");
        }
        for (phase, polls) in [
            (VolumePhase::Bootstrapping, true),
            (VolumePhase::Rescanning, true),
            (VolumePhase::Offline, false),
        ] {
            let mut model = showing(10);
            let ask = model.status_needed().expect("no status request runs");
            model.accept(status(&ask, phase.clone()));
            assert_eq!(model.polls_status(), polls, "{phase:?}");
        }
    }
}
