//! The requests the model asks for and the replies it accepts, in the core's
//! terms, so the model's handling of replies and failures is tested without
//! a daemon (DESIGN.md: Requests). `app::requests` runs each
//! [`Fetch`] on its own thread and answers with a [`Fetched`].

use gozler_client::NotRunning;
use gozler_core::include::IncludeSettings;
use gozler_core::output::ResultFormat;
use gozler_core::proto::{
    DaemonStatus, PROTO_VERSION, QueryTotals, ResultItem, Returns, Rows, SearchRequest,
};
use gozler_core::query::MatchSums;
use gozler_core::search::Search;
use gozler_core::types::SortSpec;
use std::io;
use std::ops::Range;
use std::path::PathBuf;

/// What a request is for. At most one request of each purpose is live, so
/// starting one cancels the live one of its purpose and no other (DESIGN.md:
/// window rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The list's rows: a new search's first chunk, or a chunk of the shown
    /// search.
    Results,
    /// The selected rows that the cache lacks, or the contents of the one
    /// selected folder.
    Selection,
    Status,
    Export,
    /// Settings' search for a program, which the Use Total Commander link
    /// sends.
    Program,
    /// Settings > Include's preview count.
    IncludeCount,
    /// Telling the service that the Include settings are saved; apart from
    /// the count, so a new count never cancels it.
    SettingsSaved,
}

impl Purpose {
    pub(crate) const ALL: [Purpose; 7] = [
        Purpose::Results,
        Purpose::Selection,
        Purpose::Status,
        Purpose::Export,
        Purpose::Program,
        Purpose::IncludeCount,
        Purpose::SettingsSaved,
    ];
}

/// Which request a reply answers; a newer request has a larger number, and
/// the model drops a reply that is not the newest of its purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Generation(pub(crate) u64);

/// A request the model wants sent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fetch {
    pub(crate) generation: Generation,
    pub(crate) ask: Ask,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    /// One chunk of rows, gathered from all its pages.
    Rows(ChunkAsk),
    Selection(SelectionAsk),
    /// The entries below the one selected folder and the summed size of its
    /// files: the totals of an empty search scoped to it, as `gozler
    /// -get-folder-size` asks.
    FolderSize(Search),
    Export(ExportAsk),
    Status,
    /// The rows of a search for a program, answered with
    /// [`Outcome::Rows`].
    Program(ChunkAsk),
    /// How many entries the index would keep under the settings.
    CountIncluded(IncludeSettings),
    /// The Include settings are saved, so the service applies them.
    SettingsSaved,
}

/// The rows of `search` in the order `sort` from the position `start`, at
/// most `len` of them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChunkAsk {
    pub(crate) search: Search,
    pub(crate) sort: SortSpec,
    pub(crate) start: u64,
    pub(crate) len: u64,
}

impl ChunkAsk {
    /// The request that asks the service for the chunk.
    pub(crate) fn request(&self) -> SearchRequest {
        self.search.clone().request(Returns::Rows(Rows {
            sort: self.sort,
            offset: u32::try_from(self.start).expect("a chunk starts below LIST_LIMIT"),
            limit: Some(u32::try_from(self.len).expect("a chunk fits in u32")),
            want_size: true,
            want_mtime: true,
            // Four bytes a row, so showing the Attributes column needs no
            // second request.
            want_attributes: true,
            // The status bar shows them beside the total. The service sums
            // every match for them, so only the first chunk asks, and the
            // later chunks keep its sums.
            want_sums: self.start == 0,
        }))
    }
}

/// The selected rows: one request for `span`, from the first selected
/// position to the last, widened to the cached chunks it overlaps. The
/// thread keeps only the rows at `positions` and inside `cached`, so a
/// Ctrl+click on row 0 and on row 99,999 carries two files and at most the
/// cache, not 100,000 rows (DESIGN.md: The selection).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectionAsk {
    pub(crate) span: ChunkAsk,
    /// Sorted and apart, as the list gives them.
    pub(crate) positions: Vec<Range<u64>>,
    /// The cached chunks inside `span`, sorted.
    pub(crate) cached: Vec<Range<u64>>,
}

impl SelectionAsk {
    /// Whether the row at `position` is one the reply carries.
    pub(crate) fn keeps(&self, position: u64) -> bool {
        inside(&self.positions, position) || inside(&self.cached, position)
    }
}

/// Whether `position` is in one of `ranges`, which are sorted and apart.
pub(crate) fn inside(ranges: &[Range<u64>], position: u64) -> bool {
    let after = ranges.partition_point(|r| r.end <= position);
    ranges.get(after).is_some_and(|r| r.contains(&position))
}

/// Every match of `search` in the order `sort`, written to `path` as
/// `format` says, page by page as `gozler -export-csv` writes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExportAsk {
    pub(crate) search: Search,
    pub(crate) sort: SortSpec,
    pub(crate) path: PathBuf,
    pub(crate) format: ResultFormat,
    /// Whether the file has an Attributes column, as `gozler` asks only
    /// then.
    pub(crate) want_attributes: bool,
}

impl ExportAsk {
    pub(crate) fn request(&self) -> SearchRequest {
        self.search.clone().request(Returns::Rows(Rows {
            sort: self.sort,
            offset: 0,
            limit: None,
            want_size: true,
            want_mtime: true,
            want_attributes: self.want_attributes,
            want_sums: false,
        }))
    }
}

impl Fetch {
    pub(crate) fn purpose(&self) -> Purpose {
        match self.ask {
            Ask::Rows(_) => Purpose::Results,
            Ask::Selection(_) | Ask::FolderSize(_) => Purpose::Selection,
            Ask::Export(_) => Purpose::Export,
            Ask::Status => Purpose::Status,
            Ask::Program(_) => Purpose::Program,
            Ask::CountIncluded(_) => Purpose::IncludeCount,
            Ask::SettingsSaved => Purpose::SettingsSaved,
        }
    }
}

