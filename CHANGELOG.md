# Changelog

All notable changes to **Everything for Linux** are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
  `⚠ N dir(s) not realtime · periodic rebuild` and `everything status` prints an
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

- **Pro-Search layout** (`everything-gui`): three-pane design — category
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

- **Wayland-first display**: the GUI registers the `everything-linux` app id
  with the compositor (window icon / taskbar grouping). winit already prefers
  native Wayland whenever `WAYLAND_DISPLAY` is set and falls back to X11;
  forcing X11 is done via `env -u WAYLAND_DISPLAY everything-gui`.

## [0.2.0] - 2026-08-20

### Added

- **System fonts**: the GUI now loads the desktop's UI and monospace fonts
  (via `fontdb`; e.g. Noto Sans / DejaVu) with egui's fonts as fallback, and
  font sizes were increased across the UI.
- **Light & dark themes**: the app follows the system theme on first launch
  (`dark-light`), with a persisted ☀️/🌙 toggle.
- **Search history**: queries are remembered (Enter, or when the search box
  loses focus) and persisted to `~/.config/everything-linux/gui.json`;
  ↑/↓ in the empty search box cycles history, and the 🕘 button opens the
  recent-search list (with “Clear history”).
- **Reworked GUI** (`everything-gui`): dark/light themes with accent styling,
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

[0.7.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.7.0
[0.6.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.6.0
[0.5.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.5.0
[0.4.1]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.4.1
[0.4.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.4.0
[0.3.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.3.0
[0.2.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.2.0
[0.1.0]: https://github.com/everything-for-linux/everything-for-linux/releases/tag/v0.1.0
