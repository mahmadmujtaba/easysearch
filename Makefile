# EasySearch — Makefile
#
#   make            build + test + release (easysearch-cli)
#   make build      dev build (fast)
#   make dev        alias for build
#   make release    production build (LTO + strip)   [alias: prod]
#   make test       run all tests
#   make check      type-check without producing binaries
#   make clippy     lint
#   make fmt        format sources
#   make run        build + run EasySearch (one binary: window + engine child)
#   make run-dev    same, dev profile
#   make run-gui    run just the GUI binary (dev tool)
#   make daemon     run just the engine (JSON frames on stdin/stdout)
#   make dist       copy the shareable single binary to dist/
#   make install    release binaries to ~/.local/bin + desktop entry and icon
#                   to ~/.local/share (so the launcher and the window
#                   decoration can find the logo)
#   make uninstall  remove everything `make install` put there
#   make deb        build a Debian package (.deb)
#   make rpm        build an RPM (.rpm)     [needs rpmbuild]
#   make flatpak    build a Flatpak bundle  [needs flatpak-builder]
#   make packages   build all three
#   make validate-packaging  check the desktop entry + AppStream metadata
#   make clean      remove build artifacts
#   make help       show this help

# rustup installs cargo into ~/.cargo/bin, which may not be on PATH in shells
# started before the toolchain was installed.
CARGO_BIN := $(shell command -v cargo 2>/dev/null || echo "$(HOME)/.cargo/bin/cargo")

BIN_DIR    := target/release
INSTALLDIR := $(HOME)/.local/bin
DATADIR    := $(HOME)/.local/share

# Reverse-DNS application id, shared by the desktop entry, the AppStream
# metainfo file and the Flatpak manifest. Change it in one place here and in
# packaging/common/, packaging/icons/ and packaging/flatpak/.
APP_ID     := io.github.easysearch.EasySearch

.PHONY: all build dev release prod test test-release check clippy fmt \
        run run-dev run-gui daemon daemon-dev dist install uninstall clean help \
        deb rpm flatpak packages cargo-sources validate-packaging

## Default: dev build + tests + production build.
all: build test release

## Dev build (debug, fast to compile).
build dev:
	$(CARGO_BIN) build --workspace

## Production build (LTO + strip, small & fast binaries).
release prod:
	$(CARGO_BIN) build --release --workspace

## Run all tests (dev profile).
test:
	$(CARGO_BIN) test --workspace

## Run all tests under the release profile.
test-release:
	$(CARGO_BIN) test --workspace --release

## Type-check only (no binaries produced).
check:
	$(CARGO_BIN) check --workspace

## Lint with clippy.
clippy:
	$(CARGO_BIN) clippy --workspace --all-targets

## Format all sources (rustfmt).
fmt:
	$(CARGO_BIN) fmt --all

## Build and launch EasySearch. This one binary is both the app (window +
## tray) and the engine: it spawns the engine as its child and attaches.
run: release
	./$(BIN_DIR)/easysearch

## Same as `run`, dev profile.
run-dev: build
	./target/debug/easysearch

## Build and launch just the GUI binary (a development convenience).
run-gui: release
	./$(BIN_DIR)/easysearch-gui

## Copy the single shareable binary to dist/.
dist: release
	mkdir -p dist
	install -m 0755 $(BIN_DIR)/easysearch dist/easysearch
	@echo "Shareable binary: dist/easysearch"
	@ls -lh dist/easysearch

## Build a Debian package (needs the release binaries, no root required).
deb: release
	./scripts/package-deb.sh

## Build an RPM from source (needs rpmbuild).
rpm:
	./scripts/package-rpm.sh

## Build a Flatpak bundle (needs flatpak-builder + the freedesktop SDK).
flatpak:
	./scripts/package-flatpak.sh

## Build every package format this machine has the tools for.
packages: deb
	@command -v rpmbuild >/dev/null 2>&1 && $(MAKE) --no-print-directory rpm || \
		echo "skipping rpm: rpmbuild is not installed"
	@command -v flatpak-builder >/dev/null 2>&1 && $(MAKE) --no-print-directory flatpak || \
		echo "skipping flatpak: flatpak-builder is not installed"

## Regenerate the Flatpak crate sources from Cargo.lock.
cargo-sources:
	python3 scripts/gen-cargo-sources.py

## Check the desktop entry and the AppStream metainfo file.
##
## appstreamcli is run with --no-net, so remote screenshot URLs are not fetched.
## Real errors (lines starting with "E:") fail the build; warnings and pedantic
## notes are reported and ignored.
validate-packaging:
	desktop-file-validate packaging/common/$(APP_ID).desktop
	@out=$$(appstreamcli validate --no-net packaging/common/$(APP_ID).metainfo.xml 2>&1 || true); \
	 echo "$$out"; \
	 if echo "$$out" | grep -q '^E:'; then \
	   echo "appstreamcli: metadata has errors" >&2; exit 1; \
	 fi; \
	 echo "appstreamcli: no errors"

## Run just the engine (normally the app's child; this drives it by hand).
## It speaks JSON frames on stdin/stdout — see docs/api.md.
daemon: release
	./$(BIN_DIR)/easysearch-daemon

## Run just the engine (dev profile).
daemon-dev: build
	./target/debug/easysearch-daemon

## Install release binaries into ~/.local/bin, plus the desktop entry, the icon
## and the AppStream metadata into ~/.local/share.
##
## The desktop entry and icon are what put the app in the launcher — and what let
## the compositor find the logo for the window decoration: on Wayland a window
## has no icon of its own, the decoration resolves the app id against the
## installed desktop entry.
install: release
	mkdir -p $(INSTALLDIR)
	install -m 0755 $(BIN_DIR)/easysearch $(INSTALLDIR)/easysearch
	install -m 0755 $(BIN_DIR)/easysearch-cli $(INSTALLDIR)/easysearch-cli
	install -m 0755 $(BIN_DIR)/easysearch-gui $(INSTALLDIR)/easysearch-gui
	install -m 0755 $(BIN_DIR)/easysearch-daemon $(INSTALLDIR)/easysearch-daemon
	mkdir -p $(DATADIR)/applications $(DATADIR)/metainfo \
	         $(DATADIR)/icons/hicolor/scalable/apps
	install -m 0644 packaging/common/$(APP_ID).desktop $(DATADIR)/applications/
	install -m 0644 packaging/common/$(APP_ID).metainfo.xml $(DATADIR)/metainfo/
	install -m 0644 packaging/icons/hicolor/scalable/apps/$(APP_ID).svg \
	         $(DATADIR)/icons/hicolor/scalable/apps/
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi
	@# Refresh the icon cache, or a previously installed (opaque) icon can win.
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -q -t $(DATADIR)/icons/hicolor 2>/dev/null || true; \
	fi
	@echo "Installed into $(INSTALLDIR): easysearch, easysearch-cli, easysearch-gui, easysearch-daemon"
	@echo "Desktop integration into $(DATADIR): applications, metainfo, icons/hicolor"

uninstall:
	rm -f $(INSTALLDIR)/easysearch $(INSTALLDIR)/easysearch-cli $(INSTALLDIR)/easysearch-gui $(INSTALLDIR)/easysearch-daemon
	rm -f $(DATADIR)/applications/$(APP_ID).desktop
	rm -f $(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi

clean:
	$(CARGO_BIN) clean

help:
	@sed -n 's/^## //p' $(MAKEFILE_LIST)
