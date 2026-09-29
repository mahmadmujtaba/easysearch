//! Mount discovery + exclusion set.
//!
//! Reads `/proc/self/mounts` and decides which subtrees may be indexed:
//! pseudo-filesystems, network mounts and removable media are excluded by
//! default (see `docs/scope.md` §6).

use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct Mount {
    pub point: PathBuf,
    pub device: String,
    pub fstype: String,
}

/// Which subtrees must never be entered.
#[derive(Debug, Default)]
pub struct RootSet {
    /// Roots that are searched (already canonicalized). Shared and mutable so the
    /// GUI can add or remove a root without an engine restart.
    roots: Arc<RwLock<Vec<PathBuf>>>,
    /// Mount points (within or overlapping the roots) that are excluded.
    excluded: Vec<Mount>,
    /// Directory subtrees to skip (from `config.exclude_dirs`, resolved). Shared
    /// and mutable so the GUI can change the list without an engine restart —
    /// the next walk picks it up.
    exclude_dirs: Arc<RwLock<Vec<PathBuf>>>,
}

impl RootSet {
    /// Discover mounts and compute the exclusion set for the given config.
    pub fn discover(config: &Config) -> RootSet {
        let mounts = read_mounts();
        let excluded_fstypes = config.excluded_fstypes();

        let roots = config.effective_roots();
        let mut excluded: Vec<Mount> = Vec::new();
        for m in mounts {
            // A configured root is always indexed, even on an excluded
            // fstype (e.g. explicitly indexing /tmp on tmpfs). Excluded
            // mounts that merely contain a root (like /tmp containing the
            // root /tmp/xxx) must not veto the walk either.
            if roots
                .iter()
                .any(|r| r == &m.point || r.starts_with(&m.point))
            {
                continue;
            }
            let type_excluded = excluded_fstypes.iter().any(|t| t == &m.fstype);
            let removable = config.exclude_removable && is_removable(&m.device);
            let network = config.exclude_network && is_network_fstype(&m.fstype);
            let under_media = config.exclude_removable
                && (m.point.starts_with("/media/") || m.point.starts_with("/run/media/"));
            if type_excluded || removable || network || under_media {
                excluded.push(m);
            }
        }

        RootSet {
            roots: Arc::new(RwLock::new(roots)),
            excluded,
            exclude_dirs: Arc::new(RwLock::new(config.effective_exclude_dirs())),
        }
    }

    /// True if `path` is inside any excluded mount point or excluded directory.
    pub fn is_excluded(&self, path: &Path) -> bool {
        // Excluded mounts are usually top-level (/proc, /sys, /media/...), but
        // a removable mount inside a root (e.g. ~/usb) must also be skipped.
        if self.excluded.iter().any(|m| path.starts_with(&m.point)) {
            return true;
        }
        if let Ok(dirs) = self.exclude_dirs.read()
            && dirs.iter().any(|d| path.starts_with(d))
        {
            return true;
        }
        false
    }

    /// Replace the directory-exclusion list (takes effect on the next walk).
    pub fn set_exclude_dirs(&self, dirs: Vec<PathBuf>) {
        if let Ok(mut guard) = self.exclude_dirs.write() {
            *guard = dirs;
        }
    }

    /// The current directory-exclusion list.
    pub fn exclude_dirs(&self) -> Vec<PathBuf> {
        self.exclude_dirs
            .read()
            .map(|d| d.clone())
            .unwrap_or_default()
    }

    /// True if `path` is inside one of the search roots.
    pub fn is_in_roots(&self, path: &Path) -> bool {
        self.roots
            .read()
            .map(|r| r.iter().any(|root| path.starts_with(root)))
            .unwrap_or(false)
    }

    /// The current search roots (canonicalized).
    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.read().map(|r| r.clone()).unwrap_or_default()
    }

    /// Replace the search roots (takes effect on the next walk).
    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        if let Ok(mut guard) = self.roots.write() {
            *guard = roots;
        }
    }
}

fn is_network_fstype(fstype: &str) -> bool {
    matches!(
        fstype,
        "nfs"
            | "nfs4"
            | "smbfs"
            | "cifs"
            | "sshfs"
            | "fuse.sshfs"
            | "fuse.rclone"
            | "fuse.s3fs"
            | "fuse.smb"
            | "9p"
    )
}

