//! The single `easysearch` binary must work as both halves: with `--engine` it
//! is the engine the app spawns, speaking JSON frames on stdin/stdout.
//!
//! This exercises that half through the real client (`ChildEngine`) with no
//! display, which is exactly how the app drives it.
//!
//! The child's environment is passed explicitly instead of mutating ours:
//! `std::env::set_var` is `unsafe` in edition 2024, and even then it would race
//! with other tests running on other threads.

use easysearch_core::{ChildEngine, Query};
use std::path::PathBuf;
use std::time::Duration;

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> TestDir {
        let dir = std::env::temp_dir().join(format!("easysearch-app-{tag}-{}", std::process::id()));
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

#[test]
fn search_flag_is_parsed() {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        easysearch_app::search_from_args(&args(&["--search", "report 2024"])),
        Some("report 2024".to_string())
    );
    assert_eq!(
        easysearch_app::search_from_args(&args(&["--search=*.rs"])),
        Some("*.rs".to_string())
    );
    assert_eq!(easysearch_app::search_from_args(&args(&["--toggle"])), None);
}

#[test]
fn the_single_binary_serves_as_the_engine_child() {
    let dir = TestDir::new("engine");
    let root = dir.0.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("needle.txt"), "hello").unwrap();

    // Isolated config + cache for the child, so it only indexes our tiny root.
    let cfg = dir.0.join("cfg");
    std::fs::create_dir_all(cfg.join("easysearch")).unwrap();
    std::fs::write(
        cfg.join("easysearch").join("config.json"),
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

    let exe = PathBuf::from(env!("CARGO_BIN_EXE_easysearch"));
    let engine = ChildEngine::spawn(&exe, &["--engine", "--quiet"], &envs, None)
        .expect("the binary should run as the engine");

    assert!(
        engine.wait_live(Duration::from_secs(30)),
        "the engine never became live"
    );

    let response = engine
        .search(&Query {
            name: "needle*".into(),
            limit: 5,
            ..Query::default()
        })
        .unwrap();
    assert!(
        response
            .results
            .iter()
            .any(|r| r.path.ends_with("needle.txt")),
        "unexpected results: {:?}",
        response.results
    );

    // The engine is a child of this process, so stopping it is enough.
    let _ = engine.shutdown();
}
