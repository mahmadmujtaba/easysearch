# Everything for Linux

Realtime filename **and** content search across your filesystem — a Linux
equivalent of VoidTools' *Everything* for Windows. Written in **Rust** with a
**native GUI** (egui, no web technologies) and a minimal resource footprint.

- **Realtime**: a live in-memory filename index is kept fresh by kernel
  filesystem events (`inotify`); a file created / renamed / edited / deleted is
  reflected in the next query within ~1 s.
- **Content search**: the embedded **ripgrep engine** reads files live, so
  content results are always current. An optional bounded in-RAM content cache
  accelerates repeated queries (default off).
- **Everything-style queries**: `*.pdf`, `invoice 2026`, `!draft`, regex mode,
  case toggle, hidden files, basename or full-path matching.
- **Self-updating**: check a signed HTTPS manifest and replace the binaries in
  place — no `.deb`/`.rpm`, no reinstall. See [`docs/updates.md`](docs/updates.md).
- **Lightweight**: no GC, no runtime, no bundled web engine. See the footprint
  budget in [`docs/scope.md`](docs/scope.md).

## Components

| Binary | Purpose |
|---|---|
| `everything` (CLI) | scriptable search + status |
| `everything-gui` | native desktop app (search box, toggles, results list) |
| `everything-core` (lib) | the engine: index, watcher, matcher, content search |

## Build (zero-sudo)

No root needed — the setup script installs a user-local Rust toolchain
(rustup), dev symlinks for system libraries, and wires the linker to rustup's
bundled `rust-lld` (no C compiler required):

```sh
./scripts/install-deps.sh
cargo build --release
```

Binaries land in `target/release/everything` and `target/release/everything-gui`.

> Optional, only for **.docx content search**: nothing. Word, OpenDocument and
> PDF content is extracted in-process (v0.18.0–v0.20.0); no external tool is
> needed.

## Packaging

Distribution packages are built from the same assets as the app:

```sh
make deb        # dist/everything-linux_<version>_<arch>.deb      (dpkg-deb)
make rpm        # ~/rpmbuild/RPMS/... then copied to dist/          (rpmbuild)
make flatpak    # dist/io.github.everythinglinux.EverythingForLinux.flatpak
make packages   # all of the above that this machine has tools for
make validate-packaging   # desktop entry + AppStream metadata
```

None of them need root. See [`docs/packaging.md`](docs/packaging.md) for what
they install, the dependency-derivation details, the Flatpak sandbox notes, and
which identifiers to change before publishing.

> What is still outstanding — unverified package builds, placeholder
> identifiers, known rough edges, measured footprint — is tracked in
> [`docs/pending.md`](docs/pending.md).

## Updating

Packages are for the first install; updates happen in place over the network.
The GUI checks quietly at launch (**Help ▸ Check for updates…** to do it now) and
can install and restart onto the new build; the CLI can do the same:

```sh
./target/release/everything self-update --check
./target/release/everything self-update
```

A release publishes a `manifest.json` signed with Ed25519 plus a SHA-256 per
asset. The client fetches it over **HTTPS only**, verifies the signature against
a public key compiled into the binary, verifies every download's checksum, and
only then `rename()`s the new binaries over the old ones (atomic, and safe while
the old binary is running). Anything that fails — a bad signature, a mismatched
hash, a cleartext URL, a version that is not newer — means nothing is written.

To publish a release, see [`docs/updates.md`](docs/updates.md) and
[`scripts/release-sign.sh`](scripts/release-sign.sh).

## Usage

```sh
# GUI + daemon in one binary (this is what end users run)
make run                          # or: ./target/release/everything-linux

# CLI
./target/release/everything search "*.pdf"              # glob patterns
./target/release/everything search "report 2026 !draft" # AND terms + exclude
./target/release/everything search --regex 'report[_-]\d{4}\.pdf$'
./target/release/everything search --content "TODO"     # search inside files
./target/release/everything search mtn --fuzzy          # fzf-style match
./target/release/everything search '' --content 'a\nb' --multiline  # spans lines
./target/release/everything search '*' --ext pdf,docx --min-size 1M
./target/release/everything search '*.log' --modified-within 7d
./target/release/everything search '*' --under /srv/data
./target/release/everything status                      # index state, counts
./target/release/everything self-update --check         # is a newer release out?
./target/release/everything self-update                 # install it in place

# Control a running window (bind these to desktop shortcuts — no privileged API)
./target/release/everything-linux --toggle              # show the window / hide it
./target/release/everything-linux --search TODO         # open and search
```

