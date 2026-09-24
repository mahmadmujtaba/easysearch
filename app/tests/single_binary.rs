//! The single `everything-linux` binary must be able to act as both GUI and
//! daemon. This exercises the daemon half plus the auto-start path the GUI uses
//! (`ensure_daemon_with`), without needing a display.
//!
//! The child's environment is passed explicitly instead of mutating ours:
//! `std::env::set_var` is `unsafe` in edition 2024, and even then it would race
//! with other tests running on other threads.

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

    // Isolated config + cache for the child, so it only indexes our tiny root
    // (and never touches the developer's home or real cache).
    let cfg = dir.0.join("cfg");
    std::fs::create_dir_all(cfg.join("everything-linux")).unwrap();
    std::fs::write(
        cfg.join("everything-linux").join("config.json"),
        format!(r#"{{"roots":["{}"]}}"#, root.display()),
    )
    .unwrap();
    let envs = vec![
        (
            "XDG_CONFIG_HOME".to_string(),
            cfg.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            dir.0.join("cache").to_string_lossy().into_owned(),
        ),
    ];

    let addr = free_addr();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_everything-linux"));

    // The binary should run itself as a daemon.
    let mut child = everything_app::spawn_daemon_with_env(&addr, &exe, &envs)
        .expect("spawning the daemon half of the binary failed");
    let up = everything_app::wait_for_daemon(&addr, Duration::from_secs(30));

    // Poll until the fixture is indexed and searchable.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found = false;
    while Instant::now() < deadline {
        if let Ok(body) =
            everything_core::remote::request(&addr, "GET", "/v1/search?query=needle*&limit=5", None)
            && String::from_utf8_lossy(&body).contains("needle.txt")
        {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // With a daemon already listening, ensure must not start a second one.
    let second = everything_app::ensure_daemon_with(&addr, &exe);

    let _ = child.kill();
    let _ = child.wait();

    assert!(up, "daemon never became reachable on {addr}");
    assert!(found, "daemon never returned the fixture file");
    assert!(
        matches!(second, everything_app::Ensured::AlreadyUp),
        "expected AlreadyUp, got {second:?}"
    );
}
