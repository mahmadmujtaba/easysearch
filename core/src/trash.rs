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

/// One entry in the user's trash, as reported by [`list_trashed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedItem {
    /// The name inside the trash: `<name>` in `files/` and `<name>.trashinfo`.
    pub name: String,
    /// Where the file lived before it was trashed (decoded from `Path=`).
    pub original_path: PathBuf,
    /// The recorded `DeletionDate`, verbatim (empty if it was missing).
    pub deletion_date: String,
}

/// List the user's trash, newest first.
///
/// Entries whose payload has already vanished are skipped, so everything
/// returned here can actually be [restored](restore). An unreadable or absent
/// `info/` directory is not an error — it just means an empty trash.
pub fn list_trashed() -> Vec<TrashedItem> {
    list_trashed_in(&trash_dir())
}

/// [`list_trashed`] against an explicit trash root — the seam the tests use.
fn list_trashed_in(trash: &Path) -> Vec<TrashedItem> {
    let info = trash.join("info");
    let files = trash.join("files");
    let Ok(entries) = std::fs::read_dir(&info) else {
        return Vec::new();
    };
    let mut items: Vec<TrashedItem> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".trashinfo"))
        else {
            continue;
        };
        // Only entries whose payload is still present can be restored.
        if std::fs::symlink_metadata(files.join(name)).is_err() {
            continue;
        }
        let Ok(record) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(original_path) = parse_original_path(&record) else {
            continue;
        };
        items.push(TrashedItem {
            name: name.to_string(),
            original_path,
            deletion_date: field(&record, "DeletionDate").unwrap_or_default(),
        });
    }
    // Newest first. The dates are ISO-8601-ish, so a plain string sort is
    // chronological; the name breaks ties deterministically.
    items.sort_by(|a, b| {
        b.deletion_date
            .cmp(&a.deletion_date)
            .then_with(|| b.name.cmp(&a.name))
    });
    items
}

/// Restore the trashed entry named `name` to where it came from, returning the
/// path it now lives at.
///
/// The original directory is recreated if it no longer exists, and a name that
/// is already taken gets a numeric suffix rather than being overwritten. Only
/// the trash entry named exactly `name` is touched.
pub fn restore(name: &str) -> Result<PathBuf, String> {
    restore_in(&trash_dir(), name)
}

/// [`restore`] against an explicit trash root — the seam the tests use.
fn restore_in(trash: &Path, name: &str) -> Result<PathBuf, String> {
    // `name` names a single entry: never let it escape `files/`.
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(format!("{name}: not a trash entry"));
    }
    let info_file = trash.join("info").join(format!("{name}.trashinfo"));
    let payload = trash.join("files").join(name);
    let record =
        std::fs::read_to_string(&info_file).map_err(|e| format!("{}: {e}", info_file.display()))?;
    let original = parse_original_path(&record)
        .ok_or_else(|| format!("{}: no Path= entry", info_file.display()))?;

    // The original folder may be gone (removed, or on a now-unmounted volume).
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let target = free_destination(&original);

    match std::fs::rename(&payload, &target) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Trash on a different filesystem: copy back, then drop the copy.
            std::fs::copy(&payload, &target).map_err(|e| format!("{}: {e}", payload.display()))?;
            let _ = std::fs::remove_file(&payload);
        }
        Err(e) => return Err(format!("{}: {e}", payload.display())),
    }
    let _ = std::fs::remove_file(&info_file);
    Ok(target)
}

