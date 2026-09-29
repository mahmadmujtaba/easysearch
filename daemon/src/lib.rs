//! `easysearch-daemon` — the engine **server**.
//!
//! It owns the index and answers requests as JSON frames ([`easysearch_core::proto`]).
//! There is deliberately **no socket and no HTTP**: the only way to reach it is
//! the pipe its peer holds, so the index is not exposed to anything else on the
//! machine.
//!
//! Two callers use it:
//! - the standalone `easysearch-daemon` binary and `easysearch --engine`, on
//!   stdin/stdout ([`serve`]); and
//! - the combined app's background host, which serves the window child's pipes
//!   ([`serve_reader`]) and pushes [`Event`]s to it.

use easysearch_core::api::{API_VERSION, CountDto, Health, SearchResponseDto, StatusReport};
use easysearch_core::proto::{ConfigPatch, Event, Op, Request, Response};
use easysearch_core::{Engine, Query};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Cap on concurrently served requests (each request holds one thread). A slow
/// content search must not block the status poller or the next query.
const MAX_CONCURRENCY: usize = 32;

/// A writer shared by the reply path and the host's event path.
pub type Out = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct Daemon {
    engine: Arc<Engine>,
    started: Instant,
}

impl Daemon {
    pub fn new(engine: Arc<Engine>) -> Arc<Daemon> {
        Arc::new(Daemon {
            engine,
            started: Instant::now(),
        })
    }

    /// The engine this daemon serves (the host uses it, e.g. to shut it down).
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Answer one request. Never fails: an error becomes an `error` reply.
    pub fn dispatch(&self, request: Request) -> Response {
        let id = request.id;
        match request.op {
            Op::Health => as_reply(id, &self.health()),
            Op::Status => as_reply(id, &self.status()),
            Op::Search => match payload::<Query>(request.payload) {
                Ok(q) => match self.engine.search(&q) {
                    Ok(response) => as_reply(id, &SearchResponseDto::from_response(response)),
                    Err(e) => Response::failed(id, e),
                },
                Err(e) => Response::failed(id, e),
            },
            Op::Count => match payload::<Query>(request.payload) {
                Ok(q) => match self.engine.count(&q) {
                    Ok(count) => as_reply(id, &CountDto { count }),
                    Err(e) => Response::failed(id, e),
                },
                Err(e) => Response::failed(id, e),
            },
            Op::Rebuild => {
                self.engine.rebuild();
                Response::done(id)
            }
            Op::Config => match payload::<ConfigPatch>(request.payload) {
                Ok(patch) => {
                    // Each field is optional: only what is present changes, and a
                    // walk-setting change rebuilds once at the end.
                    if let Some(on) = patch.respect {
                        self.engine.set_respect_ignore(on, false);
                    }
                    if let Some(on) = patch.follow_symlinks {
                        self.engine.set_follow_symlinks(on, false);
                    }
                    if let Some(on) = patch.content_index {
                        self.engine.set_content_index(on);
                    }
                    if let Some(on) = patch.paused {
                        self.engine.set_paused(on);
                    }
                    let walk_changed = patch.respect.is_some()
                        || patch.follow_symlinks.is_some()
                        || patch.exclude_dirs.is_some();
                    if let Some(dirs) = patch.exclude_dirs {
                        self.engine.set_exclude_dirs(dirs, false);
                    }
                    if walk_changed && patch.rebuild {
                        self.engine.rebuild();
                    }
                    Response::done(id)
                }
                Err(e) => Response::failed(id, e),
            },
            Op::Shutdown => Response::done(id),
            Op::Trim => {
                self.engine.trim_memory();
                Response::done(id)
            }
        }
    }

    fn health(&self) -> Health {
        Health {
            ok: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            api: API_VERSION,
            uptime_secs: self.started.elapsed().as_secs(),
        }
    }

    fn status(&self) -> StatusReport {
        let (files, dirs) = self.engine.counts();
        StatusReport {
            status: self.engine.status_snapshot(),
            files,
            dirs,
        }
    }
}

fn as_reply<T: Serialize>(id: u64, value: &T) -> Response {
    match serde_json::to_value(value) {
        Ok(data) => Response::ok(id, data),
        Err(e) => Response::failed(id, e.to_string()),
    }
}

