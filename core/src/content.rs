//! On-demand content search via the embedded ripgrep engine.
//!
//! Always reads live disk state (results are as fresh as the files themselves).
//! `.docx` files are searched through an in-process OOXML text extractor (no
//! external tool); a file that cannot be extracted is skipped, never fatal.

use crate::content_index::{ContentIndex, ExtractQueue};
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use rayon::prelude::*;
use regex::Regex;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Maximum size of extracted .docx text we are willing to hold in memory.
const MAX_DOCX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

/// Files whose content is searched through the preprocessor hook.
const DOCX_EXTENSIONS: &[&str] = &["docx"];

/// A compiled content query: the ripgrep matcher (live search) plus a plain
/// regex (cheap check over cached text).
pub struct ContentPattern {
    matcher: RegexMatcher,
    plain: Regex,
}

impl ContentPattern {
    pub fn new(pattern: &str) -> Result<ContentPattern, String> {
        let matcher =
            RegexMatcher::new(pattern).map_err(|e| format!("invalid content pattern: {e}"))?;
        let plain = Regex::new(pattern).map_err(|e| format!("invalid content pattern: {e}"))?;
        Ok(ContentPattern { matcher, plain })
    }

    fn matches_cached(&self, text: &str) -> bool {
        self.plain.is_match(text)
    }
}

/// Search `paths` for `pattern` (regex). Returns matched paths, capped at
/// `limit` (approximate under parallelism). Uses the optional content cache
/// when present; misses fall back to live reads. docx files found during the
/// live pass are queued so the background index can cache them.
pub fn search_contents(
    paths: &[PathBuf],
    pattern: &ContentPattern,
    limit: usize,
    cache: &ContentIndex,
    queue: Option<&ExtractQueue>,
) -> Vec<PathBuf> {
    // Fast path: everything already cached in the content index.
    let mut hits: Vec<PathBuf> = paths
        .par_iter()
        .filter(|p| {
            cache
                .get_with(p, |t| pattern.matches_cached(t))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    if hits.len() >= limit {
        hits.truncate(limit);
        return hits;
    }

    // Live path: search whatever is not cached, one Searcher per rayon thread.
    let live: Vec<&PathBuf> = paths.iter().filter(|p| !cache.contains(p)).collect();

    let live_hits: Vec<PathBuf> = live
        .par_iter()
        .map_init(
            || {
                SearcherBuilder::new()
                    .binary_detection(BinaryDetection::quit(b'\x00'))
                    .build()
            },
            |searcher, p| search_one(searcher, pattern, p, queue).then(|| (*p).clone()),
        )
        .flatten()
        .collect();

    hits.extend(live_hits);
    hits.truncate(limit);
    hits
}

fn search_one(
    searcher: &mut Searcher,
    pattern: &ContentPattern,
    path: &Path,
    queue: Option<&ExtractQueue>,
) -> bool {
    // Only regular files.
    match std::fs::metadata(path) {
        Ok(m) if !m.is_file() => return false,
        Ok(_) => {}
        Err(_) => return false,
    }

    if is_docx(path) {
        let text = match crate::content_index::extract_text(path, MAX_DOCX_TEXT_BYTES) {
            Some(t) => t,
            None => return false,
        };
        if let Some(q) = queue {
            q.send(path.to_path_buf());
        }
        let mut cur = Cursor::new(text.into_bytes());
        return search_reader(searcher, pattern, &mut cur);
    }

    let mut found = false;
    {
        let mut sink = AnyMatchSink { found: false };
        let res = searcher.search_path(pattern.matcher.clone(), path, &mut sink);
        // Binary detected / permission error / etc: not a match.
        if res.is_ok() {
            found = sink.found;
        }
    }
    found
}

fn search_reader<R: std::io::Read>(
    searcher: &mut Searcher,
    pattern: &ContentPattern,
    r: &mut R,
) -> bool {
    let mut sink = AnyMatchSink { found: false };
    match searcher.search_reader(pattern.matcher.clone(), r, &mut sink) {
        Ok(()) => sink.found,
        Err(_) => false,
    }
}

fn is_docx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| DOCX_EXTENSIONS.iter().any(|d| e.eq_ignore_ascii_case(d)))
        .unwrap_or(false)
}

/// Sink that stops at the first match and records it.
struct AnyMatchSink {
    found: bool,
}

impl Sink for AnyMatchSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, _mat: &SinkMatch) -> Result<bool, Self::Error> {
        self.found = true;
        Ok(false) // stop searching this file
    }
}
