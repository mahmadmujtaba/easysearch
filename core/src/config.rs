//! User configuration (JSON at `~/.config/easysearch/config.json`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_DIR: &str = "easysearch";
pub const CONFIG_FILE: &str = "config.json";

/// `$XDG_CONFIG_HOME` or `~/.config`.
pub(crate) fn xdg_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
}

/// Filesystem types that are never indexed (pseudo-filesystems, network,
/// overlays, ...). Network/tmpfs entries are listed here AND handled by the
/// `exclude_*` toggles; keeping them in the default list makes the exclusions
/// effective even if the toggles are changed.
pub const DEFAULT_EXCLUDED_FSTYPES: &[&str] = &[
    // pseudo / kernel
    "proc",
    "sysfs",
    "devpts",
    "devtmpfs",
    "securityfs",
    "debugfs",
    "tracefs",
    "pstore",
    "bpf",
    "cgroup",
    "cgroup2",
    "mqueue",
    "hugetlbfs",
    "configfs",
    "fusectl",
    "efivarfs",
    "ramfs",
    "binfmt_misc",
    "rpc_pipefs",
    "autofs",
    // overlays / special
    "overlay",
    "squashfs",
    "iso9660",
    // network
    "nfs",
    "nfs4",
    "smbfs",
    "cifs",
    "sshfs",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.smb",
    "gvfsd-fuse",
    "9p",
];

/// Which storage backend holds the index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    /// A SQLite database in `db_dir` (the default).
    #[default]
    Sqlite,
    /// The original memory-mapped file in `disk_index_dir`.
    Mmap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Roots to index. Empty => `$HOME` of the running user.
    pub roots: Vec<String>,
    /// Exclude removable media (USB etc.) detected via /sys/block removable flag.
    pub exclude_removable: bool,
    /// Exclude network mounts (nfs, smb/cifs, sshfs, ...).
    pub exclude_network: bool,
    /// Directory subtrees to leave out of the index, wherever they sit under a
    /// root (e.g. `~/VirtualBox VMs`, `/srv/scratch`). `~` expands to `$HOME`
    /// and a relative path is taken from `$HOME`. This is a *path* exclusion,
    /// independent of the fstype exclusions above; it is editable live in the
    /// GUI (**Tools ▸ Excluded folders…**), which saves it back here.
    pub exclude_dirs: Vec<String>,
    /// File of directory *names* to skip anywhere in the tree, comma-separated
    /// (`node_modules, .venv, venv, target, …`). `~` expands to `$HOME`; the
    /// default is `$XDG_CONFIG_HOME/easysearch/exclude-names`, seeded with a
    /// well-populated common list on first run. Unlike [`Self::exclude_dirs`]
    /// (exact paths), a name here matches that directory at any depth.
    pub exclude_names_file: Option<String>,
    /// Index dot-directories (`.git`, `.cache`, …). Off by default: hidden trees
    /// are the largest part of a home directory and rarely searched, so leaving
    /// them out keeps the index — and the SQLite tables — lean. Hidden *files*
    /// in visible directories are always indexed.
    pub index_hidden_dirs: bool,
    /// Honor `.ignore`/`.gitignore` files (and the global ignore file at
    /// `~/.config/easysearch/ignore`) while walking, so non-essential
    /// folders listed there are never indexed.
    pub respect_ignore_files: bool,
    /// Follow symbolic links into their targets while walking. Off by default:
    /// it can duplicate whole subtrees, and cycles are only detected by the
    /// walker skipping them. Live-toggleable in the GUI (Ignore files dialog).
    pub follow_symlinks: bool,
    /// Keep the index on disk (memory-mapped) instead of entirely in RAM, and
    /// keep only recent filesystem changes in memory. See docs/scope.md §9.
    pub persist_index: bool,
    /// Where the index lives: a SQLite database (the default, see
    /// `docs/scope.md`) or the original memory-mapped file.
    pub storage: Storage,
    /// Directory holding the SQLite database (default:
    /// `$XDG_CACHE_HOME/easysearch/db`).
    pub db_dir: Option<String>,
    /// Directory for the on-disk index (default: `$XDG_CACHE_HOME/easysearch`).
    pub disk_index_dir: Option<String>,
    /// When the in-memory change overlay exceeds this many entries, it is
    /// compacted into the on-disk index in the background.
    pub overlay_compaction_threshold: usize,
    /// Extra filesystem types to exclude (union of DEFAULT_EXCLUDED_FSTYPES).
    pub exclude_fstypes: Vec<String>,
    /// Optional background content cache. Default off: content queries read
    /// live data via the embedded ripgrep engine (always fresh).
    pub content_index_enabled: bool,
    /// Skip content extraction for files larger than this (bytes).
    pub content_index_max_file_bytes: u64,
    /// Total content cache cap (bytes). LRU eviction beyond this. In the default
    /// disk store this bounds the spool directory; with `--content-in-memory` it
    /// bounds resident memory.
    pub content_index_total_cap_bytes: u64,
    /// Keep cached document text **in RAM** instead of spooling it to disk.
    ///
    /// Off by default: with the disk store a content search reads each document
    /// back, uses it and drops it, so the documents searched do not stay
    /// resident. Turning this on trades up to `content_index_total_cap_bytes` of
    /// memory for another pass not having to touch the disk. Set it at boot with
    /// `--content-in-memory`, or `EASYSEARCH_CONTENT_MEMORY=1` — which is how a
    /// launcher flag reaches the engine child the app spawns.
    pub content_index_in_memory: bool,
    /// Period of the degraded-mode rescan (seconds), used when kernel watch
    /// limits are exhausted.
    pub degraded_rescan_secs: u64,
    /// Maximum number of rows returned per query.
    pub max_results: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            roots: Vec::new(),
            exclude_removable: true,
            exclude_network: true,
            exclude_dirs: Vec::new(),
            exclude_names_file: None,
            index_hidden_dirs: false,
            respect_ignore_files: true,
            follow_symlinks: false,
            persist_index: true,
            storage: Storage::default(),
            db_dir: None,
            disk_index_dir: None,
            overlay_compaction_threshold: 8192,
            exclude_fstypes: Vec::new(),
            content_index_enabled: false,
            content_index_max_file_bytes: 8 * 1024 * 1024,
            content_index_total_cap_bytes: 256 * 1024 * 1024,
            content_index_in_memory: false,
            degraded_rescan_secs: 30,
            max_results: 1000,
        }
    }
}

