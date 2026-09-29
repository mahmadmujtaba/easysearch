//! Engine: owns the disk-backed base index, the in-memory change overlay, the
//! watcher, the content cache — and answers queries. GUI and CLI link this
//! directly (zero IPC).
//!
//! Memory model: the bulk of the index lives in a memory-mapped file (kernel
//! page cache, reclaimable, ~zero RSS when cold); only the small hash table
//! and the recent-change overlay are always resident. See docs/scope.md §9.

use crate::config::{Config, Storage};
use crate::content::{ContentPattern, search_contents};
use crate::content_index::{ContentIndex, ContentStore, ExtractQueue, spawn_extractor};
use crate::disk_index::{DiskIndex, INDEX_FILE};
use crate::matcher::{CompiledQuery, Query, is_hidden, matches_category};
use crate::overlay::{Meta, Overlay};
use crate::roots::RootSet;
use crate::sqlite_index::{DB_FILE, REFRESH_AFTER_DIRTY, SqliteIndex};
use crate::walker::{self, walk_root_apply, walk_root_collect};
use crate::watcher;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// One result row for the UI / CLI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultRow {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<ResultRow>,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// Number of indexed files at query time (best effort).
    pub indexed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    Starting,
    Indexing,
    Live,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentIndexStatus {
    Disabled,
    Enabled {
        entries: usize,
        bytes: u64,
        pending: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub degraded: bool,
    pub skipped: u64,
    /// Directories that could not be watched (0 = fully realtime).
    pub watch_failures: u64,
    pub content_index: ContentIndexStatus,
    /// Pending overlay changes (awaiting compaction into the disk index).
    pub overlay_pending: usize,
    /// Entries in the disk-backed base (0 in RAM-only mode).
    pub base_entries: usize,
    pub base_files: u64,
    pub base_dirs: u64,
    /// Whether `.gitignore`/`.ignore` files (and the global ignore file) are
    /// honoured while walking. Live-toggleable via [`Engine::set_respect_ignore`].
    #[serde(default)]
    pub respect_ignore_files: bool,
    /// Whether symbolic links are followed into their targets. Live-toggleable
    /// via [`Engine::set_follow_symlinks`].
    #[serde(default)]
    pub follow_symlinks: bool,
    /// Unix time of the last completed index build/rebuild (0 = unknown, e.g. a
    /// start that served a cached index without re-walking).
    #[serde(default)]
    pub last_index_at: i64,
    /// Live indexing is paused (the watcher's changes and background rebuilds are
    /// held back). Live-toggleable via [`Engine::set_paused`].
    #[serde(default)]
    pub paused: bool,
    /// The index roots in force (as configured; `~` and relative paths shown
    /// unresolved). Live-editable via [`Engine::set_roots`].
    #[serde(default)]
    pub roots: Vec<String>,
    /// Directory subtrees excluded from the index (as configured; `~` and
    /// relative paths are shown unresolved). Live-editable via
    /// [`Engine::set_exclude_dirs`].
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            state: State::Starting,
            degraded: false,
            skipped: 0,
            watch_failures: 0,
            content_index: ContentIndexStatus::Disabled,
            overlay_pending: 0,
            base_entries: 0,
            base_files: 0,
            base_dirs: 0,
            respect_ignore_files: true,
            follow_symlinks: false,
            last_index_at: 0,
            paused: false,
            roots: Vec::new(),
            exclude_dirs: Vec::new(),
        }
    }
}

/// Upper bound on how many name-matched candidates a content query will fan
/// out over, to keep interactive queries responsive.
const CONTENT_FANOUT_CAP: usize = 50_000;

pub struct Engine {
    config: Config,
    base: Arc<RwLock<Option<Arc<DiskIndex>>>>,
    /// The SQLite index, when `config.storage == Storage::Sqlite`. The engine
    /// owns it: it is the only writer, and every query is answered from here.
    sqlite: Option<Arc<Mutex<SqliteIndex>>>,
    overlay: Arc<RwLock<Overlay>>,
    status: Arc<RwLock<Status>>,
    roots: Arc<RootSet>,
    cache: Arc<ContentIndex>,
    queue: Option<Arc<ExtractQueue>>,
    pending: Arc<AtomicUsize>,
    rebuilding: Arc<AtomicBool>,
    index_path: PathBuf,
    /// Cached (files, dirs) for the SQLite backend: `counts()` runs on every UI
    /// frame, and an aggregate over the whole table is not per-frame work.
    counts_cache: Arc<RwLock<((u64, u64), Instant)>>,
    /// Whether ignore files are honoured. Read once per walk/rebuild, so a change
    /// takes effect on the next rebuild; see [`Engine::set_respect_ignore`].
    respect_ignore: Arc<AtomicBool>,
    /// Whether symbolic links are followed. Same live-toggle semantics.
    follow_symlinks: Arc<AtomicBool>,
    /// Live indexing is paused. Read by the watcher (skip changes) and by the
    /// background rebuilds (held back). See [`Engine::set_paused`].
    paused: Arc<AtomicBool>,
}

