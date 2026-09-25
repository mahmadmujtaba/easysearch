//! Cold (and rebuild) filesystem walk using ripgrep's `ignore` walker.
//!
//! Results are either collected (`walk_root_collect`, for building the
//! on-disk index) or applied in batches to the change overlay
//! (`walk_root_apply`, for RAM-only mode and watcher-driven subtree indexing).
//! When `respect_ignore` is set, `.gitignore`/`.ignore` files in the searched
//! tree are honored, plus a global ignore file at
//! `~/.config/everything-linux/ignore`.

use crate::content_index::ExtractQueue;
use crate::engine::Status;
use crate::overlay::{Meta, Overlay};
use crate::roots::RootSet;
use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

const BATCH: usize = 512;

enum Sink {
    /// Collect everything (used to build the disk index).
    Collect(Arc<Mutex<Vec<(PathBuf, Meta)>>>),
    /// Apply to the change overlay (RAM-only mode / watcher subtrees).
    Apply(Arc<RwLock<Overlay>>),
}

impl Clone for Sink {
    fn clone(&self) -> Sink {
        match self {
            Sink::Collect(v) => Sink::Collect(Arc::clone(v)),
            Sink::Apply(o) => Sink::Apply(Arc::clone(o)),
        }
    }
}

/// Walk `root` and collect every entry.
pub fn walk_root_collect(
    root: &Path,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
) -> Vec<(PathBuf, Meta)> {
    let out = Arc::new(Mutex::new(Vec::new()));
    walk_impl(
        root,
        roots,
        queue,
        status,
        respect_ignore,
        Sink::Collect(Arc::clone(&out)),
    );
    match Arc::try_unwrap(out) {
        Ok(m) => m.into_inner().unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Walk `root` and apply every entry to the overlay.
pub fn walk_root_apply(
    root: &Path,
    overlay: &Arc<RwLock<Overlay>>,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
) {
    walk_impl(
        root,
        roots,
        queue,
        status,
        respect_ignore,
        Sink::Apply(Arc::clone(overlay)),
    );
}

fn walk_impl(
    root: &Path,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
    sink: Sink,
) {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // index hidden files too; filtering happens at query time
        .follow_links(false)
        .threads(
            std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4),
        );
    if respect_ignore {
        builder
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false);
        if let Some(ig) = global_ignore_path() {
            if let Some(err) = builder.add_ignore(ig) {
                eprintln!("global ignore: {err}");
            }
        }
    } else {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }

    let shared: Arc<Mutex<Vec<(PathBuf, Meta)>>> = Arc::new(Mutex::new(Vec::new()));

    builder.build_parallel().run(|| {
        let batch = Arc::clone(&shared);
        let roots = Arc::clone(roots);
        let queue = queue.cloned();
        let status = Arc::clone(status);
        let sink = sink.clone();
        Box::new(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    bump_skipped(&status);
                    return WalkState::Continue;
                }
            };
            let path = entry.path();
            if roots.is_excluded(path) {
                let is_dir = entry.file_type().map_or(false, |t| t.is_dir());
                return if is_dir {
                    WalkState::Skip
                } else {
                    WalkState::Continue
                };
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    bump_skipped(&status);
                    return WalkState::Continue;
                }
            };
            let is_dir = meta.is_dir();
            let emeta = Meta {
                size: if is_dir { 0 } else { meta.len() },
                mtime: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                is_dir,
            };
            let pb = path.to_path_buf();
            {
                let mut b = batch.lock().unwrap();
                b.push((pb.clone(), emeta));
                if b.len() >= BATCH {
                    let drained = std::mem::take(&mut *b);
                    drop(b);
                    flush(&sink, drained);
                }
            }
            if let Some(q) = queue.as_deref() {
                if !is_dir {
                    q.send(pb);
                }
            }
            WalkState::Continue
        })
    });

    // Final flush.
    {
        let mut guard = shared.lock().unwrap();
        if !guard.is_empty() {
            let drained = std::mem::take(&mut *guard);
            drop(guard);
            flush(&sink, drained);
        }
    }
}

fn flush(sink: &Sink, drained: Vec<(PathBuf, Meta)>) {
    match sink {
        Sink::Collect(out) => out.lock().unwrap().extend(drained),
        Sink::Apply(overlay) => apply(overlay, drained),
    }
}

fn apply(overlay: &Arc<RwLock<Overlay>>, batch: Vec<(PathBuf, Meta)>) {
    let mut ov = overlay.write().unwrap();
    for (path, meta) in batch {
        ov.upsert(path, meta);
    }
}

/// Walk `root` and collect every directory that should be watched.
///
/// Uses the same ignore-file and mount-exclusion rules as indexing, and —
/// unlike a single recursive inotify watch — *skips* unreadable directories
/// instead of failing, so one bad folder cannot knock out the whole watcher.
pub fn collect_dirs(root: &Path, roots: &Arc<RootSet>, respect_ignore: bool) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(false).follow_links(false).threads(1);
    if respect_ignore {
        builder
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false);
        if let Some(ig) = global_ignore_path() {
            let _ = builder.add_ignore(ig);
        }
    } else {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }
    let mut dirs = Vec::new();
    for entry in builder.build() {
        let Ok(entry) = entry else { continue }; // unreadable: skip, don't fail
        if roots.is_excluded(entry.path()) {
            continue;
        }
        if entry.file_type().map_or(false, |t| t.is_dir()) {
            dirs.push(entry.path().to_path_buf());
        }
    }
    dirs
}

/// Path of the global ignore file (`~/.config/everything-linux/ignore`),
/// whether or not it exists yet. Patterns use gitignore syntax, matched
/// relative to the working directory (or use `**/` prefixes to match anywhere).
pub fn global_ignore_file() -> PathBuf {
    crate::config::xdg_config_dir()
        .join("everything-linux")
        .join("ignore")
}

/// The global ignore file, when it exists.
fn global_ignore_path() -> Option<PathBuf> {
    let path = global_ignore_file();
    path.is_file().then_some(path)
}

fn bump_skipped(status: &Arc<RwLock<Status>>) {
    if let Ok(mut s) = status.write() {
        s.skipped = s.skipped.saturating_add(1);
    }
}
