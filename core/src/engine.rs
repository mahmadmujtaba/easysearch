//! Engine: owns the disk-backed base index, the in-memory change overlay, the
//! watcher, the content cache — and answers queries. GUI and CLI link this
//! directly (zero IPC).
//!
//! Memory model: the bulk of the index lives in a memory-mapped file (kernel
//! page cache, reclaimable, ~zero RSS when cold); only the small hash table
//! and the recent-change overlay are always resident. See docs/scope.md §9.

use crate::config::Config;
use crate::content::{search_contents, ContentPattern};
use crate::content_index::{spawn_extractor, ContentIndex, ExtractQueue};
use crate::disk_index::{DiskIndex, INDEX_FILE};
use crate::matcher::{is_hidden, CompiledQuery, Query};
use crate::overlay::{Meta, Overlay};
use crate::roots::RootSet;
use crate::walker::{walk_root_apply, walk_root_collect};
use crate::watcher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
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
    /// Number of indexed files at query time (best effort).
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
    /// Pending overlay changes (awaiting compaction into the disk index).
    pub overlay_pending: usize,
    /// Entries in the disk-backed base (0 in RAM-only mode).
    pub base_entries: usize,
    pub base_files: u64,
    pub base_dirs: u64,
}

/// Upper bound on how many name-matched candidates a content query will fan
/// out over, to keep interactive queries responsive.
const CONTENT_FANOUT_CAP: usize = 50_000;

pub struct Engine {
    config: Config,
    base: Arc<RwLock<Option<Arc<DiskIndex>>>>,
    overlay: Arc<RwLock<Overlay>>,
    status: Arc<RwLock<Status>>,
    roots: Arc<RootSet>,
    cache: Arc<ContentIndex>,
    queue: Option<Arc<ExtractQueue>>,
    pending: Arc<AtomicUsize>,
    rebuilding: Arc<AtomicBool>,
    index_path: PathBuf,
}

