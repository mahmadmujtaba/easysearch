//! Cold (and rescan) filesystem walk using ripgrep's `ignore` walker.
//!
//! Results are batched and applied to the shared index under a short write
//! lock, so queries can run while indexing is still in progress.
//!
//! When `respect_ignore` is set (config default), `.ignore`/`.gitignore` files
//! in the searched tree are honored (gitignore syntax), plus a global ignore
//! file at `~/.config/everything-linux/ignore`.

use crate::content_index::ExtractQueue;
use crate::engine::Status;
use crate::index::{Index, Meta};
use crate::roots::RootSet;
use ignore::{WalkBuilder, WalkState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

const BATCH: usize = 512;

/// Walk `root` and add everything to `index`.
pub fn walk_root(
    root: &Path,
    index: &Arc<RwLock<Index>>,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    respect_ignore: bool,
) {
    walk_impl(root, index, roots, queue, status, None, respect_ignore);
}

/// Walk `root`, adding entries to `index` and recording every seen path in
/// `seen` (used by degraded-mode reconciliation).
pub fn walk_root_into(
    root: &Path,
    index: &Arc<RwLock<Index>>,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    seen: Arc<Mutex<HashSet<PathBuf>>>,
    respect_ignore: bool,
) {
    walk_impl(root, index, roots, queue, status, Some(seen), respect_ignore);
}

fn walk_impl(
    root: &Path,
    index: &Arc<RwLock<Index>>,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    seen: Option<Arc<Mutex<HashSet<PathBuf>>>>,
    respect_ignore: bool,
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
        // Honor .ignore / .gitignore files everywhere (not only in git repos),
        // including the global ignore file at ~/.config/everything-linux/ignore.
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
        let seen = seen.clone();
        let index = Arc::clone(index);
        let roots = Arc::clone(roots);
        let queue = queue.cloned();
        let status = Arc::clone(status);
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
                return if is_dir { WalkState::Skip } else { WalkState::Continue };
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
                    if let Some(seen) = seen.as_deref() {
                        seen.lock().unwrap().extend(drained.iter().map(|(p, _)| p.clone()));
                    }
                    drop(b);
                    apply(&index, drained);
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

    // Final flush of any entries left in the shared batch.
    {
        let mut guard = shared.lock().unwrap();
        if !guard.is_empty() {
            let drained = std::mem::take(&mut *guard);
            if let Some(seen) = seen.as_deref() {
                seen.lock().unwrap().extend(drained.iter().map(|(p, _)| p.clone()));
            }
            drop(guard);
            apply(index, drained);
        }
    }
}

/// Path of the global ignore file at `~/.config/everything-linux/ignore`.
/// Patterns use gitignore syntax, matched relative to the working directory
/// (or use `**/` prefixes to match anywhere).
fn global_ignore_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_default()
        });
    let path = base.join("everything-linux").join("ignore");
    path.is_file().then_some(path)
}

fn apply(index: &Arc<RwLock<Index>>, batch: Vec<(PathBuf, Meta)>) {
    let mut idx = index.write().unwrap();
    for (path, meta) in batch {
        idx.upsert(path, meta);
    }
}

fn bump_skipped(status: &Arc<RwLock<Status>>) {
    if let Ok(mut s) = status.write() {
        s.skipped = s.skipped.saturating_add(1);
    }
}