/// True if the device (e.g. `/dev/sdb1`) is on removable hardware.
fn is_removable(device: &str) -> bool {
    // device like /dev/sdb1 -> /sys/block/sdb/removable
    let dev = match device.strip_prefix("/dev/") {
        Some(d) => d,
        None => return false,
    };
    let base: String = dev
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if base.is_empty() {
        return false;
    }
    let flag = PathBuf::from("/sys/block").join(&base).join("removable");
    match std::fs::read_to_string(flag) {
        Ok(s) => s.trim() == "1",
        Err(_) => false,
    }
}

/// Parse `/proc/self/mounts`. Lines: `device point fstype options dump pass`,
/// with octal escapes in device/point (`\040` = space, `\011` = tab, `\134` = \).
pub fn read_mounts() -> Vec<Mount> {
    read_mounts_from("/proc/self/mounts")
}

fn read_mounts_from(path: &str) -> Vec<Mount> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut mounts = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(device), Some(point), Some(fstype)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        mounts.push(Mount {
            point: PathBuf::from(unescape(point)),
            device: unescape(device),
            fstype: fstype.to_string(),
        });
    }
    mounts
}

/// Decode octal escapes used in /proc/self/mounts.
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let b0 = bytes[i + 1];
            let b1 = bytes[i + 2];
            let b2 = bytes[i + 3];
            if b0.is_ascii_digit() && b1.is_ascii_digit() && b2.is_ascii_digit() {
                let v = (b0 - b'0') * 64 + (b1 - b'0') * 8 + (b2 - b'0');
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mounts_with_escapes() {
        let text = "/dev/sda2 / ext4 rw 0 0\n\
                    proc /proc proc rw 0 0\n\
                    /dev/sdb1 /media/ahmad/USB\\040Drive vfat rw 0 0\n\
                    server:/home /mnt/nfs nfs4 rw 0 0\n";
        std::fs::write("/tmp/everything_mounts_test", text).unwrap();
        let mounts = read_mounts_from("/tmp/everything_mounts_test");
        std::fs::remove_file("/tmp/everything_mounts_test").ok();
        assert_eq!(mounts.len(), 4);
        assert_eq!(mounts[2].point, PathBuf::from("/media/ahmad/USB Drive"));
        assert_eq!(mounts[2].fstype, "vfat");
        assert_eq!(mounts[3].fstype, "nfs4");
    }

    #[test]
    fn exclusion_set_marks_known_mounts() {
        let set = RootSet {
            roots: Arc::new(RwLock::new(vec![PathBuf::from("/home/ahmad")])),
            excluded: vec![
                Mount {
                    point: PathBuf::from("/proc"),
                    device: "proc".into(),
                    fstype: "proc".into(),
                },
                Mount {
                    point: PathBuf::from("/home/ahmad/usb"),
                    device: "/dev/sdb1".into(),
                    fstype: "vfat".into(),
                },
            ],
            exclude_dirs: Arc::new(RwLock::new(vec![PathBuf::from("/home/ahmad/scratch")])),
        };
        assert!(set.is_excluded(Path::new("/proc/123/fd")));
        assert!(set.is_excluded(Path::new("/home/ahmad/usb/photos")));
        assert!(set.is_excluded(Path::new("/home/ahmad/scratch/tmp.bin")));
        assert!(!set.is_excluded(Path::new("/home/ahmad/Documents")));
        assert!(set.is_in_roots(Path::new("/home/ahmad/x.txt")));
        assert!(!set.is_in_roots(Path::new("/opt/x.txt")));
    }

    #[test]
    fn excluded_dirs_can_be_replaced_live() {
        let set = RootSet {
            roots: Arc::new(RwLock::new(vec![PathBuf::from("/home/ahmad")])),
            ..RootSet::default()
        };
        let scratch = Path::new("/home/ahmad/scratch/x");
        assert!(!set.is_excluded(scratch));
        set.set_exclude_dirs(vec![PathBuf::from("/home/ahmad/scratch")]);
        assert!(set.is_excluded(scratch));
        assert_eq!(set.exclude_dirs().len(), 1);
    }

    #[test]
    fn roots_can_be_replaced_live() {
        let set = RootSet::default();
        assert!(set.roots().is_empty());
        assert!(!set.is_in_roots(Path::new("/srv/data/x")));
        set.set_roots(vec![PathBuf::from("/srv/data")]);
        assert!(set.is_in_roots(Path::new("/srv/data/x")));
        assert_eq!(set.roots(), [PathBuf::from("/srv/data")]);
    }
}
