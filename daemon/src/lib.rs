//! `easysearch-daemon` — owns the search engine and serves it over a
//! localhost HTTP/JSON API, so the GUI (and any other client: scripts, another
//! language, curl) is just a consumer. See `docs/api.md`.
//!
//! Endpoints:
//! ```text
//! GET  /v1/health   → {"ok":true,"version":..,"api":1,"uptime_secs":..}
//! GET  /v1/status   → {"status":{..},"files":..,"dirs":..}
//! POST /v1/search   → Query (JSON) → SearchResponse (JSON)
//! GET  /v1/search?query=..&regex=1&content=..&limit=..&category=..&under=..
//!                  &ext=pdf,md&min_size=..&max_size=..&modified_within=..
//! POST /v1/count    → Query (JSON) → {"count":n}   (count only, no rows)
//! POST /v1/rebuild  → {"ok":true}
//! POST /v1/ignore   → {"respect":bool,"follow_symlinks":bool,"content_index":bool,"rebuild":bool} → {"ok":true}
//! POST /v1/config   → alias of /v1/ignore (live index settings)
//! POST /v1/shutdown → {"ok":true}, then the daemon stops (and exits)
//! GET  /v1/watch?timeout=25  → long-poll: same payload as /v1/status,
//!                              returned when it changes (or on timeout)
//! ```
//!
//! Bind address defaults to `127.0.0.1:5858` — the API is **localhost only**;
//! it has no authentication, so do not expose it to a network.
//!
//! Change notification is long-polling rather than SSE/WebSocket: it works with
//! every HTTP client (curl, any language), needs no framing layer, and — unlike
//! server-sent events — is not defeated by `tiny_http`'s chunk encoder, which
//! buffers small writes so short SSE frames are never flushed.

use easysearch_core::api::{
    API_VERSION, CountDto, ErrorDto, Health, SearchResponseDto, StatusReport, category_from_str,
};
use easysearch_core::remote::percent_decode;
use easysearch_core::{Engine, Query};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Cap on concurrently served requests (each request holds one thread).
const MAX_CONCURRENCY: usize = 32;

pub struct Daemon {
    engine: Arc<Engine>,
    started: Instant,
    version: String,
    inflight: AtomicUsize,
    /// Set by [`Daemon::serve_forever`]; lets `POST /v1/shutdown` stop the
    /// accept loop (used to restart onto a freshly installed binary).
    server: OnceLock<Arc<Server>>,
}

impl Daemon {
    pub fn new(engine: Arc<Engine>) -> Arc<Daemon> {
        Arc::new(Daemon {
            engine,
            started: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            inflight: AtomicUsize::new(0),
            server: OnceLock::new(),
        })
    }

