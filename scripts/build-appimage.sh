#!/usr/bin/env bash
set -euo pipefail

# Immer aus diesem Checkout bauen; keine Downloads von Werkzeugen.
PROJECT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$PROJECT_DIR"
: "${APPIMAGETOOL:?APPIMAGETOOL muss auf ein vorhandenes appimagetool zeigen}"
APPIMAGETOOL=$(realpath -- "$APPIMAGETOOL")
[[ -x "$APPIMAGETOOL" ]] || { echo 'appimagetool ist nicht ausführbar' >&2; exit 1; }
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$PROJECT_DIR/src-tauri/target"}
CARGO_TARGET_DIR=$(realpath -m -- "$CARGO_TARGET_DIR")
OUTPUT_DIR=${OUTPUT_DIR:-"$PROJECT_DIR/dist-appimage"}
mkdir -p -- "$OUTPUT_DIR"
OUTPUT_DIR=$(realpath -- "$OUTPUT_DIR")
VERSION=$(node -p 'JSON.parse(require("fs").readFileSync("src-tauri/tauri.conf.json", "utf8")).version')
# Build for the host architecture by default (x86_64 or aarch64 on a Raspberry Pi); override with ARCH.
ARCH=${ARCH:-$(uname -m)}

NO_STRIP=1 APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri build -- --bundles appimage

# Die erzeugte AppDir erhalten; nur eine temporäre Kopie bearbeiten.
WORK_DIR=$(mktemp -d "$OUTPUT_DIR/.appimage-build.XXXXXX")
trap 'rm -rf -- "$WORK_DIR"' EXIT
cp -a -- "$CARGO_TARGET_DIR/release/bundle/appimage/Dualfile.AppDir" "$WORK_DIR/AppDir"

# Gebündelte Bibliotheken verursachen bei neuerem Mesa EGL_BAD_PARAMETER.
# Quelle: https://github.com/tauri-apps/tauri/issues/15976
for library in \
    libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 \
    libwayland-server.so.0 libxkbcommon.so.0 libxcb-randr.so.0 \
    libxcb-render.so.0 libxcb-shm.so.0 libXau.so.6 libXdmcp.so.6; do
    rm -f -- "$WORK_DIR/AppDir/usr/lib/$library"
done
cd -- "$WORK_DIR"
ARCH="$ARCH" "$APPIMAGETOOL" AppDir "$OUTPUT_DIR/Dualfile-$VERSION-$ARCH.AppImage"

# Also ship the same repacked AppDir (libraries already removed) as a tarball
# with a single named top-level directory, runnable via ./Dualfile-*/AppRun.
tar -czf "$OUTPUT_DIR/Dualfile-$VERSION-$ARCH.tar.gz" -C "$WORK_DIR" \
    --transform "s,^AppDir,Dualfile-$VERSION-$ARCH," AppDir
