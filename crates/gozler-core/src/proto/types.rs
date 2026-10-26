//! Wire protocol payload types.
//!
//! Plain serde data: JSON on the wire, no I/O anywhere. Both enums are
//! internally tagged (`"t"`) so a reader can dispatch on one field, and every
//! struct tolerates unknown JSON fields. A payload change raises
//! [`PROTO_VERSION`], because the daemon serves only its own version.

use serde::{Deserialize, Serialize};

use crate::attributes::Attributes;
use crate::include::IndexSettings;
use crate::query::{MatchSums, ParsedQuery, RowsSpec, SearchTotals};
use crate::scope::VolumeMounts;
use crate::types::{IndexProgress, SortSpec, VolumePhase};

/// Protocol version spoken by this build.
///
/// A daemon answers a [`Request::Hello`] whose range holds this version, and
/// refuses any other with [`ProtoError::Unsupported`], rather than agreeing on
/// a version one side cannot actually speak. The client asks for this version
/// alone, so a daemon of another build refuses it.
pub const PROTO_VERSION: u16 = 15;

/// The oldest protocol version a daemon of this build serves. **Owner
/// ruling:** gozler serves no older protocol, of gozler or of goz, because
/// `gozler` and `gozlerd` install together from one build, and an old layout
/// kept for an old client would bind every later change.
pub const OLDEST_PROTO_VERSION: u16 = PROTO_VERSION;

/// The daemon's named pipe.
pub const PIPE_NAME: &str = r"\\.\pipe\gozler-v1";

/// The Windows service that runs the daemon, which is also its registry key,
/// so it stays the same across versions.
pub const SERVICE_NAME: &str = "gozler";

/// Server-side decoder cap for client→server frames (1 MiB). A length prefix
/// is a DoS lever; oversize frames poison the connection.
pub const MAX_CLIENT_FRAME: u32 = 1 << 20;

/// Client-side decoder cap for server→client frames (16 MiB).
pub const MAX_SERVER_FRAME: u32 = 16 << 20;

/// Maximum [`ResultItem`]s per `Results` frame; larger result sets are split
/// into multiple frames with [`QueryResults::more`] set on all but the last.
pub const PAGE_ROWS: u32 = 4096;

/// A client→server message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Request {
    /// Version negotiation. The client sends it first, so a daemon that
    /// cannot serve its requests refuses here instead of misreading them.
    Hello {
        /// Lowest protocol version the client can speak.
        proto_min: u16,
        /// Highest protocol version the client can speak.
        proto_max: u16,
        /// Human-readable client identifier (name/version), for logs.
        client: String,
    },
    /// Ask for per-volume index status.
    Status,
    /// A search whose query the client parsed.
    Search(SearchRequest),
    /// An administrator saved the Include settings, so the
    /// daemon reads them again and applies them when they changed. It reads
    /// them from where only an administrator can write, so any client may
    /// send this.
    SettingsSaved,
    /// How many entries the index would keep under these
    /// Include settings. Changes nothing.
    CountIncluded(IndexSettings),
    /// Rows of a result snapshot that a [`Returns::ResultSnapshot`] search
    /// made, as [`Response::Results`] pages.
    ResultSnapshotRows(SnapshotRowsRequest),
    /// The rows of a result snapshot in another order, with no search: a
    /// new result snapshot of them, answered as a
    /// [`Returns::ResultSnapshot`] search is, which replaces the one named.
    SortResultSnapshot(SortSnapshotRequest),
}

/// The id of a result snapshot: 128 random bits, which only the client that
/// made it learns, over its own pipe. It travels as 32 hex digits, because
/// serde reads no `u128` inside an internally tagged enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ResultSnapshotId(pub u128);

impl From<ResultSnapshotId> for String {
    fn from(id: ResultSnapshotId) -> String {
        format!("{:032x}", id.0)
    }
}

impl TryFrom<String> for ResultSnapshotId {
    type Error = std::num::ParseIntError;

    fn try_from(hex: String) -> Result<Self, Self::Error> {
        u128::from_str_radix(&hex, 16).map(ResultSnapshotId)
    }
}

