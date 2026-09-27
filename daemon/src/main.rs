//! `easysearch-daemon` — the search engine, driven over stdin/stdout by the
//! process that spawned it (see [`easysearch_core::proto`]).
//!
//! It is a **child process**: it owns the index, answers JSON frames on its
//! stdin/stdout, and exits when its parent closes the pipe or asks it to stop.
//! There is no socket, no port and no HTTP, so nothing about the index is
//! reachable from outside the process pair.
//!
//! End users normally don't need this binary: the combined `easysearch` app
//! spawns the same engine itself. This standalone binary is for tests, scripts
//! and headless setups that want to drive the protocol directly.

use clap::Parser;
use easysearch_core::{Config, Engine};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "easysearch-daemon",
    version,
    about = "EasySearch engine (JSON frames on stdin/stdout; normally a child of the app)"
)]
struct Cli {
    /// Don't print a line when the initial index becomes live.
    #[arg(long)]
    quiet: bool,
    /// Keep the content cache in RAM (up to `content_index_total_cap_bytes`)
    /// instead of spooling it to disk. Faster for repeated content searches.
    #[arg(long)]
    content_in_memory: bool,
}

fn main() {
    // Before clap, the engine, or anything else can start a thread.
    easysearch_core::process::cap_malloc_arenas();
    let _ = easysearch_core::Config::load().ensure_exclude_names_file();
    let cli = Cli::parse();
    if cli.content_in_memory {
        easysearch_core::process::use_content_memory();
    }

    let mut engine = Engine::new(Config::load());
    engine.start();
    let engine = Arc::new(engine);

    if !cli.quiet {
        // Announce readiness once the index is live (on stderr, so it cannot be
        // mistaken for a protocol frame).
        let watcher = Arc::clone(&engine);
        std::thread::Builder::new()
            .name("ready".into())
            .spawn(move || {
                if watcher.wait_live(Duration::from_secs(600)) {
                    let (files, dirs) = watcher.counts();
                    eprintln!("easysearch-daemon: index live — {files} files, {dirs} folders");
                }
            })
            .ok();
    }

    if let Err(e) = easysearch_daemon::serve(engine) {
        eprintln!("easysearch-daemon: {e}");
        std::process::exit(1);
    }
}
