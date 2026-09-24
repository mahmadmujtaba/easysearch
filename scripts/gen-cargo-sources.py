#!/usr/bin/env python3
"""Generate `packaging/flatpak/cargo-sources.json` from `Cargo.lock`.

flatpak-builder builds with no network access, so every crates.io dependency has
to be declared as a source. This does what upstream `flatpak-cargo-generator.py`
does, but without needing to download anything: it reads the `.crate` files that
`cargo fetch` already put in the local cargo cache and verifies each one against
the checksum recorded in `Cargo.lock`.

Usage:
    python3 scripts/gen-cargo-sources.py

Requires Python 3.11+ (for `tomllib`). Run `cargo fetch` first if the crate
cache is cold — every dependency is looked up locally on purpose, so that the
generated checksums are the real ones and the Flatpak build stays reproducible.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCKFILE = ROOT / "Cargo.lock"
OUTFILE = ROOT / "packaging" / "flatpak" / "cargo-sources.json"

CRATES_IO_INDEX = "registry+https://github.com/rust-lang/crates.io-index"
CRATE_URL = "https://static.crates.io/crates/{name}/{name}-{version}.crate"


def cargo_cache() -> Path:
    home = os.environ.get("CARGO_HOME") or (Path.home() / ".cargo")
    return Path(home) / "registry" / "cache"


def find_crate(cache: Path, name: str, version: str) -> Path | None:
    """Locate `<name>-<version>.crate` in any registry index cache directory."""
    for index_dir in sorted(cache.glob("*")):
        candidate = index_dir / f"{name}-{version}.crate"
        if candidate.is_file():
            return candidate
    return None


def main() -> int:
    if not LOCKFILE.is_file():
        print(f"error: {LOCKFILE} not found", file=sys.stderr)
        return 1

    cache = cargo_cache()
    lock = tomllib.loads(LOCKFILE.read_text(encoding="utf-8"))

    sources: list[dict[str, str]] = []
    missing: list[str] = []

    for package in lock.get("package", []):
        # Path patches (vendor/arrayref) and the workspace crates themselves have
        # no `source`: they come from the `dir` source in the manifest instead.
        if package.get("source") != CRATES_IO_INDEX:
            continue

        name, version = package["name"], package["version"]
        crate = find_crate(cache, name, version)
        if crate is None:
            missing.append(f"{name}-{version}")
            continue

        digest = hashlib.sha256(crate.read_bytes()).hexdigest()
        recorded = package.get("checksum")
        if recorded and recorded != digest:
            print(
                f"error: checksum mismatch for {name}-{version}\n"
                f"       Cargo.lock: {recorded}\n"
                f"       {crate}: {digest}",
                file=sys.stderr,
            )
            return 1

        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": CRATE_URL.format(name=name, version=version),
                "sha256": digest,
                "dest": "cargo/vendor",
            }
        )

    if missing:
        shown = ", ".join(missing[:10])
        more = f" (and {len(missing) - 10} more)" if len(missing) > 10 else ""
        print(
            f"error: {len(missing)} crates are not in the local cargo cache: {shown}{more}\n"
            f"       Run `cargo fetch` and try again.",
            file=sys.stderr,
        )
        return 1

    OUTFILE.write_text(json.dumps(sources, indent=4) + "\n", encoding="utf-8")
    print(f"wrote {OUTFILE.relative_to(ROOT)} ({len(sources)} crates)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
