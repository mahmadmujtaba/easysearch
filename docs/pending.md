# Pending work

Outstanding items at **v0.40.0** (2026-09-27), in rough priority order. Everything
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

2. **Repository URL.** Done: `packaging/rpm/easysearch.spec.in` and the
   `CHANGELOG` release links all point at the real repository,
   `github.com/mahmadmujtaba/easysearch`.

3. **Deb maintainer** — defaults to
   `EasySearch <easysearch@localhost>`. Override per build:
   `MAINTAINER='You <you@example.com>' make deb`, or change the default in
   `scripts/package-deb.sh`.

4. **Homepage URL** — Done: the metainfo has `<url type="homepage">` (with
   `bugtracker` and `vcs-browser`) and the deb a matching `Homepage:` field, so
   `appstreamcli` no longer warns.

5. **`LICENSE` copyright holder** — currently the neutral
   "2026 EasySearch contributors". Put your name in it.

---

## 3. Known rough edges

- **Flatpak: tray icon is unreliable.** `ksni` needs the session bus
  StatusNotifierWatcher; `--talk-name=org.kde.StatusNotifierWatcher` is granted,
  but sandboxed tray items are the least dependable part of Flatpak. Everything
  else works without it.
- **Flatpak: no network surface to expose.** The engine talks to the app over the
  child's stdin/stdout, so nothing binds a port inside or outside the sandbox —
  there is no host-reaching API and no `--share=network` to add.
- **Flatpak: only `$HOME` is visible.** Grant `--filesystem=host:ro` to search
  the rest of the disk.
- ~~**Empty files have no preview.**~~ **Fixed in v0.37.0** — a 0-byte file shows
  “This file is empty (0 bytes).”
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
| `Tags` tab, tag chips, bulk `Tag` action | **Built in v0.40.0** — a local tag store (`~/.config/easysearch/tags.json`), right-click ▸ **Tags…** (with bulk tagging of the selection), `#chip`s in result rows and the preview, a sidebar **TAGS** section with per-tag counts, filtering by tag, and a **Tag** quick action |
| `Rename` / `Delete` quick actions | Not built (destructive; can be added) |
| `Created` **column** in the table | **Built in v0.27.0** — a sortable *Created* column, read live via `stat` birth time (`—` where the filesystem has none), so no on-disk format change was needed |
| `Full Text (content + name)` scope | **Built in v0.25.0** — the *Full text (name or contents)* scope, `--any`, or `content_or_name` on the API |
| Per-location counts in the sidebar | **Built in v0.26.0** — each quick location shows its own result count for the current query |
| `Follow symlinks` advanced checkbox | **Built in v0.22.0** — Tools ▸ Ignore files… and Settings ▸ Indexing, live via the engine's `config` op |
| `Content: Any` filter | Not built (no meaningful second value today) |

The reference also has a single tab strip; this app keeps its **multi-search
tabs** above the toolbar as well (an earlier explicit request), and uses the
bottom strip for view switching.

## 5. SQLite index — done

The index lives in a SQLite database in a `db/` folder, written by the engine and
read by every consumer (the app, the GUI, the CLI). It is created when missing and
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
| `easysearch --engine` | **51 MiB** | 65 MiB |
| Total resident | **~143 MiB** | — |

