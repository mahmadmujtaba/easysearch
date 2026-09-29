# Known gaps & roadmap

Open, unresolved items only — shipped work lives in
[`../CHANGELOG.md`](../CHANGELOG.md), and the design is in
[`scope.md`](scope.md). Each entry is something we know is *unverified*,
*deliberately deferred*, or a *known rough edge*. The **feature backlog** near
the end is the one exception — a requested wishlist, kept deliberately separate.

## Packaging

| Item | Status | Needs |
| --- | --- | --- |
| `make deb` | **Verified** — builds, `dpkg-deb --info/--contents` parse, payload extracts | — |
| `make rpm` | Built and reproduced in an `ubuntu:24.04` container; never on a real rpm distro | a Fedora/openSUSE machine |
| `make flatpak` | **Never built** | `flatpak-builder` + the freedesktop SDK |
| Installing the `.deb` manually | Not exercised here (user installs it) | — |
| arm64 packages | Built in CI on `ubuntu-24.04-arm` (best-effort, non-blocking) | a confirmed arm runner on the repo |
| Package signing | **Unsigned** — no GPG key in CI | a signing key as a CI secret |
| Publishing anywhere | **Not done** (GitHub releases only) | Flathub / Copr / OBS / AUR / a PPA — see [`packaging.md`](packaging.md) |

Both package scripts fail fast with install instructions when their tool is
missing, so `make packages` degrades cleanly. Details:
[`packaging.md`](packaging.md).

## Before publishing

1. **App id** — `io.github.easysearch.EasySearch` names the desktop file, the
   metainfo `<id>`/`<launchable>`, the Flatpak `app-id` and `APP_ID` in the
   `Makefile`. Settle it before publishing: changing it later moves the desktop
   entry and icon with it (the on-disk data lives under a fixed
   `~/.config/easysearch`, so it is not affected).
2. The AppStream metainfo carries a `<release>` entry that is edited by hand each
   release; keep it in step with `VERSION` (or wire it into the tag build).
3. **Packages are unsigned.** CI builds them without a signing key, so there is
   no `Release`/`InRelease` and no repository — the `.deb`/`.rpm` are downloaded
   from the release page and installed by hand.

## Known rough edges

- **Flatpak: the tray is unreliable.** `ksni` needs the session
  StatusNotifierWatcher; the sandbox permission is granted, but sandboxed tray
  items are the least dependable part of Flatpak. Everything else works.
- **Flatpak: only `$HOME` is visible.** Grant `--filesystem=host:ro` to search
  the rest of the disk.
- **Closing the window with no tray host leaves the host headless.** The window
  process exits (the host keeps indexing, as designed), but with no tray icon
  there is nothing to reopen it — use `easysearch --show` or the launcher. With
  a StatusNotifier host (most desktops) this does not arise.
- **`easysearch-gui`** is still built but no longer packaged. If you add a
  binary or a `dlopen`ed library, update three places in step: `GUI_DEPENDS` in
  `scripts/package-deb.sh`, the RPM `Requires:` sonames, and the Flatpak
  manifest.
- **Office binaries.** Excel `.xls`/`.xlsx` and PowerPoint `.ppt`/`.pptx` are
  not text-extracted yet (and Word `.doc`/PDF scans have no text layer).

## Not started (deliberately deferred)

- `fanotify` watcher (no per-directory watch limits; needs privileges).
- Windows / macOS builds (the code stays portable).
- **SQLite FTS5** for content search — the natural next step for the index.
- A **tag/ignore** picker for paths outside the results (today you tag from a
  result row).

## Feature backlog (wishlist, shortest first)

Requested additions, ordered roughly from the smallest to the largest. None need
a network or a model — everything is local. Sizes are focused-work estimates with
tests and docs: **S** ≈ ½ day, **M** ≈ 1–3 days, **L** ≈ 3–6 days.

### S — small

- **Copy as…** — copy the paths as `file://` URIs or shell-escaped. **Done.**
- Keyboard extras on the results: `Ctrl+Enter` open containing folder, `F5` re-run
  the search, `Alt+↑` go to the parent location.
- Export the current results as CSV / TSV / JSON.
- **Open at login** — a toggle that writes an XDG autostart entry (or a systemd
  user unit).
- **Find by hash** — paste a SHA-256 to locate the file.
- Extra cleanup filters — empty files/folders and broken symlinks.
- Duplicate finder: ignore hardlinks / same-inode files.

### M — medium

- **Filter-within-results** box — narrow the visible list without re-querying.
- **Query tokens** — `ext:pdf size:>10MB modified:today folder: parent: file:`,
  plus `|` (OR) and quoted phrases; most map onto fields `Query` already has.
- **Advanced Search** dialog — a builder that composes those tokens.
- **Column chooser** — show/hide/reorder columns, add Extension / Owner / Group /
  Permissions / Inode / Link count, persisted per tab.
- **Rename in place** (`F2`) with clobber checks.
- **Restore from Trash** in-app — list trashed items with original paths.
- **Open with…** — pick an application, remembered per extension.
- **Follow the system theme** — dark/light and the accent colour via the
  freedesktop portal.
- **Index diagnostics** — last scan, watcher health, and paths skipped for
  permissions.
- **Pause indexing** plus battery/thermal throttling (UI + tray).
- **Command palette** (`Ctrl+Shift+P`) over the app's actions.
- **Editable breadcrumb** path bar with autocomplete over the index.
- **CLI parity** — `--json` output, the same filters, and `watch "query"` to
  stream new matches.
- **Multiple index roots** managed in the UI, each with its own count and status.
- **Bulk rename** by pattern (find/replace, numbering).
- **Two-pane copy/move**.
- **Drag rows out** to a file manager or another app (XDG drag-and-drop).

### L — large

- **Global hotkey** via the XDG `GlobalShortcuts` portal (Wayland) plus a native
  X11 grab — today the CLI is hand-bound by the user.
- **Structured viewers** — CSV/TSV tables, JSON/XML trees, and a hex view for
  binaries.
- **Removable-media indexing** — detect mounts, index on mount, drop on unmount.
- **Browse and search inside archives** (zip/tar/7z) without extracting.
- **Font preview** and first-page thumbnails for documents.
- **Accessibility sweep** — screen-reader labels (accesskit), high-contrast and
  reduced-motion options.
- **Live-updating results** — add/remove rows as the index changes while a query
  is open.

## Repo & tooling

- **CI builds the packages, not the tests.** `.github/workflows/packages.yml`
  builds the `.deb` and `.rpm` only when a `v*` tag is pushed (or on demand via
  *Run workflow*) and attaches them to the GitHub release; ordinary pushes to
  `master` and pull requests build nothing. The test suite is run by the
  contributor, not CI.
- **The end-to-end engine suite is `#[ignore]`d.** Every test in
  `daemon/tests/roundtrip.rs` spawns a real engine and waits for it to index, so
  the suite is slow. Run it on demand with
  `cargo test -p easysearch-daemon --test roundtrip -- --ignored`.
- **`packaging/flatpak/cargo-sources.json` is committed** (522 crates, generated
  from `Cargo.lock`). Regenerate it with `make cargo-sources` whenever
  dependencies change, or the offline Flatpak build fails.
- **Screenshots are committed** in `docs/screenshots/` (dark + light), used by
  the README and the metainfo. They come from an isolated instance indexing a
  throwaway demo tree, so no personal filenames are published. They still show
  the pre-quick-actions UI and there is no Brand-theme shot; re-shoot before a
  Flathub submission (its screenshot requirements are stricter).
