#!/bin/sh
# Stamp the AppStream metainfo <release> version and date from VERSION.
#
# The tag build runs this before packaging, so the .deb/.rpm never carry a stale
# <release> entry even if the committed file lags behind. Safe to run by hand:
# it only rewrites the version and date of the first <release> element.
#
#     scripts/sync-metainfo-release.sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

VERSION=$(cat VERSION)
DATE=${DATE:-$(date -u +%Y-%m-%d)}
FILE=packaging/common/io.github.easysearch.EasySearch.metainfo.xml

python3 - "$FILE" "$VERSION" "$DATE" <<'PY'
import re
import sys

path, version, date = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path, encoding="utf-8").read()

# The first <release ...> element only; any older entries are left alone.
pattern = re.compile(r'(<release version=")[^"]*(" date=")[^"]*(")')
new, count = pattern.subn(
    lambda m: f"{m.group(1)}{version}{m.group(2)}{date}{m.group(3)}",
    text,
    count=1,
)
if count == 0:
    sys.exit(f"no <release ...> entry found in {path}")

with open(path, "w", encoding="utf-8") as fh:
    fh.write(new)
print(f"metainfo release -> {version} ({date})")
PY
