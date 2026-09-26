# Changelog

All notable changes to **EasySearch** are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.25.0] - 2026-09-26

### Added

- **The *Full text* scope: the query matches the file name *or* its contents.**
  Previously a content query required the name to match as well (name AND
  content), which the reference UI's “Full Text (content + name)” scope could not
  express. Now the scope picker has four entries — *Filenames*, *Full path*,
  *Contents*, *Full text (name or contents)* — and the engine evaluates both
  halves and merges them, name matches first. Also `--any` on the CLI and
  `any`/`content_or_name` on the HTTP API, and `Query.content_or_name`.
- `engine::without_name()` and `engine::merge_or_hits()` keep the two backends
  (mmap and SQLite) sharing one implementation of the merge.

### Notes

- The sidebar's per-category facet counts stay name-based for *Full text*
  queries (counting the content half would mean running a content search per
  category per keystroke); that was already true for content queries.

## [0.24.0] - 2026-09-26

### Changed — the project is now **EasySearch**

- **Renamed everywhere.** The name `Everything for Linux` (and the awkward
  `everything-linux` binary) is gone, so the project no longer reads as a clone
  of someone else's product name. References to *VoidTools' Everything* as the
  thing this is a Linux equivalent of are kept — that is a description of the
  reference app, not of this project.

  | Before | After |
  | --- | --- |
  | `everything-linux` (single binary) | **`easysearch`** |
  | `everything` (CLI) | **`easysearch-cli`** |
  | `everything-gui`, `everything-daemon` | `easysearch-gui`, `easysearch-daemon` |
  | crates `everything-{core,gui,daemon,app}` | `easysearch-{core,gui,daemon,app}` |
  | Rust `everything_core::…` | `easysearch_core::…` |
  | `io.github.everythinglinux.EverythingForLinux` | `io.github.easysearch.EasySearch` |
  | `~/.config/everything-linux/`, `~/.cache/everything-linux/` | `~/.config/easysearch/`, `~/.cache/easysearch/` |
  | `$XDG_RUNTIME_DIR/everything-linux.sock` | `$XDG_RUNTIME_DIR/easysearch.sock` |
  | `EVERYTHING_*` environment variables | `EASYSEARCH_*` |

- **This is a breaking change for anyone who had it installed** (binary names,
  the config/cache directory, the Wayland app id, the environment variables and
  the control socket all move). It is done *before* the first public release, so
  there is no migration path — the old paths are simply forgotten.
- The four packaging files whose *filenames* carry the app id
  (`packaging/common/…desktop`, `…metainfo.xml`, `packaging/flatpak/…yml`,
  `packaging/icons/…svg`) and the RPM spec were renamed to match.
- `scripts/`, the `Makefile`, all seven documents and the Flatpak manifest were
  updated together; the app id is now consistent across all of them (see
  `docs/pending.md` §2 for what is still a placeholder — the repository URL).

### Added

- **A live switch for the background content index** (the last of the documented
  "no UI switch" rough edges). **Settings ▸ Indexing ▸ Background content index**
  toggles it, or `POST /v1/config` with `{"content_index":true|false}` for the
  daemon; the state is reported in `/v1/status` and `Engine::set_content_index`
  is the entry point.
- The switch owns the memory: `ContentIndex::set_enabled(false)` clears the cache
  and its order book, so "off" really is zero bytes rather than a cache that
  merely stops growing — which is what `docs/scope.md` §9's budget promises.
- `ContentIndex` and `ExtractQueue` now share one `Arc<AtomicBool>` (the queue's
  `send` is a no-op while off), so the two can never disagree. The extractor
  thread is created once and simply stays parked while the cache is off.
- `ContentIndex::clear()` is public (also used internally by `set_enabled`).

## [0.22.1] - 2026-09-26

### Changed

- **`cargo clippy --workspace --all-targets` is clean** (it had ~30 standing
  style warnings). Mostly `cargo clippy --fix`: collapsible `if` chains,
  `map_or(false, ..)` → `is_some_and`, `map_or(true, ..)` → `is_none_or`,
  `% 2 != 0` → `!is_multiple_of(2)`, a derived `Default` for `Category`, elided
  lifetimes, and test configs built with struct-update syntax instead of
  reassignment. The two long internal signatures (`rebuild_once`,
  `start_watcher`) keep their parameters and carry a documented `#[allow]`,
  because bundling them would only move the list.

## [0.22.0] - 2026-09-26

### Added

