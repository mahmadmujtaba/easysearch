# Pending work

Outstanding items at **v0.11.0** (2026-09-24), in rough priority order. Everything
here is either *unverified*, *deliberately deferred*, or a *known rough edge* —
it is not a wishlist.

For what the project is trying to be, see [`scope.md`](scope.md). For the
packaging details, see [`packaging.md`](packaging.md).

---

## 1. Packaging that has not actually been built

`make deb` is verified. The other two are written but **never executed**,
because the tools are not installed on this machine:

| Item | Status | What is needed |
| --- | --- | --- |
| `make deb` | **Verified** — builds, `dpkg-deb --info/--contents` parse, payload extracts, binary reports `0.11.0` | — |
| `make rpm` | **Never built** | `rpmbuild` (`dnf install rpm-build` / `apt install rpm`) |
| `make flatpak` | **Never built** | `flatpak-builder` + `org.freedesktop.Sdk//24.08` + `org.freedesktop.Sdk.Extension.rust-stable//24.08` |
| Installing the `.deb` | **Never done** — only extracted with `dpkg-deb -x` | a machine where you are willing to run `sudo apt install ./dist/*.deb` |
| Publishing anywhere | **Not done** | Flathub / Copr / OBS / AUR / a PPA |

Expect the first Flatpak build to need a round of iteration (runtime version,
SDK extension, `finish-args`). The RPM spec is conventional but has never been
through `rpmbuild`'s eyes — likely small fixes in `%files` or the `Requires:`
sonames.

Both scripts fail fast with install instructions when their tool is missing, so
`make packages` degrades cleanly.

---

## 2. Placeholders to replace before publishing

1. **Application id** — `io.github.everythinglinux.EverythingForLinux` is
   invented. It lives in **10 files**, and two of them are *filenames* that must
   be renamed too:

   | File | Occurrences |
   | --- | --- |
   | `packaging/common/…EverythingForLinux.desktop` *(filename + 2 refs)* | 2 |
   | `packaging/common/…EverythingForLinux.metainfo.xml` *(filename + id, launchable, developer)* | 3 |
   | `packaging/flatpak/…EverythingForLinux.yml` *(filename + app-id + install paths)* | 7 |
   | `packaging/icons/hicolor/scalable/apps/…EverythingForLinux.svg` *(filename)* | — |
   | `Makefile` (`APP_ID`) | 1 |
   | `gui/src/lib.rs` (`APP_ID`, used as the Wayland app id) | 1 |
   | `packaging/rpm/everything-linux.spec.in` (`%global app_id`) | 1 |
   | `scripts/package-deb.sh`, `scripts/package-flatpak.sh` | 1 + 2 |
   | `README.md`, `docs/packaging.md` | 1 + 6 |

   Note: the app id was changed *from* `everything-linux` in v0.11.0, so any KDE
   window rule or shortcut that matched the old class may need re-doing.

2. **Deb maintainer** — defaults to
   `Everything for Linux <everything-linux@localhost>`. Override per build:
   `MAINTAINER='You <you@example.com>' make deb`, or change the default in
   `scripts/package-deb.sh`.

3. **Homepage URL** — there is deliberately no `<url type="homepage">` in the
   metainfo. `appstreamcli` caught that a placeholder URL 404s, and a dead link
   in a launcher is worse than a warning. Add your real URL and the
   `url-homepage-missing` warning disappears.

4. **`LICENSE` copyright holder** — currently the neutral
   "2026 Everything for Linux contributors". Put your name in it.

---

## 3. Known rough edges

- **Flatpak: tray icon is unreliable.** `ksni` needs the session bus
  StatusNotifierWatcher; `--talk-name=org.kde.StatusNotifierWatcher` is granted,
  but sandboxed tray items are the least dependable part of Flatpak. Everything
  else works without it.
- **Flatpak: the HTTP/JSON API is unreachable from the host.** Network is not
  granted, so the daemon binds loopback *inside* the sandbox only. Add
  `--share=network` if you want to `curl` it from outside.
- **Flatpak: only `$HOME` is visible.** Grant `--filesystem=host:ro` to search
  the rest of the disk.
- **Empty files have no preview.** A 0-byte text file shows “No text preview
  for this file.” rather than an empty document.
- **The optional content index has no switch.** `content_index_enabled` exists
  and works, but is only read from `~/.config/everything-linux/config.json` —
  there is no CLI flag and no GUI toggle (the sidebar shows its state read-only).
  Note that file does **not** exist on this machine yet, so the app is running
  entirely on defaults.
