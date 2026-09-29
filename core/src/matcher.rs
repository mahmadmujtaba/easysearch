//! Query model and matching.
//!
//! Everything-style semantics: whitespace-separated terms are ANDed,
//! a leading `!` excludes, matching is case-insensitive by default,
//! against basename or full path. Two modes per query: glob patterns
//! (`*.pdf`) or regex (`report\d+\.pdf$`).

use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result categories for the sidebar quick filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    #[default]
    All,
    Recent {
        max_age_secs: i64,
    },
    Images,
    Docs,
    Code,
    Archives,
    Audio,
    Video,
    Large {
        min_bytes: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Query {
    /// Raw query string (whitespace-separated terms; `!term` excludes).
    pub name: String,
    /// Treat every term as a regex instead of a glob.
    pub regex_mode: bool,
    pub case_sensitive: bool,
    pub include_hidden: bool,
    /// Match against the full path instead of the basename.
    pub full_path: bool,
    /// Optional content pattern (regex, searched inside files).
    pub content: Option<String>,
    /// Sidebar category filter (see [`Category`]).
    pub category: Category,
    /// Include directories in results (`false` = files only).
    pub include_dirs: bool,
    /// Restrict results to paths under this directory (the sidebar's Location
    /// filter). `None` = no restriction.
    pub under: Option<String>,
    /// Restrict results to files whose final extension is in this list
    /// (canonicalised on compile: trimmed, leading `.` stripped, lowercased,
    /// deduped). Empty = no extension filter. When non-empty, directories never
    /// match.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Inclusive lower bound on file size in bytes. When set, directories never
    /// match.
    #[serde(default)]
    pub min_size: Option<u64>,
    /// Inclusive upper bound on file size in bytes. When set, directories never
    /// match.
    #[serde(default)]
    pub max_size: Option<u64>,
    /// Match only entries modified within this many seconds of now
    /// (`now - mtime <= secs`; a future mtime still matches).
    #[serde(default)]
    pub modified_within_secs: Option<i64>,
    /// Fuzzy ("fzf-style") matching: a term matches when its characters appear
    /// in order somewhere in the target, so `mtn` finds `meeting-notes.md`.
    /// Applies to include terms only — `!term` exclusions keep their ordinary
    /// substring/glob meaning.
    #[serde(default)]
    pub fuzzy: bool,
    /// Content pattern may span lines (`foo\nbar`), instead of being matched
    /// against one line at a time. Much slower, so it is opt-in.
    #[serde(default)]
    pub multiline: bool,
    /// Match the name query **or** the content pattern, instead of requiring
    /// both (the "Full text" scope). Has no effect without a content pattern.
    #[serde(default)]
    pub content_or_name: bool,
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            name: String::new(),
            regex_mode: false,
            case_sensitive: false,
            include_hidden: false,
            full_path: false,
            content: None,
            category: Category::All,
            include_dirs: true,
            under: None,
            extensions: Vec::new(),
            min_size: None,
            max_size: None,
            modified_within_secs: None,
            fuzzy: false,
            multiline: false,
            content_or_name: false,
            limit: 1000,
        }
    }
}

/// Filters pulled out of `key:value` tokens in the query text.
///
/// Recognised keys: `ext:` (extensions), `size:` (`>`, `<`, `>=`, `<=`, `a..b`, and
/// B/KB/MB/GB/TB units, binary), `modified:`/`mod:`/`date:` (`today`, `yesterday`,
/// `week`, `month`, `year`, or `Nd`/`Nw`/`Nh`), `in:`/`under:` (a directory
/// prefix) and `file:` (files only). Anything else keeps its literal meaning, so
/// `time:12:30` still matches that text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryTokens {
    pub extensions: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_within_secs: Option<i64>,
    pub under: Option<String>,
    pub files_only: bool,
}

/// Split a query into terms, honouring double quotes: `"a b"` yields one term
/// whose text is `a b` (quotes stripped). The bool is true when the term was
/// quoted, so the caller can leave a quoted `key:value` as literal text.
fn split_terms(name: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut quoted = false;
    let mut has = false;
    for c in name.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                quoted = true;
                has = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has {
                    out.push((std::mem::take(&mut cur), quoted));
                    quoted = false;
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push((cur, quoted));
    }
    out
}

