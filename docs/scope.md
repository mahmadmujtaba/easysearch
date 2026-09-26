# EasySearch — Scope

**Status:** Draft for review · **Date:** 2026-08-20 · **Target platform:** Debian 13 (trixie) / Linux, x86_64 (cross-platform design)

> This document defines *what* the project builds and *why*, before any implementation.
> It is the contract between intent and code. Read this before touching `core/`, `gui/`, or `cli/`.

---

## 1. Problem Statement

On Windows, **Everything (VoidTools)** provides instant, realtime search of the entire
filesystem — filenames and, with an extension, file contents. Linux has no direct
equivalent that is both *realtime* and *lightweight*:

| Linux tool | Realtime? | Footprint | Content search? | Verdict |
|---|---|---|---|---|
| `find` / `fd` | Yes (on-demand) | small | No | Slow for whole-FS scans on every query |
| `locate` / `plocate` | **No** — index updated by cron (`updatedb`) | small | No | Fast but **stale**; misses brand-new files |
| `ripgrep` | Yes (on-demand) | small | Yes (excellent) | Filenames only via `--files`; no name index |
| Baloo (KDE) / Recoll / Tracker | Yes (inotify index) | **heavy** (indexer daemons, DBs, CPU on content re-index) | Yes | The opposite of "minimal footprint" |
| `fzf` | n/a (filter tool) | small | No | Frontend, not a search engine |

**What we want:** an *Everything-like* tool — a GUI that searches **filenames** and
**file contents** across the whole filesystem, where results reflect filesystem state
*in realtime* (a file created / renamed / deleted / edited shows up in the next query),
while keeping memory and CPU footprint minimal.

**How we get there (one sentence):** a small always-running **Rust core** keeps a live
in-memory **filename index** updated by kernel filesystem events (`notify`/inotify), and
answers **content** queries with the **ripgrep engine embedded as a library** — so content
results are always current, at the fastest speed available. A **background content index**
(optional, default off) can additionally cache extracted text in RAM to accelerate
repeated content queries, and is kept fresh by the same event pipeline.

---

## 2. Goals / Non-Goals

### In scope (this project)

- Realtime whole-filesystem **filename/path search** (glob patterns *and* regex).
- Realtime **content search** over text-based files (code, configs, plain text, logs)
  and **Word `.docx`** documents, **OpenDocument `.odt`** documents and the text
  layer of **PDF** files.
- Lightweight **native GUI** (search box, results list, match-mode toggles, status bar) —
  no web technologies.
- **CLI** for scripting; optional headless **daemon** with a localhost HTTP API.
- Minimal resource footprint (explicit budgets in §9).
- Reuse ripgrep's proven engine and Linux's kernel event APIs instead of reimplementing
  them (§7). Cross-platform *design* (Linux first, Windows/macOS feasible later).

### Out of scope (explicitly not building)

- Content indexing into a database (Baloo/Recoll territory — heavy; unnecessary because
  the embedded ripgrep engine reads live data faster than any incremental indexer for
  human-scale queries).
- Binary file content search (images, video, archives) — text, **Word `.docx`**,
  **OpenDocument `.odt`** and **PDF (text layer)** are searchable.
