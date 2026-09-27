//! `easysearch-gui` — standalone GUI entry point.
//!
//! The GUI itself lives in this crate's library so that the combined
//! single-binary app (`easysearch`) can reuse it. Prefer that binary for
//! end users; this one is handy for development.

fn main() -> eframe::Result {
    // Before the window, the tray, the status poller or the engine can start a
    // thread. The cache flags are read when the engine loads its config.
    easysearch_core::process::cap_malloc_arenas();
    let _ = easysearch_core::Config::load().ensure_exclude_names_file();
    if std::env::args().any(|a| a == "--content-in-memory") {
        easysearch_core::process::use_content_memory();
    }
    easysearch_gui::run(easysearch_gui::select_backend())
}
