#!/usr/bin/env python3
"""Offline lockfile license inventory (Python 3.11+). Missing metadata stays UNKNOWN."""
import argparse
import json
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def cargo_metadata(vendor):
    result = {}
    for manifest in sorted(vendor.glob("*/Cargo.toml")):
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
        key = (package.get("name"), package.get("version"))
        license_id = package.get("license")
        if not isinstance(license_id, str):
            license_id = "UNKNOWN (license-file requires review)" if package.get("license-file") else "UNKNOWN"
        result[key] = license_id
    return result


def inventory(root, vendor):
    metadata = cargo_metadata(vendor)
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
