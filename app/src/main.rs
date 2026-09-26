//! `everything-linux` — Everything for Linux in a single file.
//!
//! One executable that is both the GUI and the search daemon: run it and it
//! starts a daemon for you (re-executing itself) and opens the GUI attached to
//! it. Run it as `everything-linux --daemon` to run only the engine, which is
//! also what the auto-start path uses internally.

use std::process::ExitCode;

const HELP: &str = "\
Everything for Linux — realtime file and content search

USAGE:
    everything-linux [OPTIONS]

Run with no options to open the GUI. A search daemon is started automatically
(this same binary re-executes itself in daemon mode) and the GUI attaches to
it, so the HTTP API on the daemon address keeps working for other clients and
the index keeps running after the GUI is closed.

OPTIONS:
    --addr <HOST:PORT>   daemon address (default 127.0.0.1:5858)
    --daemon             run only the search daemon, no GUI
    --quiet              daemon: don't log when the index becomes live
    -h, --help           show this help
    -V, --version        show the version

CONTROL (talk to a running window — bind these to desktop shortcuts):
    --toggle             show the window if hidden, hide it if visible
    --show / --hide      show / hide the window
    --search <QUERY>     show the window and run a search
    --quit               ask the running app to exit

The first --toggle with nothing running starts the app, so one key can both
launch it and toggle the window.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("everything-linux {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Control commands drive an already-running window (global-hotkey support);
    // `--toggle` falls through to a normal start when nothing is listening.
    if let Some(code) = everything_app::control_command(&args) {
        return ExitCode::from(code as u8);
    }

    let addr = everything_app::addr_from_args(&args)
        .unwrap_or_else(|| everything_app::DEFAULT.to_string());

    if args.iter().any(|a| a == "--daemon") {
        let quiet = args.iter().any(|a| a == "--quiet");
        return match everything_app::run_daemon(&addr, quiet) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("everything-linux: {e}");
                ExitCode::FAILURE
            }
        };
    }

    match everything_app::run_gui(&addr) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("everything-linux: {e}");
            ExitCode::FAILURE
        }
    }
}
