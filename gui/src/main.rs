//! `everything-gui` — standalone GUI entry point.
//!
//! The GUI itself lives in this crate's library so that the combined
//! single-binary app (`everything-linux`) can reuse it. Prefer that binary for
//! end users; this one is handy for development.

fn main() -> eframe::Result {
    everything_gui::run(everything_gui::select_backend())
}