    /// Accept connections until the server stops, one thread per request.
    pub fn serve_forever(self: Arc<Self>, server: Arc<Server>) {
        let _ = self.server.set(Arc::clone(&server));
        let mut handles: Vec<std::thread::JoinHandle<()>> = Vec::new();
        for request in server.incoming_requests() {
            while self.inflight.load(Ordering::Relaxed) >= MAX_CONCURRENCY {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.inflight.fetch_add(1, Ordering::Relaxed);
            let daemon = Arc::clone(&self);
            match std::thread::Builder::new()
                .name("http".into())
                .spawn(move || {
                    daemon.handle(request);
                    daemon.inflight.fetch_sub(1, Ordering::Relaxed);
                }) {
                Ok(h) => handles.push(h),
                Err(e) => {
                    eprintln!("daemon: cannot spawn handler thread: {e}");
                    self.inflight.fetch_sub(1, Ordering::Relaxed);
                }
            }
            handles.retain(|h| !h.is_finished());
        }
    }

    fn handle(&self, mut request: Request) {
        let method = request.method().clone();
        let url = request.url().to_string();
        let (path, params) = match url.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (url, String::new()),
        };

        // Streaming responses are not used (see the module docs), so every
        // route answers with a complete JSON body.
        let (code, body) = match (&method, path.as_str()) {
            (Method::Get, "/v1/health") => (200, self.health_json()),
            (Method::Get, "/v1/status") => (200, self.status_json()),
            (Method::Get, "/v1/watch") => {
                let timeout = param(&params, "timeout")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(25)
                    .min(120);
                (200, self.watch_json(Duration::from_secs(timeout)))
            }
            (Method::Get, "/v1/search") => match parse_query_params(&params) {
                Ok(q) => self.search_json(&q),
                Err(e) => (400, err_json(&e)),
            },
            (Method::Post, "/v1/search") | (Method::Post, "/v1/count") => {
                let count_only = path == "/v1/count";
                let mut body = String::new();
                match request.as_reader().read_to_string(&mut body) {
                    Ok(_) => match serde_json::from_str::<Query>(&body) {
                        Ok(q) if count_only => match self.engine.count(&q) {
                            Ok(count) => (
                                200,
                                serde_json::to_string(&CountDto { count }).unwrap_or_default(),
                            ),
                            Err(e) => (400, err_json(&e)),
                        },
                        Ok(q) => self.search_json(&q),
                        Err(e) => (400, err_json(&format!("invalid query JSON: {e}"))),
                    },
                    Err(e) => (400, err_json(&format!("cannot read body: {e}"))),
                }
            }
            (Method::Post, "/v1/rebuild") => {
                self.engine.rebuild();
                (200, r#"{"ok":true}"#.to_string())
            }
            (Method::Post, "/v1/shutdown") => {
                self.request_shutdown();
                (200, r#"{"ok":true}"#.to_string())
            }
            (Method::Post, "/v1/ignore") | (Method::Post, "/v1/config") => {
                let mut body = String::new();
                match request.as_reader().read_to_string(&mut body) {
                    Ok(_) => {
                        let value: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                        // Each field is optional: only what is present is changed,
                        // and the rebuild happens once at the end.
                        let respect = value
                            .get("respect")
                            .or_else(|| value.get("respect_ignore_files"))
                            .and_then(|v| v.as_bool());
                        let follow = value.get("follow_symlinks").and_then(|v| v.as_bool());
                        let content_index = value.get("content_index").and_then(|v| v.as_bool());
                        let rebuild = value
                            .get("rebuild")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        if let Some(on) = respect {
                            self.engine.set_respect_ignore(on, false);
                        }
                        if let Some(on) = follow {
                            self.engine.set_follow_symlinks(on, false);
                        }
                        if let Some(on) = content_index {
                            self.engine.set_content_index(on);
                        }
                        if (respect.is_some() || follow.is_some()) && rebuild {
                            self.engine.rebuild();
                        }
                        (200, r#"{"ok":true}"#.to_string())
                    }
                    Err(e) => (400, err_json(&format!("cannot read body: {e}"))),
                }
            }
            (Method::Get, "/") => (200, self.health_json()),
            _ => (404, err_json(&format!("no route for {method} {path}"))),
        };
        let response = Response::from_data(body.into_bytes())
            .with_status_code(StatusCode(code))
            .with_header(header("Content-Type", "application/json; charset=utf-8"));
        if let Err(e) = request.respond(response) {
            eprintln!("daemon: failed to send response: {e}");
        }
    }

    /// Stop serving (after the response has gone out) so `run_forever` returns
    /// and the process exits. Used by an in-place update to drop the old binary.
    fn request_shutdown(&self) {
        if let Some(server) = self.server.get() {
            let server = Arc::clone(server);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                server.unblock();
            });
        }
    }

    fn health_json(&self) -> String {
        let h = Health {
            ok: true,
            version: self.version.clone(),
            api: API_VERSION,
            uptime_secs: self.started.elapsed().as_secs(),
        };
        serde_json::to_string(&h).unwrap_or_default()
    }

    fn status_json(&self) -> String {
        let (files, dirs) = self.engine.counts();
        let report = StatusReport {
            status: self.engine.status_snapshot(),
            files,
            dirs,
        };
        serde_json::to_string(&report).unwrap_or_default()
    }

    fn search_json(&self, q: &Query) -> (u16, String) {
        match self.engine.search(q) {
            Ok(resp) => (
                200,
                serde_json::to_string(&SearchResponseDto::from_response(resp)).unwrap_or_default(),
            ),
            Err(e) => (400, err_json(&e)),
        }
    }

    /// Long-poll: return the status report as soon as it differs from the one
    /// seen at request time, or after `timeout` at the latest.
    fn watch_json(&self, timeout: Duration) -> String {
        let start = self.status_json();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            let now = self.status_json();
            if now != start {
                return now;
            }
        }
        start
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

/// Bind `addr` and serve requests until the process exits.
///
/// Shared by the `easysearch-daemon` binary and the combined single-binary app
/// (which re-executes itself in daemon mode).
pub fn run_forever(engine: Arc<Engine>, addr: &str) -> Result<(), String> {
    let server = Server::http(addr).map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| addr.to_string());
    eprintln!(
        "easysearch-daemon {} — listening on http://{bound}",
        env!("CARGO_PKG_VERSION")
    );
    Daemon::new(engine).serve_forever(Arc::new(server));
    Ok(())
}

fn err_json(msg: &str) -> String {
    serde_json::to_string(&ErrorDto::new(msg)).unwrap_or_default()
}

fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Value of one query parameter (percent-decoded), if present.
fn param(params: &str, key: &str) -> Option<String> {
    params.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// Build a [`Query`] from `GET /v1/search` query parameters.
fn parse_query_params(params: &str) -> Result<Query, String> {
    let mut q = Query::default();
    for pair in params.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, percent_decode(v)),
            None => (pair, String::new()),
        };
        match key {
            "query" | "q" => q.name = value,
            "regex" => q.regex_mode = truthy(&value),
            "case" | "case_sensitive" => q.case_sensitive = truthy(&value),
            "content" => {
                q.content = if value.is_empty() { None } else { Some(value) };
            }
            "hidden" => q.include_hidden = truthy(&value),
            "fuzzy" => q.fuzzy = truthy(&value),
            "multiline" => q.multiline = truthy(&value),
            "path" | "full_path" => q.full_path = truthy(&value),
            "dirs" | "include_dirs" => q.include_dirs = truthy(&value),
            "under" => {
                q.under = if value.is_empty() { None } else { Some(value) };
            }
            // Comma-separated extensions; trim entries and ignore empties.
            // Canonicalisation (leading dot, case) happens in `CompiledQuery`.
            "ext" => {
                q.extensions = value
                    .split(',')
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect();
            }
            "min_size" => {
                q.min_size = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid min_size {value:?}"))?,
                );
            }
            "max_size" => {
                q.max_size = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid max_size {value:?}"))?,
                );
            }
            "modified_within" | "modified_within_secs" => {
                q.modified_within_secs = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid modified_within {value:?}"))?,
                );
            }
            "limit" => {
                q.limit = value
                    .parse()
                    .map_err(|_| format!("invalid limit {value:?}"))?;
            }
            "category" => {
                q.category = category_from_str(&value)
                    .ok_or_else(|| format!("unknown category {value:?}"))?;
            }
            other => return Err(format!("unknown parameter {other:?}")),
        }
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use easysearch_core::Category;

    #[test]
    fn parses_get_query_params() {
        let q =
            parse_query_params("query=*.pdf&regex=1&hidden=true&limit=5&category=docs").unwrap();
        assert_eq!(q.name, "*.pdf");
        assert!(q.regex_mode);
        assert!(q.include_hidden);
        assert_eq!(q.limit, 5);
        assert_eq!(q.category, Category::Docs);
    }

    #[test]
    fn decodes_percent_escapes() {
        let q = parse_query_params("query=annual%20report%20%2A.csv").unwrap();
        assert_eq!(q.name, "annual report *.csv");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_query_params("nope=1").is_err());
        assert!(parse_query_params("limit=abc").is_err());
        assert!(parse_query_params("category=weird").is_err());
    }

    #[test]
    fn parses_ext_and_size_and_recency_params() {
        let q = parse_query_params(
            "ext=pdf,%20docx,,md&min_size=1024&max_size=10485760&modified_within=3600",
        )
        .unwrap();
        assert_eq!(q.extensions, vec!["pdf", "docx", "md"]);
        assert_eq!(q.min_size, Some(1024));
        assert_eq!(q.max_size, Some(10 * 1024 * 1024));
        assert_eq!(q.modified_within_secs, Some(3600));

        // The `modified_within_secs` alias is accepted too.
        let q = parse_query_params("modified_within_secs=60").unwrap();
        assert_eq!(q.modified_within_secs, Some(60));

        assert!(parse_query_params("min_size=big").is_err());
        assert!(parse_query_params("modified_within=soon").is_err());
    }

    #[test]
    fn parses_the_fuzzy_flag() {
        assert!(
            !parse_query_params("query=x").unwrap().fuzzy,
            "off by default"
        );
        assert!(parse_query_params("query=x&fuzzy=1").unwrap().fuzzy);
        assert!(parse_query_params("query=x&fuzzy=true").unwrap().fuzzy);
        assert!(!parse_query_params("query=x&fuzzy=no").unwrap().fuzzy);
    }

    #[test]
    fn parses_the_multiline_flag() {
        assert!(
            !parse_query_params("query=x").unwrap().multiline,
            "off by default"
        );
        assert!(
            parse_query_params("query=x&content=a&multiline=1")
                .unwrap()
                .multiline
        );
        assert!(
            !parse_query_params("query=x&multiline=false")
                .unwrap()
                .multiline
        );
    }
}
