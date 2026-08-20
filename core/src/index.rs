//! In-memory path index.
//!
//! A `Vec` of entries (append-only, walk order) plus a `HashMap` from path to
//! position for O(1) updates from the watcher. Queries do a linear scan, which
//! is sub-50 ms at ~1M entries with glob/regex matching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default)]
pub struct Meta {
    pub size: u64,
    /// Modification time in unix seconds.
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub meta: Meta,
}

#[derive(Default)]
pub struct Index {
    entries: Vec<IndexEntry>,
    by_path: HashMap<PathBuf, usize>,
    pub files: u64,
    pub dirs: u64,
}

impl Index {
    pub fn new() -> Index {
        Index::default()
    }

    /// Insert or update an entry. Idempotent — safe to call from both the walker
    /// and the watcher concurrently (caller holds the write lock).
    pub fn upsert(&mut self, path: PathBuf, meta: Meta) {
        if let Some(&pos) = self.by_path.get(&path) {
            let prev = &self.entries[pos].meta;
            if prev.is_dir != meta.is_dir {
                // A file became a dir or vice versa: adjust the counters.
                if meta.is_dir {
                    self.files = self.files.saturating_sub(1);
                    self.dirs += 1;
                } else {
                    self.dirs = self.dirs.saturating_sub(1);
                    self.files += 1;
                }
            }
            self.entries[pos].meta = meta;
            return;
        }
        if meta.is_dir {
            self.dirs += 1;
        } else {
            self.files += 1;
        }
        let pos = self.entries.len();
        self.entries.push(IndexEntry { path, meta });
        self.by_path.insert(self.entries[pos].path.clone(), pos);
    }

    pub fn get(&self, path: &Path) -> Option<&IndexEntry> {
        self.by_path.get(path).map(|&pos| &self.entries[pos])
    }

    /// Remove one path (and adjust counters). No-op if absent.
    pub fn remove(&mut self, path: &Path) {
        if let Some(&pos) = self.by_path.get(path) {
            let e = &self.entries[pos];
            if e.meta.is_dir {
                self.dirs = self.dirs.saturating_sub(1);
            } else {
                self.files = self.files.saturating_sub(1);
            }
            self.remove_at(pos);
        }
    }

    /// Remove a path and every descendant (used for dir deletes/renames).
    pub fn remove_subtree(&mut self, path: &Path) {
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].path == path || self.entries[i].path.starts_with(path) {
                let e = &self.entries[i];
                if e.meta.is_dir {
                    self.dirs = self.dirs.saturating_sub(1);
                } else {
                    self.files = self.files.saturating_sub(1);
                }
                self.remove_at(i);
                // remove_at swap-removes; re-check the same slot.
            } else {
                i += 1;
            }
        }
    }

    /// Drop everything not present in `seen` (degraded-mode reconciliation).
    pub fn retain_known(&mut self, seen: &std::collections::HashSet<PathBuf>) {
        let mut i = 0;
        while i < self.entries.len() {
            if !seen.contains(&self.entries[i].path) {
                let e = &self.entries[i];
                if e.meta.is_dir {
                    self.dirs = self.dirs.saturating_sub(1);
                } else {
                    self.files = self.files.saturating_sub(1);
                }
                self.remove_at(i);
            } else {
                i += 1;
            }
        }
    }

    fn remove_at(&mut self, pos: usize) {
        let last = self.entries.len() - 1;
        self.by_path.remove(&self.entries[pos].path);
        if pos != last {
            self.entries.swap(pos, last);
            let moved = &self.entries[pos];
            self.by_path.insert(moved.path.clone(), pos);
        }
        self.entries.pop();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// All file paths (skips directories). Used to fan out content-only
    /// searches to the parallel searcher.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|e| !e.meta.is_dir)
            .map(|e| e.path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str) -> (PathBuf, Meta) {
        (
            PathBuf::from(path),
            Meta {
                size: 1,
                mtime: 0,
                is_dir: false,
            },
        )
    }

    #[test]
    fn upsert_update_remove() {
        let mut idx = Index::new();
        let (p1, m1) = f("/a/b.txt");
        idx.upsert(p1.clone(), m1);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.files, 1);

        // update meta
        idx.upsert(p1.clone(), Meta { size: 9, mtime: 5, is_dir: false });
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.files, 1);
        assert_eq!(idx.get(Path::new("/a/b.txt")).unwrap().meta.size, 9);

        idx.remove(Path::new("/a/b.txt"));
        assert!(idx.is_empty());
        assert_eq!(idx.files, 0);
        idx.remove(Path::new("/a/b.txt")); // idempotent
    }

    #[test]
    fn subtree_removal() {
        let mut idx = Index::new();
        idx.upsert(f("/root").0.clone(), Meta { size: 0, mtime: 0, is_dir: true });
        let (p, m) = f("/root/x.txt");
        idx.upsert(p, m);
        let (p, m) = f("/root/deep/y.txt");
        idx.upsert(p, m);
        let (p, m) = f("/root2/z.txt");
        idx.upsert(p, m);
        idx.remove_subtree(Path::new("/root"));
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.files, 1); // only /root2/z.txt remains
        assert_eq!(idx.dirs, 0);
    }

    #[test]
    fn retain_known_drops_missing() {
        let mut idx = Index::new();
        for p in ["/a.txt", "/b.txt", "/c.txt"] {
            let (path, meta) = f(p);
            idx.upsert(path, meta);
        }
        let mut seen = std::collections::HashSet::new();
        seen.insert(PathBuf::from("/a.txt"));
        seen.insert(PathBuf::from("/c.txt"));
        idx.retain_known(&seen);
        assert_eq!(idx.len(), 2);
        assert!(idx.get(Path::new("/a.txt")).is_some());
        assert!(idx.get(Path::new("/c.txt")).is_some());
    }
}
