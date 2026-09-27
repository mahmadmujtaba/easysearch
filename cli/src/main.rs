//! `easysearch-cli` — headless search CLI.
//!
//! It indexes in-process: there is no daemon to attach to, so a script gets an
//! answer without starting (or disturbing) the GUI app. Running it while the app
//! is up opens the same on-disk index read/write; if you only need counts or a
//! quick lookup, that is fine, but the app remains the only *watcher*.

use clap::{Parser, Subcommand};
use easysearch_core::{Backend, Config, Query, State};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "easysearch-cli",
    version,
    about = "Realtime file and content search for Linux (Everything-style)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search filenames and/or file contents (realtime, live index)
    Search {
        /// Everything-style query: space-separated terms are ANDed,
        /// `!term` excludes (e.g. "invoice 2026 *.pdf !draft").
        /// Omit (or pass "") for content-only searches.
        #[arg(default_value = "")]
        query: String,
        /// Treat query terms as regex instead of glob patterns
        #[arg(long)]
        regex: bool,
        /// Also search file contents with this regex pattern
        /// (combined with the name query: results must match both)
        #[arg(long, value_name = "PATTERN")]
        content: Option<String>,
        /// With --content: match the *name* query OR the content pattern
        /// instead of requiring both (the GUI's "Full text" scope)
        #[arg(long)]
        any: bool,
        /// Let the content pattern span lines (e.g. --content 'foo\nbar')
        #[arg(long)]
        multiline: bool,
        /// Case-sensitive matching
        #[arg(long)]
        case: bool,
        /// Include hidden files and directories
        #[arg(long)]
        hidden: bool,
        /// Fuzzy (fzf-style) matching: the term's characters in order, anywhere
        /// (`mtn` finds `meeting-notes.md`). Exclusions (`!term`) stay literal.
        #[arg(long)]
        fuzzy: bool,
        /// Match against the full path instead of the basename
        #[arg(long)]
        path: bool,
        /// Restrict results to this directory subtree
        #[arg(long, value_name = "DIR")]
        under: Option<String>,
        /// Only files with one of these extensions (comma-separated, e.g. pdf,docx,md)
        #[arg(long, value_name = "LIST")]
        ext: Option<String>,
        /// Minimum file size — plain bytes or K/M/G suffix, 1024-based (e.g. 10M)
        #[arg(long, value_name = "SIZE")]
        min_size: Option<String>,
        /// Maximum file size — plain bytes or K/M/G suffix, 1024-based (e.g. 500K)
        #[arg(long, value_name = "SIZE")]
        max_size: Option<String>,
        /// Only files modified within this age (seconds, or s/m/h/d/w suffix, e.g. 7d)
        #[arg(long, value_name = "DURATION")]
        modified_within: Option<String>,
        /// Maximum number of results
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Don't wait for the initial index to finish before searching
        #[arg(long)]
        no_wait: bool,
    },
    /// Show index status (state, counts, watcher mode)
    Status,
    /// Start the engine (and wait for the index) — useful for warm-up
    Index,
}

