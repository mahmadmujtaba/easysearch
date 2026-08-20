# Everything for Linux — Makefile
#
#   make            build + test + release (everything)
#   make build      dev build (fast)
#   make dev        alias for build
#   make release    production build (LTO + strip)   [alias: prod]
#   make test       run all tests
#   make check      type-check without producing binaries
#   make clippy     lint
#   make fmt        format sources
#   make run        run the production GUI
#   make run-dev    run the dev GUI
#   make install    copy release binaries to ~/.local/bin
#   make clean      remove build artifacts
#   make help       show this help

# rustup installs cargo into ~/.cargo/bin, which may not be on PATH in shells
# started before the toolchain was installed.
CARGO_BIN := $(shell command -v cargo 2>/dev/null || echo "$(HOME)/.cargo/bin/cargo")

BIN_DIR    := target/release
INSTALLDIR := $(HOME)/.local/bin

.PHONY: all build dev release prod test test-release check clippy fmt \
        run run-dev install uninstall clean help

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

## Build and launch the production GUI.
run: release
	./$(BIN_DIR)/everything-gui

## Build and launch the dev GUI.
run-dev: build
	./target/debug/everything-gui

## Install release binaries into ~/.local/bin.
install: release
	mkdir -p $(INSTALLDIR)
	install -m 0755 $(BIN_DIR)/everything $(INSTALLDIR)/everything
	install -m 0755 $(BIN_DIR)/everything-gui $(INSTALLDIR)/everything-gui
	@echo "Installed: $(INSTALLDIR)/everything, $(INSTALLDIR)/everything-gui"

uninstall:
	rm -f $(INSTALLDIR)/everything $(INSTALLDIR)/everything-gui

clean:
	$(CARGO_BIN) clean

help:
	@sed -n 's/^## //p' $(MAKEFILE_LIST)
