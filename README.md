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
- **Lightweight**: no GC, no runtime, no database. See the footprint budget in
  [`docs/scope.md`](docs/scope.md).

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

> Optional, only for **.docx content search**: `sudo apt install docx2txt`.

## Usage

```sh
# GUI
./target/release/everything-gui

# CLI
./target/release/everything search "*.pdf"              # glob patterns
./target/release/everything search "report 2026 !draft" # AND terms + exclude
./target/release/everything search --regex 'report[_-]\d{4}\.pdf$'
./target/release/everything search --content "TODO"     # search inside files
./target/release/everything status                      # index state
```

Query semantics (Everything-style):

- space-separated terms are **ANDed**; `!term` **excludes**
- terms without glob metacharacters are **substring** matches (`draft` matches
  `draft.pdf`); `*`, `?`, `[...]` work as globs
- `--regex` treats each term as a regex (filenames and content)
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
- **persist_index**: `true` (default) keeps the index on disk (memory-mapped)
  with only recent changes in RAM; `false` keeps the whole index in memory.
- **disk_index_dir**: where the mapped index lives (default
  `~/.cache/everything-linux`).
- **overlay_compaction_threshold**: how many pending changes trigger a
  background compaction (default 8192).
- **content_index_enabled**: `true` enables the background content cache
  (bounded, LRU; keeps repeated content queries fast).
- A global ignore file at `~/.config/everything-linux/ignore` adds extra
  exclusions.

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

## Realtime & watch limits

The index is updated by `inotify`. The kernel caps watches per user
(`fs.inotify.max_user_watches`, default 123,040 on Debian 13). If a tree is too
large, the app automatically falls back to **degraded mode** (periodic rebuild,
visible in the status bar). You can raise the cap:

```sh
sudo sysctl fs.inotify.max_user_watches=1048576   # persists until reboot
# make permanent: echo 'fs.inotify.max_user_watches=1048576' | sudo tee /etc/sysctl.d/90-inotify.conf
```

## Low-memory index (how it stays small)

The index is designed to keep RAM low by using the filesystem:

- The bulk of the index (paths + metadata) is serialized into a compact binary
  file at `~/.cache/everything-linux/index-v1.bin` and **memory-mapped** —
  cold pages cost zero RSS and are evicted by the kernel under pressure; warm
  pages live in the reclaimable page cache.
- Only a small **change overlay** (recent creates/edits/deletes from the
  watcher) and a compact hash table (~4 MB) stay resident. Queries scan the
  mapped file zero-copy.
- When the overlay grows (default 8192 entries), it is **compacted** into the
  file in the background. Startup is instant: the previous index is mapped on
  launch while a background rebuild re-validates it.

Measured on a real `$HOME` (≈137k files): headless engine idle ≈ **15 MiB**
(was ≈ 80 MiB with the original in-RAM index); warm start ~0.6 s; filename
query < 1 ms. The GUI adds the native window/GL stack (~55 MiB on this box).

## Footprint (budget)

- binaries < 10 MB (stripped, LTO; GUI ≈ 12 MB)
- idle RAM ≈ 15 MiB headless; index on disk (mmap), overlay in RAM
- idle CPU ≈ 0 % (event-driven)
- filename query at 1M entries < 50 ms; content queries stream results

See [`docs/scope.md`](docs/scope.md) for the full scope, design rationale, and
realtime/freshness guarantees.
