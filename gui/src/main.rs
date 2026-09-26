//! `easysearch-gui` — standalone GUI entry point.
//!
//! The GUI itself lives in this crate's library so that the combined
//! single-binary app (`easysearch`) can reuse it. Prefer that binary for
//! end users; this one is handy for development.

fn main() -> eframe::Result {
    easysearch_gui::run(easysearch_gui::select_backend())
}