/// The default exclude-names list, shipped as a data file and written to
/// [`Config::exclude_names_path`] on first run.
pub const DEFAULT_EXCLUDE_NAMES: &str = include_str!("../data/exclude-names");

/// A parsed exclude-names list: directory names (and path suffixes) skipped
/// anywhere in the tree.
#[derive(Clone, Debug, Default)]
pub struct ExcludeNames {
    /// Bare names, matched against a directory's own name.
    simple: std::collections::HashSet<String>,
    /// Slash-separated items (`go/pkg/mod`), matched against a path's tail.
    suffixes: Vec<Vec<String>>,
}

impl ExcludeNames {
    /// Parse the comma/newline-separated list. `#` starts a comment; a trailing
    /// slash is ignored, so `node_modules/` and `node_modules` are the same.
    pub fn parse(text: &str) -> ExcludeNames {
        let mut names = ExcludeNames::default();
        for raw in text.split([',', '\n', '\r']) {
            let item = raw.trim().trim_end_matches('/');
            if item.is_empty() || item.starts_with('#') {
                continue;
            }
            if item.contains('/') {
                names
                    .suffixes
                    .push(item.split('/').map(str::to_string).collect());
            } else {
                names.simple.insert(item.to_string());
            }
        }
        names
    }

    pub fn is_empty(&self) -> bool {
        self.simple.is_empty() && self.suffixes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.simple.len() + self.suffixes.len()
    }

    /// The configured items, sorted, for a stable fingerprint.
    pub fn items(&self) -> Vec<String> {
        let mut items: Vec<String> = self.simple.iter().cloned().collect();
        for suffix in &self.suffixes {
            items.push(suffix.join("/"));
        }
        items.sort();
        items
    }

    /// True when `path` names an excluded directory.
    pub fn matches(&self, path: &Path) -> bool {
        if let Some(name) = path.file_name()
            && self.simple.contains(name.to_string_lossy().as_ref())
        {
            return true;
        }
        if self.suffixes.is_empty() {
            return false;
        }
        let comps: Vec<String> = path
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        self.suffixes.iter().any(|suffix| {
            comps.len() >= suffix.len() && comps[comps.len() - suffix.len()..] == suffix[..]
        })
    }
}

/// Expand a leading `~` to `$HOME`; anything else is used as-is.
fn expand_home(raw: &str) -> PathBuf {
    if let (Some(rest), Some(home)) = (raw.strip_prefix("~/"), std::env::var_os("HOME")) {
        return PathBuf::from(home).join(rest);
    }
    if raw == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(raw)
}

impl Config {
    /// Default location: `$XDG_CONFIG_HOME/easysearch/config.json`.
    pub fn default_path() -> PathBuf {
        let base = xdg_config_dir();
        base.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// Default location of the on-disk index: `$XDG_CACHE_HOME/easysearch`.
    pub fn default_disk_index_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".cache"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        base.join(CONFIG_DIR)
    }

