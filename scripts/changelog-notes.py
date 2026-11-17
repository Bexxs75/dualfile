#!/usr/bin/env python3
"""Print the CHANGELOG section of one version as release notes (### headings become ##)."""
import argparse
from pathlib import Path
import re
import sys


def section(text, version):
    version = version.removeprefix("v")
    head = re.compile(r"^## \[" + re.escape(version) + r"\][^\n]*\n", re.M)
    match = head.search(text)
    if not match:
        raise ValueError(f"CHANGELOG has no section for {version}")
    rest = text[match.end():]
    end = re.search(r"^## \[", rest, re.M)
    body = (rest[:end.start()] if end else rest).strip()
    if not body:
        raise ValueError(f"CHANGELOG section for {version} is empty")
    return re.sub(r"^###(?=\s)", "##", body, flags=re.M) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--changelog", type=Path, default=Path("CHANGELOG.md"))
    args = parser.parse_args()
    try:
        sys.stdout.write(section(args.changelog.read_text(encoding="utf-8"), args.version))
    except (OSError, ValueError) as error:
        parser.exit(2, f"error: {error}\n")


if __name__ == "__main__":
    main()
