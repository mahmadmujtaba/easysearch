# Changelog

All notable changes to **EasySearch** are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.39.0] - 2026-09-27

### Added

- **Syntax highlighting in the preview pane.** Source code is now coloured —
  comments, strings, numbers, keywords, types and call sites — by a small
  tokeniser (no parser, no external crate) that understands each language's
  comment and string syntax. It covers the major languages: Rust, C/C++,
  Java, Go, Python, JavaScript/TypeScript, shell, JSON/YAML/TOML/INI,
  HTML/XML, CSS, SQL, Ruby, PHP, Lua, Kotlin, Swift, C#, R, Haskell, Scala,
  Dart, Perl, Nix, Elixir, Erlang and Clojure, mapped from the file extension
  (and a few well-known names such as `Makefile` and `Dockerfile`). Fenced code
  blocks inside a **Markdown** preview are highlighted using the fence's
  language tag. Colours are drawn from the active theme (Dark/Light/Brand), so
  highlighting matches the appearance; files over 128 KiB fall back to plain
  monospace so layout never stalls.

## [0.38.0] - 2026-09-27

### Changed

- **EasySearch is now fully offline.** All network code is gone: no in-app
  updates, no update check, no telemetry, no remote API. The app opens no
  sockets — its only descriptors are the index database, the files it indexes
  and the local `$XDG_RUNTIME_DIR` control socket. Install and upgrade through
  your package manager (`.deb`/`.rpm`), which is the one place you already trust
  to fetch and verify software.

### Removed

- **The in-place self-updater.** `easysearch-cli self-update`, the GUI
  **Help ▸ Check for updates…** item and the *Software update* window, the
  launch-time update check and the `⬆ v… available` status-bar badge,
  **Settings ▸ Updates**, `docs/updates.md`, `scripts/release-sign.sh`, the
  `ed25519-dalek`/`sha2` core dependencies and the compiled-in release public
  key are all removed. `core/src/update.rs` is deleted.

## [0.37.0] - 2026-09-26

### Added

- **Real previews, by kind.** The preview pane now shows:
  - **Text of all forms** — any text-like file (source, config, log, csv, …) is
    read up to 256 KiB and shown monospace, with a note when it was cut;
  - **Markdown**, rendered with light styling (headings, bullets, quotes, fenced
    code, emphasis markers stripped);
  - **PDF and office** — the document's text layer, extracted in-process by the
    same reader content search uses (no external tool); `xls`/`xlsx`/`ppt`/`pptx`
    say so rather than guessing;
  - **Audio and video** — duration, format, codecs, resolution and common tags
    through `ffprobe`, plus a first-frame thumbnail through `ffmpeg`, both best
    effort (the pane explains itself when neither is installed).
- **First-class OpenDocument (LibreOffice) support.** `.odt`, `.ods`, `.odp` and
  `.odg` are read through the in-process ODF `content.xml` reader — text
  extraction, content search and previews all handle them like `.docx`.

## [0.36.0] - 2026-09-26

### Added

- **A third theme, “Brand”, drawn from the logo**: a deep teal ground with the
  mark's teal as the accent, its pink for errors and archives, and its lime for
  “good”. Settings ▸ Appearance and View ▸ Theme now offer **Dark** (default),
  **Light** and **Brand**. The choice is stored by name; the old boolean
  `"dark"` (and the pre-0.33 `null`) is still read, so `gui.json` files keep
  working.
- **The logo in the top-right of the menu bar**, with its version on hover.
- **Colourful icons instead of coloured dots and text chips.** The sidebar's
  categories, saved searches and locations are now painted glyphs (a folder for
  a location, a clock for *Recent*, a frame for *Images*, `<>` for *Code*, a
  note for *Audio*, …), and result rows — including **directories** — show a
  coloured type icon (folder, document, image, archive, audio, video, code,
  plain file) instead of the `DIR`/`PDF` text chip. All drawn with egui's
  painter, so no icon theme or bundled asset is needed.

## [0.35.0] - 2026-09-26

### Changed