/// Where the optional content cache keeps the text it extracts.
///
/// Disk by default: a content search then reads a document back, uses it and
/// drops it, so searching contents does not leave the documents it read sitting
/// in memory. `--content-in-memory` asks for the RAM store instead.
fn content_store(config: &Config) -> ContentStore {
    if config.content_index_in_memory {
        ContentStore::Memory
    } else {
        ContentStore::Disk
    }
}

impl Engine {
    pub fn new(config: Config) -> Engine {
        let roots = Arc::new(RootSet::discover(&config));
        // Live switch for the optional content cache (see `set_content_index`).
        let content_enabled = Arc::new(AtomicBool::new(config.content_index_enabled));
        let status = Arc::new(RwLock::new(Status {
            state: State::Starting,
            degraded: false,
            skipped: 0,
            watch_failures: 0,
            content_index: ContentIndexStatus::Disabled,
            overlay_pending: 0,
            base_entries: 0,
            base_files: 0,
            base_dirs: 0,
            respect_ignore_files: config.respect_ignore_files,
            follow_symlinks: config.follow_symlinks,
            last_index_at: 0,
            paused: false,
            roots: config.roots.clone(),
            exclude_dirs: config.exclude_dirs.clone(),
        }));
        let cache = Arc::new(ContentIndex::new(
            Arc::clone(&content_enabled),
            config.content_index_max_file_bytes,
            config.content_index_total_cap_bytes,
            content_store(&config),
            config.content_spool_dir(),
        ));
        let pending = Arc::new(AtomicUsize::new(0));
        // Created regardless of the switch (which can be flipped live, see
        // `set_content_index`): while it is off nothing is queued and the
        // extractor thread stays parked, so an idle cache still costs nothing.
        let (extract_queue, extract_rx) =
            ExtractQueue::new(Arc::clone(&content_enabled), Arc::clone(&pending));
        spawn_extractor(Arc::clone(&cache), extract_rx, Arc::clone(&pending));
        let queue = Some(Arc::new(extract_queue));

        let base = Arc::new(RwLock::new(None));
        let index_path = if config.persist_index {
            config.disk_index_dir().join(INDEX_FILE)
        } else {
            PathBuf::new()
        };

        // Cached (files, dirs); seeded from the database below so the very first
        // frame reports the real numbers.
        let counts_cache = Arc::new(RwLock::new(((0u64, 0u64), Instant::now())));

        // SQLite backend: open (creating the database if missing). The initial
        // fill happens on a background thread in `start`, like the mmap build.
        // RAM-only mode (`persist_index = false`) means “keep no index on disk”,
        // so a disk database is deliberately not opened in that mode.
        let sqlite = if config.storage == Storage::Sqlite && config.persist_index {
            match SqliteIndex::open(&config.db_dir()) {
                Ok(idx) => {
                    // Serve from the existing database immediately — `start`
                    // re-validates it in the background, exactly like the mmap
                    // path below. Without this the first frames would report an
                    // empty index even though the data is already on disk.
                    if let Ok((files, dirs)) = idx.counts() {
                        *counts_cache.write().unwrap() = ((files, dirs), Instant::now());
                        let mut s = status.write().unwrap();
                        s.base_files = files;
                        s.base_dirs = dirs;
                        s.base_entries = (files + dirs) as usize;
                    }
                    Some(Arc::new(Mutex::new(idx)))
                }
                Err(e) => {
                    eprintln!("sqlite index unavailable ({e}); falling back to the mmap index");
                    None
                }
            }
        } else {
            None
        };

        if config.persist_index && sqlite.is_none() {
            // Serve from yesterday's index immediately; a background rebuild
            // re-validates it against the live filesystem.
            if let Ok(idx) = DiskIndex::load(&index_path) {
                *base.write().unwrap() = Some(Arc::new(idx));
            }
            // No cache yet / stale: build at startup.
        }

        let respect_ignore_files = config.respect_ignore_files;
        let follow_symlinks = config.follow_symlinks;

        Engine {
            config,
            base,
            sqlite,
            overlay: Arc::new(RwLock::new(Overlay::new())),
            status,
            roots,
            cache,
            queue,
            pending,
            rebuilding: Arc::new(AtomicBool::new(false)),
            index_path,
            counts_cache,
            respect_ignore: Arc::new(AtomicBool::new(respect_ignore_files)),
            follow_symlinks: Arc::new(AtomicBool::new(follow_symlinks)),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Move whatever the watcher has collected into the database.
    ///
    /// One transaction per batch, which is why the overlay exists: per-event
    /// commits to SQLite would be an order of magnitude slower.
    fn flush_overlay(&self) {
        let Some(db) = &self.sqlite else {
            return;
        };
        if self.overlay.read().unwrap().pending_changes() == 0 {
            return;
        }
        // Lock order is always database-then-overlay (see `spawn_rebuild`).
        let Ok(mut db) = db.lock() else { return };
        let (added, removed) = self.overlay.write().unwrap().take();
        if let Err(e) = db.apply(&added, &removed) {
            eprintln!(
                "sqlite: applying {} change(s) failed: {e}",
                added.len() + removed.len()
            );
        }
    }

    /// Whether the SQLite backend is active.
    pub fn uses_sqlite(&self) -> bool {
        self.sqlite.is_some()
    }

    /// Turn honoring of `.gitignore`/`.ignore` files (and the global ignore file)
    /// on or off. `rebuild` folds the change into the index right away.
    pub fn set_respect_ignore(&self, on: bool, rebuild: bool) {
        self.respect_ignore.store(on, Ordering::Relaxed);
        if let Ok(mut s) = self.status.write() {
            s.respect_ignore_files = on;
        }
        if rebuild {
            self.rebuild();
        }
    }

    /// Current value of the ignore-files setting.
    pub fn respect_ignore(&self) -> bool {
        self.respect_ignore.load(Ordering::Relaxed)
    }

    /// Pause or resume live indexing.
    ///
    /// While paused the watcher ignores filesystem events and background rebuilds
    /// are held back, so the index stops churning (useful on battery or when the
    /// machine is hot). Resuming re-syncs the index, since changes made while
    /// paused were not observed.
    pub fn set_paused(&self, on: bool) {
        let was = self.paused.swap(on, Ordering::SeqCst);
        if let Ok(mut s) = self.status.write() {
            s.paused = on;
        }
        if was && !on {
            self.resync();
        }
    }

    /// Whether live indexing is paused.
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Bring the index back in step with the filesystem after a pause.
    fn resync(&self) {
        if self.config.persist_index {
            self.spawn_rebuild(true);
            return;
        }
        // RAM-only: re-walk into the overlay, as the degraded rescan does.
        let roots = Arc::clone(&self.roots);
        let status = Arc::clone(&self.status);
        let queue = self.queue.clone();
        let overlay = Arc::clone(&self.overlay);
        let opts = self.walk_options();
        std::thread::Builder::new()
            .name("resync".into())
            .spawn(move || {
                for root in &roots.roots() {
                    walk_root_apply(
                        root,
                        &overlay,
                        &roots,
                        queue.as_ref(),
                        &status,
                        opts.clone(),
                    );
                }
            })
            .ok();
    }

    /// Follow symbolic links into their targets (and rebuild so it takes effect).
    pub fn set_follow_symlinks(&self, on: bool, rebuild: bool) {
        self.follow_symlinks.store(on, Ordering::Relaxed);
        if let Ok(mut s) = self.status.write() {
            s.follow_symlinks = on;
        }
        if rebuild {
            self.rebuild();
        }
    }

    /// Current value of the follow-symlinks setting.
    pub fn follow_symlinks(&self) -> bool {
        self.follow_symlinks.load(Ordering::Relaxed)
    }

    /// Replace the excluded-directory list (resolved) and, when `rebuild`, fold
    /// the change into the index at once so the tree is re-walked without the
    /// excluded subtrees.
    pub fn set_exclude_dirs(&self, dirs: Vec<String>, rebuild: bool) {
        let mut config = self.config.clone();
        config.exclude_dirs = dirs;
        self.roots.set_exclude_dirs(config.effective_exclude_dirs());
        if let Ok(mut s) = self.status.write() {
            s.exclude_dirs = config.exclude_dirs;
        }
        if rebuild {
            self.rebuild();
        }
    }

    /// The excluded-directory list in force (resolved absolute paths).
    pub fn exclude_dirs(&self) -> Vec<PathBuf> {
        self.roots.exclude_dirs()
    }

    /// Replace the index roots (resolved by the engine) and, by default, rebuild
    /// so the index matches the new set at once.
    ///
    /// Live watching of a *newly added* root only begins at the next start; the
    /// rebuild already indexes its contents, so search results are correct
    /// immediately.
    pub fn set_roots(&self, roots: Vec<String>, rebuild: bool) {
        let mut config = self.config.clone();
        config.roots = roots;
        self.roots.set_roots(config.effective_roots());
        if let Ok(mut s) = self.status.write() {
            s.roots = config.roots.clone();
        }
        if rebuild {
            self.rebuild();
        }
    }

    /// The index roots in force (resolved absolute paths).
    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.roots()
    }

    /// The walk fingerprint over the *live* roots and exclusions (see
    /// [`Config::walk_fingerprint`]). A live change would otherwise look like a
    /// mismatch on the next start and trigger a needless rebuild.
    fn walk_key(&self) -> String {
        let mut config = self.config.clone();
        config.roots = self
            .roots
            .roots()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        config.exclude_dirs = self
            .roots
            .exclude_dirs()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        config.walk_fingerprint()
    }

    /// Turn the optional background content cache on or off.
    ///
    /// Switching it on is lazy (documents are cached as they are searched) and
    /// switching it off frees everything it held, so it costs nothing while off.
    /// No rebuild is needed either way, hence no `rebuild` parameter.
    pub fn set_content_index(&self, on: bool) {
        self.cache.set_enabled(on);
    }

    /// Whether the background content cache is on.
    pub fn content_index_enabled(&self) -> bool {
        self.cache.enabled()
    }

    /// The walk settings in force right now (read fresh for every walk).
    fn walk_options(&self) -> walker::WalkOptions {
        walker::WalkOptions::new(
            self.respect_ignore.load(Ordering::Relaxed),
            self.follow_symlinks.load(Ordering::Relaxed),
        )
        .with_skip_hidden_dirs(!self.config.index_hidden_dirs)
        .with_exclude_names(Arc::new(self.config.effective_exclude_names()))
    }

    /// Start the watcher and the initial index build.
    pub fn start(&mut self) {
        self.start_watcher();

        // SQLite backend: create the database if it is missing (or its schema
        // changed), otherwise serve from it at once and re-validate in the
        // background — the same shape as the mmap path below.
        if let Some(db) = self.sqlite.clone() {
            let walk_key = self.walk_key();
            let needs_rebuild = db
                .lock()
                .map(|d| d.needs_rebuild(&walk_key))
                .unwrap_or(true);
            if needs_rebuild {
                let roots = Arc::clone(&self.roots);
                let status = Arc::clone(&self.status);
                let queue = self.queue.clone();
                let opts = self.walk_options();
                let counts_cache = Arc::clone(&self.counts_cache);
                std::thread::Builder::new()
                    .name("sqlite-build".into())
                    .spawn(move || {
                        status.write().unwrap().state = State::Indexing;
                        let entries = build_entries(&roots, queue.as_ref(), &status, opts);
                        let n = entries.len();
                        if let Ok(mut d) = db.lock() {
                            match d.rebuild(&entries, &walk_key) {
                                Ok(()) => {
                                    let (files, dirs) = d.counts().unwrap_or((0, 0));
                                    *counts_cache.write().unwrap() =
                                        ((files, dirs), Instant::now());
                                    let mut s = status.write().unwrap();
                                    s.base_entries = n;
                                    s.base_files = files;
                                    s.base_dirs = dirs;
                                }
                                Err(e) => eprintln!("sqlite: initial build failed: {e}"),
                            }
                        }
                        trim_allocator();
                        {
                            let mut s = status.write().unwrap();
                            s.state = State::Live;
                            s.last_index_at = now_unix();
                        }
                    })
                    .expect("failed to spawn sqlite-build thread");
            } else {
                if let Ok(d) = db.lock() {
                    let (files, dirs) = d.counts().unwrap_or((0, 0));
                    *self.counts_cache.write().unwrap() = ((files, dirs), Instant::now());
                    let mut s = self.status.write().unwrap();
                    s.base_entries = (files + dirs) as usize;
                    s.base_files = files;
                    s.base_dirs = dirs;
                    s.state = State::Live;
                    // Served from the cache: date it from the database file.
                    s.last_index_at = file_mtime(&self.config.db_dir().join(DB_FILE));
                }
                self.spawn_rebuild(false);
            }
            return;
        }

        let has_cache = self.config.persist_index && self.base.read().unwrap().is_some();
        if has_cache {
            // Searchable immediately; refresh the base in the background.
            {
                let mut s = self.status.write().unwrap();
                s.state = State::Live;
                s.last_index_at = file_mtime(&self.index_path);
            }
            self.spawn_rebuild(false);
        } else {
            let roots = Arc::clone(&self.roots);
            let status = Arc::clone(&self.status);
            let queue = self.queue.clone();
            let overlay = Arc::clone(&self.overlay);
            let base = Arc::clone(&self.base);
            let opts = self.walk_options();
            let persist = self.config.persist_index;
            let index_path = self.index_path.clone();
            std::thread::Builder::new()
                .name("index-build".into())
                .spawn(move || {
                    status.write().unwrap().state = State::Indexing;
                    if persist {
                        let entries = build_entries(&roots, queue.as_ref(), &status, opts);
                        match DiskIndex::write_and_load(&index_path, &entries) {
                            Ok(idx) => *base.write().unwrap() = Some(Arc::new(idx)),
                            Err(e) => eprintln!("initial index build failed: {e}"),
                        }
                        trim_allocator();
                    } else {
                        for root in &roots.roots() {
                            walk_root_apply(
                                root,
                                &overlay,
                                &roots,
                                queue.as_ref(),
                                &status,
                                opts.clone(),
                            );
                        }
                    }
                    {
                        let mut s = status.write().unwrap();
                        s.state = State::Live;
                        s.last_index_at = now_unix();
                    }
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
        let opts = self.walk_options();

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
            let opts = self.walk_options();
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
                let opts = opts.clone();
                std::thread::Builder::new()
                    .name("rescan".into())
                    .spawn(move || {
                        loop {
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
                                    opts.clone(),
                                );
                            } else {
                                for root in &roots.roots() {
                                    walk_root_apply(
                                        root,
                                        &overlay,
                                        &roots,
                                        queue.as_ref(),
                                        &status,
                                        opts.clone(),
                                    );
                                }
                            }
                        }
                    })
                    .expect("failed to spawn rescan thread");
            })
        };

