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

/// Extensions whose text is read from a document package rather than as bytes.
pub const EXTRACT_EXTENSIONS: &[&str] = &["docx", "odt", "pdf"];

/// True if `path` is a document whose text lives inside an OOXML/ODF package
/// (a ZIP of XML) instead of being the file's own bytes.
pub fn needs_extraction(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTRACT_EXTENSIONS.iter().any(|d| e.eq_ignore_ascii_case(d)))
        .unwrap_or(false)
}

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
/// - `.docx` / `.odt`: in-process package extraction (see [`extract_package_text`]),
/// - `.pdf`: the text layer, in-process (see [`extract_pdf_text`]).
///
/// No external tool is used for any of them.
pub fn extract_text(path: &Path, max_bytes: u64) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => extract_pdf_text(path, max_bytes),
        "docx" | "odt" => extract_package_text(path, max_bytes),
        _ => extract_plain_text(path, max_bytes),
    }
}

/// Extract the **text layer** of a PDF, in-process.
///
/// A scanned, image-only PDF has no text layer and therefore yields nothing —
/// that is a property of the file, not a failure. PDFs are the least predictable
/// input this engine handles, so the parser runs under `catch_unwind`: a
/// malformed document must skip one file, never take down a query or worker.
fn extract_pdf_text(path: &Path, max_bytes: u64) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    // A text layer is far smaller than the file; refuse absurd inputs outright.
    if bytes.len() as u64 > max_bytes.saturating_mul(8) {
        return None;
    }
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(&bytes)
    }))
    .ok()?
    .ok()?;
    let text = parsed.trim();
    if text.is_empty() || text.len() as u64 > max_bytes {
        return None;
    }
    Some(text.to_string())
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

/// How to read one document-package format: which ZIP parts hold text, and
/// which XML elements are text, line ends, tabs and spaces.
struct Markup {
    /// Element names whose character data is visible text.
    containers: &'static [&'static str],
    /// Element names that end a line (on their end tag).
    line_ends: &'static [&'static str],
    /// Element names that add a tab (on their end tag).
    tab_ends: &'static [&'static str],
    /// Empty elements that add a newline.
    empty_newlines: &'static [&'static str],
    /// Empty elements that add a tab.
    empty_tabs: &'static [&'static str],
    /// Empty elements that add a space.
    empty_spaces: &'static [&'static str],
    /// ZIP parts that are always tried first.
    static_parts: &'static [&'static str],
    /// Whether an *optional* ZIP part can hold visible text.
    part_matches: fn(&str) -> bool,
}

/// WordprocessingML (`.docx`): text lives in `<w:t>` runs; paragraphs, table
/// rows and cells become whitespace. Headers/footers/notes are included.
const WORD_RULES: Markup = Markup {
    containers: &["t", "delText"],
    line_ends: &["p", "tr"],
    tab_ends: &["tc"],
    empty_newlines: &["br", "cr"],
    empty_tabs: &["tab"],
    empty_spaces: &[],
    static_parts: &[
        "word/document.xml",
        "word/footnotes.xml",
        "word/endnotes.xml",
        "word/comments.xml",
    ],
    part_matches: is_docx_text_part,
};

/// OpenDocument Text (`.odt`): the body is `content.xml`, where the text sits
/// directly inside paragraphs and headings (with inline spans/links).
const ODF_RULES: Markup = Markup {
    containers: &["p", "h", "list-item", "table-cell"],
    line_ends: &["p", "h", "list-item", "table-row"],
    tab_ends: &["table-cell"],
    empty_newlines: &["line-break"],
    empty_tabs: &["tab"],
    empty_spaces: &["s"],
    static_parts: &["content.xml"],
    part_matches: no_extra_parts,
};

fn no_extra_parts(_name: &str) -> bool {
    false
}

/// Extract the visible text of a `.docx` or `.odt`, in-process.
///
/// Both are ZIP packages of XML; only the element rules differ. The body is read
/// part by part and converted with [`append_markup_text`]. A malformed package is
/// skipped (`None`), never fatal.
fn extract_package_text(path: &Path, max_bytes: u64) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let rules = if ext == "odt" {
        &ODF_RULES
    } else {
        &WORD_RULES
    };
    extract_zip_xml_text(path, rules, max_bytes)
}