- **Follow symbolic links** (the last of the reference UI's advanced options).
  Off by default — it can duplicate whole subtrees — but when on, the contents of
  a symlinked folder are indexed too. The walker detects and skips cycles.
- Walk settings are now one value: `walker::WalkOptions { respect_ignore,
  follow_symlinks }` replaces the lone `respect_ignore: bool` threaded through
  every walker/watcher signature, so the next option will not ripple through
  them again.
- Live toggle for it, exactly like the ignore setting:
  `Engine::set_follow_symlinks`, reported as
  `Status::follow_symlinks`, changed by the GUI checkboxes in **Tools ▸ Ignore
  files…** and **Settings ▸ Indexing**, and set over the API by
  `POST /v1/config` (`/v1/ignore` is now an alias of it, and every field of the
  body is optional). `Config::follow_symlinks` sets the default.

## [0.21.0] - 2026-09-26

### Added

- **Multiline content regex (Phase 3).** A content pattern can now span lines —
  `--content 'foo\nbar' --multiline` finds a file where `foo` ends one line and
  `bar` begins the next. Off by default, because it makes the searcher buffer
  whole files instead of scanning line by line, which is much slower. Available
  as `Query.multiline`, `--multiline` in the CLI, `?multiline=1` on the HTTP API,
  and the **Multiline** chip in the filter bar (shown in content mode).

## [0.20.0] - 2026-09-26

### Added

- **OpenDocument (`.odt`) and PDF content search (Phase 3).** Both are read
  in-process, with no external tool.
  - `.odt` reuses the document-package reader: an ODF file is a ZIP of XML too,
    so the extraction is now table-driven — one set of element rules for
    WordprocessingML (`<w:t>`, `<w:p>`, `<w:tab/>`, …) and one for OpenDocument
    (`<text:p>`, `<text:h>`, `<text:line-break/>`, `<text:s/>`, table cells).
  - `.pdf` uses the pure-Rust `pdf-extract` crate for the **text layer**. A
    scanned, image-only PDF has no text layer, which is a property of the file.
    PDF parsing is the least predictable input here, so it runs under
    `catch_unwind`: a malformed document skips one file and never takes down a
    query or the extraction worker.
- `content_index::needs_extraction()` and `EXTRACT_EXTENSIONS` describe the
  formats that need extraction; `content.rs` and the index use them, so the two
  paths cannot disagree.
- `packaging/flatpak/cargo-sources.json` regenerated for the new dependencies.

## [0.19.0] - 2026-09-26

### Added

- **A global hotkey (Phase 2), in the only way that works on Wayland.** Wayland
  has no global-hotkey API and every desktop invents its own, so instead of a
  privileged hook the running window listens on
  `$XDG_RUNTIME_DIR/easysearch.sock` (mode `0600`) and a second invocation
  drives it:

  ```sh
  easysearch --toggle        # show if hidden, hide if visible
  easysearch --show          # bring to the front
  easysearch --hide
  easysearch --search TODO   # open and search
  easysearch --quit          # exit (the index daemon keeps running)
  ```

  Bind one as a **custom shortcut** in the desktop's own settings (KDE, GNOME,
  Sway/i3/Hyprland — see `docs/ui.md`). The first `--toggle` with nothing running
  *starts* the app, so one key launches it and then hides/shows it. Starting the
  app is the only fall-through: the other commands are a quiet no-op when nothing
  is running. A second GUI never steals the socket, and a stale file left by a
  crash is cleaned up.
- `--help` documents the control commands.

## [0.18.0] - 2026-09-26

### Changed

- **`.docx` content search is fully in-process (Phase 2).** The external
  `docx2txt` tool is no longer needed or used. The engine reads the OOXML
  package (a ZIP) itself and extracts the visible text from `word/document.xml`
  plus headers, footers, footnotes, endnotes and comments: `<w:t>` run text
  (and deleted `<w:delText>`), tabs, line breaks and table cells become
  whitespace that keeps words separated, and character entities (`&amp;`, `&#…;`)
  are resolved. Before, a missing `docx2txt` made docx content search silently
  return nothing; now it works out of the box, and a malformed package is
  skipped rather than fatal.
- New dependencies `zip` (the `deflate` feature only) and `quick-xml`, both pure
  Rust. `packaging/flatpak/cargo-sources.json` was regenerated for the offline
  Flatpak build.
- `scripts/install-deps.sh`, the README and `scope.md` no longer ask you to
  install `docx2txt`.

## [0.17.0] - 2026-09-26

### Added

- **Fuzzy (fzf-style) searching (Phase 2).** With the **Fuzzy** toolbar button
  (or **Search ▸ Fuzzy matching**, or **Settings ▸ Search**) a term matches when
  its characters appear *in order* anywhere in the target, so `mtn` finds
  `meeting-notes.md` and `qr` finds `quarterly-report-2026.pdf`. It is a filter
  *and* a ranking: the Relevance column and the Relevance sort switch to a
  transparent subsequence score (word-start and contiguous-run bonuses, a gap
  penalty, a small length preference), so the tightest, earliest match sorts
  first. `!term` exclusions deliberately keep their literal substring meaning.
- `Query.fuzzy` in the engine, `--fuzzy` in the CLI, and `fuzzy` in both HTTP
  API forms (`?fuzzy=1` and the `Query` field).
  `easysearch_core::matcher::fuzzy_score` is public for callers that want the
  score itself.