impl Engine {
    pub fn new(config: Config) -> Engine {
        let roots = Arc::new(RootSet::discover(&config));
        let status = Arc::new(RwLock::new(Status {
            state: State::Starting,
            degraded: false,
            skipped: 0,
            content_index: ContentIndexStatus::Disabled,
            overlay_pending: 0,
            base_entries: 0,
            base_files: 0,
            base_dirs: 0,
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

        let base = Arc::new(RwLock::new(None));
        let index_path = if config.persist_index {
            config.disk_index_dir().join(INDEX_FILE)
        } else {
            PathBuf::new()
        };
        if config.persist_index {
            // Serve from yesterday's index immediately; a background rebuild
            // re-validates it against the live filesystem.
            match DiskIndex::load(&index_path) {
                Ok(idx) => *base.write().unwrap() = Some(Arc::new(idx)),
                Err(_) => {} // no cache yet / stale: build at startup
            }
        }

        Engine {
            config,
            base,
            overlay: Arc::new(RwLock::new(Overlay::new())),
            status,
            roots,
            cache,
            queue,
            pending,
            rebuilding: Arc::new(AtomicBool::new(false)),
            index_path,
        }
    }

    /// Start the watcher and the initial index build.
    pub fn start(&mut self) {
        self.start_watcher();

        let has_cache = self.config.persist_index && self.base.read().unwrap().is_some();
        if has_cache {
            // Searchable immediately; refresh the base in the background.
            self.status.write().unwrap().state = State::Live;
            self.spawn_rebuild();
        } else {
            let roots = Arc::clone(&self.roots);
            let status = Arc::clone(&self.status);
            let queue = self.queue.clone();
            let overlay = Arc::clone(&self.overlay);
            let base = Arc::clone(&self.base);
            let respect_ignore = self.config.respect_ignore_files;
            let persist = self.config.persist_index;
            let index_path = self.index_path.clone();
            std::thread::Builder::new()
                .name("index-build".into())
                .spawn(move || {
                    status.write().unwrap().state = State::Indexing;
                    if persist {
                        let entries = build_entries(&roots, queue.as_ref(), &status, respect_ignore);
                        match DiskIndex::write_and_load(&index_path, &entries) {
                            Ok(idx) => *base.write().unwrap() = Some(Arc::new(idx)),
                            Err(e) => eprintln!("initial index build failed: {e}"),
                        }
                    } else {
                        for root in &roots.roots {
                            walk_root_apply(
                                root,
                                &overlay,
                                &roots,
                                queue.as_ref(),
                                &status,
                                respect_ignore,
                            );
                        }
                    }
                    status.write().unwrap().state = State::Live;
                })
                .expect("failed to spawn index-build thread");
        }
    }

    fn start_watcher(&mut self) {
        let overlay = Arc::clone(&self.overlay);
        let status = Arc::clone(&self.status);
        let roots = Arc::clone(&self.roots);
        let cache = Arc::clone(&self.cache);
        let queue = self.queue.clone();
        let secs = self.config.degraded_rescan_secs.max(5);
        let respect_ignore = self.config.respect_ignore_files;

        // Degraded mode: the kernel watcher failed (usually exhausted watch
        // limits). Fall back to periodic full rebuilds.
        let started = Arc::new(AtomicBool::new(false));
        let on_error = {
            let status = Arc::clone(&self.status);
            let started = Arc::clone(&started);
            let index_path = self.index_path.clone();
            let roots = Arc::clone(&self.roots);
            let base = Arc::clone(&self.base);
            let overlay = Arc::clone(&self.overlay);
            let queue = self.queue.clone();
            let rebuilding = Arc::clone(&self.rebuilding);
            let persist = self.config.persist_index;
            let respect_ignore = self.config.respect_ignore_files;
            Arc::new(move || {
                status.write().unwrap().degraded = true;
                if started.swap(true, Ordering::SeqCst) {
                    return;
                }
                // Clone into the rescan thread's environment (the outer
                // closure is Fn and may be invoked repeatedly).
                let index_path = index_path.clone();
                let roots = Arc::clone(&roots);
                let base = Arc::clone(&base);
                let overlay = Arc::clone(&overlay);
                let queue = queue.clone();
                let rebuilding = Arc::clone(&rebuilding);
                let status = Arc::clone(&status);
                std::thread::Builder::new()
                    .name("rescan".into())
                    .spawn(move || loop {
                        std::thread::sleep(Duration::from_secs(secs));
                        if persist {
                            rebuild_once(
                                &index_path,
                                &roots,
                                &base,
                                &overlay,
                                queue.as_ref(),
                                &rebuilding,
                                &status,
                                respect_ignore,
                            );
                        } else {
                            for root in &roots.roots {
                                walk_root_apply(
                                    root,
                                    &overlay,
                                    &roots,
                                    queue.as_ref(),
                                    &status,
                                    respect_ignore,
                                );
                            }
                        }
                    })
                    .expect("failed to spawn rescan thread");
            })
        };

        let handle = watcher::start_watcher(
            self.roots.roots.clone(),
            overlay,
            roots,
            cache,
            queue,
            status,
            respect_ignore,
            on_error,
        );
        drop(handle); // detached: the thread runs until process exit
    }

    /// Trigger a background compaction/rebuild of the disk index.
    pub fn rebuild(&self) {
        if self.config.persist_index {
            self.spawn_rebuild();
        }
    }

    fn spawn_rebuild(&self) {
        let roots = Arc::clone(&self.roots);
        let status = Arc::clone(&self.status);
        let base = Arc::clone(&self.base);
        let overlay = Arc::clone(&self.overlay);
        let queue = self.queue.clone();
        let rebuilding = Arc::clone(&self.rebuilding);
        let respect_ignore = self.config.respect_ignore_files;
        let index_path = self.index_path.clone();
        std::thread::Builder::new()
            .name("index-rebuild".into())
            .spawn(move || {
                rebuild_once(
                    &index_path,
                    &roots,
                    &base,
                    &overlay,
                    queue.as_ref(),
                    &rebuilding,
                    &status,
                    respect_ignore,
                );
            })
            .expect("failed to spawn rebuild thread");
    }

    /// Compaction trigger: fold a large change overlay back into the disk file.
    fn maybe_compact(&self) {
        if !self.config.persist_index || self.rebuilding.load(Ordering::Relaxed) {
            return;
        }
        let pending = self.overlay.read().unwrap().pending_changes();
        if pending > self.config.overlay_compaction_threshold {
            self.spawn_rebuild();
        }
    }

    /// Block until the initial build finishes (bounded by `timeout`).
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
        s.overlay_pending = self.overlay.read().unwrap().pending_changes();
        match self.base.read().unwrap().as_deref() {
            Some(b) => {
                s.base_entries = b.len();
                s.base_files = b.files();
                s.base_dirs = b.dirs();
            }
            None => {
                s.base_entries = 0;
                s.base_files = 0;
                s.base_dirs = 0;
            }
        }
        s
    }

    /// Run one query against the live index (base + overlay).
    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        let t0 = Instant::now();
        let cq = CompiledQuery::compile(q)?;
        let limit = cq.limit;
        let want_content = cq.content.is_some();
        self.maybe_compact();

        let cap = if want_content { limit.max(CONTENT_FANOUT_CAP) } else { limit };
        let name_filter = |p: &Path, meta: Meta| -> Option<(PathBuf, Meta)> {
            if !cq.include_hidden && is_hidden(p) {
                return None;
            }
            if cq.has_name_filter && !cq.name_matches(p) {
                return None;
            }
            Some((p.to_path_buf(), meta))
        };

        let (results, truncated) = if want_content {
            let pattern = ContentPattern::new(cq.content.as_deref().unwrap_or(""))?;
            let candidates: Vec<PathBuf> = if cq.has_name_filter {
                self.collect(&name_filter, cap)
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect()
            } else {
                // content-only: every indexed file
                self.collect(
                    &|p, meta| (!meta.is_dir).then(|| (p.to_path_buf(), meta)),
                    cap,
                )
                .into_iter()
                .map(|(p, _)| p)
                .collect()
            };
            let matched =
                search_contents(&candidates, &pattern, limit, &self.cache, self.queue.as_deref());
            let truncated = matched.len() >= limit;
            (
                matched.into_iter().map(|p| self.row_for(&p)).collect(),
                truncated,
            )
        } else {
            let hits = self.collect(&name_filter, cap);
            let truncated = hits.len() >= limit;
            (
                hits.into_iter()
                    .take(limit)
                    .map(|(p, m)| ResultRow {
                        path: p,
                        size: m.size,
                        mtime: m.mtime,
                        is_dir: m.is_dir,
                    })
                    .collect(),
                truncated,
            )
        };

        Ok(SearchResponse {
            results,
            truncated,
            elapsed_ms: t0.elapsed().as_millis() as u64,
            indexed: self.counts().0,
        })
    }

    /// Iterate the base index (skipping overlay-removed paths, applying
    /// overlay metadata) plus overlay-only additions, keeping entries for
    /// which `f` returns `Some`. Bounded by `cap`.
    fn collect<F>(&self, f: &F, cap: usize) -> Vec<(PathBuf, Meta)>
    where
        F: Fn(&Path, Meta) -> Option<(PathBuf, Meta)>,
    {
        let mut out: Vec<(PathBuf, Meta)> = Vec::new();
        {
            let base = self.base.read().unwrap();
            let ov = self.overlay.read().unwrap();
            if let Some(b) = base.as_deref() {
                for i in 0..b.len() {
                    let (cow, meta) = b.entry(i);
                    let p = Path::new(cow.as_ref());
                    if ov.is_removed(p) {
                        continue;
                    }
                    let meta = ov.added.get(p).copied().unwrap_or(meta);
                    if let Some(item) = f(p, meta) {
                        out.push(item);
                        if out.len() >= cap {
                            return out;
                        }
                    }
                }
            }
            for (path, meta) in ov.added.iter() {
                if ov.removed.contains(path) {
                    continue;
                }
                let in_base = base.as_deref().map_or(false, |b| b.contains(path));
                if in_base {
                    continue;
                }
                if let Some(item) = f(path, *meta) {
                    out.push(item);
                    if out.len() >= cap {
                        break;
                    }
                }
            }
        }
        out
    }

    fn row_for(&self, path: &Path) -> ResultRow {
        let m = self.meta_of(path);
        ResultRow {
            path: path.to_path_buf(),
            size: m.size,
            mtime: m.mtime,
            is_dir: m.is_dir,
        }
    }

    fn meta_of(&self, path: &Path) -> Meta {
        let base = self.base.read().unwrap();
        let ov = self.overlay.read().unwrap();
        if let Some(m) = ov.added.get(path) {
            return *m;
        }
        if ov.is_removed(path) {
            return Meta::default();
        }
        base.as_deref().and_then(|b| b.meta_of(path)).unwrap_or_default()
    }

    /// (files, dirs) — base plus overlay deltas.
    pub fn counts(&self) -> (u64, u64) {
        let base = self.base.read().unwrap();
        let ov = self.overlay.read().unwrap();
        let b = base.as_deref();
        let (mut files, mut dirs) = b.map(|b| (b.files(), b.dirs())).unwrap_or((0, 0));
        for (p, m) in &ov.added {
            if b.map_or(true, |b| !b.contains(p)) {
                if m.is_dir {
                    dirs += 1;
                } else {
                    files += 1;
                }
            }
        }
        for p in &ov.removed {
            if let Some(m) = b.and_then(|b| b.meta_of(p)) {
                if m.is_dir {
                    dirs = dirs.saturating_sub(1);
                } else {
                    files = files.saturating_sub(1);
                }
            }
        }
        (files, dirs)
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

/// Walk all roots and collect every entry (used to build the disk index).
fn build_entries(
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
) -> Vec<(PathBuf, Meta)> {
    let mut out = Vec::new();
    for root in &roots.roots {
        out.extend(walk_root_collect(root, roots, queue, status, respect_ignore));
    }
    out
}

/// One background compaction: walk live, write the disk index, swap the base,
/// and prune the overlay to true deltas.
fn rebuild_once(
    index_path: &Path,
    roots: &Arc<RootSet>,
    base: &RwLock<Option<Arc<DiskIndex>>>,
    overlay: &RwLock<Overlay>,
    queue: Option<&Arc<ExtractQueue>>,
    rebuilding: &AtomicBool,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
) {
    if rebuilding.swap(true, Ordering::SeqCst) {
        return;
    }
    let entries = build_entries(roots, queue, status, respect_ignore);
    match DiskIndex::write_and_load(index_path, &entries) {
        Ok(idx) => {
            let idx = Arc::new(idx);
            *base.write().unwrap() = Some(Arc::clone(&idx));
            overlay
                .write()
                .unwrap()
                .prune_against(|p| idx.contains(p));
        }
        Err(e) => eprintln!("index rebuild failed: {e}"),
    }
    rebuilding.store(false, Ordering::SeqCst);
}
