//! What the results list shows, and the chunk of rows asked for
//! (DESIGN.md: Rows on demand). The list keeps the shown rows
//! until the new search's first chunk arrives, so it does not empty on every
//! keystroke, and every chunk is asked for with the shown search, so it
//! names the rows the person sees even while the typed text has already
//! changed.

use super::fetch::{Chunk, ChunkAsk, DaemonFailure, Generation, Local, LocalService, SelectionAsk};
use gozler_client::Repair;
use gozler_core::proto::ResultItem;
use gozler_core::query::{MatchSums, MatchedParts, QueryError};
use gozler_core::search::Search;
use gozler_core::types::SortSpec;
use std::ops::Range;
use std::path::PathBuf;

/// The list reaches the first 100,000 rows of a search (DESIGN.md: Searching).
pub(crate) const LIST_LIMIT: u64 = 100_000;

/// The rows of one request. Every request scans every match before it
/// sorts, so a larger chunk costs the daemon little more and halves the
/// waits on a long scroll.
pub(crate) const CHUNK_ROWS: u64 = 16_384;

/// The most rows the cache keeps, in whole chunks, so memory stays the same
/// for any number of matches (DESIGN.md: Memory budget).
const CACHE_ROWS: u64 = 32_768;

/// What the list area shows. Rows and a message never show together.
pub(crate) enum Shown {
    /// Before the first reply.
    Nothing,
    /// The rows of a search; none when nothing matches.
    Rows(ShownRows),
    Message(ListMessage),
}

/// Why the list shows a message instead of rows; each has its own text and
/// buttons (DESIGN.md: States).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListMessage {
    Parse(QueryError),
    /// The Location folder does not exist, so the window does not search,
    /// as `gozler` refuses the same search (DESIGN.md: S8).
    MissingFolder(String),
    Failure(DaemonFailure),
    /// The search is empty, and Settings says to show nothing then (DESIGN.md:
    /// S6).
    TypeToSearch,
}

/// The button a message offers for its fix (DESIGN.md: Daemon failures).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fix {
    Repair(Repair),
    /// Points the service at the gozlerd.exe beside the window.
    UseLocal(LocalService),
    /// Downloads the latest release's setup over a service older than the
    /// window. A newer release's status text runs it too.
    Update,
    /// Opens Services, where the person sees why the service is not
    /// running when Windows did not tell gozler.
    OpenServices,
}

/// A button below the list's message (DESIGN.md: S5, S8, S9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListButton {
    Fix(Fix),
    ClearFilters,
    SearchEverywhere,
    ChooseFolder,
}

impl ListButton {
    /// The button's catalogue key; Clear filters and Choose folder… are
    /// the filter pane's buttons, with their captions.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ListButton::Fix(fix) => fix.label(),
            ListButton::ClearFilters => "command.clear_filters",
            ListButton::SearchEverywhere => "list.search_everywhere",
            ListButton::ChooseFolder => "filter.choose_folder",
        }
    }

    /// Whether the button runs its fix as administrator
    /// ([`Fix::needs_administrator`]).
    pub(crate) fn needs_administrator(&self) -> bool {
        matches!(self, ListButton::Fix(fix) if fix.needs_administrator())
    }
}

impl ListMessage {
    /// The message's fixes, in the order their buttons show.
    pub(crate) fn fixes(&self) -> Vec<Fix> {
        match self {
            ListMessage::Parse(_) | ListMessage::MissingFolder(_) | ListMessage::TypeToSearch => {
                Vec::new()
            }
            ListMessage::Failure(failure) => [Fix::use_local(failure), Fix::of(failure)]
                .into_iter()
                .flatten()
                .collect(),
        }
    }
}

impl Fix {
    /// The fix of `failure`, if the window has one.
    pub(crate) fn of(failure: &DaemonFailure) -> Option<Fix> {
        match failure {
            DaemonFailure::NotRunning(why) => {
                Some(why.repair().map_or(Fix::OpenServices, Fix::Repair))
            }
            DaemonFailure::TooOld { .. } => Some(Fix::Update),
            // The owner check is a security check, whose fix is not the
            // window's; the others end by themselves or by a new search.
            DaemonFailure::NotReady
            | DaemonFailure::NotGozler
            | DaemonFailure::Refused { .. }
            | DaemonFailure::Lost => None,
        }
    }

