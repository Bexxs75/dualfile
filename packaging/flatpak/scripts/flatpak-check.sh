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
if [ "$status" -ne 0 ]; then
    echo 'Flatpak metadata checks incomplete or failed (see above).' >&2
else
    echo 'Flatpak metadata checks passed.'
fi
exit "$status"
