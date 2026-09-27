#!/bin/sh
# Build a Debian package from the release binaries.
#
#     make deb            # or: scripts/package-deb.sh
#
# Needs no root: dpkg-deb fixes up ownership with --root-owner-group.
# Output: dist/easysearch_<version>_<arch>.deb
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

PKG=easysearch
APP_ID=io.github.easysearch.EasySearch
VERSION=$(cat VERSION)
ARCH=${ARCH:-$(dpkg --print-architecture)}
BIN_DIR=${BIN_DIR:-target/release}
OUT_DIR=${OUT_DIR:-dist}
MAINTAINER=${MAINTAINER:-"EasySearch maintainers <ahmad.mujtaba11@gmail.com>"}

# winit/glutin load these with dlopen() at runtime, so dpkg-shlibdeps cannot see
# them. These are the Debian/Ubuntu packages that ship the exact sonames the
# binary loads (`dpkg -S` on the installed libraries reports the same names).
GUI_DEPENDS='libegl1, libgl1, libwayland-client0, libwayland-egl1, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxrender1'

err() {
    echo "error: $*" >&2
    exit 1
}

for b in easysearch easysearch-cli easysearch-daemon; do
    [ -x "$BIN_DIR/$b" ] || err "$BIN_DIR/$b is missing — run 'make release' first"
done

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT INT TERM

PKGROOT=$STAGE/${PKG}_${VERSION}_${ARCH}
install -d \
    "$PKGROOT/DEBIAN" \
    "$PKGROOT/usr/bin" \
    "$PKGROOT/usr/share/applications" \
    "$PKGROOT/usr/share/metainfo" \
    "$PKGROOT/usr/share/icons/hicolor/scalable/apps" \
    "$PKGROOT/usr/share/doc/$PKG"

# easysearch already is the GUI *and* the daemon (it re-executes itself
# with --daemon), so the separate easysearch-gui binary — a development
# convenience that would add another ~17 MB — is deliberately not shipped.
install -m 0755 "$BIN_DIR/easysearch" "$PKGROOT/usr/bin/"
install -m 0755 "$BIN_DIR/easysearch-cli" "$PKGROOT/usr/bin/"
install -m 0755 "$BIN_DIR/easysearch-daemon" "$PKGROOT/usr/bin/"

install -m 0644 "packaging/common/$APP_ID.desktop" \
    "$PKGROOT/usr/share/applications/$APP_ID.desktop"
install -m 0644 "packaging/common/$APP_ID.metainfo.xml" \
    "$PKGROOT/usr/share/metainfo/$APP_ID.metainfo.xml"
install -m 0644 "packaging/icons/hicolor/scalable/apps/$APP_ID.svg" \
    "$PKGROOT/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -m 0644 LICENSE "$PKGROOT/usr/share/doc/$PKG/copyright"
install -m 0644 README.md "$PKGROOT/usr/share/doc/$PKG/README.md"

# Rasterised icons: the SVG alone is enough on modern desktops, but PNGs keep
# older icon caches happy. Best effort — a converter may not be installed.
SVG=packaging/icons/hicolor/scalable/apps/$APP_ID.svg
render_icon() {
    size=$1
    dest="$PKGROOT/usr/share/icons/hicolor/${size}x${size}/apps"
    install -d "$dest"
    if command -v magick >/dev/null 2>&1; then
        # `PNG32:` forces an RGBA PNG, so the icon keeps its transparent
        # background whatever the SVG delegate's own defaults are.
        magick -background none "$SVG" -resize "${size}x${size}" "PNG32:$dest/$APP_ID.png"
    else
        rsvg-convert -w "$size" -h "$size" "$SVG" -o "$dest/$APP_ID.png"
    fi
    chmod 0644 "$dest/$APP_ID.png"
}
if command -v magick >/dev/null 2>&1 || command -v rsvg-convert >/dev/null 2>&1; then
    for size in 16 24 32 48 64 128 256 512; do
        render_icon "$size"
    done
else
    echo "note: install ImageMagick or librsvg2-bin for PNG icons; shipping SVG only" >&2
fi

# --- dependencies ---------------------------------------------------------
# dpkg-shlibdeps only reports libraries the binary links against; it cannot see
# dlopen()ed ones, so those are listed explicitly above.
SHLIB_DEPS=
if command -v dpkg-shlibdeps >/dev/null 2>&1; then
    install -d "$STAGE/shl/debian"
    cat >"$STAGE/shl/debian/control" <<EOF
Source: $PKG
Section: utils
Priority: optional
Maintainer: $MAINTAINER
Standards-Version: 4.6.2

Package: $PKG
Architecture: $ARCH
Description: dependency probe
EOF
    SHLIB_DEPS=$(cd "$STAGE/shl" &&
        dpkg-shlibdeps -O -e "$ROOT/$BIN_DIR/easysearch" 2>/dev/null |
        sed -n 's/^shlibs:Depends=//p') || SHLIB_DEPS=
fi

DEPENDS=$GUI_DEPENDS
if [ -n "$SHLIB_DEPS" ]; then
    DEPENDS="$SHLIB_DEPS, $GUI_DEPENDS"
fi

SIZE=$(du -sk "$PKGROOT" | cut -f1)

sed \
    -e "s|@PKG@|$PKG|g" \
    -e "s|@VERSION@|$VERSION|g" \
    -e "s|@ARCH@|$ARCH|g" \
    -e "s|@MAINTAINER@|$MAINTAINER|g" \
    -e "s|@DEPENDS@|$DEPENDS|g" \
    -e "s|@SIZE@|$SIZE|g" \
    packaging/debian/control.in >"$PKGROOT/DEBIAN/control"

mkdir -p "$OUT_DIR"
DEB="$OUT_DIR/${PKG}_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "$PKGROOT" "$DEB" >/dev/null

# --- checks ---------------------------------------------------------------
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$PKGROOT/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null 2>&1; then
    appstreamcli validate --no-net \
        "$PKGROOT/usr/share/metainfo/$APP_ID.metainfo.xml" ||
        echo "note: appstreamcli reported the issues above" >&2
fi

echo
dpkg-deb --info "$DEB"
echo
echo "Package contents:"
dpkg-deb --contents "$DEB"
