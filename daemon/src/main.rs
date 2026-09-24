//! `everything-daemon` — runs the search engine as its own process and serves
//! it over a localhost HTTP/JSON API (`docs/api.md`).
//!
//! Keeping the engine out of the GUI means the index keeps running (and other
//! clients keep working: CLI, scripts, curl, any language) even if no GUI is
//! running at all.
//!
//! End users normally don't need this binary: the combined `everything-linux`
//! app starts a daemon by re-executing itself. This standalone binary is for
//! running the engine on its own (servers, scripts, headless setups).

use clap::Parser;
use everything_core::{Config, Engine};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "everything-daemon",
    version,
    about = "Realtime file and content search daemon (localhost HTTP/JSON API)"
)]
struct Cli {
    /// Address to bind. Localhost only — the API has no authentication.
    #[arg(long, default_value = everything_core::api::DEFAULT_ADDR)]
    addr: String,
    /// Don't print a line when the initial index becomes live.
    #[arg(long)]
    quiet: bool,
}

fn main() {
    let cli = Cli::parse();

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
                    eprintln!("everything-daemon: index live — {files} files, {dirs} folders");
                }
            })
            .ok();
    }

    if let Err(e) = everything_daemon::run_forever(engine, &cli.addr) {
        eprintln!("everything-daemon: {e}");
        std::process::exit(1);
    }
}
