# Changelog

All notable changes to **Everything for Linux** are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-20

Initial release — a realtime filename **and** content search engine for Linux
(Everything-for-Windows equivalent), in Rust with a native egui GUI.

### Added

- **Realtime filename index**: live in-memory/on-disk index kept fresh by kernel
  filesystem events (`notify`/inotify); create / rename / edit / delete is
  reflected in the next query within ~1 s.
- **Everything-style queries**: space-separated AND terms, `!term` excludes,
  glob patterns (`*.pdf`) and substring terms, optional regex mode, case
  toggle, hidden-file toggle, basename or full-path matching.
- **Content search**: the embedded ripgrep engine (`grep-searcher`) reads files
  live — results are always current; optional bounded in-RAM content cache
  (default off) accelerates repeated queries.
- **`.docx` content search** via a preprocessor hook (`docx2txt`); degrades
  gracefully when the tool is absent.
- **Low-memory index architecture**: the bulk of the index lives in a
  memory-mapped file (`~/.cache/everything-linux/index-v1.bin`, kernel page
  cache, reclaimable); only a small recent-change overlay and a compact hash
  table stay resident. Background compaction folds the overlay back into the
  file. RAM-only mode available via `persist_index: false`.
- **Instant warm start**: the previous index is memory-mapped on launch and
  revalidated in the background.
- **Ignore files**: `.gitignore`/`.ignore` in the searched tree are honored
  (gitignore syntax), plus a global ignore at
  `~/.config/everything-linux/ignore`. Shipped `.gitignore` excludes
  `node_modules`, `target`, build dirs, caches, editor settings, VCS internals.
- **Roots & exclusions**: default root is the running user's `$HOME`
  (configurable); pseudo-filesystems, network mounts, and USB/removable media
  excluded by default.
- **Degraded mode**: if kernel watch limits are exhausted, the engine falls
  back to periodic full rebuilds and reports the state in the UI.
- **Frontends**: native `everything-gui` (eframe/egui) and a scriptable
  `everything` CLI; both link the `everything-core` library directly (zero IPC).
- **Zero-sudo build**: `scripts/install-deps.sh` sets up rustup + rustup's
  bundled `rust-lld` + user-local library symlinks — no C compiler required.
- **Build shim**: `vendor/arrayref` replaces the crates.io `arrayref` crate
  (all upstream versions 0.3.5+ were yanked), restoring dependency resolution
  for the egui/winit stack.

### Performance (measured, Debian 13, real `$HOME`, ~137k files)

- Filename query: < 1 ms engine time; content query ≈ 23 ms across 137k files.
- Warm start (cache present): fully searchable in ~0.6 s.
- Idle memory: headless engine ≈ 15 MiB (was ~80 MiB with the in-RAM index);
  GUI ≈ 151 MiB incl. the Mesa GL stack (~55 MiB) and the 23 MiB mmap index.
- Binaries: `everything` 3.3 MB, `everything-gui` 12 MB (stripped, LTO).

### Known limitations

- Text-based files only (code, config, plain text, `.docx`); binary content
  search (PDF/images/archives) is out of scope for now.
- Results are returned in index order, not ranked; fuzzy/substring ranking is a
  planned enhancement.
- Non-UTF-8 file names are matched lossily.
- Network filesystems and removable media are not indexed by default.

[0.1.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.1.0