- **Filename search is the default, on every start.** The search scope is no
  longer restored across restarts: tabs come back with their query and filters
  but always in **Filenames**, so opening the app can never drop you into the
  expensive content search by surprise. Content search remains a per-session
  choice (it is still remembered while you switch tabs).
- **Leaving content search releases its memory.** Content search runs ripgrep
  over your files, so it is the heaviest thing the app does. Switching the scope
  back to *Filenames* (or clearing the filters, or unticking *Match contents*)
  now drops the rows that search produced and asks the engine to return its freed
  heap pages (`malloc_trim`), instead of keeping both resident until the next
  search happens to replace them.
- **Content search warns before it costs you.** Switching to *Contents* / *Full
  text* shows a one-time notice (more memory and CPU, slower first results, and
  the content cache is spooled to disk, not RAM); the status bar carries a
  reminder while the scope is active, and the *Content Search* button's hover
  text says so too.

### Added

- A `trim` op in the engine protocol, with `Engine::trim_memory`,
  `ChildEngine::trim_memory` and `Backend::trim_memory` behind it.

## [0.34.0] - 2026-09-26

### Changed

- **The engine is now a child process of the app, over pipes — there is no HTTP,
  no socket and no port.** The daemon used to listen on `127.0.0.1:5858` and
  answer a REST-ish JSON API; that meant a local service to secure (no auth, any
  process on the machine could reach it) and a third tier (app, daemon, CLI) to
  keep in step. The app now spawns `easysearch --engine` as its own child, keeps
  its stdin/stdout, and they exchange one JSON object per line
  ([`docs/api.md`](docs/api.md), the new *Engine protocol*). Nothing about the
  index is reachable from outside the process pair, so there is no endpoint to
  expose. Requests are multiplexed by id and answered on the engine's own
  threads, so a slow content search never blocks the status poller. The engine's
  log is `~/.cache/easysearch/engine.log`.
- **The tray moved back into the app, and closing the window hides it.** With the
  engine as a child of the app, the tray belongs to the app process too: the X
  button hides the window (the engine keeps indexing, the tray stays), and *Quit*
  — the tray's *Quit*, *File ▸ Quit EasySearch*, or `easysearch --quit` — stops
  the app and its engine together. `easysearch --stop`, the "quit on close"
  setting and the "Stop background service" menu item are gone; the File menu has
  *Hide window* and *Quit EasySearch*.
- **The CLI runs the engine in-process.** There is no daemon to attach to, so
  `easysearch-cli --remote ADDR` is gone; a script gets an answer without
  touching the app. `easysearch-cli self-update --restart-app` (was
  `--restart-daemon`) asks the running app to quit so the next launch uses the
  new binary.
- **`easysearch-gui` (the standalone dev binary) runs the engine in-process.**
  `--daemon ADDR` / `EASYSEARCH_DAEMON` are gone with the HTTP tier.

### Removed