    /// The Use local fix of `failure`, which shows before its other fix:
    /// a service older than the window, with a gozlerd.exe beside the
    /// window that serves the window's protocol.
    pub(crate) fn use_local(failure: &DaemonFailure) -> Option<Fix> {
        match failure {
            DaemonFailure::TooOld { programs } => match &programs.local {
                Local::Offered(local) => Some(Fix::UseLocal(local.clone())),
                Local::NotOffered | Local::Mismatched => None,
            },
            _ => None,
        }
    }

    /// Whether the fix runs commands as administrator, so its button shows
    /// the administrator shield, as Windows' buttons that ask for
    /// permission do. The setup asks for permission itself.
    pub(crate) fn needs_administrator(&self) -> bool {
        match self {
            Fix::Repair(repair) => repair.commands().is_some(),
            Fix::UseLocal(_) => true,
            Fix::Update | Fix::OpenServices => false,
        }
    }

    /// The catalogue key of Update's caption while GitHub's latest release
    /// is older than the window.
    pub(crate) const DOWNGRADE_LABEL: &'static str = "fix.downgrade";

    /// The fix buttons that show together, each as its catalogue key and
    /// whether it shows the administrator shield: Use local beside Update
    /// or Downgrade, and every other fix alone. The list's message and the
    /// Service group show them, so each group's access keys differ, and the
    /// buttons are as wide as the widest of them from the start.
    pub(crate) fn label_groups() -> Vec<Vec<(&'static str, bool)>> {
        let label = |fix: &Fix| (fix.label(), fix.needs_administrator());
        let use_local = label(&Fix::UseLocal(LocalService {
            program: PathBuf::new(),
            version: String::new(),
            installed: None,
        }));
        let alone = [
            Fix::Repair(Repair::StartService),
            Fix::Repair(Repair::TurnOnService),
            Fix::Repair(Repair::InstallService {
                folder: PathBuf::new(),
            }),
            Fix::Repair(Repair::Download),
            Fix::OpenServices,
        ];
        alone
            .iter()
            .map(|fix| vec![label(fix)])
            .chain([
                vec![use_local, label(&Fix::Update)],
                vec![use_local, (Fix::DOWNGRADE_LABEL, false)],
            ])
            .collect()
    }

    /// The button's catalogue key.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Fix::Repair(repair) => match repair {
                Repair::StartService => "fix.start_service",
                Repair::TurnOnService => "fix.turn_on_service",
                Repair::InstallService { .. } => "fix.install_service",
                Repair::Download => "fix.download",
            },
            Fix::UseLocal(_) => "fix.use_local",
            Fix::Update => "fix.update",
            Fix::OpenServices => "fix.open_services",
        }
    }
}

pub(crate) struct ShownRows {
    /// The ask of the first chunk; a later chunk changes its start.
    ask: ChunkAsk,
    total: u64,
    /// As the latest chunk says, with `total`.
    sums: Option<MatchSums>,
    chunks: Vec<CachedChunk>,
    /// `None` when the query's terms do not compile, which the daemon
    /// refuses before any row is shown.
    matched: Option<MatchedParts>,
    /// The order of use, for dropping the chunk used least recently.
    clock: u64,
}

struct CachedChunk {
    start: u64,
    rows: Vec<ResultItem>,
    used: u64,
}

/// The request whose reply the results wait for; a reply of another
/// generation is stale.
enum InFlight {
    /// A new search, whose first chunk's ask its rows will be shown with.
    Search(Generation, ChunkAsk),
    Chunk(Generation),
}

/// What accepting a chunk changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowsChange {
    Stale,
    /// A new search's first rows replace the old ones; `count` is the
    /// list's new row count.
    NewSearch {
        count: u64,
    },
    /// Rows of the shown search arrived; the count changed when the total
    /// did.
    Rows {
        rows: Range<u64>,
        count: u64,
    },
}

pub(crate) struct Results {
    shown: Shown,
    in_flight: Option<InFlight>,
}

impl Results {
    pub(crate) fn new() -> Results {
        Results {
            shown: Shown::Nothing,
            in_flight: None,
        }
    }

    pub(crate) fn shown(&self) -> &Shown {
        &self.shown
    }