- **`everything-gui` is still built but no longer packaged.** If you add a
  binary or a `dlopen`ed library, update three places in step:
  `GUI_DEPENDS` in `scripts/package-deb.sh`, the `Requires:` sonames in
  `packaging/rpm/everything-linux.spec.in`, and the install list in the Flatpak
  manifest.
- **Pre-existing clippy style warnings** — 6 in `gui`, 14 in `core`
  (collapsible `if`s, needless borrows). Cosmetic; no new ones are added by
  recent work.

---

## 4. UI reference: what was not built

The window was rebuilt to the layout in `ui-screenshots/main1.png`. These parts
of that reference are **not** implemented, so they are absent rather than faked:

| Reference element | Status |
| --- | --- |
| `Duplicate Finder` toolbar button | Not built (would be a whole feature) |
| `Fuzzy` toolbar button | Not built (needs fuzzy ranking in the engine) |
| `Tags` tab, tag chips, bulk `Tag` action | Not built (needs a tag store) |
| `Rename` / `Delete` quick actions | Not built (destructive; can be added) |
| `Created` **column** in the table | Deferred: birth time is shown in the Details tab, but a table column would need `btime` stored in the on-disk index (a format change) |
| `Full Text (content + name)` scope | Offered as `Filenames` / `Full path` / `Contents` instead: the engine ANDs name and content, so an OR mode needs an engine change |
| Per-location counts in the sidebar | Locations are listed with their paths; only the categories carry live counts |
| `Follow symlinks` advanced checkbox | Not built (the walker does not follow symlinks) |
| `Content: Any` filter | Not built (no meaningful second value today) |

The reference also has a single tab strip; this app keeps its **multi-search
tabs** above the toolbar as well (an earlier explicit request), and uses the
bottom strip for view switching.

## 5. Measured footprint (for reference)

Taken on this machine: KDE/Plasma on Wayland, release build, ~85 000 files,
~45 s after launch (index settled).

| Process | RSS | Peak RSS |
| --- | --- | --- |
| `everything-linux` (GUI) | **92 MiB** | 94 MiB |
| `everything-linux --daemon` | **51 MiB** | 65 MiB |
| Total resident | **~143 MiB** | — |

For comparison, the budget in [`scope.md` §9](scope.md#9-resource-footprint-budget-non-negotiable-targets)
documents ≈ 151 MiB for the GUI (v0.1.0, 137k files). So memory is at or under
the documented expectation, not a regression.

Worth revisiting if you want it lower: the GUI and the daemon are two processes
that each map the same index, so the *same* page-cache pages and the same
initialisation cost are paid twice. The alternative — the GUI as a thin client
over the HTTP API, with a single owner of the index — would remove one process
and its copy of the index from the resident set.

---

## 6. Not started (deliberately deferred)

From [`scope.md` §12](scope.md#12-delivery-phases). Phase 1 is the committed
deliverable; these are gated on request:

**Phase 2 — polish**
- Bundled docx extractor (drop the `docx2txt` dependency)
- Global hotkey
- Substring / fuzzy filename ranking (fzf-style)
- `.gitignore` handling UI

**Phase 3 — stretch**
- `fanotify` watcher (no per-directory watch limits; needs privileges)
- Multiline content regex
- PDF / ODT text extraction
- Windows / macOS builds

---

## 7. Repo & tooling gaps

- **No CI.** There is no `.github/`, `.gitlab-ci.yml` or similar. Nothing runs
  the test suite or the packaging scripts automatically; `cargo test
  --workspace` (42 tests) and `make validate-packaging` are manual.
- **`dist/` is gitignored**, so the `.deb`, the shareable single binary and any
  future `.rpm`/`.flatpak` are never committed — they are build outputs only.
- **`packaging/flatpak/cargo-sources.json` *is* committed** (491 crates,
  generated from `Cargo.lock`). It must be regenerated — `make cargo-sources` —
  whenever dependencies change, or the Flatpak build will fail offline.
- **Screenshots could not be captured** while the 0.12.0 UI was being built:
  KWin's screenshot DBus service stopped replying
  (`KWin screenshot request failed: Did not receive a reply`), for `spectacle`
  and with the app closed too. The redesign was therefore verified by building,
  running and reading the layout code, not visually — it is worth a look on
  first launch.
