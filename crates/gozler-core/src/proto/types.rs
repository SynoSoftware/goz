//! Wire protocol payload types.
//!
//! Plain serde data: JSON on the wire, no I/O anywhere. Both enums are
//! internally tagged (`"t"`) so a reader can dispatch on one field, and every
//! struct tolerates unknown JSON fields (serde default), so a new field with a
//! default needs no version bump. A request an older daemon cannot serve, such
//! as a new kind of term in a [`ParsedQuery`], raises [`PROTO_VERSION`].

use serde::{Deserialize, Serialize};

use crate::query::ParsedQuery;
use crate::scope::VolumeMounts;
use crate::types::{SortSpec, VolumePhase};

/// Protocol version spoken by this build.
///
/// A daemon serves every version from [`OLDEST_PROTO_VERSION`] up to this
/// one: it answers a [`Request::Hello`] with the highest version both sides
/// speak, and refuses with [`ProtoError::Unsupported`] when the ranges do not
/// meet, rather than agreeing on a version one side cannot actually speak.
/// The client asks for this version alone, so a daemon older than the client
/// refuses it.
pub const PROTO_VERSION: u16 = 4;

/// The oldest protocol version a daemon of this build serves: protocol 1's
/// clients send [`Request::Query`] without a `Hello`.
pub const OLDEST_PROTO_VERSION: u16 = 1;

/// The daemon's named pipe.
pub const PIPE_NAME: &str = r"\\.\pipe\goz-v1";

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
    /// cannot serve its requests refuses here instead of misreading them. The
    /// daemon serves requests without one, as protocol 1's client sends none.
    Hello {
        /// Lowest protocol version the client can speak.
        proto_min: u16,
        /// Highest protocol version the client can speak.
        proto_max: u16,
        /// Human-readable client identifier (name/version), for logs.
        client: String,
    },
    /// Protocol 1's search: query text that the daemon parses.
    Query(QueryRequest),
    /// Ask for per-volume index status.
    Status,
    /// From protocol 2: a search whose query the client parsed.
    Search(SearchRequest),
}

/// Parameters of one protocol 1 search, which the daemon still serves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    /// The search string, which the daemon parses with
    /// [`parse_query_with_case`](crate::query::parse_query_with_case).
    pub query: String,
    /// Optional scope folder (es `-path`), already absolutized by the client.
    pub scope: Option<String>,
    pub sort: SortSpec,
    /// Number of leading matches to skip (pagination).
    pub offset: u32,
    /// Maximum matches to return; `None` = all.
    pub limit: Option<u32>,
    /// Populate [`ResultItem::size`] (skip the lookup otherwise).
    pub want_size: bool,
    /// Populate [`ResultItem::mtime_ft`] (skip the lookup otherwise).
    pub want_mtime: bool,
    pub match_case: bool,
}

/// One search, from protocol 2 on.
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
    /// The request failed.
    Error {
        /// Machine-readable failure class.
        code: ProtoError,
        /// Human-readable detail for the user/log.
        message: String,
    },
}

/// One page of matches for a [`QueryRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryResults {
    /// Full match count regardless of paging (es `totitems` semantics).
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
}

/// The totals of every match of a [`Returns::Totals`] search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryTotals {
    /// Every match, files and folders.
    pub count: u64,
    /// The summed size of the matching files in bytes. A folder has no size,
    /// and a file whose size is unknown adds nothing.
    pub size: u64,
    /// As [`QueryResults::volumes_incomplete`].
    pub volumes_incomplete: bool,
    /// As [`QueryResults::metadata_pending`]: some sizes may be unknown.
    pub metadata_pending: bool,
}

