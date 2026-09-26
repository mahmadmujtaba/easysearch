# Packaging

EasySearch builds three distribution formats from one set of assets:

| Format | Command | Output | Needs |
| --- | --- | --- | --- |
| Debian | `make deb` | `dist/easysearch_<version>_<arch>.deb` | `dpkg-deb` (any Debian/Ubuntu) |
| RPM | `make rpm` | `~/rpmbuild/RPMS/*/easysearch-<version>-*.rpm`, copied to `dist/` | `rpmbuild` |
| Flatpak | `make flatpak` | `dist/io.github.easysearch.EasySearch.flatpak` | `flatpak-builder` + the freedesktop SDK |

`make packages` builds whichever of the three this machine has tools for, and
says so for the ones it skips. `make validate-packaging` checks the desktop
entry and the AppStream metadata.

Nothing here needs `sudo`.

On Debian and Ubuntu the RPM is built with the distribution's `rpm` package,
which provides `rpmbuild`. That environment has no rpm database entry for Rust
(the toolchain comes from rustup), so the spec's `BuildRequires` cannot be
satisfied; `scripts/package-rpm.sh` therefore passes `rpmbuild --nodeps`, the
tools being supplied by the environment. On Fedora the same job is done by
`dnf builddep` before the build. Nothing else about the build changes: `%prep`,
`%build` (cargo), `%install` and `%files` all run as written.

## What gets installed

All three formats install the same payload:

```
/usr/bin/easysearch                  the app: GUI + search daemon in one file
/usr/bin/easysearch-cli                        scriptable CLI
/usr/bin/easysearch-daemon                 headless daemon (HTTP/JSON API)
/usr/share/applications/io.github.easysearch.EasySearch.desktop
/usr/share/metainfo/io.github.easysearch.EasySearch.metainfo.xml
/usr/share/icons/hicolor/{scalable,64x64,128x128,256x256,512x512}/apps/io.github.easysearch.EasySearch.{svg,png}
/usr/share/doc/easysearch/README.md          both formats
/usr/share/doc/easysearch/copyright          the deb's licence
/usr/share/doc/easysearch/LICENSE            the rpm's licence
```

`easysearch-gui` is a development convenience that duplicates `easysearch`
(which already runs the GUI *and* the daemon) and would add another ~17 MB, so
it is deliberately not packaged. `make install` still installs it locally.

The installed size of the `.deb` is ~23 MB (6.4 MB compressed), essentially all
of it the three Rust binaries.

## Layout

```
packaging/
  common/           desktop entry + AppStream metainfo (shared by all formats)
  icons/            hicolor icon theme tree (SVG; PNGs are rendered at build time).
                    The mark is drawn in code too — see `gui/src/logo.rs`, which
                    the SVG mirrors shape for shape.
  debian/           control template (substituted by the build script)
  rpm/              spec template (substituted by the build script)
  flatpak/          manifest + generated cargo-sources.json
scripts/
  package-deb.sh    staging tree → dpkg-deb
  package-rpm.sh    source tarball → rpmbuild
  package-flatpak.sh  cargo sources → flatpak-builder → build-bundle
  gen-cargo-sources.py  Cargo.lock → cargo-sources.json (offline, checksummed)
```

## Continuous integration

`.github/workflows/packages.yml` builds both formats on every push to `master`
— which is what a merged pull request produces — and uploads them to the run as
the artifact `easysearch-master-packages`: the `.deb`, the `.rpm` and a
`SHA256SUMS`, kept for 90 days. *Run workflow* rebuilds by hand, and a newer push
cancels a build already in flight.

The job installs `rpm` (which provides `rpmbuild` on Ubuntu) and then runs the
same `scripts/package-deb.sh` and `scripts/package-rpm.sh` you would run locally,
so CI is a second check on `make deb` / `make rpm` rather than a parallel
implementation. It needs no secrets and no root. Artifacts are **not** published
anywhere — there is no release job yet, so nothing is signed or uploaded to a
release host.

The checkout is a plain clone: `.cargo/config.toml` is deliberately not tracked
(it is generated per machine by `scripts/install-deps.sh` and embeds absolute
paths), so a clone and CI link with the system C compiler. Run that script if
your machine has no compiler.

## Before you publish

These are placeholders and should be changed to your own identifiers:

1. **Application id.** `io.github.easysearch.EasySearch` is used in
   the desktop entry file name, the metainfo `<id>` and `<launchable>`, the
   Flatpak `app-id`, and in `APP_ID` in the `Makefile`. It is also the one
   `appstreamcli --pedantic` note left (`cid-contains-uppercase-letter`, because
   of the `EasySearch` segment); that is not an error and validation passes.
2. **Maintainer.** `scripts/package-deb.sh` defaults the deb maintainer to
   `EasySearch <easysearch@localhost>`; override it with
   `MAINTAINER='Your Name <you@example.com>' make deb`, or edit the default.