The GUI follows the “FileSearch Pro” reference layout (see
[`docs/ui.md`](docs/ui.md)): a menu bar and a labelled toolbar (Back/Forward through the
location history, Home, Index, Content Search, Regex, Recent, Saved), a search row
(query + scope + location + `Search`), a filter bar (Type / Size / Modified / Path /
Ext / Case / Hidden), a results header with sorting and row density, then three
panes — a sidebar (categories with live counts, saved searches, indexed locations),
the results table (`#`, Name, Path, coloured type pill, Size, Modified, Match,
Relevance, bulk checkboxes) and a right-hand panel with Preview/Details tabs and
quick actions. Under it: view tabs, bulk actions, recent searches and a live status
bar (index state, CPU, RAM, query stats, 100/110/125% zoom). It follows the
desktop's light/dark scheme live and uses your system fonts.

Query semantics (Everything-style):

- space-separated terms are **ANDed**; `!term` **excludes**
- terms without glob metacharacters are **substring** matches (`draft` matches
  `draft.pdf`); `*`, `?`, `[...]` work as globs
- `--fuzzy` (or the **Fuzzy** toolbar button) matches the term's characters *in
  order* anywhere, so `mtn` finds `meeting-notes.md`; it also drives the
  Relevance ranking. `!term` exclusions stay literal
- `--regex` treats each term as a regex (filenames and content); `--multiline`
  lets the content pattern span lines (`foo\nbar`), which is slower
- case-insensitive by default (`--case` to change)
- hidden files/dirs are indexed but hidden from results (`--hidden` to include)

## Configuration

`~/.config/everything-linux/config.json` (optional; defaults shown):

```json
{
  "roots": [],
  "exclude_removable": true,
  "exclude_network": true,
  "respect_ignore_files": true,
  "persist_index": true,
  "storage": "sqlite",
  "db_dir": null,
  "disk_index_dir": null,
  "overlay_compaction_threshold": 8192,
  "exclude_fstypes": [],
  "content_index_enabled": false,
  "content_index_max_file_bytes": 8388608,
  "content_index_total_cap_bytes": 268435456,
  "degraded_rescan_secs": 30,
  "max_results": 1000
}
```

- **roots**: empty = `$HOME` of the user running the program. Add paths to
  index more (e.g. `["/home/me", "/srv/data"]`).
- **storage**: `"sqlite"` (default) keeps the index in a SQLite database;
  `"mmap"` uses the original memory-mapped file. See [`docs/sqlite.md`](docs/sqlite.md).
- **db_dir**: where the SQLite database lives (default
  `~/.cache/everything-linux/db`).
- **persist_index**: `true` (default) keeps an index on disk with only recent
  changes in RAM; `false` keeps everything in RAM (no database, no mmap file).
- **disk_index_dir**: where the mmap index lives (default
  `~/.cache/everything-linux`); the database defaults to a `db/` folder inside it.
- **overlay_compaction_threshold**: how many pending changes trigger a
  background compaction (mmap backend).
- **content_index_enabled**: `true` enables the background content cache
  (bounded, LRU; keeps repeated content queries fast).
- A global ignore file at `~/.config/everything-linux/ignore` adds extra
  exclusions.

Every key, with its default and effect, is documented in [`docs/config.md`](docs/config.md).

## Non-essential folders

`~/.gitignore` (installed by `scripts/install-deps.sh`, or copy the project's
`.gitignore`) keeps noise out of the index: `node_modules`, `target`, build
dirs, caches, editor settings, VCS internals. Any `.gitignore`/`.ignore` in the
searched tree is honored.

## Display backends (Wayland-first, X11 second)

