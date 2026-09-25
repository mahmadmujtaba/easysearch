//! User configuration (JSON at `~/.config/everything-linux/config.json`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CONFIG_DIR: &str = "everything-linux";
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
    /// Honor `.ignore`/`.gitignore` files (and the global ignore file at
    /// `~/.config/everything-linux/ignore`) while walking, so non-essential
    /// folders listed there are never indexed.
    pub respect_ignore_files: bool,
    /// Keep the index on disk (memory-mapped) instead of entirely in RAM, and
    /// keep only recent filesystem changes in memory. See docs/scope.md §9.
    pub persist_index: bool,
    /// Where the index lives: a SQLite database (the default, see
    /// `docs/sqlite.md`) or the original memory-mapped file.
    pub storage: Storage,
    /// Directory holding the SQLite database (default:
    /// `$XDG_CACHE_HOME/everything-linux/db`).
    pub db_dir: Option<String>,
    /// Directory for the on-disk index (default: `$XDG_CACHE_HOME/everything-linux`).
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
    /// Total in-RAM content cache cap (bytes). LRU eviction beyond this.
    pub content_index_total_cap_bytes: u64,
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
            respect_ignore_files: true,
            persist_index: true,
            storage: Storage::default(),
            db_dir: None,
            disk_index_dir: None,
            overlay_compaction_threshold: 8192,
            exclude_fstypes: Vec::new(),
            content_index_enabled: false,
            content_index_max_file_bytes: 8 * 1024 * 1024,
            content_index_total_cap_bytes: 256 * 1024 * 1024,
            degraded_rescan_secs: 30,
            max_results: 1000,
        }
    }
}

impl Config {
    /// Default location: `$XDG_CONFIG_HOME/everything-linux/config.json`.
    pub fn default_path() -> PathBuf {
        let base = xdg_config_dir();
        base.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// Default location of the on-disk index: `$XDG_CACHE_HOME/everything-linux`.
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

    /// Load config from the default path; returns defaults if absent or invalid.
    pub fn load() -> Config {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &std::path::Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("config: ignoring invalid {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
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
}