### Notes

- Fuzzy is a **global mode** (like *Include folders*), not per-tab like Regex.
  Matching is a subsequence test, so it cannot be pushed into the SQL index; the
  SQLite backend still applies every coarse filter in SQL and runs the
  subsequence test in Rust over the survivors.

## [0.16.0] - 2026-09-26

### Added

- **An ignore-files UI (Phase 2).** **Tools ▸ Ignore files…** edits the global
  ignore list (`~/.config/easysearch/ignore`, `.gitignore` syntax) with
  **Save & rebuild** / **Reload** / **Rebuild index**, and **Settings ▸ Indexing**
  toggles whether `.gitignore`/`.ignore` files are honoured at all.
- **The toggle is live.** `Engine::set_respect_ignore` updates an atomic flag and
  rebuilds; the daemon exposes it as `POST /v1/ignore` and reports the current
  value as `status.respect_ignore_files`, so the GUI can change it on a remote
  daemon and read back the truth. Previously the setting could only be changed by
  hand-editing `config.json` and restarting.
- `easysearch_core::walker::global_ignore_file()` exposes the ignore-file path.

## [0.15.1] - 2026-09-26

### Fixed

- **An empty file no longer reads as a failed preview.** The preview pane said
  “No text preview for this file.” for a 0-byte text file; it now says “This file
  is empty (0 bytes)”, and a file whose bytes are not text says “No text preview
  for this binary file.” A NUL/dense-control heuristic decides which, and it
  tolerates a multi-byte character split by the 64 KiB read cap.

## [0.15.0] - 2026-09-26

### Added

- **In-place self-updates over the internet** — no `.deb`/`.rpm`, no package
  manager, no reinstall. A release publishes a signed `manifest.json` (plus a
  detached Ed25519 signature); the app fetches it over HTTPS only, verifies the
  signature against a public key compiled into the binary, checks each download's
  SHA-256, and `rename()`s the new binaries over the running ones (atomic, and
  safe while the old binary is executing). Every step fails closed: no key, no
  signature, a non-HTTPS URL, a bad hash, or a not-newer version all mean
  *nothing is written*. Assets are raw binaries, so there is no archive to
  unpack. See [`docs/updates.md`](docs/updates.md).
- **GUI: Help ▸ Check for updates…** opens a *Software update* window (release
  notes, progress bar, **Install update**, **Restart now**). A quiet check runs at
  launch at most once a day, and a `⬆ v… available` badge appears beside the
  version in the status bar. Both are configurable in **Settings ▸ Updates**.
- **CLI: `easysearch-cli self-update`** (`--check`, `--yes`, `--manifest`,
  `--pubkey`, `--dir`, `--restart-daemon`).
- **`scripts/release-sign.sh`** builds the manifest from `target/release/*`,
  signs it with `openssl`, and verifies its own signature before you ship it.
- **`POST /v1/shutdown`** on the daemon, so an in-place update can stop the old
  daemon and let the relaunched app start the new one.
- **The running version in the status bar** (bottom-right, next to the hints).
- **Focus follows the pointer.** Clicking anywhere outside the search field now
  drops its keyboard focus, so ↑/↓/PgUp/PgDn/Enter drive the results list; typing
  any printable key immediately re-grabs the field and starts a new query.

### Changed

- The status bar's right-hand block now shows the version, and — when one is
  known — a click-through badge for the available update.

## [0.14.1] - 2026-09-26

### Fixed

- **A stale multi-selection.** The checked set was never pruned when the result
  list changed, so after a new query or a tab switch the header kept reporting
  “N selected” and Copy paths / Find duplicates in selection acted on files that
  were no longer on screen. Selected paths not in the current results are now
  dropped whenever results are assigned (and a query with no matches clears it).
- **Two different numbers both labelled “files”.** The results header said
  `N files indexed` using files **+ folders**, while the status bar said
  `(N files)` using files only. The header now counts files, and the status bar's
  total is labelled `Entries:` so the two can't be confused.
- The duplicates window could look stuck on “comparing…” if egui was otherwise
  idle when the scan finished; the UI now keeps repainting while a scan runs, and
  progress is reported every 64 files instead of once per file (a 50 000-file
  scan was sending 50 000 messages).

## [0.14.0] - 2026-09-26

### Added

- **A full right-click menu on results**, which acts on the whole multi-selection
  when the clicked row is part of it and on that row otherwise: Open · Open
  containing folder · Open in terminal · Copy path(s) · Copy name(s) · Show in
  Details panel · Filter to this folder · Search for this name · Add to / Remove
  from selection · Select all · Invert · Clear selection · and the duplicate
  scans below.
