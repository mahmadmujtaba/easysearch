//! In-memory change overlay over the disk-backed base index.
//!
//! With the on-disk index enabled, this holds only *recent* filesystem changes
//! (creates, edits, deletes) — normally a handful of entries, so idle memory
//! stays tiny. With `persist_index=false` (no base index), the overlay *is*
//! the whole index.
//!
//! A path present in both `added` and `removed` is treated as removed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Metadata kept per indexed path (16 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meta {
    pub size: u64,
    /// Modification time in unix seconds.
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Default)]
pub struct Overlay {
    /// Newly created / modified files and dirs (path → current metadata).
    pub added: HashMap<PathBuf, Meta>,
    /// Deleted paths; the whole subtree is treated as gone.
    pub removed: HashSet<PathBuf>,
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay::default()
    }

    /// Insert or refresh a path. Idempotent; also cancels a pending removal.
    pub fn upsert(&mut self, path: PathBuf, meta: Meta) {
        self.removed.remove(&path);
        self.added.insert(path, meta);
    }

    /// Mark a path (and its whole subtree) as removed.
    pub fn remove(&mut self, path: &Path) {
        self.added.remove(path);
        self.removed.insert(path.to_path_buf());
    }

    /// True if `path` is (transitively) marked removed.
    pub fn is_removed(&self, path: &Path) -> bool {
        self.removed.iter().any(|r| path.starts_with(r))
    }

    /// Size of the pending change set (drives compaction).
    pub fn pending_changes(&self) -> usize {
        self.added.len() + self.removed.len()
    }

    /// Drop overlay entries that are now redundant against `base` (called
    /// after a compaction/rebuild so the overlay only holds true deltas).
    pub fn prune_against<F: Fn(&Path) -> bool>(&mut self, in_base: F) {
        self.added.retain(|p, _| !in_base(p));
        self.removed.retain(|p| in_base(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str) -> (PathBuf, Meta) {
        (
            PathBuf::from(path),
            Meta { size: 1, mtime: 0, is_dir: false },
        )
    }

    #[test]
    fn upsert_cancels_removal() {
        let mut o = Overlay::new();
        let (p, m) = f("/a/b.txt");
        o.remove(&p);
        assert!(o.is_removed(Path::new("/a/b.txt")));
        o.upsert(p.clone(), m);
        assert!(!o.is_removed(&p));
        assert!(o.added.contains_key(&p));
    }

    #[test]
    fn remove_covers_subtree() {
        let mut o = Overlay::new();
        o.remove(Path::new("/a"));
        assert!(o.is_removed(Path::new("/a/x/y.txt")));
        assert!(!o.is_removed(Path::new("/ab/c")));
        assert!(!o.is_removed(Path::new("/a.txt")));
    }

    #[test]
    fn prune_keeps_only_deltas() {
        let mut o = Overlay::new();
        let (p1, m1) = f("/new.txt");
        o.upsert(p1.clone(), m1);
        let (p2, m2) = f("/gone.txt");
        o.upsert(p2.clone(), m2);
        o.remove(Path::new("/gone.txt"));
        o.prune_against(|p| p == Path::new("/gone.txt"));
        // /new.txt is not in base → stays; /gone.txt is in base → stays removed
        assert!(o.added.contains_key(Path::new("/new.txt")));
        assert!(o.removed.contains(Path::new("/gone.txt")));
    }
}
