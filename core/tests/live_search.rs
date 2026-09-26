//! End-to-end tests with real filesystem events (inotify).
//!
//! These verify the core promise of the project: a file created / edited /
//! deleted on disk shows up in the *next query* within ~1 s.

use everything_core::{Config, Engine, Query};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// A dedicated test directory under the system temp dir.
///
/// Note: NOT dot-prefixed (unlike `tempfile`'s `.tmpXXXX` names) — hidden-path
/// filtering is part of the default query behavior and would hide all
/// fixtures otherwise.
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> TestDir {
        let dir = std::env::temp_dir().join(format!("everything-it-{tag}-{}", std::process::id()));
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
    let cfg = Config {
        roots: vec![root.to_string_lossy().into_owned()],
        // Isolated on-disk index cache (no pollution of ~/.cache, no cross-test races).
        disk_index_dir: Some(root.join(".cache-dir").to_string_lossy().into_owned()),
        ..Config::default()
    };
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
    let dir = TestDir::new("main");
    let root = dir.0.clone();

    // Seed the tree before the engine starts.
    std::fs::write(root.join("report_2026.pdf"), "annual report body").unwrap();
    std::fs::write(root.join("invoice.txt"), "invoice for q3 2026").unwrap();
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::write(root.join("sub/deep/notes.md"), "# Notes\nTODO: refactor").unwrap();

    let engine = test_engine(&root);

    // --- glob name search
    let resp = engine
        .search(&Query {
            name: "*.pdf".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(
        resp.results
            .iter()
            .any(|r| r.path.ends_with("report_2026.pdf"))
    );

    // --- regex name search
    let resp = engine
        .search(&Query {
            name: r"report[_-]\d{4}\.pdf".into(),
            regex_mode: true,
            ..Query::default()
        })
        .unwrap();
    assert!(
        resp.results
            .iter()
            .any(|r| r.path.ends_with("report_2026.pdf"))
    );

    // --- Everything-style AND + exclude
    let resp = engine
        .search(&Query {
            name: "invoice txt".into(),
            ..Query::default()
        })
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
            .search(&Query {
                name: "brand_new*".into(),
                ..Query::default()
            })
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
            .search(&Query {
                name: "brand_new*".into(),
                ..Query::default()
            })
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
                name: "secret".into(),
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
        .search(&Query {
            name: "secret".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(!resp.results.iter().any(|r| r.path.ends_with(".secret.txt")));
}

#[test]
fn newly_created_directory_is_indexed_live() {
    let dir = TestDir::new("newdir");
    let root = dir.0.clone();
    std::fs::write(root.join("existing.txt"), "x").unwrap();
    let engine = test_engine(&root);

    // Create a new directory with a file inside it after start.
    let new_dir = root.join("freshdir");
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("inside.txt"), "deep content").unwrap();

    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query {
                name: "inside*".into(),
                ..Query::default()
            })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("inside.txt"))),
        "file inside a newly created directory was not indexed"
    );
}

