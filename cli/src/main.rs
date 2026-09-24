//! `everything` — headless search CLI.
//!
//! By default it indexes in-process (no daemon needed). With `--remote ADDR` it
//! instead queries a running `everything-daemon`, so the CLI keeps working even
//! when no GUI is involved.

use clap::{Parser, Subcommand};
use everything_core::{Backend, Config, Query, State};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "everything",
    version,
    about = "Realtime file and content search for Linux (Everything-style)"
)]
struct Cli {
    /// Query a running daemon instead of indexing in this process
    /// (e.g. --remote 127.0.0.1:5858).
    #[arg(long, global = true, value_name = "ADDR")]
    remote: Option<String>,

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
        /// Case-sensitive matching
        #[arg(long)]
        case: bool,
        /// Include hidden files and directories
        #[arg(long)]
        hidden: bool,
        /// Match against the full path instead of the basename
        #[arg(long)]
        path: bool,
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
    let cli = Cli::parse();
    // Local (default) or a running daemon.
    let backend = match &cli.remote {
        Some(addr) => Backend::remote(addr.clone()),
        None => Backend::local(Config::load()),
    };

    match cli.command {
        Command::Search {
            query,
            regex,
            content,
            case,
            hidden,
            path,
            limit,
            no_wait,
        } => {
            if !no_wait {
                backend.wait_live(Duration::from_secs(120));
            }
            let q = Query {
                name: query,
                regex_mode: regex,
                case_sensitive: case,
                include_hidden: hidden,
                full_path: path,
                content,
                category: everything_core::Category::All,
                include_dirs: true,
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
                    "index:          mmap-backed ({} entries, {} files / {} dirs in base)",
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
                everything_core::ContentIndexStatus::Disabled => {
                    println!("content index:  disabled")
                }
                everything_core::ContentIndexStatus::Enabled {
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
