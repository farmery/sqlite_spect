#!/usr/bin/env bash
# Prove that the recommended dev_dependencies wiring keeps
# libsqlite_spect_ffi out of release builds. Run from CI after any
# change to the plugin's platform packaging.
#
# Usage:
#   ./scripts/verify_release.sh android   # builds example APK, greps for .so
#   ./scripts/verify_release.sh ios       # builds Runner.app, greps otool -L
#
# Exits 0 on success (no inspector artefact in the release binary), 1 on
# failure.

set -euo pipefail

PLATFORM="${1:-}"
PLUGIN_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLE_ROOT="$PLUGIN_ROOT/example"

if [[ -z "$PLATFORM" ]]; then
  echo "usage: $0 <android|ios>" >&2
  exit 2
fi

cd "$EXAMPLE_ROOT"

case "$PLATFORM" in
  android)
    echo "==> flutter build apk --release"
    flutter build apk --release
    APK="build/app/outputs/flutter-apk/app-release.apk"
    if [[ ! -f "$APK" ]]; then
      echo "expected release APK at $APK — build failed?" >&2
      exit 1
    fi
    # `unzip -l | grep` — the .so lives under lib/<abi>/. Any hit = failure.
    if unzip -l "$APK" | grep -q "libsqlite_spect_ffi.so"; then
      echo "✗ release APK contains libsqlite_spect_ffi.so" >&2
      echo "  Move the sqlite_spect dependency to dev_dependencies to exclude it." >&2
      exit 1
    fi
    echo "✔ APK is clean of libsqlite_spect_ffi.so"
    ;;

  ios)
    echo "==> flutter build ios --release --no-codesign"
    flutter build ios --release --no-codesign
    APP="build/ios/iphoneos/Runner.app/Runner"
    if [[ ! -x "$APP" ]]; then
      echo "expected Runner binary at $APP — build failed?" >&2
      exit 1
    fi
    if otool -L "$APP" | grep -q "libsqlite_spect_ffi"; then
      echo "✗ release ios binary references libsqlite_spect_ffi" >&2
      exit 1
    fi
    echo "✔ iOS Runner is clean of libsqlite_spect_ffi"
    ;;

  *)
    echo "unknown platform: $PLATFORM (expected android|ios)" >&2
    exit 2
    ;;
esac
