//! Query model and matching.
//!
//! Everything-style semantics: whitespace-separated terms are ANDed,
//! a leading `!` excludes, matching is case-insensitive by default,
//! against basename or full path. Two modes per query: glob patterns
//! (`*.pdf`) or regex (`report\d+\.pdf$`).

use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};
use std::path::Path;

#[derive(Clone, Debug)]
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
            limit: 1000,
        }
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
        let pattern = if has_metachar { tok.to_string() } else { format!("*{tok}*") };
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
}
