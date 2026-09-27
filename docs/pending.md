# Known gaps & roadmap

Open, unresolved items only — shipped work lives in
[`../CHANGELOG.md`](../CHANGELOG.md), and the design is in
[`scope.md`](scope.md). Each entry is something we know is *unverified*,
*deliberately deferred*, or a *known rough edge* — not a wishlist.

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

## Repo & tooling

- **CI builds the packages, not the tests.** `.github/workflows/packages.yml`
  builds the `.deb` and `.rpm` on every push to `master` and every PR (uploaded
  as a run artifact), and a `v*` tag attaches them to the GitHub release. The
  test suite is run by the contributor, not CI.
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
