//! Disk-backed base index: a compact binary file, memory-mapped for
//! zero-copy scans, plus a small hash table for O(1) membership checks.
//!
//! Memory model (see docs/scope.md §9): the file lives in the kernel page
//! cache, so *cold* pages cost no RSS and are evicted under pressure; queries
//! scan it without copying path strings. Only the ~4 MB hash table and the
//! small change overlay are always resident.
//!
//! On-disk layout (all integers little-endian):
//! ```text
//! offset 0   magic "EVLI0001" (8 bytes)
//!        8   entry_count : u64
//!       16   files       : u64
//!       24   dirs        : u64
//!       32   blob_len    : u64
//!       40   records[entry_count]  each 32 bytes:
//!              path_len  : u32   is_dir : u8   pad : [u8;3]
//!              size      : u64   mtime  : i64
//!              path_off  : u64   (offset into blob)
//!       40 + entry_count*32   blob: concatenated raw path bytes
//! ```

use crate::overlay::Meta;
use memmap2::{Mmap, MmapOptions};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const INDEX_FILE: &str = "index-v1.bin";
const MAGIC: &[u8; 8] = b"EVLI0001";
const HEADER_SIZE: usize = 40;
const RECORD_SIZE: usize = 32;

pub struct DiskIndex {
    mmap: Mmap,
    entry_count: usize,
    files: u64,
    dirs: u64,
    blob_start: usize,
    /// Path hash → record index. FNV-1a 64-bit; collisions are astronomically
    /// rare and their worst case is a benign duplicate in the overlay.
    by_hash: HashMap<u64, u32>,
}

impl DiskIndex {
    /// Map an existing index file. Validates the header.
    pub fn load(path: &Path) -> io::Result<DiskIndex> {
        let file = File::open(path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        if mmap.len() < HEADER_SIZE || &mmap[0..8] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad index header",
            ));
        }
        let entry_count = u64_at(&mmap, 8) as usize;
        let files = u64_at(&mmap, 16);
        let dirs = u64_at(&mmap, 24);
        let blob_len = u64_at(&mmap, 32) as usize;
        let blob_start = HEADER_SIZE + entry_count * RECORD_SIZE;
        if mmap.len() < blob_start + blob_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "index truncated",
            ));
        }
        let mut by_hash = HashMap::with_capacity(entry_count);
        for i in 0..entry_count {
            let bytes = path_bytes(&mmap, blob_start, i);
            by_hash.insert(path_hash(bytes), i as u32);
        }
        Ok(DiskIndex {
            mmap,
            entry_count,
            files,
            dirs,
            blob_start,
            by_hash,
        })
    }

    /// Serialize `entries` and atomically write them to `path` (temp + rename),
    /// then load and return the mapped index.
    pub fn write_and_load(path: &Path, entries: &[(PathBuf, Meta)]) -> io::Result<DiskIndex> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{}.tmp.{}", INDEX_FILE, std::process::id()));

        let mut files = 0u64;
        let mut dirs = 0u64;
        for (_, m) in entries {
            if m.is_dir {
                dirs += 1;
            } else {
                files += 1;
            }
        }

        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(MAGIC)?;
        w.write_all(&(entries.len() as u64).to_le_bytes())?;
        w.write_all(&files.to_le_bytes())?;
        w.write_all(&dirs.to_le_bytes())?;
        w.write_all(&0u64.to_le_bytes())?; // blob_len placeholder (patched below)

        let mut blob_off: u64 = 0;
        for (path, meta) in entries {
            let bytes = path.as_os_str().as_encoded_bytes();
            w.write_all(&(bytes.len() as u32).to_le_bytes())?;
            w.write_all(&[meta.is_dir as u8, 0, 0, 0])?;
            w.write_all(&meta.size.to_le_bytes())?;
            w.write_all(&(meta.mtime as u64).to_le_bytes())?;
            w.write_all(&blob_off.to_le_bytes())?;
            blob_off += bytes.len() as u64;
        }
        let blob_len = blob_off;
        for (path, _) in entries {
            w.write_all(path.as_os_str().as_encoded_bytes())?;
        }
        w.flush()?;
        drop(w);

        // Patch the blob_len header field (known only after the records).
        {
            use std::io::{Seek, SeekFrom};
            let mut f = std::fs::OpenOptions::new().write(true).open(&tmp)?;
            f.seek(SeekFrom::Start(32))?;
            f.write_all(&blob_len.to_le_bytes())?;
        }

        std::fs::rename(&tmp, path)?;
        DiskIndex::load(path)
    }

    pub fn len(&self) -> usize {
        self.entry_count
    }

    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    pub fn files(&self) -> u64 {
        self.files
    }

    pub fn dirs(&self) -> u64 {
        self.dirs
    }

    /// Metadata of `path`, if present in the base.
    pub fn meta_of(&self, path: &Path) -> Option<Meta> {
        self.record_index(path).map(|i| record_meta(&self.mmap, i))
    }

    /// True if `path` is present in the base index.
    pub fn contains(&self, path: &Path) -> bool {
        self.record_index(path).is_some()
    }

    fn record_index(&self, path: &Path) -> Option<usize> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let h = path_hash(bytes);
        match self.by_hash.get(&h) {
            Some(&i) => {
                let i = i as usize;
                (path_bytes(&self.mmap, self.blob_start, i) == bytes).then_some(i)
            }
            None => None,
        }
    }

    /// Zero-copy view of entry `i` (borrows from the mmap when valid UTF-8).
    pub fn entry(&self, i: usize) -> (std::borrow::Cow<'_, str>, Meta) {
        let meta = record_meta(&self.mmap, i);
        let bytes = path_bytes(&self.mmap, self.blob_start, i);
        (String::from_utf8_lossy(bytes), meta)
    }

    /// Scan all entries in order (zero-copy).
    pub fn for_each(&self, mut f: impl FnMut(usize, &str, Meta)) {
        for i in 0..self.entry_count {
            let (cow, meta) = self.entry(i);
            f(i, &cow, meta);
        }
    }
}