/// Which rows of a result snapshot a [`Request::ResultSnapshotRows`] reads,
/// and with which values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRowsRequest {
    pub id: ResultSnapshotId,
    /// The place of the first row in the result order.
    pub offset: u32,
    /// The most rows to return; `None` = every row from `offset`.
    pub limit: Option<u32>,
    pub want_size: bool,
    pub want_mtime: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub want_created: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub want_attributes: bool,
}

/// Which result snapshot a [`Request::SortResultSnapshot`] sorts, and the
/// rows of it to send: `rows`' sort orders the new result snapshot, and its
/// offset and limit pick the rows sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SortSnapshotRequest {
    pub id: ResultSnapshotId,
    pub rows: Rows,
}

/// One search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: ParsedQuery,
    /// Optional scope folder (es `-path`): an absolute path with `.` and `..`
    /// already removed, as [`build_search`](crate::escompat::build_search)
    /// writes it.
    pub scope: Option<String>,
    pub returns: Returns,
}

/// What a [`SearchRequest`] asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Returns {
    /// The matches, as [`Response::Results`] pages.
    Rows(Rows),
    /// Only how many entries match and the summed size of the matching files,
    /// as one [`Response::Totals`], so no row crosses the pipe.
    Totals,
    /// Every match, kept in the daemon as a result snapshot, which
    /// [`Request::ResultSnapshotRows`] reads later: one
    /// [`Response::ResultSnapshot`], then the [`Response::Results`] pages of
    /// the rows that `rows`' offset and limit ask for. `rows`' sort orders
    /// it. `replaces` names the client's earlier result snapshot, which the
    /// daemon drops.
    ResultSnapshot {
        rows: Rows,
        replaces: Option<ResultSnapshotId>,
    },
}

/// Which matches a [`Returns::Rows`] search returns, in what order, and with
/// which values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rows {
    pub sort: SortSpec,
    /// Number of leading matches to skip (pagination).
    pub offset: u32,
    /// Maximum matches to return; `None` = all.
    pub limit: Option<u32>,
    /// Populate [`ResultItem::size`] (skip the lookup otherwise).
    pub want_size: bool,
    /// Populate [`ResultItem::mtime_ft`] (skip the lookup otherwise).
    pub want_mtime: bool,
    /// Populate [`ResultItem::created_ft`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub want_created: bool,
    /// Populate [`ResultItem::attributes`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub want_attributes: bool,
    /// Fill [`QueryResults::sums`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub want_sums: bool,
}

impl Rows {
    /// The rows the search returns. [`super::encode_page`] gives each row the
    /// values this asks for.
    pub fn spec(&self) -> RowsSpec {
        RowsSpec {
            sort: self.sort,
            offset: self.offset,
            limit: self.limit,
            sums: self.want_sums,
        }
    }
}

/// A server→client message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Response {
    /// Reply to [`Request::Hello`].
    Hello {
        /// Negotiated protocol version.
        proto: u16,
        /// Human-readable server identifier (name/version), for logs.
        server: String,
        /// Whether the index is ready to serve queries.
        ready: bool,
    },
    /// One page of query results.
    Results(QueryResults),
    /// Reply to [`Request::Status`].
    Status(DaemonStatus),
    /// Reply to a [`Returns::Totals`] search.
    Totals(QueryTotals),
    /// Reply to [`Request::SettingsSaved`]: the daemon applies the saved
    /// settings.
    SettingsApplied,
    /// The result snapshot a [`Returns::ResultSnapshot`] search or a
    /// [`Request::SortResultSnapshot`] made; its rows' pages follow.
    ResultSnapshot {
        id: ResultSnapshotId,
        /// The rows it holds, a gone row counted.
        total: u64,
    },
    /// Reply to [`Request::CountIncluded`].
    IncludedCount {
        /// The entries every indexed volume would keep. A volume that the
        /// settings add is not indexed yet, so it adds none.
        entries: u64,
    },
    /// The request failed.
    Error {
        /// Machine-readable failure class.
        code: ProtoError,
        /// Human-readable detail for the user/log.
        message: String,
    },
}

