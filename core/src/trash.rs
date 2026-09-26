//! Move files to the freedesktop **trash** — recoverable, never permanent.
//!
//! This follows the [Trash specification][spec]: a file is moved into
//! `$XDG_DATA_HOME/Trash/files/` and a matching `…/info/<name>.trashinfo`
//! records where it came from, so the desktop's file manager (and any other
//! tool) can list and restore it. Nothing here deletes data: the original is
//! only removed once the copy inside the trash exists, and a name collision
//! gets a numeric suffix rather than overwriting — so two files with the same
//! name from different directories can both be trashed safely.
//!
//! Only regular files are accepted (the duplicate finder trashes files); a
//! directory is refused rather than half-moved.
//!
//! [spec]: https://specifications.freedesktop.org/trash-spec/latest/

use std::path::{Path, PathBuf};

/// The trash root: `$XDG_DATA_HOME/Trash`, i.e. `~/.local/share/Trash`.
pub fn trash_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".local").join("share"))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    base.join("Trash")
}

/// Move `path` to the user's trash. Returns the path it now lives at inside the
/// trash (under `…/Trash/files/`).
pub fn move_to_trash(path: &Path) -> Result<PathBuf, String> {
    move_to_trash_in(&trash_dir(), path)
}

/// [`move_to_trash`] against an explicit trash root — the seam the tests use, so
/// they never touch the real `~/.local/share/Trash`.
fn move_to_trash_in(trash: &Path, path: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!(
            "{}: only files can be moved to the trash",
            path.display()
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: no file name", path.display()))?
        .to_string_lossy()
        .into_owned();
    // The `.trashinfo` records the absolute original path.
    let absolute = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let files = trash.join("files");
    let info = trash.join("info");
    std::fs::create_dir_all(&files).map_err(|e| format!("{}: {e}", files.display()))?;
    std::fs::create_dir_all(&info).map_err(|e| format!("{}: {e}", info.display()))?;

    // A free name: the file name, else `name.2`, `name.3`, … (checked against
    // both the payload and its info file so two trashes can never collide).
    let mut candidate = name.clone();
    let mut n = 1;
    loop {
        let dest = files.join(&candidate);
        let info_file = info.join(format!("{candidate}.trashinfo"));
        if !dest.exists() && !info_file.exists() {
            break;
        }
        n += 1;
        candidate = format!("{name}.{n}");
    }
    let dest = files.join(&candidate);
    let info_file = info.join(format!("{candidate}.trashinfo"));

    let record = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        encode_path(&absolute),
        now_local()
    );
    // Write the record first: if the move then fails, the only thing left to
    // undo is this file, and no payload is orphaned.
    if let Err(e) = std::fs::write(&info_file, record) {
        return Err(format!("{}: {e}", info_file.display()));
    }

    match std::fs::rename(path, &dest) {
        Ok(()) => Ok(dest),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Trash on a different filesystem: copy, then remove the original.
            match std::fs::copy(path, &dest).and_then(|_| std::fs::remove_file(path)) {
                Ok(()) => Ok(dest),
                Err(e) => {
                    let _ = std::fs::remove_file(&dest);
                    let _ = std::fs::remove_file(&info_file);
                    Err(format!("{}: {e}", path.display()))
                }
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&info_file);
            Err(format!("{}: {e}", path.display()))
        }
    }
}

/// Percent-encode a path the way the trash spec wants it in the `Path=` field.
/// `/` and the URL unreserved set are kept literal; everything else (spaces,
/// `%`, newlines, non-ASCII) becomes `%XX`.
fn encode_path(path: &Path) -> String {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0xf));
        }
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    char::from_digit(nibble as u32, 16)
        .unwrap_or('0')
        .to_ascii_uppercase()
}

/// Local time as `YYYY-MM-DDTHH:MM:SS` (the spec's `DeletionDate`).
fn now_local() -> String {
    // SAFETY: `localtime_r` initialises the caller-owned `tm`; we only read it.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return String::new();
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir()
                .join(format!("easysearch-trash-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        /// A trash root inside the scratch dir.
        fn trash(&self) -> PathBuf {
            self.0.join("Trash")
        }
        fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn moves_a_file_and_writes_a_trashinfo_record() {
        let s = Scratch::new("basic");
        let src = s.file("docs/report.txt", b"hello");
        let expected = encode_path(&src.canonicalize().unwrap());
        let dest = move_to_trash_in(&s.trash(), &src).unwrap();

        assert!(!src.exists(), "the original is gone");
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
        assert_eq!(dest, s.trash().join("files").join("report.txt"));

        let record = std::fs::read_to_string(s.trash().join("info/report.txt.trashinfo")).unwrap();
        assert!(record.starts_with("[Trash Info]\n"), "{record:?}");
        let path_line = record
            .lines()
            .find(|l| l.starts_with("Path="))
            .expect("a Path= line");
        let encoded = path_line.trim_start_matches("Path=");
        assert!(!encoded.contains(' '), "spaces are encoded: {encoded}");
        // The original absolute path is what was recorded (encoded).
        assert_eq!(encoded, expected);
        assert!(record.contains("\nDeletionDate="), "{record:?}");
    }

    #[test]
    fn a_name_collision_gets_a_suffix_and_keeps_both() {
        let s = Scratch::new("collision");
        let a = s.file("one/dup.bin", b"A");
        let b = s.file("two/dup.bin", b"B");
        let da = move_to_trash_in(&s.trash(), &a).unwrap();
        let db = move_to_trash_in(&s.trash(), &b).unwrap();
        assert_ne!(da, db, "the second does not overwrite the first");
        assert_eq!(std::fs::read(&da).unwrap(), b"A");
        assert_eq!(std::fs::read(&db).unwrap(), b"B");
        assert!(db.ends_with("dup.bin.2"), "{db:?}");
        assert!(s.trash().join("info/dup.bin.2.trashinfo").is_file());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let s = Scratch::new("missing");
        let err = move_to_trash_in(&s.trash(), &s.0.join("nope.txt")).unwrap_err();
        assert!(err.contains("nope.txt"), "{err}");
    }

    #[test]
    fn a_directory_is_refused() {
        let s = Scratch::new("dir");
        let dir = s.0.join("folder");
        std::fs::create_dir_all(&dir).unwrap();
        let err = move_to_trash_in(&s.trash(), &dir).unwrap_err();
        assert!(err.contains("only files"), "{err}");
        assert!(dir.exists(), "the directory is left alone");
    }

    #[test]
    fn percent_encoding_covers_spaces_and_non_ascii() {
        assert_eq!(encode_path(Path::new("/a b/c")), "/a%20b/c");
        assert_eq!(encode_path(Path::new("/a%b")), "/a%25b");
        assert_eq!(encode_path(Path::new("/caf\u{e9}")), "/caf%C3%A9");
        assert_eq!(encode_path(Path::new("/keep-_.~/x")), "/keep-_.~/x");
    }

    #[test]
    fn the_deletion_date_looks_like_the_spec() {
        let stamp = now_local();
        assert_eq!(stamp.len(), 19, "{stamp:?}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert!(stamp.starts_with("20"), "{stamp:?}");
    }
}