3. **Homepage.** Done: the metainfo carries `<url type="homepage">` (plus
   `bugtracker` and `vcs-browser`) pointing at
   <https://github.com/mahmadmujtaba/easysearch>, and the deb's `Homepage:`
   field matches, so the old `url-homepage-missing` warning is gone.
4. **Version.** Bump `VERSION`; `Cargo.toml`, the deb, the spec and the Flatpak
   build all read from it.

## Dependencies, and why they are listed by hand

Debian and RPM both compute the dependencies of the *linked* libraries
automatically (`dpkg-shlibdeps`, `%{?elfdeps}`), and the deb script does exactly
that. But the GUI stack — winit/glutin — opens its libraries with `dlopen()` at
runtime, so the automatic scanners cannot see them. Those are declared
explicitly:

- **deb**: package names (`libegl1`, `libgl1`, `libwayland-client0`, …) verified
  with `dpkg -S` against the actual sonames the binary loads.
- **rpm**: sonames (`libEGL.so.1()(64bit)`, …), which is exactly what the
  loader asks for and is distribution-independent.

If you add a dependency that is loaded the same way, add it in **both**
`scripts/package-deb.sh` (`GUI_DEPENDS`) and `packaging/rpm/easysearch.spec.in`.

## Flatpak

The manifest is `packaging/flatpak/io.github.easysearch.EasySearch.yml`.
The sandbox builds fully offline against vendored crates, so every crates.io
dependency is declared in `packaging/flatpak/cargo-sources.json` — generated
from `Cargo.lock` by `scripts/gen-cargo-sources.py` (the same job as upstream's
`flatpak-cargo-generator.py`, but reading the local cargo cache and verifying
each crate against the checksum in `Cargo.lock`, so it needs no network).
`make flatpak` refreshes it on every build; the generated file is committed so
`flatpak-builder` also works when invoked directly.

Things worth knowing:

- **Filesystem access.** The sandbox only sees what `finish-args` grants. It
  grants `--filesystem=home` because that is the app's default search root. To
  search the rest of the disk, add `--filesystem=host:ro`.
- **The HTTP/JSON API stays inside the sandbox.** Network access is not granted,
  so the daemon listens on loopback *within* the sandbox and other host
  processes cannot reach it. If that matters to you, add `--share=network`.
  If binding loopback ever fails, the app falls back to an in-process engine.
- **Tray icon.** `ksni` needs the StatusNotifierWatcher on the session bus;
  `--talk-name=org.kde.StatusNotifierWatcher` is granted. Tray icons in a
  sandbox remain the least reliable part of Flatpak; the app works without it.
- **Everything else is unrestricted** — the app reads and writes only under
  `$HOME` and `$XDG_CACHE_HOME`, and the index cache lives in the sandbox.

## What has been verified

Verified on this machine (KDE/Plasma, Wayland, Debian-family, no root):

- `make deb` → a valid `.deb`: `dpkg-deb --info`/`--contents` parse, the staging
  tree is `root/root` with mode 0755/0644, the payload extracts and contains the
  desktop entry, metainfo, SVG and four PNG icon sizes.
- `desktop-file-validate` passes; `appstreamcli validate` reports no errors.
- Dependency lists are derived from the running binary's actual libraries
  (`dpkg-shlibdeps` plus the `dlopen`ed sonames), then mapped to real packages
  with `dpkg -S`.
- `scripts/gen-cargo-sources.py` regenerates all 491 crates.io sources and every
  checksum matches `Cargo.lock`.
- The **RPM**, which this machine cannot build (no `rpmbuild`), was reproduced in
  an `ubuntu:24.04` container — the image `ubuntu-latest` is — with the cargo
  build staged, so `%prep`/`%install`/`%files`/brp ran for real:
  - `rpmbuild --nodeps -bb` completes and writes
    `easysearch-<version>-1.x86_64.rpm`.
  - The payload is the three binaries, the desktop entry, the metainfo file, the
    SVG icon and the two doc files — no unpackaged files.
  - Its `Requires` carry the sonames the GUI `dlopen()`s
    (`libEGL.so.1()(64bit)`, `libwayland-client.so.0()(64bit)`, …) next to the
    ELF-derived `libc`/`libgcc`/`libm` ones, so installing on Fedora/openSUSE
    pulls in the right runtime libraries.
  - `rpmbuild` passes `HOME`/`PATH` through to `%build`, so the rustup shim
    `cargo` is found (checked with a one-line probe spec).

`.github/workflows/packages.yml` runs the same build on every push to `master`,
so the RPM path is exercised on each merge rather than only by hand.

**Not verified here**, because the tools are not installed on this machine:

- `make flatpak` — no `flatpak-builder`. The manifest, the offline vendored
  source config and the generated sources are in place, but a build has not been
  run. Expect the usual first-run iteration (runtime version, SDK extension).

The rpm and flatpak scripts fail fast with install instructions when their tool is
missing, so `make packages` on a machine without them degrades cleanly.
