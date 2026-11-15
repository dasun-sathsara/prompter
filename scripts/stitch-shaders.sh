#!/usr/bin/env bash
# Work around an upstream gap so the Apple silicon build can be produced on Linux.
#
# GPUI's macOS crate (gpui_apple) generates its runtime shader source in a build
# script that is gated with #[cfg(target_os = "macos")]. That gate is evaluated
# against the *host*, so cross-building from Linux skips the step and the crate
# then fails on a missing OUT_DIR/stitched_shaders.metal.
#
# This script runs the same generation logic on Linux: it builds a throwaway copy
# of that build script with the gate removed and fixed paths, then reruns cargo.
#
#   MAC_SDK=/path/to/MacOSX14.5.sdk ./scripts/stitch-shaders.sh
#
# Run it only when the cross build fails with that missing-file error.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/scripts/cross-env.sh"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/aarch64-apple-darwin/release"
GPUI_CHECKOUT="$(ls -d "$HOME"/.cargo/git/checkouts/zed-*/*/crates/gpui_apple | head -1)"
if [ -z "$GPUI_CHECKOUT" ] || [ ! -f "$GPUI_CHECKOUT/build.rs" ]; then
    echo "could not find the gpui_apple checkout" >&2
    exit 1
fi

# The first build has to run far enough to create the build-script directory.
cd "$ROOT"
cargo build --release --target aarch64-apple-darwin >/dev/null 2>&1 || true
OUT="$(ls -dt "$TARGET_DIR"/build/gpui_apple-*/out 2>/dev/null | head -1)"
if [ -z "$OUT" ]; then
    echo "no gpui_apple build output yet; run the cross build once" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src"
cat > "$WORK/Cargo.toml" <<'EOF'
[package]
name = "shadergen"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[build-dependencies]
cbindgen = { version = "0.28.0", default-features = false }
EOF
: > "$WORK/src/lib.rs"

python3 - "$GPUI_CHECKOUT" "$OUT" "$WORK/build.rs" <<'PY'
import sys
apple_dir, out, dest = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(f"{apple_dir}/build.rs").read()
# Run on any host: drop the cfg gate and keep only the runtime-shader path.
s = s.replace('    #[cfg(any(target_os = "macos", target_os = "ios"))]\n    apple_build::run();',
              '    apple_build::run();')
s = s.replace('#[cfg(any(target_os = "macos", target_os = "ios"))]\nmod apple_build {',
              'mod apple_build {')
s = s.replace('    #[cfg(not(feature = "runtime_shaders"))]\n    use std::ffi::OsString;\n', '')
s = s.replace('        #[cfg(feature = "runtime_shaders")]\n        emit_stitched_shaders(&header_path);\n        #[cfg(not(feature = "runtime_shaders"))]\n        compile_metal_shaders(&header_path);',
              '        emit_stitched_shaders(&header_path);')
s = s.replace('    #[cfg(not(feature = "runtime_shaders"))]\n    fn compile_metal_shaders',
              '    #[allow(dead_code)]\n    fn compile_metal_shaders')
s = s.replace('    #[cfg(feature = "runtime_shaders")]\n    fn emit_stitched_shaders',
              '    fn emit_stitched_shaders')
s = s.replace('PathBuf::from(env::var("OUT_DIR").unwrap()).join("scene.h")',
              'PathBuf::from("%s").join("scene.h")' % out)
s = s.replace('PathBuf::from(env::var("OUT_DIR").unwrap()).join("stitched_shaders.metal")',
              'PathBuf::from("%s").join("stitched_shaders.metal")' % out)
s = s.replace('let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());',
              'let crate_dir = PathBuf::from("%s");' % apple_dir)
s = s.replace('PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../gpui")',
              'PathBuf::from("%s/../gpui")' % apple_dir)
s = s.replace('let shader_source_path = "./src/shaders.metal";',
              'let shader_source_path = "%s/src/shaders.metal";' % apple_dir)
open(dest, "w").write(s)
PY

(cd "$WORK" && cargo build >/dev/null 2>&1)
echo "wrote $(ls "$OUT")"
cd "$ROOT"
cargo build --release --target aarch64-apple-darwin