fn payload<T: DeserializeOwned>(value: Option<serde_json::Value>) -> Result<T, String> {
    let value = value.ok_or("this request needs a payload")?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Write one framed value as a line, whole (never interleaved with another
/// thread's): a reply, or an [`Event`].
pub fn write_value(out: &Out, value: &impl Serialize) {
    if let Ok(mut out) = out.lock() {
        write_line(&mut *out, value);
    }
}

/// Write one line into any writer (the seam `write_value` and the tests use).
fn write_line(into: &mut impl Write, value: &impl Serialize) {
    let Ok(line) = serde_json::to_string(value) else {
        return;
    };
    let _ = into
        .write_all(line.as_bytes())
        .and_then(|_| into.write_all(b"\n"))
        .and_then(|_| into.flush());
}

/// Push an unsolicited event to the window on the other end of `out`.
pub fn write_event(out: &Out, event: &Event) {
    write_value(out, event);
}

/// Read request frames from `reader` and answer on `out` until it ends.
///
/// Each request is answered on its own thread, so one long search does not
/// serialize the others. `out` is shared with the caller so it can interleave
/// events between replies (each frame is a whole, mutex-guarded line).
pub fn serve_reader(
    daemon: Arc<Daemon>,
    reader: impl Read + Send + 'static,
    out: Out,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("engine-serve".into())
        .spawn(move || {
            let inflight = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let mut handles: Vec<JoinHandle<()>> = Vec::new();

            for line in BufReader::new(reader).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break, // the other end went away
                };
                if line.trim().is_empty() {
                    continue;
                }
                let request = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => request,
                    Err(e) => {
                        // No id to answer: report against 0 so the peer can log it.
                        write_value(&out, &Response::failed(0, format!("bad request: {e}")));
                        continue;
                    }
                };

                while inflight.load(Ordering::Relaxed) >= MAX_CONCURRENCY {
                    std::thread::sleep(Duration::from_millis(5));
                }
                inflight.fetch_add(1, Ordering::Relaxed);

                let daemon = Arc::clone(&daemon);
                let out = Arc::clone(&out);
                let task_inflight = Arc::clone(&inflight);
                let task_stop = Arc::clone(&stop);
                match std::thread::Builder::new()
                    .name("engine-req".into())
                    .spawn(move || {
                        let shutdown = request.op == Op::Shutdown;
                        let response = daemon.dispatch(request);
                        write_value(&out, &response);
                        task_inflight.fetch_sub(1, Ordering::Relaxed);
                        if shutdown {
                            task_stop.store(true, Ordering::Relaxed);
                        }
                    }) {
                    Ok(handle) => handles.push(handle),
                    Err(_) => {
                        inflight.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                handles.retain(|h| !h.is_finished());

                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }

            // Let any in-flight reply reach the peer before the thread ends.
            for handle in handles {
                let _ = handle.join();
            }
        })
        .expect("spawn engine-serve thread")
}

/// Serve the engine on this process's stdin/stdout (the `--engine` mode).
pub fn serve(engine: Arc<Engine>) -> Result<(), String> {
    let daemon = Daemon::new(engine);
    let out: Out = Arc::new(Mutex::new(Box::new(std::io::stdout())));
    let handle = serve_reader(daemon, std::io::stdin(), out);
    let _ = handle.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> Arc<Daemon> {
        // A config pointing at an empty temp dir: the walker has nothing to do.
        let dir =
            std::env::temp_dir().join(format!("easysearch-daemon-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let config = easysearch_core::Config {
            roots: vec![dir.to_string_lossy().into_owned()],
            persist_index: false,
            ..easysearch_core::Config::default()
        };
        let mut engine = Engine::new(config);
        engine.start();
        Daemon::new(Arc::new(engine))
    }

    #[test]
    fn health_reports_the_protocol_version() {
        let reply = daemon().dispatch(Request {
            id: 4,
            op: Op::Health,
            payload: None,
        });
        let health: Health = serde_json::from_value(reply.into_data().unwrap()).unwrap();
        assert!(health.ok);
        assert_eq!(health.api, API_VERSION);
    }

    #[test]
    fn a_search_without_a_payload_is_an_error_reply() {
        let reply = daemon().dispatch(Request {
            id: 9,
            op: Op::Search,
            payload: None,
        });
        assert_eq!(reply.id, 9);
        assert!(reply.error.is_some(), "a malformed request must not panic");
    }

    #[test]
    fn a_config_patch_needs_no_rebuild_to_answer() {
        let reply = daemon().dispatch(Request {
            id: 11,
            op: Op::Config,
            payload: Some(serde_json::json!({"content_index": false})),
        });
        assert!(reply.into_data().is_ok());
    }

    #[test]
    fn shutdown_is_a_clean_no_content_reply() {
        let reply = daemon().dispatch(Request {
            id: 12,
            op: Op::Shutdown,
            payload: None,
        });
        assert!(reply.error.is_none());
        assert!(reply.data.is_none());
    }

    #[test]
    fn a_reader_serves_frames_and_events_share_the_writer() {
        let d = daemon();
        let mut buf: Vec<u8> = Vec::new();
        write_line(&mut buf, &Event::Show);
        write_line(&mut buf, &Response::done(3));
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], r#"{"event":"show"}"#);
        assert_eq!(lines[1], r#"{"id":3}"#);

        // And a real frame round-trips through the reader into a sink.
        let input: &'static [u8] = b"{\"id\":1,\"op\":\"health\"}\n";
        let lines = Arc::new(Mutex::new(0usize));
        let sink: Out = Arc::new(Mutex::new(Box::new(Sink(Arc::clone(&lines)))));
        serve_reader(Arc::clone(&d), input, Arc::clone(&sink))
            .join()
            .unwrap();
        assert!(*lines.lock().unwrap() >= 1);
    }

    /// A `Write` sink that only counts the newlines written to it.
    struct Sink(Arc<Mutex<usize>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            *self.0.lock().unwrap() += buf.iter().filter(|b| **b == b'\n').count();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