fn record_meta(mmap: &Mmap, i: usize) -> Meta {
    let r = record(mmap, i);
    Meta {
        size: u64_at(r, 8),
        mtime: u64_at(r, 16) as i64,
        is_dir: r[4] != 0,
    }
}

fn path_bytes<'a>(mmap: &'a Mmap, blob_start: usize, i: usize) -> &'a [u8] {
    let r = record(mmap, i);
    let len = u32_at(r, 0) as usize;
    let off = u64_at(r, 24) as usize;
    &mmap[blob_start + off..blob_start + off + len]
}

fn record(mmap: &Mmap, i: usize) -> &[u8] {
    let start = HEADER_SIZE + i * RECORD_SIZE;
    &mmap[start..start + RECORD_SIZE]
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// FNV-1a 64-bit — fast, deterministic, no dependencies.
pub fn path_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn meta(is_dir: bool) -> Meta {
        Meta {
            size: 7,
            mtime: 1234,
            is_dir,
        }
    }

    fn write_temp(entries: &[(PathBuf, Meta)]) -> (std::path::PathBuf, DiskIndex) {
        let dir = std::env::temp_dir().join(format!("diskidx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(INDEX_FILE);
        let idx = DiskIndex::write_and_load(&path, entries).unwrap();
        (path, idx)
    }

    #[test]
    fn roundtrip_and_membership() {
        let entries = vec![
            (PathBuf::from("/a/b.txt"), meta(false)),
            (PathBuf::from("/a/deep/note.md"), meta(false)),
            (PathBuf::from("/a"), meta(true)),
        ];
        let (_, idx) = write_temp(&entries);
        assert_eq!(idx.len(), 3);
        assert_eq!(idx.files(), 2);
        assert_eq!(idx.dirs(), 1);
        assert!(idx.contains(Path::new("/a/deep/note.md")));
        assert!(!idx.contains(Path::new("/a/deep/other.md")));
        assert_eq!(idx.meta_of(Path::new("/a/b.txt")).unwrap().size, 7);
        assert_eq!(idx.meta_of(Path::new("/a/b.txt")).unwrap().mtime, 1234);

        // Zero-copy scan yields the same paths in order.
        let mut seen: Vec<String> = Vec::new();
        idx.for_each(|_, p, _| seen.push(p.to_string()));
        assert_eq!(seen, vec!["/a/b.txt", "/a/deep/note.md", "/a"]);
    }

    #[test]
    fn non_utf8_paths_are_lossy_but_present() {
        let dir = std::env::temp_dir().join(format!("diskidx-utf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(INDEX_FILE);
        // path with an invalid UTF-8 byte
        let os = std::ffi::OsStr::from_bytes(b"/tmp/bad-\xff\xfe.txt");
        let entries = vec![(PathBuf::from(os), meta(false))];
        let idx = DiskIndex::write_and_load(&path, &entries).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(idx.contains(Path::new(os)));
        let (cow, _) = idx.entry(0);
        assert!(cow.contains("bad-"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("diskidx-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(INDEX_FILE);
        let entries = vec![(PathBuf::from("/x.txt"), meta(false))];
        let _ = DiskIndex::write_and_load(&path, &entries).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec![INDEX_FILE.to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