/// One page of matches for a [`SearchRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryResults {
    /// The match count less the rows the reply left out up to this page;
    /// the last page's total holds (es `totitems` semantics).
    pub total: u64,
    /// The matches in this page, at most [`PAGE_ROWS`].
    pub items: Vec<ResultItem>,
    /// `true` → another `Results` frame follows for the same query.
    pub more: bool,
    /// `true` → at least one volume is bootstrapping/rescanning/failed, so
    /// results may be incomplete. The honesty bit: never presented silently.
    pub volumes_incomplete: bool,
    /// `true` → the size and date pass failed or stopped on at least one
    /// volume in scope; requested size and date columns may be `None`.
    pub metadata_pending: bool,
    /// Index generation the results were computed against.
    pub generation: u64,
    /// Every match's files and folders and their sizes, when the request
    /// asked for them with [`Rows::want_sums`] and the daemon knows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sums: Option<MatchSums>,
}

impl QueryResults {
    /// Folds `page`, the next page of the same result set, into this one, so
    /// every reader of a paged answer agrees on what the pages say together:
    /// the rows join, `total`, `generation` and `sums` are the latest page's,
    /// and a flag holds when any page sets it.
    pub fn add_page(&mut self, page: QueryResults) {
        self.total = page.total;
        self.items.extend(page.items);
        self.more = page.more;
        self.volumes_incomplete |= page.volumes_incomplete;
        self.metadata_pending |= page.metadata_pending;
        self.generation = page.generation;
        self.sums = page.sums;
    }
}

/// The totals of every match of a [`Returns::Totals`] search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryTotals {
    /// Every match, files and folders.
    pub count: u64,
    /// The summed size of the matching files in bytes. A folder adds nothing,
    /// because its size is the sum of its files, and a file whose size is
    /// unknown adds nothing.
    pub size: u64,
    /// As [`QueryResults::volumes_incomplete`].
    pub volumes_incomplete: bool,
    /// As [`QueryResults::metadata_pending`]: some sizes may be unknown.
    pub metadata_pending: bool,
}

impl From<SearchTotals> for QueryTotals {
    fn from(found: SearchTotals) -> Self {
        QueryTotals {
            count: found.totals.count,
            size: found.totals.size,
            volumes_incomplete: found.volumes_incomplete,
            metadata_pending: found.metadata_pending,
        }
    }
}

/// One matched file or directory. A row of a result snapshot whose file is
/// gone has an empty path and no other field ([`Self::is_gone`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultItem {
    /// Full path, WTF-8 lossy-decoded to UTF-8 (lone surrogates → U+FFFD).
    pub path: String,
    /// Exact UTF-16 code units of the path, present ONLY when [`Self::path`]
    /// was lossy (the name contained unpaired surrogates). Clients that need
    /// exact fidelity reconstruct the `OsString` from this.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path_u16: Option<Vec<u16>>,
    /// Size in bytes, a folder's being the size of the files below it; `None`
    /// = unknown or not requested.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub size: Option<u64>,
    /// Raw FILETIME modification stamp; `None` = unknown or not requested.
    /// The client formats it (locale/timezone stays client-side).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mtime_ft: Option<i64>,
    /// Raw FILETIME creation stamp; `None` = unknown, not requested, or
    /// Dates created off.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_ft: Option<i64>,
    pub is_dir: bool,
    /// The attributes, as `FILE_ATTRIBUTE_*` bits; `None` = not requested.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub attributes: Option<Attributes>,
}

impl ResultItem {
    /// A row of a result snapshot whose file was deleted, moved out of the
    /// search's folder, or has no full path now.
    pub fn gone() -> Self {
        ResultItem {
            path: String::new(),
            path_u16: None,
            size: None,
            mtime_ft: None,
            created_ft: None,
            is_dir: false,
            attributes: None,
        }
    }

    /// Whether this is a [`Self::gone`] row: no file has an empty path.
    pub fn is_gone(&self) -> bool {
        self.path.is_empty()
    }
}

