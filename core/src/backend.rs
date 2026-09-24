//! Search backend: the engine either in-process or behind the daemon API.
//!
//! One type covers both, so the GUI and CLI can switch between "link the engine
//! directly" and "talk to `everything-daemon`" without changing their call
//! sites. See `docs/api.md`.

use crate::config::Config;
use crate::engine::{Engine, SearchResponse, State, Status};
use crate::matcher::Query;
use crate::remote::Remote;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Backend {
    /// The engine lives in this process (zero IPC).
    Local(Arc<Engine>),
    /// The engine lives in `everything-daemon`; queries go over HTTP.
    Remote(Remote),
}

impl Backend {
    /// Build and start a local engine.
    pub fn local(config: Config) -> Backend {
        let mut engine = Engine::new(config);
        engine.start();
        Backend::Local(Arc::new(engine))
    }

    /// Talk to a daemon at `addr` (e.g. `127.0.0.1:5858`).
    pub fn remote(addr: impl Into<String>) -> Backend {
        Backend::Remote(Remote::new(addr))
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }

    /// Short human label for the status bar.
    pub fn label(&self) -> String {
        match self {
            Backend::Local(_) => "in-process".to_string(),
            Backend::Remote(r) => format!("daemon {}", r.addr()),
        }
    }

    /// False when a remote daemon cannot be reached (always true in-process).
    pub fn connected(&self) -> bool {
        match self {
            Backend::Local(_) => true,
            Backend::Remote(r) => r.connected(),
        }
    }

    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        match self {
            Backend::Local(e) => e.search(q),
            Backend::Remote(r) => r.search(q),
        }
    }

    pub fn status_snapshot(&self) -> Status {
        match self {
            Backend::Local(e) => e.status_snapshot(),
            Backend::Remote(r) => r.status(),
        }
    }

    pub fn counts(&self) -> (u64, u64) {
        match self {
            Backend::Local(e) => e.counts(),
            Backend::Remote(r) => r
                .report()
                .map(|rep| (rep.files, rep.dirs))
                .unwrap_or((0, 0)),
        }
    }

    pub fn rebuild(&self) {
        match self {
            Backend::Local(e) => e.rebuild(),
            Backend::Remote(r) => {
                let _ = r.rebuild();
            }
        }
    }

    /// Number of entries matching `q`, without shipping them (used for the
    /// sidebar's per-category counts).
    pub fn count(&self, q: &Query) -> Result<u64, String> {
        match self {
            Backend::Local(e) => e.count(q),
            Backend::Remote(r) => r.count(q),
        }
    }

    /// Block until the index is live (bounded by `timeout`).
    pub fn wait_live(&self, timeout: Duration) -> bool {
        match self {
            Backend::Local(e) => e.wait_live(timeout),
            Backend::Remote(_) => {
                let deadline = Instant::now() + timeout;
                while Instant::now() < deadline {
                    if self.status_snapshot().state == State::Live {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                false
            }
        }
    }
}