- **Find duplicates in results** (and *in selection*): groups files with
  identical contents and shows what is reclaimable, largest waste first, with
  Select-in-results / Copy paths / Reveal per group. Three passes, cheapest
  first — group by size, then compare size + the first 64 KiB, then hash the
  survivors in full (SHA-256) — so the expensive pass runs over the smallest set.
  Files sharing a long prefix but differing later are correctly *not* reported.
  Empty files are ignored, the scan runs on a background thread with progress,
  and it is capped at 50 000 candidates (the window says “scan capped”). Nothing
  is ever deleted for you.
- `sha2` is now a dependency, so hashing happens in-process.

### Changed

- The Details panel's SHA-256 no longer shells out to `sha256sum`; it is computed
  in-process, which also removes a coreutils dependency at runtime.
- “Filter to this folder” previously copied the folder path to the clipboard
  instead of filtering; it now sets the location filter as its name promises.

## [0.13.2] - 2026-09-26

Documentation brought back in line with the code — the design documents still
described the v0.1.0 architecture (a memory-mapped index and a three-pane UI).

### Added

- **`docs/ui.md`** — a guide to the GUI: the layout, query syntax, what every
  filter maps to, the columns, the (now written-down) relevance heuristic,
  saved searches, the shortcut table, theming, and the known limits.
- **`docs/config.md`** — every `config.json` key with its default and effect,
  plus the list of related files and their locations.
- A **Documentation** index in the README.

### Changed

- **`docs/scope.md`**: the architecture diagram and notes now describe the daemon
  owning a SQLite index with clients reading through it (the default path *does*
  use IPC, deliberately); §4 gained the filter dimensions (location, extension,
  size, recency); §9's storage rows and memory paragraph describe SQLite; §10 is
  a design summary of the current GUI; §11 the real project layout; §12 records
  phase status. The reversed SQLite decision is written up in §13 rather than
  deleted, with the reason.
- **`README.md`**: the UI description, the index location and the configuration
  section (which now documents `storage` and `db_dir`).

### Fixed

- Three README links pointed at `ui.md`/`sqlite.md`/`config.md` without the
  `docs/` prefix, and the in-app shortcut list was missing `Ctrl+A`.

## [0.13.1] - 2026-09-25

### Fixed

- **The three UI zoom levels (100% / 110% / 125%) are visible again.** They had
  moved to the far right of the status bar, where the keyboard-hints string could
  push them out of the window; they now sit on the left beside the CPU/RAM
  readout, and there is also a `Zoom` section in the Settings menu.

## [0.13.0] - 2026-09-25

### Added

- **The index now lives in SQLite** (`$XDG_CACHE_HOME/easysearch/db/index.db`).
  The daemon owns the database — it is the only writer — and keeps it current
  from kernel filesystem events, folding each batch into one transaction; every
  query (GUI, CLI, HTTP API) is answered from the database. See
  [`docs/sqlite.md`](docs/sqlite.md).
  - **Created when missing**, or when `schema_version` changes, or when a
    previous build was interrupted (a `complete` marker is written inside the
    same transaction as the rows, so a partial index can never be mistaken for
    a small complete one).
  - **Refreshed** when the delta backlog passes `REFRESH_AFTER_DIRTY`
    (20 000 changes): a full rebuild is cheaper and safer than replaying a very
    long delta stream.
  - **One predicate.** The coarse filters (directory prefix, extension, size,
    mtime, hidden, files-only) are pushed into SQL because they are exactly the
    conditions the engine already applies; the name and category predicates then
    run through the same `accepts()` the mmap backend uses, so the two backends
    cannot disagree. A test asserts `count` == `search().len()` for every shape.
  - New config: `storage` (`sqlite` | `mmap`, default `sqlite`) and `db_dir`.
    `storage = "mmap"` keeps the original memory-mapped index; RAM-only mode
    (`persist_index = false`) deliberately does not open a disk database.
  - `rusqlite` with the `bundled` SQLite, so no system `libsqlite3` or
    `pkg-config` is required at build or run time.

### Changed

- Queries with filters are now answered by SQL pushdown: `--ext toml`,
  `--min-size 10K` and `--modified-within 7d` return in 1–2 ms on a 100k-file
  index (measured). Unfiltered name queries stream the table through the shared
  predicate, which is slower than the mmap scan — hence the `mmap` escape hatch.
- The CLI's status line no longer claims the index is `mmap-backed`.

## [0.12.0] - 2026-09-25

A UI overhaul to the “FileSearch Pro” reference design (see
`ui-screenshots/main1.png`): the dense single-bar window is replaced by a menu
bar, a labelled toolbar, a search row, a filter bar and a results header, over a
three-pane body and a view/status footer.

### Added

- **Labelled toolbar** — Back/Forward (walking the location history), Home,
  Index (rebuild), Content Search, Regex, Recent and Saved, each with a
  hand-painted icon (no icon font, no emoji) and an active state.
- **Search row** — the query field, a scope picker (Filenames / Full path /
  Contents), a location picker, and a primary `Search` button.
- **Filter bar** — Type, Size, Modified, Path, Ext (with extension chips), Case,
  Hidden and `Clear Filters`. These drive real engine filters: extensions,
  inclusive size bounds and modified-within recency.