    /// Effective directory for the SQLite database (the `db/` folder).
    ///
    /// Defaults to a `db` folder beside the mmap index, so a caller that
    /// redirects `disk_index_dir` (tests, portable installs) redirects the
    /// database with it.
    pub fn db_dir(&self) -> PathBuf {
        match &self.db_dir {
            Some(d) => PathBuf::from(d),
            None => self.disk_index_dir().join("db"),
        }
    }

    /// Effective on-disk index directory.
    pub fn disk_index_dir(&self) -> PathBuf {
        match &self.disk_index_dir {
            Some(d) => PathBuf::from(d),
            None => Self::default_disk_index_dir(),
        }
    }

    /// Directory the content cache spools to when it is not holding text in RAM.
    ///
    /// Beside the index and the database, so a caller that redirects
    /// `disk_index_dir` (tests, portable installs) redirects it too.
    pub fn content_spool_dir(&self) -> PathBuf {
        self.disk_index_dir().join("content")
    }

    /// Load config from the default path; returns defaults if absent or invalid.
    pub fn load() -> Config {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &std::path::Path) -> Config {
        let mut config = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("config: ignoring invalid {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        };
        // Boot-time override: the launcher passes the flag through the
        // environment so it reaches the engine child the app spawns.
        if let Some(on) =
            content_memory_from_env(std::env::var("EASYSEARCH_CONTENT_MEMORY").ok().as_deref())
        {
            config.content_index_in_memory = on;
        }
        config
    }

    /// Effective exclusion list: defaults + user extras.
    pub fn excluded_fstypes(&self) -> Vec<String> {
        let mut v: Vec<String> = DEFAULT_EXCLUDED_FSTYPES
            .iter()
            .map(|s| s.to_string())
            .collect();
        for extra in &self.exclude_fstypes {
            if !v.iter().any(|s| s == extra) {
                v.push(extra.clone());
            }
        }
        v
    }

    /// Effective search roots (resolved, deduplicated).
    pub fn effective_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = if self.roots.is_empty() {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            vec![home]
        } else {
            self.roots.iter().map(PathBuf::from).collect()
        };
        for r in roots.iter_mut() {
            *r = r.canonicalize().unwrap_or_else(|_| r.clone());
        }
        roots.sort();
        roots.dedup();
        roots
    }