/// A destination for a restore: the original path, or `stem.2.ext` / `stem.2`
/// beside it when that is already taken.
fn free_destination(original: &Path) -> PathBuf {
    if !original.exists() {
        return original.to_path_buf();
    }
    let parent = original.parent().unwrap_or_else(|| Path::new(""));
    let stem = original
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = original
        .extension()
        .map(|s| s.to_string_lossy().into_owned());
    let mut n = 1;
    loop {
        n += 1;
        let candidate = match &ext {
            Some(e) => parent.join(format!("{stem}.{n}.{e}")),
            None => parent.join(format!("{stem}.{n}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
}

/// The original absolute path from a `.trashinfo` record's `Path=` field.
fn parse_original_path(record: &str) -> Option<PathBuf> {
    field(record, "Path").map(|encoded| decode_path(&encoded))
}

/// The value of `Key=` in a `.trashinfo` record, if present.
fn field(record: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    record
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
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

/// The inverse of [`encode_path`]: turn a `%XX` sequence back into a byte. Plain
/// bytes (including `/`) pass through, and a malformed `%` is kept literally.
fn decode_path(encoded: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let bytes = encoded.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

    /// Write a `files/` payload plus its `info/` record by hand so a test can
    /// pin the deletion date and assert on ordering.
    fn put_trashed(s: &Scratch, name: &str, original: &Path, date: &str) {
        std::fs::create_dir_all(s.trash().join("files")).unwrap();
        std::fs::create_dir_all(s.trash().join("info")).unwrap();
        std::fs::write(s.trash().join("files").join(name), b"payload").unwrap();
        let record = format!(
            "[Trash Info]\nPath={}\nDeletionDate={date}\n",
            encode_path(original)
        );
        std::fs::write(
            s.trash().join("info").join(format!("{name}.trashinfo")),
            record,
        )
        .unwrap();
    }

    #[test]
    fn a_trashed_file_is_listed_with_its_original_path() {
        let s = Scratch::new("list");
        let src = s.file("docs/a name.txt", b"hello");
        let absolute = src.canonicalize().unwrap();
        move_to_trash_in(&s.trash(), &src).unwrap();

        let items = list_trashed_in(&s.trash());
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].name, "a name.txt");
        assert_eq!(items[0].original_path, absolute, "the space is decoded");
        assert_eq!(items[0].deletion_date.len(), 19);
    }

    #[test]
    fn listing_is_newest_first_and_skips_orphans() {
        let s = Scratch::new("order");
        put_trashed(
            &s,
            "old.txt",
            Path::new("/tmp/old.txt"),
            "2020-01-01T00:00:00",
        );
        put_trashed(
            &s,
            "new.txt",
            Path::new("/tmp/new.txt"),
            "2024-06-01T00:00:00",
        );
        // An info record with no payload is not restorable, so it is skipped.
        std::fs::write(
            s.trash().join("info/orphan.txt.trashinfo"),
            "[Trash Info]\nPath=/tmp/orphan.txt\nDeletionDate=2026-01-01T00:00:00\n",
        )
        .unwrap();

        let names: Vec<String> = list_trashed_in(&s.trash())
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, ["new.txt", "old.txt"]);
    }

    #[test]
    fn an_absent_trash_is_empty_not_an_error() {
        let s = Scratch::new("empty");
        assert!(list_trashed_in(&s.trash()).is_empty());
    }

    #[test]
    fn restore_puts_the_file_back_and_clears_the_record() {
        let s = Scratch::new("restore");
        let src = s.file("docs/report.txt", b"hello");
        let absolute = src.canonicalize().unwrap();
        move_to_trash_in(&s.trash(), &src).unwrap();
        assert!(!src.exists());

        let back = restore_in(&s.trash(), "report.txt").unwrap();
        assert_eq!(back, absolute);
        assert_eq!(std::fs::read(&back).unwrap(), b"hello");
        assert!(
            !s.trash().join("files/report.txt").exists(),
            "payload moved out"
        );
        assert!(!s.trash().join("info/report.txt.trashinfo").exists());
        assert!(list_trashed_in(&s.trash()).is_empty());
    }

    #[test]
    fn restore_recreates_the_original_folder() {
        let s = Scratch::new("restore-mkdir");
        let src = s.file("gone/report.txt", b"hello");
        let absolute = src.canonicalize().unwrap();
        move_to_trash_in(&s.trash(), &src).unwrap();
        std::fs::remove_dir_all(s.0.join("gone")).unwrap();

        let back = restore_in(&s.trash(), "report.txt").unwrap();
        assert_eq!(back, absolute);
        assert!(back.exists());
    }

    #[test]
    fn restore_does_not_clobber_a_file_that_reappeared() {
        let s = Scratch::new("restore-clobber");
        let src = s.file("docs/report.txt", b"original");
        move_to_trash_in(&s.trash(), &src).unwrap();
        // Something takes the name back while the file sits in the trash.
        std::fs::write(&src, b"interloper").unwrap();

        let back = restore_in(&s.trash(), "report.txt").unwrap();
        assert_eq!(back, s.0.join("docs/report.2.txt"));
        assert_eq!(std::fs::read(&src).unwrap(), b"interloper", "kept intact");
        assert_eq!(std::fs::read(&back).unwrap(), b"original");
    }

    #[test]
    fn restore_rejects_a_name_that_escapes_the_trash() {
        let s = Scratch::new("restore-escape");
        for bad in ["", ".", "..", "../secret", "a/b"] {
            assert!(restore_in(&s.trash(), bad).is_err(), "{bad:?} is refused");
        }
    }

    #[test]
    fn restore_of_an_unknown_name_is_an_error() {
        let s = Scratch::new("restore-missing");
        let err = restore_in(&s.trash(), "nope.txt").unwrap_err();
        assert!(err.contains("nope.txt"), "{err}");
    }

    #[test]
    fn decode_path_inverts_encode_path() {
        for p in ["/a b/c", "/a%b", "/caf\u{e9}", "/keep-_.~/x"] {
            let decoded = decode_path(&encode_path(Path::new(p)));
            assert_eq!(decoded, Path::new(p), "{p:?}");
        }
        // A stray, malformed `%` is preserved rather than dropped.
        assert_eq!(decode_path("/a%zz"), Path::new("/a%zz"));
    }
}