For comparison, the budget in [`scope.md` §9](scope.md#9-resource-footprint-budget-non-negotiable-targets)
documents ≈ 151 MiB for the GUI (v0.1.0, 137k files). So memory is at or under
the documented expectation, not a regression.

Worth revisiting if you want it lower: the app and its engine are two processes,
and the GUI keeps an in-process engine as a spawn fallback, so the same
page-cache pages and the same initialisation cost can be paid twice. The
alternative — the GUI strictly as a thin client over the child protocol, with the
engine as the single owner of the index — would remove the fallback's copy of the
index from the resident set.

**Resolved in v0.32.0 — the 1.4 GB `easysearch` process.** An installed build was
seen holding ~1.44 GB resident (100 % anonymous heap, vs 66 MB of database on
disk). The bulk was the optional content cache keeping every searched document's
text in a RAM map, plus glibc malloc arenas: `smaps` showed **22 mappings of
exactly 64 MiB** — arenas that glibc never unmaps. Both levers were moved: the
content cache is now disk-spooled and **off at boot** (see
[`config.md`](config.md) and [`scope.md` §9](scope.md#9-resource-footprint-budget-non-negotiable-targets)),
and every entry point caps `MALLOC_ARENA_MAX=2` before any thread starts. Idle CPU
was measured at 0 %, so CPU was never part of this.

---

## 7. Not started (deliberately deferred)

From [`scope.md` §12](scope.md#12-delivery-phases). Phase 1 is the committed
deliverable; these are gated on request:

**Phase 2 — polish**
- ~~Bundled docx extractor (drop the `docx2txt` dependency)~~ — **done in v0.18.0** (in-process `zip` + `quick-xml` OOXML reader)
- ~~Global hotkey~~ — **done in v0.19.0** (control socket: `easysearch --toggle/--show/--hide/--search/--quit`, bound as a desktop custom shortcut)
- ~~Substring / fuzzy filename ranking (fzf-style)~~ — **done in v0.17.0** (`Query.fuzzy`, `--fuzzy`, the Fuzzy toolbar button, and the fuzzy Relevance ranking)
- ~~`.gitignore` handling UI~~ — **done in v0.16.0** (Tools ▸ Ignore files…, live via the engine's `config` op)

**Phase 3 — stretch**
- `fanotify` watcher (no per-directory watch limits; needs privileges)
- ~~PDF / ODT text extraction~~ — **done in v0.20.0** (ODT via the same `zip` + `quick-xml` package reader; PDF via `pdf-extract`, text layer only, guarded with `catch_unwind`)
- ~~Multiline content regex~~ — **done in v0.21.0** (`--multiline`, the filter-bar "Multiline" chip; off by default because it is much slower)
- Windows / macOS builds

---

## 8. Repo & tooling gaps

- **CI builds the packages.** `.github/workflows/packages.yml` builds the `.deb`
  and the `.rpm` on every push to `master` (i.e. every merged PR) and on every
  pull request, uploading them as the run artifact `easysearch-packages`; pushing
  a `v*` tag additionally attaches them to the GitHub release for that tag. It
  does **not** run the test suite.
- **`dist/` is gitignored**, so the `.deb`, the shareable single binary and any
  future `.rpm`/`.flatpak` are never committed — they are build outputs only.
- **`packaging/flatpak/cargo-sources.json` *is* committed** (522 crates,
  generated from `Cargo.lock`). It must be regenerated — `make cargo-sources` —
  whenever dependencies change, or the Flatpak build will fail offline.
- **Screenshots are committed now.** The 0.12.0 redesign could not be captured at
  the time (KWin's screenshot DBus service stopped replying). `docs/screenshots/`
  now holds real captures of the app in the dark and light themes, used by the
  README and the metainfo; they were taken from an isolated instance indexing a
  throwaway demo tree, so no personal filenames are published.

---

## 9. No in-app updates — deliberately removed

EasySearch has **no network code at all**. An in-place updater (a signed HTTPS
manifest, Ed25519 verification and atomic binary replacement) existed through
v0.37.0, but it was removed in v0.38.0: keeping the app local removes a whole
class of security and privacy concerns, and your package manager is already the
right place to fetch, verify and install software.

- The `self-update` CLI subcommand, `docs/updates.md`,
  `scripts/release-sign.sh`, the `ed25519-dalek`/`sha2` core dependencies and the
  compiled-in release public key are gone.
- Installs and upgrades go through `.deb`/`.rpm` (or Flatpak), built by the same
  CI that already attaches them to a tagged release (see §8).
- There is no update check, no telemetry and no remote API. A release is just a
  newer package.
