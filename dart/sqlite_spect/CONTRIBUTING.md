# Contributing to sqlite_spect

## Platform support status

**Fully supported:**
- Android (plugin + CLI discovery/attach)

**Partial:**
- iOS (plugin FFI works, but CLI discovery/attach not implemented)

**Not yet supported:**
- macOS, Linux, Windows (plugin, CLI discovery/attach)

## Prerequisites

- Flutter ≥3.44 (Dart ≥3.12) — enforced by the plugin's pubspec
- Node.js + npm (v20 or later) for the web client
- Rust stable toolchain
- Android: NDK (`NDK_HOME`) for the Android scripts; an emulator or device for
  running the example app
- iOS: Xcode

## Building from a fresh clone

Native artifacts (web client bundle, `.so`/xcframework/dylib) are not
committed to the repo — build them locally, in this order:

```bash
# 1. Web client — cargo embeds dist/ into the Rust binary at compile time,
#    so this must run before any cargo build of the crates.
(cd client/sqlite_spect_client && npm ci && npm run build)

# 2. Native libraries for the platforms you need (from dart/sqlite_spect/)
cd dart/sqlite_spect
./scripts/build_android.sh    # requires NDK_HOME
./scripts/build_ios.sh
```

Consumer apps get the compiled artifacts via the plugin's platform manifests
(`android/build.gradle`, `ios/*.podspec`).

To test the `attach` flow locally, build the CLI (requires step 1):

```bash
cargo build -p sqlite_spect_core --features cli --bin sqlite_spect
./target/debug/sqlite_spect attach
```

To verify that the native library is excluded from release builds:

```bash
./scripts/verify_release.sh android
./scripts/verify_release.sh ios
```

Consumers installing from pub.dev get prebuilt binaries: the release
workflow builds these same artifacts in CI and re-includes them in the
published package (by commenting out their `.gitignore` entries right
before `flutter pub publish`).

## Known gaps / TODOs

**iOS `attach`** — `cmd_attach_ios` in `crates/sqlite_spect_core/src/attach.rs`
exits immediately with an error. Needs implementation for both simulator (simctl)
and physical device (mDNS) discovery.

**Probe streaming** — `Inspector.recordQuery` accepts calls and the Dart API is
fully wired, but the server-side subscriber routing (`probe.tail` /
`probe.untail` in `src/rpc.rs`) is stubbed with TODOs. Queries are not yet
streamed to the browser UI.

**macOS/Linux/Windows support** — the plugin FFI can build for these platforms,
but CLI discovery/attach has no implementation. Desktop SQLite inspection is
planned as a follow-up.