/// Parse `key:value` filter tokens out of `name`, returning the remaining terms
/// (to match) and the filters they specified.
pub fn parse_query_tokens(name: &str) -> (Vec<String>, QueryTokens) {
    let mut kept: Vec<String> = Vec::new();
    let mut toks = QueryTokens::default();
    for (term, quoted) in split_terms(name) {
        if term.is_empty() {
            continue;
        }
        if !quoted {
            if let Some(v) = term.strip_prefix("ext:") {
                for e in v.split([',', ';']) {
                    let e = e.trim().trim_start_matches('.').to_ascii_lowercase();
                    if !e.is_empty() && !toks.extensions.contains(&e) {
                        toks.extensions.push(e);
                    }
                }
                continue;
            }
            if let Some(v) = term.strip_prefix("size:")
                && let Some((min, max)) = parse_size(v)
            {
                toks.min_size = min.or(toks.min_size);
                toks.max_size = max.or(toks.max_size);
                continue;
            }
            if let Some(v) = term
                .strip_prefix("modified:")
                .or_else(|| term.strip_prefix("mod:"))
                .or_else(|| term.strip_prefix("date:"))
                && let Some(secs) = parse_age(v)
            {
                toks.modified_within_secs = Some(secs);
                continue;
            }
            if let Some(v) = term
                .strip_prefix("in:")
                .or_else(|| term.strip_prefix("under:"))
                && !v.is_empty()
            {
                toks.under = Some(v.to_string());
                continue;
            }
            if term == "file:" || term == "files:" {
                toks.files_only = true;
                continue;
            }
        }
        kept.push(term);
    }
    (kept, toks)
}

/// Parse a `size:` value — `>10MB`, `>=1KB`, `<2GB`, `1MB..100MB`, or an exact
/// `10MB` — into `(min, max)` byte bounds.
fn parse_size(v: &str) -> Option<(Option<u64>, Option<u64>)> {
    let v = v.trim();
    if let Some((a, b)) = v.split_once("..") {
        return Some((Some(parse_bytes(a)?), Some(parse_bytes(b)?)));
    }
    if let Some(rest) = v.strip_prefix(">=") {
        return Some((Some(parse_bytes(rest)?), None));
    }
    if let Some(rest) = v.strip_prefix("<=") {
        return Some((None, Some(parse_bytes(rest)?)));
    }
    if let Some(rest) = v.strip_prefix('>') {
        return Some((Some(parse_bytes(rest)?.saturating_add(1)), None));
    }
    if let Some(rest) = v.strip_prefix('<') {
        return Some((None, Some(parse_bytes(rest)?.saturating_sub(1))));
    }
    let n = parse_bytes(v)?;
    Some((Some(n), Some(n)))
}

/// Parse a byte quantity: `10`, `10B`, `1KB`, `2.5MB`, `1GB`, `1TB` (binary).
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("tb") {
        (n, 1024u64.pow(4))
    } else if let Some(n) = s.strip_suffix("gb") {
        (n, 1024u64.pow(3))
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, 1024u64.pow(2))
    } else if let Some(n) = s.strip_suffix("kb") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 1)
    } else {
        (s.as_str(), 1)
    };
    let value: f64 = num.trim().parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(value.mul_add(mult as f64, 0.5) as u64)
}

/// Parse a `modified:` value into a maximum age in seconds.
fn parse_age(v: &str) -> Option<i64> {
    let v = v.trim().to_ascii_lowercase();
    let secs = match v.as_str() {
        "today" => 24 * 3600,
        "yesterday" => 48 * 3600,
        "week" | "thisweek" => 7 * 24 * 3600,
        "month" => 30 * 24 * 3600,
        "year" => 365 * 24 * 3600,
        other => {
            let (num, unit) = other.split_at(other.len().saturating_sub(1));
            let n: i64 = num.parse().ok()?;
            match unit {
                "h" => n * 3600,
                "d" => n * 24 * 3600,
                "w" => n * 7 * 24 * 3600,
                _ => return None,
            }
        }
    };
    Some(secs)
}

