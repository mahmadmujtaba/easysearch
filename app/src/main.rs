//! `easysearch` — EasySearch in a single file.
//!
//! One executable plays every role:
//!
//! - **host** (`easysearch --daemon`): the engine + the tray + the control
//!   socket, with no window — started automatically when needed and kept alive
//!   in the background;
//! - **window** (`easysearch --window`): a GUI process the host spawns, talking
//!   to it over the child's pipes; closing it just ends that process;
//! - **engine** (`easysearch --engine`): the raw engine on stdin/stdout;
//! - **control** (`--toggle`, `--show`, `--hide`, `--search`, `--quit`): drive a
//!   running host from a desktop shortcut.
//!
//! Running it with no options makes sure the host is up and shows a window. The
//! window and the host are a parent and its child, so there is no socket, port
//! or HTTP between them (`docs/scope.md`).

use std::process::ExitCode;

const HELP: &str = "\
EasySearch — realtime file and content search

USAGE:
    easysearch [OPTIONS]

Run with no options to open a window. A background process (the tray and the
search engine) is started automatically and keeps the index warm; closing the
window leaves it running, and the next launch opens a window instantly.

OPTIONS:
    -h, --help           show this help
    -V, --version        show the version

CONTROL (talk to the running instance — bind these to desktop shortcuts):
    --toggle             show a window if none is open, close it otherwise
    --show / --hide      open / close the window
    --search <QUERY>     open a window and run a search
    --quit               stop the window, the tray and the engine

INTERNAL (not for humans):
    --daemon             run only the background process (engine + tray)
    --window             run only the window (connects to the host on stdio)
    --engine             run only the engine, speaking JSON on stdin/stdout
    --quiet              engine: don't log when the index becomes live
    --content-in-memory  keep the content cache in RAM instead of spooling it to
                         disk; faster for repeated content searches, but it holds
                         up to the configured cap (256 MB) resident

The first --toggle, --show or --search with nothing running starts the host, so
one key both launches the app and drives its window.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before anything can start a thread: bound glibc's malloc arenas, and take
    // the content-cache mode from the command line so the host inherits it.
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

    // The window half of the app: the host spawns this and speaks to it over our
    // stdin/stdout.
    if args.iter().any(|a| a == "--window") {
        let query = easysearch_app::search_from_args(&args);
        return match easysearch_app::run_window(query) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("easysearch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // The background host: engine + tray + control socket.
    if args.iter().any(|a| a == "--daemon") {
        return match easysearch_app::run_daemon() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("easysearch: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // The raw engine half: the parent has its pipes attached.
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

    // Control commands drive the host, starting it if needed.
    if let Some(code) = easysearch_app::control_command(&args) {
        return ExitCode::from(code as u8);
    }

    // No flags at all: the desktop launcher. Ensure the host and show a window.
    match easysearch_app::run_app() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("easysearch: {e}");
            ExitCode::FAILURE
        }
    }
}
