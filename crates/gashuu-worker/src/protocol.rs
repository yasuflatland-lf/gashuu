//! JSON metadata shared by the main thread and worker.

use crate::wire_error::WireError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Allocated by the main thread, with wrapping arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReqId(pub u32);

/// The next unconsumed pixel buffer in the envelope is this image's straight-alpha RGBA8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub width: u32,
    pub height: u32,
}

pub type WireResult<T> = Result<T, WireError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum WorkerRequest {
    Open {
        req: ReqId,
        session: u64,
        id: String,
        allow_rar: bool,
    },
    /// Applied out-of-band, with no response.
    Close { session: u64 },
    DecodeSpread {
        req: ReqId,
        session: u64,
        leading: u32,
        trailing: Option<u32>,
    },
    Thumbnail {
        req: ReqId,
        session: u64,
        index: u32,
        max_side: u32,
    },
    Cover {
        req: ReqId,
        gen: u64,
        id: String,
        max_side: u32,
        want_count: bool,
    },
    /// Discard covers with generations below this value, with no response.
    CancelCovers { below: u64 },
    /// The structured clone envelope carries the selected File in field f.
    Import {
        req: ReqId,
        name: String,
        size: u64,
        allow_rar: bool,
    },
    /// Clear persisted covers, with no response.
    ClearCovers,
    /// Prune persisted covers to a byte budget, with no response.
    PruneCovers { max_bytes: u64 },
    /// Remove the book directory, with no response.
    DeleteBook { id: String },
    ProbePageCount {
        req: ReqId,
        id: String,
        allow_rar: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum WorkerResponse {
    Ready,
    Opened {
        req: ReqId,
        result: WireResult<OpenedMeta>,
    },
    /// Successful slots consume pixel buffers in leading, then trailing order.
    Spread {
        req: ReqId,
        leading: (u32, WireResult<ImageMeta>),
        trailing: Option<(u32, WireResult<ImageMeta>)>,
    },
    Thumbnail {
        req: ReqId,
        result: WireResult<ImageMeta>,
    },
    Cover {
        req: ReqId,
        result: WireResult<ImageMeta>,
    },
    /// Sent after a successful cover when requested, using the same archive listing.
    CoverCount {
        req: ReqId,
        count: u32,
    },
    PageCount {
        req: ReqId,
        result: WireResult<u32>,
    },
    Imported {
        req: ReqId,
        result: WireResult<ImportedMeta>,
    },
    /// Malformed metadata or a request for an unknown session.
    Rejected {
        req: Option<ReqId>,
        error: WireError,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenedMeta {
    pub id: String,
    pub page_count: u32,
    pub skipped: u32,
    pub listing_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedMeta {
    pub id: String,
    pub title: String,
    pub page_count: u32,
}

pub fn encode<T: Serialize>(msg: &T) -> String {
    serde_json::to_string(msg).expect("worker protocol messages are JSON serializable")
}

pub fn decode<T: DeserializeOwned>(text: &str) -> Result<T, WireError> {
    serde_json::from_str(text).map_err(|error| WireError::Other {
        detail: format!("protocol: {error}"),
    })
}