/// A reply to a [`Fetch`]. An export sends several: its progress, then its
/// end.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fetched {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    Rows(Chunk),
    Selection(SelectionRows),
    /// The reply to [`Ask::FolderSize`].
    Totals(QueryTotals),
    /// The reply to [`Ask::CountIncluded`].
    IncludedCount(u64),
    /// The reply to [`Ask::SettingsSaved`].
    SettingsApplied,
    /// The service's status, and the gozlerd.exe beside the window when it
    /// is a different build from the service's, which Use local offers.
    Status(DaemonStatus, Option<LocalService>),
    /// The rows an export has written so far.
    ExportProgress {
        rows: u64,
    },
    Exported {
        rows: u64,
    },
    /// Creating or writing the export's file failed; the partial file is
    /// deleted.
    ExportFailed(WriteFailure),
    Failed(DaemonFailure),
}

/// The rows a [`SelectionAsk`] keeps, read at one moment.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectionRows {
    /// By position, in position order.
    pub(crate) rows: Vec<(u64, ResultItem)>,
    /// The ask's [`SelectionAsk::cached`], whose chunks these rows replace.
    pub(crate) cached: Vec<Range<u64>>,
}

/// Why a file could not be written, as Windows said it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WriteFailure {
    pub(crate) kind: io::ErrorKind,
    /// The Windows error code, when Windows gave one.
    pub(crate) os_code: Option<i32>,
}

impl From<&io::Error> for WriteFailure {
    fn from(error: &io::Error) -> WriteFailure {
        WriteFailure {
            kind: error.kind(),
            os_code: error.raw_os_error(),
        }
    }
}

/// The rows of one chunk, from the position `start`, and what every page of
/// it said.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Chunk {
    pub(crate) start: u64,
    pub(crate) rows: Vec<ResultItem>,
    /// Every match of the search, not only the rows of the chunk.
    pub(crate) total: u64,
    /// A volume in scope is not complete, as a page says.
    pub(crate) volumes_incomplete: bool,
    /// Some sizes and dates in scope are unknown, as a page says.
    pub(crate) metadata_pending: bool,
    /// Every match's files and folders and their sizes; `None` from a
    /// service that does not send them.
    pub(crate) sums: Option<MatchSums>,
}

/// The failures the window shows differently (DESIGN.md: Daemon failures);
/// `app::requests` maps the client's error to them. A pipe that cannot be
/// opened or read shows as [`DaemonFailure::Lost`], because both mean the
/// pipe failed after the service was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonFailure {
    NotRunning(NotRunning),
    /// The service runs but its pipe is not open yet.
    NotReady,
    /// The pipe's server is not the elevated gozler service.
    NotGozler,
    /// The service is older than the window. `local` is the gozlerd.exe
    /// beside the window, when it serves the window's protocol.
    TooOld {
        local: Option<LocalService>,
    },
    /// The service refused the request; `detail` is its English text.
    Refused {
        detail: String,
    },
    Lost,
}

/// A gozlerd.exe that serves this window's protocol and is a different build
/// from the service's, which Use local points the service at (DESIGN.md:
/// Service fixes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalService {
    pub(crate) program: PathBuf,
    /// Its version, as gozlerd's `protocol` command prints it.
    pub(crate) version: String,
    /// The version of the program the service runs, such as `0.3.0`, when
    /// it could be read.
    pub(crate) installed: Option<String>,
}

impl LocalService {
    /// The gozlerd.exe at `program`, from what its hidden `protocol`
    /// command printed: the oldest and newest protocol versions it serves,
    /// and its version. `None` when it does not serve this window's
    /// protocol, or printed something else.
    pub(crate) fn from_protocol(
        program: PathBuf,
        printed: &str,
        installed: Option<String>,
    ) -> Option<LocalService> {
        // The version may hold a space, before a local build's time.
        let mut words = printed.trim().splitn(3, ' ');
        let oldest: u16 = words.next()?.parse().ok()?;
        let newest: u16 = words.next()?.parse().ok()?;
        let version = words.next()?.to_owned();
        (oldest..=newest)
            .contains(&PROTO_VERSION)
            .then_some(LocalService {
                program,
                version,
                installed,
            })
    }
}

impl DaemonFailure {
    /// Whether the failure ends by itself, so the window asks for status
    /// until the service answers. An older service ends when the setup
    /// replaces it; a refused request and a failed owner check fail the same way
    /// again.
    pub(crate) fn recovers(&self) -> bool {
        match self {
            DaemonFailure::NotRunning(_)
            | DaemonFailure::NotReady
            | DaemonFailure::TooOld { .. }
            | DaemonFailure::Lost => true,
            DaemonFailure::NotGozler | DaemonFailure::Refused { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Use local over a service that refuses the window again would stop
    /// searching for nothing, so only a gozlerd.exe whose range holds the
    /// window's protocol is offered.
    #[test]
    fn only_a_gozlerd_that_serves_the_window_is_local() {
        let local = |printed: &str| {
            LocalService::from_protocol(PathBuf::from("gozlerd.exe"), printed, None)
        };
        assert_eq!(
            local(&format!("1 {PROTO_VERSION} 0.5.0\r\n")),
            Some(LocalService {
                program: PathBuf::from("gozlerd.exe"),
                version: "0.5.0".into(),
                installed: None,
            })
        );
        assert_eq!(local(&format!("1 {} 0.4.0", PROTO_VERSION - 1)), None);
        assert_eq!(local(&format!("{} 99 0.9.0", PROTO_VERSION + 1)), None);
    }
}
