# Pending work

Outstanding items at **v0.15.0** (2026-09-26), in rough priority order. Everything
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

1. **Application id — settled.** The project was renamed to **EasySearch** in
   v0.23.0, and the app id is now
   `io.github.easysearch.EasySearch`. It is consistent across the whole tree
   (the Wayland app id in `gui/src/lib.rs`, the `Makefile`'s `APP_ID`, the
   desktop entry, metainfo, Flatpak manifest and RPM spec, and the
   `packaging/common/…EasySearch.*` / `packaging/icons/…EasySearch.svg`
   filenames). If you change it again, those are the places, and two of them are
   *filenames*:

   | File | What changes |
   | --- | --- |
   | `packaging/common/…EasySearch.desktop` *(filename + 2 refs)* | id |
   | `packaging/common/…EasySearch.metainfo.xml` *(filename + id, launchable, developer)* | id |
   | `packaging/flatpak/…EasySearch.yml` *(filename + app-id + install paths)* | id |
   | `packaging/icons/hicolor/scalable/apps/…EasySearch.svg` *(filename)* | filename |
   | `Makefile` (`APP_ID`), `gui/src/lib.rs` (`APP_ID`) | id |
   | `packaging/rpm/easysearch.spec.in` (`%global app_id`) | id |
   | `scripts/package-{deb,flatpak}.sh` | id |
   | `README.md`, `docs/packaging.md` | prose |

   Note the id is also the Wayland `app_id`, so a KDE window rule or shortcut
   that matched the previous class needs re-doing.

2. **Repository URL.** `packaging/rpm/easysearch.spec.in` and
   `core/src/update.rs` still point at placeholder GitHub paths
   (`github.com/easysearch/easysearch`). Replace them with the real host once the
   repository exists — the updater's `DEFAULT_MANIFEST_URL` is the same URL.

3. **Deb maintainer** — defaults to
   `EasySearch <easysearch@localhost>`. Override per build:
   `MAINTAINER='You <you@example.com>' make deb`, or change the default in
   `scripts/package-deb.sh`.

4. **Homepage URL** — there is deliberately no `<url type="homepage">` in the
   metainfo. `appstreamcli` caught that a placeholder URL 404s, and a dead link
   in a launcher is worse than a warning. Add your real URL and the
   `url-homepage-missing` warning disappears.

5. **`LICENSE` copyright holder** — currently the neutral
   "2026 EasySearch contributors". Put your name in it.

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
- **`easysearch-gui` is still built but no longer packaged.** If you add a
  binary or a `dlopen`ed library, update three places in step:
  `GUI_DEPENDS` in `scripts/package-deb.sh`, the `Requires:` sonames in
  `packaging/rpm/easysearch.spec.in`, and the install list in the Flatpak
  manifest.
- **Clippy is clean.** The 20-odd pre-existing style warnings (collapsible `if`s,
  needless `map_or`, single-pattern `match`es, `Default` reassignment in tests)
  were fixed in v0.22.1; `cargo clippy --workspace --all-targets` reports nothing,
  and the two genuinely long internal signatures carry an `#[allow]` with a note
  rather than a cosmetic reshuffle.

---

## 4. UI reference: what was not built

The window was rebuilt to the layout in `ui-screenshots/main1.png`. These parts
of that reference are **not** implemented, so they are absent rather than faked:

| Reference element | Status |
| --- | --- |
| `Duplicate Finder` toolbar button | **Built in v0.14.0** — right-click a selection ▸ *Find duplicates in results* (also *in selection*) |
| `Fuzzy` toolbar button | **Built in v0.17.0** — fzf-style subsequence matching, with a matching Relevance score |
| `Tags` tab, tag chips, bulk `Tag` action | Not built (needs a tag store) |
| `Rename` / `Delete` quick actions | Not built (destructive; can be added) |
| `Created` **column** in the table | **Built in v0.27.0** — a sortable *Created* column, read live via `stat` birth time (`—` where the filesystem has none), so no on-disk format change was needed |
| `Full Text (content + name)` scope | **Built in v0.25.0** — the *Full text (name or contents)* scope, `--any`, or `content_or_name` on the API |
| Per-location counts in the sidebar | **Built in v0.26.0** — each quick location shows its own result count for the current query |
| `Follow symlinks` advanced checkbox | **Built in v0.22.0** — Tools ▸ Ignore files… and Settings ▸ Indexing, live via `POST /v1/config` |
| `Content: Any` filter | Not built (no meaningful second value today) |

