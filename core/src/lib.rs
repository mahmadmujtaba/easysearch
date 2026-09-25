//! everything-core — realtime filename + content search engine.
//!
//! Design (see `docs/scope.md`):
//! - a disk-backed, memory-mapped base index (kernel page cache, low RSS)
//!   kept fresh by kernel filesystem events (`notify`) through a small
//!   in-memory change overlay,
//! - Everything-style name matching via `globset` + `regex`,
//! - content search via the embedded ripgrep engine (`grep-searcher`),
//! - an optional bounded in-RAM content cache for repeated queries.

pub mod api;
pub mod backend;
pub mod config;
pub mod content;
pub mod content_index;
pub mod disk_index;
pub mod engine;
pub mod matcher;
pub mod overlay;
pub mod remote;
pub mod roots;
pub mod sqlite_index;
pub mod update;
pub mod walker;
pub mod watcher;

pub use backend::Backend;
pub use config::Config;
pub use engine::{ContentIndexStatus, Engine, ResultRow, SearchResponse, State, Status};
pub use matcher::{Category, CompiledQuery, Query};
pub use remote::Remote;
pub use update::{Available, Fetcher, InstallReport, Stage, UpdateConfig, UpdateError, Updater};
