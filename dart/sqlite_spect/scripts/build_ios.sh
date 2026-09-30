#!/usr/bin/env bash
# Build sqlite_spect_ffi.xcframework for the Flutter plugin. Combines the
# device slice (aarch64-apple-ios) with a fat simulator slice
# (aarch64-apple-ios-sim + x86_64-apple-ios) so `flutter run` works on both
# an iPhone and the iOS simulator.
#
# Usage: ./scripts/build_ios.sh

set -euo pipefail

PLUGIN_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_ROOT="$(cd "$PLUGIN_ROOT/../.." && pwd)"
CRATE="sqlite_spect_ffi"
LIB="lib${CRATE}.a"

cd "$WORKSPACE_ROOT"

# bindgen (via tracing-oslog) can't find <os/log.h> when cross-compiling
# unless it gets the target SDK's sysroot. The simulator targets also need an
# explicit clang triple override — libclang rejects "aarch64-apple-ios-sim"
# as a version string ("version 'sim' is invalid").
build_target() {
  local target="$1" sdk="$2" clang_target="${3:-}"
  local sdk_path
  sdk_path="$(xcrun --sdk "$sdk" --show-sdk-path)"
  local args="--sysroot=$sdk_path"
  [[ -n "$clang_target" ]] && args="$args --target=$clang_target"
  echo "==> Building $target (sdk: $sdk)"
  SDKROOT="$sdk_path" \
  BINDGEN_EXTRA_CLANG_ARGS="$args" \
    cargo build --release -p "$CRATE" --target "$target"
}

build_target aarch64-apple-ios iphoneos
build_target aarch64-apple-ios-sim iphonesimulator arm64-apple-ios
build_target x86_64-apple-ios iphonesimulator

STAGE="$(mktemp -d)"
mkdir -p "$STAGE/sim"

# Simulator fat static lib.
lipo -create \
  "target/aarch64-apple-ios-sim/release/$LIB" \
  "target/x86_64-apple-ios/release/$LIB" \
  -output "$STAGE/sim/$LIB"

XCF_OUT="$PLUGIN_ROOT/ios/Frameworks/${CRATE}.xcframework"
rm -rf "$XCF_OUT"

xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/release/$LIB" \
  -library "$STAGE/sim/$LIB" \
  -output "$XCF_OUT"

echo "done — $XCF_OUT"
