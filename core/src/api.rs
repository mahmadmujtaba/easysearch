//! Wire types for the local daemon HTTP API (see `docs/api.md`).
//!
//! `Query` and `Status` are serialized directly (both derive serde). The search
//! *response* needs a DTO because `ResultRow::path` is an `OsString` that cannot
//! round-trip through JSON: it is sent as a (lossy) UTF-8 string.

use crate::engine::{ResultRow, SearchResponse, Status};
use crate::matcher::Category;
use serde::{Deserialize, Serialize};

/// API version reported by [`Health`]; bump on breaking wire changes.
pub const API_VERSION: u32 = 1;

/// Default daemon address (localhost only).
pub const DEFAULT_ADDR: &str = "127.0.0.1:5858";

/// Default port used when none is given.
pub const DEFAULT_PORT: u16 = 5858;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub api: u32,
    /// Seconds since the daemon started.
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

impl ErrorDto {
    pub fn new(msg: impl Into<String>) -> Self {
        ErrorDto { error: msg.into() }
    }
}

/// Parse a category name used by the `?category=` query parameter.
pub fn category_from_str(s: &str) -> Option<Category> {
    Some(match s.to_ascii_lowercase().as_str() {
        "" | "all" => Category::All,
        "recent" => Category::Recent {
            max_age_secs: 7 * 24 * 3600,
        },
        "images" => Category::Images,
        "docs" | "documents" => Category::Docs,
        "code" => Category::Code,
        "archives" => Category::Archives,
        "audio" => Category::Audio,
        "video" => Category::Video,
        "large" => Category::Large {
            min_bytes: 1024 * 1024 * 1024,
        },
        _ => return None,
    })
}
