#!/usr/bin/env python3
"""Offline lockfile license inventory (Python 3.11+). Missing metadata stays UNKNOWN."""
import argparse
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def _license_of(manifest):
    try:
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
    except (OSError, tomllib.TOMLDecodeError):
        return None, None
    key = (package.get("name"), package.get("version"))
    license_id = package.get("license")
    if not isinstance(license_id, str):
        license_id = "UNKNOWN (license-file requires review)" if package.get("license-file") else "UNKNOWN"
    return key, license_id


def cargo_metadata(vendor):
    result = {}
    for manifest in sorted(vendor.glob("*/Cargo.toml")):
        key, license_id = _license_of(manifest)
        if key:
            result[key] = license_id
    return result


def cargo_registry_cache():
    """Offline fallback: the already-extracted crate sources in the local Cargo registry cache
    (populated by a build or `cargo fetch`). No downloads; missing crates simply stay absent."""
    result = {}
    cargo_home = Path(os.environ.get("CARGO_HOME") or (Path.home() / ".cargo"))
    for manifest in (cargo_home / "registry" / "src").glob("*/*/Cargo.toml"):
        key, license_id = _license_of(manifest)
        if key and key[0] and key not in result:
            result[key] = license_id
    return result


def inventory(root, vendor):
    # The registry cache fills Cargo licenses when no --vendor mirror is given; an explicit mirror wins.
    metadata = {**cargo_registry_cache(), **cargo_metadata(vendor)}
    rows = set()
    for lock in (root / "Cargo.lock", root / "src-tauri/Cargo.lock"):
        for package in tomllib.loads(lock.read_text(encoding="utf-8"))["package"]:
            if "source" not in package:  # Workspace packages are covered by LICENSE.
                continue
            name, version = package["name"], package["version"]
            # Only registry metadata can be matched to a vendored name/version.
            license_id = metadata.get((name, version), "UNKNOWN") if package["source"].startswith("registry+") else "UNKNOWN (non-registry source)"
            rows.add(("Cargo", name, version, license_id))
    lock = json.loads((root / "package-lock.json").read_text(encoding="utf-8"))
    for location, package in lock["packages"].items():
        if not location:
            continue
        name = package.get("name") or location.rsplit("node_modules/", 1)[-1]
        version = package.get("version", "UNKNOWN")
        license_id = package.get("license")
        if not license_id:
            manifest = root / location / "package.json"
            if manifest.is_file():
                local = json.loads(manifest.read_text(encoding="utf-8"))
                if local.get("name") == name and local.get("version") == version:
                    license_id = local.get("license")
        if isinstance(license_id, dict):
            license_id = license_id.get("type")
        rows.add(("npm", name, version, license_id if isinstance(license_id, str) else "UNKNOWN"))
    return sorted(rows)


def copyleft(license_id):
    return bool(re.search(r"\b(?:A|L)?GPL(?:-|\b)", license_id, re.I))


def render(rows):
    unknown = [row for row in rows if row[3].startswith("UNKNOWN")]
    flagged = [row for row in rows if copyleft(row[3])]
    lines = ["# Third-party licenses", "",
        "Generated offline by `python3 scripts/gen-third-party.py` (Python 3.11+).",
        "Scope: both Cargo lockfiles and all npm lockfile packages, including development and",
        "platform-specific dependencies. This is an inventory, not a list of files shipped in a binary.",
        "Rust metadata: `vendor/*/Cargo.toml`; npm: lockfile, falling back to matching installed package metadata.",
        "Use `--vendor DIR` for an offline metadata mirror. No downloads or dependency resolution are performed.",
        f"Packages: **{len(rows)}**. Unknown: **{len(unknown)}**.", "",
        "UNKNOWN is a release-review blocker: provide matching source metadata and regenerate before distribution.",
        "License expressions are upstream declarations; alternatives (OR) and exceptions need individual review.",
        "System libraries, AppImage tooling/runtime and external archive helpers are outside these lockfiles.",
        "Keep required upstream notices and license texts when distributing binaries; this table does not replace them.", "",
        "## GPL / AGPL / LGPL review", ""]
    if flagged:
        lines += [f"- {eco} `{name}` {version}: `{lic}`" for eco, name, version, lic in flagged]
    else:
        lines += ["No GPL/AGPL/LGPL declaration found in available metadata. UNKNOWN entries prevent a compatibility conclusion."]
    lines += ["", "## Shipped helper programs (not in the lockfiles)", "",
        "- **7-Zip 26.06** (`7zzs`, installed as `usr/bin/7zz`) in the AppImage and tarball, one static build per architecture "
        "(x86_64, aarch64) from https://github.com/ip7z/7zip/releases/tag/26.06 (see https://www.7-zip.org/). "
        "Dualfile starts it as a separate program to read RAR archives; it is not linked into Dualfile.",
        "  Licenses: GNU LGPL (most code), GNU LGPL with the unRAR license restriction (RAR decoder), and further notices for some files; "
        "the full text is shipped as `usr/share/doc/7zip/License.txt`.",
        "  The unRAR restriction: the unRAR sources cannot be used to re-create the RAR compression algorithm, which is proprietary; "
        "the RAR code may not be used to develop a RAR (WinRAR) compatible archiver.",
        "  Source code (LGPL offer): https://github.com/ip7z/7zip/releases/download/26.06/7z2606-src.tar.xz "
        "(SHA-256 `a4e374c8f864f51df48c2cfd0e1413ec3b2ea030deae852d6a655402ebb61ac4`). Details: `docs/third-party-7zip.md`."]
    lines += ["", "## Inventory", "", "| Ecosystem | Package | Version | License |", "|---|---|---|---|"]
    def cell(value):
        return value.replace("|", "\\|").replace("\n", " ").replace("\r", " ")
    lines += ["| " + " | ".join(cell(value) for value in row) + " |" for row in rows]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vendor", type=Path, default=ROOT / "vendor")
    parser.add_argument("--output", type=Path, default=ROOT / "THIRD-PARTY-LICENSES.md")
    parser.add_argument("--strict", action="store_true", help="exit nonzero if metadata is missing")
    args = parser.parse_args()
    rows = inventory(ROOT, args.vendor)
    args.output.write_text(render(rows), encoding="utf-8")
    unknown = sum(row[3].startswith("UNKNOWN") for row in rows)
    flagged = sum(copyleft(row[3]) for row in rows)
    print(f"Packages: {len(rows)}; UNKNOWN: {unknown}; GPL/AGPL/LGPL expressions: {flagged}")
    if args.strict and unknown:
        parser.exit(1, "Incomplete license metadata; distribution review required.\n")


if __name__ == "__main__":
    main()
