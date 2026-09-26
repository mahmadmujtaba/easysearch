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
#   make run        build + run EasySearch (one binary: GUI + daemon)
#   make run-dev    same, dev profile
#   make run-gui    run just the GUI binary (dev tool)
#   make daemon     run just the search daemon (HTTP/JSON API)
#   make dist       copy the shareable single binary to dist/
#   make install    copy release binaries to ~/.local/bin
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

## Build and launch EasySearch. This one binary is both the GUI and
## the search daemon: it starts a daemon (re-executing itself) and attaches.
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

## Build and launch the search daemon (GUI and CLI then attach to it).
daemon: release
	./$(BIN_DIR)/easysearch-daemon --addr 127.0.0.1:5858

## Build and launch the daemon (dev profile).
daemon-dev: build
	./target/debug/easysearch-daemon --addr 127.0.0.1:5858

## Install release binaries into ~/.local/bin.
install: release
	mkdir -p $(INSTALLDIR)
	install -m 0755 $(BIN_DIR)/easysearch $(INSTALLDIR)/easysearch
	install -m 0755 $(BIN_DIR)/easysearch-cli $(INSTALLDIR)/easysearch-cli
	install -m 0755 $(BIN_DIR)/easysearch-gui $(INSTALLDIR)/easysearch-gui
	install -m 0755 $(BIN_DIR)/easysearch-daemon $(INSTALLDIR)/easysearch-daemon
	@echo "Installed into $(INSTALLDIR): easysearch, easysearch-cli, easysearch-gui, easysearch-daemon"

uninstall:
	rm -f $(INSTALLDIR)/easysearch $(INSTALLDIR)/easysearch-cli $(INSTALLDIR)/easysearch-gui $(INSTALLDIR)/easysearch-daemon

clean:
	$(CARGO_BIN) clean

help:
	@sed -n 's/^## //p' $(MAKEFILE_LIST)