    /// Path of the exclude-names file: the configured one (`~` expanded), or
    /// `$XDG_CONFIG_HOME/easysearch/exclude-names` by default.
    pub fn exclude_names_path(&self) -> PathBuf {
        match self
            .exclude_names_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => expand_home(raw),
            None => xdg_config_dir().join(CONFIG_DIR).join("exclude-names"),
        }
    }

    /// The excluded directory names, read from [`Self::exclude_names_path`]. A
    /// missing file falls back to the shipped [`DEFAULT_EXCLUDE_NAMES`], so a
    /// first run already skips virtual environments and dependency trees.
    pub fn effective_exclude_names(&self) -> ExcludeNames {
        match std::fs::read_to_string(self.exclude_names_path()) {
            Ok(text) => ExcludeNames::parse(&text),
            Err(_) => ExcludeNames::parse(DEFAULT_EXCLUDE_NAMES),
        }
    }

    /// A fingerprint of the settings that decide *which paths* are indexed.
    ///
    /// Stored in the SQLite `meta` table as `walk_key`: when it changes (a new
    /// root or exclusion, hidden directories turned on), the next start rebuilds
    /// the index so the tables never keep rows the walk no longer produces.
    pub fn walk_fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut parts: Vec<String> = Vec::new();
        for root in self.effective_roots() {
            parts.push(format!("root:{}", root.display()));
        }
        for dir in self.effective_exclude_dirs() {
            parts.push(format!("dir:{}", dir.display()));
        }
        for name in self.effective_exclude_names().items() {
            parts.push(format!("name:{name}"));
        }
        parts.push(format!("hidden:{}", self.index_hidden_dirs));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        parts.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Write the default exclude-names file when it does not exist yet. An
    /// existing file (edited, emptied, or deleted by choice) is never touched.
    pub fn ensure_exclude_names_file(&self) -> std::io::Result<()> {
        let path = self.exclude_names_path();
        if path.exists() {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, DEFAULT_EXCLUDE_NAMES)
    }

    /// Resolved directory exclusions: `~` expanded, relative paths taken from
    /// `$HOME`, made absolute and deduplicated. A path that does not exist yet
    /// is kept as-is, so it starts excluding as soon as it appears.
    pub fn effective_exclude_dirs(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut dirs: Vec<PathBuf> = Vec::new();
        for raw in &self.exclude_dirs {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
                home.as_ref().map(|h| h.join(rest)).unwrap_or_default()
            } else if trimmed == "~" {
                home.clone().unwrap_or_default()
            } else {
                PathBuf::from(trimmed)
            };
            let expanded = if expanded.as_os_str().is_empty() {
                PathBuf::from(trimmed)
            } else {
                expanded
            };
            let absolute = if expanded.is_absolute() {
                expanded
            } else {
                home.as_ref().map(|h| h.join(&expanded)).unwrap_or(expanded)
            };
            let path = absolute.canonicalize().unwrap_or(absolute);
            if !dirs.contains(&path) {
                dirs.push(path);
            }
        }
        dirs
    }

    /// Write this config to the default path (pretty JSON, atomically).
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::default_path())
    }

    /// Write this config to `path` (pretty JSON, atomically).
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Parse the `EASYSEARCH_CONTENT_MEMORY` boot override.
///
/// `Some(true)`/`Some(false)` for a recognised value, `None` when the variable
/// is unset or meaningless — in which case the config file decides.
fn content_memory_from_env(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_content_memory_boot_flag_is_parsed_leniently() {
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert_eq!(content_memory_from_env(Some(on)), Some(true), "{on}");
        }
        for off in ["0", "false", "No", "off"] {
            assert_eq!(content_memory_from_env(Some(off)), Some(false), "{off}");
        }
        assert_eq!(content_memory_from_env(None), None);
        assert_eq!(content_memory_from_env(Some("maybe")), None);
    }

    #[test]
    fn exclude_dirs_are_canonicalised_and_deduplicated() {
        let dir = std::env::temp_dir().join(format!("easysearch-exclude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config {
            exclude_dirs: vec![
                dir.display().to_string(),
                dir.display().to_string(), // duplicate
                "   ".into(),              // blank
            ],
            ..Config::default()
        };
        let dirs = config.effective_exclude_dirs();
        assert_eq!(
            dirs.len(),
            1,
            "blank and duplicate entries collapse: {dirs:?}"
        );
        assert_eq!(dirs[0], dir.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exclude_dirs_expand_a_leading_tilde() {
        let Some(home) = std::env::var_os("HOME") else {
            return; // no HOME in this environment: nothing to assert
        };
        let config = Config {
            exclude_dirs: vec!["~/work/scratch".into()],
            ..Config::default()
        };
        let dirs = config.effective_exclude_dirs();
        assert_eq!(dirs.len(), 1);
        assert!(
            dirs[0].starts_with(PathBuf::from(home).canonicalize().unwrap_or_default()),
            "{dirs:?}"
        );
        assert!(dirs[0].ends_with("work/scratch"), "{dirs:?}");
    }

    #[test]
    fn the_content_spool_sits_beside_the_index() {
        let config = Config {
            disk_index_dir: Some("/tmp/idx".into()),
            ..Config::default()
        };
        assert_eq!(
            config.content_spool_dir(),
            PathBuf::from("/tmp/idx/content")
        );
        assert!(Config::default().content_spool_dir().ends_with("content"));
    }

    #[test]
    fn the_content_cache_is_off_and_disk_backed_by_default() {
        let config = Config::default();
        assert!(!config.content_index_enabled, "off at boot");
        assert!(!config.content_index_in_memory, "disk store by default");
    }

    #[test]
    fn exclude_names_parse_comma_newline_comments_and_slashes() {
        let names = ExcludeNames::parse(
            "# a comment, but comma-free is the rule\n\
             node_modules/,.venv, venv \n\
             go/pkg/mod,target\n",
        );
        assert!(names.matches(Path::new("/home/u/proj/node_modules")));
        assert!(names.matches(Path::new("/home/u/.venv")));
        assert!(names.matches(Path::new("/home/u/app/venv")));
        assert!(names.matches(Path::new("/home/u/proj/target")));
        // A suffix item matches the path tail, not a bare name.
        assert!(names.matches(Path::new("/home/u/go/pkg/mod")));
        assert!(!names.matches(Path::new("/home/u/pkg/mod")));
        assert!(!names.matches(Path::new("/home/u/project")));
        // The comment did not leak a name.
        assert!(!names.matches(Path::new("/home/u/comment")));
    }

    #[test]
    fn the_default_exclude_list_is_well_populated() {
        let names = ExcludeNames::parse(DEFAULT_EXCLUDE_NAMES);
        assert!(names.len() >= 40, "only {} items", names.len());
        for wanted in [
            "node_modules",
            ".venv",
            "venv",
            "__pycache__",
            "target",
            ".gradle",
            ".dart_tool",
        ] {
            assert!(
                names.matches(Path::new(&format!("/x/{wanted}"))),
                "default list is missing {wanted}"
            );
        }
    }
}
