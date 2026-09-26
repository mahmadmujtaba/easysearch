//! EasySearch as **one binary, two processes**: the app (window + tray) and the
//! engine it spawns as its child.
//!
//! The engine is a real separate process, so the index keeps running and stays
//! responsive while the window is hidden, and a crash in the UI cannot corrupt
//! the index. It is *not* a network service: the app owns the child's
//! stdin/stdout and speaks JSON frames over them (`docs/api.md`), so there is no
//! socket, no port, and nothing about the index is reachable from outside the
//! app and its engine.
//!
//! Fallback: if the child cannot be spawned, the window runs the engine
//! in-process instead of failing.

use easysearch_core::api::API_VERSION;
use easysearch_core::ipc as gui_ipc;
use easysearch_core::{Backend, ChildEngine, Config, Engine};
use std::path::PathBuf;
use std::time::Duration;

/// Read `--search <QUERY>` / `--search=<QUERY>` out of the arguments, if present.
///
/// Used so a `--search` that has to launch the app still runs the search once
/// the window is up.
pub fn search_from_args(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(rest) = arg.strip_prefix("--search=") {
            return Some(rest.to_string());
        }
        if arg == "--search" {
            return it.next().cloned();
        }
    }
    None
}

/// Where the engine child writes its stderr (`$XDG_CACHE_HOME/easysearch`).
pub fn engine_log_path() -> Option<PathBuf> {
    let dir = Config::default_disk_index_dir();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("engine.log"))
}

/// Spawn this executable in engine mode, keeping its pipes.
///
/// The child's stderr goes to the cache log, so it never writes to a terminal
/// the app does not own.
pub fn spawn_engine_child() -> std::io::Result<ChildEngine> {
    let exe = std::env::current_exe()?;
    let log = engine_log_path().and_then(|p| std::fs::File::create(p).ok());
    ChildEngine::spawn(&exe, &["--engine", "--quiet"], &[], log)
}

/// Run the engine in this process, serving the protocol on stdin/stdout.
///
/// This is what `easysearch --engine` does — the mode the app spawns itself in.
pub fn run_engine(quiet: bool) -> Result<(), String> {
    let mut engine = Engine::new(Config::load());
    engine.start();
    let engine = std::sync::Arc::new(engine);

    if !quiet {
        let watcher = std::sync::Arc::clone(&engine);
        std::thread::Builder::new()
            .name("ready".into())
            .spawn(move || {
                if watcher.wait_live(Duration::from_secs(600)) {
                    let (files, dirs) = watcher.counts();
                    eprintln!("easysearch: engine index live — {files} files, {dirs} folders");
                }
            })
            .ok();
    }
    easysearch_daemon::serve(engine)
}

/// Open the GUI with the engine running as its child (or in-process, as a
/// fallback).
///
/// Returns `Err` only if the window subsystem itself fails (mapped to a string
/// so this crate needs no direct dependency on the GUI toolkit).
pub fn run_gui() -> Result<(), String> {
    let engine = match spawn_engine_child() {
        Ok(engine) => {
            if let Ok(health) = engine.health()
                && health.api != API_VERSION
            {
                eprintln!(
                    "easysearch: warning: engine protocol {} differs from expected {API_VERSION}",
                    health.api
                );
            }
            eprintln!("easysearch: engine started (pid {:?})", engine.pid());
            Backend::child(engine)
        }
        Err(e) => {
            eprintln!("easysearch: cannot start the engine ({e}) — running it in this process");
            Backend::local(Config::load())
        }
    };

    let initial = search_from_args(&std::env::args().skip(1).collect::<Vec<_>>());
    easysearch_gui::run_with_query(std::sync::Arc::new(engine), initial).map_err(|e| e.to_string())
}

/// Handle the control commands that talk to a *running* window.
///
/// Returns `Some(exit_code)` when the command was handled, or `None` when a
/// window-opening command (`--toggle`, `--show`, `--search`) found nothing
/// listening — in that case the caller should start the app, so one shortcut
/// both launches it and drives its window. A fresh `--search QUERY` start carries
/// the query through [`search_from_args`].
///
/// This is how a global hotkey is bound on Wayland (which has no global-hotkey
/// API): the desktop runs `easysearch --toggle` and it reaches the instance that
/// is already up. See `docs/ui.md`.
pub fn control_command(args: &[String]) -> Option<i32> {
    let mut command: Option<gui_ipc::Command> = None;
    let mut opens_window = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--toggle" => {
                opens_window = true;
                command = Some(gui_ipc::Command::Toggle);
            }
            "--show" => {
                opens_window = true;
                command = Some(gui_ipc::Command::Show);
            }
            "--hide" => command = Some(gui_ipc::Command::Hide),
            "--quit" => command = Some(gui_ipc::Command::Quit),
            "--search" => {
                opens_window = true;
                let query = args.get(i + 1).cloned().unwrap_or_default();
                command = Some(gui_ipc::Command::Search(query));
                i += 1;
            }
            _ if arg.starts_with("--search=") => {
                opens_window = true;
                command = Some(gui_ipc::Command::Search(
                    arg["--search=".len()..].to_string(),
                ));
            }
            _ => {}
        }
        i += 1;
    }

    let command = command?;
    match gui_ipc::send(&command) {
        Ok(true) => Some(0),
        // Nothing is listening. A command that opens the window falls through so
        // the caller starts the app (launcher semantics); the rest are quiet
        // no-ops because there is nothing to control.
        Ok(false) if opens_window => None,
        Ok(false) => {
            eprintln!(
                "easysearch: nothing is running to {}. (Start the app first.)",
                command.encode()
            );
            Some(0)
        }
        Err(e) => {
            eprintln!("easysearch: cannot reach the running instance: {e}");
            Some(1)
        }
    }
}