fn main() {
    // Before clap or the in-process engine can start a thread.
    easysearch_core::process::cap_malloc_arenas();
    let _ = easysearch_core::Config::load().ensure_exclude_names_file();
    let cli = Cli::parse();
    // The engine runs in this process: there is no service to attach to.
    let backend = Backend::local(Config::load());

    match cli.command {
        Command::Search {
            query,
            regex,
            content,
            any,
            multiline,
            case,
            hidden,
            fuzzy,
            path,
            under,
            ext,
            min_size,
            max_size,
            modified_within,
            limit,
            no_wait,
        } => {
            if !no_wait {
                backend.wait_live(Duration::from_secs(120));
            }
            let extensions = ext
                .as_deref()
                .map(|list| {
                    list.split(',')
                        .map(|e| e.trim().to_string())
                        .filter(|e| !e.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let min_size = min_size
                .as_deref()
                .map(|s| parse_or_exit("--min-size", s, parse_size));
            let max_size = max_size
                .as_deref()
                .map(|s| parse_or_exit("--max-size", s, parse_size));
            let modified_within_secs = modified_within
                .as_deref()
                .map(|s| parse_or_exit("--modified-within", s, parse_duration_secs));
            let q = Query {
                name: query,
                regex_mode: regex,
                case_sensitive: case,
                include_hidden: hidden,
                full_path: path,
                content,
                category: easysearch_core::Category::All,
                include_dirs: true,
                under,
                extensions,
                min_size,
                max_size,
                modified_within_secs,
                fuzzy,
                multiline,
                content_or_name: any,
                limit,
            };
            match backend.search(&q) {
                Ok(resp) => {
                    for r in &resp.results {
                        println!("{}", r.path.display());
                    }
                    eprintln!(
                        "{} result(s){} in {} ms ({} files indexed)",
                        resp.results.len(),
                        if resp.truncated { ", truncated" } else { "" },
                        resp.elapsed_ms,
                        resp.indexed,
                    );
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::Status | Command::Index => {
            backend.wait_live(Duration::from_secs(120));
            let s = backend.status_snapshot();
            let (files, dirs) = backend.counts();
            println!("state:          {:?}", s.state);
            println!("files:          {files}");
            println!("dirs:           {dirs}");
            if s.base_entries > 0 {
                println!(
                    "index:          disk-backed ({} entries, {} files / {} dirs in base)",
                    s.base_entries, s.base_files, s.base_dirs
                );
            } else {
                println!("index:          in-memory (RAM-only mode)");
            }
            println!("overlay:        {} pending change(s)", s.overlay_pending);
            println!(
                "watcher:        {}",
                if s.degraded {
                    "degraded (periodic rebuild)"
                } else {
                    "live (inotify)"
                }
            );
            if s.watch_failures > 0 {
                println!(
                    "unwatchable:    {} dir(s) (covered by periodic rebuild)",
                    s.watch_failures
                );
            }
            println!("skipped dirs:   {}", s.skipped);
            match &s.content_index {
                easysearch_core::ContentIndexStatus::Disabled => {
                    println!("content index:  disabled")
                }
                easysearch_core::ContentIndexStatus::Enabled {
                    entries,
                    bytes,
                    pending,
                } => {
                    println!(
                        "content index:  enabled ({entries} files, {} MiB, {pending} pending)",
                        bytes / (1024 * 1024)
                    );
                }
            }
            if s.state != State::Live {
                eprintln!("warning: index did not reach Live state");
            }
        }
    }
}

/// Run `parse` on a flag value, printing a clear error and exiting non-zero on
/// bad input.
fn parse_or_exit<T>(flag: &str, value: &str, parse: impl Fn(&str) -> Result<T, String>) -> T {
    match parse(value) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {flag}: {e}");
            std::process::exit(2);
        }
    }
}

/// Parse a byte size: plain bytes, or a `K`/`M`/`G` suffix (case-insensitive,
/// 1024-based), e.g. `1024`, `10K`, `10M`, `2G`.
fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let Some(last) = t.chars().last() else {
        return Err("empty size".into());
    };
    let (digits, multiplier): (&str, u64) = match last.to_ascii_uppercase() {
        'K' => (&t[..t.len() - 1], 1024),
        'M' => (&t[..t.len() - 1], 1024 * 1024),
        'G' => (&t[..t.len() - 1], 1024 * 1024 * 1024),
        _ => (t, 1),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("invalid size {s:?} (use bytes or a K/M/G suffix)"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| format!("size {s:?} is too large"))
}

/// Parse a duration into seconds: plain seconds, or a `s`/`m`/`h`/`d`/`w`
/// suffix (case-insensitive), e.g. `3600`, `30m`, `24h`, `7d`.
fn parse_duration_secs(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let Some(last) = t.chars().last() else {
        return Err("empty duration".into());
    };
    let (digits, multiplier): (&str, i64) = match last.to_ascii_lowercase() {
        's' => (&t[..t.len() - 1], 1),
        'm' => (&t[..t.len() - 1], 60),
        'h' => (&t[..t.len() - 1], 3600),
        'd' => (&t[..t.len() - 1], 86_400),
        'w' => (&t[..t.len() - 1], 604_800),
        _ => (t, 1),
    };
    let n: i64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration {s:?} (use seconds or an s/m/h/d/w suffix)"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| format!("duration {s:?} is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_suffixes_are_1024_based_and_case_insensitive() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("10K").unwrap(), 10 * 1024);
        assert_eq!(parse_size("10k").unwrap(), 10 * 1024);
        assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size(" 3m ").unwrap(), 3 * 1024 * 1024);
        assert!(parse_size("10X").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn duration_suffixes_map_to_seconds() {
        assert_eq!(parse_duration_secs("3600").unwrap(), 3600);
        assert_eq!(parse_duration_secs("30m").unwrap(), 1800);
        assert_eq!(parse_duration_secs("24h").unwrap(), 86_400);
        assert_eq!(parse_duration_secs("7d").unwrap(), 604_800);
        assert_eq!(parse_duration_secs("2w").unwrap(), 1_209_600);
        assert!(parse_duration_secs("7q").is_err());
        assert!(parse_duration_secs("").is_err());
    }
}
