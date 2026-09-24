//! Everything for Linux as **one file**: the GUI and the engine daemon in a
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

use everything_core::api::{Health, API_VERSION, DEFAULT_ADDR};
use everything_core::{remote, Backend, Config, Engine};
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

/// Spawn `exe --daemon --addr <addr>`, detached from this process.
pub fn spawn_daemon(addr: &str, exe: &Path) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .arg("--addr")
        .arg(addr)
        .arg("--quiet")
        .stdin(Stdio::null());

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

/// Where the auto-started daemon writes its log (`$XDG_CACHE_HOME/everything-linux`).
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
                    eprintln!("everything-daemon: index live — {files} files, {dirs} folders");
                }
            })
            .ok();
    }
    everything_daemon::run_forever(engine, addr)
}

/// Open the GUI, starting a daemon first when possible.
///
/// Returns `Err` only if the window subsystem itself fails (mapped to a string
/// so this crate needs no direct dependency on the GUI toolkit).
pub fn run_gui(addr: &str) -> Result<(), String> {
    let backend = match ensure_daemon(addr) {
        Ensured::AlreadyUp => {
            eprintln!("everything-linux: using the daemon already running on {addr}");
            Backend::remote(addr)
        }
        Ensured::Started(_child) => {
            eprintln!("everything-linux: started a search daemon on {addr}");
            if let Some(h) = health(addr) {
                if h.api != API_VERSION {
                    eprintln!(
                        "everything-linux: warning: daemon API {} differs from expected {API_VERSION}",
                        h.api
                    );
                }
            }
            Backend::remote(addr)
        }
        Ensured::Failed(why) => {
            eprintln!("everything-linux: {why} — running the engine in-process instead");
            Backend::local(Config::load())
        }
    };
    everything_gui::run(std::sync::Arc::new(backend)).map_err(|e| e.to_string())
}

/// Default daemon address (re-exported for the binary's help text).
pub const DEFAULT: &str = DEFAULT_ADDR;
