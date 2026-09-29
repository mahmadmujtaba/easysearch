//! Search backend: the engine either in-process or in a child process.
//!
//! One type covers both, so the GUI and CLI call the same methods whether the
//! engine is linked into them or running as their child (see [`crate::child`]).
//! There is no network transport: the engine's pipes are private to the pair.

use crate::child::ChildEngine;
use crate::config::Config;
use crate::engine::{Engine, SearchResponse, Status};
use crate::matcher::Query;
use std::sync::Arc;
use std::time::Duration;

pub enum Backend {
    /// The engine lives in this process (zero IPC).
    Local(Arc<Engine>),
    /// The engine runs as this process's child, over its stdin/stdout.
    Child(ChildEngine),
}

impl Backend {
    /// Build and start a local engine.
    pub fn local(config: Config) -> Backend {
        let mut engine = Engine::new(config);
        engine.start();
        Backend::Local(Arc::new(engine))
    }

    /// Drive an engine child the caller has already spawned.
    pub fn child(engine: ChildEngine) -> Backend {
        Backend::Child(engine)
    }

    /// True when the engine is a separate process.
    pub fn is_service(&self) -> bool {
        matches!(self, Backend::Child(_))
    }

    /// Short human label for the status bar.
    pub fn label(&self) -> String {
        match self {
            Backend::Local(_) => "in-process".to_string(),
            Backend::Child(c) => match c.pid() {
                Some(pid) => format!("engine pid {pid}"),
                None => "engine".to_string(),
            },
        }
    }

    /// False when the engine child cannot be reached (always true in-process).
    pub fn connected(&self) -> bool {
        match self {
            Backend::Local(_) => true,
            Backend::Child(c) => c.connected(),
        }
    }

    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        match self {
            Backend::Local(e) => e.search(q),
            Backend::Child(c) => c.search(q),
        }
    }

    pub fn status_snapshot(&self) -> Status {
        match self {
            Backend::Local(e) => e.status_snapshot(),
            Backend::Child(c) => c.status(),
        }
    }

    pub fn counts(&self) -> (u64, u64) {
        match self {
            Backend::Local(e) => e.counts(),
            Backend::Child(c) => c
                .report()
                .map(|rep| (rep.files, rep.dirs))
                .unwrap_or((0, 0)),
        }
    }

    pub fn rebuild(&self) {
        match self {
            Backend::Local(e) => e.rebuild(),
            Backend::Child(c) => {
                let _ = c.rebuild();
            }
        }
    }

    /// Return freed heap pages to the OS — in this process, or in the engine
    /// child when the engine is a separate process.
    pub fn trim_memory(&self) {
        match self {
            Backend::Local(e) => e.trim_memory(),
            Backend::Child(c) => {
                let _ = c.trim_memory();
            }
        }
    }

    /// Stop the engine child (no-op in-process). Returns true if an engine was
    /// asked to stop — useful before relaunching onto a newly installed binary.
    pub fn shutdown(&self) -> bool {
        match self {
            Backend::Local(_) => false,
            Backend::Child(c) => c.shutdown().is_ok(),
        }
    }

    /// Turn honoring of ignore files on/off (in-process engine, or the child).
    pub fn set_respect_ignore(&self, on: bool) {
        match self {
            Backend::Local(e) => e.set_respect_ignore(on, true),
            Backend::Child(c) => {
                let _ = c.set_respect_ignore(on);
            }
        }
    }

    /// Turn following of symbolic links on/off (in-process engine, or the child).
    pub fn set_follow_symlinks(&self, on: bool) {
        match self {
            Backend::Local(e) => e.set_follow_symlinks(on, true),
            Backend::Child(c) => {
                let _ = c.set_follow_symlinks(on);
            }
        }
    }

    /// Pause or resume live indexing (in-process engine, or the child).
    pub fn set_paused(&self, on: bool) {
        match self {
            Backend::Local(e) => e.set_paused(on),
            Backend::Child(c) => {
                let _ = c.set_paused(on);
            }
        }
    }

    /// Replace the excluded-directory list (in-process engine, or the child).
    /// The engine resolves `~`/relative entries and rebuilds its index.
    pub fn set_exclude_dirs(&self, dirs: Vec<String>) {
        match self {
            Backend::Local(e) => e.set_exclude_dirs(dirs, true),
            Backend::Child(c) => {
                let _ = c.set_exclude_dirs(dirs);
            }
        }
    }

    /// Turn the optional background content cache on/off. Switching it on is
    /// lazy, so no rebuild follows.
    pub fn set_content_index(&self, on: bool) {
        match self {
            Backend::Local(e) => e.set_content_index(on),
            Backend::Child(c) => {
                let _ = c.set_content_index(on);
            }
        }
    }

    /// Number of entries matching `q`, without shipping them (used for the
    /// sidebar's per-category counts).
    pub fn count(&self, q: &Query) -> Result<u64, String> {
        match self {
            Backend::Local(e) => e.count(q),
            Backend::Child(c) => c.count(q),
        }
    }

    /// Block until the index is live (bounded by `timeout`).
    pub fn wait_live(&self, timeout: Duration) -> bool {
        match self {
            Backend::Local(e) => e.wait_live(timeout),
            Backend::Child(c) => c.wait_live(timeout),
        }
    }
}
