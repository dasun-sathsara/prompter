#!/usr/bin/env bash
# Wrap the built binary in a .app bundle.
#
#   ./scripts/bundle.sh [path-to-binary] [output-dir]
#
# Defaults to the apple-silicon release build and dist/.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/aarch64-apple-darwin/release/prompter}"
OUT="${2:-$ROOT/dist}"
APP="$OUT/Prompter.app"

if [ ! -f "$BIN" ]; then
    echo "no binary at $BIN" >&2
    exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/prompter"
chmod 755 "$APP/Contents/MacOS/prompter"
cp "$ROOT/resources/Info.plist" "$APP/Contents/Info.plist"
if [ -f "$ROOT/resources/icon.icns" ]; then
    cp "$ROOT/resources/icon.icns" "$APP/Contents/Resources/icon.icns"
fi

# Ad-hoc signature: enough for local use on Apple silicon. Has no effect on
# Linux, where this script may also run while cross-building.
if command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "$APP" >/dev/null 2>&1 || true
fi

echo "$APP"
