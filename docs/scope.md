# Everything for Linux — Scope

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
  and **Word `.docx`** documents.
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
- Binary file content search (PDF, images, video, archives) — text/docx only for now.
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
┌───────────────────────────  single process, one binary family  ───────────────────────────┐
│                                                                                             │
│   core/  (library crate: all search logic)                                                  │
│   ├── indexer ── ignore::WalkParallel (cold walk) ──▶ in-memory path index (RAM)            │
│   │      ▲                                                     │                             │
│   │      └── notify crate (inotify) ◀── kernel events          │  create/delete/rename/edit │
│   │                                  (create/delete/rename/edit) ▼                           │
│   ├── matcher ── globset (pattern mode) + regex crate (regex mode) over indexed paths       │
│   ├── content  ── grep-searcher + grep-regex + ignore (ripgrep engine, in-process)          │
│   │                 └─ preprocessor hook (docx extraction)                                   │
│   ├── content_index (OPTIONAL, default off) ── background extractor + RAM text cache        │
│   │                 (bounded, LRU; kept fresh by the same watcher events)                    │
│   └── events   ── channel: watcher → index (single source of truth, lock-free reads)        │
│                                                                                             │
│   gui/  (eframe/egui — native, no web tech) ── links core ──▶ zero-IPC, instant search      │
│   cli/  (clap)                              ── links core ──▶ scripted search / status       │
│   daemon/ (tiny_http, optional)             ── links core ──▶ localhost HTTP API             │
│                                                                                             │
│   Single binary per frontend; core is a library so GUI/CLI/daemon share one codebase.       │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **No IPC in the default path**: GUI and CLI link `core/` directly — the index lives in
  the same process, so queries are memory-speed with no serialization.
- The optional `daemon/` exists only for scripted/remote use (binds `127.0.0.1`, read-only).
- The GUI and the daemon can run simultaneously because the index is a `core/` data
  structure each process builds for itself; no shared-state contention.

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
| Content text cache (optional) | in-RAM `HashMap` + byte-cap LRU (no DB) | SQLite/FTS (rejected: heavier, phase 3 if ever) |
| GUI | **`eframe`/`egui`** (immediate-mode, native, Win/Linux/macOS) | Slint, iced, gtk4-rs (all viable; egui = minimal deps + instant re-render per keystroke) |
| CLI | **`clap`** | hand-rolled parser |
| Optional daemon HTTP | **`tiny_http`** (small, stdlib-ish) | axum (heavier) |
| Serialization (daemon only) | **`serde_json`** | — |
| `.docx` extraction | external `docx2txt` subprocess (preprocessor hook) | bundled zip+XML extractor (phase 3, removes the dep) |

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
- Pattern matching runs over the in-memory index → sub-50 ms at 1M entries.

### 4.2 Content search

- A separate toggle enables content matching. When on, the query's terms are fed to the
  **embedded ripgrep engine** (`grep-searcher`), constrained to:
  - the current filename-filtered result set (content **and** name → intersection), and
  - the user's scope root (default: whole FS minus exclusions, §6).
- Regex in content mode uses ripgrep's regex engine (same syntax as `rg`; multiline via
  `-U`-style flags in a later phase).
- **Realtime guarantee for content:** results are always computed from live disk state at
  query time. Editing a file then re-running the query immediately shows the change.
- `.docx` content: a **preprocessor hook** extracts text (default: `docx2txt`
  subprocess; phase-3 bundled fallback) so Office files behave like text. If the
  extractor is unavailable, docx files are skipped and the UI notes it.
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
  Additional roots configurable in `~/.config/everything-linux/config.json`.
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
| `.docx` text extraction | **`docx2txt`** subprocess via preprocessor hook | apt-installable; bundled fallback in phase 3 |
| Open results | **`xdg-open`** | Desktop-standard "open file / containing folder" |
| Build toolchain | **apt** `rustc`/`cargo` 1.85 (or rustup) + `build-essential`, `pkg-config`, X11/Wayland/GL dev libs | Verified available on target; one-time `sudo apt install` |

**Explicitly rejected:** Baloo/Recoll/Tracker (heavy daemons, content DBs), a custom
`find`-based reimplementation, subprocess-per-query `rg`, a hand-rolled content indexer.

> If a required tool is missing at runtime, the app **degrades gracefully** (docx
> skipped) and surfaces the exact `apt install` command needed.

---

## 8. Optional Daemon HTTP API (localhost only)

`tiny_http`, bound to `127.0.0.1`, port `24123` by default. Read-only; no auth on loopback.
The GUI and CLI do **not** use this by default (they link `core/` directly).

| Endpoint | Purpose |
|---|---|
| `GET /api/status` | index size, roots, watcher state (live/degraded), skipped-dir counts |
| `GET /api/search` | `name`, `name_regex`, `content`, `case`, `hidden`, `path_match`, `limit`, `scope` → `{results:[{path,size,mtime,is_dir}], total, truncated}` |
| `POST /api/index/rebuild` | full rescan (e.g. after system restore) |