    /// Whether a search has started, so Refresh has one to run again.
    pub(crate) fn has_searched(&self) -> bool {
        !matches!(self.shown, Shown::Nothing) || self.in_flight.is_some()
    }

    /// Whether a new search waits for its first rows.
    pub(crate) fn searching(&self) -> bool {
        matches!(self.in_flight, Some(InFlight::Search(..)))
    }

    /// Every match of the shown search.
    pub(crate) fn total(&self) -> u64 {
        match &self.shown {
            Shown::Rows(rows) => rows.total,
            Shown::Nothing | Shown::Message(_) => 0,
        }
    }

    /// Every match's files and folders and their sizes, when the service
    /// sent them.
    pub(crate) fn sums(&self) -> Option<MatchSums> {
        match &self.shown {
            Shown::Rows(rows) => rows.sums,
            Shown::Nothing | Shown::Message(_) => None,
        }
    }

    /// The rows the list has: every match up to [`LIST_LIMIT`].
    pub(crate) fn count(&self) -> u64 {
        self.total().min(LIST_LIMIT)
    }

    /// The row at `index`, or `None` while its chunk has not arrived.
    pub(crate) fn row(&self, index: u64) -> Option<&ResultItem> {
        let Shown::Rows(shown) = &self.shown else {
            return None;
        };
        let chunk = shown
            .chunks
            .iter()
            .find(|c| (c.start..c.start + c.rows.len() as u64).contains(&index))?;
        chunk.rows.get(usize::try_from(index - chunk.start).ok()?)
    }

    /// The byte ranges of the row's full path that the shown search's text
    /// terms matched.
    pub(crate) fn matched_parts(&self, row: &ResultItem) -> Vec<Range<usize>> {
        match &self.shown {
            Shown::Rows(ShownRows {
                matched: Some(matched),
                ..
            }) => matched.ranges(&row.path),
            _ => Vec::new(),
        }
    }

    /// The shown search, for the index state its scope decides.
    pub(crate) fn shown_search(&self) -> Option<&Search> {
        self.shown_ask().map(|ask| &ask.search)
    }

    /// The shown search and its sort, which every request about the shown
    /// rows starts from.
    pub(crate) fn shown_ask(&self) -> Option<&ChunkAsk> {
        match &self.shown {
            Shown::Rows(rows) => Some(&rows.ask),
            Shown::Nothing | Shown::Message(_) => None,
        }
    }

    /// The rows at `positions`, if the cache holds every one of them.
    pub(crate) fn cached_rows(&self, positions: &[Range<u64>]) -> Option<Vec<ResultItem>> {
        positions
            .iter()
            .flat_map(Range::clone)
            .map(|position| self.row(position).cloned())
            .collect()
    }

    /// The request for the rows at `positions`, from the first to the last,
    /// widened to the cached chunks that it overlaps, so the reply replaces
    /// them whole (DESIGN.md: The selection). `None` while no rows are shown.
    pub(crate) fn selection_ask(&self, positions: Vec<Range<u64>>) -> Option<SelectionAsk> {
        let Shown::Rows(shown) = &self.shown else {
            return None;
        };
        let mut start = positions.first()?.start;
        let mut end = positions.last()?.end;
        let overlapped: Vec<Range<u64>> = shown
            .chunks
            .iter()
            .map(|c| c.start..c.start + c.rows.len() as u64)
            .filter(|chunk| chunk.start < end && start < chunk.end)
            .collect();
        for chunk in &overlapped {
            start = start.min(chunk.start);
            end = end.max(chunk.end);
        }
        let mut cached = overlapped;
        cached.sort_by_key(|chunk| chunk.start);
        Some(SelectionAsk {
            span: ChunkAsk {
                start,
                len: end - start,
                ..shown.ask.clone()
            },
            positions,
            cached,
        })
    }

