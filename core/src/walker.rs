//! Cold (and rebuild) filesystem walk using ripgrep's `ignore` walker.
//!
//! Results are either collected (`walk_root_collect`, for building the
//! on-disk index) or applied in batches to the change overlay
//! (`walk_root_apply`, for RAM-only mode and watcher-driven subtree indexing).
//! [`WalkOptions`] carries the two user-visible knobs: honoring
//! `.gitignore`/`.ignore` files (plus the global ignore file at
//! `~/.config/easysearch/ignore`) and following symbolic links.

use crate::content_index::ExtractQueue;
use crate::engine::Status;
use crate::overlay::{Meta, Overlay};
use crate::roots::RootSet;
use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

const BATCH: usize = 512;

/// Settings that shape a walk. Bundled into one value so adding an option does
/// not ripple through every walker/watcher signature.
#[derive(Clone, Debug, Default)]
pub struct WalkOptions {
    /// Honor `.gitignore`/`.ignore` files in the tree, plus the global ignore
    /// file at `~/.config/easysearch/ignore`.
    pub respect_ignore: bool,
    /// Follow symbolic links into their targets (cycles are detected by the
    /// walker and skipped).
    pub follow_symlinks: bool,
    /// Never descend into a dot-directory (`.git`, `.cache`, …). Hidden *files*
    /// are still indexed — the query's Hidden switch decides whether to show
    /// them — but a hidden directory's whole subtree stays out of the index.
    pub skip_hidden_dirs: bool,
    /// Directory *names* to skip anywhere in the tree (virtual environments,
    /// dependency trees, build caches). Shared so the thread pool can clone the
    /// walk options cheaply.
    pub exclude_names: Arc<crate::config::ExcludeNames>,
}

impl WalkOptions {
    pub fn new(respect_ignore: bool, follow_symlinks: bool) -> WalkOptions {
        WalkOptions {
            respect_ignore,
            follow_symlinks,
            skip_hidden_dirs: true,
            exclude_names: Arc::new(crate::config::ExcludeNames::default()),
        }
    }

    /// Set whether dot-directories are skipped (see [`Self::skip_hidden_dirs`]).
    pub fn with_skip_hidden_dirs(mut self, skip: bool) -> Self {
        self.skip_hidden_dirs = skip;
        self
    }

    /// Replace the excluded directory-name set.
    pub fn with_exclude_names(mut self, names: Arc<crate::config::ExcludeNames>) -> Self {
        self.exclude_names = names;
        self
    }

    /// A `filter_entry` predicate that prunes hidden directories (except `root`
    /// itself) and directories whose name is in [`WalkOptions::exclude_names`].
    fn dir_filter(
        &self,
        root: &Path,
    ) -> impl Fn(&ignore::DirEntry) -> bool + Send + Sync + 'static {
        let root = root.to_path_buf();
        let skip_hidden = self.skip_hidden_dirs;
        let exclude = Arc::clone(&self.exclude_names);
        move |entry: &ignore::DirEntry| {
            if entry.path() == root {
                return true; // an explicitly-configured root is never pruned
            }
            if !entry.file_type().is_some_and(|t| t.is_dir()) {
                return true; // files are filtered at query time
            }
            !prunes_dir(entry.path(), skip_hidden, &exclude)
        }
    }
}

/// Whether a directory is pruned during a walk: a dot-directory while hidden
/// trees are skipped, or a directory whose name is excluded.
fn prunes_dir(path: &Path, skip_hidden: bool, exclude: &crate::config::ExcludeNames) -> bool {
    if skip_hidden
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    {
        return true;
    }
    exclude.matches(path)
}

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
    opts: WalkOptions,
) -> Vec<(PathBuf, Meta)> {
    let out = Arc::new(Mutex::new(Vec::new()));
    walk_impl(
        root,
        roots,
        queue,
        status,
        opts,
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
    opts: WalkOptions,
) {
    walk_impl(
        root,
        roots,
        queue,
        status,
        opts,
        Sink::Apply(Arc::clone(overlay)),
    );
}

fn walk_impl(
    root: &Path,
    roots: &Arc<RootSet>,
    queue: Option<&Arc<ExtractQueue>>,
    status: &Arc<RwLock<Status>>,
    opts: WalkOptions,
    sink: Sink,
) {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // index hidden *files*; hidden dirs are pruned by dir_filter
        .follow_links(opts.follow_symlinks)
        .threads(
            std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4),
        );
    if opts.respect_ignore {
        builder
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false);
        if let Some(ig) = global_ignore_path()
            && let Some(err) = builder.add_ignore(ig)
        {
            eprintln!("global ignore: {err}");
        }
    } else {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }

    builder.filter_entry(opts.dir_filter(root));

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
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
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
            if let Some(q) = queue.as_deref()
                && !is_dir
            {
                q.send(pb);
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
pub fn collect_dirs(root: &Path, roots: &Arc<RootSet>, opts: WalkOptions) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(opts.follow_symlinks)
        .threads(1);
    if opts.respect_ignore {
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
    builder.filter_entry(opts.dir_filter(root));
    let mut dirs = Vec::new();
    for entry in builder.build() {
        let Ok(entry) = entry else { continue }; // unreadable: skip, don't fail
        if roots.is_excluded(entry.path()) {
            continue;
        }
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            dirs.push(entry.path().to_path_buf());
        }
    }
    dirs
}

/// Path of the global ignore file (`~/.config/easysearch/ignore`),
/// whether or not it exists yet. Patterns use gitignore syntax, matched
/// relative to the working directory (or use `**/` prefixes to match anywhere).
pub fn global_ignore_file() -> PathBuf {
    crate::config::xdg_config_dir()
        .join("easysearch")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ExcludeNames;

    #[test]
    fn hidden_and_excluded_directories_are_pruned() {
        let exclude = ExcludeNames::parse("node_modules,target,go/pkg/mod");

        // Dot-directories go when hidden trees are skipped.
        assert!(prunes_dir(Path::new("/u/.cache"), true, &exclude));
        assert!(prunes_dir(Path::new("/u/proj/.git"), true, &exclude));
        // …but stay when they are indexed on purpose.
        assert!(!prunes_dir(Path::new("/u/.config"), false, &exclude));

        // Excluded names, at any depth.
        assert!(prunes_dir(
            Path::new("/u/proj/node_modules"),
            true,
            &exclude
        ));
        assert!(prunes_dir(Path::new("/u/a/b/target"), true, &exclude));
        // Suffix items match the path tail only.
        assert!(prunes_dir(Path::new("/u/go/pkg/mod"), true, &exclude));
        assert!(!prunes_dir(Path::new("/u/pkg/mod"), true, &exclude));

        // Ordinary directories are kept.
        assert!(!prunes_dir(Path::new("/u/Documents"), true, &exclude));
    }
}