- **Results header** — `N results • N files indexed • N ms`, a `Sort by` menu
  (Relevance, Name, Size, Modified) and Cozy/Compact row density.
- **New result columns** — `#`, Name (two-line with breadcrumbs in Cozy), Path,
  a coloured Type pill, Size, Modified, `Match` (which query terms this hit
  matched) and `Relevance` (a 0–100 score with a bar). Row numbers and
  bulk-selection checkboxes are included.
- **Preview panel** — Preview/Details tabs. Details shows Name, Path, Size with
  exact bytes, Modified, Created (filesystem birth time), MIME type,
  Permissions and SHA-256 (computed on demand for files up to 512 MB, via
  `sha256sum`). Quick Actions: Open, Reveal, Copy path, Terminal.
- **View tab strip and bulk actions** — Results / Preview / Details / Search
  History, with Select All, Invert and Copy paths over the checkbox selection.
- **Recent searches** chip row, and **saved searches** (toolbar, Tools menu and
  sidebar) persisted in the GUI preferences.
- **Sidebar sections** — a “Search Everywhere” box that filters the lists, the
  categories with live counts, saved searches, indexed locations, and Advanced
  Search (Include folders, plus the content-index state).
- **Status bar** — index state, live system CPU and RAM (from `/proc`), query
  result count, search time, indexed entries, UI zoom and keyboard hints.
- `Ctrl+A` selects every result row.

### Changed

- The old in-bar `.*`/`Aa` toggles, the bottom zoom/type/folders strip and the
  per-row hover actions are gone; they are replaced by the toolbar, the filter
  bar, the status-bar zoom and the row context menu (which still offers Open,
  Open containing folder, Open in terminal and Copy path).

### Fixed

- The element that removed the old bottom strip also closed the `impl App`
  block; the file no longer fails to parse.

## [0.11.0] - 2026-09-24

### Added

- **Debian packages** — `make deb` stages the release binaries, the desktop
  entry, the AppStream metainfo and the icons and builds a `.deb` with
  `dpkg-deb`, needing no root. `Depends` combines `dpkg-shlibdeps` output with
  the GUI libraries the binary `dlopen`s (mapped to real package names), so the
  dependency list is complete rather than guessed.
- **RPM packages** — `make rpm` builds the spec from a source tarball with
  `rpmbuild`; the `dlopen`ed libraries are required by soname
  (`libEGL.so.1()(64bit)`), which is exact and distribution-independent.
- **Flatpak** — a manifest plus `cargo-sources.json` generated from
  `Cargo.lock`, so the sandbox builds offline against vendored crates.
- **Shared packaging assets**: a desktop entry, AppStream metainfo, and a flat
  SVG app icon (rendered to PNG at build time).
- **A `LICENSE` file** (MIT), which the packages install as their copyright
  file. The copyright holder line is a placeholder.
- New Makefile targets: `deb`, `rpm`, `flatpak`, `packages`, `cargo-sources` and
  `validate-packaging`. See `docs/packaging.md`.

### Changed

- The packages ship `easysearch` (GUI + daemon in one binary), the CLI and
  a headless daemon. The separate `easysearch-gui` binary is a development
  convenience that duplicated `easysearch` and added ~17 MB, so it is no
  longer packaged (the deb is ~6.4 MB compressed / ~23 MB installed).

## [0.10.0] - 2026-09-24

### Added

- **Count without rows** — `POST /v1/count` takes the same `Query` object as a
  search and returns just `{"count":n}`, using the same matching predicate so
  the two can never disagree (shared `Engine::count`, `Backend::count`,
  `Remote::count`).
- **Location filter** — `Query.under: Option<String>` restricts results to
  paths inside a directory; exposed as `under` on `GET /v1/search`, as
  `--under <DIR>` in the CLI, and as *Locations* chips in the GUI sidebar.
- **A richer sidebar**: a brand header with a live `LIVE`/`INDEXING` pill, two
  tiles for *matches* / *shown* plus the indexed file count, per-category
  result counts, one-click *Locations* chips (the new location filter), inline
  search options, and a collapsible **TIPS** cheat-sheet whose state persists.

### Changed

- **The preview pane is on by default** for new profiles.
- **“Follow system” now tracks the desktop live.** The theme is re-read when the
  desktop rewrites its configuration (`~/.config/kdeglobals`, GTK `settings.ini`,
  dconf, XFCE xsettings) and, as a safety net, every 15 seconds via the XDG
  portal — so switching between light and dark flips the app without a restart.
- **KDE uses `kdeglobals` as the source of truth.** On Plasma, `dark-light`
  could report a light scheme for a dark desktop; the window background colour in
  `~/.config/kdeglobals` is now consulted first, then the portal, then GTK.

### Fixed

- Sidebar and facet counts no longer render above the visible area, and the
  first count request is issued for the initial (empty) query.

