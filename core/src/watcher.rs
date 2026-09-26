//! Realtime watcher: kernel filesystem events → overlay updates.
//!
//! Runs the `notify` event loop on a dedicated thread. Every event mutates
//! the small in-memory change overlay immediately (< 1 s target); degraded
//! mode is triggered when the kernel reports errors (typically exhausted
//! watch limits) and falls back to periodic full rebuilds.

use crate::content_index::{ContentIndex, ExtractQueue};
use crate::engine::Status;
use crate::overlay::{Meta, Overlay};
use crate::roots::RootSet;
use crate::walker::{WalkOptions, walk_root_apply};
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, mpsc};
use std::thread::JoinHandle;

/// Spawn the watcher thread. `on_error` is invoked when watching is impaired
/// (unreadable subtrees or a watcher-level error); the engine then falls back
/// to periodic rebuilds for the affected paths.
///
/// Watches are installed **per directory** (non-recursively) rather than as a
/// single recursive watch: one unreadable directory is skipped and counted
/// instead of aborting the whole watch — which a single root-owned folder
/// inside a Steam/Proton prefix would otherwise cause.
///
/// The parameters are the engine's own fields; they are passed explicitly so the
/// thread closure owns them.
#[allow(clippy::too_many_arguments)]
pub fn start_watcher(
    roots: Vec<PathBuf>,
    overlay: Arc<RwLock<Overlay>>,
    roots_set: Arc<RootSet>,
    cache: Arc<ContentIndex>,
    queue: Option<Arc<ExtractQueue>>,
    status: Arc<RwLock<Status>>,
    opts: WalkOptions,
    on_error: Arc<dyn Fn() + Send + Sync>,
) -> JoinHandle<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    std::thread::Builder::new()
        .name("watcher".into())
        .spawn(move || {
            // The watcher lives inside the thread for its whole lifetime
            // (dropping it removes every watch).
            let mut watcher = match RecommendedWatcher::new(tx, NotifyConfig::default()) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("filesystem watcher unavailable: {e}");
                    on_error();
                    return;
                }
            };

            let mut failures: u64 = 0;
            let mut watched: u64 = 0;
            for root in &roots {
                // Non-recursive on the root itself catches new top-level entries;
                // every eligible directory is then watched individually.
                match watcher.watch(root, RecursiveMode::NonRecursive) {
                    Ok(_) => watched += 1,
                    Err(e) => {
                        failures += 1;
                        if failures <= 5 {
                            eprintln!("watch {root:?} failed: {e}");
                        }
                    }
                }
                for dir in crate::walker::collect_dirs(root, &roots_set, opts) {
                    match watcher.watch(&dir, RecursiveMode::NonRecursive) {
                        Ok(_) => watched += 1,
                        Err(e) => {
                            failures += 1;
                            if failures <= 5 {
                                eprintln!("watch {dir:?} failed: {e}");
                            }
                        }
                    }
                }
            }
            {
                let mut s = status.write().unwrap();
                s.watch_failures = failures;
                if failures > 0 {
                    s.degraded = true;
                    eprintln!(
                        "watcher: {watched} dirs live, {failures} unwatchable (periodic rebuild covers them)"
                    );
                }
            }
            if failures > 0 {
                on_error();
            }

            for res in rx {
                match res {
                    Ok(event) => {
                        // A directory created (or moved in) needs its own watches:
                        // this root's watch is non-recursive, so nothing else
                        // will cover it.
                        let new_dir = matches!(event.kind, EventKind::Create(_))
                            || matches!(
                                event.kind,
                                EventKind::Modify(notify::event::ModifyKind::Name(
                                    notify::event::RenameMode::To
                                ))
                            );
                        if new_dir {
                            for p in event.paths.iter().filter(|p| {
                                p.is_dir() && roots_set.is_in_roots(p) && !roots_set.is_excluded(p)
                            }) {
                                let _ = watcher.watch(p, RecursiveMode::NonRecursive);
                                for dir in crate::walker::collect_dirs(p, &roots_set, opts) {
                                    let _ = watcher.watch(&dir, RecursiveMode::NonRecursive);
                                }
                            }
                        }
                        handle_event(
                            &event,
                            &overlay,
                            &roots_set,
                            &cache,
                            queue.as_ref(),
                            &status,
                            opts,
                        );
                    }
                    Err(e) => {
                        eprintln!("watcher error: {e}");
                        on_error();
                    }
                }
            }
        })
        .expect("failed to spawn watcher thread")
}

fn handle_event(
    event: &Event,
    overlay: &Arc<RwLock<Overlay>>,
    roots: &Arc<RootSet>,
    cache: &ContentIndex,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    opts: WalkOptions,
) {
    let Some(path) = event.paths.first().cloned() else {
        return;
    };
    if !roots.is_in_roots(&path) {
        return;
    }
    match event.kind {
        EventKind::Create(_) => add_path(&path, overlay, roots, queue, status, opts),
        EventKind::Remove(_) => remove_path(&path, overlay, cache),
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::From)) => {
            remove_path(&path, overlay, cache)
        }
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::To)) => {
            add_path(&path, overlay, roots, queue, status, opts)
        }
        EventKind::Modify(
            notify::event::ModifyKind::Data(_) | notify::event::ModifyKind::Metadata(_),
        ) => refresh_meta(&path, overlay, cache, queue),
        _ => {}
    }
}

fn remove_path(path: &Path, overlay: &Arc<RwLock<Overlay>>, cache: &ContentIndex) {
    overlay.write().unwrap().remove(path);
    cache.remove(path);
}

fn add_path(
    path: &Path,
    overlay: &Arc<RwLock<Overlay>>,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    opts: WalkOptions,
) {
    if roots.is_excluded(path) {
        return;
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return,
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
    overlay.write().unwrap().upsert(path.to_path_buf(), emeta);
    if is_dir {
        // A new directory may already contain files that predate the watch:
        // index its subtree immediately (into the overlay).
        walk_root_apply(path, overlay, roots, queue, status, opts);
    } else if let Some(q) = queue {
        q.send(path.to_path_buf());
    }
}

fn refresh_meta(
    path: &Path,
    overlay: &Arc<RwLock<Overlay>>,
    cache: &ContentIndex,
    queue: Option<&Arc<ExtractQueue>>,
) {
    // Content changed (or metadata): update size/mtime and invalidate the
    // content cache so the next query reads live data (or re-extracts).
    cache.remove(path);
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {
            let emeta = Meta {
                size: m.len(),
                mtime: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                is_dir: false,
            };
            overlay.write().unwrap().upsert(path.to_path_buf(), emeta);
            if let Some(q) = queue {
                q.send(path.to_path_buf());
            }
        }
        Ok(m) if m.is_dir() => {
            let emeta = Meta {
                size: 0,
                mtime: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                is_dir: true,
            };
            overlay.write().unwrap().upsert(path.to_path_buf(), emeta);
        }
        _ => {}
    }
}
