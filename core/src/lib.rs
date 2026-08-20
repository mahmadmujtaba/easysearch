//! everything-core — realtime filename + content search engine.
//!
//! Design (see `docs/scope.md`):
//! - a live in-memory path index, kept fresh by kernel filesystem events (`notify`),
//! - Everything-style name matching via `globset` + `regex`,
//! - content search via the embedded ripgrep engine (`grep-searcher`),
//! - an optional bounded in-RAM content cache for repeated queries.

pub mod config;
pub mod content;
pub mod content_index;
pub mod engine;
pub mod index;
pub mod matcher;
pub mod roots;
pub mod walker;
pub mod watcher;

pub use config::Config;
pub use engine::{Engine, ResultRow, SearchResponse, State, Status};
pub use matcher::{CompiledQuery, Query};