/// True if `path`/`meta` matches the category filter.
pub fn matches_category(cat: &Category, path: &Path, meta: &crate::overlay::Meta) -> bool {
    let ext = |path: &Path| -> Option<String> {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
    };
    let in_set = |path: &Path, set: &[&str]| -> bool {
        ext(path).is_some_and(|e| set.contains(&e.as_str()))
    };
    match cat {
        Category::All => true,
        Category::Recent { max_age_secs } => {
            if meta.mtime <= 0 {
                return false;
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            now - meta.mtime <= *max_age_secs
        }
        Category::Images => in_set(
            path,
            &[
                "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tiff", "ico",
            ],
        ),
        Category::Docs => in_set(
            path,
            &[
                "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "log", "csv", "xls", "xlsx",
                "ods", "ppt", "pptx", "odp", "epub", "tex",
            ],
        ),
        Category::Code => in_set(
            path,
            &[
                "rs", "py", "js", "ts", "go", "c", "cpp", "h", "hpp", "java", "rb", "sh", "toml",
                "json", "yaml", "yml", "html", "css", "sql", "php", "lua", "zig", "ex", "exs",
                "kt", "swift", "v", "nix", "ps1", "bat",
            ],
        ),
        Category::Archives => in_set(
            path,
            &[
                "zip", "tar", "gz", "xz", "bz2", "7z", "rar", "zst", "deb", "rpm",
            ],
        ),
        Category::Audio => in_set(
            path,
            &["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "mid"],
        ),
        Category::Video => in_set(
            path,
            &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "mpg", "mpeg", "wmv",
            ],
        ),
        Category::Large { min_bytes } => !meta.is_dir && meta.size >= *min_bytes,
    }
}

#[derive(Clone, Debug)]
pub enum Term {
    Glob(GlobMatcher),
    Regex(Regex),
    /// Subsequence match with a quality score (see [`fuzzy_score`]).
    Fuzzy {
        needle: String,
        case_sensitive: bool,
    },
}

impl Term {
    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Term::Glob(g) => g.is_match(text),
            Term::Regex(r) => r.is_match(text),
            Term::Fuzzy {
                needle,
                case_sensitive,
            } => fuzzy_score(needle, text, *case_sensitive).is_some(),
        }
    }
}

/// A compiled, runnable query.
#[derive(Clone, Debug)]
pub struct CompiledQuery {
    pub terms: Vec<Term>,
    pub excludes: Vec<Term>,
    pub case_sensitive: bool,
    pub include_hidden: bool,
    pub full_path: bool,
    pub content: Option<String>,
    pub category: Category,
    pub include_dirs: bool,
    pub under: Option<String>,
    /// Canonicalised extension filter (see [`Query::extensions`]); empty = off.
    pub extensions: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_within_secs: Option<i64>,
    pub limit: usize,
    /// The content pattern may span lines (see [`Query::multiline`]).
    pub multiline: bool,
    /// Name terms and the content pattern are alternatives, not both required.
    pub content_or_name: bool,
    /// True if any name term/exclusion was given.
    pub has_name_filter: bool,
}

impl CompiledQuery {
    pub fn compile(q: &Query) -> Result<CompiledQuery, String> {
        let ci = !q.case_sensitive;
        // Pull out `key:value` filter tokens (Everything-style); the rest is the
        // name query. Quoted terms stay literal, so `"ext:pdf"` matches that text.
        let (kept, toks) = parse_query_tokens(&q.name);
        let mut terms = Vec::new();
        let mut excludes = Vec::new();
        for raw in &kept {
            let (neg, tok) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw.as_str()),
            };
            if tok.is_empty() {
                continue;
            }
            let term = compile_term(tok, q.regex_mode, ci, q.fuzzy && !neg)?;
            if neg {
                excludes.push(term);
            } else {
                terms.push(term);
            }
        }
        // Canonicalise the extension filter: trim, strip one leading '.',
        // lowercase, drop empties, dedupe (order preserved). An `ext:` token
        // overrides the query's own list.
        let ext_source: &[String] = if toks.extensions.is_empty() {
            &q.extensions
        } else {
            &toks.extensions
        };
        let mut extensions: Vec<String> = Vec::new();
        for raw in ext_source {
            let trimmed = raw.trim();
            let normalized = trimmed
                .strip_prefix('.')
                .unwrap_or(trimmed)
                .to_ascii_lowercase();
            if normalized.is_empty() || extensions.contains(&normalized) {
                continue;
            }
            extensions.push(normalized);
        }
        let min_size = toks.min_size.or(q.min_size);
        let max_size = toks.max_size.or(q.max_size);
        if let (Some(min), Some(max)) = (min_size, max_size)
            && min > max
        {
            return Err(format!(
                "min_size ({min} bytes) is greater than max_size ({max} bytes)"
            ));
        }
        Ok(CompiledQuery {
            terms,
            excludes,
            case_sensitive: q.case_sensitive,
            include_hidden: q.include_hidden,
            full_path: q.full_path,
            content: q.content.clone(),
            category: q.category,
            include_dirs: if toks.files_only {
                false
            } else {
                q.include_dirs
            },
            under: toks.under.clone().or_else(|| q.under.clone()),
            extensions,
            min_size,
            max_size,
            modified_within_secs: toks.modified_within_secs.or(q.modified_within_secs),
            limit: q.limit.max(1),
            multiline: q.multiline,
            content_or_name: q.content_or_name,
            has_name_filter: !kept.is_empty(),
        })
    }

    /// True if `path`'s final extension is in the filter (always true when the
    /// filter is empty). Directory exclusion is handled by the engine predicate
    /// [`crate::engine`]'s `accepts`, not here.
    pub fn extension_matches(&self, path: &Path) -> bool {
        if self.extensions.is_empty() {
            return true;
        }
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .is_some_and(|e| self.extensions.iter().any(|x| x == &e))
    }

    /// Match one indexed path against the name terms.
    pub fn name_matches(&self, path: &Path) -> bool {
        let target: &Path = match path.file_name() {
            Some(name) if !self.full_path => name.as_ref(),
            _ => path,
        };
        let text = target.to_string_lossy();
        let all = self.terms.iter().all(|t| t.is_match(&text));
        let none_excluded = !self.excludes.iter().any(|t| t.is_match(&text));
        all && none_excluded
    }
}

