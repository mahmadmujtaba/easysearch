//! End-to-end tests with real filesystem events (inotify).
//!
//! These verify the core promise of the project: a file created / edited /
//! deleted on disk shows up in the *next query* within ~1 s.

use everything_core::{Config, Engine, Query};
use std::time::{Duration, Instant};

fn wait_until(timeout: Duration, cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn test_engine(root: &std::path::Path) -> Engine {
    let mut cfg = Config::default();
    cfg.roots = vec![root.to_string_lossy().into_owned()];
    let mut engine = Engine::new(cfg);
    engine.start();
    assert!(
        engine.wait_live(Duration::from_secs(30)),
        "initial walk did not finish"
    );
    engine
}

#[test]
fn name_regex_content_and_realtime() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Seed the tree before the engine starts.
    std::fs::write(root.join("report_2026.pdf"), "annual report body").unwrap();
    std::fs::write(root.join("invoice.txt"), "invoice for q3 2026").unwrap();
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::write(root.join("sub/deep/notes.md"), "# Notes\nTODO: refactor").unwrap();

    let engine = test_engine(root);

    // --- glob name search
    let resp = engine
        .search(&Query { name: "*.pdf".into(), ..Query::default() })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("report_2026.pdf")));

    // --- regex name search
    let resp = engine
        .search(&Query {
            name: r"report[_-]\d{4}\.pdf".into(),
            regex_mode: true,
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("report_2026.pdf")));

    // --- Everything-style AND + exclude
    let resp = engine
        .search(&Query { name: "invoice q3".into(), ..Query::default() })
        .unwrap();
    assert_eq!(resp.results.len(), 1);

    // --- content search (always fresh, live reads)
    let resp = engine
        .search(&Query {
            name: String::new(),
            content: Some("TODO".into()),
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("notes.md")));

    // --- REALTIME: new file appears within 5 s
    let new_path = root.join("brand_new.txt");
    std::fs::write(&new_path, "fresh content here").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query { name: "brand_new*".into(), ..Query::default() })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("brand_new.txt"))),
        "created file was not picked up by the watcher"
    );

    // --- REALTIME: edited content is searchable immediately (live read)
    std::fs::write(root.join("invoice.txt"), "invoice UPDATED marker").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query {
                name: String::new(),
                content: Some("UPDATED".into()),
                ..Query::default()
            })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("invoice.txt"))),
        "edited file content was not found"
    );

    // --- REALTIME: deletion disappears from results within 5 s
    std::fs::remove_file(&new_path).unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || !engine
            .search(&Query { name: "brand_new*".into(), ..Query::default() })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("brand_new.txt"))),
        "deleted file is still in the index"
    );

    // --- hidden files: index them, filter at query time
    std::fs::write(root.join(".secret.txt"), "s").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query {
                name: "secret*".into(),
                include_hidden: true,
                ..Query::default()
            })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with(".secret.txt"))),
        "hidden file was not indexed"
    );
    let resp = engine
        .search(&Query { name: "secret*".into(), ..Query::default() })
        .unwrap();
    assert!(!resp.results.iter().any(|r| r.path.ends_with(".secret.txt")));
}

#[test]
fn newly_created_directory_is_indexed_live() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("existing.txt"), "x").unwrap();
    let engine = test_engine(root);

    // Create a new directory with a file inside it after start.
    let new_dir = root.join("freshdir");
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("inside.txt"), "deep content").unwrap();

    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query { name: "inside*".into(), ..Query::default() })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("inside.txt"))),
        "file inside a newly created directory was not indexed"
    );
}
