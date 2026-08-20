//! Engine: owns the shared index, watcher, walker and content cache, and
//! answers queries. GUI and CLI link this directly (zero IPC).

use crate::config::Config;
use crate::content::{search_contents, ContentPattern};
use crate::content_index::{spawn_extractor, ContentIndex, ExtractQueue};
use crate::index::{Index, Meta};
use crate::matcher::{is_hidden, CompiledQuery, Query};
use crate::roots::RootSet;
use crate::walker::{walk_root, walk_root_into};
use crate::watcher;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// One result row for the UI / CLI.
#[derive(Clone, Debug)]
pub struct ResultRow {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct SearchResponse {
    pub results: Vec<ResultRow>,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// Number of indexed files at query time.
    pub indexed: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Starting,
    Indexing,
    Live,
}

#[derive(Clone, Debug)]
pub enum ContentIndexStatus {
    Disabled,
    Enabled { entries: usize, bytes: u64, pending: usize },
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    pub degraded: bool,
    pub skipped: u64,
    pub content_index: ContentIndexStatus,
}

/// Upper bound on how many name-matched candidates a content query will fan
/// out over, to keep interactive queries responsive.
const CONTENT_FANOUT_CAP: usize = 50_000;

pub struct Engine {
    config: Config,
    index: Arc<RwLock<Index>>,
    status: Arc<RwLock<Status>>,
    roots: Arc<RootSet>,
    cache: Arc<ContentIndex>,
    queue: Option<Arc<ExtractQueue>>,
    pending: Arc<AtomicUsize>,
}

impl Engine {
    pub fn new(config: Config) -> Engine {
        let roots = Arc::new(RootSet::discover(&config));
        let status = Arc::new(RwLock::new(Status {
            state: State::Starting,
            degraded: false,
            skipped: 0,
            content_index: ContentIndexStatus::Disabled,
        }));
        let cache = Arc::new(ContentIndex::new(
            config.content_index_enabled,
            config.content_index_max_file_bytes,
            config.content_index_total_cap_bytes,
        ));
        let pending = Arc::new(AtomicUsize::new(0));
        let queue = if config.content_index_enabled {
            let (q, rx) = ExtractQueue::new();
            spawn_extractor(Arc::clone(&cache), rx, Arc::clone(&pending));
            Some(Arc::new(q))
        } else {
            None
        };
        Engine {
            config,
            index: Arc::new(RwLock::new(Index::new())),
            status,
            roots,
            cache,
            queue,
            pending,
        }
    }

    /// Start the cold walk and the realtime watcher.
    pub fn start(&mut self) {
        self.start_walk();
        self.start_watcher();
    }

    fn start_walk(&mut self) {
        {
            let mut s = self.status.write().unwrap();
            s.state = State::Indexing;
            s.skipped = 0;
        }
        let index = Arc::clone(&self.index);
        let status = Arc::clone(&self.status);
        let roots = Arc::clone(&self.roots);
        let queue = self.queue.clone();
        let respect_ignore = self.config.respect_ignore_files;
        let walk_handle = std::thread::Builder::new()
            .name("index-walk".into())
            .spawn(move || {
                for root in &roots.roots {
                    walk_root(root, &index, &roots, queue.as_ref(), &status, respect_ignore);
                }
                status.write().unwrap().state = State::Live;
            })
            .expect("failed to spawn walk thread");
        drop(walk_handle); // detached: the thread runs until process exit
    }