    /// Gives each chunk of `cached` that the cache still holds the rows of
    /// `rows` inside it, which a selection reply read at one moment, and
    /// returns the positions it replaced. `cached` and `rows` are in
    /// position order.
    pub(crate) fn replace_cached(
        &mut self,
        cached: &[Range<u64>],
        rows: Vec<(u64, ResultItem)>,
    ) -> Range<u64> {
        let Shown::Rows(shown) = &mut self.shown else {
            return 0..0;
        };
        let mut rows = rows.into_iter().peekable();
        let mut replaced: Option<Range<u64>> = None;
        for range in cached {
            let inside: Vec<ResultItem> =
                std::iter::from_fn(|| rows.next_if(|(position, _)| *position < range.end))
                    .filter(|(position, _)| *position >= range.start)
                    .map(|(_, row)| row)
                    .collect();
            let Some(chunk) = shown.chunks.iter_mut().find(|c| c.start == range.start) else {
                continue;
            };
            chunk.rows = inside;
            let all = replaced.get_or_insert(range.clone());
            *all = all.start.min(range.start)..all.end.max(range.end);
        }
        replaced.unwrap_or(0..0)
    }

    /// A new search in the order `sort`, whose first chunk `generation`
    /// asks for with the returned ask; the shown rows stay until it arrives.
    pub(crate) fn start_search(
        &mut self,
        generation: Generation,
        search: Search,
        sort: SortSpec,
    ) -> ChunkAsk {
        let ask = ChunkAsk {
            search,
            sort,
            start: 0,
            len: CHUNK_ROWS,
        };
        self.in_flight = Some(InFlight::Search(generation, ask.clone()));
        ask
    }

    /// Replaces whatever the list shows with `message`, and drops the
    /// request in flight, whose reply would show rows again.
    pub(crate) fn show_message(&mut self, message: ListMessage) {
        self.shown = Shown::Message(message);
        self.in_flight = None;
    }

    /// Whether `generation` is the request the results wait for.
    pub(crate) fn awaits(&self, generation: Generation) -> bool {
        matches!(&self.in_flight, Some(InFlight::Search(g, _) | InFlight::Chunk(g)) if *g == generation)
    }

    /// The ask for the first chunk that the rows `visible` need and the
    /// cache lacks, which `generation` then asks for, and marks the cached
    /// chunks they show as used. `None` while a search or a chunk is in
    /// flight, so fast scrolling never queues a request for every chunk it
    /// passes.
    pub(crate) fn next_chunk(
        &mut self,
        visible: Range<u64>,
        generation: Generation,
    ) -> Option<ChunkAsk> {
        let count = self.count();
        let Shown::Rows(shown) = &mut self.shown else {
            return None;
        };
        let visible = visible.start..visible.end.min(count);
        shown.clock += 1;
        let mut missing = None;
        let mut start = visible.start - visible.start % CHUNK_ROWS;
        while start < visible.end {
            match shown.chunks.iter_mut().find(|c| c.start == start) {
                Some(chunk) => chunk.used = shown.clock,
                None => {
                    missing.get_or_insert(start);
                }
            }
            start += CHUNK_ROWS;
        }
        if self.in_flight.is_some() {
            return None;
        }
        let start = missing?;
        self.in_flight = Some(InFlight::Chunk(generation));
        Some(ChunkAsk {
            start,
            len: CHUNK_ROWS.min(LIST_LIMIT - start),
            ..shown.ask.clone()
        })
    }

    /// Accepts the rows of the request `generation` asked for.
    pub(crate) fn accept(&mut self, generation: Generation, chunk: Chunk) -> RowsChange {
        if !self.awaits(generation) {
            return RowsChange::Stale;
        }
        match self.in_flight.take() {
            Some(InFlight::Search(_, ask)) => {
                let matched = MatchedParts::new(&ask.search.query).ok();
                let mut shown = ShownRows {
                    ask,
                    total: chunk.total,
                    sums: chunk.sums,
                    chunks: Vec::new(),
                    matched,
                    clock: 0,
                };
                shown.insert(chunk.start, chunk.rows);
                self.shown = Shown::Rows(shown);
                RowsChange::NewSearch {
                    count: self.count(),
                }
            }
            Some(InFlight::Chunk(_)) => {
                let Shown::Rows(shown) = &mut self.shown else {
                    return RowsChange::Stale;
                };
                let rows = chunk.start..chunk.start + chunk.rows.len() as u64;
                shown.total = chunk.total;
                shown.sums = chunk.sums.or(shown.sums);
                shown.insert(chunk.start, chunk.rows);
                RowsChange::Rows {
                    rows,
                    count: self.count(),
                }
            }
            None => RowsChange::Stale,
        }
    }
}