        let handle = watcher::start_watcher(
            self.roots.roots(),
            overlay,
            roots,
            cache,
            queue,
            status,
            opts,
            Arc::clone(&self.paused),
            on_error,
        );
        drop(handle); // detached: the thread runs until process exit
    }

    /// Trigger a background compaction/rebuild of the disk index.
    pub fn rebuild(&self) {
        if self.config.persist_index {
            self.spawn_rebuild(true);
        }
    }

    fn spawn_rebuild(&self, force: bool) {
        // Background rebuilds are held back while paused; a user-initiated one
        // (`force`) still runs, so "Rebuild index" is never a no-op.
        if !force && self.paused.load(Ordering::Relaxed) {
            return;
        }
        if let Some(db) = self.sqlite.clone() {
            let roots = Arc::clone(&self.roots);
            let status = Arc::clone(&self.status);
            let overlay = Arc::clone(&self.overlay);
            let queue = self.queue.clone();
            let rebuilding = Arc::clone(&self.rebuilding);
            let counts_cache = Arc::clone(&self.counts_cache);
            let opts = self.walk_options();
            let walk_key = self.walk_key();
            std::thread::Builder::new()
                .name("sqlite-rebuild".into())
                .spawn(move || {
                    if rebuilding.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let entries = build_entries(&roots, queue.as_ref(), &status, opts);
                    let n = entries.len();
                    if let Ok(mut d) = db.lock() {
                        match d.rebuild(&entries, &walk_key) {
                            Ok(()) => {
                                let (files, dirs) = d.counts().unwrap_or((0, 0));
                                *counts_cache.write().unwrap() = ((files, dirs), Instant::now());
                                // Keep only deltas the walk could not have seen.
                                overlay.write().unwrap().prune_against(|p| d.contains(p));
                                let mut s = status.write().unwrap();
                                s.base_entries = n;
                                s.base_files = files;
                                s.base_dirs = dirs;
                                s.last_index_at = now_unix();
                            }
                            Err(e) => eprintln!("sqlite: rebuild failed: {e}"),
                        }
                    }
                    rebuilding.store(false, Ordering::SeqCst);
                    trim_allocator();
                })
                .expect("failed to spawn sqlite-rebuild thread");
            return;
        }

        let roots = Arc::clone(&self.roots);
        let status = Arc::clone(&self.status);
        let base = Arc::clone(&self.base);
        let overlay = Arc::clone(&self.overlay);
        let queue = self.queue.clone();
        let rebuilding = Arc::clone(&self.rebuilding);
        let opts = self.walk_options();
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
                    opts,
                );
            })
            .expect("failed to spawn rebuild thread");
    }

    /// Compaction trigger: fold a large change overlay back into the disk file.
    fn maybe_compact(&self) {
        if let Some(db) = &self.sqlite {
            // SQLite: fold pending changes in, and fall back to a full rebuild
            // (the "refresh") once the delta backlog gets large.
            self.flush_overlay();
            let dirty = db.lock().map(|d| d.dirty()).unwrap_or(0);
            if dirty > REFRESH_AFTER_DIRTY && !self.rebuilding.load(Ordering::Relaxed) {
                self.spawn_rebuild(false);
            }
            return;
        }
        if !self.config.persist_index || self.rebuilding.load(Ordering::Relaxed) {
            return;
        }
        let pending = self.overlay.read().unwrap().pending_changes();
        if pending > self.config.overlay_compaction_threshold {
            self.spawn_rebuild(false);
        }
    }

    /// Return freed heap pages to the OS (`malloc_trim(0)`).
    ///
    /// Content search scans whole files through ripgrep, and the buffers it uses
    /// are the largest transient allocation this engine makes; once the caller
    /// leaves that mode, this hands the freed pages back instead of letting the
    /// allocator keep them resident.
    pub fn trim_memory(&self) {
        trim_allocator();
    }

    /// Block until the initial build finishes (bounded by `timeout`).
    pub fn wait_live(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(s) = self.status.read()
                && s.state == State::Live
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn status_snapshot(&self) -> Status {
        let mut s = self.status.read().unwrap().clone();
        s.respect_ignore_files = self.respect_ignore.load(Ordering::Relaxed);
        s.paused = self.paused.load(Ordering::Relaxed);
        s.follow_symlinks = self.follow_symlinks.load(Ordering::Relaxed);
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
        if self.sqlite.is_some() {
            let (files, dirs) = self.counts();
            s.base_files = files;
            s.base_dirs = dirs;
            s.base_entries = (files + dirs) as usize;
            return s;
        }
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
        if self.sqlite.is_some() {
            return self.search_sqlite(&cq, t0);
        }
        let limit = cq.limit;
        let want_content = cq.content.is_some();
        self.maybe_compact();

        let cap = if want_content {
            limit.max(CONTENT_FANOUT_CAP)
        } else {
            limit
        };
        let name_filter = |p: &Path, meta: Meta| -> Option<(PathBuf, Meta)> {
            accepts(&cq, p, meta, false).then(|| (p.to_path_buf(), meta))
        };

        let (results, truncated) = if want_content && cq.content_or_name && cq.has_name_filter {
            // “Full text”: a file matches if its *name* matches or its *content*
            // does, so both halves are evaluated and merged.
            let pattern = ContentPattern::new(
                cq.content.as_deref().unwrap_or(""),
                cq.multiline,
                cq.case_sensitive,
            )?;
            let name_hits: Vec<PathBuf> = self
                .collect(&name_filter, cap)
                .into_iter()
                .map(|(p, _)| p)
                .collect();
            let name_less = without_name(&cq);
            let candidates: Vec<PathBuf> = self
                .collect(
                    &|p, meta| accepts(&name_less, p, meta, true).then(|| (p.to_path_buf(), meta)),
                    cap,
                )
                .into_iter()
                .map(|(p, _)| p)
                .collect();
            let content_hits = search_contents(
                &candidates,
                &pattern,
                cap,
                &self.cache,
                self.queue.as_deref(),
            );
            let (paths, truncated) = merge_or_hits(name_hits, content_hits, limit);
            (
                paths.into_iter().map(|p| self.row_for(&p)).collect(),
                truncated,
            )
        } else if want_content {
            let pattern = ContentPattern::new(
                cq.content.as_deref().unwrap_or(""),
                cq.multiline,
                cq.case_sensitive,
            )?;
            let candidates: Vec<PathBuf> = if cq.has_name_filter {
                self.collect(&name_filter, cap)
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect()
            } else {
                // content-only: every indexed file
                self.collect(
                    &|p, meta| accepts(&cq, p, meta, true).then(|| (p.to_path_buf(), meta)),
                    cap,
                )
                .into_iter()
                .map(|(p, _)| p)
                .collect()
            };
            let matched = search_contents(
                &candidates,
                &pattern,
                limit,
                &self.cache,
                self.queue.as_deref(),
            );
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

    /// SQLite twin of [`search`](Engine::search): pending changes are folded
    /// into the database first, so a query never misses a file the watcher has
    /// already seen.
    fn search_sqlite(&self, cq: &CompiledQuery, t0: Instant) -> Result<SearchResponse, String> {
        self.maybe_compact(); // flushes the overlay and applies the refresh policy
        let db = self
            .sqlite
            .as_ref()
            .ok_or_else(|| "sqlite backend missing".to_string())?;
        let db = db.lock().map_err(|_| "sqlite index poisoned".to_string())?;
        let limit = cq.limit;
        let want_content = cq.content.is_some();
        let cap = if want_content {
            limit.max(CONTENT_FANOUT_CAP)
        } else {
            limit
        };

        let (results, truncated) = if want_content && cq.content_or_name && cq.has_name_filter {
            // “Full text”: match the name *or* the content.
            let pattern = ContentPattern::new(
                cq.content.as_deref().unwrap_or(""),
                cq.multiline,
                cq.case_sensitive,
            )?;
            let name_hits: Vec<PathBuf> = db.candidates(cq, cap)?;
            let name_less = without_name(cq);
            let candidates = db.candidates(&name_less, cap)?;
            let content_hits = search_contents(
                &candidates,
                &pattern,
                cap,
                &self.cache,
                self.queue.as_deref(),
            );
            let (paths, truncated) = merge_or_hits(name_hits, content_hits, limit);
            let rows = paths
                .into_iter()
                .map(|p| {
                    let m = db.meta_of(&p).unwrap_or_default();
                    ResultRow {
                        path: p,
                        size: m.size,
                        mtime: m.mtime,
                        is_dir: m.is_dir,
                    }
                })
                .collect();
            (rows, truncated)
        } else if want_content {
            let pattern = ContentPattern::new(
                cq.content.as_deref().unwrap_or(""),
                cq.multiline,
                cq.case_sensitive,
            )?;
            let candidates = db.candidates(cq, cap)?;
            let matched = search_contents(
                &candidates,
                &pattern,
                limit,
                &self.cache,
                self.queue.as_deref(),
            );
            let truncated = matched.len() >= limit;
            let rows = matched
                .into_iter()
                .map(|p| {
                    let m = db.meta_of(&p).unwrap_or_default();
                    ResultRow {
                        path: p,
                        size: m.size,
                        mtime: m.mtime,
                        is_dir: m.is_dir,
                    }
                })
                .collect();
            (rows, truncated)
        } else {
            let hits = db.search(cq)?;
            let truncated = hits.len() >= limit;
            (hits, truncated)
        };

        Ok(SearchResponse {
            results,
            truncated,
            elapsed_ms: t0.elapsed().as_millis() as u64,
            indexed: db.counts().map(|(f, _)| f).unwrap_or(0),
        })
    }

    /// Iterate the base index (skipping overlay-removed paths, applying
    /// overlay metadata) plus overlay-only additions, keeping entries for
    /// which `f` returns `Some`. Bounded by `cap`.
    /// Count entries matching `q` without materialising the rows.
    ///
    /// Used for the sidebar's per-category counts: same predicate as [`search`],
    /// but it walks the mmap index without allocating a path per match.
    ///
    /// [`search`]: Engine::search
    pub fn count(&self, q: &Query) -> Result<u64, String> {
        let cq = CompiledQuery::compile(q)?;
        if let Some(db) = &self.sqlite {
            self.flush_overlay();
            let db = db.lock().map_err(|_| "sqlite index poisoned".to_string())?;
            return db.count(&cq);
        }
        let files_only = cq.content.is_some();
        let mut n: u64 = 0;
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
                    if accepts(&cq, p, meta, files_only) {
                        n += 1;
                    }
                }
            }
            for (path, meta) in ov.added.iter() {
                if ov.removed.contains(path) {
                    continue;
                }
                if base.as_deref().is_some_and(|b| b.contains(path)) {
                    continue;
                }
                if accepts(&cq, path, *meta, files_only) {
                    n += 1;
                }
            }
        }
        Ok(n)
    }

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
                let in_base = base.as_deref().is_some_and(|b| b.contains(path));
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
        if let Some(db) = &self.sqlite {
            return db
                .lock()
                .ok()
                .and_then(|d| d.meta_of(path))
                .unwrap_or_default();
        }
        let base = self.base.read().unwrap();
        let ov = self.overlay.read().unwrap();
        if let Some(m) = ov.added.get(path) {
            return *m;
        }
        if ov.is_removed(path) {
            return Meta::default();
        }
        base.as_deref()
            .and_then(|b| b.meta_of(path))
            .unwrap_or_default()
    }

    /// (files, dirs) — base plus overlay deltas.
    pub fn counts(&self) -> (u64, u64) {
        if let Some(db) = &self.sqlite {
            {
                let c = self.counts_cache.read().unwrap();
                if c.1.elapsed() < Duration::from_secs(2) {
                    return c.0;
                }
            }
            self.flush_overlay();
            let fresh = db
                .lock()
                .map(|d| d.counts().unwrap_or((0, 0)))
                .unwrap_or((0, 0));
            *self.counts_cache.write().unwrap() = (fresh, Instant::now());
            return fresh;
        }
        let base = self.base.read().unwrap();
        let ov = self.overlay.read().unwrap();
        let b = base.as_deref();
        let (mut files, mut dirs) = b.map(|b| (b.files(), b.dirs())).unwrap_or((0, 0));
        for (p, m) in &ov.added {
            if b.is_none_or(|b| !b.contains(p)) {
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
/// The shared "does this entry match the query?" predicate, used by both
/// [`Engine::search`] and [`Engine::count`] so the two can never disagree.
///
/// `files_only` forces directories out even when `include_dirs` is set (content
/// search only ever looks inside files).
/// A copy of `cq` with the name terms and exclusions removed, so `accepts` can
/// collect the candidates for the *content* half of a “Full text” (OR) query
/// without also demanding that the name match.
fn without_name(cq: &CompiledQuery) -> CompiledQuery {
    let mut bare = cq.clone();
    bare.or_groups.clear();
    bare.excludes.clear();
    bare.folder = None;
    bare.has_name_filter = false;
    bare
}

/// Merge the two halves of an OR query — name matches first, then the content
/// matches that are not already there — and cap the result.
fn merge_or_hits(
    mut name_hits: Vec<PathBuf>,
    content_hits: Vec<PathBuf>,
    limit: usize,
) -> (Vec<PathBuf>, bool) {
    let mut seen: HashSet<PathBuf> = name_hits.iter().cloned().collect();
    for path in content_hits {
        if seen.insert(path.clone()) {
            name_hits.push(path);
        }
    }
    let truncated = name_hits.len() > limit;
    name_hits.truncate(limit);
    (name_hits, truncated)
}

pub(crate) fn accepts(cq: &CompiledQuery, p: &Path, meta: Meta, files_only: bool) -> bool {
    if meta.is_dir && (files_only || !cq.include_dirs) {
        return false;
    }
    // Extension filter: files only, final extension must be listed.
    if !cq.extensions.is_empty() && (meta.is_dir || !cq.extension_matches(p)) {
        return false;
    }
    // Size bounds are file-only and inclusive.
    let has_size_bound = cq.min_size.is_some() || cq.max_size.is_some();
    if (has_size_bound && meta.is_dir)
        || cq.min_size.is_some_and(|min| meta.size < min)
        || cq.max_size.is_some_and(|max| meta.size > max)
    {
        return false;
    }
    // Recency: `now - mtime <= secs` (a future mtime has a negative age and
    // therefore still matches).
    if let Some(secs) = cq.modified_within_secs
        && now_unix() - meta.mtime > secs
    {
        return false;
    }
    if !cq.include_hidden && is_hidden(p) {
        return false;
    }
    if cq.has_name_filter && !cq.name_matches(p) {
        return false;
    }
    if !matches_category(&cq.category, p, &meta) {
        return false;
    }
    if let Some(prefix) = cq.under.as_deref()
        && !p.starts_with(prefix)
    {
        return false;
    }
    true
}

/// Modification time of a file in unix seconds (0 if it cannot be read).
fn file_mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Current unix time in whole seconds (0 if the clock is before the epoch).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn build_entries(
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    opts: walker::WalkOptions,
) -> Vec<(PathBuf, Meta)> {
    let mut out = Vec::new();
    for root in &roots.roots() {
        out.extend(walk_root_collect(root, roots, queue, status, opts.clone()));
    }
    out
}

/// One background compaction: walk live, write the disk index, swap the base,
/// and prune the overlay to true deltas.
///
/// The parameters are the engine's own fields, passed explicitly so the thread
/// closure owns them; bundling them into a struct would only move the list.
#[allow(clippy::too_many_arguments)]
fn rebuild_once(
    index_path: &Path,
    roots: &Arc<RootSet>,
    base: &RwLock<Option<Arc<DiskIndex>>>,
    overlay: &RwLock<Overlay>,
    queue: Option<&Arc<ExtractQueue>>,
    rebuilding: &AtomicBool,
    status: &Arc<RwLock<Status>>,
    opts: walker::WalkOptions,
) {
    if rebuilding.swap(true, Ordering::SeqCst) {
        return;
    }
    let entries = build_entries(roots, queue, status, opts);
    match DiskIndex::write_and_load(index_path, &entries) {
        Ok(idx) => {
            let idx = Arc::new(idx);
            *base.write().unwrap() = Some(Arc::clone(&idx));
            overlay.write().unwrap().prune_against(|p| idx.contains(p));
            status.write().unwrap().last_index_at = now_unix();
        }
        Err(e) => eprintln!("index rebuild failed: {e}"),
    }
    // The walk's transient allocations were freed above; hand the pages back
    // to the kernel instead of leaving them in glibc's arenas.
    trim_allocator();
    rebuilding.store(false, Ordering::SeqCst);
}

/// Return freed-but-cached allocator pages to the kernel (glibc only).
fn trim_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    let _ = ();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(q: &Query, path: &str, size: u64, mtime: i64, is_dir: bool) -> bool {
        let cq = CompiledQuery::compile(q).unwrap();
        accepts(
            &cq,
            Path::new(path),
            Meta {
                size,
                mtime,
                is_dir,
            },
            false,
        )
    }

    #[test]
    fn extension_filter_keeps_only_listed_files() {
        let q = Query {
            extensions: vec!["pdf".into()],
            ..Query::default()
        };
        assert!(accept(&q, "/x/report.pdf", 1, 0, false));
        assert!(accept(&q, "/x/REPORT.PDF", 1, 0, false));
        assert!(!accept(&q, "/x/report.txt", 1, 0, false));
        // A directory (even one named `*.pdf`) never matches an ext filter.
        assert!(!accept(&q, "/x/folder.pdf", 1, 0, true));
    }

    #[test]
    fn size_bounds_are_inclusive_and_file_only() {
        let q = Query {
            min_size: Some(10),
            max_size: Some(20),
            ..Query::default()
        };
        assert!(accept(&q, "/x/a.bin", 10, 0, false));
        assert!(accept(&q, "/x/a.bin", 20, 0, false));
        assert!(!accept(&q, "/x/a.bin", 9, 0, false));
        assert!(!accept(&q, "/x/a.bin", 21, 0, false));
        assert!(!accept(&q, "/x/dir", 15, 0, true));
    }

    #[test]
    fn modified_within_keeps_recent_and_future() {
        let now = now_unix();
        let q = Query {
            modified_within_secs: Some(3600),
            ..Query::default()
        };
        assert!(accept(&q, "/x/recent", 1, now - 60, false));
        assert!(!accept(&q, "/x/old", 1, now - 7200, false));
        // A future mtime has a negative age and must still match.
        assert!(accept(&q, "/x/future", 1, now + 100_000, false));
    }
}
