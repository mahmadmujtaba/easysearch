//! Optional background content index (default off).
//!
//! When enabled, a worker thread extracts text from newly indexed / changed
//! files and caches it in RAM (bounded, insertion-order LRU). Content queries
//! use the cache when available and fall back to live reads otherwise, so
//! freshness is never compromised — the cache only accelerates.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, mpsc::Receiver, Arc, Mutex, RwLock};
use std::thread::JoinHandle;

const DOCX_EXTENSIONS: &[&str] = &["docx"];

/// Queue of paths awaiting background text extraction.
///
/// `pending` mirrors the channel length so the UI can show progress without
/// touching the receiver (which lives on the extraction thread).
pub struct ExtractQueue {
    tx: mpsc::Sender<PathBuf>,
    pub pending: AtomicUsize,
}

impl ExtractQueue {
    pub fn new() -> (ExtractQueue, Receiver<PathBuf>) {
        let (tx, rx) = mpsc::channel();
        (
            ExtractQueue {
                tx,
                pending: AtomicUsize::new(0),
            },
            rx,
        )
    }

    pub fn send(&self, path: PathBuf) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(path);
    }
}

pub struct ContentIndex {
    enabled: bool,
    pub max_file_bytes: u64,
    total_cap: u64,
    cache: RwLock<HashMap<PathBuf, String>>,
    /// Insertion order for LRU-ish eviction.
    order: Mutex<VecDeque<PathBuf>>,
    total_bytes: AtomicU64,
}

impl ContentIndex {
    pub fn new(enabled: bool, max_file_bytes: u64, total_cap: u64) -> ContentIndex {
        ContentIndex {
            enabled,
            max_file_bytes,
            total_cap,
            cache: RwLock::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            total_bytes: AtomicU64::new(0),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.cache
            .read()
            .map(|g| g.contains_key(path))
            .unwrap_or(false)
    }

    /// Run `f` over the cached text for `path`, if present.
    pub fn get_with<R>(&self, path: &Path, f: impl FnOnce(&str) -> R) -> Option<R> {
        let g = self.cache.read().ok()?;
        g.get(path).map(|t| f(t))
    }

    pub fn insert(&self, path: PathBuf, text: String) {
        if !self.enabled {
            return;
        }
        let bytes = text.len() as u64;
        let mut order = self.order.lock().unwrap();
        let mut cache = self.cache.write().unwrap();
        let mut total = self.total_bytes.load(Ordering::Relaxed);
        if let Some(prev) = cache.get(&path) {
            total = total.saturating_sub(prev.len() as u64);
        }
        // Evict oldest entries until the new text fits under the cap.
        while !order.is_empty() && total.saturating_add(bytes) > self.total_cap {
            let victim = match order.pop_front() {
                Some(v) => v,
                None => break,
            };
            if let Some(vt) = cache.remove(&victim) {
                total = total.saturating_sub(vt.len() as u64);
            }
        }
        cache.insert(path.clone(), text);
        order.push_back(path);
        self.total_bytes
            .store(total.saturating_add(bytes), Ordering::Relaxed);
    }

    /// Remove one path (and any descendants) from the cache.
    pub fn remove(&self, path: &Path) {
        let mut order = self.order.lock().unwrap();
        let mut cache = self.cache.write().unwrap();
        let mut total = self.total_bytes.load(Ordering::Relaxed);
        let doomed: Vec<PathBuf> = cache
            .keys()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();
        for d in doomed {
            if let Some(t) = cache.remove(&d) {
                total = total.saturating_sub(t.len() as u64);
            }
        }
        order.retain(|p| !p.starts_with(path));
        self.total_bytes.store(total, Ordering::Relaxed);
    }

    pub fn stats(&self) -> (usize, u64) {
        (
            self.cache.read().map(|g| g.len()).unwrap_or(0),
            self.total_bytes.load(Ordering::Relaxed),
        )
    }
}

/// Spawn the background extraction worker. Consumes `rx` until it closes.
pub fn spawn_extractor(
    cache: Arc<ContentIndex>,
    rx: Receiver<PathBuf>,
    pending: Arc<AtomicUsize>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("content-index".into())
        .spawn(move || {
            for path in rx {
                if let Some(text) = extract_text(&path, cache.max_file_bytes) {
                    cache.insert(path, text);
                }
                pending.fetch_sub(1, Ordering::Relaxed);
            }
        })
        .expect("failed to spawn content-index thread")
}

/// Extract searchable text from a file.
/// - plain text: read (capped), skip binary-looking files (NUL byte),
/// - `.docx`: via the `docx2txt` tool (skipped when unavailable).
pub fn extract_text(path: &Path, max_bytes: u64) -> Option<String> {
    let is_docx = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| DOCX_EXTENSIONS.iter().any(|d| e.eq_ignore_ascii_case(d)))
        .unwrap_or(false);
    if is_docx {
        extract_docx_text(path, max_bytes)
    } else {
        extract_plain_text(path, max_bytes)
    }
}

fn extract_plain_text(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut take = file.take(max_bytes.saturating_add(1));
    let mut buf = Vec::with_capacity(64 * 1024);
    take.read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max_bytes {
        return None; // over the size cap
    }
    if buf[..buf.len().min(8192)].contains(&0) {
        return None; // binary
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn extract_docx_text(path: &Path, max_bytes: u64) -> Option<String> {
    let out = Command::new("docx2txt").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    if out.stdout.len() as u64 > max_bytes {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