/// Daemon-wide index status.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Per-volume status, one entry per indexed volume.
    pub volumes: Vec<VolumeStatus>,
    /// `true` → at least one volume is not fully live.
    pub volumes_incomplete: bool,
    /// Daemon working-set bytes (what Task Manager's Memory column tracks),
    /// self-reported at the moment of the status request. 0 = unavailable.
    #[serde(default)]
    pub process_working_set: u64,
    /// Daemon commit charge (private bytes). This is the real footprint the
    /// process holds regardless of what is currently resident. 0 = unavailable.
    #[serde(default)]
    pub process_private_bytes: u64,
    /// The Include settings the daemon applies; `None` from a daemon before
    /// protocol 8.
    #[serde(default)]
    pub include: Option<IndexSettings>,
    /// The fixed NTFS and ReFS volumes that the Include settings leave out,
    /// which [`Self::volumes`] does not list.
    #[serde(default)]
    pub left_out: Vec<LeftOutVolume>,
}

/// A volume that the Include settings leave out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftOutVolume {
    /// Stable volume GUID path identifier.
    pub guid: String,
    /// Mount points (drive letters / folder mounts) of the volume.
    pub mounts: Vec<String>,
}

impl LeftOutVolume {
    /// Where the volume is mounted.
    pub fn mounted(&self) -> VolumeMounts<'_> {
        VolumeMounts {
            mounts: &self.mounts,
            guid: &self.guid,
        }
    }
}

/// Heap bytes of one index component on the wire: live bytes and allocated
/// (capacity) bytes. See `IndexMemory` in gozler-core for what each component is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemPair {
    pub used: u64,
    pub alloc: u64,
}

/// Per-component memory breakdown of one volume's index, for `--status`.
/// All-zero when the daemon predates this field (serde default).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VolumeMemory {
    pub entries: MemPair,
    pub arena_raw: MemPair,
    pub arena_folded: MemPair,
    pub frn_map: MemPair,
    /// Per-unique-name tables (pairs, refcounts, chain heads, intern table).
    pub name_tables: MemPair,
    pub dir_children: MemPair,
    /// The folder security descriptors, each distinct one once.
    pub descriptors: MemPair,
    /// Which FRN-map backing is in use ("dense" / "sparse").
    pub frn_map_kind: String,
}

impl VolumeMemory {
    /// The sum of every component.
    pub fn total(&self) -> MemPair {
        let parts = [
            self.entries,
            self.arena_raw,
            self.arena_folded,
            self.frn_map,
            self.name_tables,
            self.dir_children,
            self.descriptors,
        ];
        MemPair {
            used: parts.iter().map(|p| p.used).sum(),
            alloc: parts.iter().map(|p| p.alloc).sum(),
        }
    }
}

/// Status of one indexed volume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumeStatus {
    /// Stable volume GUID path identifier.
    pub guid: String,
    /// Mount points (drive letters / folder mounts) of the volume.
    pub mounts: Vec<String>,
    pub phase: VolumePhase,
    pub progress: Option<IndexProgress>,
    /// Number of indexed entries on the volume.
    pub entries: u64,
    /// Index generation for the volume.
    pub generation: u64,
    /// `true` → the size and date pass failed or stopped for this volume, so
    /// some sizes and dates are unknown.
    pub metadata_pending: bool,
    /// Maximum size of the volume's USN journal in bytes; `None` for a volume
    /// without one (or from daemons predating the field). It bounds how long
    /// the daemon can be down and still replay changes instead of rescanning.
    #[serde(default)]
    pub journal_max_bytes: Option<u64>,
    /// Index drift counters. Non-zero values are not errors on their own (a
    /// missed delete or a slot reuse is normal on a busy volume), but they are
    /// the only outward sign that the index and the volume have diverged.
    #[serde(default)]
    pub placeholders_created: u64,
    #[serde(default)]
    pub delete_of_unknown: u64,
    #[serde(default)]
    pub stale_slots: u64,
    /// Hard-link changes whose Win32 link-set walk could not complete (the
    /// walk failed, or a link's folder is not indexed yet). See
    /// `ApplyOutcome::needs_link_reconcile`: successful walks are reconciled
    /// live; these are skipped and counted. Non-zero means a few names on this
    /// volume may be stale until the next rescan.
    #[serde(default)]
    pub link_reconciles_dropped: u64,
    /// Folder descriptor reads that failed since the volume was last
    /// scanned. Each such folder's entries are hidden from every user who
    /// does not see every file.
    pub descriptor_reads_failed: u64,
    /// Per-component index memory breakdown. `None` from daemons predating it.
    #[serde(default)]
    pub memory: Option<VolumeMemory>,
}