- `core/src/remote.rs` (the HTTP/1.1 client), the `tiny_http` dependency, the
  daemon's HTTP routing and query-parameter parsing, `--addr`, `easysearch
  --stop`, and `core::api::DEFAULT_ADDR` / `percent_decode` /
  `category_from_str`. `core/src/api.rs` is now just the protocol's wire types.

### Added

- `core/src/proto.rs` (the frame types) and `core/src/child.rs` (the client the
  app uses to drive its engine child).

### Docs

- A full sweep for the old architecture: `docs/api.md` is now the engine
  protocol; `scope.md`, `ui.md`, `config.md`, `sqlite.md`, `packaging.md`,
  `updates.md`, `pending.md`, `README.md`, the screenshots guide, the Makefile
  and the packaging prose no longer describe an HTTP service, a `--daemon`/`--addr`
  flag, or a desktop-followed theme.

## [0.33.0] - 2026-09-26

### Changed

- **The tray icon moved into the background service, and closing the window now
  frees the window.** The tray belonged to the GUI, so hiding the window kept a
  full GL context and a second copy of the index resident just to draw one icon.
  The daemon owns the tray now: closing the window *exits the window process*,
  while the index, the HTTP/JSON API and the tray keep running. The tray's *Open*
  hands a command to the window if one is up and otherwise starts a fresh GUI
  attached to the same daemon (`--toggle`/`--show` already did that). File ▸ now
  offers *Close window* and *Stop background service…*, and `easysearch --stop`
  is the CLI counterpart (the window is a separate process, so `--quit` cannot
  stop the service). No tray available means the window simply closes.
- **The theme is an explicit, remembered choice: Dark by default.** The app no
  longer follows the desktop's light/dark setting, live or otherwise — Settings ▸
  Appearance (and View ▸ Theme) offer *Dark* / *Light* only, stored as
  `"dark"` in `gui.json`. A legacy `"dark": null` (the old “follow system”)
  resolves to Dark rather than resetting the file, and the theme-file polling,
  `dark-light` dependency and GTK/KDE scheme sniffing are gone.
- **Fonts come from the system.** Instead of our own preference list, the app asks
  fontconfig for the file the desktop's configured family resolves to (KDE's
  `kdeglobals`, then GTK's `settings.ini`, then fontconfig's `sans-serif` /
  `monospace`), via `fc-match`, and draws with that. The bundled egui faces are
  kept only as glyph fallbacks (emoji, CJK, rare symbols).

### Added

- **The installed icon can no longer regress to an opaque tile.** The packaging
  SVG must be the transparent export — a compile-time test compares it against
  `icons/transparent-logo.svg` — and `scripts/package-deb.sh` renders its PNGs as
  `PNG32:` at 16–512 px, so the icon the launcher shows always carries an alpha
  channel. `make install` also refreshes the hicolor icon cache, which is why a
  stale (cream) icon could survive an upgrade.

### Internal

- `ipc` (the control socket) and `logo` (the brand asset + rasteriser) moved into
  `core`, since the daemon and the GUI both need them; `ksni` and `dark-light`
  left the GUI crate.

## [0.32.1] - 2026-09-26

### Changed

- **The app now shows the transparent logo, not the cream tile.** The window
  icon, the tray pixmap, the About dialog and the welcome screen all embed
  `gui/assets/logo.png`, and that was the *coloured* export — a wordmark on its
  own cream background. On the dark theme it read as a pasted-on light square.
  The embedded asset and the installed hicolor icon are now
  `icons/transparent-logo.*`, so the mark sits directly on the theme's own
  background. The tray's ARGB32 buffer is premultiplied for it (the convention
  Qt/KDE expect), which straight alpha from a PNG would otherwise leave with
  bright fringes. `gui/src/logo.rs` now asserts the asset is square *and*
  genuinely transparent, so embedding the opaque tile again is a test failure.
  The README keeps the coloured export — a self-contained tile reads on both
  light and dark pages.

## [0.32.0] - 2026-09-26

### Changed

- **Content search no longer keeps documents in memory.** The content cache
  used to hold every searched document's text in a RAM map (up to the
  `content_index_total_cap_bytes` cap, 256 MB) and never gave the pages back to
  the OS, which is what made an installed `easysearch` sit at ~1.4 GB resident
  after a few content searches. The cache is now **disk-backed by default**: text
  is spooled to `<disk_index_dir>/content/<hash>.txt`, the map holds only
  bookkeeping (path, length, last use), and a lookup reads the spool file, uses
  it and drops it — so the footprint tracks the number of files, not their
  contents. Eviction, replacement and `clear` delete the spool files, and
  rebuilding the store wipes the directory first.
- **A content cache in RAM is now an explicit boot choice.** Pass
  `--content-in-memory` (or set `EASYSEARCH_CONTENT_MEMORY=1`) to keep the old
  behaviour when you would rather trade memory for not re-reading the disk. It
  is a start-up flag because it changes what the daemon does, not a setting to
  flip mid-session; `content_index_in_memory` in `config.json` gives the same
  choice declaratively. Content indexing itself stays **off at boot** as before.
- **glibc's malloc arenas are capped at two** (`MALLOC_ARENA_MAX=2`) before any
  thread starts, in every entry point. On a many-core machine the default ceiling
  of 8 × cores let the allocator grow dozens of 64 MiB arenas that were never
  unmapped; capping this is why the fixes above are enough. An explicit
  `MALLOC_ARENA_MAX` in the environment is left alone.

### Fixed

- **The content cache's “N pending” progress no longer underflows.** The queue
  incremented a private counter while the extraction thread decremented the
  engine's *separate* one, so the unsigned count wrapped on the first extraction
  and the status bar could show a nonsensical "18446744073709550116 pending".
  Both now share one `Arc<AtomicUsize>`. (Pre-existing, found while measuring
  this release.)

### Added

- **Closing the window keeps EasySearch running in the tray by default.** The X
  button now hides the window instead of quitting, so the index and the last
  search stay warm and one click brings it back. Quit explicitly from the File
  menu, the tray's *Quit*, or `easysearch --quit`. The old opt-in became its
  inverse — Settings has *Quit when the window is closed* (off by default) — and
  existing `gui.json` files fall back to the new default. When no system tray is
  available the window is closed for real, since there would be no way to get it
  back.
- **`easysearch --search QUERY` and `--show` now launch the app when nothing is
  running.** They previously reported "nothing is running" and did nothing, so a
  desktop shortcut could not both start the app and drive it. As with `--toggle`,
  they now fall through to a normal start, and a fresh `--search QUERY` runs that
  query in the window as soon as it opens.

## [0.31.1] - 2026-09-26

### Fixed

- **The release step now proves that it published something.** The `v0.29.1` tag
  ran its "attach to the release" job to completion — every step reported
  success — yet no release for it exists, while `v0.30.0` and `v0.31.0`, pushed
  minutes either side, published correctly. Rather than trust the exit code of
  `gh release create`, the step now reads the release back and fails the run
  unless it is there with all three assets, so a silent no-op cannot pass as
  success again. The tag it works on comes from `GITHUB_REF` — the run's own ref
  — instead of being assumed from `github.ref_name`.
- `v0.29.1` therefore has no GitHub release. Its `.deb`, `.rpm` and
  `SHA256SUMS` are still on that run's artifact; re-tagging would be the only way
  to publish it, since a re-run uses the workflow file from that same commit.

## [0.31.0] - 2026-09-26

### Changed

- **The real logo replaces the drawn placeholder.** `icons/` now holds the
  project's artwork — the *EASY SEARCH* wordmark: a magnifier over a pink
  starburst, teal and pink lettering, a green swoosh, on cream — and it is used
  everywhere the mark appears:
  - `packaging/icons/hicolor/scalable/apps/io.github.easysearch.EasySearch.svg`
    is `icons/colored-logo.svg` under the application id, so launchers, icon
    themes and software centres find it;
  - `gui/assets/logo.png` is a 512 px copy of the same tile, compiled into the
    binary for the window icon, the tray pixmap and the About dialog;
  - the README shows `icons/colored-logo.svg`.
  The hand-drawn magnifier-and-bolt rasteriser is gone: `gui/src/logo.rs` is now
  a thin loader around the embedded artwork (its geometry regression tests went
  with it, replaced by checks that the asset decodes and resizes opaquely).
- **`make install` installs desktop integration, not just binaries.** The desktop
  entry, the AppStream metadata and the hicolor icon go to `~/.local/share/…`,
  and `make uninstall` removes them. This is what puts the app in the launcher
  *and* what puts the logo in the window decoration: on Wayland a window carries
  no icon of its own, so the decoration resolves the application id against the
  installed desktop entry — without it the title bar shows a generic
  placeholder even though everything else works.
- The 13 px mark beside the version in the status bar is gone — a wordmark does
  not read at that size. The logo is shown in the About dialog (168 px) and on
  the welcome screen (200 px) instead.
- `docs/screenshots/` was re-shot.

## [0.30.0] - 2026-09-26

### Added

- **A logo, drawn in code and shown in the app.** `gui/src/logo.rs` rasterises one
  mark — a magnifier over a lightning bolt, on a rounded badge — and the app now
  uses it for the **window icon** (so X11 and icon-theme-less sessions show it,
  not only Wayland via the desktop entry), the **tray pixmap**, the **About
  dialog**, the idle empty state, and at 13 px next to the version in the
  **status bar**. It is supersampled, so it is smooth at every size, and it needs
  no image asset.
- The design lives in one place. The geometry is written in the same 512-unit
  space as `packaging/icons/…/EasySearch.svg`, which was redrawn to match, so the
  launcher icon and the in-app mark cannot drift apart. The README and the About
  dialog show the mark too.

### Changed

- **The tray icon is the same mark now.** It used to be a lightning bolt on a
  dark square from its own little rasteriser, which did not match the icon
  shipped to launchers.
- `docs/screenshots/` was re-shot with the mark in the status bar.

### Fixed

- Two geometry bugs in the new rasteriser, both caught by diffing it against a
  render of the SVG: the handle's direction was measured from the lens centre
  rather than from its own start, drawing it a third too long and into the
  badge's corner; and the lens hole was not punched through the handle's inner
  end, which left a ring-coloured sliver inside the lens. Each now has a
  regression test.

## [0.29.1] - 2026-09-26

### Fixed

- **The release job could not create a release.** `gh` needs the repository
  remote, and `--generate-notes` needs the commit and tag history, but the job
  only downloaded the build artifact — so `gh release create` died with
  `fatal: not a git repository (or any of the parent directories): .git`. It now
  checks the repository out first (`fetch-depth: 0`). Nothing was published for
  `v0.29.0` because of this; its `.deb`, `.rpm` and `SHA256SUMS` are still on
  that run's artifact.

## [0.29.0] - 2026-09-26

### Added

- **The packaging pipeline also validates pull requests and cuts releases.**
  `.github/workflows/packages.yml` builds the `.deb` and the `.rpm` on every pull
  request too, so packaging breakage is caught *before* it is merged, and pushing
  a `v*` tag attaches both packages (plus `SHA256SUMS`) to the GitHub release for
  that tag. The release job downloads the artifact the build job produced — no
  second build — and uses the automatic `GITHUB_TOKEN`, so still no secrets. Tag
  builds are never cancelled by the concurrency group; branch builds still are.
- **README screenshots in both themes** — `docs/screenshots/dark.png` and
  `docs/screenshots/light.png`, shown in `README.md` and referenced from the
  AppStream metainfo so software centres show them too. They come from an
  isolated instance whose `$HOME` is a throwaway demo tree, so no personal
  filenames are published.

### Changed

- **The placeholder repository URLs are real.** `DEFAULT_MANIFEST_URL`
  (`core/src/update.rs`), the RPM spec's `URL`, the `CHANGELOG`'s release links
  and the docs all point at `github.com/mahmadmujtaba/easysearch`. The metainfo
  gained `<url type="homepage">`, `bugtracker` and `vcs-browser`, and the deb a
  matching `Homepage:` field. That clears the `url-homepage-missing` warning: `make
  validate-packaging` now reports no errors and no warnings (one pedantic note
  remains, about the uppercase in the app id).

## [0.28.1] - 2026-09-26

### Fixed

- **The packaging pipeline's RPM step now actually builds.** v0.28.0 added the
  workflow and fixed the spec's unpackaged doc files, but `rpmbuild` still could
  not start: it refuses to run when the spec's `BuildRequires` (`cargo`,
  `rust >= 1.88`, `desktop-file-utils`) are missing from the rpm database — which
  they always are on Debian/Ubuntu, where Rust comes from rustup.
  `scripts/package-rpm.sh` now passes `--nodeps`, the tools being provided by the
  environment (the job `dnf builddep` does on Fedora). That is the change that
  takes the workflow from red to green.
- `rpmbuild` passes `HOME`/`PATH` through to `%build`, so the rustup `cargo` shim
  is found there — checked with a probe spec rather than assumed.

### Changed

- The workflow uses current action majors — `actions/checkout@v7`,
  `actions/cache@v6`, `actions/upload-artifact@v7` — clearing the Node 20
  deprecation warning the first run reported.
- The RPM was reproduced end-to-end in an `ubuntu:24.04` container (the image
  `ubuntu-latest` is): it builds, the payload is complete (three binaries, desktop
  entry, metainfo, SVG, both doc files) and its `Requires` carry the `dlopen`ed
  GUI sonames next to the `libc`/`libgcc` ones. Recorded in `docs/packaging.md`.

## [0.28.0] - 2026-09-26

### Added

- **A packaging pipeline.** `.github/workflows/packages.yml` builds the `.deb` and
  the `.rpm` on every push to `master` — which is what a merged pull request
  produces — and uploads them to the run as `easysearch-master-packages` (both
  packages plus a `SHA256SUMS`, kept 90 days). *Run workflow* rebuilds by hand,
  and a newer push cancels a build already in flight. The job installs `rpm`
  (which provides `rpmbuild` on Ubuntu) and calls the same
  `scripts/package-deb.sh` / `scripts/package-rpm.sh` used locally, so CI doubles
  as a second check on `make deb` / `make rpm`. No secrets, no root.

### Fixed

- **The RPM spec's doc files were unpackaged.** It installed `LICENSE` and
  `README.md` into `/usr/share/doc/easysearch` but declared them with
  `%license`/`%doc`, which place files in a *name-version* directory, so `rpmbuild`
  would fail with “Installed (but unpackaged) file(s) found”. Both are now listed
  by their real paths in `%files`, which is portable across rpm implementations and
  keeps the layout consistent with the `.deb`. The bug was latent because the spec
  had never been executed — the RPM step still needed the `--nodeps` fix that
  lands in 0.28.1.

### Changed

- **`.cargo/config.toml` is no longer tracked.** It is generated per machine by
  `scripts/install-deps.sh` and embeds absolute paths (the `rust-lld` linker
  wrapper and a `~/.local/lib` search path), so a fresh clone — and CI — could not
  build with it. It is now gitignored; a clone links with the system C compiler,
  and the file stays on disk, untouched, where it was generated.

## [0.27.0] - 2026-09-26

### Added

- **The `Created` column** — the last reference-UI table gap. Every result row now
  shows its **birth time** ("3 d ago", "1 mo ago", …), and the `Created` header
  sorts by it exactly as Name / Size / Modified do: click for ascending, again for
  descending, a third time to clear.
- The value is read live with `stat` rather than stored in the index. The table
  only ever renders a screenful of rows, so one `stat` per visible row (and per
  sort click) is cheap, and the column stays right for a file created since the
  last index build. It also avoids any change to the on-disk formats — neither the
  SQLite `SCHEMA_VERSION` nor the memory-mapped `MAGIC` moved, so nothing rebuilds.
- Where the filesystem records no birth time (many do not) the cell shows `—` and
  those rows sort together at one end; ext4, btrfs and xfs do record one.
- New tests cover the column: the sort cycle (`ascending → descending → off`, keyed
  on its own column) and `birth_secs` (a missing path is `0` → `—`; a fresh file's
  time is recent where the filesystem supplies one). 106 tests pass and
  `clippy --workspace --all-targets` is clean.

## [0.26.0] - 2026-09-26

### Added

- **Per-location counts in the sidebar** (the last reference-UI gap of its kind):
  Home, Desktop, Documents, … each show their own result count for the current
  query, so you can see where the matches are before narrowing to one.
- The facet worker now computes both lists in one pass — `CountRequest` carries
  the locations and `Counts` returns `per_location` alongside `per_category`,
  with the same throttling/coalescing as before. Each location is a count with
  `under` set to that directory; on the SQLite backend that is an indexed prefix
  query, so the extra work is small.
- The sidebar's *Advanced search* note about a read-only content-index state is
  gone — it is a live switch as of v0.24.0.

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

[0.9.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.9.0
[0.8.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.8.0
[0.7.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.7.0
[0.6.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.6.0
[0.5.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.5.0
[0.4.1]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.4.1
[0.4.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.4.0
[0.3.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.3.0
[0.2.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.2.0
[0.1.0]: https://github.com/mahmadmujtaba/easysearch/releases/tag/v0.1.0
