//! The requests the model asks for and the replies it accepts, in the core's
//! terms, so the model's handling of replies and failures is tested without
//! a daemon (docs/plans/ui-design.md: Requests). `app::requests` runs each
//! [`Fetch`] on its own thread and answers with a [`Fetched`].

use gozler_client::NotRunning;
use gozler_core::proto::{DaemonStatus, ResultItem, Returns, Rows, SearchRequest};
use gozler_core::search::Search;
use gozler_core::types::SortSpec;

/// What a request is for. At most one request of each purpose is live, so
/// starting one cancels the live one of its purpose and no other (design
/// rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The list's rows: a new search's first chunk, or a chunk of the shown
    /// search.
    Results,
    Status,
}

impl Purpose {
    pub(crate) const ALL: [Purpose; 2] = [Purpose::Results, Purpose::Status];
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
    Status,
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
        }))
    }
}

impl Fetch {
    pub(crate) fn purpose(&self) -> Purpose {
        match self.ask {
            Ask::Rows(_) => Purpose::Results,
            Ask::Status => Purpose::Status,
        }
    }
}

/// A reply to a [`Fetch`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fetched {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    Rows(Chunk),
    Status(DaemonStatus),
    Failed(DaemonFailure),
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
}

/// The failures the window shows differently (spec: Daemon failures);
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
    TooOld,
    /// The service refused the request; `detail` is its English text.
    Refused {
        detail: String,
    },
    Lost,
}

impl DaemonFailure {
    /// Whether the failure ends by itself, so the window asks for status
    /// until the service answers. A refused request, a failed owner check
    /// and an older service fail the same way again.
    pub(crate) fn recovers(&self) -> bool {
        match self {
            DaemonFailure::NotRunning(_) | DaemonFailure::NotReady | DaemonFailure::Lost => true,
            DaemonFailure::NotGozler | DaemonFailure::TooOld | DaemonFailure::Refused { .. } => {
                false
            }
        }
    }
}
