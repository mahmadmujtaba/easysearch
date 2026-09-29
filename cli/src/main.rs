//! `easysearch-cli` — headless search CLI.
//!
//! It indexes in-process: there is no daemon to attach to, so a script gets an
//! answer without starting (or disturbing) the GUI app. Running it while the app
//! is up opens the same on-disk index read/write; if you only need counts or a
//! quick lookup, that is fine, but the app remains the only *watcher*.
//!
//! `search` runs one query and exits; `watch` re-runs it on a timer and streams
//! matches it has not printed before. Both accept `--json` for scripting.

use clap::{Args, Parser, Subcommand};
use easysearch_core::{Backend, Config, Query, ResultRow, State};
use std::collections::HashSet;
use std::io::Write;
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

/// The filters `search` and `watch` share.
#[derive(Args)]
struct SearchArgs {
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
    /// Print JSON instead of plain paths: one response object for `search`,
    /// one result object per line for `watch`
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Search filenames and/or file contents (realtime, live index)
    Search {
        #[command(flatten)]
        args: SearchArgs,
    },
    /// Re-run a query on a timer and print newly matching paths as they appear
    /// (streams the baseline first, then only paths not seen before)
    Watch {
        #[command(flatten)]
        args: SearchArgs,
        /// Seconds between rescans
        #[arg(long, default_value_t = 2)]
        interval: u64,
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
        Command::Search { args } => run_search(&backend, &args),
        Command::Watch { args, interval } => run_watch(&backend, &args, interval),
        Command::Status | Command::Index => print_status(&backend),
    }
}

/// Build the [`Query`] the shared flags describe.
fn build_query(args: &SearchArgs) -> Result<Query, String> {
    let extensions = args
        .ext
        .as_deref()
        .map(|list| {
            list.split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Query {
        name: args.query.clone(),
        regex_mode: args.regex,
        case_sensitive: args.case,
        include_hidden: args.hidden,
        full_path: args.path,
        content: args.content.clone(),
        category: easysearch_core::Category::All,
        include_dirs: true,
        under: args.under.clone(),
        extensions,
        min_size: args
            .min_size
            .as_deref()
            .map(|s| parse_or_exit("--min-size", s, parse_size)),
        max_size: args
            .max_size
            .as_deref()
            .map(|s| parse_or_exit("--max-size", s, parse_size)),
        modified_within_secs: args
            .modified_within
            .as_deref()
            .map(|s| parse_or_exit("--modified-within", s, parse_duration_secs)),
        fuzzy: args.fuzzy,
        multiline: args.multiline,
        content_or_name: args.any,
        limit: args.limit,
    })
}

fn run_search(backend: &Backend, args: &SearchArgs) {
    if !args.no_wait {
        backend.wait_live(Duration::from_secs(120));
    }
    let q = match build_query(args) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    match backend.search(&q) {
        Ok(resp) => {
            if args.json {
                match serde_json::to_string(&resp) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("error: could not encode JSON: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                for r in &resp.results {
                    println!("{}", r.path.display());
                }
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

/// Re-run the query every `interval` seconds and print paths not seen before.
/// Runs until interrupted (`Ctrl-C`).
fn run_watch(backend: &Backend, args: &SearchArgs, interval: u64) {
    backend.wait_live(Duration::from_secs(120));
    let q = match build_query(args) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    let every = Duration::from_secs(interval.max(1));
    loop {
        match backend.search(&q) {
            Ok(resp) => {
                for row in &resp.results {
                    let key = row.path.display().to_string();
                    if seen.insert(key) {
                        print_row(row, args.json);
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // Flush so a pipe sees each match as it appears, not at exit.
        let _ = std::io::stdout().flush();
        std::thread::sleep(every);
    }
}

/// One match: a path, or (with `--json`) a JSON object per line.
fn print_row(row: &ResultRow, json: bool) {
    if json {
        match serde_json::to_string(row) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: could not encode JSON: {e}"),
        }
    } else {
        println!("{}", row.path.display());
    }
}

fn print_status(backend: &Backend) {
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
    println!("paused:         {}", s.paused);
    if s.last_index_at > 0 {
        println!("last indexed:   {} (unix)", s.last_index_at);
    }
    match &s.content_index {
        easysearch_core::ContentIndexStatus::Disabled => println!("content index:  disabled"),
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

    #[test]
    fn cli_parses_search_and_watch_json() {
        // `--json` and the shared filters reach both subcommands.
        let cli = Cli::try_parse_from([
            "easysearch-cli",
            "search",
            "report",
            "--ext",
            "pdf,md",
            "--limit",
            "5",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Search { args } => {
                assert_eq!(args.query, "report");
                assert_eq!(args.ext.as_deref(), Some("pdf,md"));
                assert_eq!(args.limit, 5);
                assert!(args.json);
            }
            _ => panic!("expected a search"),
        }

        let cli = Cli::try_parse_from([
            "easysearch-cli",
            "watch",
            "TODO",
            "--interval",
            "9",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Watch { args, interval } => {
                assert_eq!(args.query, "TODO");
                assert_eq!(interval, 9);
                assert!(args.json);
            }
            _ => panic!("expected a watch"),
        }
    }
}
