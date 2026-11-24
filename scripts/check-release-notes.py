#!/usr/bin/env python3
"""Check the release texts of one version (shared by release.yml and release-preflight.sh).

Checks src/release-notes.ts (entry by key, date, non-empty de and en lines, no placeholders;
layout-tolerant: multi-line and one-line blocks) and the CHANGELOG section (present, not empty,
no placeholders). Prints one problem per line; exit 1 if there is any.
"""
import argparse
import importlib.util
from pathlib import Path
import re
import sys

# Unambiguous placeholders only: the plain word "version" is legitimate text.
# Upper-case markers are case-sensitive; the phrases are not.
MARKER = re.compile(r"\b(TODO|TBD|FIXME|XXX|VERSION)\b|\{\{|\}\}|<[A-Za-z][^<>]*>")
PHRASE = re.compile(r"pending release review|lorem ipsum", re.I)
CL_MARKER = re.compile(r"\b(TODO|TBD|FIXME|XXX|VERSION)\b|\{\{|\}\}")
STRING = re.compile(r'"((?:[^"\\\n]|\\.)*)"')


def balanced(text, start):
    """text[start] is '{'; return the index after the matching '}' (string-aware), or None."""
    depth, i, n = 0, start, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            m = STRING.match(text, i)
            if not m:
                return None
            i = m.end()
            continue
        if text.startswith("//", i):
            i = text.find("\n", i)
            if i < 0:
                return None
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return None


def block_after(text, pattern):
    m = re.search(pattern, text, re.M)
    if not m:
        return None
    end = balanced(text, m.end() - 1)
    return text[m.end():end - 1] if end else None


def check_notes(ts, version):
    errors = []
    body = block_after(ts, r'^\s*"' + re.escape(version) + r'"\s*:\s*\{')
    if body is None:
        return [f"src/release-notes.ts has no (readable) entry for {version}"]
    if not re.search(r'\bdate\s*:\s*"\d{4}-\d{2}-\d{2}"', body):
        errors.append("release-notes.ts: date missing or not YYYY-MM-DD")
    for lang in ("de", "en"):
        part = block_after(body, r'(?<![A-Za-z0-9_])' + lang + r'\s*:\s*\{')
        lines = [m.group(1) for m in STRING.finditer(part)] if part is not None else []
        if not lines or not all(x.strip() for x in lines):
            errors.append(f"release-notes.ts {lang}: no or empty lines")
        elif any(MARKER.search(x) or PHRASE.search(x) for x in lines):
            errors.append(f"release-notes.ts {lang}: placeholder text")
    return errors


def check_changelog(text, version):
    spec = importlib.util.spec_from_file_location("changelog_notes", Path(__file__).with_name("changelog-notes.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    try:
        notes = mod.section(text, version)
    except ValueError as error:
        return [str(error)]
    if CL_MARKER.search(notes) or PHRASE.search(notes):
        return ["CHANGELOG section contains a placeholder"]
    return []


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("version")
    p.add_argument("--notes", type=Path, default=Path("src/release-notes.ts"))
    p.add_argument("--changelog", type=Path, default=Path("CHANGELOG.md"))
    a = p.parse_args()
    version = a.version.removeprefix("v")
    try:
        errors = check_notes(a.notes.read_text(encoding="utf-8"), version)
        errors += check_changelog(a.changelog.read_text(encoding="utf-8"), version)
    except OSError as error:
        errors = [str(error)]
    for e in errors:
        print(e)
    if not errors:
        print(f"OK: release notes and CHANGELOG are complete for {version}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
