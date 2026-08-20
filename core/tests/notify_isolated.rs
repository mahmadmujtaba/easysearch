//! Isolation check: does `notify` deliver inotify events in this environment?

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn notify_delivers_create_events() {
    let dir = std::env::temp_dir().join(format!("notify-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default()).unwrap();
    watcher.watch(&dir, RecursiveMode::Recursive).unwrap();

    std::fs::write(dir.join("hello.txt"), "hi").unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(ev)) => {
                eprintln!("event: {ev:?}");
                if ev.paths.iter().any(|p| p.ends_with("hello.txt")) {
                    got = true;
                    break;
                }
            }
            Ok(Err(e)) => eprintln!("error event: {e}"),
            Err(_) => {}
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(got, "no Create event received for hello.txt within 5s");
}
