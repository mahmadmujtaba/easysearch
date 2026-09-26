//! EasySearch as **one file**: the GUI and the engine daemon in a
//! single executable.
//!
//! Run with no arguments it opens the GUI and makes sure a search daemon is
//! listening first — re-executing *itself* in `--daemon` mode when needed — so
//! users only download and run one thing. Because the engine is a real separate
//! process, its HTTP API (`docs/api.md`) works for other clients at the same
//! time, and the index keeps running if the GUI is closed.
//!
//! Fallback: if a daemon cannot be started (e.g. the port is taken by something
//! unrelated), the GUI runs the engine in-process instead of failing.

use easysearch_core::api::{API_VERSION, DEFAULT_ADDR, Health};
use easysearch_core::ipc as gui_ipc;
use easysearch_core::{Backend, Config, Engine, remote};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

/// Quick TCP probe timeout used before asking the daemon anything.
const PROBE: Duration = Duration::from_millis(300);
/// How long to wait for a freshly spawned daemon to answer.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of [`ensure_daemon_with`].
#[derive(Debug)]
pub enum Ensured {
    /// A healthy daemon was already listening.
    AlreadyUp,
    /// We started it; the child is returned so the caller can keep or drop it
    /// (dropping leaves the daemon running, which is what the GUI wants).
    Started(Child),
    /// No daemon could be started.
    Failed(String),
}

/// Read `--addr <host:port>` / `--addr=<host:port>` from arguments.
pub fn addr_from_args(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(rest) = arg.strip_prefix("--addr=") {
            return Some(rest.to_string());
        }
        if arg == "--addr" {
            return it.next().cloned();
        }
    }
    None
}

/// True when a healthy daemon answers at `addr`.
pub fn daemon_is_up(addr: &str) -> bool {
    if !remote::probe(addr, PROBE) {
        return false;
    }
    health(addr).is_some()
}

/// Fetch `/v1/health` from `addr`, if it answers with valid JSON.
pub fn health(addr: &str) -> Option<Health> {
    let body = remote::request(addr, "GET", "/v1/health", None).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Read the query out of `--search <QUERY>` / `--search=<QUERY>`, if present.
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

/// Make sure a daemon serves `addr`, spawning `exe` in `--daemon` mode if not.
pub fn ensure_daemon_with(addr: &str, exe: &Path) -> Ensured {
    if daemon_is_up(addr) {
        return Ensured::AlreadyUp;
    }
    let mut child = match spawn_daemon(addr, exe) {
        Ok(child) => child,
        Err(e) => return Ensured::Failed(format!("cannot start {addr}: {e}")),
    };

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if daemon_is_up(addr) {
            return Ensured::Started(child);
        }
        // If it exited already, no point waiting out the timeout.
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ensured::Failed(format!("daemon exited before serving {addr}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    Ensured::Failed(format!("daemon did not answer on {addr} in time"))
}

/// Like [`ensure_daemon_with`], using this executable.
pub fn ensure_daemon(addr: &str) -> Ensured {
    match std::env::current_exe() {
        Ok(exe) => ensure_daemon_with(addr, &exe),
        Err(e) => Ensured::Failed(format!("cannot locate own executable: {e}")),
    }
}

/// Poll `addr` until a healthy daemon answers (bounded).
pub fn wait_for_daemon(addr: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if daemon_is_up(addr) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Spawn `exe --daemon --addr <addr>`, detached from this process.
pub fn spawn_daemon(addr: &str, exe: &Path) -> std::io::Result<Child> {
    spawn_daemon_with_env(addr, exe, &[])
}

/// Like [`spawn_daemon`], with extra environment variables for the child.
///
/// Passing the child's environment explicitly (rather than mutating ours) keeps
/// callers — including tests — thread-safe: `std::env::set_var` is `unsafe` in
/// edition 2024 precisely because it races with other threads.
pub fn spawn_daemon_with_env(
    addr: &str,
    exe: &Path,
    envs: &[(String, String)],
) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .arg("--addr")
        .arg(addr)
        .arg("--quiet")
        .stdin(Stdio::null());
    for (key, value) in envs {
        cmd.env(key, value);
    }

    // Send the daemon's output to a log file so it never writes to a closed
    // stdout once the GUI exits.
    match daemon_log_path().and_then(|p| std::fs::File::create(p).ok()) {
        Some(out) => {
            if let Ok(err) = out.try_clone() {
                cmd.stdout(out).stderr(err);
            } else {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    // Detach into its own session so the daemon outlives the GUI (and the
    // terminal it was launched from).
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    cmd.spawn()
}

/// Where the auto-started daemon writes its log (`$XDG_CACHE_HOME/easysearch`).
pub fn daemon_log_path() -> Option<PathBuf> {
    let dir = Config::default_disk_index_dir();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("daemon.log"))
}

/// Run the engine as a daemon until the process exits.
pub fn run_daemon(addr: &str, quiet: bool) -> Result<(), String> {
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
                    eprintln!("easysearch-daemon: index live — {files} files, {dirs} folders");
                }
            })
            .ok();
    }
    easysearch_daemon::run_forever(engine, addr)
}

/// Open the GUI, starting a daemon first when possible.
///
/// `--search <QUERY>` on the command line runs that search in the window as
/// soon as it opens, so the launcher path and the control path behave alike.
///
/// Returns `Err` only if the window subsystem itself fails (mapped to a string
/// so this crate needs no direct dependency on the GUI toolkit).
pub fn run_gui(addr: &str) -> Result<(), String> {
    let backend = match ensure_daemon(addr) {
        Ensured::AlreadyUp => {
            eprintln!("easysearch: using the daemon already running on {addr}");
            Backend::remote(addr)
        }
        Ensured::Started(_child) => {
            eprintln!("easysearch: started a search daemon on {addr}");
            if let Some(h) = health(addr)
                && h.api != API_VERSION
            {
                eprintln!(
                    "easysearch: warning: daemon API {} differs from expected {API_VERSION}",
                    h.api
                );
            }
            Backend::remote(addr)
        }
        Ensured::Failed(why) => {
            eprintln!("easysearch: {why} — running the engine in-process instead");
            Backend::local(Config::load())
        }
    };
    let initial = search_from_args(&std::env::args().skip(1).collect::<Vec<_>>());
    easysearch_gui::run_with_query(std::sync::Arc::new(backend), initial).map_err(|e| e.to_string())
}

/// Ask a running daemon to stop (`POST /v1/shutdown`).
///
/// The CLI counterpart to the tray's *Quit*. The window is a separate process,
/// so `--quit` cannot stop the service — and the daemon may be the only thing
/// running, with no window to talk to at all.
pub fn stop_daemon(addr: &str) -> Result<(), String> {
    remote::request(addr, "POST", "/v1/shutdown", Some(b"{}")).map(|_| ())
}

/// Default daemon address (re-exported for the binary's help text).
pub const DEFAULT: &str = DEFAULT_ADDR;

/// Handle the control commands that talk to a *running* GUI.
///
/// Returns `Some(exit_code)` when the command was handled, or `None` when a
/// window-opening command (`--toggle`, `--show`, `--search`) found nothing
/// listening — in that case the caller should start the GUI, so one shortcut
/// both launches the app and drives its window. A fresh `--search QUERY` start
/// carries the query through [`search_from_args`].
///
/// This is how a global hotkey is bound on Wayland (which has no global-hotkey
/// API): the desktop runs `easysearch --toggle` and it reaches the
/// instance that is already up. See `docs/ui.md`.
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
        // the caller starts the GUI (launcher semantics); the rest are quiet
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
