//! `easysearch-daemon` — runs the search engine as its own process and serves
//! it over a localhost HTTP/JSON API (`docs/api.md`).
//!
//! Keeping the engine out of the GUI means the index keeps running (and other
//! clients keep working: CLI, scripts, curl, any language) even if no GUI is
//! running at all.
//!
//! End users normally don't need this binary: the combined `easysearch`
//! app starts a daemon by re-executing itself. This standalone binary is for
//! running the engine on its own (servers, scripts, headless setups).

use clap::Parser;
use easysearch_core::{Config, Engine};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "easysearch-daemon",
    version,
    about = "Realtime file and content search daemon (localhost HTTP/JSON API)"
)]
struct Cli {
    /// Address to bind. Localhost only — the API has no authentication.
    #[arg(long, default_value = easysearch_core::api::DEFAULT_ADDR)]
    addr: String,
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
    let cli = Cli::parse();
    if cli.content_in_memory {
        easysearch_core::process::use_content_memory();
    }

    let mut engine = Engine::new(Config::load());
    engine.start();
    let engine = Arc::new(engine);

    if !cli.quiet {
        // Announce readiness once the index is live (useful for scripts).
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

    if let Err(e) = easysearch_daemon::run_forever(engine, &cli.addr) {
        eprintln!("easysearch-daemon: {e}");
        std::process::exit(1);
    }
}
