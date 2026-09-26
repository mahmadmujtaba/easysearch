# EasySearch

<img src="icons/colored-logo.svg" width="240" align="right" alt="EasySearch">

Realtime filename **and** content search across your filesystem — a Linux
equivalent of VoidTools' *Everything*. Written in **Rust** with a **native GUI**
(egui, no web technologies) and a deliberately small footprint.

- **Realtime** — the kernel reports every create/rename/edit/delete and the
  index reflects it in about a second.
- **Filename search** — Everything-style glob and regex matching, fuzzy
  (fzf-style) matching, per-category counts, sorting.
- **Content search** — the embedded **ripgrep engine** reads files live,
  including the text layer of Word, OpenDocument and PDF files, in-process.
- **Previews** — images, audio/video metadata, document text, rendered
  Markdown, and **syntax-highlighted source code** for ~30 languages.
- **Tags, excluded folders, duplicate finder** — label files, keep whole trees
  out of the index, and move duplicate copies to the **Trash** (recoverable).
- **Lives in the tray** — closing the window keeps the engine indexing; clicking
  the tray icon shows or hides it.
- **Fully offline** — no sockets, no telemetry, no updater. Everything is local.

## Screenshots

The same window in both themes — search box, filter bar, results table, sidebar
and preview pane are all themed.

**Dark** (the default) · **Light**

![EasySearch in the dark theme](docs/screenshots/dark.png)

![EasySearch in the light theme](docs/screenshots/light.png)

_A third **Brand** theme uses the logo's teal palette. The images are shot
against a throwaway demo tree, so no personal filenames appear._

## Install

Packages (see [`docs/packaging.md`](docs/packaging.md)):

```sh
make deb        # dist/easysearch_<version>_<arch>.deb   (dpkg-deb)
make rpm        # → dist/…                               (rpmbuild)
make flatpak    # dist/io.github.easysearch.EasySearch.flatpak
make packages   # everything this machine has tools for
```

Or build and install user-locally — **no root needed**:

```sh
./scripts/install-deps.sh      # user-local Rust + dev libraries
cargo build --release
make install                   # ~/.local/bin + desktop entry, icon, metainfo
```

`make install` puts the binaries in `~/.local/bin/easysearch*` and the desktop
entry, icon and AppStream metadata in `~/.local/share`, so the app shows up in
your launcher with the right icon.

## Quick start

```sh
easysearch                       # the app: window + tray + engine child

# CLI (scriptable; runs the engine in-process)
easysearch-cli search "*.pdf"
easysearch-cli search "report 2026 !draft"
easysearch-cli search TODO --content "TODO"      # search inside files
easysearch-cli status                            # index state and counts

# Control a running window (bind these to desktop shortcuts)
easysearch --toggle              # show the window if hidden, hide it if visible
easysearch --search TODO         # open it and run a search
easysearch --quit                # stop the app and its engine
```

The GUI is a single window with a menu bar, search and filter rows, a results
table, a sidebar (categories, saved searches, tags, locations) and a
preview/details pane. See [`docs/ui.md`](docs/ui.md) for the full guide.

## Supported formats

**Filename search** covers everything the index walks. **Content search** reads
text-like files live, plus these formats, extracted **in-process** (no external
tool):

| Format | Extensions |
| --- | --- |
| Plain text, code, config, logs, CSV, … | anything text-like |
| Microsoft Word | `.docx` |
| LibreOffice / OpenDocument | `.odt` `.ods` `.odp` `.odg` |
| PDF | `.pdf` (text layer only) |

Binary Office formats (`.doc`, `.xls`/`.xlsx`, `.ppt`/`.pptx`) are not extracted
yet. Malformed files are skipped, never fatal.

**Previews** render images, audio/video (metadata from `ffprobe`, a first frame
from `ffmpeg`, both best-effort), the document text above, Markdown, and
**syntax-highlighted code** (Rust, C/C++, Java, Go, Python, JS/TS, shell,
JSON/YAML/TOML/INI, HTML/XML, CSS, SQL, Ruby, PHP, Lua, Kotlin, Swift, C#, R,
Haskell, Scala, Dart, Perl, Nix, Elixir, Erlang, Clojure).

## Configuration

`~/.config/easysearch/config.json` — optional; every key has a default. The
defaults, with what each one does, are in
[`docs/config.md`](docs/config.md). The most useful:

- **`roots`** — directories to index; empty means your home directory.
- **`exclude_dirs`** — whole subtrees to leave out (editable live in
  **Tools ▸ Excluded folders…**).
- **`storage`** — `"sqlite"` (default) or `"mmap"`; `persist_index: false`
  keeps nothing on disk.

The GUI's own state (theme, zoom, tabs, saved searches, tags) lives beside it in
`gui.json` and `tags.json`.

## How it works

One binary is the whole app. `easysearch` opens the window and tray, and spawns
**itself** as the engine (`easysearch --engine`), talking to it as a child
process over stdin/stdout JSON frames — no socket, no port. The engine owns a
live **SQLite** index on disk (so RAM stays low), kept fresh by kernel
filesystem events, and answers content queries with ripgrep's own crates. Close
the window and the engine keeps indexing behind the tray; quench it with
**Quit**.

The design, process model, footprint budget and trade-offs are in
[`docs/scope.md`](docs/scope.md).

## Documentation

| Document | What it covers |
| --- | --- |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Architecture, dev setup, and how to open your first PR |
| [`docs/scope.md`](docs/scope.md) | Design: process model, search semantics, the engine protocol, storage, footprint |
| [`docs/ui.md`](docs/ui.md) | Using the GUI: layout, filters, tags, duplicates, shortcuts |
| [`docs/config.md`](docs/config.md) | Every `config.json` key, its default and its effect |
| [`docs/packaging.md`](docs/packaging.md) | Building `.deb`, `.rpm` and Flatpak packages |
| [`docs/pending.md`](docs/pending.md) | Known gaps and what is planned |
| [`CHANGELOG.md`](CHANGELOG.md) | Release-by-release history |

## Contributing

Contributions are welcome — from a typo fix to a new feature. Start with
[`CONTRIBUTING.md`](CONTRIBUTING.md); it explains the architecture, how to set up
a machine without root, and how to get a first pull request merged.

## License

MIT — see [`LICENSE`](LICENSE).