## 9. Resource Footprint Budget (non-negotiable targets)

| Metric | Target |
|---|---|
| Binary size (stripped, `lto`+`strip`) | **< 10 MB** per binary |
| Backend idle RSS (no GC, no runtime) | **≈ 8 MB + ~150–250 B per indexed path** |
| Backend idle CPU | **≈ 0 %** (event-driven; no polling loops) |
| Cold index, 1M files | < 30 s; searchable from first second |
| Filename query latency (1M entries) | < 50 ms |
| Content query (typical tree) | first result < 2 s; results streamed, cancellable |
| Typical `/home` (≈300k files) | ≈ 60–90 MB total RSS |
| Content index (when **enabled**) | + extracted text only (≤ 256 MB cap, LRU); **0 MB / 0 CPU when off** |

## 10. GUI (egui) Specification

- **Search box** (top, focus-on-start): type → debounced live results (Everything-style).
- **Toggle row:** regex | content | case-sensitive | hidden files | full-path match.
- **Results list** (egui table/selectable rows): name, full path, size, mtime, type icon;
  results stream in as computed.
- **Status bar:** `12,438 files indexed · live · 0.02 s` + degraded-mode indicator +
  content-index state (`off` / `indexing… 42%` / `cached`).
- **Interactions:** double-click → open with default app (`xdg-open`); right-click menu →
  open containing folder, copy path. Keyboard: ↑/↓/Enter.
- Empty state: "Indexing… 68%" or "No results".
- CLI mirror (`everything search "*.pdf"`) prints matched paths for scripting.

## 11. Project Layout (Cargo workspace)

```
everything-for-linux/
├── docs/scope.md              ← this document
├── README.md                  ← install (build deps), usage, sysctl notes
├── scripts/install-deps.sh    ← apt: rustc cargo build-essential pkg-config
│                                libxkbcommon-dev libwayland-dev libgl1-mesa-dev docx2txt
├── Cargo.toml                 ← workspace
├── core/                      ← library: indexer, watcher, matcher, content searcher
│   └── src/{lib,indexer,watcher,matcher,content,config}.rs
├── gui/                       ← eframe/egui frontend (links core)
│   └── src/main.rs
├── cli/                       ← clap frontend (links core)
│   └── src/main.rs
├── daemon/                    ← optional tiny_http frontend (links core)
│   └── src/main.rs
└── tests/                     ← integration tests (temp dirs, fake FS events)
```

## 12. Delivery Phases

| Phase | Contents | Exit criteria |
|---|---|---|
| **1 — MVP** | `core/`: cold walk (`ignore`) + live index (`notify`) + name search (glob+regex via `globset`/`regex`, AND/`!`, basename/full-path, hidden toggle) + content search (embedded `grep-searcher`, text/code, docx via preprocessor) + **optional background content index** (default off) + degraded mode. `gui/` (egui) with all toggles. `cli/`. Config file (default root `$HOME`, USB/external mounts excluded). | Searches `$HOME` live; create-a-file-then-search finds it in < 1 s; footprint budget met; builds on Debian 13 |
| **2 — Polish** | Bundled docx extractor (drop `docx2txt` dep); daemon + HTTP API; tray icon + global hotkey; substring ranking (fzf-style); settings dialog; `.gitignore` handling UI | Feedback from real daily use |
| **3 — Stretch** | fanotify watcher (no per-dir watch limits, needs privileges); multiline content regex; PDF/ODT extraction; fuzzy filename ranking; Windows/macOS builds | Only if requested |

**Phase 1 is the committed deliverable. Phases 2–3 are gated on user request.**

## 13. Risks & Open Questions

**Risks (mitigations in place):** inotify watch exhaustion (§5.2 degraded mode); first
`cargo build` is slow (crate compile; mitigated by release profile + `lto`); crates.io
network access needed for dependencies (verify at first build; Debian's packaged crates
are a fallback); docx extraction cost (only docx files, only when content search is on).

**Decisions — all resolved 2026-08-20 (user confirmation):**
1. **Stack:** Rust + egui + embedded ripgrep crates + `notify` — **confirmed**. (Slint as
   GUI fallback, Go as language fallback, both documented but not pursued.)
2. **Apt installs:** one-time `sudo apt install rustc cargo build-essential pkg-config
   libxkbcommon-dev libwayland-dev libgl1-mesa-dev docx2txt` — **approved**.
3. **Default scope:** **`$HOME` of the running user**, configurable roots. Whole-FS is an
   opt-in config change, not the default.
4. **Content search:** on-demand embedded ripgrep (always fresh) is the default **and** an
   **optional background content index** (default off) is in scope.
5. **Removable/USB mounts:** ignored by default; opt-in via config.

---

*References: VoidTools Everything; `ripgrep` (BurntSushi, crates `grep-searcher` /
`ignore` / `globset` / `regex`); `notify` crate; `egui`/`eframe`; kernel `fs.inotify.*`;
Baloo/Recoll/Tracker documentation (as counter-examples).*