    fn start_watcher(&mut self) {
        let index = Arc::clone(&self.index);
        let status = Arc::clone(&self.status);
        let roots = Arc::clone(&self.roots);
        let cache = Arc::clone(&self.cache);
        let queue = self.queue.clone();
        let secs = self.config.degraded_rescan_secs.max(5);

        // Degraded mode: the kernel watcher failed (usually exhausted watch
        // limits). Switch to periodic full rescans of the roots.
        let started = Arc::new(Mutex::new(false));
        let on_error = {
            let index = Arc::clone(&self.index);
            let status = Arc::clone(&self.status);
            let roots = Arc::clone(&self.roots);
            let queue = self.queue.clone();
            let started = Arc::clone(&started);
            let respect_ignore = self.config.respect_ignore_files;
            Arc::new(move || {
                status.write().unwrap().degraded = true;
                let mut already = started.lock().unwrap();
                if *already {
                    return;
                }
                *already = true;
                drop(already);
                // Clone into the rescan thread's environment (the outer
                // closure is Fn and may be invoked repeatedly).
                let index = Arc::clone(&index);
                let status = Arc::clone(&status);
                let roots = Arc::clone(&roots);
                let queue = queue.clone();
                std::thread::Builder::new()
                    .name("rescan".into())
                    .spawn(move || loop {
                        std::thread::sleep(Duration::from_secs(secs));
                        let seen = Arc::new(Mutex::new(HashSet::new()));
                        for root in &roots.roots {
                            walk_root_into(
                                root,
                                &index,
                                &roots,
                                queue.as_ref(),
                                &status,
                                Arc::clone(&seen),
                                respect_ignore,
                            );
                        }
                        let seen = Arc::try_unwrap(seen)
                            .expect("seen has extra refs")
                            .into_inner()
                            .unwrap();
                        index.write().unwrap().retain_known(&seen);
                    })
                    .expect("failed to spawn rescan thread");
            })
        };

        let handle = watcher::start_watcher(
            self.roots.roots.clone(),
            index,
            roots,
            cache,
            queue,
            status,
            self.config.respect_ignore_files,
            on_error,
        );
        drop(handle); // detached: the thread runs until process exit
    }

    /// A full rescan (e.g. after a system restore or if the index looks stale).
    pub fn rebuild(&mut self) {
        self.start_walk();
    }

    /// Block until the initial walk finishes (bounded by `timeout`).
    pub fn wait_live(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(s) = self.status.read() {
                if s.state == State::Live {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn status_snapshot(&self) -> Status {
        let mut s = self.status.read().unwrap().clone();
        s.content_index = if self.cache.enabled() {
            let (entries, bytes) = self.cache.stats();
            ContentIndexStatus::Enabled {
                entries,
                bytes,
                pending: self.pending.load(Ordering::Relaxed),
            }
        } else {
            ContentIndexStatus::Disabled
        };
        s
    }

    /// Run one query against the live index.
    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        let t0 = Instant::now();
        let cq = CompiledQuery::compile(q)?;
        let limit = cq.limit;
        let want_content = cq.content.is_some();

        // Phase 1: name matching against the in-memory index (fast).
        let name_hits: Vec<PathBuf> = {
            let idx = self.index.read().unwrap();
            let cap = if want_content { limit.max(CONTENT_FANOUT_CAP) } else { limit };
            let mut hits = Vec::new();
            for e in idx.entries() {
                if !cq.include_hidden && is_hidden(&e.path) {
                    continue;
                }
                if cq.has_name_filter && !cq.name_matches(&e.path) {
                    continue;
                }
                hits.push(e.path.clone());
                if hits.len() >= cap {
                    break;
                }
            }
            hits
        };

        let (results, truncated) = if want_content {
            let pattern = ContentPattern::new(cq.content.as_deref().unwrap_or(""))?;
            let paths: Vec<PathBuf> = if cq.has_name_filter {
                name_hits
            } else {
                let idx = self.index.read().unwrap();
                idx.file_paths()
            };
            let matched = search_contents(&paths, &pattern, limit, &self.cache, self.queue.as_deref());
            let truncated = matched.len() >= limit;
            (
                matched.into_iter().map(|p| self.row_for(&p)).collect(),
                truncated,
            )
        } else {
            let truncated = name_hits.len() >= limit;
            (
                name_hits.into_iter().take(limit).map(|p| self.row_for(&p)).collect(),
                truncated,
            )
        };

        Ok(SearchResponse {
            results,
            truncated,
            elapsed_ms: t0.elapsed().as_millis() as u64,
            indexed: self.index.read().unwrap().files,
        })
    }

    fn row_for(&self, path: &Path) -> ResultRow {
        let meta: Meta = self
            .index
            .read()
            .unwrap()
            .get(path)
            .map(|e| e.meta)
            .unwrap_or_default();
        ResultRow {
            path: path.to_path_buf(),
            size: meta.size,
            mtime: meta.mtime,
            is_dir: meta.is_dir,
        }
    }

    /// (files, dirs) counts for the status bar.
    pub fn counts(&self) -> (u64, u64) {
        let idx = self.index.read().unwrap();
        (idx.files, idx.dirs)
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &Config {
        &self.config
    }
}