## [0.9.0] - 2026-09-24

### Added

- **One file to run: `easysearch`.** A single binary that is both the GUI
  and the search daemon. Run with no arguments it makes sure a daemon is
  listening — re-executing *itself* in `--daemon` mode, detached into its own
  session — and attaches the GUI to it, so the HTTP API keeps serving other
  clients and the index keeps running after the GUI is closed. If a daemon
  cannot be started (for example the port is taken), the GUI falls back to an
  in-process engine instead of failing. The daemon's log goes to
  `$XDG_CACHE_HOME/easysearch/daemon.log`.
- `make dist` copies the shareable single binary to `dist/easysearch`
  (~17 MB); `make run` now launches it and `make install` installs it alongside
  the per-component binaries.

### Changed

- Workspace migrated to Rust **edition 2024** (from 2021). No behavioural changes
  were required apart from `std::env::set_var` becoming `unsafe`: tests now pass
  the child's environment explicitly instead of mutating ours, which is also
  race-free.
- The GUI crate is now a library plus a thin binary, so the combined app reuses
  the same UI; a shared daemon entry point (`run_forever`) backs both the
  standalone daemon and the combined binary.
- `docs/api.md` documents the single-file app and the sharing workflow.

## [0.8.0] - 2026-09-24

### Added

- **The search engine is now its own process.** A new `easysearch-daemon`
  binary owns the index, watcher and content cache and serves them over an
  HTTP/JSON API on `127.0.0.1:5858` (localhost only, no authentication); see
  `docs/api.md`. Endpoints: `GET /v1/health`, `GET /v1/status`,
  `POST|GET /v1/search`, `POST /v1/rebuild`, and `GET /v1/watch` (long-poll).
  Because the engine no longer lives inside the GUI, it keeps running — and other
  clients keep working — when no GUI is running at all.
- `easysearch-gui` is now a client: it attaches to a daemon (`--daemon ADDR`,
  `EASYSEARCH_DAEMON`, or auto-detected on the default address) and falls back to
  an in-process engine when none is running, so it always works. The status bar
  shows the active backend and flags an unreachable daemon in red.
- `easysearch-cli` (CLI) gained `--remote ADDR` to query a daemon instead of
  indexing locally.
- `core` gained `Backend` (in-process vs. daemon behind one interface), a
  dependency-free HTTP/1.1 client (`core::remote`, std only, with chunked
  decoding) and `core::api` wire types. `Query`, `Status` and related engine
  types now derive serde.
- `make daemon` / `make daemon-dev`; the daemon ships in `make install`.

### Changed

- `docs/scope.md` §8 documents the implemented API instead of a plan.

### Notes

- Change notification is long-polling (`/v1/watch`) rather than SSE/WebSocket:
  `tiny_http` streams through a chunk encoder that buffers small writes, so short
  server-sent-event frames are never flushed.
- Only one process should own the on-disk index: when a daemon is running,
  clients attach to it rather than starting a second in-process engine.

## [0.7.0] - 2026-09-24

### Added

- **Search tabs.** Each tab is an independent search with its own query,
  regex / case / contents / hidden / full-path toggles, type filter, results,
  selection and sort. A tab strip under the menu bar switches, closes (`×`) and
  adds (`+`) tabs; keyboard shortcuts are `Ctrl+T` (new), `Ctrl+W` (close),
  `Ctrl+Tab` (next) and `Ctrl+1..9` (select), with matching *File* menu items.
  Switching tabs is instant — results are kept per tab — and background tabs
  keep searching, with stale responses dropped.
- **Session persistence.** Open tabs (query, filters and which tab was active)
  are saved to `gui.json` and restored on startup. State is saved when tabs are
  added / closed / switched, when the search box loses focus, on exit, and by a
  throttled background save while editing — so closing the app keeps the
  searches you had open. Older `gui.json` files without tabs still load.

### Fixed

- Toggling `.*` (regex), `Aa` (case), *Match contents*, *Hidden files* or
  *Full path* now re-runs the search immediately. Previously the change had no
  effect until the query text was edited.

## [0.6.0] - 2026-09-24

### Added

- **Bottom control strip**: a UI **zoom** selector with three levels (100% /
  110% / 125%), a **file-type** dropdown (the same categories as the sidebar,
  kept in sync whichever you use), and a **Folders** toggle that includes or
  excludes directories from results. Zoom, the folder toggle and the type
  choice persist in `gui.json`.
- `Query::include_dirs` (default `true`), so directories can be excluded by the
  engine rather than by trimming the result list — counts, limits and
  truncation stay correct. The CLI passes the default, and an end-to-end test
  covers both settings.

### Fixed

- **Result columns no longer break when the UI is zoomed.** `egui_extras` caches
  every column's width — including a `remainder` column — as soon as `resizable`
  is enabled, and then lays the columns out absolutely. Changing zoom changed the
  available (logical) width, so the cached total overflowed and the Size /
  Modified / Actions columns were pushed off-screen and clipped. Column widths
  are now recomputed each frame from the available width: the metadata columns
  keep a fixed size and the Name column takes the remainder, so the layout is
  stable under zoom and window resizing.

