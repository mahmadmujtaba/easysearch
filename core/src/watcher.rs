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
use crate::walker::walk_root_apply;
use notify::{Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::JoinHandle;

/// Spawn the watcher thread. `on_error` is invoked when the kernel reports a
/// watcher error (e.g. exhausted watch limits); the engine uses it to switch
/// to degraded mode with periodic rebuilds.
pub fn start_watcher(
    roots: Vec<PathBuf>,
    overlay: Arc<RwLock<Overlay>>,
    roots_set: Arc<RootSet>,
    cache: Arc<ContentIndex>,
    queue: Option<Arc<ExtractQueue>>,
    status: Arc<RwLock<Status>>,
    respect_ignore: bool,
    on_error: Arc<dyn Fn() + Send + Sync>,
) -> JoinHandle<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(tx, NotifyConfig::default())
        .expect("failed to create filesystem watcher (inotify)");
    for root in &roots {
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            eprintln!("watch {root:?} failed: {e}");
            on_error();
        }
    }

    std::thread::Builder::new()
        .name("watcher".into())
        .spawn(move || {
            // Keep the watcher alive for the lifetime of this thread: dropping
            // it removes the inotify watches and no further events arrive.
            let _keep_alive = watcher;
            for res in rx {
                match res {
                    Ok(event) => handle_event(
                        &event,
                        &overlay,
                        &roots_set,
                        &cache,
                        queue.as_ref(),
                        &status,
                        respect_ignore,
                    ),
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
    respect_ignore: bool,
) {
    let Some(path) = event.paths.first().cloned() else {
        return;
    };
    if !roots.is_in_roots(&path) {
        return;
    }
    match event.kind {
        EventKind::Create(_) => add_path(&path, overlay, roots, queue, status, respect_ignore),
        EventKind::Remove(_) => remove_path(&path, overlay, cache),
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::From)) => {
            remove_path(&path, overlay, cache)
        }
        EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::To)) => {
            add_path(&path, overlay, roots, queue, status, respect_ignore)
        }
        EventKind::Modify(notify::event::ModifyKind::Data(_) | notify::event::ModifyKind::Metadata(_)) => {
            refresh_meta(&path, overlay, cache, queue)
        }
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
    respect_ignore: bool,
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
        walk_root_apply(path, overlay, roots, queue, status, respect_ignore);
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
