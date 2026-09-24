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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    All,
    Recent { max_age_secs: i64 },
    Images,
    Docs,
    Code,
    Archives,
    Audio,
    Video,
    Large { min_bytes: u64 },
}

impl Default for Category {
    fn default() -> Self {
        Category::All
    }
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
            limit: 1000,
        }
    }
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
}

impl Term {
    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Term::Glob(g) => g.is_match(text),
            Term::Regex(r) => r.is_match(text),
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
    pub limit: usize,
    /// True if any name term/exclusion was given.
    pub has_name_filter: bool,
}

impl CompiledQuery {
    pub fn compile(q: &Query) -> Result<CompiledQuery, String> {
        let ci = !q.case_sensitive;
        let mut terms = Vec::new();
        let mut excludes = Vec::new();
        for raw in q.name.split_whitespace() {
            let (neg, tok) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if tok.is_empty() {
                continue;
            }
            let term = compile_term(tok, q.regex_mode, ci)?;
            if neg {
                excludes.push(term);
            } else {
                terms.push(term);
            }
        }
        Ok(CompiledQuery {
            terms,
            excludes,
            case_sensitive: q.case_sensitive,
            include_hidden: q.include_hidden,
            full_path: q.full_path,
            content: q.content.clone(),
            category: q.category,
            include_dirs: q.include_dirs,
            under: q.under.clone(),
            limit: q.limit.max(1),
            has_name_filter: !q.name.split_whitespace().any(|t| t.is_empty()),
        })
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

fn compile_term(tok: &str, regex_mode: bool, case_insensitive: bool) -> Result<Term, String> {
    if regex_mode {
        RegexBuilder::new(tok)
            .case_insensitive(case_insensitive)
            .build()
            .map(Term::Regex)
            .map_err(|e| format!("invalid regex {tok:?}: {e}"))
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
}
