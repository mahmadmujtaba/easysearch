//! EasySearch as a **host and a window**: one binary that runs a background
//! process owning the tray and the engine, and spawns the GUI as a short-lived
//! child.
//!
//! Why a host at all? A window cannot be unmapped on Wayland, and `winit`
//! refuses to recreate its event loop, so a window that is *closed* cannot be
//! reopened inside the same process. Making the tray and the index live in a
//! host that outlives any window is the only way for "close the window" to mean
//! the window is really gone while the app keeps indexing:
//!
//! ```text
//!   easysearch --daemon        host: engine + tray + control socket (no window)
//!        │
//!        └── easysearch --window   … a window, over the child's pipes (JSON frames)
//! ```
//!
//! When the window closes (its X button, the tray toggle, `--hide`), that
//! process ends and the host simply spawns a fresh one on the next
//! `--show`/`--toggle`. The engine never restarts, so the index stays warm and
//! the search is instant. There is still no network: the window and the host are
//! a parent and its child, and the only socket is the local
//! `$XDG_RUNTIME_DIR/easysearch.sock` control channel.

use easysearch_core::ipc as gui_ipc;
use easysearch_core::proto::{Event, Op, Request};
use easysearch_core::{Backend, ChildEngine, Config, Engine};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use easysearch_gui::tray::TrayMsg;

/// Read `--search <QUERY>` / `--search=<QUERY>` out of the arguments, if present.
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

/// Where the host writes its log (`$XDG_CACHE_HOME/easysearch`).
pub fn engine_log_path() -> Option<PathBuf> {
    let dir = Config::default_disk_index_dir();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("engine.log"))
}

/// Run the engine in this process, serving the protocol on stdin/stdout.
///
/// This is `easysearch --engine`: the standalone engine, used by the tests and
/// by anyone driving the protocol by hand.
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

/// One action requested of the host, from the tray or the control socket.
#[derive(Clone, Debug)]
enum DaemonMsg {
    Show,
    /// Show the window and focus the search box (the tray's left-click / *Open*).
    Open,
    Hide,
    Toggle,
    /// Show the window, clear its results and focus the search box.
    NewSearch,
    /// Empty the open window's result list.
    ClearResults,
    /// Rebuild the on-disk index (the engine lives here, so no window is needed).
    RebuildIndex,
    /// Reveal the index folder in the file manager.
    OpenIndexFolder,
    /// Open the Settings dialog.
    Settings,
    /// Open the About dialog.
    About,
    /// Drop the recent-search history.
    ClearHistory,
    Search(String),
    Quit,
}

impl DaemonMsg {
    fn from_ipc(c: gui_ipc::Command) -> DaemonMsg {
        match c {
            gui_ipc::Command::Toggle => DaemonMsg::Toggle,
            gui_ipc::Command::Show => DaemonMsg::Show,
            gui_ipc::Command::Hide => DaemonMsg::Hide,
            gui_ipc::Command::Quit => DaemonMsg::Quit,
            gui_ipc::Command::Search(q) => DaemonMsg::Search(q),
        }
    }

    fn from_tray(m: TrayMsg) -> DaemonMsg {
        match m {
            TrayMsg::Open => DaemonMsg::Open,
            TrayMsg::Toggle => DaemonMsg::Toggle,
            TrayMsg::NewSearch => DaemonMsg::NewSearch,
            TrayMsg::ClearResults => DaemonMsg::ClearResults,
            TrayMsg::Search(q) => DaemonMsg::Search(q),
            TrayMsg::ClearHistory => DaemonMsg::ClearHistory,
            TrayMsg::RebuildIndex => DaemonMsg::RebuildIndex,
            TrayMsg::OpenIndexFolder => DaemonMsg::OpenIndexFolder,
            TrayMsg::Settings => DaemonMsg::Settings,
            TrayMsg::About => DaemonMsg::About,
            TrayMsg::Quit => DaemonMsg::Quit,
        }
    }
}

