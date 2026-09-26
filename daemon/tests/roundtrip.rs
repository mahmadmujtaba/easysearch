//! End-to-end: spawn the engine as a **child process** and drive it over its
//! pipes — the same path the app uses — plus one raw frame exchange that pins the
//! wire format independently of the client.
//!
//! The child's stderr is swallowed, which is what keeps protocol frames and human
//! log lines apart.
//!
//! **Pending.** Every test here spawns a real engine and waits for it to index, so
//! the suite is slow; it is `#[ignore]`d by default. Run it on demand with
//! `cargo test -p easysearch-daemon --test roundtrip -- --ignored`.

use easysearch_core::api::{Health, StatusReport};
use easysearch_core::proto::{Op, Request, Response};
use easysearch_core::{ChildEngine, ContentIndexStatus, Query, State};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A dedicated test directory under the system temp dir (not dot-prefixed, so the
/// default hidden-path filtering does not hide the fixtures).
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> TestDir {
        let dir = std::env::temp_dir().join(format!(
            "easysearch-engine-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
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

/// An isolated config + cache for the child, so it only indexes the fixture root
/// and never touches the developer's home or real cache.
fn isolated_env(dir: &Path, root: &Path) -> Vec<(String, String)> {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("easysearch")).unwrap();
    std::fs::write(
        cfg.join("easysearch").join("config.json"),
        format!(r#"{{"roots":["{}"]}}"#, root.display()),
    )
    .unwrap();
    vec![
        (
            "XDG_CONFIG_HOME".to_string(),
            cfg.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            dir.join("cache").to_string_lossy().into_owned(),
        ),
    ]
}

/// A running engine child over a fixture tree.
struct Fixture {
    _dir: TestDir,
    child: ChildEngine,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let dir = TestDir::new(tag);
        let root = dir.0.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), "hello").unwrap();
        std::fs::create_dir_all(root.join("folder")).unwrap();

        let envs = isolated_env(&dir.0, &root);
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_easysearch-daemon"));
        let child = ChildEngine::spawn(&exe, &[], &envs, None).expect("spawn the engine");
        Fixture { _dir: dir, child }
    }

    /// Poll the engine's status until `check` is satisfied.
    fn wait_for(&self, what: &str, check: impl Fn(&StatusReport) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Some(report) = self.child.report()
                && check(&report)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Explicit: the engine exits when we say so (and again on drop, harmlessly).
        let _ = self.child.shutdown();
    }
}

#[test]
#[ignore = "pending: spawns a real engine and waits to index; run with --ignored"]
fn the_engine_serves_searches_as_a_child() {
    let fixture = Fixture::new("search");
    assert!(
        fixture.child.wait_live(Duration::from_secs(30)),
        "the engine never became live"
    );

    let response = fixture
        .child
        .search(&Query {
            name: "*.txt".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(
        response
            .results
            .iter()
            .any(|r| r.path.ends_with("alpha.txt")),
        "expected alpha.txt in {:?}",
        response.results
    );

    // Counts and connectivity come from the polled status.
    fixture.wait_for("a connected status", |_| fixture.child.connected());
    let report = fixture.child.report().expect("a status report");
    assert!(report.files >= 1, "expected at least one file");
    assert!(report.dirs >= 1, "expected at least one folder");

    let health = fixture.child.health().unwrap();
    assert!(health.ok);
    assert_eq!(health.api, easysearch_core::api::API_VERSION);
}

#[test]
#[ignore = "pending: spawns a real engine and waits to index; run with --ignored"]
fn walk_settings_round_trip_through_the_child() {
    let fixture = Fixture::new("settings");
    fixture.wait_for("a live index", |r| r.status.state == State::Live);
    assert!(fixture.wait_for("the default settings", |r| {
        r.status.respect_ignore_files && !r.status.follow_symlinks
    }));

    fixture.child.set_respect_ignore(false).unwrap();
    fixture.wait_for("ignore handling off", |r| !r.status.respect_ignore_files);

    fixture.child.set_follow_symlinks(true).unwrap();
    fixture.wait_for("symlink following on", |r| r.status.follow_symlinks);

    // Reversible, and changing one leaves the other alone.
    fixture.child.set_respect_ignore(true).unwrap();
    fixture.wait_for("ignore handling back on", |r| {
        r.status.respect_ignore_files && r.status.follow_symlinks
    });
    fixture.child.set_follow_symlinks(false).unwrap();
    fixture.wait_for("symlink following off", |r| {
        r.status.respect_ignore_files && !r.status.follow_symlinks
    });
}

#[test]
#[ignore = "pending: spawns a real engine and waits to index; run with --ignored"]
fn the_content_cache_switch_round_trips() {
    let fixture = Fixture::new("cindex");
    fixture.wait_for("a live index", |r| r.status.state == State::Live);
    assert!(
        matches!(
            fixture.child.report().unwrap().status.content_index,
            ContentIndexStatus::Disabled
        ),
        "the background content cache is off by default"
    );

    fixture.child.set_content_index(true).unwrap();
    fixture.wait_for("the content cache on", |r| {
        !matches!(r.status.content_index, ContentIndexStatus::Disabled)
    });

    fixture.child.set_content_index(false).unwrap();
    fixture.wait_for("the content cache off", |r| {
        matches!(r.status.content_index, ContentIndexStatus::Disabled)
    });
}

#[test]
#[ignore = "pending: spawns a real engine and waits to index; run with --ignored"]
fn frames_are_newline_delimited_json() {
    // Drive the binary with no client at all, to pin the protocol other tools
    // would have to speak.
    let dir = TestDir::new("raw");
    let root = dir.0.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("report.csv"), "a,b").unwrap();
    let envs = isolated_env(&dir.0, &root);

    let exe = PathBuf::from(env!("CARGO_BIN_EXE_easysearch-daemon"));
    let mut command = Command::new(exe);
    command
        .arg("--quiet")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in &envs {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn the engine");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut next_id = 1u64;

    let health_line = serde_json::to_string(&Request {
        id: next_id,
        op: Op::Health,
        payload: None,
    })
    .unwrap();
    stdin.write_all(health_line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let health_reply: Response = serde_json::from_str(&line).expect("one JSON object per line");
    assert_eq!(health_reply.id, next_id);
    let health: Health = serde_json::from_value(health_reply.into_data().unwrap()).unwrap();
    assert!(health.ok);

    // A search needs a live index: poll `status` over the same pipe first.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        next_id += 1;
        let status_line = serde_json::to_string(&Request {
            id: next_id,
            op: Op::Status,
            payload: None,
        })
        .unwrap();
        stdin.write_all(status_line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let reply: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(reply.id, next_id, "replies come back in the order asked");
        let report: StatusReport = serde_json::from_value(reply.into_data().unwrap()).unwrap();
        if report.status.state == State::Live {
            break;
        }
        assert!(Instant::now() < deadline, "the index never became live");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A search arrives as a structured payload, not as query parameters.
    next_id += 1;
    let search_id = next_id;
    let search_line = serde_json::to_string(&Request {
        id: search_id,
        op: Op::Search,
        payload: Some(
            serde_json::to_value(Query {
                name: "report".into(),
                ..Query::default()
            })
            .unwrap(),
        ),
    })
    .unwrap();
    stdin.write_all(search_line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let reply: Response = serde_json::from_str(&line).unwrap();
    assert_eq!(reply.id, search_id);
    let data = reply.into_data().unwrap();
    assert!(
        data["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"].as_str().unwrap_or("").ends_with("report.csv")),
        "unexpected search reply: {data}"
    );

    // An unknown op is a parse failure answered on id 0, not a crash.
    stdin.write_all(b"{\"id\":99,\"op\":\"nope\"}\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let reply: Response = serde_json::from_str(&line).unwrap();
    assert_eq!(reply.id, 0);
    assert!(reply.error.is_some());

    // Shutdown ends the process.
    next_id += 1;
    let shutdown_id = next_id;
    let shutdown_line = serde_json::to_string(&Request {
        id: shutdown_id,
        op: Op::Shutdown,
        payload: None,
    })
    .unwrap();
    stdin.write_all(shutdown_line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let reply: Response = serde_json::from_str(&line).unwrap();
    assert_eq!(reply.id, shutdown_id);
    let _ = child.wait();
}