- Network filesystems as *indexed roots* (SMB/NFS mounts excluded by default; see §6).
- Persistent database or boot-time cache — the index lives in RAM, rebuilt on start.
- Web-based UI (Electron/Tauri/webview) — explicitly rejected by design decision.
- Windows/macOS *delivery* in phase 1 (code stays portable; packaging is later).
- Editing, moving, or deleting files from within the tool (read-only search; "open in
  file manager" only).

---

## 3. Architecture Overview

```
                     Rust workspace — one language, one crate family
┌───────────────────────────  one binary family, two processes  ─────────────────────────────┐
│                                                                                             │
│   core/  (library crate: all search logic)                                                  │
│   ├── indexer ── ignore walk (cold walk) ──▶ SQLite database  db/index.db                   │
│   │      ▲                                           │  sole writer, WAL mode             │
│   │      └── notify crate (inotify) ◀─ kernel events ─┘  batched transactions               │
│   │                                  (create/delete/rename/edit)                            │
│   │                       small in-memory change overlay (flushed into the db per query)    │
│   ├── matcher ── globset (pattern mode) + regex crate (regex mode) over indexed paths       │
│   ├── content  ── grep-searcher + grep-regex + ignore (ripgrep engine, in-process)          │
│   │                 └─ preprocessor hook (docx extraction)                                   │
│   ├── content_index (OPTIONAL, default off) ── background extractor + RAM text cache        │
│   │                 (bounded, LRU; kept fresh by the same watcher events)                    │
│   └── queries  ── SQL pushdown (dir/ext/size/mtime/hidden) + one shared predicate          │
│                     ∪ pending overlay deltas                                                 │
│                                                                                             │
│   app/  (one binary: easysearch) ── starts or attaches to the daemon, runs the GUI     │
│   daemon/ (tiny_http)  OWNS the index ── localhost HTTP/JSON, keeps the database current     │
│   cli/  (clap)         in-process or --remote ── scripted search / status                   │
│   gui/  (eframe/egui)  the UI ── reads via the daemon when one is running                   │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **One owner of the index.** The daemon is the only writer of the SQLite database and
  keeps it current from kernel events; the GUI, CLI and any other client only read. WAL
  mode means readers never block the writer.
- **The default path does use IPC** — deliberately. `easysearch` makes sure a daemon
  is listening (re-executing itself with `--daemon`) and talks to it over localhost
  HTTP/JSON. If a daemon cannot be started, the GUI falls back to an in-process engine, so
  a single process is still a supported configuration (one writer either way).
- The index is durable and shared: closing the GUI leaves the daemon indexing, and the next
  start serves from the database immediately while it is re-validated in the background.

### 3.1 Tech stack decisions (2026-08-20, per user direction)

**Language: Rust.** Rationale — the single biggest speed lever for content search is
ripgrep, which *is* Rust; embedding it as a library (`grep-searcher`, `grep-regex`,
`ignore`, `globset`) gives the fastest available content search in-process, with zero
runtime dependencies. Rust also delivers the smallest footprint (no GC, no runtime) and
strong native GUI options. Go was evaluated and rejected: it cannot embed ripgrep (would
require subprocess-per-query or a slower RE2 engine) and has a weaker native-GUI story.

| Component | Choice | Alternative considered / rejected |
|---|---|---|
| Language | **Rust** (edition 2021+) | Go (rejected: no ripgrep embedding, GC baseline) |
| Filename patterns | **`globset`** (ripgrep's glob engine) | custom fnmatch |
| Filename regex | **`regex`** crate | Go `regexp` (RE2, no backrefs) |
| Content search | **`grep-searcher` + `grep-regex`** (ripgrep engine) | subprocess `rg` (rejected: IPC + external dep) |
| Filesystem walk | **`ignore`** crate (gitignore-aware, parallel) | hand-rolled walkdir |
| Realtime events | **`notify`** crate (inotify on Linux; ReadDirectoryChangesW / FSEvents elsewhere) | `inotifywait` subprocess (rejected: external dep, no cross-platform) |
| Parallel search fan-out | **`rayon`** | hand-rolled threads |
| Content text cache (optional) | in-RAM `HashMap` + byte-cap LRU | — (the SQLite index is separate; FTS5 is the natural next step) |
| Index persistence | **SQLite** (`rusqlite`, bundled) — WAL, one writer, SQL pushdown for filters | `memmap2` + custom binary format (kept as the `storage = "mmap"` escape hatch), full-RAM (rejected: 80 MiB) |
| GUI | **`eframe`/`egui`** (immediate-mode, native, Win/Linux/macOS) | Slint, iced, gtk4-rs (all viable; egui = minimal deps + instant re-render per keystroke) |
| CLI | **`clap`** | hand-rolled parser |
| Optional daemon HTTP | **`tiny_http`** (small, stdlib-ish) | axum (heavier) |
| Serialization (daemon only) | **`serde_json`** | — |
| `.docx` / `.odt` / `.pdf` extraction | **in-process** (`zip` + `quick-xml`; `pdf-extract`), no external tool | external `docx2txt` subprocess (removed in v0.18.0) |

**Explicitly rejected:** Python (too slow — user direction), web-based UI frameworks
(Electron/Tauri/webview — user direction), Baloo/Recoll/Tracker (heavy), subprocess-per-
query `rg` (slow path for interactive use), a hand-rolled content indexer.

---

## 4. Search Semantics

### 4.1 Filename / path search (the Everything-style core)

Match is applied to each indexed path. Two modes, switchable per query:

| Mode | Example | Engine |
|---|---|---|
| **Pattern (glob)** | `*.pdf` | `globset` (Everything-style: `*` = any run, `?` = one char, `[abc]` = class) |
| **Regex** | `report[_-]\d{4}\.docx$` | `regex` crate against the path |

- Match target is configurable: **basename** (default, Everything-like) or **full path**.
- Multiple whitespace-separated terms are ANDed (each term matched independently):
  `invoice 2026` → files matching *invoice* **and** *2026*.
- `!term` excludes (Everything-compatible: `*.tmp !draft`).
- Case-insensitive by default, case-sensitive toggle.
- Hidden files/dirs excluded by default, toggle to include (`.git`, `.cache`, …).
- Pattern matching runs over the index → sub-50 ms at 1M entries. With the SQLite
  backend the coarse dimensions below are pushed into SQL, so a filtered query does not
  walk the whole table.

**Filter dimensions** (all optional, all supported by the CLI, the HTTP API and the GUI's
filter bar). They are enforced by the same predicate whether the query is a `search` or a
`count`, so the two can never disagree:

| Filter | Meaning |
|---|---|
| `under` | only paths inside a directory (component-wise prefix) |
| `extensions` | only files whose final extension is listed (implies files-only) |
| `min_size` / `max_size` | inclusive byte bounds (implies files-only) |
| `modified_within_secs` | only files modified within the last N seconds |
| `category` | All / Recent / Images / Documents / Code / Archives / Audio / Video / Large |
| `include_hidden`, `include_dirs` | hidden files, folders in results |

### 4.2 Content search

- A separate toggle enables content matching. When on, the query's terms are fed to the
  **embedded ripgrep engine** (`grep-searcher`), constrained to:
  - the current filename-filtered result set (content **and** name → intersection), and
  - the user's scope root (default: whole FS minus exclusions, §6).
- Regex in content mode uses ripgrep's regex engine (same syntax as `rg`). A
  pattern can span lines with **Multiline** (`--multiline`, `?multiline=1`), which
  is off by default because it is much slower.
- **Realtime guarantee for content:** results are always computed from live disk state at
  query time. Editing a file then re-running the query immediately shows the change.
- `.docx` / `.odt` / `.pdf` content: text is extracted **in-process**, so Office
  and PDF files behave like text with no external tool to install. For OOXML/ODF
  (both ZIPs of XML) the run text, tabs, line breaks and table cells become
  whitespace that keeps words separated, and headers, footers, footnotes,
  endnotes and comments are included. For PDF it is the text layer only — a
  scanned, image-only PDF has none, which is a property of the file. Malformed
  input is skipped, never fatal (the PDF parser even runs under `catch_unwind`).
- Binary files are never content-searched (binary detection quits on NUL, exactly like
  `rg`).

**Optional background content index (default off).** When enabled (config/CLI flag), a
background worker extracts text from newly indexed and changed files (skipping files over
a size cap, default 8 MB) and caches it in RAM (byte cap, default 256 MB, LRU eviction).
Content queries then hit the cache first — repeated queries become nearly free — and fall
back to live search for files not yet indexed. Freshness contract is unchanged: watcher
events (`create`/`modify`/`delete`) incrementally re-index changed files, so the cache
never serves stale content beyond the same < 1 s window as the name index. Disabled =
zero extra memory and zero background CPU (pure on-demand search).

### 4.3 Combining name + content

One query box, three effective combinations:

| Name match | Content match | Result |
|---|---|---|
| on | off | everything-style name search |
| off | on | content-only search (all indexed paths walked) |
| on | on | intersection: files matching both (name narrows, content verifies) |

---

## 5. Realtime Design (kernel-event driven)

### 5.1 Index freshness pipeline

1. **Cold start:** parallel `ignore`-crate walk of configured roots (thread pool). Partial
   results are queryable while indexing continues; UI shows progress ("indexing… 68%").
2. **Steady state:** the `notify` crate watches every indexed directory (inotify on
   Linux). Events — `create`, `delete`, `rename`, `modify` — update the in-memory index
   immediately (target: reflected in results **< 1 s** of the filesystem event).
3. New directories created under a watched root are watched automatically (recursive
   watch maintenance by the watcher).

### 5.2 Watch-limit reality check

- Kernel limits watches per user: `fs.inotify.max_user_watches` (this machine: **123,040**;
  each watch ≈ 1 KB kernel memory). A large `/home` can exceed this.
- **Degraded mode** (automatic): if the watcher exhausts watches, unwatched subtrees are
  re-scanned periodically (default 30 s) and the status bar shows "live for N dirs ·
  periodic for M dirs". Realtime is preserved where possible; the user is never silently
  served a stale index.
- Users can raise the limit (documented in README: `sysctl fs.inotify.max_user_watches=...`).

### 5.3 What "realtime" means, precisely

| Property | Target |
|---|---|
| create / rename / delete → index | < 1 s |
| content edit → content query result | immediate (live read) |
| content edit → filename index metadata | < 1 s (modify event) |
| degraded mode (watch exhaustion) | ≤ 30 s (periodic rescan) |

---

## 6. Search Roots & Exclusions

- **Default roots:** `$HOME` of the user who runs the program (user decision 2026-08-20).
  Additional roots configurable in `~/.config/easysearch/config.json`.
- **Ignore files:** `.gitignore` / `.ignore` files in the searched tree are honored
  (gitignore syntax; `config.respect_ignore_files`, default on), plus a global ignore
  file at `~/.config/easysearch/ignore`. The shipped `.gitignore` excludes
  `node_modules`, `target`, build dirs, caches, editor settings, and VCS internals
  (copy it to `~/.gitignore` to apply to the default root).
- **Never indexed (pseudo-FS / noise):** `/proc`, `/sys`, `/dev`, `/run`, `/tmp`.
- **USB and external/removable mounts: excluded by default** (user decision 2026-08-20) —
  detection via `/proc/self/mounts` (mount type + source) plus the backing device's
  `removable` flag under `/sys/block/*/removable`. Opt-in via config.
- **Default-excluded mounts:** network mounts (NFS/SMB/CIFS/sshfs/FUSE), `tmpfs`
  overlays — opt-in.
- **Permissions:** unreadable directories are skipped with a counted, non-spammy notice
  ("skipped 3 dirs (permission denied)"), never a crash.
- Symlinks: indexed, never followed out of a root (prevents loops).

---

## 7. Reuse of Existing Tools (don't rebuild what Linux has)

| Component | Reuse as | Notes |
|---|---|---|
| Content search engine | **ripgrep's crates** (`grep-searcher`, `grep-regex`, `ignore`, `globset`) | The same code as the `rg` binary, in-process; no subprocess, no runtime dep |
| Filesystem events | **`notify`** crate | Wraps inotify (Linux); same API on Windows/macOS |
| `.docx` / `.odt` / `.pdf` text extraction | **`zip` + `quick-xml`** for OOXML/ODF, **`pdf-extract`** for PDF | All in-process: a package is a ZIP of XML, and a PDF text layer needs no subprocess or apt-installed tool |
| Open results | **`xdg-open`** | Desktop-standard "open file / containing folder" |
| Build toolchain | **apt** `rustc`/`cargo` 1.85 (or rustup) + `build-essential`, `pkg-config`, X11/Wayland/GL dev libs | Verified available on target; one-time `sudo apt install` |

**Explicitly rejected:** Baloo/Recoll/Tracker (heavy daemons, content DBs), a custom
`find`-based reimplementation, subprocess-per-query `rg`, a hand-rolled content indexer.

> Every content format is read **in-process**, so there is no external tool to
> install for `.docx` / `.odt` / `.pdf` content search. A file that cannot be
> parsed is skipped, and the query still succeeds.

---

## 8. Daemon HTTP API (localhost only) — implemented in `daemon/`

`easysearch-daemon` owns the engine as a **separate process** and serves it over
`tiny_http`, bound to `127.0.0.1:5858` by default. Read-only; no auth, loopback
only. Full reference: **`docs/api.md`**.

Splitting the engine out of the GUI means the index keeps running — and every
client keeps working — even when no GUI is running at all:

- `easysearch-gui` attaches to a daemon (`--daemon ADDR`, `EASYSEARCH_DAEMON`, or
auto-detected on the default address) and falls back to an in-process engine when
none is running, so it can never become unusable.
- `easysearch-cli` (CLI) indexes in-process by default and supports `--remote ADDR`.
- Any other client can use plain HTTP + JSON (`curl`, scripts, any language).

| Endpoint | Purpose |
|---|---|
| `GET /v1/health` | liveness/version probe |
| `GET /v1/status` | index size, watcher state (live/degraded), skipped-dir counts |
| `POST /v1/search` | `Query` JSON → `{results:[{path,size,mtime,is_dir}], truncated, elapsed_ms, indexed}` |
| `GET /v1/search` | same, via query parameters (`query`, `regex`, `content`, `case`, `hidden`, `path`, `dirs`, `limit`, `category`) |
| `POST /v1/rebuild` | full rescan (e.g. after system restore) |
| `GET /v1/watch` | long-poll: status returned when it changes, or on timeout |

Change notification is **long-polling**, not SSE/WebSocket: it works with every
HTTP client and is not defeated by `tiny_http`'s chunked encoder, which buffers
small writes (so short SSE frames are never flushed).

## 9. Resource Footprint Budget (non-negotiable targets)

| Metric | Target |
|---|---|
| Binary size (stripped, `lto`+`strip`) | **< 10 MB** per binary |
| Idle RSS, headless engine | **≈ 15 MiB** with the disk-backed index |
| Index storage | **on disk** — SQLite in `$XDG_CACHE_HOME/easysearch/db/` (WAL; page cache reclaimable) |
| Resident index structures | ≈ 4 MB hash table + small change overlay |
| Backend idle CPU | **≈ 0 %** (event-driven; no polling loops) |
| Cold index, 1M files | < 30 s; searchable from first second |
| Warm start (cache present) | **< 1 s** to first search |
| Filename query latency (1M entries) | < 50 ms |
| Content query (typical tree) | first result < 2 s; results streamed, cancellable |
| Content index (when **enabled**) | text spooled to `disk_index_dir/content/` on disk by default; ≤ 256 MB spool, LRU; **0 MB / 0 CPU when off**. `--content-in-memory` moves the ≤ 256 MB cap into RAM |

**Memory architecture (v0.32):** the optional content cache is **disk-backed by
default** — extracted text is spooled to `~/.cache/easysearch/content/` and
read back per lookup, so searching content does not leave the documents resident;
`--content-in-memory` (`EASYSEARCH_CONTENT_MEMORY=1`) keeps the old RAM map.
Every entry point calls `MALLOC_ARENA_MAX=2` before starting a thread, which
stops glibc from growing dozens of never-unmapped 64 MiB arenas on many-core
machines — the reason a long-running content search could sit at well over a
gigabyte of anonymous RSS.

**Memory architecture (v0.13):** the index lives in a SQLite database
(`~/.cache/easysearch/db/index.db`, WAL mode) owned by the daemon; only a small
in-memory overlay of recent changes is always resident, and it is folded into the database
in one transaction before each query. `storage = "mmap"` in `config.json` switches back to
the original memory-mapped file (`index-v1.bin`), and `persist_index = false` keeps nothing
on disk at all. `malloc_trim` returns walk-transient pages to the kernel after each build.

**Measured (v0.1.0, Debian 13, real `$HOME`, ≈137k files):** CLI binary 3.3 MB,
GUI 12 MB; headless engine idle ≈ 15 MiB (vs ≈ 80 MiB before the disk-backed
index); warm start 0.6 s; filename query < 1 ms engine time; content query
≈ 23 ms. GUI ≈ 151 MiB incl. the Mesa GL stack (≈ 55 MiB) and the 23 MiB mmap
index; remaining anonymous memory is egui UI state + allocator arena residual.

**Measured (v0.13, KDE/Plasma Wayland, real `$HOME`, 100 479 entries):** GUI
≈ 92 MiB and the daemon ≈ 51 MiB (≈ 143 MiB together); filename queries 1–26 ms;
filtered queries 1–2 ms (SQL pushdown); content query ≈ 113 ms; a full build of
the database takes a few seconds and 65 MB on disk. See `docs/pending.md` §6 for
the measurement method.

## 10. GUI (egui) Specification

**Display backends: Wayland-first, X11 second.** The frontend registers the
`io.github.easysearch.EasySearch` app id with the compositor (it matches the
installed desktop entry, so the launcher and window icon line up). winit's selection is
built-in and already Wayland-first: `WAYLAND_DISPLAY` set → native Wayland (preferred,
since an X11 display can exist under Wayland via XWayland), only `DISPLAY` set → X11
fallback. Force X11 by launching with `WAYLAND_DISPLAY` unset. See
[`ui.md`](ui.md) for the user-facing guide; this section is the design summary.

Layout, top to bottom (the “FileSearch Pro” reference in `ui-screenshots/main1.png`):

- **Menu bar:** File · Search · Filters · Tools · Settings · Help.
- **Search tab strip:** one pill per open search, restored across restarts.
- **Toolbar:** Back/Forward (walking the location history), Home, Index (rebuild),
  Content Search, Regex, Recent, Saved — painted icons, active states.
- **Search row:** the query field, a scope picker (Filenames / Full path / Contents), a
  location picker, and the primary `Search` button.
- **Filter bar:** Type, Size, Modified, Path, Ext chips, Case, Hidden, Clear Filters.
  Every control drives a real engine filter (§4.1) — nothing decorative.
- **Results header:** `N results · N files indexed · N ms`, `Sort by`
  (Relevance/Name/Size/Modified/Created) and Cozy/Compact row density.
- **Three panes:** sidebar (a filter box, categories with live counts, saved searches,
  indexed locations, Advanced Search) · results table (`#`, Name with breadcrumbs, Path,
  coloured Type pill, Size, Modified, Created, Match, Relevance, bulk-selection checkboxes) ·
  right panel (Preview / Details tabs, with MIME type, permissions, creation time and an
  on-demand SHA-256, plus Quick Actions).
- **Footer:** view tabs (Results / Preview / Details / Search History), bulk actions
  (Select All, Invert, Copy paths), recent-search chips, and a live status bar
  (index state, system CPU and RAM, query stats, 100/110/125% zoom, keyboard hints).
- **Interactions:** double-click or Enter → open with the default app (`xdg-open`);
  right-click → Open, Open containing folder, Open in terminal, Copy path, Filter to this
  folder. Relevance is a transparent heuristic: which query terms hit the *name* and how
  (exact > prefix > substring), then path hits, with a small bonus for short names. In
  fuzzy mode the same column is driven by the engine's subsequence score
  (word-start/runs up, gaps down, short names preferred).
- **Theming:** follows the desktop's light/dark scheme live (KDE `kdeglobals`, GTK
  settings, or the XDG portal) and uses the system UI/mono fonts. Empty state suggests
  what to search or reports indexing progress.
- CLI mirror (`easysearch-cli search "*.pdf"`) prints matched paths for scripting.

## 11. Project Layout (Cargo workspace)

```
easysearch/
├── docs/                      ← scope.md (this), ui.md, config.md, api.md,
│                                sqlite.md, packaging.md, pending.md
├── README.md                  ← install, usage, configuration, footprint
├── Makefile                   ← build/test/release/run, packaging, dist
├── scripts/                   ← install-deps.sh, package-{deb,rpm,flatpak}.sh,
│                                gen-cargo-sources.py
├── packaging/                 ← desktop entry, AppStream metainfo, icons,
│                                deb control template, rpm spec, flatpak manifest
├── ui-screenshots/            ← design references and captures
├── Cargo.toml                 ← workspace
├── core/                      ← library: walker, watcher, matcher, content,
│   └── src/                     content_index, disk_index (mmap), sqlite_index,
│                                engine, backend, remote, update, config, api
├── app/                       ← the single binary users run (GUI + daemon mode)
├── gui/                       ← eframe/egui frontend (+ the system tray, control socket)
├── cli/                       ← clap frontend
├── daemon/                    ← tiny_http frontend: owns the index
└── vendor/                    ← the arrayref shim (see Cargo.toml)
```

## 12. Delivery Phases

| Phase | Status |
|---|---|
| **1 — MVP** (cold walk + live index + name/content search + GUI/CLI + config) | **Done** — shipped in v0.1.0 |
| **2 — Polish** | **Done**: daemon + HTTP API, tray icon, settings dialog, tabs and session persistence, saved searches, light/dark following, packaging (.deb/.rpm/Flatpak metadata), `.gitignore` management UI (v0.16.0), fuzzy ranking (v0.17.0), bundled docx extractor (v0.18.0), global hotkey via the control socket (v0.19.0) |
| **3 — Stretch** | **Partly done**: **PDF / ODT extraction (v0.20.0)**, **multiline content regex (v0.21.0)**. **Outstanding**: `fanotify` watcher, Windows/macOS builds |
| **4 — SQLite index** | **Done** — v0.13.0. See [`sqlite.md`](sqlite.md) |
| **5 — Packaging** | **Partly done**: `.deb` builds and verifies; RPM and Flatpak are written but have never been built (tools unavailable here). See [`packaging.md`](packaging.md) |

What remains is tracked, itemised and prioritised in [`pending.md`](pending.md) —
that document, not this one, is the live backlog.

## 13. Risks & Open Questions

**Risks (mitigations in place):** inotify watch exhaustion (§5.2 degraded mode); first
`cargo build` is slow (crate compile; mitigated by release profile + `lto`); crates.io
network access needed for dependencies (verify at first build; Debian's packaged crates
are a fallback); document extraction cost (docx/odt/pdf files, only when content search
is on).

**Decisions — all resolved 2026-08-20 (user confirmation):**
1. **Stack:** Rust + egui + embedded ripgrep crates + `notify` — **confirmed**. (Slint as
   GUI fallback, Go as language fallback, both documented but not pursued.)
2. **Apt installs:** one-time `sudo apt install rustc cargo build-essential pkg-config
   libxkbcommon-dev libwayland-dev libgl1-mesa-dev` — **approved**. (No `docx2txt`:
   docx extraction is in-process since v0.18.0.)
3. **Default scope:** **`$HOME` of the running user**, configurable roots. Whole-FS is an
   opt-in config change, not the default.
4. **Content search:** on-demand embedded ripgrep (always fresh) is the default **and** an
   **optional background content index** (default off) is in scope.
5. **Removable/USB mounts:** ignored by default; opt-in via config.
6. **SQLite index (v0.13.0) — reverses the original §3.1 decision.** The 2026-08-20
   analysis rejected SQLite as “heavier” in favour of a custom mmap format. That was
   right for the phase-1 goal (fastest possible filename scan) but it left the index as
   a bespoke binary format with no real query language, so every new filter meant new
   scan code and the GUI could not ask questions of the index. SQLite was adopted on
   user direction in order to get durability, SQL pushdown for filters, and a foundation
   for FTS5 content search. The mmap backend is retained behind `storage = "mmap"`, and
   the honest trade-off (bare name queries are now slower; filtered ones are faster) is
   recorded in [`sqlite.md`](sqlite.md).

---

*References: VoidTools Everything; `ripgrep` (BurntSushi, crates `grep-searcher` /
`ignore` / `globset` / `regex`); `notify` crate; `egui`/`eframe`; kernel `fs.inotify.*`;
Baloo/Recoll/Tracker documentation (as counter-examples).*
