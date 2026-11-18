#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ "$(uname -s)" != Darwin ]]; then
    echo "Esegui questo script su macOS, con Xcode Command Line Tools installati." >&2
    exit 1
fi
TARGET="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
case "$TARGET" in
    aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) echo "Target supportati: aarch64-apple-darwin o x86_64-apple-darwin" >&2; exit 1 ;;
esac
rustup target add "$TARGET"
cargo build --locked --release --target "$TARGET"

OUT="$ROOT/dist/macos/$TARGET"
APP="$OUT/marzianetto.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/$TARGET/release/marzianetto" "$APP/Contents/MacOS/marzianetto"
chmod +x "$APP/Contents/MacOS/marzianetto"
cp "$ROOT/packaging/assets/marzianetto.icns" "$APP/Contents/Resources/marzianetto.icns"
python3 - "$ROOT" "$APP" <<'PY'
import pathlib, plistlib, re, sys
root, app = map(pathlib.Path, sys.argv[1:])
version = re.search(r'^version\s*=\s*"([^"]+)"', (root / 'Cargo.toml').read_text(), re.M).group(1)
info = {
    'CFBundleName': 'marzianetto',
    'CFBundleDisplayName': 'marzianetto',
    'CFBundleIdentifier': 'it.marzianetto.app',
    'CFBundleExecutable': 'marzianetto',
    'CFBundleIconFile': 'marzianetto.icns',
    'CFBundlePackageType': 'APPL',
    'CFBundleShortVersionString': version,
    'CFBundleVersion': version,
    'NSHighResolutionCapable': True,
}
with (app / 'Contents/Info.plist').open('wb') as stream:
    plistlib.dump(info, stream)
PY
# Firma ad hoc per uso locale, NON equivalente a Developer ID/notarizzazione.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
STAGE="$(mktemp -d "$OUT/dmg-root.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/marzianetto.app"
if [[ ! -e "$STAGE/Applications" && ! -L "$STAGE/Applications" ]]; then
    ln -s /Applications "$STAGE/Applications"
fi
hdiutil create -volname "marzianetto" -srcfolder "$STAGE" -ov -format UDZO "$OUT/marzianetto-$TARGET.dmg"
UPDATE="$OUT/marzianetto-$TARGET.app.zip"
ditto -c -k --keepParent "$APP" "$STAGE/update.zip"
mv -f "$STAGE/update.zip" "$UPDATE"
echo "Creati: $APP"
echo "DMG: $OUT/marzianetto-$TARGET.dmg"
echo "ZIP aggiornamenti: $UPDATE"
echo "Per distribuirlo pubblicamente, firma con Developer ID e notarizza: vedi BUILDING.md."
