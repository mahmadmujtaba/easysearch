//! End-to-end: start the daemon on an ephemeral port and drive it with the core
//! HTTP client (the same path the GUI uses), plus raw requests to pin the wire
//! format that other clients depend on.

use everything_core::api::{Health, StatusReport};
use everything_core::remote::request;
use everything_core::{Backend, Config, Engine, Query, State};
use everything_daemon::Daemon;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A dedicated test directory under the system temp dir (not dot-prefixed, so
/// the default hidden-path filtering does not hide the fixtures).
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> TestDir {
        let dir =
            std::env::temp_dir().join(format!("everything-daemon-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Start a daemon over an isolated root; returns its base address.
fn start_daemon(root: &std::path::Path) -> String {
    let mut cfg = Config::default();
    cfg.roots = vec![root.to_string_lossy().into_owned()];
    cfg.disk_index_dir = Some(root.join(".cache-dir").to_string_lossy().into_owned());
    let mut engine = Engine::new(cfg);
    engine.start();

    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server
        .server_addr()
        .to_ip()
        .expect("daemon bound to an IP")
        .port();
    let daemon = Daemon::new(Arc::new(engine));
    std::thread::spawn(move || daemon.serve_forever(server));
    format!("127.0.0.1:{port}")
}

fn wait_live(backend: &Backend, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if backend.status_snapshot().state == State::Live {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Poll `/v1/status` until the daemon reports its index live.
fn wait_live_http(addr: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(body) = request(addr, "GET", "/v1/status", None) {
            if let Ok(report) = serde_json::from_slice::<StatusReport>(&body) {
                if report.status.state == State::Live {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn remote_backend_searches_over_http() {
    let dir = TestDir::new("search");
    let root = dir.0.clone();
    std::fs::write(root.join("alpha.txt"), "hello").unwrap();
    std::fs::create_dir_all(root.join("folder")).unwrap();

    let addr = start_daemon(&root);
    let backend = Backend::remote(&addr);
    assert!(backend.is_remote());

    assert!(
        wait_live(&backend, Duration::from_secs(30)),
        "daemon not live"
    );

    let resp = backend
        .search(&Query {
            name: "*.txt".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("alpha.txt")));

    // Counts and connectivity come from the polled status.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !backend.connected() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        backend.connected(),
        "daemon should be reported as connected"
    );
    let (files, dirs) = backend.counts();
    assert!(files >= 1, "expected at least one file, got {files}");
    assert!(dirs >= 1, "expected at least one folder, got {dirs}");
}

#[test]
fn rest_api_shapes_are_stable() {
    let dir = TestDir::new("rest");
    let root = dir.0.clone();
    std::fs::write(root.join("report.csv"), "a,b").unwrap();

    let addr = start_daemon(&root);

    // Health
    let body = request(&addr, "GET", "/v1/health", None).unwrap();
    let health: Health = serde_json::from_slice(&body).unwrap();
    assert!(health.ok);
    assert_eq!(health.api, everything_core::api::API_VERSION);

    // Status
    assert!(
        wait_live_http(&addr, Duration::from_secs(30)),
        "daemon index never became live"
    );
    let body = request(&addr, "GET", "/v1/status", None).unwrap();
    let report: StatusReport = serde_json::from_slice(&body).unwrap();
    assert!(report.files >= 1);

    // GET search with percent-encoded pattern
    let body = request(&addr, "GET", "/v1/search?query=*.csv&limit=10", None).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let results = json["results"].as_array().expect("results array");
    assert!(results
        .iter()
        .any(|r| r["path"].as_str().unwrap_or("").ends_with("report.csv")));

    // POST search
    let q = serde_json::to_vec(&Query {
        name: "report".into(),
        ..Query::default()
    })
    .unwrap();
    let body = request(&addr, "POST", "/v1/search", Some(&q)).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["results"].as_array().unwrap().is_empty());

    // Unknown route → JSON error
    let err = request(&addr, "GET", "/v1/nope", None).unwrap_err();
    assert!(err.contains("no route"), "unexpected error: {err}");

    // Rebuild
    request(&addr, "POST", "/v1/rebuild", Some(b"{}")).unwrap();

    // Long-poll: returns a valid status report (immediately on change, or after
    // the timeout).
    let body = request(&addr, "GET", "/v1/watch?timeout=1", None).unwrap();
    let report: StatusReport = serde_json::from_slice(&body).unwrap();
    assert_eq!(report.status.state, State::Live);
}
