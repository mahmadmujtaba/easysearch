# Packaging

EasySearch builds three distribution formats from one set of assets:

| Format | Command | Output | Needs |
| --- | --- | --- | --- |
| Debian | `make deb` | `dist/easysearch_<version>_<arch>.deb` | `dpkg-deb` / `dpkg-dev` |
| RPM | `make rpm` | `~/rpmbuild/RPMS/*/easysearch-<version>-*.rpm`, copied to `dist/` | `rpmbuild` |
| Flatpak | `make flatpak` | `dist/io.github.easysearch.EasySearch.flatpak` | `flatpak-builder` + the freedesktop SDK |

`make packages` builds whichever of the three this machine has tools for, and
says so for the ones it skips. `make validate-packaging` runs
`desktop-file-validate` and `appstreamcli` over the desktop entry and the
AppStream metadata. Nothing here needs `sudo`.

On Debian/Ubuntu the RPM is built with the distro `rpm` package (which provides
`rpmbuild`), but the Rust toolchain comes from rustup and is not an rpm, so the
spec's `BuildRequires` cannot be satisfied; `scripts/package-rpm.sh` therefore
passes `rpmbuild --nodeps`, the tools being supplied by the environment (on
Fedora the equivalent is `dnf builddep`).

## What gets installed

All three formats install the same payload:

```
/usr/bin/easysearch            the app: host, window and engine in one file
/usr/bin/easysearch-cli        scriptable CLI
/usr/bin/easysearch-daemon     standalone engine (stdio protocol; tests, scripts, headless)
/usr/share/applications/io.github.easysearch.EasySearch.desktop
/usr/share/metainfo/io.github.easysearch.EasySearch.metainfo.xml
/usr/share/icons/hicolor/…/apps/io.github.easysearch.EasySearch.{svg,png}
/usr/share/doc/easysearch/README.md          both formats
/usr/share/doc/easysearch/copyright          the deb's licence
/usr/share/doc/easysearch/LICENSE            the rpm's licence
```

`easysearch-gui` is a development convenience that duplicates `easysearch`
(which already provides the GUI *and* the engine) and would add ~17 MB, so it is
not packaged; `make install` still installs it locally.

`make install` is the user-local counterpart: binaries into `~/.local/bin`, and
the desktop entry, AppStream metadata and hicolor icon into `~/.local/share/…`.
`make uninstall` removes all of it. The desktop entry and icon are what put the
app in the launcher — and what let the compositor resolve the logo for the
*window decoration* on Wayland, where a window carries no icon of its own.

## Layout

```
packaging/
  common/    desktop entry + AppStream metainfo (shared by all formats)
  icons/     hicolor icon tree; SVG is icons/transparent-logo.svg under the app
             id, PNGs rendered at build time
  debian/    control template (substituted by the build script)
  rpm/       spec template (substituted by the build script)
  flatpak/   manifest + generated cargo-sources.json
scripts/
  package-deb.sh      staging tree → dpkg-deb
  package-rpm.sh      source tarball → rpmbuild
  package-flatpak.sh  cargo sources → flatpak-builder → build-bundle
  gen-cargo-sources.py  Cargo.lock → cargo-sources.json (offline, checksummed)
```

## Icons and identifiers

The packaging SVG is `icons/transparent-logo.svg` plus a header comment; the app
also embeds a 512 px transparent render (`core/assets/logo.png`) for its window
icon, tray pixmap and About dialog. The two are kept in step by hand. `make deb`
additionally renders PNG icons from the SVG.

The maintainer is **EasySearch maintainers <ahmad.mujtaba11@gmail.com>** — the deb
`Maintainer`, the rpm `Packager` and the rpm changelog entry; override a single
build with `MAINTAINER='Name <mail>' make deb`. The app id
`io.github.easysearch.EasySearch` names the desktop file, the metainfo
`<id>`/`<launchable>`, the Flatpak `app-id` and `APP_ID` in the `Makefile`;
settle it before publishing. `VERSION` is read by `Cargo.toml`, the deb, the spec
and the Flatpak build.

## Flatpak

The manifest is `packaging/flatpak/io.github.easysearch.EasySearch.yml`. The
sandbox builds fully offline against vendored crates, so every crates.io
dependency is declared in `packaging/flatpak/cargo-sources.json`, generated from
`Cargo.lock` by `scripts/gen-cargo-sources.py` (it reads the local cargo cache
and verifies each crate against its `Cargo.lock` checksum, so it needs no
network). `make flatpak` refreshes it on every build; the file is committed so
`flatpak-builder` also works when invoked directly.

The sandbox only sees what `finish-args` grants: `--filesystem=home` (the app's
default search root; add `--filesystem=host:ro` to search more), no network (the
host and its windows talk over private pipes and a local Unix socket, so nothing
is reachable over the network), and `--talk-name=org.kde.StatusNotifierWatcher`
for the tray. Everything else the app touches is under `$HOME` /
`$XDG_CACHE_HOME`.

## Dependencies are listed by hand

Debian and RPM both derive the dependencies of *linked* libraries automatically
(`dpkg-shlibdeps`, `%{?elfdeps}`), but the GUI stack (winit/glutin) opens its
libraries with `dlopen()` at runtime, so the scanners cannot see them. Those are
declared explicitly: package names in the deb (`GUI_DEPENDS` in
`scripts/package-deb.sh`, verified with `dpkg -S`) and sonames in the rpm
(`Requires: libEGL.so.1()(64bit)`, … — distribution-independent). A dependency
loaded this way must be added in **both** places, or the two formats drift.

## Continuous integration

`.github/workflows/packages.yml` builds the `.deb` and `.rpm` on every push to
`master` and every pull request, running the same `scripts/package-deb.sh` and
`scripts/package-rpm.sh` you would run locally, and uploads them plus a
`SHA256SUMS` as the artifact `easysearch-packages` (kept 90 days). A `v*` tag
additionally attaches the two packages and `SHA256SUMS` to the GitHub release. It
needs no secrets and no root, and it does **not** run the test suite. A newer
push cancels an in-flight build (tag builds are never cancelled).

## What has been verified

* `make deb` → a valid `.deb`: `dpkg-deb --info`/`--contents` parse, the staging
  tree is `root/root` at 0755/0644, and the payload contains the desktop entry,
  metainfo, SVG and PNG icons. `desktop-file-validate` passes and
  `appstreamcli validate` reports no errors.
* `scripts/gen-cargo-sources.py` regenerates all 522 crates.io sources, each
  checksum matching `Cargo.lock`.
* The **RPM**, which this machine cannot build, was reproduced in an
  `ubuntu:24.04` container: `rpmbuild --nodeps -bb` completes, the payload is the
  three binaries plus the desktop entry, metainfo, SVG and two doc files with no
  unpackaged files, and `Requires` carry the `dlopen`ed sonames.
* **Not verified:** `make flatpak` — `flatpak-builder` is not installed here. The
  manifest and generated sources are in place, but a build has not been run;
  expect the usual first-run iteration (runtime/extension versions).

The rpm and flatpak scripts fail fast with install instructions when their tool
is missing, so `make packages` on a machine without them degrades cleanly.
