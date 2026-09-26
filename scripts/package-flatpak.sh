#!/bin/sh
# Build a Flatpak bundle.
#
#     make flatpak                  # or: scripts/package-flatpak.sh
#     INSTALL=1 make flatpak        # also install it for the current user
#
# Output: dist/io.github.easysearch.EasySearch.flatpak
#
# Prerequisites:
#     flatpak-builder (flatpak-builder package) and, once,
#     flatpak install flathub org.freedesktop.Platform//24.08 \
#                             org.freedesktop.Sdk//24.08 \
#                             org.freedesktop.Sdk.Extension.rust-stable//24.08
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

APP_ID=io.github.easysearch.EasySearch
MANIFEST=packaging/flatpak/$APP_ID.yml
BUILD_DIR=build-flatpak
REPO_DIR=$PWD/dist/flatpak-repo
BUNDLE=dist/$APP_ID.flatpak
RUNTIME=org.freedesktop.Platform//24.08
SDK=org.freedesktop.Sdk//24.08
RUST_SDK=org.freedesktop.Sdk.Extension.rust-stable//24.08

if ! command -v flatpak-builder >/dev/null 2>&1; then
    cat >&2 <<'EOF'
error: flatpak-builder is not installed.

  Fedora         sudo dnf install flatpak-builder
  Debian/Ubuntu  sudo apt install flatpak-builder
  openSUSE       sudo zypper install flatpak-builder
  Arch           sudo pacman -S flatpak-builder

Then fetch the runtime, SDK and Rust extension once:

  flatpak install flathub org.freedesktop.Platform//24.08 \
                          org.freedesktop.Sdk//24.08 \
                          org.freedesktop.Sdk.Extension.rust-stable//24.08

EOF
    exit 1
fi

missing=
for ref in "$RUNTIME" "$SDK" "$RUST_SDK"; do
    flatpak info "$ref" >/dev/null 2>&1 || missing="$missing $ref"
done
if [ -n "$missing" ]; then
    echo "error: these Flatpak runtimes are missing:$missing" >&2
    echo "       flatpak install flathub$missing" >&2
    exit 1
fi

# Every crates.io dependency has to be declared as a source (the sandbox builds
# offline). Kept in sync from Cargo.lock on every run.
python3 scripts/gen-cargo-sources.py

mkdir -p dist
echo "==> flatpak-builder"
flatpak-builder \
    --user \
    --force-clean \
    --repo="$REPO_DIR" \
    "$BUILD_DIR" "$MANIFEST"

echo "==> bundling"
flatpak build-bundle "$REPO_DIR" "$BUNDLE" "$APP_ID" --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo

if [ "${INSTALL:-0}" = "1" ]; then
    echo "==> installing for the current user"
    flatpak --user install -y "$REPO_DIR" "$APP_ID"
fi

echo
ls -lh "$BUNDLE"
echo "Install with: flatpak install --user $BUNDLE"
