#!/usr/bin/env python3
"""Create an offline Tauri updater manifest for an already signed final AppImage."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
from urllib.parse import quote, urlsplit


PLATFORMS = ("linux-x86_64", "linux-aarch64")


def make_manifest(version, release_url, signature_files, notes="", pub_date=None):
    """signature_files maps a Tauri platform key to its <AppImage>.AppImage.sig path."""
    # Tauri expects the version without the Git tag's v prefix.
    version = version.removeprefix("v")
    number = r"(?:0|[1-9][0-9]*)"
    pre = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    if not re.fullmatch(rf"{number}\.{number}\.{number}(?:-{pre}(?:\.{pre})*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", version):
        raise ValueError("version must be a semantic version")
    url = urlsplit(release_url)
    if (url.scheme != "https" or not url.hostname or url.username or url.password
            or url.query or url.fragment or any(c.isspace() for c in release_url)):
        raise ValueError("release URL must be an HTTPS download directory without credentials, query or fragment")
    if not signature_files:
        raise ValueError("at least one signature is required")
    platforms = {}
    for platform, signature_file in signature_files.items():
        if platform not in PLATFORMS:
            raise ValueError(f"platform must be one of {', '.join(PLATFORMS)}")
        signature_file = Path(signature_file)
        if not signature_file.name.endswith(".AppImage.sig"):
            raise ValueError("signature must belong to the final .AppImage")
        signature = signature_file.read_text(encoding="utf-8").strip()
        if not signature:
            raise ValueError("signature file is empty")
        platforms[platform] = {"signature": signature,
            "url": release_url.rstrip("/") + "/" + quote(signature_file.name[:-4], safe="")}
    if pub_date is None:
        pub_date = datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})", pub_date):
        raise ValueError("pub_date must be RFC 3339 with a timezone")
    datetime.fromisoformat(pub_date.replace("Z", "+00:00"))
    return {"version": version, "notes": notes, "pub_date": pub_date, "platforms": platforms}


def parse_signature(value):
    platform, separator, path = value.partition("=")
    if not separator or not platform or not path:
        raise argparse.ArgumentTypeError("expected PLATFORM=PATH")
    return platform, Path(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--release-url", required=True, help="HTTPS release asset download directory")
    parser.add_argument("--signature", required=True, action="append", type=parse_signature,
                        metavar="PLATFORM=PATH", help="repeatable, e.g. linux-x86_64=Dualfile-1.0.0-x86_64.AppImage.sig")
    parser.add_argument("--notes-file", type=Path)
    parser.add_argument("--pub-date")
    parser.add_argument("--output", type=Path, default=Path("latest.json"))
    args = parser.parse_args()
    try:
        signatures = dict(args.signature)
        if len(signatures) != len(args.signature):
            raise ValueError("duplicate platform in --signature")
        result = make_manifest(args.version, args.release_url, signatures,
                               args.notes_file.read_text(encoding="utf-8") if args.notes_file else "",
                               args.pub_date)
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError) as error:
        parser.exit(2, f"error: {error}\n")


if __name__ == "__main__":
    main()
