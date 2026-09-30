//! Reading archives (zip and plain tar) without extracting them to disk.
//!
//! Used by the content search — to look inside a member without unpacking the
//! archive — and by the preview, to list what an archive holds. Only **zip** and
//! uncompressed **tar** are handled: a compressed tar (`.tar.gz`), `.7z`, `.rar`
//! and the rest need a decompressor this crate does not carry, and are reported
//! as unreadable rather than guessed at.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// One member of an archive, for a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    /// Directories are listed too, but never searched.
    pub is_dir: bool,
}

/// The largest number of members a listing will return.
const LIST_LIMIT: usize = 5000;

/// Any file whose *name* says it is an archive (broader than what we can read).
pub fn is_archive_name(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some(
            "zip"
                | "tar"
                | "gz"
                | "tgz"
                | "xz"
                | "bz2"
                | "7z"
                | "rar"
                | "zst"
                | "deb"
                | "rpm"
                | "jar"
                | "war"
                | "apk"
                | "cbz"
        )
    )
}

/// An archive whose contents we can actually read (`zip`, `jar`, `cbz`, `tar`).
pub fn is_readable_archive(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("zip" | "jar" | "war" | "apk" | "cbz" | "tar")
    )
}

/// The members of `path`, capped at [`LIST_LIMIT`]. Empty when the file cannot
/// be opened or is not a readable archive.
pub fn list(path: &Path) -> Vec<ArchiveEntry> {
    if !is_readable_archive(path) {
        return Vec::new();
    }
    let entries = if is_tar(path) {
        read_tar(path, u64::MAX, u64::MAX)
            .map(|members| {
                members
                    .into_iter()
                    .map(|m| ArchiveEntry {
                        name: m.name,
                        size: m.size,
                        is_dir: m.is_dir,
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        list_zip(path).unwrap_or_default()
    };
    let mut entries = entries;
    entries.truncate(LIST_LIMIT);
    entries
}

/// The text-bearing members of `path`, for a content search: `(name, bytes)`
/// pairs, skipping directories and members over `max_member`, stopping once
/// `max_total` bytes have been read.
pub fn read_members(path: &Path, max_total: u64, max_member: u64) -> Vec<(String, Vec<u8>)> {
    if !is_readable_archive(path) {
        return Vec::new();
    }
    if is_tar(path) {
        return read_tar(path, max_total, max_member)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.data.map(|d| (m.name, d)))
            .collect();
    }
    read_members_zip(path, max_total, max_member).unwrap_or_default()
}

fn is_tar(path: &Path) -> bool {
    ext_lower(path).as_deref() == Some("tar")
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

// --- zip ------------------------------------------------------------------

fn list_zip(path: &Path) -> Option<Vec<ArchiveEntry>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        out.push(ArchiveEntry {
            name: entry.name().to_string(),
            size: entry.size(),
            is_dir: entry.is_dir(),
        });
    }
    Some(out)
}

fn read_members_zip(
    path: &Path,
    max_total: u64,
    max_member: u64,
) -> Option<Vec<(String, Vec<u8>)>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut out = Vec::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        if total >= max_total {
            break;
        }
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() || entry.size() > max_member {
            continue;
        }
        let name = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size().min(1 << 20) as usize);
        if entry.take(max_member).read_to_end(&mut buf).is_err() {
            continue;
        }
        total += buf.len() as u64;
        out.push((name, buf));
    }
    Some(out)
}

// --- tar ------------------------------------------------------------------

struct TarMember {
    name: String,
    size: u64,
    is_dir: bool,
    data: Option<Vec<u8>>,
}

