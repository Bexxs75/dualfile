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

# RAR helper: the static 7-Zip console build (7zzs) ships as usr/bin/7zz (found by rar.rs,
# see docs/third-party-7zip.md). The archive must be downloaded and its SHA-256 verified by the
# caller (release.yml); this script does not download anything. SKIP_7ZZ=1 builds without it.
if [[ "${SKIP_7ZZ:-0}" != 1 ]]; then
    : "${SEVENZIP_ARCHIVE:?SEVENZIP_ARCHIVE muss auf das geprüfte 7z*-linux-*.tar.xz zeigen (oder SKIP_7ZZ=1)}"
    [[ -f "$SEVENZIP_ARCHIVE" ]] || { echo 'SEVENZIP_ARCHIVE nicht gefunden' >&2; exit 1; }
    mkdir -p -- "$WORK_DIR/7zip"
    tar -xJf "$SEVENZIP_ARCHIVE" -C "$WORK_DIR/7zip" 7zzs License.txt
    install -D -m 0755 -- "$WORK_DIR/7zip/7zzs" "$WORK_DIR/AppDir/usr/bin/7zz"
    install -D -m 0644 -- "$WORK_DIR/7zip/License.txt" "$WORK_DIR/AppDir/usr/share/doc/7zip/License.txt"
    # Fail the build instead of shipping a broken or dynamically linked helper.
    [[ -x "$WORK_DIR/AppDir/usr/bin/7zz" && -s "$WORK_DIR/AppDir/usr/share/doc/7zip/License.txt" ]] \
        || { echo '7zz oder Lizenzdatei fehlt in der AppDir' >&2; exit 1; }
    file -b -- "$WORK_DIR/AppDir/usr/bin/7zz" | grep -q 'statically linked' \
        || { echo '7zz ist nicht statisch gelinkt' >&2; exit 1; }
fi
cd -- "$WORK_DIR"
ARCH="$ARCH" "$APPIMAGETOOL" AppDir "$OUTPUT_DIR/Dualfile-$VERSION-$ARCH.AppImage"

# Also ship the same repacked AppDir (libraries already removed) as a tarball
# with a single named top-level directory, runnable via ./Dualfile-*/AppRun.
tar -czf "$OUTPUT_DIR/Dualfile-$VERSION-$ARCH.tar.gz" -C "$WORK_DIR" \
    --transform "s,^AppDir,Dualfile-$VERSION-$ARCH," AppDir

if [[ "${SKIP_7ZZ:-0}" != 1 ]]; then
    tar -tzf "$OUTPUT_DIR/Dualfile-$VERSION-$ARCH.tar.gz" | grep -q "^Dualfile-$VERSION-$ARCH/usr/bin/7zz$" \
        || { echo '7zz fehlt im Tarball' >&2; exit 1; }
fi