fn compile_term(
    tok: &str,
    regex_mode: bool,
    case_insensitive: bool,
    fuzzy: bool,
) -> Result<Term, String> {
    if regex_mode {
        RegexBuilder::new(tok)
            .case_insensitive(case_insensitive)
            .build()
            .map(Term::Regex)
            .map_err(|e| format!("invalid regex {tok:?}: {e}"))
    } else if fuzzy {
        // Case folding happens inside `fuzzy_score`.
        Ok(Term::Fuzzy {
            needle: tok.to_string(),
            case_sensitive: !case_insensitive,
        })
    } else {
        // Everything-style: a term without glob metacharacters is a substring
        // match ("draft" matches "draft.pdf"), so wrap it in `*...*`.
        let has_metachar = tok.contains(['*', '?', '[']);
        let pattern = if has_metachar {
            tok.to_string()
        } else {
            format!("*{tok}*")
        };
        GlobBuilder::new(&pattern)
            .case_insensitive(case_insensitive)
            .literal_separator(false)
            .build()
            .map(|g| Term::Glob(g.compile_matcher()))
            .map_err(|e| format!("invalid pattern {tok:?}: {e}"))
    }
}

/// Score a fuzzy (subsequence) match of `needle` in `haystack`, or `None` when
/// the needle's characters do not all appear in order.
///
/// This is the ranking used for the *Relevance* sort when fuzzy mode is on, and
/// the predicate behind [`Term::Fuzzy`]. Higher is better; the scale is a
/// transparent heuristic (the same spirit as `docs/scope.md` §10's relevance):
///
/// * `+16` for every matched character, `+6` more when it continues a run;
/// * `+8` when the match starts a word (string start, after `/ . _ - space (`),
///   or at a lower→upper transition);
/// * a gap penalty of up to `-12`, so compact matches beat scattered ones;
/// * a small length preference, so `a/b.md` beats `a/very/long/b.md`.
///
/// Case is compared ASCII-insensitively unless `case_sensitive`.
pub fn fuzzy_score(needle: &str, haystack: &str, case_sensitive: bool) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    let mut score: i32 = 0;
    let mut cursor = 0usize;
    let mut prev: Option<usize> = None;

    for nc in needle.chars() {
        let mut found = None;
        while cursor < hay.len() {
            let c = hay[cursor];
            let hit = if case_sensitive {
                c == nc
            } else {
                c.eq_ignore_ascii_case(&nc)
            };
            cursor += 1;
            if hit {
                found = Some(cursor - 1);
                break;
            }
        }
        let idx = found?; // a missing character means "not a match at all"

        score += 16;
        match prev {
            Some(p) if idx == p + 1 => score += 6,
            Some(p) => score -= ((idx - p - 1) as i32).min(12),
            None => {}
        }
        if is_word_start(&hay, idx) {
            score += 8;
        }
        prev = Some(idx);
    }

    // Prefer matches in shorter names (bounded so it never dominates).
    score -= ((hay.len() / 16) as i32).min(16);
    Some(score)
}