fn extract_zip_xml_text(path: &Path, rules: &Markup, max_bytes: u64) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    // The known parts first, then any optional part this format allows
    // (Word headers/footers are numbered).
    let mut parts: Vec<String> = rules.static_parts.iter().map(|s| s.to_string()).collect();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let name = entry.name().to_string();
        if (rules.part_matches)(&name) && !parts.iter().any(|p| p == &name) {
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
        append_markup_text(&xml, rules, &mut out, max_bytes);
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

/// Append the visible text of one document XML part to `out`, using `rules`.
fn append_markup_text(xml: &str, rules: &Markup, out: &mut String, max_bytes: u64) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut in_text = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if is_one_of(name, rules.containers) {
                    in_text = true;
                }
                if is_one_of(name, rules.empty_tabs) {
                    out.push('\t');
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if is_one_of(name, rules.containers) {
                    in_text = false;
                }
                if is_one_of(name, rules.line_ends) {
                    out.push('\n');
                }
                if is_one_of(name, rules.tab_ends) {
                    out.push('\t');
                }
            }
            Ok(Event::Empty(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if is_one_of(name, rules.empty_newlines) {
                    out.push('\n');
                }
                if is_one_of(name, rules.empty_tabs) {
                    out.push('\t');
                }
                if is_one_of(name, rules.empty_spaces) {
                    out.push(' ');
                }
            }
            Ok(Event::Text(t)) => {
                if in_text && let Ok(text) = t.decode() {
                    out.push_str(&text);
                }
            }
            // quick-xml reports entity references as their own event, so resolve
            // them to the characters the text would contain.
            Ok(Event::GeneralRef(r)) => {
                if in_text {
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
                if in_text {
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

fn is_one_of(name: &[u8], list: &[&str]) -> bool {
    list.iter().any(|candidate| name == candidate.as_bytes())
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

    /// Build a document package (a ZIP) with the given parts.
    fn fake_package(scratch: &Scratch, file: &str, parts: &[(&str, &str)]) -> PathBuf {
        let path = scratch.path().join(file);
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
        let path = fake_package(
            &scratch,
            "doc.docx",
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
    fn extracts_opendocument_text() {
        // Headings, inline spans, an entity, a line break, a tab, a table and a
        // `<text:s/>` space.
        let content = r#"<?xml version="1.0"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0">
  <office:body><office:text>
    <text:h text:outline-level="1">Annual Report</text:h>
    <text:p>Revenue was <text:span>up</text:span> 12&#37;.</text:p>
    <text:p>Line one<text:line-break/>line two<text:tab/>tabbed</text:p>
    <table:table><table:table-row>
      <table:table-cell><text:p>Cell A</text:p></table:table-cell>
      <table:table-cell><text:p>Cell B</text:p></table:table-cell>
    </table:table-row></table:table>
    <text:p>Words<text:s/>joined</text:p>
  </office:text></office:body>
</office:document-content>"#;
        let scratch = Scratch::new("odt");
        let path = fake_package(
            &scratch,
            "doc.odt",
            &[
                ("content.xml", content),
                ("styles.xml", "<x>NOT BODY TEXT</x>"),
            ],
        );

        let text = extract_text(&path, 1024 * 1024).expect("odt text");
        assert!(text.contains("Annual Report"), "{text:?}");
        // Inline spans stay attached, and the numeric entity is resolved.
        assert!(text.contains("Revenue was up 12%."), "{text:?}");
        // A break and a tab separate words.
        assert!(!text.contains("Line oneline two"), "{text:?}");
        assert!(text.contains("tabbed"), "{text:?}");
        // `` is a real space.
        assert!(text.contains("Words joined"), "{text:?}");
        // Table cells are separated.
        assert!(text.contains("Cell A"), "{text:?}");
        assert!(text.contains("Cell B"), "{text:?}");
        assert!(!text.contains("Cell ACell B"), "{text:?}");
        // Only the body part is read.
        assert!(!text.contains("NOT BODY TEXT"), "{text:?}");
    }

    /// Assemble a minimal, valid one-page PDF with a single text object.
    /// Written by hand (with a real xref) so the test does not depend on a
    /// fixture file.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 20 100 Td ({text}) Tj ET");
        let objects: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .into(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    #[test]
    fn extracts_a_pdf_text_layer() {
        let scratch = Scratch::new("pdf");
        let path = scratch.path().join("doc.pdf");
        std::fs::write(&path, minimal_pdf("Hello quarterly report")).unwrap();
        let text = extract_text(&path, 1024 * 1024).expect("pdf text layer");
        assert!(text.contains("quarterly"), "{text:?}");
    }

    #[test]
    fn a_malformed_pdf_is_skipped_not_a_panic() {
        let scratch = Scratch::new("badpdf");
        let path = scratch.path().join("broken.pdf");
        std::fs::write(&path, b"%PDF-1.4 this is not a real document").unwrap();
        // The parser may or may not recover; either way it must not panic and
        // must never invent text.
        if let Some(text) = extract_text(&path, 1024 * 1024) {
            assert!(!text.contains("quarterly"), "{text:?}");
        }
    }

    #[test]
    fn docx_extraction_respects_the_size_cap() {
        let big = format!(
            r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>"#,
            "a".repeat(4096)
        );
        let scratch = Scratch::new("cap");
        let path = fake_package(&scratch, "doc.docx", &[("word/document.xml", &big)]);
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
