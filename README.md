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
- **Lives in the tray** — closing the window exits the window only; the app keeps
  indexing in the background, and the tray icon reopens it (left-click opens ready
  to search; the right-click menu adds clear-results, rebuild, recent searches,
  settings and more).
- **Fully offline** — nothing on the network, no telemetry, no updater.

## Install

Prebuilt packages are attached to each
[release](https://github.com/mahmadmujtaba/easysearch/releases). Download the
latest one for your distribution and install it **however you prefer** — the
command line, or your desktop's usual app store.

### Debian / Ubuntu, and derivatives (`.deb`)

Download `easysearch_<version>_amd64.deb` from the
[latest release](https://github.com/mahmadmujtaba/easysearch/releases/latest),
then pick a way that suits you:

- **Command line** — `apt` pulls in the GUI libraries for you:

  ```sh
  sudo apt install ./easysearch_<version>_amd64.deb
  ```

  `sudo dpkg -i …` works too; `sudo apt-get -f install` fixes a missing
  dependency afterwards.
- **A GUI package manager** — just open the downloaded `.deb` with your usual
  installer: **GDebi**, **Synaptic** ( *File ▸ Add downloaded packages…* ) or
  **Discover**, or double-click it in your file manager. They install it and any
  dependencies for you.

### Fedora, RHEL, openSUSE, and derivatives (`.rpm`)

Download `easysearch-<version>-1.x86_64.rpm` from the same release page, then:

- **Command line:**

  ```sh
  sudo dnf install ./easysearch-<version>-1.x86_64.rpm       # Fedora / RHEL
  sudo zypper install ./easysearch-<version>-1.x86_64.rpm    # openSUSE
  ```

  `sudo rpm -i …` works too.
- **A GUI installer** — open the downloaded `.rpm` with **GNOME Software**, KDE
  **Discover**, or **YaST**; they install it and its dependencies for you. In KDE
  you can also right-click the file in Dolphin ▸ *Open With ▸ Discover*.

Either package installs `/usr/bin/easysearch` (the app: host + window + engine),
`easysearch-cli`, `easysearch-daemon`, the desktop entry and the icon, so
EasySearch appears in your launcher straight away.

> **Flatpak is coming.** A manifest and vendored sources live in
> [`packaging/flatpak/`](packaging/flatpak/), but a bundle is not published yet.

### From source (no root)

```sh
git clone https://github.com/mahmadmujtaba/easysearch.git
cd easysearch
./scripts/install-deps.sh      # user-local Rust + dev libraries
cargo build --release
make install                   # ~/.local/bin + desktop entry, icon, metainfo
```

`make install` puts the binaries in `~/.local/bin/easysearch*` and the desktop
entry, icon and AppStream metadata in `~/.local/share`, so the app shows up in
your launcher with the right icon. Building the packages yourself (`make deb`,
`make rpm`, `make flatpak`) is covered in [`docs/packaging.md`](docs/packaging.md).

## Quick start

```sh
easysearch                       # the app: starts the host, then opens a window

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
- **`exclude_names_file`** — directory names skipped anywhere (virtual
  environments, `node_modules`, `target`, …); a common list is seeded at
  `~/.config/easysearch/exclude-names`. Hidden dot-directories are skipped too
  (`index_hidden_dirs: false`), so they never reach the index.
- **`storage`** — `"sqlite"` (default) or `"mmap"`; `persist_index: false`
  keeps nothing on disk.

The GUI's own state (theme, zoom, tabs, saved searches, tags) lives beside it in
`gui.json` and `tags.json`.

## How it works

One binary is the whole app, run as a **background host** and a **window**.
`easysearch` with no arguments makes sure the host is running — a detached
process (`easysearch --daemon`) that owns the tray, the control socket and the
live **SQLite** index — and asks it to open a window (`easysearch --window`).
The host spawns each window as a child and they exchange newline-delimited JSON
frames over the child's stdin/stdout; no socket to the network, no port. The
index is kept fresh by kernel filesystem events, and content queries use
ripgrep's own crates.

Closing the window (its X, the tray toggle, `--hide`) ends the **window** process
and nothing else: the host keeps indexing, so reopening is instant. Only
**Quit** stops the host and its engine.

**Why a host process?** On Wayland a window cannot be unmapped and `winit`
forbids recreating its event loop, so a window that is *closed* could not be
reopened inside the same process. Putting the tray and the index in a host that
outlives any window is what lets "close" mean the window is really gone while
the app keeps running. The design, process model, footprint budget and trade-offs
are in [`docs/scope.md`](docs/scope.md).

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

## Note to users: this project is AI-assisted

**EasySearch is developed with AI assistance.** Most of the code, tests and
documentation are written by an AI coding agent working from the maintainer's
direction, then reviewed and tested by a human before it is released. It is a
real, working program — but it is also a young one, so please treat it that way:
keep backups of anything precious, expect the occasional rough edge, and report
what you find. Bug reports, corrections and patches are genuinely welcome.

## Contributing

Contributions are welcome — from a typo fix to a new feature. Start with
[`CONTRIBUTING.md`](CONTRIBUTING.md); it explains the architecture, how to set up
a machine without root, and how to get a first pull request merged.

## License

MIT — see [`LICENSE`](LICENSE).
