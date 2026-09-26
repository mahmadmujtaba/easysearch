#!/bin/sh
# Build an RPM from source with rpmbuild.
#
#     make rpm            # or: scripts/package-rpm.sh
#
# Output: $RPMBUILD_TOP/RPMS/*/easysearch-<version>-<release>.*.rpm
# (default $RPMBUILD_TOP is ~/rpmbuild; the packages are also copied to dist/.)
#
# Building needs rpmbuild, which ships separately from rpm itself:
#     Fedora        sudo dnf install rpm-build
#     Debian/Ubuntu sudo apt install rpm
#     openSUSE      sudo zypper install rpm-build
# The build fetches crates with cargo, so either allow network access or warm
# the cargo cache with `cargo fetch` beforehand.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

VERSION=$(cat VERSION)
NAME=easysearch
TARBALL_DIR=easysearch-$VERSION
TOP=${RPMBUILD_TOP:-$HOME/rpmbuild}
OUT_DIR=dist

err() {
    echo "error: $*" >&2
    exit 1
}

if ! command -v rpmbuild >/dev/null 2>&1; then
    cat >&2 <<'EOF'
error: rpmbuild is not installed.

  Fedora         sudo dnf install rpm-build
  Debian/Ubuntu  sudo apt install rpm
  openSUSE       sudo zypper install rpm-build

The spec lands in ~/rpmbuild/SPECS and the source tarball in ~/rpmbuild/SOURCES,
so you can also build it by hand in an environment that has rpmbuild.

EOF
    exit 1
fi

mkdir -p "$TOP/BUILD" "$TOP/BUILDROOT" "$TOP/RPMS" "$TOP/SOURCES" "$TOP/SPECS" "$TOP/SRPMS"

# Source tarball with the expected top-level directory name. Built from the
# working tree (not `git archive`) so uncommitted work is packaged too.
PARENT=$(dirname "$ROOT")
BASE=$(basename "$ROOT")
tar -czf "$TOP/SOURCES/$TARBALL_DIR.tar.gz" \
    --exclude='.git' \
    --exclude='.shots' \
    --exclude='build-flatpak' \
    --exclude='dist' \
    --exclude='target' \
    --transform "s|^$BASE|$TARBALL_DIR|" \
    -C "$PARENT" "$BASE"
echo "source tarball: $TOP/SOURCES/$TARBALL_DIR.tar.gz"

sed -e "s|@VERSION@|$VERSION|g" \
    packaging/rpm/$NAME.spec.in >"$TOP/SPECS/$NAME.spec"

# --nodeps: the spec's BuildRequires (cargo, rust >= 1.88, desktop-file-utils)
# are not rpm packages on Debian/Ubuntu, where Rust comes from rustup, so
# rpmbuild would refuse to start the build. The tools are provided by the
# environment, exactly as they are on Fedora after `dnf builddep`.
rpmbuild -ba --nodeps \
    --define "_topdir $TOP" \
    "$TOP/SPECS/$NAME.spec" || err "rpmbuild failed (see the output above)"

mkdir -p "$OUT_DIR"
for rpm in "$TOP"/RPMS/*/"$NAME"-*.rpm; do
    [ -f "$rpm" ] || continue
    cp -f "$rpm" "$OUT_DIR/"
done

echo
ls -lh "$OUT_DIR"/"$NAME"-*.rpm 2>/dev/null || true
