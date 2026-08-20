//! Cold (and rescan) filesystem walk using ripgrep's `ignore` walker.
//!
//! Results are batched and applied to the shared index under a short write
//! lock, so queries can run while indexing is still in progress.

use crate::content_index::ExtractQueue;
use crate::engine::Status;
use crate::index::{Index, Meta};
use crate::roots::RootSet;
use ignore::gitignore::GitignoreBuilder;
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
        if let Some(ig) = load_global_ignore(root) {
            builder.add_ignore(ig);
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
        Box::new(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    bump_skipped(status);
                    return WalkState::Continue;
                }
            };
            let path = entry.path();
            if roots.is_excluded(path) {
                let is_dir = entry.file_type().map_or(false, |t| t.is_dir());
                return if is_dir { WalkState::Skip } else { WalkState::Continue };
            }
            let meta = match entry.metadata() {