impl ShownRows {
    /// Caches a chunk, and drops the chunk used least recently when the
    /// cache holds more than [`CACHE_ROWS`].
    fn insert(&mut self, start: u64, rows: Vec<ResultItem>) {
        self.clock += 1;
        self.chunks.retain(|c| c.start != start);
        if rows.is_empty() {
            return;
        }
        self.chunks.push(CachedChunk {
            start,
            rows,
            used: self.clock,
        });
        while self.chunks.len() as u64 * CHUNK_ROWS > CACHE_ROWS {
            let oldest = self
                .chunks
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| c.used)
                .map(|(i, _)| i)
                .expect("the cache holds the chunk just inserted");
            self.chunks.swap_remove(oldest);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gozler_core::query::parse_query;

    fn search() -> Search {
        Search {
            query: parse_query("").expect("the empty query parses"),
            scope: None,
        }
    }

    /// A chunk of `len` rows from `start`, of a search with `total` matches.
    pub(crate) fn chunk(start: u64, len: u64, total: u64) -> Chunk {
        Chunk {
            start,
            rows: (start..start + len)
                .map(|i| ResultItem {
                    path: format!(r"C:\row{i}"),
                    path_u16: None,
                    size: None,
                    mtime_ft: None,
                    is_dir: false,
                    attributes: None,
                })
                .collect(),
            total,
            volumes_incomplete: false,
            metadata_pending: false,
            sums: None,
        }
    }

    /// Results that show the first chunk of a search with `total` matches.
    fn showing(total: u64) -> Results {
        let mut results = Results::new();
        results.start_search(Generation(1), search(), SortSpec::default());
        let first = chunk(0, CHUNK_ROWS.min(total), total);
        results.accept(Generation(1), first);
        results
    }

    /// Fast scrolling would queue a request for every chunk it passes, and a
    /// chunk asked for during a new search would name the old search's rows.
    #[test]
    fn at_most_one_chunk_is_in_flight_and_none_while_a_search_is() {
        let mut results = showing(50_000);
        let far = 2 * CHUNK_ROWS + 5;
        let asked = results.next_chunk(far..far + 40, Generation(2));
        assert_eq!(asked.map(|a| a.start), Some(2 * CHUNK_ROWS));
        let near = CHUNK_ROWS..CHUNK_ROWS + 40;
        assert_eq!(results.next_chunk(near, Generation(3)), None);

        let mut results = showing(50_000);
        results.start_search(Generation(2), search(), SortSpec::default());
        assert_eq!(results.next_chunk(far..far + 40, Generation(3)), None);
    }

    /// The daemon's index changes between two chunks, so a later reply's
    /// total is the list's new count, which stops at the list's limit.
    #[test]
    fn a_changed_total_changes_the_count_which_stops_at_the_limit() {
        let mut results = showing(3 * CHUNK_ROWS);
        let asked = results.next_chunk(CHUNK_ROWS..CHUNK_ROWS + 40, Generation(2));
        let start = asked.expect("the rows are not cached").start;
        let accepted = results.accept(Generation(2), chunk(start, 1_000, 200_000));
        assert_eq!(
            accepted,
            RowsChange::Rows {
                rows: start..start + 1_000,
                count: LIST_LIMIT
            }
        );
        assert_eq!(showing(LIST_LIMIT + 1).count(), LIST_LIMIT);
    }

    /// Without the eviction the cache grows with every chunk scrolled to,
    /// and memory with it; dropping the chunk in view would empty the rows
    /// the person looks at.
    #[test]
    fn the_cache_drops_the_chunk_used_least_recently() {
        let mut results = showing(LIST_LIMIT);
        let cached = CACHE_ROWS / CHUNK_ROWS;
        for n in 1..=cached {
            let start = n * CHUNK_ROWS;
            // The first chunk stays in view while the others arrive.
            results.next_chunk(0..10, Generation(0));
            let asked = results.next_chunk(start..start + 10, Generation(n + 1));
            assert_eq!(asked.map(|a| a.start), Some(start));
            results.accept(Generation(n + 1), chunk(start, CHUNK_ROWS, LIST_LIMIT));
        }
        assert!(results.row(0).is_some(), "the chunk in view stays");
        assert!(
            results.row(CHUNK_ROWS).is_none(),
            "the chunk used least recently is dropped"
        );
        assert!(results.row(cached * CHUNK_ROWS).is_some());
    }
}
