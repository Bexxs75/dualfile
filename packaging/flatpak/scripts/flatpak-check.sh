#!/bin/sh
# Run every available validator, including when the other tool is missing.
set -u
base=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd) || exit 1
status=0
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$base/com.dualfileapp.Dualfile.desktop" || status=1
else
    echo 'MISSING: desktop-file-validate; desktop validation was not run.' >&2
    status=1
fi
if command -v appstreamcli >/dev/null 2>&1; then
    appstreamcli validate --strict --no-net "$base/com.dualfileapp.Dualfile.metainfo.xml" || status=1
else
    echo 'MISSING: appstreamcli; AppStream validation was not run.' >&2
    status=1
fi
# Parse the actual source entry rather than accepting matching text in comments.
if command -v python3 >/dev/null 2>&1; then
    python3 - "$base/com.dualfileapp.Dualfile.yml" <<'PYTHON' || status=1
import sys
from pathlib import Path

try:
    import yaml
except ImportError:
    sys.exit("MISSING: Python PyYAML; manifest validation was not run.")

manifest = Path(sys.argv[1])
text = manifest.read_text()
errors = []
for stale in ("TODO_SOURCE_CHECKOUT", "node20"):
    if stale in text:
        errors.append(f"stale manifest text: {stale}")
try:
    data = yaml.safe_load(text)
    module = next(m for m in data["modules"] if m["name"] == "dualfile")
    sources = [s for s in module["sources"] if isinstance(s, dict) and s.get("type") == "dir"]
    if len(sources) != 1 or sources[0].get("path") != "../..":
        errors.append("expected one dir source with path ../..")
    else:
        required = {".git", "node_modules", "src-tauri/target", "target", "dist",
                    "build-flatpak", ".flatpak-builder", ".cargo", "src-tauri/.cargo"}
        skip = sources[0].get("skip", [])
        if not isinstance(skip, list) or not all(isinstance(s, str) for s in skip):
            errors.append("dir source skip must be a list of paths")
        else:
            missing = required - set(skip)
            if missing:
                errors.append("missing source exclusions: " + ", ".join(sorted(missing)))
except (yaml.YAMLError, KeyError, TypeError, StopIteration) as exc:
    errors.append(f"invalid manifest structure: {exc}")
for error in errors:
    print(f"FAIL: {error}", file=sys.stderr)
if errors:
    sys.exit(1)
print("Flatpak source manifest checks passed.")
PYTHON
else
    echo 'MISSING: python3; manifest validation was not run.' >&2
    status=1
fi
if [ "$status" -ne 0 ]; then
    echo 'Flatpak metadata/source checks incomplete or failed (see above).' >&2
else
    echo 'Flatpak metadata/source checks passed.'
fi
exit "$status"
