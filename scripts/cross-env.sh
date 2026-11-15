#!/bin/sh
# Environment for cross-linking Rust to aarch64-apple-darwin from Linux.
#
#   MAC_SDK=/path/to/MacOSX14.5.sdk . scripts/cross-env.sh
#
# Needs clang and lld (Ubuntu: clang, lld-18, llvm-18), the rustup target
# aarch64-apple-darwin, and a macOS SDK. A public one works:
#   https://github.com/joseluisq/macosx-sdks/releases  (MacOSX14.5.sdk.tar.xz)
#
# The resulting binary is ad-hoc signed by ld64.lld, which is what an Apple
# silicon Mac needs to run it at all.

set -eu

: "${MAC_SDK:?set MAC_SDK to an unpacked MacOSX*.sdk directory}"

export SDKROOT="$MAC_SDK"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
export MAC_TARGET=aarch64-apple-darwin

# clang drives ld64.lld for a darwin target; -isysroot and -target are what let
# rustc's -lSystem resolve without xcrun.
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=clang
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-target -C link-arg=arm64-apple-macos${MACOSX_DEPLOYMENT_TARGET} -C link-arg=-isysroot -C link-arg=$MAC_SDK -C link-arg=-fuse-ld=lld -C link-arg=-Wl,-adhoc_codesign"

# For build scripts that compile C.
export CC_aarch64_apple_darwin=clang
export CXX_aarch64_apple_darwin=clang
export CFLAGS_aarch64_apple_darwin="-target arm64-apple-macos${MACOSX_DEPLOYMENT_TARGET} -isysroot $MAC_SDK"
export CXXFLAGS_aarch64_apple_darwin="-target arm64-apple-macos${MACOSX_DEPLOYMENT_TARGET} -isysroot $MAC_SDK"
