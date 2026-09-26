//! `easysearch` — EasySearch in a single file.
//!
//! One executable that is both the **app** (window + tray) and the **engine**:
//! run it and it opens the GUI and spawns the engine as its child
//! (`easysearch --engine`, the same binary with the pipes attached). The engine
//! owns the index; the app owns the window and the tray. Closing the window
//! hides it, so the engine keeps indexing; quitting stops both.
//!
//! There is no socket, no port and no HTTP: the app and its engine talk over the
//! child's stdin/stdout (`docs/scope.md`).

use std::process::ExitCode;

const HELP: &str = "\
EasySearch — realtime file and content search

USAGE:
    easysearch [OPTIONS]

Run with no options to open the GUI. The engine (this same binary, re-executed
with --engine) is started as a child process and owns the index; the app keeps
its pipes, so nothing is exposed to the network. Closing the window hides it to
the tray and the engine keeps running; quitting stops both.

OPTIONS:
    --engine             run only the engine, speaking JSON on stdin/stdout
                         (this is how the app spawns it; not for humans)
    --quiet              engine: don't log when the index becomes live
    --content-in-memory  keep the content cache in RAM instead of spooling it to
                         disk; faster for repeated content searches, but it holds
                         up to the configured cap (256 MB) resident
    -h, --help           show this help
    -V, --version        show the version

CONTROL (talk to a running window — bind these to desktop shortcuts):
    --toggle             show the window if hidden, hide it if visible
    --show / --hide      show / hide the window
    --search <QUERY>     show the window and run a search
    --quit               stop the app (window and engine)

The first --toggle, --show or --search with nothing running starts the app, so
one key both launches it and drives it.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before anything can start a thread: bound glibc's malloc arenas, and take
    // the content-cache mode from the command line so the engine child inherits
    // it through the environment.
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

    // The engine half: the parent has its pipes attached.
    if args.iter().any(|a| a == "--engine") {
        let quiet = args.iter().any(|a| a == "--quiet");
        return match easysearch_app::run_engine(quiet) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("easysearch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    match easysearch_app::run_gui() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("easysearch: {e}");
            ExitCode::FAILURE
        }
    }
}
