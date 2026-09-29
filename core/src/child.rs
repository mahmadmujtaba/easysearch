//! The window-side half of the engine protocol: JSON frames over a pair of
//! pipes ([`crate::proto`]).
//!
//! There is no socket and no HTTP. The frames travel over pipes that are private
//! to the two processes:
//!
//! - [`ChildEngine::spawn`] spawns the engine as this process's child and keeps
//!   its stdin/stdout (the CLI, the tests, and the standalone `easysearch-gui`).
//! - [`ChildEngine::from_pipes`] adopts this process's **own** stdin/stdout, so
//!   the window can be a child of a background host that owns the engine and the
//!   tray (the combined `easysearch` app).
//!
//! Either way requests are multiplexed by id, so a slow content search never
//! blocks the status poller, and the host may push unsolicited [`Event`]s down
//! the same pipe (show/hide/quit/search).

use crate::api::{CountDto, Health, SearchResponseDto, StatusReport};
use crate::engine::{SearchResponse, State, Status};
use crate::matcher::Query;
use crate::proto::{ConfigPatch, Event, Op, Request, Response};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// How long to wait for one reply. Content searches over a big tree can take a
/// while, but a hung engine must not hang the caller for ever.
const IO_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the background poller refreshes status.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Shared plumbing between the client, the reader thread and the poller.
struct Inner {
    stdin: Mutex<Box<dyn Write + Send>>,
    /// Replies waiting for their caller, keyed by request id.
    pending: Mutex<HashMap<u64, mpsc::Sender<Response>>>,
    next_id: AtomicU64,
    /// Cleared on drop so the poller thread can stop.
    alive: AtomicBool,
    /// Unsolicited events pushed by the host (see [`Event`]).
    events: mpsc::Sender<Event>,
}

/// A running engine, reached over pipes.
pub struct ChildEngine {
    inner: Arc<Inner>,
    /// The child process, when this client spawned it.
    child: Option<Mutex<Child>>,
    report: Arc<RwLock<Option<StatusReport>>>,
    connected: Arc<AtomicBool>,
    events: Mutex<Option<mpsc::Receiver<Event>>>,
}