## [0.5.0] - 2026-09-24

### Changed

- **Reworked the GUI's visual design and typography.** A complete `Theme` now
  defines every surface, text weight and accent for both appearances, and
  egui's `Visuals` is fully specified (panels, popups, menus, widget states,
  selection, borders, shadows, corner radii). Light mode no longer inherits
  dark-tuned constants: a refined Tokyo Night for dark, and a clean cool-grey /
  white theme with an accessible blue accent for light. Typography uses one
  consistent scale (Heading 19 / Body 14.5 / Button 13.5 / Small 12 /
  Monospace 13) with roomier spacing and interaction sizes.
- **System fonts are now resolved correctly.** Fonts are located via fontconfig
  (`fc-list`) and resolved to the smallest upright, normal-weight, single-face
  file. Previously the whole system font directory (~3,000 files) was scanned
  and the chosen family was copied wholesale — for a font shipped as a `.ttc`
  collection this copied the entire collection (e.g. a 12.4 MiB, 36-face file)
  and used face 0, which is not necessarily the Regular weight.
- **Result rows and navigation are aligned and icon-free.** Emoji were replaced
  by drawn elements whose metrics do not depend on the font: rounded file-type
  chips (a short token such as `DIR` / `PDF` / `RS`), a painted sidebar (colour
  dot plus accent bar, precisely centred label), a vector magnifier, drawn
  folder / copy / terminal hover actions, and a flat sortable header. Filename
  and breadcrumb form a clear two-line hierarchy with right-aligned size and
  modified columns.
- Search bar, empty state, preview pane and status bar re-themed; the live
  indicator is a green dot rather than an emoji.

### Removed

- The `fontdb` dependency; font resolution now uses fontconfig.

### Performance

- Lower idle memory: loading system fonts no longer scans every installed font
  or copies large `.ttc` collections (measured ≈196 MiB → ≈161 MiB RSS on the
  same build profile).

## [0.4.1] - 2026-09-24

### Fixed

- **Realtime watcher no longer degrades on unreadable directories.** Watches are
  now installed per directory (non-recursive) instead of as a single recursive
  root watch, so one root-owned folder (for example inside a Steam/Proton
  `compatdata` prefix) can no longer abort the whole watch and silently drop the
  session into periodic-rebuild mode. Directories created at runtime are watched
  automatically, and the watcher covers the whole root instead of just the
  top level.
- Partial watch coverage is now visible: the GUI status bar shows
  `⚠ N dir(s) not realtime · periodic rebuild` and `easysearch-cli status` prints an
  `unwatchable:` line, instead of reporting a healthy session while rebuilds
  quietly mask the gap.

### Changed

- `.gitignore` / `~/.gitignore`: added Steam/Proton/Wine prefixes, Flatpak/Snap
  and container blobs, browser caches, and toolchain/virtualenv directories.
  This keeps the index lean and avoids watcher-hostile root-owned trees.

## [0.4.0] - 2026-08-20

### Added

- **Menu bar** (File / Edit / View / Settings / Help): File → new search,
  reload index, quit; Edit → search toggles + clear history; View → preview
  pane + theme (system/dark/light); Settings → settings dialog (theme, max
  results, preview, open config file, reset GUI settings); Help → About and
  keyboard-shortcuts dialogs.
- **Search history in the tray**: the tray menu now has a “Recent searches”
  submenu (last 12 queries) that re-runs a search on click; history is shared
  with the GUI and stays in sync.
- **System tray icon** (StatusNotifierItem via `ksni`/`zbus`, pure Rust):
  works on KDE/Qt natively and on GTK desktops that host SNI (GNOME +
  AppIndicator, XFCE, Cinnamon, MATE). Includes a programmatically drawn
  lightning-bolt icon, an Open/Quit menu, left-click to toggle the window,
  and close-to-tray behavior (only “Quit” exits the app).

### Fixed

- Results table: rows now sense mouse clicks (selection + double-click to open)
  instead of keyboard-only navigation; header sorting applies immediately to
  the current results; row height raised (52 px) with tighter cell spacing so
  two-line rows are no longer cropped with larger fonts.
- CLI: construct `Query` with the new `category` field (default `All`).

### Changed

- **X button now quits** the app by default; “close to tray” is an opt-in
  setting (Settings dialog) that hides the window to the tray instead.

## [0.3.0] - 2026-08-20

### Added

- **Pro-Search layout** (`easysearch-gui`): three-pane design — category
  sidebar, floating search bar, results table, and a live preview pane.
  Tokyo Night palette (`#1a1b26` bg, `#7aa2f7` accent).
- **Sidebar categories** (engine-backed `Category` filter): Recent (7 days),
  Images, Docs, Code, Archives, Audio, Video, and Large files (> 1 GiB).
