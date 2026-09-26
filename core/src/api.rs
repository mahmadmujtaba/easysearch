//! Wire types for the engine protocol (see `docs/scope.md`).
//!
//! `Query` and `Status` are serialized directly (both derive serde). The search
//! *response* needs a DTO because `ResultRow::path` is an `OsString` that cannot
//! round-trip through JSON: it is sent as a (lossy) UTF-8 string.

use crate::engine::{ResultRow, SearchResponse, Status};
use serde::{Deserialize, Serialize};

/// Protocol version reported by [`Health`]; bump on breaking wire changes.
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub api: u32,
    /// Seconds since the engine started.
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowDto {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponseDto {
    pub results: Vec<RowDto>,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub indexed: u64,
}

impl SearchResponseDto {
    pub fn from_response(r: SearchResponse) -> Self {
        SearchResponseDto {
            results: r
                .results
                .iter()
                .map(|row| RowDto {
                    path: row.path.to_string_lossy().into_owned(),
                    size: row.size,
                    mtime: row.mtime,
                    is_dir: row.is_dir,
                })
                .collect(),
            truncated: r.truncated,
            elapsed_ms: r.elapsed_ms,
            indexed: r.indexed,
        }
    }

    pub fn into_response(self) -> SearchResponse {
        SearchResponse {
            results: self
                .results
                .into_iter()
                .map(|row| ResultRow {
                    path: row.path.into(),
                    size: row.size,
                    mtime: row.mtime,
                    is_dir: row.is_dir,
                })
                .collect(),
            truncated: self.truncated,
            elapsed_ms: self.elapsed_ms,
            indexed: self.indexed,
        }
    }
}

/// Status plus exact index counts (computed once, server-side).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub status: Status,
    pub files: u64,
    pub dirs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDto {
    pub error: String,
}

/// Response of the `count` op: how many entries match, without shipping them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountDto {
    pub count: u64,
}

impl ErrorDto {
    pub fn new(msg: impl Into<String>) -> Self {
        ErrorDto { error: msg.into() }
    }
}