/// True if position `i` begins a word: the string start, a character after a
/// separator, or a lower→upper (camelCase) transition.
fn is_word_start(hay: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = hay[i - 1];
    matches!(prev, '/' | '\\' | '.' | '_' | '-' | ' ' | '(' | '[' | ',')
        || (prev.is_ascii_lowercase() && hay[i].is_ascii_uppercase())
}

/// True if any path component starts with `.` (hidden file/dir).
pub fn is_hidden(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with('.') && s.len() > 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(name: &str, regex_mode: bool) -> CompiledQuery {
        CompiledQuery::compile(&Query {
            name: name.into(),
            regex_mode,
            ..Query::default()
        })
        .unwrap()
    }

    #[test]
    fn glob_terms_and_excludes() {
        let cq = q("*.pdf !draft", false);
        assert!(cq.name_matches(Path::new("report.pdf")));
        assert!(!cq.name_matches(Path::new("draft.pdf")));
        assert!(!cq.name_matches(Path::new("report.txt")));
    }

    #[test]
    fn multiple_terms_are_anded() {
        let cq = q("invoice 2026", false);
        assert!(cq.name_matches(Path::new("invoice-2026-03.pdf")));
        assert!(!cq.name_matches(Path::new("invoice-2025-03.pdf")));
    }

    #[test]
    fn regex_mode() {
        let cq = q("report[_-][0-9]{4}\\.pdf$", true);
        assert!(cq.name_matches(Path::new("report_2024.pdf")));
        assert!(!cq.name_matches(Path::new("report_2024.pdf.bak")));
    }

    #[test]
    fn case_insensitive_by_default() {
        let cq = q("README", false);
        assert!(cq.name_matches(Path::new("readme.md")));
    }

    #[test]
    fn invalid_regex_reports_error() {
        let err = CompiledQuery::compile(&Query {
            name: "(".into(),
            regex_mode: true,
            ..Query::default()
        });
        assert!(err.is_err());
    }

    #[test]
    fn hidden_detection() {
        assert!(is_hidden(Path::new("/home/u/.config/x")));
        assert!(is_hidden(Path::new("/home/u/.git/config")));
        assert!(!is_hidden(Path::new("/home/u/docs")));
    }

    #[test]
    fn category_matching() {
        use crate::overlay::Meta;
        let m = |size: u64, mtime: i64, is_dir: bool| Meta {
            size,
            mtime,
            is_dir,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(matches_category(
            &Category::Images,
            Path::new("/x/photo.PNG"),
            &m(0, 0, false)
        ));
        assert!(!matches_category(
            &Category::Images,
            Path::new("/x/photo.pdf"),
            &m(0, 0, false)
        ));
        assert!(matches_category(
            &Category::Large { min_bytes: 1024 },
            Path::new("/x/big.bin"),
            &m(2048, 0, false)
        ));
        assert!(!matches_category(
            &Category::Large { min_bytes: 1024 },
            Path::new("/x/small.txt"),
            &m(512, 0, false)
        ));
        assert!(matches_category(
            &Category::Recent { max_age_secs: 3600 },
            Path::new("/x/n.txt"),
            &m(1, now - 60, false)
        ));
        assert!(!matches_category(
            &Category::Recent { max_age_secs: 3600 },
            Path::new("/x/o.txt"),
            &m(1, now - 7200, false)
        ));
        assert!(matches_category(
            &Category::Code,
            Path::new("/x/main.rs"),
            &m(1, 0, false)
        ));
        assert!(!matches_category(
            &Category::Code,
            Path::new("/x/main.md"),
            &m(1, 0, false)
        ));
        assert!(matches_category(
            &Category::Docs,
            Path::new("/x/note.md"),
            &m(1, 0, false)
        ));
    }

    #[test]
    fn extensions_are_canonicalised() {
        let cq = CompiledQuery::compile(&Query {
            extensions: vec![
                " .PDF ".into(),
                "Md".into(),
                "pdf".into(),
                "  ".into(),
                String::new(),
                ".txt".into(),
            ],
            ..Query::default()
        })
        .unwrap();
        assert_eq!(cq.extensions, vec!["pdf", "md", "txt"]);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let cq = CompiledQuery::compile(&Query {
            extensions: vec![".PDF".into()],
            ..Query::default()
        })
        .unwrap();
        assert!(cq.extension_matches(Path::new("/x/report.pdf")));
        assert!(cq.extension_matches(Path::new("/x/REPORT.PDF")));
        assert!(!cq.extension_matches(Path::new("/x/report.txt")));
        assert!(!cq.extension_matches(Path::new("/x/archive")));
        // Final extension only: `report.tar.gz` is a `gz`, not a `tar`.
        let gz = CompiledQuery::compile(&Query {
            extensions: vec!["gz".into()],
            ..Query::default()
        })
        .unwrap();
        assert!(gz.extension_matches(Path::new("/x/report.tar.gz")));
        assert!(!gz.extension_matches(Path::new("/x/report.gz.bak")));
    }

    #[test]
    fn extension_filter_off_when_empty() {
        let cq = CompiledQuery::compile(&Query::default()).unwrap();
        assert!(cq.extension_matches(Path::new("/x/anything")));
    }

    #[test]
    fn min_size_greater_than_max_is_a_compile_error() {
        let err = CompiledQuery::compile(&Query {
            min_size: Some(10),
            max_size: Some(5),
            ..Query::default()
        })
        .unwrap_err();
        assert!(err.contains("min_size"), "unclear error: {err}");
        assert!(err.contains("max_size"), "unclear error: {err}");
        // Equal bounds are allowed (inclusive, single-byte range).
        assert!(
            CompiledQuery::compile(&Query {
                min_size: Some(7),
                max_size: Some(7),
                ..Query::default()
            })
            .is_ok()
        );
    }

    #[test]
    fn legacy_json_without_new_fields_still_parses() {
        // The pre-filter payload shape: no extension/size/recency fields.
        let json = r#"{
            "name":"*.pdf",
            "regex_mode":false,
            "case_sensitive":false,
            "include_hidden":false,
            "full_path":false,
            "content":null,
            "category":"All",
            "include_dirs":true,
            "under":null,
            "limit":10
        }"#;
        let q: Query = serde_json::from_str(json).unwrap();
        assert!(q.extensions.is_empty());
        assert_eq!(q.min_size, None);
        assert_eq!(q.max_size, None);
        assert_eq!(q.modified_within_secs, None);
        assert!(!q.fuzzy, "fuzzy must default to off for old payloads");

        // The new fields survive a full round-trip.
        let original = Query {
            name: "*".into(),
            extensions: vec!["PDF".into()],
            min_size: Some(10),
            max_size: Some(20),
            modified_within_secs: Some(60),
            ..Query::default()
        };
        let round: Query =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(round.extensions, vec!["PDF"]);
        assert_eq!(round.min_size, Some(10));
        assert_eq!(round.max_size, Some(20));
        assert_eq!(round.modified_within_secs, Some(60));
    }

    #[test]
    fn fuzzy_matches_subsequences_in_order() {
        // Characters may be spread out, but must appear in order.
        assert!(fuzzy_score("mtn", "meeting-notes.md", false).is_some());
        assert!(fuzzy_score("MTN", "meeting-notes.md", false).is_some());
        assert!(fuzzy_score("rpt", "report.pdf", false).is_some());
        // Out-of-order does not match: "c" then "b" is not a subsequence of "abc".
        assert!(fuzzy_score("bc", "abc", false).is_some());
        assert!(fuzzy_score("cb", "abc", false).is_none());
        assert!(fuzzy_score("zzz", "meeting-notes.md", false).is_none());
        assert_eq!(fuzzy_score("", "anything", false), Some(0));
        // Case sensitivity is honored when asked for.
        assert!(fuzzy_score("MTN", "meeting-notes.md", true).is_none());
        assert!(fuzzy_score("met", "meeting-notes.md", true).is_some());
    }

    #[test]
    fn fuzzy_ranks_compacter_and_word_start_matches_higher() {
        // A contiguous match beats one scattered across filler characters.
        // (Separators like `-` intentionally count as word starts, so a
        // hyphenated spelling scores *well* — fzf behaves the same way.)
        let tight = fuzzy_score("note", "notes.txt", false).unwrap();
        let loose = fuzzy_score("note", "nqoqtqex.txt", false).unwrap();
        assert!(tight > loose, "{tight} should beat {loose}");
        // …and shorter names get a small preference.
        let short = fuzzy_score("a", "a.txt", false).unwrap();
        let long = fuzzy_score("a", "/a/much/longer/path/name.txt", false).unwrap();
        assert!(short > long, "{short} should beat {long}");
        // A word-start hit outscores the same letters buried mid-word.
        let start = fuzzy_score("rep", "report.txt", false).unwrap();
        let mid = fuzzy_score("rep", "xrep.txt", false).unwrap();
        assert!(start > mid, "{start} should beat {mid}");
    }

    #[test]
    fn fuzzy_mode_filters_by_subsequence() {
        let cq = CompiledQuery::compile(&Query {
            name: "mtn".into(),
            fuzzy: true,
            ..Query::default()
        })
        .unwrap();
        assert!(cq.name_matches(Path::new("/x/meeting-notes.md")));
        assert!(!cq.name_matches(Path::new("/x/readme.txt")));

        // Several terms are still ANDed.
        let cq = CompiledQuery::compile(&Query {
            name: "mtn notes".into(),
            fuzzy: true,
            ..Query::default()
        })
        .unwrap();
        assert!(cq.name_matches(Path::new("/x/meeting-notes.md")));
        assert!(!cq.name_matches(Path::new("/x/agenda.md")));

        // Without fuzzy the same term is an ordinary substring, which does not
        // match here — that is the difference the toggle buys.
        let plain = CompiledQuery::compile(&Query {
            name: "mtn".into(),
            ..Query::default()
        })
        .unwrap();
        assert!(!plain.name_matches(Path::new("/x/meeting-notes.md")));
    }

    #[test]
    fn fuzzy_exclusions_stay_literal() {
        // `!tmp` keeps its substring meaning even in fuzzy mode, so it only
        // drops names that really contain "tmp".
        let cq = CompiledQuery::compile(&Query {
            name: "mtn !tmp".into(),
            fuzzy: true,
            ..Query::default()
        })
        .unwrap();
        assert!(cq.name_matches(Path::new("/x/meeting-notes.md")));
        assert!(!cq.name_matches(Path::new("/x/meeting-tmp-notes.md")));
        // A scattered "t-m-p" is not an exclusion under substring semantics.
        assert!(cq.name_matches(Path::new("/x/meeting-time-notes.md")));
    }

    #[test]
    fn ext_token_filters_extensions() {
        let cq = q("report ext:pdf,doc", false);
        assert_eq!(cq.extensions, vec!["pdf", "doc"]);
        assert!(cq.extension_matches(Path::new("/a/report.pdf")));
        assert!(!cq.extension_matches(Path::new("/a/report.txt")));
        // The token is not part of the name query.
        assert!(cq.name_matches(Path::new("/a/report.pdf")));
    }

    #[test]
    fn size_token_parses_units_and_bounds() {
        let cq = q("size:>1MB", false);
        assert_eq!(cq.min_size, Some(1024 * 1024 + 1));
        assert_eq!(cq.max_size, None);
        let cq = q("size:1KB..2KB", false);
        assert_eq!((cq.min_size, cq.max_size), (Some(1024), Some(2048)));
        let cq = q("size:<10B", false);
        assert_eq!(cq.max_size, Some(9));
        // An unparseable value stays a literal term.
        let cq = q("size:big", false);
        assert_eq!(cq.min_size, None);
        assert_eq!(cq.terms.len(), 1);
    }

    #[test]
    fn modified_and_under_tokens() {
        let cq = q("modified:today in:/var/log", false);
        assert_eq!(cq.modified_within_secs, Some(24 * 3600));
        assert_eq!(cq.under.as_deref(), Some("/var/log"));
    }

    #[test]
    fn file_token_is_files_only() {
        let cq = q("file: notes", false);
        assert!(!cq.include_dirs);
        assert_eq!(cq.terms.len(), 1);
    }

    #[test]
    fn quoted_phrases_and_quoted_tokens() {
        let (kept, toks) = parse_query_tokens("\"ext:pdf\"");
        assert!(toks.extensions.is_empty());
        assert_eq!(kept, vec!["ext:pdf"]);
        // A quoted phrase is one term and matches a run with a space.
        let cq = q("\"meeting notes\"", false);
        assert!(cq.name_matches(Path::new("/x/meeting notes.md")));
        assert!(!cq.name_matches(Path::new("/x/notes-meeting.md")));
    }

    #[test]
    fn unknown_colon_token_is_literal() {
        let (kept, toks) = parse_query_tokens("time:12:30");
        assert_eq!(kept, vec!["time:12:30"]);
        assert_eq!(toks, QueryTokens::default());
    }
}