The GUI is **Wayland-first**: winit connects to a native Wayland session
whenever `WAYLAND_DISPLAY` is set (it prefers Wayland because an X11 display
can exist under Wayland via XWayland) and automatically falls back to **X11**
when only `DISPLAY` is present. The app id `everything-linux` is registered
with the Wayland compositor for window icon / taskbar grouping.

To force a backend:

```sh
everything-gui                            # auto: Wayland, else X11
export -n WAYLAND_DISPLAY; everything-gui # force X11 (or: env -u WAYLAND_DISPLAY)
```

## System tray

The app shows a tray icon (StatusNotifierItem over D-Bus) with an Open/Quit
menu and a “Recent searches” submenu; left-click toggles the window. By
default the **X button quits** the app; enable *Settings → “Keep running in
tray when the window is closed”* to hide to the tray instead (then only
“Quit” exits). Works on KDE/Qt natively and on GTK desktops that host SNI
(GNOME with the AppIndicator extension, XFCE, Cinnamon, MATE).

## Realtime & watch limits

The index is updated by `inotify`. The kernel caps watches per user
(`fs.inotify.max_user_watches`, default 123,040 on Debian 13). If a tree is too
large, the app automatically falls back to **degraded mode** (periodic rebuild,
visible in the status bar). You can raise the cap:

```sh
sudo sysctl fs.inotify.max_user_watches=1048576   # persists until reboot
# make permanent: echo 'fs.inotify.max_user_watches=1048576' | sudo tee /etc/sysctl.d/90-inotify.conf
```

## Where the index lives (and why RAM stays low)

The index is a **SQLite database** in `~/.cache/everything-linux/db/`
(`index.db`, WAL mode), owned by the daemon — the only writer:

- The bulk of the index is on disk; only a small **change overlay** of recent
  creates/edits/deletes stays resident. The overlay is folded into the database
  in one transaction before each query, so a query never misses a change the
  watcher has already seen.
- **Startup is instant.** A complete database is served immediately while a
  background pass re-validates it against the live filesystem. The database is
  created from a full walk when it is missing, when the schema version changes,
  or when a previous build was interrupted (a `complete` marker is written in the
  same transaction as the rows, so a partial index is never mistaken for a small
  complete one).
- If the delta backlog grows past 20 000 changes, a **full rebuild** replaces
  replaying a very long delta stream.
- `storage = "mmap"` in the config switches back to the original memory-mapped
  file (`index-v1.bin`, zero-copy scan) and `persist_index = false` keeps nothing
  on disk at all. Details and the trade-offs: [`sqlite.md`](docs/sqlite.md).

Measured on a real `$HOME`: ≈ 15 MiB idle for the headless engine (mmap mode, ≈137k
files); on 100 479 entries the SQLite daemon sits at ≈ 51 MiB and the GUI at ≈ 92 MiB,
with filename queries at 1–26 ms and filtered queries at 1–2 ms (SQL pushdown).
The GUI adds the native window/GL stack.

## Footprint (budget)

- binaries < 10 MB (stripped, LTO; the GUI binary is ≈ 12 MB)
- idle RAM ≈ 15 MiB headless; the index is on disk, only the change overlay is in RAM
- idle CPU ≈ 0 % (event-driven)
- filename query at 1M entries < 50 ms; content queries stream results

## Documentation

| Document | What it covers |
|---|---|
| [`docs/scope.md`](docs/scope.md) | Scope, architecture, search semantics, realtime guarantees, footprint budget, delivery phases |
| [`docs/ui.md`](docs/ui.md) | Using the GUI: layout, filters, saved searches, shortcuts, relevance scoring |
| [`docs/config.md`](docs/config.md) | Every `config.json` key, its default and its effect |
| [`docs/api.md`](docs/api.md) | The daemon's HTTP/JSON API |
| [`docs/sqlite.md`](docs/sqlite.md) | The SQLite index: schema, flush/refresh policy, trade-offs |
| [`docs/packaging.md`](docs/packaging.md) | Building `.deb`, `.rpm` and Flatpak packages |
| [`docs/pending.md`](docs/pending.md) | What is still outstanding, prioritised |
