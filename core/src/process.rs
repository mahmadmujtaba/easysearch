//! Process-level setup shared by every binary.
//!
//! Both of these must happen before anything else runs, so the binaries call them
//! first thing in `main`.

/// Cap glibc's per-thread malloc arenas.
///
/// glibc gives every thread its own arena and grows each one to 64 MiB without
/// ever returning it to the OS. An engine that serves a request per thread — plus
/// the GUI's status poller and tray, and the engine's watcher and extractor
/// threads — therefore ends up with one 64 MiB heap per thread it has ever used,
/// and the default ceiling is 8 × cores (96 arenas, ~6 GB, on a 12-core machine).
/// We measured a live engine holding 1.4 GB of RSS for a 100 000-entry index:
/// twenty-two 64 MiB arenas, most of them empty, because memory the content cache
/// had freed was never handed back.
///
/// Two arenas is ample for this workload and keeps the footprint flat.
///
/// Must run before the process creates any thread; an explicit `MALLOC_ARENA_MAX`
/// in the environment wins.
pub fn cap_malloc_arenas() {
    if let Some(cap) = arena_cap(std::env::var("MALLOC_ARENA_MAX").ok().as_deref()) {
        // SAFETY: called from `main` before any thread exists, so nothing can be
        // reading the environment concurrently.
        unsafe { std::env::set_var("MALLOC_ARENA_MAX", cap) };
    }
}

/// The cap to apply, or `None` when one is already set.
fn arena_cap(current: Option<&str>) -> Option<&'static str> {
    if current.is_none() { Some("2") } else { None }
}

/// Ask for the in-RAM content cache (`--content-in-memory`).
///
/// Set through the environment rather than a parameter because the engine child
/// the app spawns is a separate process that loads its own config — this is how the choice
/// reaches it.
///
/// Must run before the engine is built: the environment is read once, when the
/// config is loaded.
pub fn use_content_memory() {
    // SAFETY: called from `main` before any thread exists.
    unsafe { std::env::set_var("EASYSEARCH_CONTENT_MEMORY", "1") };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_arena_cap_is_left_alone() {
        assert_eq!(arena_cap(None), Some("2"));
        for set in ["8", "1", ""] {
            assert_eq!(arena_cap(Some(set)), None, "{set:?} was set explicitly");
        }
    }
}
