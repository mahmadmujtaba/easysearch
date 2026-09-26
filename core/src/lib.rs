//! easysearch-core — realtime filename + content search engine.
//!
//! Design (see `docs/scope.md`):
//! - a disk-backed, memory-mapped base index (kernel page cache, low RSS)
//!   kept fresh by kernel filesystem events (`notify`) through a small
//!   in-memory change overlay,
//! - Everything-style name matching via `globset` + `regex`,
//! - content search via the embedded ripgrep engine (`grep-searcher`),
//! - an optional bounded content cache (spooled to disk by default, RAM with
//!   `--content-in-memory`) for repeated queries.

pub mod api;
pub mod backend;
pub mod child;
pub mod config;
pub mod content;
pub mod content_index;
pub mod disk_index;
pub mod engine;
pub mod ipc;
pub mod logo;
pub mod matcher;
pub mod overlay;
pub mod process;
pub mod proto;
pub mod roots;
pub mod sqlite_index;
pub mod tags;
pub mod trash;
pub mod walker;
pub mod watcher;

pub use backend::Backend;
pub use child::ChildEngine;
pub use config::Config;
pub use engine::{ContentIndexStatus, Engine, ResultRow, SearchResponse, State, Status};
pub use matcher::{Category, CompiledQuery, Query};
pub use tags::TagStore;