impl VolumeStatus {
    /// Where the volume is mounted.
    pub fn mounted(&self) -> VolumeMounts<'_> {
        VolumeMounts {
            mounts: &self.mounts,
            guid: &self.guid,
        }
    }
}

/// Machine-readable failure class carried by [`Response::Error`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtoError {
    /// The query cannot run as sent: the query string failed to parse, or its
    /// scope is on no indexed volume.
    BadQuery,
    /// The client requested an unsupported protocol version or feature.
    Unsupported,
    /// The summed size of the matches does not fit in 64 bits, which only a
    /// corrupt size in the index can cause.
    Overflow,
    /// The result snapshot a [`Request::ResultSnapshotRows`] names was
    /// dropped, or a volume's index was replaced since the search; a new
    /// search answers instead.
    ResultSnapshotGone,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wtf8;

    fn rt_request(req: &Request) {
        let json = serde_json::to_string(req).expect("serialize");
        let back: Request = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, req, "json: {json}");
    }

    fn rt_response(resp: &Response) {
        let json = serde_json::to_string(resp).expect("serialize");
        let back: Response = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, resp, "json: {json}");
    }

    /// A result snapshot's id takes all 128 bits, which serde cannot read as
    /// a number inside a tagged request or reply; the daemon would refuse
    /// every read and the window search again without end.
    #[test]
    fn a_result_snapshot_id_round_trips_in_its_request_and_reply() {
        let id = ResultSnapshotId(u128::MAX - 5);
        rt_response(&Response::ResultSnapshot { id, total: 3 });
        rt_request(&Request::ResultSnapshotRows(SnapshotRowsRequest {
            id,
            offset: 1,
            limit: None,
            want_size: true,
            want_mtime: false,
            want_created: false,
            want_attributes: false,
        }));
    }

    fn sample_items() -> Vec<ResultItem> {
        vec![
            ResultItem {
                path: r"C:\src\main.rs".into(),
                path_u16: None,
                size: Some(1234),
                mtime_ft: Some(133_500_000_000_000_000),
                created_ft: None,
                is_dir: false,
                attributes: None,
            },
            ResultItem {
                path: r"C:\src".into(),
                path_u16: None,
                size: None,
                mtime_ft: None,
                created_ft: None,
                is_dir: true,
                attributes: None,
            },
        ]
    }

    #[test]
    fn request_hello_round_trips() {
        rt_request(&Request::Hello {
            proto_min: 1,
            proto_max: 3,
            client: "gozler-cli 0.1.0".into(),
        });
    }

    #[test]
    fn request_status_round_trips() {
        rt_request(&Request::Status);
    }

    #[test]
    fn response_hello_round_trips() {
        rt_response(&Response::Hello {
            proto: PROTO_VERSION,
            server: "gozd 0.1.0".into(),
            ready: false,
        });
    }

    #[test]
    fn response_results_round_trips() {
        rt_response(&Response::Results(QueryResults {
            total: 987_654,
            items: sample_items(),
            more: true,
            volumes_incomplete: true,
            metadata_pending: true,
            generation: 42,
            sums: Some(MatchSums {
                files: 3,
                file_bytes: 7,
                files_without_size: 1,
                folders: 2,
                folder_bytes: 11,
                folder_bytes_at_least: true,
            }),
        }));
        // Empty page.
        rt_response(&Response::Results(QueryResults {
            total: 0,
            items: vec![],
            more: false,
            volumes_incomplete: false,
            metadata_pending: false,
            generation: 0,
            sums: None,
        }));
    }

    #[test]
    fn response_status_round_trips_every_phase() {
        let phases = [
            VolumePhase::Bootstrapping,
            VolumePhase::Live,
            VolumePhase::Rescanning,
            VolumePhase::Offline,
            VolumePhase::Failed {
                reason: "journal wrapped".into(),
            },
        ];
        let volumes: Vec<VolumeStatus> = phases
            .into_iter()
            .enumerate()
            .map(|(i, phase)| VolumeStatus {
                guid: format!(r"\\?\Volume{{0000000{i}-aaaa-bbbb-cccc-ddddeeeeffff}}\"),
                mounts: vec![format!(r"{}:\", char::from(b'C' + i as u8))],
                phase,
                progress: Some(IndexProgress::Scanning {
                    entries: 400,
                    percent: Some(62),
                }),
                entries: 1_000_000 + i as u64,
                generation: i as u64,
                metadata_pending: i % 2 == 0,
                journal_max_bytes: (i % 2 == 1).then_some(64 << 20),
                placeholders_created: i as u64,
                delete_of_unknown: 2 * i as u64,
                stale_slots: 3 * i as u64,
                link_reconciles_dropped: 4 * i as u64,
                descriptor_reads_failed: 5 * i as u64,
                memory: Some(VolumeMemory {
                    entries: MemPair {
                        used: 10 * i as u64,
                        alloc: 20 * i as u64,
                    },
                    frn_map_kind: "sparse".into(),
                    ..VolumeMemory::default()
                }),
            })
            .collect();
        rt_response(&Response::Status(DaemonStatus {
            volumes,
            volumes_incomplete: true,
            process_working_set: 123_456_789,
            process_private_bytes: 987_654_321,
            include: Some(crate::include::IndexSettings::default()),
            left_out: vec![LeftOutVolume {
                guid: r"\\?\Volume{d}\".into(),
                mounts: vec![r"D:\".into()],
            }],
        }));
    }

    #[test]
    fn response_error_round_trips_every_code() {
        for code in [
            ProtoError::BadQuery,
            ProtoError::Unsupported,
            ProtoError::Overflow,
        ] {
            rt_response(&Response::Error {
                code,
                message: format!("{code:?} happened"),
            });
        }
    }

    #[test]
    fn clean_result_item_omits_optional_fields() {
        let item = ResultItem {
            path: r"C:\clean.txt".into(),
            path_u16: None,
            size: None,
            mtime_ft: None,
            created_ft: None,
            is_dir: false,
            attributes: None,
        };
        let json = serde_json::to_string(&item).expect("serialize");
        assert!(!json.contains("path_u16"), "json: {json}");
        assert!(!json.contains("size"), "json: {json}");
        assert!(!json.contains("mtime_ft"), "json: {json}");
    }

    #[test]
    fn absent_optional_fields_deserialize_to_none() {
        let item: ResultItem =
            serde_json::from_str(r#"{"path":"C:\\a.txt","is_dir":false}"#).expect("deserialize");
        assert_eq!(item.path_u16, None);
        assert_eq!(item.size, None);
        assert_eq!(item.mtime_ft, None);
    }

    #[test]
    fn lossy_path_carries_exact_code_units() {
        // A name with an unpaired high surrogate: WTF-8 flags it lossy, so
        // the item ships path_u16 with the exact original units.
        let units: Vec<u16> = vec![b'C' as u16, b':' as u16, b'\\' as u16, 0xD800, b'x' as u16];
        let mut wtf8_bytes = Vec::new();
        wtf8::from_utf16(&units, &mut wtf8_bytes);
        assert!(core::str::from_utf8(&wtf8_bytes).is_err());
        let item = ResultItem {
            path: wtf8::to_string_lossy(&wtf8_bytes),
            path_u16: Some(units.clone()),
            size: Some(7),
            mtime_ft: Some(-1),
            created_ft: None,
            is_dir: false,
            attributes: None,
        };
        assert!(item.path.contains('\u{FFFD}'));
        let json = serde_json::to_string(&item).expect("serialize");
        assert!(json.contains("path_u16"), "json: {json}");
        let back: ResultItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, item);
        assert_eq!(back.path_u16.as_deref(), Some(units.as_slice()));
    }

    #[test]
    fn unknown_json_fields_are_ignored() {
        // Additive evolution: a newer peer may send fields we don't know.
        let resp: Response = serde_json::from_str(
            r#"{"t":"Hello","proto":1,"server":"gozd","ready":true,"new_field":[1,2,3]}"#,
        )
        .expect("top-level extra field");
        assert_eq!(
            resp,
            Response::Hello {
                proto: 1,
                server: "gozd".into(),
                ready: true
            }
        );
    }
}