/// Read a tar's members. Data is kept only for regular files within the
/// budgets; everything else is skipped by seeking, so a huge member costs only
/// the header read.
fn read_tar(path: &Path, max_total: u64, max_member: u64) -> Option<Vec<TarMember>> {
    let mut r = BufReader::new(std::fs::File::open(path).ok()?);
    let mut header = [0u8; 512];
    let mut members = Vec::new();
    let mut total = 0u64;
    let mut long_name: Option<String> = None;
    loop {
        if r.read_exact(&mut header).is_err() {
            break;
        }
        // Two zero blocks mark the end (one is treated as the end too).
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let size = parse_octal(&header[124..136]);
        let typeflag = header[156];
        let blocks = size.div_ceil(512) * 512;
        match typeflag {
            // GNU long name: the name is the entry's data.
            b'L' => {
                let mut name = vec![0u8; size as usize];
                if r.read_exact(&mut name).is_err() {
                    break;
                }
                let _ = r.seek(SeekFrom::Current((blocks - size) as i64));
                long_name = Some(cstr(&name));
                continue;
            }
            // pax headers and other metadata: skip their data.
            b'x' | b'g' => {
                let _ = r.seek(SeekFrom::Current(blocks as i64));
                continue;
            }
            _ => {}
        }
        let name = long_name.take().unwrap_or_else(|| cstr(&header[0..100]));
        let is_dir = typeflag == b'5' || name.ends_with('/');
        let is_file = matches!(typeflag, 0 | b'0' | b'7');
        let keep = is_file && !is_dir && size <= max_member && total < max_total;
        let data = if keep {
            let mut buf = vec![0u8; size as usize];
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            let _ = r.seek(SeekFrom::Current((blocks - size) as i64));
            total += size;
            Some(buf)
        } else {
            let _ = r.seek(SeekFrom::Current(blocks as i64));
            None
        };
        members.push(TarMember {
            name,
            size,
            is_dir,
            data,
        });
        if members.len() >= 20_000 {
            break;
        }
    }
    Some(members)
}

/// A NUL-terminated (or fixed-width) field as a lossy string.
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// An octal field (tar's numeric encoding). All-zero is 0.
fn parse_octal(bytes: &[u8]) -> u64 {
    let text = cstr(bytes);
    u64::from_str_radix(text.trim(), 8).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("easysearch-archive-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a minimal ustar archive by hand (no tar crate).
    fn write_tar(path: &Path, files: &[(&str, &[u8])]) {
        let mut out: Vec<u8> = Vec::new();
        for (name, data) in files {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}", data.len());
            header[124..124 + size.len()].copy_from_slice(size.as_bytes());
            header[156] = b'0';
            // Checksum: spaces while summing, then the octal result.
            for b in &mut header[148..156] {
                *b = b' ';
            }
            let sum: u32 = header.iter().map(|&b| b as u32).sum();
            let chk = format!("{:06o}\0 ", sum);
            header[148..148 + chk.len()].copy_from_slice(chk.as_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        out.extend(std::iter::repeat_n(0u8, 1024));
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(&out).unwrap();
    }

    #[test]
    fn a_zip_lists_and_reads_its_members() {
        let dir = scratch("zip");
        let path = dir.join("a.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("hello.txt", opts).unwrap();
        zip.write_all(b"first").unwrap();
        zip.start_file("nested/notes.md", opts).unwrap();
        zip.write_all(b"second entry").unwrap();
        zip.finish().unwrap();

        let names: Vec<String> = list(&path).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["hello.txt", "nested/notes.md"]);
        let members = read_members(&path, 1 << 20, 1 << 20);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].1, b"first");
        assert_eq!(members[1].0, "nested/notes.md");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tar_lists_and_reads_its_members() {
        let dir = scratch("tar");
        let path = dir.join("a.tar");
        write_tar(&path, &[("one.txt", b"alpha"), ("two.txt", b"beta beta")]);

        let entries = list(&path);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(entries[0].name, "one.txt");
        assert_eq!(entries[1].size, 9);
        let members = read_members(&path, 1 << 20, 1 << 20);
        assert_eq!(members[0].1, b"alpha");
        assert_eq!(members[1].0, "two.txt");

        // Budgets are honoured: a member over the cap is skipped.
        let members = read_members(&path, 1 << 20, 5);
        assert_eq!(members.len(), 1, "only the 5-byte member fits");
        assert_eq!(members[0].0, "one.txt");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_names_are_recognised() {
        assert!(is_readable_archive(Path::new("/a/x.zip")));
        assert!(is_readable_archive(Path::new("/a/x.tar")));
        assert!(
            !is_readable_archive(Path::new("/a/x.tar.gz")),
            "no decompressor"
        );
        assert!(!is_readable_archive(Path::new("/a/x.7z")));
        assert!(is_archive_name(Path::new("/a/x.tar.gz")));
        assert!(is_archive_name(Path::new("/a/x.7z")));
        assert!(!is_archive_name(Path::new("/a/x.txt")));
        // An unreadable archive lists nothing rather than erroring.
        assert!(list(Path::new("/a/x.7z")).is_empty());
    }

    #[test]
    fn octal_fields_parse() {
        assert_eq!(parse_octal(b"00000000012\0"), 10);
        assert_eq!(parse_octal(b"00000000000\0"), 0);
        assert_eq!(parse_octal(b"        "), 0);
    }
}
