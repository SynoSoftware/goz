//! The requests the model asks for and the replies it accepts, in the core's
//! terms, so the model's handling of replies and failures is tested without
//! a daemon (DESIGN.md: Requests). `app::requests` runs each
//! [`Fetch`] on its own thread and answers with a [`Fetched`].

use super::panel::Side;
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

/// A list that shows gozlerd's results: Search's results list, or a
/// search panel's list (file-manager-spec.md: Search in a panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Search,
    Panel(Side),
}

impl List {
    pub(crate) const ALL: [List; 3] = [
        List::Search,
        List::Panel(Side::Left),
        List::Panel(Side::Right),
    ];
}

/// What a request is for. At most one request of each purpose is live, so
/// starting one cancels the live one of its purpose and no other (DESIGN.md:
/// window rule 4). Each list has its own `Results` and `Selection`, so a
/// search in one list never cancels another list's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The list's rows: a new search's first chunk, or a chunk of the shown
    /// search.
    Results(List),
    /// The selected rows that the cache lacks, or the contents of the one
    /// selected folder.
    Selection(List),
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
    /// A panel's folder sizes, one purpose for each panel, so one panel's
    /// request never cancels the other's (file-manager-spec.md: Folder
    /// sizes).
    FolderSizes(Side),
}

impl Purpose {
    pub(crate) const ALL: [Purpose; 13] = [
        Purpose::Results(List::Search),
        Purpose::Results(List::Panel(Side::Left)),
        Purpose::Results(List::Panel(Side::Right)),
        Purpose::Selection(List::Search),
        Purpose::Selection(List::Panel(Side::Left)),
        Purpose::Selection(List::Panel(Side::Right)),
        Purpose::Status,
        Purpose::Export,
        Purpose::Program,
        Purpose::IncludeCount,
        Purpose::SettingsSaved,
        Purpose::FolderSizes(Side::Left),
        Purpose::FolderSizes(Side::Right),
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
    /// One chunk of a list's rows, gathered from all its pages.
    Rows(List, ChunkAsk),
    Selection(List, SelectionAsk),
    /// The entries below the one selected folder of Search's list and the
    /// summed size of its files: the totals of an empty search scoped to
    /// it, as `gozler -get-folder-size` asks. Only Search's inspector shows
    /// them.
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
    /// The direct subfolders of a panel's folder, whose sizes the panel
    /// shows, answered with [`Outcome::Rows`].
    FolderSizes(Side, ChunkAsk),
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
            Ask::Rows(list, _) => Purpose::Results(list),
            Ask::Selection(list, _) => Purpose::Selection(list),
            Ask::FolderSize(_) => Purpose::Selection(List::Search),
            Ask::Export(_) => Purpose::Export,
            Ask::Status => Purpose::Status,
            Ask::Program(_) => Purpose::Program,
            Ask::CountIncluded(_) => Purpose::IncludeCount,
            Ask::SettingsSaved => Purpose::SettingsSaved,
            Ask::FolderSizes(side, _) => Purpose::FolderSizes(side),
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
    /// The service's status, and the gozlerd.exe files around it.
    Status(DaemonStatus, Programs),
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
    /// The service is older than the window.
    TooOld {
        programs: Programs,
    },
    /// The service refused the request; `detail` is its English text.
    Refused {
        detail: String,
    },
    Lost,
}

/// The program the service's configuration names and the gozlerd.exe
/// beside the window, which Settings > Index shows, and what Use local makes
/// of them (DESIGN.md: The Drives and service page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Programs {
    pub(crate) local: Local,
    /// `None` when the service's configuration or its program cannot be
    /// read.
    pub(crate) service: Option<ProgramFile>,
    pub(crate) beside: Beside,
}

impl Programs {
    /// While the files' check runs, or when neither file can be read.
    pub(crate) const UNKNOWN: Programs = Programs {
        local: Local::NotOffered,
        service: None,
        beside: Beside::Unknown,
    };
}

/// The gozlerd.exe beside the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Beside {
    /// There is none that can be read, or the check has not answered.
    Unknown,
    /// It holds the bytes of the service's program, so it is that build.
    SameAsService(PathBuf),
    Other(ProgramFile),
}

/// A gozlerd.exe file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProgramFile {
    pub(crate) path: PathBuf,
    /// As the program prints it, such as `0.3.0 (2026-10-20 09:14)`; `None`
    /// when it was not run or did not answer.
    pub(crate) version: Option<String>,
    /// Its last write time, as a FILETIME. It tells two builds apart, which
    /// the version's time does not: that is when gozler-core last changed.
    pub(crate) modified: i64,
}

/// What Use local finds in the gozlerd.exe beside the window (DESIGN.md:
/// Service fixes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Local {
    /// Nothing to offer: there is none, it is the service's own program, it
    /// does not serve the window's protocol, or its check has not ended.
    NotOffered,
    Offered(LocalService),
    /// Not the gozlerd.exe that the setup's build paired with this window,
    /// so Use local stays off.
    Mismatched,
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
