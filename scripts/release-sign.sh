#!/bin/sh
# Build and sign a release manifest for in-place self-updates.
#
#     scripts/release-sign.sh 0.15.0
#     scripts/release-sign.sh 0.15.0 --dir target/release --base-url https://…/releases/download
#
# Produces, in `dist/` (or --out):
#   manifest.json        the signed release description
#   manifest.json.sig    detached Ed25519 signature, hex (what clients verify)
#   <name>-<arch>        copies of the binaries, ready to upload
#
# See docs/updates.md for the format, the key setup, and how clients verify.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

usage() {
    cat <<'EOF'
Usage: scripts/release-sign.sh <version> [options]

  <version>              e.g. 0.15.0 (a leading "v" is stripped)

Options:
  --dir <DIR>            where the built binaries are (default: target/release)
  --out <DIR>            where to write the manifest (default: dist)
  --base-url <URL>       public base URL assets are served from
                         (default: $EASYSEARCH_RELEASE_BASE_URL, else a placeholder)
  --key <FILE>           signing key (default: $EASYSEARCH_RELEASE_KEY, else
                         ~/.config/easysearch/release-signing-key.pem)
  -h, --help             this help

The matching public key must be compiled into the client
(core/src/update.rs, RELEASE_PUBLIC_KEY_HEX); see docs/updates.md.
EOF
}

VERSION=""
SRC="target/release"
OUT="dist"
BASE_URL="${EASYSEARCH_RELEASE_BASE_URL:-https://example.invalid/easysearch}"
KEY="${EASYSEARCH_RELEASE_KEY:-$HOME/.config/easysearch/release-signing-key.pem}"

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) SRC="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --base-url) BASE_URL="$2"; shift 2 ;;
        --key) KEY="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "release-sign: unknown option $1" >&2; usage >&2; exit 2 ;;
        *) VERSION="$1"; shift ;;
    esac
done

if [ -z "$VERSION" ]; then
    usage >&2
    exit 2
fi
VERSION="${VERSION#v}"

# --- target triple (the assets are per-architecture) -------------------------
case "$(uname -m)" in
    x86_64 | amd64) TRIPLE="x86_64-unknown-linux-gnu"; ARCH="x86_64" ;;
    aarch64 | arm64) TRIPLE="aarch64-unknown-linux-gnu"; ARCH="aarch64" ;;
    *) TRIPLE="$(uname -m)-unknown-linux-gnu"; ARCH="$(uname -m)" ;;
esac

command -v jq >/dev/null 2>&1 || { echo "release-sign: jq is required" >&2; exit 1; }
[ -f "$KEY" ] || {
    echo "release-sign: signing key not found: $KEY" >&2
    echo "  create one (and embed its public half) — see docs/updates.md" >&2
    exit 1
}

# --- collect assets ----------------------------------------------------------
# The manifest's `name` is the installed file name; the uploaded file adds the
# architecture so one release host can serve several architectures.
BINARIES="easysearch easysearch-cli easysearch-gui easysearch-daemon"

mkdir -p "$OUT"
ASSETS="$OUT/.assets.jsonl"
: > "$ASSETS"

found=0
for name in $BINARIES; do
    src="$SRC/$name"
    [ -f "$src" ] || continue
    asset="$name-$ARCH"
    cp -f "$src" "$OUT/$asset"
    chmod 755 "$OUT/$asset"
    sha=$(sha256sum "$OUT/$asset" | cut -d' ' -f1)
    size=$(wc -c <"$OUT/$asset" | tr -d ' ')
    url="$BASE_URL/v$VERSION/$asset"
    jq -nc \
        --arg name "$name" --arg url "$url" --arg sha "$sha" \
        --argjson size "$size" --arg target "$TRIPLE" \
        '{name:$name,url:$url,sha256:$sha,size:$size,target:$target}' >>"$ASSETS"
    echo "  + $asset ($size bytes)"
    found=$((found + 1))
done

if [ "$found" -eq 0 ]; then
    echo "release-sign: no binaries found in $SRC" >&2
    exit 1
fi

NOTES_FILE="${EASYSEARCH_RELEASE_NOTES:-}"
if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then
    jq -n \
        --arg v "$VERSION" \
        --arg d "$(date -u +%Y-%m-%d)" \
        --rawfile notes "$NOTES_FILE" \
        --slurpfile assets "$ASSETS" \
        '{version:$v, released:$d, notes:$notes, assets:$assets}' >"$OUT/manifest.json"
else
    jq -n \
        --arg v "$VERSION" \
        --arg d "$(date -u +%Y-%m-%d)" \
        --arg notes "Release $VERSION" \
        --slurpfile assets "$ASSETS" \
        '{version:$v, released:$d, notes:$notes, assets:$assets}' >"$OUT/manifest.json"
fi
rm -f "$ASSETS"

# --- sign, then verify the signature before shipping it ----------------------
SIG_DER="$OUT/manifest.json.sig.der"
openssl pkeyutl -sign -rawin -inkey "$KEY" -in "$OUT/manifest.json" -out "$SIG_DER"
# The client and the manifest format use hex, not DER.
od -An -tx1 -v "$SIG_DER" | tr -d ' \n' >"$OUT/manifest.json.sig"
printf '\n' >>"$OUT/manifest.json.sig"

PUB_TMP="$OUT/.pub.pem"
openssl pkey -in "$KEY" -pubout -out "$PUB_TMP"
if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_TMP" \
    -in "$OUT/manifest.json" -sigfile "$SIG_DER" >/dev/null 2>&1; then
    echo "signature verified"
else
    rm -f "$PUB_TMP" "$SIG_DER"
    echo "release-sign: the signature did not verify — aborting" >&2
    exit 1
fi
rm -f "$PUB_TMP" "$SIG_DER"

echo
echo "wrote $OUT/manifest.json and $OUT/manifest.json.sig for v$VERSION ($found asset(s))"
echo "upload both (and the $ARCH binaries) to: $BASE_URL/v$VERSION/"
echo "clients read the manifest at: $BASE_URL/manifest.json"