- **Floating search bar** with in-bar toggles (`.*` regex, `Aa` case) and an
  options menu (`≡`): content match, hidden files, full-path, preview pane,
  theme, and recent searches.
- **Result rows**: file-type badges (colored dots), clickable breadcrumbs
  (`Home › Pictures › Screenshots`), an accent left-bar on the selected row,
  and hover-revealed actions (open folder, copy path, open terminal).
- **Preview pane**: image thumbnails, file-type label, and quick actions
  (Open, Copy Path, Terminal-in-folder).
- **Empty states**: search tips and clickable recent searches when idle.
- **Live-index indicator**: a pulsing “Indexing…” dot or a green “⚡ Live”
  badge in the status bar.

### Changed

- **Wayland-first display**: the GUI registers the `easysearch` app id
  with the compositor (window icon / taskbar grouping). winit already prefers
  native Wayland whenever `WAYLAND_DISPLAY` is set and falls back to X11;
  forcing X11 is done via `env -u WAYLAND_DISPLAY easysearch-gui`.

## [0.2.0] - 2026-08-20

### Added

- **System fonts**: the GUI now loads the desktop's UI and monospace fonts
  (via `fontdb`; e.g. Noto Sans / DejaVu) with egui's fonts as fallback, and
  font sizes were increased across the UI.
- **Light & dark themes**: the app follows the system theme on first launch
  (`dark-light`), with a persisted ☀️/🌙 toggle.
- **Search history**: queries are remembered (Enter, or when the search box
  loses focus) and persisted to `~/.config/easysearch/gui.json`;
  ↑/↓ in the empty search box cycles history, and the 🕘 button opens the
  recent-search list (with “Clear history”).
- **Reworked GUI** (`easysearch-gui`): dark/light themes with accent styling,
  file-type icons, a virtualized results table (name / size / modified columns
  with click-to-sort), full keyboard navigation (↑/↓/PgUp/PgDn, Enter to open,
  Esc to clear, Ctrl+F to focus search), a resizable file **preview pane**, and
  a polished search bar with mode hints and an indexing spinner.
- **Makefile**: `make` runs the full pipeline (dev build + tests + production
  build); `make build`/`make dev` and `make release`/`make prod` for dev vs
  production builds, plus `test`, `check`, `clippy`, `fmt`, `run`, `install`,
  `clean`, `help`.

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
  memory-mapped file (`~/.cache/easysearch/index-v1.bin`, kernel page
  cache, reclaimable); only a small recent-change overlay and a compact hash
  table stay resident. Background compaction folds the overlay back into the
  file. RAM-only mode available via `persist_index: false`.
- **Instant warm start**: the previous index is memory-mapped on launch and
  revalidated in the background.
- **Ignore files**: `.gitignore`/`.ignore` in the searched tree are honored
  (gitignore syntax), plus a global ignore at
  `~/.config/easysearch/ignore`. Shipped `.gitignore` excludes
  `node_modules`, `target`, build dirs, caches, editor settings, VCS internals.
- **Roots & exclusions**: default root is the running user's `$HOME`
  (configurable); pseudo-filesystems, network mounts, and USB/removable media
  excluded by default.
- **Degraded mode**: if kernel watch limits are exhausted, the engine falls
  back to periodic full rebuilds and reports the state in the UI.
- **Frontends**: native `easysearch-gui` (eframe/egui) and a scriptable
  `easysearch-cli` CLI; both link the `easysearch-core` library directly (zero IPC).
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
- Binaries: `easysearch-cli` 3.3 MB, `easysearch-gui` 12 MB (stripped, LTO).

### Known limitations

- Text-based files only (code, config, plain text, `.docx`); binary content
  search (PDF/images/archives) is out of scope for now.
- Results are returned in index order, not ranked; fuzzy/substring ranking is a
  planned enhancement.
- Non-UTF-8 file names are matched lossily.
- Network filesystems and removable media are not indexed by default.

[0.9.0]: https://github.com/easysearch/easysearch/releases/tag/v0.9.0
[0.8.0]: https://github.com/easysearch/easysearch/releases/tag/v0.8.0
[0.7.0]: https://github.com/easysearch/easysearch/releases/tag/v0.7.0
[0.6.0]: https://github.com/easysearch/easysearch/releases/tag/v0.6.0
[0.5.0]: https://github.com/easysearch/easysearch/releases/tag/v0.5.0
[0.4.1]: https://github.com/easysearch/easysearch/releases/tag/v0.4.1
[0.4.0]: https://github.com/easysearch/easysearch/releases/tag/v0.4.0
[0.3.0]: https://github.com/easysearch/easysearch/releases/tag/v0.3.0
[0.2.0]: https://github.com/easysearch/easysearch/releases/tag/v0.2.0
[0.1.0]: https://github.com/easysearch/easysearch/releases/tag/v0.1.0
