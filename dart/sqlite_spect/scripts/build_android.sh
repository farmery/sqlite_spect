#!/usr/bin/env bash
# Build libsqlite_spect_ffi.so for every Android ABI and stage the
# outputs under android/src/main/jniLibs/<abi>/ so the plugin's Gradle build
# picks them up. Requires the Android NDK (r25+ recommended).
#
# Usage:
#   NDK_HOME=/path/to/android-ndk ./scripts/build_android.sh
# Optional:
#   DEV=1 to build --profile dev instead of --release.

set -euo pipefail

if [[ -z "${NDK_HOME:-}" ]]; then
  echo "error: NDK_HOME must point at an Android NDK install" >&2
  exit 2
fi

PLUGIN_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_ROOT="$(cd "$PLUGIN_ROOT/../.." && pwd)"
CRATE="sqlite_spect_ffi"

# NDK 25/26 only ships darwin-x86_64 host tools (they run under Rosetta on
# Apple Silicon). NDK 27+ has darwin-arm64. Prefer arm64 when present.
for host in darwin-arm64 darwin-x86_64 linux-x86_64; do
  candidate="$NDK_HOME/toolchains/llvm/prebuilt/$host/bin"
  if [[ -d "$candidate" ]]; then
    TC="$candidate"
    break
  fi
done
if [[ -z "${TC:-}" ]]; then
  echo "error: could not find prebuilt toolchain under $NDK_HOME/toolchains/llvm/prebuilt/" >&2
  exit 2
fi
echo "using toolchain: $TC"

# Rows: cargo target triple : NDK clang wrapper : jniLibs ABI directory.
TARGETS=(
  "aarch64-linux-android:aarch64-linux-android21-clang:arm64-v8a"
  "armv7-linux-androideabi:armv7a-linux-androideabi21-clang:armeabi-v7a"
  "x86_64-linux-android:x86_64-linux-android21-clang:x86_64"
)

if [[ -n "${DEV:-}" ]]; then
  PROFILE_ARGS=()
  OUT_DIR="debug"
else
  PROFILE_ARGS=("--release")
  OUT_DIR="release"
fi

# Convert a lowercased-with-dashes triple to UPPER_UNDERSCORE form for cargo
# env vars. `tr` works everywhere; bash-4-only `${var^^}` does not (macOS
# ships bash 3.2).
upper_underscore() {
  echo "$1" | tr 'a-z-' 'A-Z_'
}

for entry in "${TARGETS[@]}"; do
  IFS=":" read -r triple wrapper abi <<< "$entry"
  echo "==> Building $triple -> $abi"

  clang="$TC/$wrapper"
  ar="$TC/llvm-ar"
  utrip="$(upper_underscore "$triple")"
  # `bash` rejects dashes in exported names, so use the underscore-triple
  # fallback that cc-rs also checks (see its `get_var`).
  ltrip="${triple//-/_}"

  # cc crate (used by rusqlite's bundled sqlite build) reads CC_<triple>
  # with an underscore-fallback; cargo linker override wants UPPER_UNDERSCORE.
  export "CC_${ltrip}=$clang"
  export "CXX_${ltrip}=$clang"
  export "AR_${ltrip}=$ar"
  export "CARGO_TARGET_${utrip}_LINKER=$clang"
  export "CARGO_TARGET_${utrip}_AR=$ar"

  (cd "$WORKSPACE_ROOT" && cargo build "${PROFILE_ARGS[@]}" -p "$CRATE" --target "$triple")

  dest="$PLUGIN_ROOT/android/src/main/jniLibs/$abi"
  mkdir -p "$dest"
  cp "$WORKSPACE_ROOT/target/$triple/$OUT_DIR/lib${CRATE}.so" "$dest/"
  echo "    staged $dest/lib${CRATE}.so"
done

echo
echo "done — .so files are under $PLUGIN_ROOT/android/src/main/jniLibs/"
