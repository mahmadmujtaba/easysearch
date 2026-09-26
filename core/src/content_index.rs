//! Optional background content index (default off).
//!
//! When enabled, a worker thread extracts text from newly indexed / changed
//! files and caches it in RAM (bounded, insertion-order LRU). Content queries
//! use the cache when available and fall back to live reads otherwise, so
//! freshness is never compromised — the cache only accelerates.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc, mpsc::Receiver};
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
/// - `.docx`: in-process OOXML extraction (see [`extract_docx_text`]) — no
///   external `docx2txt` needed.
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

/// XML parts inside a `.docx` that carry visible text.
const DOCX_STATIC_PARTS: &[&str] = &[
    "word/document.xml",
    "word/footnotes.xml",
    "word/endnotes.xml",
    "word/comments.xml",
];

/// Extract text from a `.docx`, in-process.
///
/// A `.docx` is an OOXML package (a ZIP). The readable payload lives in
/// `word/*.xml` as WordprocessingML, so this opens the archive, walks those XML
/// parts, keeps the character data of `<w:t>` runs (and deleted `<w:delText>`),
/// and turns `<w:p>`/`<w:br>` into newlines, `<w:tab>` into a tab and a table
/// cell (`<w:tc>`) into a tab. Headers, footers and comments are included.
fn extract_docx_text(path: &Path, max_bytes: u64) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    // Document first, then the optional parts, then anything else that looks
    // like a text-bearing Word part (headers/footers are numbered).
    let mut parts: Vec<String> = DOCX_STATIC_PARTS.iter().map(|s| s.to_string()).collect();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let name = entry.name().to_string();
        if is_docx_text_part(&name) && !parts.iter().any(|p| p == &name) {
            parts.push(name);
        }
    }

    let mut out = String::new();
    for part in parts {
        if out.len() as u64 > max_bytes {
            break;
        }
        let Ok(mut entry) = archive.by_name(&part) else {
            continue; // optional part that is not present
        };
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_err() {
            continue;
        }
        append_wordml_text(&xml, &mut out, max_bytes);
    }

    if out.len() as u64 > max_bytes || out.is_empty() {
        return None;
    }
    Some(out)
}

/// True for the ZIP entries that can contain readable Word text.
fn is_docx_text_part(name: &str) -> bool {
    let Some(base) = name
        .strip_prefix("word/")
        .and_then(|n| n.strip_suffix(".xml"))
    else {
        return false;
    };
    matches!(base, "document" | "footnotes" | "endnotes" | "comments")
        || base.starts_with("header")
        || base.starts_with("footer")
}

/// Append the visible text of one WordprocessingML part to `out`.
fn append_wordml_text(xml: &str, out: &mut String, max_bytes: u64) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut in_run_text = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"t" | b"delText" => in_run_text = true,
                b"tab" => out.push('\t'),
                _ => {}
            },
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"t" | b"delText" => in_run_text = false,
                b"p" | b"tr" => out.push('\n'),
                b"tc" => out.push('\t'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"br" | b"cr" => out.push('\n'),
                b"tab" => out.push('\t'),
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_run_text && let Ok(text) = t.decode() {
                    out.push_str(&text);
                }
            }
            // quick-xml reports entity references as their own event, so resolve
            // them to the characters the text would contain.
            Ok(Event::GeneralRef(r)) => {
                if in_run_text {
                    match r.resolve_char_ref() {
                        Ok(Some(c)) => out.push(c),
                        _ => {
                            let name = r.decode().map(|n| n.into_owned()).unwrap_or_default();
                            match name.as_str() {
                                "amp" => out.push('&'),
                                "lt" => out.push('<'),
                                "gt" => out.push('>'),
                                "quot" => out.push('"'),
                                "apos" => out.push('\''),
                                // An unknown entity becomes a space, so words on
                                // either side stay searchable and separated.
                                _ => out.push(' '),
                            }
                        }
                    }
                }
            }
            Ok(Event::CData(t)) => {
                if in_run_text {
                    out.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
        if out.len() as u64 > max_bytes {
            break;
        }
    }
}

/// Element local name, dropping any namespace prefix (`w:t` → `t`).
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A unique scratch directory, removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("evfl-docx-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Build a `.docx` (a ZIP) with the given `word/*.xml` parts.
    fn fake_docx(scratch: &Scratch, parts: &[(&str, &str)]) -> PathBuf {
        let path = scratch.path().join("doc.docx");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, xml) in parts {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn extracts_wordprocessingml_text() {
        // Two paragraphs, a tab, an intra-paragraph break, an escaped ampersand,
        // a table cell and a header part.
        let document = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Quarterly</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>Report</w:t></w:r></w:p>
    <w:p><w:r><w:t>R&amp;D budget</w:t><w:br/><w:t>approved</w:t></w:r></w:p>
    <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell A</w:t></w:r></w:p></w:tc>
      <w:tc><w:p><w:r><w:t>Cell B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
  </w:body>
</w:document>"#;
        let header = r#"<?xml version="1.0"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>CONFIDENTIAL</w:t></w:r></w:p></w:hdr>"#;

        let scratch = Scratch::new("text");
        let path = fake_docx(
            &scratch,
            &[
                ("word/document.xml", document),
                ("word/header1.xml", header),
                (
                    "word/styles.xml",
                    "<w:styles><w:t>SHOULD NOT APPEAR</w:t></w:styles>",
                ),
            ],
        );

        let text = extract_text(&path, 1024 * 1024).expect("docx text");
        assert!(text.contains("Quarterly"), "{text:?}");
        assert!(text.contains("Report"), "{text:?}");
        // Exactly how whitespace falls is irrelevant for search, but words must
        // not be glued together: a tab, a break and a cell all separate them.
        assert!(
            !text.contains("QuarterlyReport"),
            "a tab must separate words: {text:?}"
        );
        assert!(
            text.contains("R&D budget"),
            "entities must be resolved: {text:?}"
        );
        assert!(
            !text.contains("budgetapproved"),
            "a break must separate words: {text:?}"
        );
        assert!(text.contains("Cell A"), "{text:?}");
        assert!(text.contains("Cell B"), "{text:?}");
        assert!(
            !text.contains("Cell ACell B"),
            "cells must be separated: {text:?}"
        );
        // The header is included…
        assert!(text.contains("CONFIDENTIAL"), "{text:?}");
        // …but unrelated parts are not.
        assert!(!text.contains("SHOULD NOT APPEAR"), "{text:?}");
    }

    #[test]
    fn docx_extraction_respects_the_size_cap() {
        let big = format!(
            r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>"#,
            "a".repeat(4096)
        );
        let scratch = Scratch::new("cap");
        let path = fake_docx(&scratch, &[("word/document.xml", &big)]);
        assert!(extract_text(&path, 1024 * 1024).is_some());
        // A cap smaller than the text means the file is skipped, not truncated.
        assert!(extract_text(&path, 64).is_none());
    }

    #[test]
    fn a_non_zip_docx_is_skipped_not_a_panic() {
        let scratch = Scratch::new("broken");
        let path = scratch.path().join("broken.docx");
        std::fs::write(&path, b"this is not a zip archive").unwrap();
        assert!(extract_text(&path, 1024 * 1024).is_none());
    }
}
