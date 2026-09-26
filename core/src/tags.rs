//! Per-path tags — a small, local overlay on the filename index.
//!
//! Tags are a *client-side* concern: the engine indexes names and content, but
//! “this file is work” or “that folder is archived” is user metadata that must
//! survive across runs, so it lives in a JSON file the app owns rather than in
//! the index. The store is deliberately tiny and forgiving — a missing or
//! corrupt file loads as empty, an unknown field is ignored, and removing the
//! last tag for a path drops the entry — so it can never make the app fail to
//! start.
//!
//! Paths are stored as the user's filesystem spells them (exactly the bytes the
//! engine returned), so a tag lines up with a result regardless of symlinks.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The persisted shape. `version` is written for forward compatibility; a file
/// from the future still loads (the extra fields are ignored).
#[derive(Default, Serialize, Deserialize)]
struct OnDisk {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    tags: BTreeMap<String, BTreeSet<String>>,
}

fn default_version() -> u32 {
    1
}

/// A persisted map of path → tag set.
pub struct TagStore {
    path: PathBuf,
    map: BTreeMap<PathBuf, BTreeSet<String>>,
    dirty: bool,
}

impl TagStore {
    /// Load the store at `path`. A missing, unreadable or malformed file yields
    /// an empty store (never an error), so tagging can never block startup.
    pub fn load(path: PathBuf) -> TagStore {
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<OnDisk>(&text).ok())
            .map(|disk| {
                disk.tags
                    .into_iter()
                    .filter_map(|(p, tags)| {
                        let tags: BTreeSet<String> =
                            tags.into_iter().filter_map(|t| clean(&t)).collect();
                        if tags.is_empty() {
                            None
                        } else {
                            Some((PathBuf::from(p), tags))
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        TagStore {
            path,
            map,
            dirty: false,
        }
    }

    /// The default location: `$XDG_CONFIG_HOME/easysearch/tags.json` (i.e.
    /// `~/.config/easysearch/tags.json`), beside `gui.json`.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".config"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        base.join("easysearch").join("tags.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The number of tagged paths.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Tags on `path`, if any.
    pub fn tags_of(&self, path: &Path) -> Option<&BTreeSet<String>> {
        self.map.get(path)
    }

    /// Does `path` carry `tag`?
    pub fn has(&self, path: &Path, tag: &str) -> bool {
        match clean(tag) {
            Some(tag) => self.map.get(path).is_some_and(|s| s.contains(&tag)),
            None => false,
        }
    }

    /// Replace `path`'s tags. An empty set drops the entry. Returns true when
    /// something changed.
    pub fn set(&mut self, path: &Path, tags: impl IntoIterator<Item = String>) -> bool {
        let next: BTreeSet<String> = tags.into_iter().filter_map(|t| clean(&t)).collect();
        let before = self.map.get(path).cloned();
        if next.is_empty() {
            self.map.remove(path);
        } else {
            self.map.insert(path.to_path_buf(), next.clone());
        }
        let changed = before.as_ref() != if next.is_empty() { None } else { Some(&next) };
        self.dirty |= changed;
        changed
    }

    /// Add one tag to `path`. Returns true when the tag was not already there.
    pub fn add(&mut self, path: &Path, tag: &str) -> bool {
        let Some(tag) = clean(tag) else {
            return false;
        };
        let inserted = self.map.entry(path.to_path_buf()).or_default().insert(tag);
        self.dirty |= inserted;
        inserted
    }

    /// Remove one tag from `path`, dropping the entry if it was the last one.
    /// Returns true when the tag was present.
    pub fn remove(&mut self, path: &Path, tag: &str) -> bool {
        let Some(tag) = clean(tag) else {
            return false;
        };
        let Some(set) = self.map.get_mut(path) else {
            return false;
        };
        let removed = set.remove(&tag);
        if set.is_empty() {
            self.map.remove(path);
        }
        self.dirty |= removed;
        removed
    }

    /// Every tag in use, with the number of paths carrying it.
    pub fn all(&self) -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for tags in self.map.values() {
            for tag in tags {
                *counts.entry(tag.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Paths carrying `tag`.
    pub fn paths_with<'a>(&'a self, tag: &str) -> Vec<&'a Path> {
        let Some(tag) = clean(tag) else {
            return Vec::new();
        };
        self.map
            .iter()
            .filter(|(_, tags)| tags.contains(&tag))
            .map(|(p, _)| p.as_path())
            .collect()
    }

    /// Drop tags for paths that no longer exist. Returns how many were dropped.
    pub fn prune_missing(&mut self) -> usize {
        let before = self.map.len();
        self.map.retain(|p, _| p.exists());
        let dropped = before - self.map.len();
        self.dirty |= dropped > 0;
        dropped
    }

    /// Persist if there are unsaved changes. Written atomically (a temp file
    /// renamed over the target) so an interrupted save cannot corrupt the file.
    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let disk = OnDisk {
            version: default_version(),
            tags: self
                .map
                .iter()
                .map(|(p, tags)| (p.to_string_lossy().into_owned(), tags.clone()))
                .collect(),
        };
        let text = serde_json::to_string_pretty(&disk)?;
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }
}

/// A tag as it is stored: trimmed, without a leading `#`, and never empty or
/// containing control characters. `None` means “not a usable tag”.
fn clean(tag: &str) -> Option<String> {
    let t = tag.trim().trim_start_matches('#').trim();
    if t.is_empty() || t.chars().any(char::is_control) {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("easysearch-tags-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir.join("tags.json"))
        }
        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn add_remove_and_query() {
        let s = Scratch::new("basic");
        let mut store = TagStore::load(s.path());
        let a = Path::new("/tmp/a.txt");
        let b = Path::new("/tmp/b.txt");

        assert!(store.add(a, "work"));
        assert!(!store.add(a, "work"), "adding twice is a no-op");
        store.add(a, "urgent");
        store.add(b, "work");

        assert!(store.has(a, "work"));
        assert!(store.has(a, "#work"), "a leading # is accepted");
        assert!(!store.has(b, "urgent"));
        assert_eq!(
            store
                .tags_of(a)
                .unwrap()
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["urgent", "work"]
        );

        assert_eq!(store.all().get("work"), Some(&2));
        assert_eq!(store.all().get("urgent"), Some(&1));
        let mut work = store.paths_with("work");
        work.sort();
        assert_eq!(work, vec![a, b]);

        assert!(store.remove(a, "work"));
        assert!(!store.has(a, "work"));
        assert!(!store.remove(a, "work"), "removing again changes nothing");
    }

    #[test]
    fn the_last_tag_drops_the_entry() {
        let s = Scratch::new("drop");
        let mut store = TagStore::load(s.path());
        let p = Path::new("/tmp/only.txt");
        store.add(p, "solo");
        assert_eq!(store.len(), 1);
        store.remove(p, "solo");
        assert!(store.tags_of(p).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn empty_names_are_rejected() {
        let s = Scratch::new("empty");
        let mut store = TagStore::load(s.path());
        let p = Path::new("/tmp/x");
        assert!(!store.add(p, "   "));
        assert!(!store.add(p, "#"));
        assert!(store.is_empty());
    }

    #[test]
    fn set_replaces_the_whole_set() {
        let s = Scratch::new("set");
        let mut store = TagStore::load(s.path());
        let p = Path::new("/tmp/x");
        store.add(p, "old");
        store.set(p, vec!["new".to_string(), "other".to_string()]);
        let tags = store.tags_of(p).unwrap();
        assert!(!tags.contains("old"));
        assert_eq!(tags.len(), 2);
        store.set(p, Vec::<String>::new());
        assert!(store.tags_of(p).is_none());
    }

    #[test]
    fn a_corrupt_file_loads_empty() {
        let s = Scratch::new("corrupt");
        std::fs::write(s.path(), b"{ not json at all").unwrap();
        let store = TagStore::load(s.path());
        assert!(store.is_empty());
    }

    #[test]
    fn save_and_reload_round_trips() {
        let s = Scratch::new("roundtrip");
        let p = Path::new("/home/me/report.pdf");
        {
            let mut store = TagStore::load(s.path());
            store.add(p, "work");
            store.add(p, "2026");
            store.save().unwrap();
        }
        let store = TagStore::load(s.path());
        assert!(store.has(p, "work"));
        assert!(store.has(p, "2026"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn saving_without_changes_writes_nothing() {
        let s = Scratch::new("nodirty");
        let mut store = TagStore::load(s.path());
        store.save().unwrap();
        assert!(!s.path().exists(), "a clean store is not written");
    }

    #[test]
    fn prune_drops_paths_that_no_longer_exist() {
        let s = Scratch::new("prune");
        let live = std::env::temp_dir().join(format!("easysearch-live-{}", std::process::id()));
        std::fs::write(&live, b"x").unwrap();
        let mut store = TagStore::load(s.path());
        store.add(&live, "keep");
        store.add(Path::new("/definitely/not/here"), "gone");
        assert_eq!(store.len(), 2);
        assert_eq!(store.prune_missing(), 1);
        assert_eq!(store.len(), 1);
        assert!(store.has(&live, "keep"));
        let _ = std::fs::remove_file(&live);
    }
}