/// Run the background host: the engine, the tray and the control socket, plus
/// the window process it opens on demand. Returns when the user quits.
pub fn run_daemon() -> Result<(), String> {
    // The engine runs here, so it survives every window open/close.
    let mut engine = Engine::new(Config::load());
    engine.start();
    let daemon = easysearch_daemon::Daemon::new(Arc::new(engine));

    let (tx, rx) = mpsc::channel::<DaemonMsg>();

    // System tray (best effort: some desktops have no StatusNotifier host).
    let history = Arc::new(Mutex::new(easysearch_gui::recent_searches()));
    let _tray = match easysearch_gui::tray::spawn_tray(
        &easysearch_gui::app_title(),
        Arc::clone(&history),
    ) {
        Ok((tray_rx, handle)) => {
            forward_tray(tray_rx, tx.clone());
            Some(handle)
        }
        Err(e) => {
            eprintln!("easysearch: system tray unavailable: {e}");
            None
        }
    };

    // Control socket: `easysearch --toggle|--show|--hide|--search|--quit`.
    {
        let (ipc_tx, ipc_rx) = mpsc::channel::<gui_ipc::Command>();
        gui_ipc::spawn_listener(ipc_tx);
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("ipc-forward".into())
            .spawn(move || {
                for cmd in ipc_rx {
                    if tx.send(DaemonMsg::from_ipc(cmd)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }

    // Keep the tray's recent-searches menu in step with the window's history.
    {
        let history = Arc::clone(&history);
        std::thread::Builder::new()
            .name("history".into())
            .spawn(move || {
                loop {
                    let recent = easysearch_gui::recent_searches();
                    if let Ok(mut h) = history.lock()
                        && *h != recent
                    {
                        *h = recent;
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
            })
            .ok();
    }

    let mut window: Option<Window> = None;
    loop {
        // Reap a window whose process has ended.
        if window.as_mut().is_some_and(|w| w.finished()) {
            window = None;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(msg) => match msg {
                // Actions that need a window: open one if none is up, then tell it
                // what to do.
                DaemonMsg::Show => to_window(&mut window, &daemon, Event::Show),
                DaemonMsg::Open => to_window(&mut window, &daemon, Event::FocusSearch),
                DaemonMsg::NewSearch => to_window(&mut window, &daemon, Event::NewSearch),
                DaemonMsg::OpenIndexFolder => {
                    to_window(&mut window, &daemon, Event::OpenIndexFolder)
                }
                DaemonMsg::Settings => to_window(&mut window, &daemon, Event::Settings),
                DaemonMsg::About => to_window(&mut window, &daemon, Event::About),
                // Clearing results only makes sense with a window already open.
                DaemonMsg::ClearResults => {
                    if let Some(w) = &window {
                        w.send(Event::ClearResults);
                    }
                }
                DaemonMsg::Search(q) => match &window {
                    Some(w) => w.send(Event::Search { query: q }),
                    None => window = Some(Window::spawn(Arc::clone(&daemon), Some(q))),
                },
                DaemonMsg::Toggle => match &window {
                    Some(w) => w.send(Event::Hide),
                    None => window = Some(Window::spawn(Arc::clone(&daemon), None)),
                },
                DaemonMsg::Hide => {
                    if let Some(w) = &window {
                        w.send(Event::Hide);
                    }
                }
                // The engine lives in the host, so a rebuild needs no window.
                DaemonMsg::RebuildIndex => {
                    daemon.dispatch(Request {
                        id: 0,
                        op: Op::Rebuild,
                        payload: None,
                    });
                }
                // Clearing history with a window open has to happen *there*, or
                // the window's own prefs save would write the old list back.
                DaemonMsg::ClearHistory => match &window {
                    Some(w) => w.send(Event::ClearHistory),
                    None => {
                        easysearch_gui::clear_recent_searches();
                        if let Ok(mut h) = history.lock() {
                            h.clear();
                        }
                    }
                },
                DaemonMsg::Quit => {
                    if let Some(w) = &window {
                        w.send(Event::Quit);
                        // Give the window a moment to save and exit before we go.
                        let deadline = Instant::now() + Duration::from_millis(800);
                        while Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                    break;
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
            // Every sender is gone (no tray, no socket): nothing left to serve.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

fn forward_tray(rx: Receiver<TrayMsg>, tx: Sender<DaemonMsg>) {
    std::thread::Builder::new()
        .name("tray-forward".into())
        .spawn(move || {
            for msg in rx {
                if tx.send(DaemonMsg::from_tray(msg)).is_err() {
                    break;
                }
            }
        })
        .ok();
}

/// Send `event` to the window, opening a fresh one first if none is up.
fn to_window(window: &mut Option<Window>, daemon: &Arc<easysearch_daemon::Daemon>, event: Event) {
    match window {
        Some(w) => w.send(event),
        None => {
            let w = Window::spawn(Arc::clone(daemon), None);
            w.send(event);
            *window = Some(w);
        }
    }
}

/// A running window process, reached over its stdin/stdout.
struct Window {
    /// The window's stdin: replies and events are written here.
    out: easysearch_daemon::Out,
    child: std::process::Child,
}

impl Window {
    /// Spawn `easysearch --window` and serve its requests from `daemon`.
    fn spawn(daemon: Arc<easysearch_daemon::Daemon>, query: Option<String>) -> Window {
        let exe = std::env::current_exe().expect("cannot find our own executable");
        let mut cmd = Command::new(exe);
        cmd.arg("--window");
        if let Some(q) = query {
            cmd.arg("--search").arg(q);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        match engine_log_path().and_then(|p| std::fs::File::create(p).ok()) {
            Some(log) => {
                cmd.stderr(log);
            }
            None => {
                cmd.stderr(Stdio::null());
            }
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                eprintln!("easysearch: cannot open a window: {e}");
                // A window that never opened would leave the host with nothing to
                // do; exit so the next launcher starts cleanly.
                std::process::exit(1);
            }
        };
        let stdin = child.stdin.take().expect("window stdin");
        let stdout = child.stdout.take().expect("window stdout");
        let out: easysearch_daemon::Out = Arc::new(Mutex::new(Box::new(stdin)));

        // Serve the window's requests; the thread ends when the window exits.
        let _ = easysearch_daemon::serve_reader(daemon, stdout, Arc::clone(&out));

        Window { out, child }
    }

    fn send(&self, event: Event) {
        easysearch_daemon::write_event(&self.out, &event);
    }

    fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
    }
}

/// Run one window, talking to the host over this process's stdin/stdout.
///
/// This is `easysearch --window`: the GUI process the host spawns. It exits when
/// its window closes, leaving the host and the index running.
pub fn run_window(query: Option<String>) -> Result<(), String> {
    // Our stdin/stdout are the host's pipes: frames in, frames out.
    let client = ChildEngine::from_stdio();
    let events = client.take_events().unwrap_or_else(|| mpsc::channel().1);
    let quit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let backend = Arc::new(Backend::child(client));
    easysearch_gui::run_with(backend, query, events, quit).map_err(|e| e.to_string())
}

/// The no-argument entry point (the desktop launcher): make sure the host is
/// running and ask it to show a window.
pub fn run_app() -> Result<(), String> {
    if gui_ipc::send(&gui_ipc::Command::Show).unwrap_or(false) {
        return Ok(());
    }
    ensure_host()?;
    match gui_ipc::send(&gui_ipc::Command::Show) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the background process is not listening".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Start the background host detached from this process, and wait until its
/// control socket is up.
fn ensure_host() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    match engine_log_path().and_then(|p| std::fs::File::create(p).ok()) {
        Some(log) => {
            cmd.stderr(log);
        }
        None => {
            cmd.stderr(Stdio::null());
        }
    }
    // Detach into a new session, so the host outlives this launcher and the
    // terminal it was started from (no controlling terminal, no SIGHUP).
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| format!("cannot start the background process: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if gui_ipc::socket_path().exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("the background process did not start".into())
}

/// Handle the control commands that talk to a *running* host.
///
/// A window-opening command (`--toggle`, `--show`, `--search`) starts the host
/// first if nothing is listening, so one desktop shortcut both launches the app
/// and drives it. `--hide` and `--quit` are quiet no-ops when nothing runs.
pub fn control_command(args: &[String]) -> Option<i32> {
    let (command, opens_window) = parse_control(args)?;
    match gui_ipc::send(&command) {
        Ok(true) => Some(0),
        Ok(false) if opens_window => match ensure_host() {
            Ok(()) => match gui_ipc::send(&command) {
                Ok(true) => Some(0),
                _ => {
                    eprintln!(
                        "easysearch: the background process did not accept {}",
                        command.encode()
                    );
                    Some(1)
                }
            },
            Err(e) => {
                eprintln!("easysearch: {e}");
                Some(1)
            }
        },
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

/// Parse the control flags. `Some((command, opens_window))` when one is present.
fn parse_control(args: &[String]) -> Option<(gui_ipc::Command, bool)> {
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
    command.map(|c| (c, opens_window))
}