impl ChildEngine {
    /// Spawn `exe` with `args` as the engine, keeping its pipes.
    ///
    /// `envs` are extra environment variables for the child (tests isolate the
    /// config and cache this way). `log` receives the child's stderr; without one
    /// it goes to `/dev/null`.
    pub fn spawn(
        exe: &Path,
        args: &[&str],
        envs: &[(String, String)],
        log: Option<std::fs::File>,
    ) -> std::io::Result<ChildEngine> {
        let mut command = Command::new(exe);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        match log {
            Some(file) => match file.try_clone() {
                Ok(err) => {
                    command.stderr(err);
                }
                Err(_) => {
                    command.stderr(Stdio::null());
                }
            },
            None => {
                command.stderr(Stdio::null());
            }
        }

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no engine stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no engine stdout")
        })?;

        Ok(Self::build(stdout, Box::new(stdin), Some(child)))
    }

    /// Adopt this process's own stdin/stdout as the channel to a host that owns
    /// the engine — the window-as-a-child case. The caller keeps its stderr for
    /// logging; stdin/stdout become protocol-only.
    pub fn from_stdio() -> ChildEngine {
        Self::build(std::io::stdin(), Box::new(std::io::stdout()), None)
    }

    /// Build the client around an already-open reader/writer pair.
    fn build(
        reader: impl Read + Send + 'static,
        writer: Box<dyn Write + Send>,
        child: Option<Child>,
    ) -> ChildEngine {
        let (event_tx, event_rx) = mpsc::channel::<Event>();
        let inner = Arc::new(Inner {
            stdin: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            events: event_tx,
        });
        let report = Arc::new(RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));

        // Incoming frames: a reply goes to its waiter, an event to the UI.
        {
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("engine-in".into())
                .spawn(move || {
                    for line in BufReader::new(reader).lines() {
                        let Ok(line) = line else { break };
                        if line.trim().is_empty() {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                            continue;
                        };
                        if value.get("id").is_some() {
                            let Ok(response) = serde_json::from_value::<Response>(value) else {
                                continue;
                            };
                            let waiter = inner.pending.lock().unwrap().remove(&response.id);
                            if let Some(waiter) = waiter {
                                let _ = waiter.send(response);
                            }
                        } else if value.get("event").is_some()
                            && let Ok(event) = serde_json::from_value::<Event>(value)
                        {
                            let _ = inner.events.send(event);
                        }
                    }
                })
                .ok();
        }

        let engine = ChildEngine {
            inner,
            child: child.map(Mutex::new),
            report: Arc::clone(&report),
            connected: Arc::clone(&connected),
            events: Mutex::new(Some(event_rx)),
        };

        // Status is polled, so the UI can read it every frame for free.
        {
            let inner = Arc::clone(&engine.inner);
            std::thread::Builder::new()
                .name("engine-poll".into())
                .spawn(move || {
                    while inner.alive.load(Ordering::Relaxed) {
                        let frame = {
                            let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
                            let (tx, rx) = mpsc::channel();
                            inner.pending.lock().unwrap().insert(id, tx);
                            let request = Request {
                                id,
                                op: Op::Status,
                                payload: None,
                            };
                            let sent = serde_json::to_string(&request).ok().and_then(|line| {
                                let mut stdin = inner.stdin.lock().ok()?;
                                stdin
                                    .write_all(line.as_bytes())
                                    .and_then(|_| stdin.write_all(b"\n"))
                                    .and_then(|_| stdin.flush())
                                    .ok()
                            });
                            if sent.is_none() {
                                inner.pending.lock().unwrap().remove(&id);
                            }
                            rx.recv_timeout(IO_TIMEOUT).ok()
                        };
                        match frame.and_then(|r| r.into_data().ok()) {
                            Some(value) => match serde_json::from_value::<StatusReport>(value) {
                                Ok(fresh) => {
                                    *report.write().unwrap() = Some(fresh);
                                    connected.store(true, Ordering::Relaxed);
                                }
                                Err(_) => connected.store(false, Ordering::Relaxed),
                            },
                            None => connected.store(false, Ordering::Relaxed),
                        }
                        if !inner.alive.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                })
                .ok();
        }

        engine
    }

    /// Take the receiver for host-pushed events. Only the first call returns one.
    pub fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events.lock().ok().and_then(|mut e| e.take())
    }

    /// The child's process id, when this client spawned one.
    pub fn pid(&self) -> Option<u32> {
        self.child
            .as_ref()
            .and_then(|c| c.lock().ok().map(|c| c.id()))
    }

    /// True if the last status poll succeeded.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// One request/response round trip.
    fn call(
        &self,
        op: Op,
        payload: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);

        let request = Request { id, op, payload };
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let sent = {
            let mut stdin = self
                .inner
                .stdin
                .lock()
                .map_err(|_| "engine stdin poisoned")?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
        };
        if let Err(e) = sent {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(format!("cannot send {op:?} to the engine: {e}"));
        }

        match rx.recv_timeout(IO_TIMEOUT) {
            Ok(response) => response.into_data(),
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err(format!("the engine did not answer {op:?}"))
            }
        }
    }

    fn typed<T: serde::de::DeserializeOwned>(
        &self,
        op: Op,
        payload: Option<serde_json::Value>,
    ) -> Result<T, String> {
        let value = self.call(op, payload)?;
        serde_json::from_value(value).map_err(|e| format!("bad {op:?} reply: {e}"))
    }

    pub fn health(&self) -> Result<Health, String> {
        self.typed(Op::Health, None)
    }

    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        let payload = serde_json::to_value(q).map_err(|e| e.to_string())?;
        let dto: SearchResponseDto = self.typed(Op::Search, Some(payload))?;
        Ok(dto.into_response())
    }

    pub fn count(&self, q: &Query) -> Result<u64, String> {
        let payload = serde_json::to_value(q).map_err(|e| e.to_string())?;
        let dto: CountDto = self.typed(Op::Count, Some(payload))?;
        Ok(dto.count)
    }

    pub fn rebuild(&self) -> Result<(), String> {
        self.call(Op::Rebuild, None).map(|_| ())
    }

    /// Ask the engine to return freed heap pages to the OS.
    pub fn trim_memory(&self) -> Result<(), String> {
        self.call(Op::Trim, None).map(|_| ())
    }

    /// Ask the engine to stop (it exits; the owner then reaps it).
    pub fn shutdown(&self) -> Result<(), String> {
        self.call(Op::Shutdown, None).map(|_| ())
    }

    fn patch(&self, patch: ConfigPatch) -> Result<(), String> {
        let payload = serde_json::to_value(patch).map_err(|e| e.to_string())?;
        self.call(Op::Config, Some(payload)).map(|_| ())
    }

    /// Turn ignore-file handling on/off in the engine (rebuilding its index).
    pub fn set_respect_ignore(&self, on: bool) -> Result<(), String> {
        self.patch(ConfigPatch {
            respect: Some(on),
            rebuild: true,
            ..Default::default()
        })
    }

    /// Turn symlink following on/off in the engine (rebuilding its index).
    pub fn set_follow_symlinks(&self, on: bool) -> Result<(), String> {
        self.patch(ConfigPatch {
            follow_symlinks: Some(on),
            rebuild: true,
            ..Default::default()
        })
    }

    /// Replace the engine's excluded-directory list (rebuilding its index).
    pub fn set_exclude_dirs(&self, dirs: Vec<String>) -> Result<(), String> {
        self.patch(ConfigPatch {
            exclude_dirs: Some(dirs),
            rebuild: true,
            ..Default::default()
        })
    }

    /// Turn the engine's background content cache on/off (no rebuild needed).
    pub fn set_content_index(&self, on: bool) -> Result<(), String> {
        self.patch(ConfigPatch {
            content_index: Some(on),
            ..Default::default()
        })
    }

    /// Pause or resume live indexing in the engine.
    pub fn set_paused(&self, on: bool) -> Result<(), String> {
        self.patch(ConfigPatch {
            paused: Some(on),
            ..Default::default()
        })
    }

    /// Replace the engine's index roots (rebuilding its index).
    pub fn set_roots(&self, roots: Vec<String>) -> Result<(), String> {
        self.patch(ConfigPatch {
            roots: Some(roots),
            rebuild: true,
            ..Default::default()
        })
    }

    /// Last polled status report (`None` until the first successful poll).
    pub fn report(&self) -> Option<StatusReport> {
        self.report.read().ok().and_then(|r| r.clone())
    }

    pub fn status(&self) -> Status {
        self.report().map(|r| r.status).unwrap_or_default()
    }

    /// Block until the engine's index is live (bounded).
    pub fn wait_live(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.status().state == State::Live {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

impl Drop for ChildEngine {
    /// When this client spawned the engine, leaving kills it, so a crashed or
    /// closed app never leaves an orphaned indexer behind. A client that adopted
    /// its own stdio has no child to kill.
    fn drop(&mut self) {
        self.inner.alive.store(false, Ordering::Relaxed);
        if let Some(child) = &self.child
            && let Ok(mut child) = child.lock()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
