#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    echo "Esegui questo script su Linux x86_64." >&2
    exit 1
fi

cargo build --locked --release
TOOLS="$ROOT/target/appimage-tools"
mkdir -p "$TOOLS" "$ROOT/dist/linux"
curl --fail --location --retry 3 \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    --output "$TOOLS/linuxdeploy-x86_64.AppImage"
chmod +x "$TOOLS/linuxdeploy-x86_64.AppImage"

# Una AppDir nuova evita di includere file residui delle build precedenti.
STAGE="$(mktemp -d "$ROOT/target/appimage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export LDAI_OUTPUT="$ROOT/dist/linux/marzianetto-linux-x86_64.AppImage"
"$TOOLS/linuxdeploy-x86_64.AppImage" \
    --appdir "$STAGE/AppDir" \
    --executable "$ROOT/target/release/marzianetto" \
    --desktop-file "$ROOT/packaging/assets/it.marzianetto.app.desktop" \
    --icon-file "$ROOT/packaging/assets/it.marzianetto.app.svg" \
    --output appimage
chmod +x "$LDAI_OUTPUT"

# Verifica che il pacchetto prodotto contenga l'eseguibile, senza richiedere FUSE.
mkdir -p "$STAGE/check"
(
    cd "$STAGE/check"
    "$LDAI_OUTPUT" --appimage-extract >/dev/null
    test -x squashfs-root/AppRun
    test -x squashfs-root/usr/bin/marzianetto
)
echo "AppImage: $LDAI_OUTPUT"
