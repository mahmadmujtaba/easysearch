//! The single `everything-linux` binary must be able to act as both GUI and
//! daemon. This exercises the daemon half plus the auto-start path the GUI uses
//! (`ensure_daemon_with`), without needing a display.
//!
//! Kept as one test because the child inherits the parent's `XDG_*` environment.

use std::path::PathBuf;
use std::time::{Duration, Instant};

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> TestDir {
        let dir = std::env::temp_dir().join(format!("everything-app-{tag}-{}", std::process::id()));
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

fn free_addr() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("127.0.0.1:{port}")
}

#[test]
fn addr_flag_is_parsed() {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        everything_app::addr_from_args(&args(&["--daemon", "--addr", "127.0.0.1:1234"])),
        Some("127.0.0.1:1234".to_string())
    );
    assert_eq!(
        everything_app::addr_from_args(&args(&["--addr=0.0.0.0:9"])),
        Some("0.0.0.0:9".to_string())
    );
    assert_eq!(everything_app::addr_from_args(&args(&["--daemon"])), None);
}

#[test]
fn single_binary_starts_a_daemon_and_serves_search() {
    let dir = TestDir::new("daemon");
    let root = dir.0.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("needle.txt"), "hello").unwrap();

    // Isolated config + cache so the child only indexes our tiny root (and does
    // not touch the developer's home or real cache).
    let cfg = dir.0.join("cfg");
    std::fs::create_dir_all(cfg.join("everything-linux")).unwrap();
    std::fs::write(
        cfg.join("everything-linux").join("config.json"),
        format!(r#"{{"roots":["{}"]}}"#, root.display()),
    )
    .unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &cfg);
    std::env::set_var("XDG_CACHE_HOME", dir.0.join("cache"));

    let addr = free_addr();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_everything-linux"));

    // The binary should re-exec itself in daemon mode.
    let mut child = match everything_app::ensure_daemon_with(&addr, &exe) {
        everything_app::Ensured::Started(child) => child,
        other => panic!("expected a freshly started daemon, got {other:?}"),
    };

    // Poll until the fixture is indexed and searchable.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found = false;
    while Instant::now() < deadline {
        if let Ok(body) =
            everything_core::remote::request(&addr, "GET", "/v1/search?query=needle*&limit=5", None)
        {
            if String::from_utf8_lossy(&body).contains("needle.txt") {
                found = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // A second call must recognise the running daemon rather than spawn another.
    let second = everything_app::ensure_daemon_with(&addr, &exe);

    let _ = child.kill();
    let _ = child.wait();

    assert!(found, "daemon never returned the fixture file");
    assert!(
        matches!(second, everything_app::Ensured::AlreadyUp),
        "expected AlreadyUp, got {second:?}"
    );
}