The reference also has a single tab strip; this app keeps its **multi-search
tabs** above the toolbar as well (an earlier explicit request), and uses the
bottom strip for view switching.

## 5. SQLite index — done

The index lives in a SQLite database in a `db/` folder, written by the daemon and
read by every consumer (GUI, CLI, HTTP API). It is created when missing and
rebuilt when the schema changes or a build was interrupted; live changes are
folded into it in batched transactions before each query, and a large delta
backlog triggers a full rebuild. Design and measurements are in
[`sqlite.md`](sqlite.md).

`storage = "mmap"` in `~/.config/easysearch/config.json` falls back to the
original memory-mapped index.

## 6. Measured footprint (for reference)

Taken on this machine: KDE/Plasma on Wayland, release build, ~85 000 files,
~45 s after launch (index settled).

| Process | RSS | Peak RSS |
| --- | --- | --- |
| `easysearch` (GUI) | **92 MiB** | 94 MiB |
| `easysearch --daemon` | **51 MiB** | 65 MiB |
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

## 7. Not started (deliberately deferred)

From [`scope.md` §12](scope.md#12-delivery-phases). Phase 1 is the committed
deliverable; these are gated on request:

**Phase 2 — polish**
- ~~Bundled docx extractor (drop the `docx2txt` dependency)~~ — **done in v0.18.0** (in-process `zip` + `quick-xml` OOXML reader)
- ~~Global hotkey~~ — **done in v0.19.0** (control socket: `easysearch --toggle/--show/--hide/--search/--quit`, bound as a desktop custom shortcut)
- ~~Substring / fuzzy filename ranking (fzf-style)~~ — **done in v0.17.0** (`Query.fuzzy`, `--fuzzy`, `?fuzzy=1`, the Fuzzy toolbar button, and the fuzzy Relevance ranking)
- ~~`.gitignore` handling UI~~ — **done in v0.16.0** (Tools ▸ Ignore files…, live via `POST /v1/ignore`)

**Phase 3 — stretch**
- `fanotify` watcher (no per-directory watch limits; needs privileges)
- ~~PDF / ODT text extraction~~ — **done in v0.20.0** (ODT via the same `zip` + `quick-xml` package reader; PDF via `pdf-extract`, text layer only, guarded with `catch_unwind`)
- ~~Multiline content regex~~ — **done in v0.21.0** (`--multiline`, `?multiline=1`, the filter-bar "Multiline" chip; off by default because it is much slower)
- Windows / macOS builds

---

## 8. Repo & tooling gaps

- **No CI.** There is no `.github/`, `.gitlab-ci.yml` or similar. Nothing runs
  the test suite or the packaging scripts automatically; `cargo test
  --workspace` and `make validate-packaging` are manual.
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

---

## 9. Self-update — what is deliberately not done yet

The in-place updater is complete and tested end-to-end (13 unit tests plus a
manual run against real `openssl`-signed artifacts); see
[`updates.md`](updates.md). These pieces are intentionally outside the repo or
not yet built:

- **No release host is configured.** `DEFAULT_MANIFEST_URL` is a placeholder
  GitHub path. Point it (or `EASYSEARCH_UPDATE_URL`) at the real host.
- **No public release has been signed yet.** The keypair exists and its public
  half is compiled into the binary; the private half lives at
  `~/.config/easysearch/release-signing-key.pem` (never committed). Back it
  up — losing it means future updates are refused by existing installs.
- **Manual release step.** Nothing runs `scripts/release-sign.sh`
  automatically; wire it into CI when there is one.
- **No key rotation / revocation.** Changing the key needs a rebuild with the new
  public key embedded; a follow-up could accept a signed *key-change* manifest.
- **Only Linux ELF binaries.** The manifest is per-target, but no Windows/macOS
  packaging exists (see §7).
