#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
STAGE="$ROOT/packaging/flatpak/sources"
mkdir -p "$STAGE/.cargo" "$STAGE/assets"
# Il download avviene PRIMA della sandbox: flatpak-builder compila offline.
cargo vendor --locked --versioned-dirs "$STAGE/vendor" > /dev/null
cat > "$STAGE/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"
[source.vendored-sources]
directory = "vendor"
EOF
cp Cargo.toml Cargo.lock "$STAGE/"
# Copia solo il codice, mai target/, dist/ o directory personali.
mkdir -p "$STAGE/src"
cp src/*.rs "$STAGE/src/"
cp packaging/assets/* "$STAGE/assets/"
echo "Sorgenti Flatpak pronti in $STAGE"
echo "Prosegui su Linux con i comandi di BUILDING.md."