#[test]
fn gitignore_is_respected() {
    let dir = TestDir::new("gitignore");
    let root = dir.0.clone();
    std::fs::write(root.join("keep.txt"), "keep me").unwrap();
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::fs::write(root.join("node_modules/dep.js"), "dep").unwrap();
    std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();

    let engine = test_engine(&root);

    let resp = engine
        .search(&Query {
            name: "*".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(
        resp.results.iter().any(|r| r.path.ends_with("keep.txt")),
        "keep.txt should be indexed"
    );
    assert!(
        !resp.results.iter().any(|r| r.path.ends_with("dep.js")),
        "node_modules/ should be excluded by .gitignore"
    );
}

#[test]
fn disk_index_is_reused_across_restarts() {
    // First run builds the on-disk index; a second engine on the same cache
    // dir must serve from it immediately (base present, then revalidated).
    let dir = TestDir::new("reuse");
    let root = dir.0.clone();
    std::fs::write(root.join("alpha.txt"), "a").unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/beta.md"), "b").unwrap();

    let cfg = Config {
        roots: vec![root.to_string_lossy().into_owned()],
        disk_index_dir: Some(root.join(".cache-dir").to_string_lossy().into_owned()),
        ..Config::default()
    };

    let mut e1 = Engine::new(cfg.clone());
    e1.start();
    assert!(e1.wait_live(Duration::from_secs(30)));
    drop(e1);

    // Second engine: cache exists → base loads immediately.
    let mut e2 = Engine::new(cfg);
    let snap = e2.status_snapshot();
    assert!(
        snap.base_entries >= 3,
        "expected cached base entries, got {}",
        snap.base_entries
    );
    e2.start();
    assert!(e2.wait_live(Duration::from_secs(30)));

    // Searchable from cache (before the background revalidation even lands).
    let resp = e2
        .search(&Query {
            name: "alpha*".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("alpha.txt")));
    let resp = e2
        .search(&Query {
            name: "beta*".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("beta.md")));
}

#[test]
fn ram_mode_still_works() {
    let dir = TestDir::new("ram");
    let root = dir.0.clone();
    std::fs::write(root.join("mem.txt"), "ram resident").unwrap();

    let cfg = Config {
        roots: vec![root.to_string_lossy().into_owned()],
        persist_index: false, // pure in-memory mode
        ..Config::default()
    };
    let mut engine = Engine::new(cfg);
    engine.start();
    assert!(engine.wait_live(Duration::from_secs(30)));

    let resp = engine
        .search(&Query {
            name: "mem*".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(resp.results.iter().any(|r| r.path.ends_with("mem.txt")));

    // Realtime still works in RAM mode.
    std::fs::write(root.join("new_ram.txt"), "x").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || engine
            .search(&Query {
                name: "new_ram*".into(),
                ..Query::default()
            })
            .unwrap()
            .results
            .iter()
            .any(|r| r.path.ends_with("new_ram.txt"))),
        "realtime create not picked up in RAM mode"
    );
}

#[test]
fn count_agrees_with_search_and_respects_under() {
    let dir = TestDir::new("count");
    let root = dir.0.clone();
    std::fs::create_dir_all(root.join("a/sub")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::write(root.join("a/one.txt"), "x").unwrap();
    std::fs::write(root.join("a/sub/two.txt"), "x").unwrap();
    std::fs::write(root.join("b/three.txt"), "x").unwrap();
    std::fs::write(root.join("b/four.md"), "x").unwrap();

    let engine = test_engine(&root);

    // count() must agree with search() for the same query.
    let q = Query {
        name: "*.txt".into(),
        ..Query::default()
    };
    let resp = engine.search(&q).unwrap();
    assert_eq!(
        engine.count(&q).unwrap() as usize,
        resp.results.len(),
        "count and search disagree"
    );

    // `under` restricts to one directory subtree.
    let sub = root.join("b");
    let q = Query {
        name: "*.txt".into(),
        under: Some(sub.to_string_lossy().into_owned()),
        ..Query::default()
    };
    let resp = engine.search(&q).unwrap();
    assert!(resp.results.iter().all(|r| r.path.starts_with(&sub)));
    assert!(resp.results.iter().any(|r| r.path.ends_with("three.txt")));
    assert!(!resp.results.iter().any(|r| r.path.ends_with("one.txt")));
    assert_eq!(engine.count(&q).unwrap() as usize, resp.results.len());

    // Folders can be excluded, and count() honours it too.
    let q = Query {
        include_dirs: false,
        ..Query::default()
    };
    let resp = engine.search(&q).unwrap();
    assert!(resp.results.iter().all(|r| !r.is_dir));
    assert_eq!(engine.count(&q).unwrap(), 4, "four fixture files");
}
#[test]
fn include_dirs_toggle_filters_folders() {
    let dir = TestDir::new("dirs");
    let root = dir.0.clone();

    std::fs::create_dir_all(root.join("reports-2026")).unwrap();
    std::fs::write(root.join("reports-2026.txt"), "x").unwrap();

    let engine = test_engine(&root);

    // Default: folders are part of the results.
    let with = engine
        .search(&Query {
            name: "reports*".into(),
            ..Query::default()
        })
        .unwrap();
    assert!(
        with.results.iter().any(|r| r.is_dir),
        "folder should be included by default"
    );
    assert!(with.results.iter().any(|r| !r.is_dir));

    // Opting out must drop the folder but keep the file.
    let without = engine
        .search(&Query {
            name: "reports*".into(),
            include_dirs: false,
            ..Query::default()
        })
        .unwrap();
    assert!(!without.results.is_empty(), "the file should still match");
    assert!(
        without.results.iter().all(|r| !r.is_dir),
        "folders must be excluded when include_dirs is false"
    );
}

#[test]
fn extension_size_and_recency_filters_agree_and_reduce() {
    let dir = TestDir::new("filters");
    let root = dir.0.clone();

    // Sizes are chosen so each filter clearly partitions the fixtures.
    std::fs::write(root.join("small.pdf"), vec![b'a'; 100]).unwrap();
    std::fs::write(root.join("large.pdf"), vec![b'a'; 4096]).unwrap();
    std::fs::write(root.join("notes.md"), vec![b'b'; 512]).unwrap();
    std::fs::write(root.join("stale.txt"), vec![b'c'; 100]).unwrap();
    // A directory carrying a file extension: must never match the ext filter.
    std::fs::create_dir_all(root.join("folder.pdf")).unwrap();

    // Age `stale.txt` by 30 days before the engine indexes it, so the base
    // index records the old mtime.
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(root.join("stale.txt"))
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(30 * 24 * 3600))
            .unwrap();
    }

    let engine = test_engine(&root);

    // Baseline: every fixture (the hidden `.cache-dir` is filtered out).
    let all = Query {
        name: "*".into(),
        ..Query::default()
    };
    let base_len = engine.search(&all).unwrap().results.len();
    assert_eq!(engine.count(&all).unwrap() as usize, base_len);
    assert!(base_len >= 5, "expected the fixtures, got {base_len}");

    // --- extension filter: only listed extensions, never directories
    let ext = Query {
        name: "*".into(),
        extensions: vec!["pdf".into()],
        ..Query::default()
    };
    let resp = engine.search(&ext).unwrap();
    assert_eq!(engine.count(&ext).unwrap() as usize, resp.results.len());
    assert!(resp.results.iter().all(|r| !r.is_dir));
    assert!(
        resp.results
            .iter()
            .all(|r| r.path.extension().and_then(|e| e.to_str()) == Some("pdf"))
    );
    assert!(resp.results.iter().any(|r| r.path.ends_with("small.pdf")));
    assert!(resp.results.iter().any(|r| r.path.ends_with("large.pdf")));
    assert!(
        resp.results.len() < base_len,
        "extension filter should reduce the set"
    );

    // --- size bounds are inclusive (512 and 4096 both included)
    let sized = Query {
        name: "*".into(),
        min_size: Some(512),
        max_size: Some(4096),
        ..Query::default()
    };
    let resp = engine.search(&sized).unwrap();
    assert_eq!(engine.count(&sized).unwrap() as usize, resp.results.len());
    assert!(resp.results.iter().all(|r| !r.is_dir));
    assert!(resp.results.iter().all(|r| r.size >= 512 && r.size <= 4096));
    assert!(resp.results.iter().any(|r| r.path.ends_with("notes.md")));
    assert!(resp.results.iter().any(|r| r.path.ends_with("large.pdf")));
    assert!(!resp.results.iter().any(|r| r.path.ends_with("small.pdf")));
    assert!(
        resp.results.len() < base_len,
        "size filter should reduce the set"
    );

    // --- recency filter: the 30-day-old file drops out
    let recent = Query {
        name: "*".into(),
        modified_within_secs: Some(24 * 3600),
        ..Query::default()
    };
    let resp = engine.search(&recent).unwrap();
    assert_eq!(engine.count(&recent).unwrap() as usize, resp.results.len());
    assert!(!resp.results.iter().any(|r| r.path.ends_with("stale.txt")));
    assert!(resp.results.iter().any(|r| r.path.ends_with("small.pdf")));
    assert!(
        resp.results.len() < base_len,
        "recency filter should reduce the set"
    );
}
