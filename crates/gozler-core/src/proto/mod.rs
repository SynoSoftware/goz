//! Wire protocol: request/response types + sans-io frame codec.
//!
//! Shared verbatim by the tokio daemon and the blocking CLI. Nothing here
//! does I/O. The wire format is a 4-byte LE length prefix followed by a
//! payload. Client→server payloads are JSON ([`Request`]). Server→client
//! payloads are `[tag: u8][body]`: JSON for control [`Response`]s, and the
//! compact binary encoding of [`binary`] for results pages.
//!
//! - [`types`]: serde payload types and protocol constants.
//! - [`frame`]: [`encode_frame`] / [`FrameDecoder`], the request JSON helpers
//!   and the tagged response frames.

pub mod binary;
pub mod frame;
pub mod types;

pub use binary::{
    PageEnd, PageItem, RESP_TAG_JSON, RESP_TAG_RESULTS, ResultsPage, SentFolders,
    decode_results_payload, encode_page,
};
pub use frame::{
    FrameDecoder, FrameError, RespFrameError, decode_request, decode_response_frame, encode_frame,
    encode_request, encode_response_frame,
};
pub use types::{
    DaemonStatus, LeftOutVolume, MAX_CLIENT_FRAME, MAX_SERVER_FRAME, MemPair, OLDEST_PROTO_VERSION,
    PAGE_ROWS, PIPE_NAME, PROTO_VERSION, ProtoError, QueryResults, QueryTotals, Request, Response,
    ResultItem, Returns, Rows, SERVICE_NAME, SearchRequest, VolumeMemory, VolumeStatus,
};
