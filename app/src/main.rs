//! `easysearch` — EasySearch in a single file.
//!
//! One executable that is both the GUI and the search daemon: run it and it
//! starts a daemon for you (re-executing itself) and opens the GUI attached to
//! it. Run it as `easysearch --daemon` to run only the engine, which is
//! also what the auto-start path uses internally.

use std::process::ExitCode;

const HELP: &str = "\
EasySearch — realtime file and content search

USAGE:
    easysearch [OPTIONS]

Run with no options to open the GUI. A search daemon is started automatically
(this same binary re-executes itself in daemon mode) and the GUI attaches to
it. The daemon owns the index and the tray icon; closing the window leaves both
running, and the HTTP API keeps answering for other clients.

OPTIONS:
    --addr <HOST:PORT>   daemon address (default 127.0.0.1:5858)
    --daemon             run only the search daemon, no GUI
    --quiet              daemon: don't log when the index becomes live
    --content-in-memory  keep the content cache in RAM instead of spooling it to
                         disk; faster for repeated content searches, but it holds
                         up to the configured cap (256 MB) resident
    -h, --help           show this help
    -V, --version        show the version

CONTROL (talk to a running window — bind these to desktop shortcuts):
    --toggle             show the window if hidden, hide it if visible
    --show / --hide      show / hide the window
    --search <QUERY>     show the window and run a search
    --quit               close the window; the background service keeps running
    --stop               stop the background service (`POST /v1/shutdown`)

The first --toggle, --show or --search with nothing running starts the app, so
one key both launches it and drives it.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before anything can start a thread: bound glibc's malloc arenas, and take
    // the content-cache mode from the command line so the daemon spawned below
    // inherits it through the environment.
    easysearch_core::process::cap_malloc_arenas();
    if args.iter().any(|a| a == "--content-in-memory") {
        easysearch_core::process::use_content_memory();
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("easysearch {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Control commands drive an already-running window (global-hotkey support);
    // `--toggle`/`--show`/`--search` fall through to a normal start when nothing
    // is listening.
    if let Some(code) = easysearch_app::control_command(&args) {
        return ExitCode::from(code as u8);
    }

    let addr = easysearch_app::addr_from_args(&args)
        .unwrap_or_else(|| easysearch_app::DEFAULT.to_string());

    // `--stop` stops the background service itself (the window may not exist).
    if args.iter().any(|a| a == "--stop") {
        return match easysearch_app::stop_daemon(&addr) {
            Ok(()) => {
                println!("easysearch: asked the service on {addr} to stop");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("easysearch: cannot stop the service on {addr}: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if args.iter().any(|a| a == "--daemon") {
        let quiet = args.iter().any(|a| a == "--quiet");
        return match easysearch_app::run_daemon(&addr, quiet) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("easysearch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    match easysearch_app::run_gui(&addr) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("easysearch: {e}");
            ExitCode::FAILURE
        }
    }
}