/// One matched file or directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultItem {
    /// Full path, WTF-8 lossy-decoded to UTF-8 (lone surrogates → U+FFFD).
    pub path: String,
    /// Exact UTF-16 code units of the path, present ONLY when [`Self::path`]
    /// was lossy (the name contained unpaired surrogates). Clients that need
    /// exact fidelity reconstruct the `OsString` from this.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path_u16: Option<Vec<u16>>,
    /// File size in bytes; `None` = unknown, not requested, or a directory.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub size: Option<u64>,
    /// Raw FILETIME modification stamp; `None` = unknown or not requested.
    /// The client formats it (locale/timezone stays client-side).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mtime_ft: Option<i64>,
    pub is_dir: bool,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SortDir, SortKey};
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

    fn sample_query() -> QueryRequest {
        QueryRequest {
            query: "*.rs size:>1kb".into(),
            scope: Some(r"C:\src".into()),
            sort: SortSpec {
                key: SortKey::Size,
                dir: SortDir::Desc,
            },
            offset: 128,
            limit: Some(50),
            want_size: true,
            want_mtime: false,
            match_case: true,
        }
    }

    fn sample_items() -> Vec<ResultItem> {
        vec![
            ResultItem {
                path: r"C:\src\main.rs".into(),
                path_u16: None,
                size: Some(1234),
                mtime_ft: Some(133_500_000_000_000_000),
                is_dir: false,
            },
            ResultItem {
                path: r"C:\src".into(),
                path_u16: None,
                size: None,
                mtime_ft: None,
                is_dir: true,
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
    fn request_query_round_trips() {
        rt_request(&Request::Query(sample_query()));
        // Minimal form too: no scope, no limit.
        rt_request(&Request::Query(QueryRequest {
            query: String::new(),
            scope: None,
            sort: SortSpec::default(),
            offset: 0,
            limit: None,
            want_size: false,
            want_mtime: false,
            match_case: false,
        }));
    }

    #[test]
    fn request_status_round_trips() {
        rt_request(&Request::Status);
    }

    /// A daemon of a later protocol must still read the requests of this
    /// one, and this client must read the daemon's totals, so the JSON of
    /// every protocol 2 shape is pinned here as a peer writes it.
    #[test]
    fn protocol_2_payloads_keep_their_json() {
        use crate::query::{Clause, Kind, SizeRange, Term, Test};
        let term = |negated, test| Term { negated, test };
        let search = Request::Search(SearchRequest {
            query: ParsedQuery {
                clauses: vec![
                    Clause {
                        terms: vec![
                            term(false, Test::Name("a".into())),
                            term(true, Test::Path("b/c".into())),
                        ],
                    },
                    Clause {
                        terms: vec![
                            term(false, Test::NameWildcard("*.d".into())),
                            term(false, Test::PathWildcard("*/e/*".into())),
                            term(false, Test::Extension(vec!["f".into()])),
                            term(false, Test::Size(SizeRange { min: 1, max: 2 })),
                            term(false, Test::Kind(Kind::Folder)),
                        ],
                    },
                ],
                match_case: true,
            },
            scope: Some("C:/x".into()),
            returns: Returns::Rows(Rows {
                sort: SortSpec::default(),
                offset: 0,
                limit: Some(20),
                want_size: true,
                want_mtime: false,
            }),
        });
        let search_json = concat!(
            r#"{"t":"Search","query":{"clauses":[{"terms":["#,
            r#"{"negated":false,"test":{"Name":"a"}},"#,
            r#"{"negated":true,"test":{"Path":"b/c"}}]},{"terms":["#,
            r#"{"negated":false,"test":{"NameWildcard":"*.d"}},"#,
            r#"{"negated":false,"test":{"PathWildcard":"*/e/*"}},"#,
            r#"{"negated":false,"test":{"Extension":["f"]}},"#,
            r#"{"negated":false,"test":{"Size":{"min":1,"max":2}}},"#,
            r#"{"negated":false,"test":{"Kind":"Folder"}}]}],"match_case":true},"#,
            r#""scope":"C:/x","returns":{"Rows":{"sort":{"key":"Name","dir":"Asc"},"#,
            r#""offset":0,"limit":20,"want_size":true,"want_mtime":false}}}"#,
        );
        assert_eq!(
            serde_json::to_string(&search).expect("serialize"),
            search_json
        );
        assert_eq!(
            serde_json::from_str::<Request>(search_json).expect("deserialize"),
            search
        );

        let totals = concat!(
            r#"{"t":"Search","query":{"clauses":[],"match_case":false},"#,
            r#""scope":null,"returns":"Totals"}"#,
        );
        assert!(matches!(
            serde_json::from_str::<Request>(totals).expect("deserialize"),
            Request::Search(SearchRequest {
                returns: Returns::Totals,
                ..
            })
        ));

        let reply = Response::Totals(QueryTotals {
            count: 3,
            size: 4,
            volumes_incomplete: true,
            metadata_pending: false,
        });
        let reply_json = concat!(
            r#"{"t":"Totals","count":3,"size":4,"#,
            r#""volumes_incomplete":true,"metadata_pending":false}"#,
        );
        assert_eq!(
            serde_json::to_string(&reply).expect("serialize"),
            reply_json
        );
        assert_eq!(
            serde_json::from_str::<Response>(reply_json).expect("deserialize"),
            reply
        );
    }

    /// Protocol 3 adds the `parent:` test; a later daemon must still read it
    /// as this client writes it.
    #[test]
    fn protocol_3_parent_test_keeps_its_json() {
        use crate::query::{Clause, Test};
        let search = Request::Search(SearchRequest {
            query: ParsedQuery {
                clauses: vec![Clause::single(Test::Parent(r"C:\repo".into()))],
                match_case: false,
            },
            scope: None,
            returns: Returns::Totals,
        });
        let json = concat!(
            r#"{"t":"Search","query":{"clauses":[{"terms":["#,
            r#"{"negated":false,"test":{"Parent":"C:\\repo"}}]}],"match_case":false},"#,
            r#""scope":null,"returns":"Totals"}"#,
        );
        assert_eq!(serde_json::to_string(&search).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<Request>(json).expect("deserialize"),
            search
        );
    }

    /// Protocol 4 adds the whole-word and regular expression tests; a later
    /// daemon must still read them as this client writes them.
    #[test]
    fn protocol_4_word_and_regex_tests_keep_their_json() {
        use crate::query::{Clause, Term, Test};
        let term = |test| Term {
            negated: false,
            test,
        };
        let search = Request::Search(SearchRequest {
            query: ParsedQuery {
                clauses: vec![Clause {
                    terms: vec![
                        term(Test::NameWholeWord("a".into())),
                        term(Test::PathWholeWord(r"b\c".into())),
                        term(Test::NameRegex("^d|e$".into())),
                        term(Test::PathRegex(r"f\\g".into())),
                    ],
                }],
                match_case: false,
            },
            scope: None,
            returns: Returns::Totals,
        });
        let json = concat!(
            r#"{"t":"Search","query":{"clauses":[{"terms":["#,
            r#"{"negated":false,"test":{"NameWholeWord":"a"}},"#,
            r#"{"negated":false,"test":{"PathWholeWord":"b\\c"}},"#,
            r#"{"negated":false,"test":{"NameRegex":"^d|e$"}},"#,
            r#"{"negated":false,"test":{"PathRegex":"f\\\\g"}}]}],"match_case":false},"#,
            r#""scope":null,"returns":"Totals"}"#,
        );
        assert_eq!(serde_json::to_string(&search).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<Request>(json).expect("deserialize"),
            search
        );
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
        }));
        // Empty page.
        rt_response(&Response::Results(QueryResults {
            total: 0,
            items: vec![],
            more: false,
            volumes_incomplete: false,
            metadata_pending: false,
            generation: 0,
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
                entries: 1_000_000 + i as u64,
                generation: i as u64,
                metadata_pending: i % 2 == 0,
                journal_max_bytes: (i % 2 == 1).then_some(64 << 20),
                placeholders_created: i as u64,
                delete_of_unknown: 2 * i as u64,
                stale_slots: 3 * i as u64,
                link_reconciles_dropped: 4 * i as u64,
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
            is_dir: false,
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
        let lossy = wtf8::from_utf16(&units, &mut wtf8_bytes);
        assert!(lossy);
        let item = ResultItem {
            path: wtf8::to_string_lossy(&wtf8_bytes),
            path_u16: Some(units.clone()),
            size: Some(7),
            mtime_ft: Some(-1),
            is_dir: false,
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

        let req: Request = serde_json::from_str(
            r#"{"t":"Query","query":"a","scope":null,"sort":{"key":"Name","dir":"Asc"},
                "offset":0,"limit":null,"want_size":false,"want_mtime":false,
                "match_case":false,"future_flag":true}"#,
        )
        .expect("nested extra field");
        assert!(matches!(req, Request::Query(q) if q.query == "a"));
    }
}
