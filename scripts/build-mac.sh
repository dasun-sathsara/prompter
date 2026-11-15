#!/usr/bin/env bash
# Cross-build the Apple silicon app bundle on Linux.
#
#   MAC_SDK=/path/to/MacOSX14.5.sdk ./scripts/build-mac.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/scripts/cross-env.sh"
cd "$ROOT"
cargo build --release --target aarch64-apple-darwin
./scripts/bundle.sh "$ROOT/target/aarch64-apple-darwin/release/prompter" "$ROOT/dist"
