# AGENTS.md

Guidance for ZCode agents working in this repository.

## What this is

sqlite_spect is an in-app SQLite inspector for Flutter. A Rust engine is
embedded in the host app via `dart:ffi` and serves a React inspector UI over a
loopback HTTP+WebSocket server; a companion CLI (`sqlite_spect attach`)
discovers a running app via mDNS/adb and opens the inspector in a browser.
Debug/staging only — the plugin no-ops in release mode.

| Path | What it is |
|------|------------|
| `dart/sqlite_spect/` | Flutter plugin (published to pub.dev) — Dart API, hand-written FFI bindings, platform manifests |
| `crates/sqlite_spect_core/` | Rust engine: axum HTTP+WS server, rusqlite access, JSON-RPC dispatch (`src/rpc.rs`), mDNS/adb discovery, CLI bin |
| `crates/sqlite_spect_ffi/` | C ABI (`cdylib`/`staticlib`) over the engine, loaded from Dart |
| `client/sqlite_spect_client/` | React 19 + Vite + TypeScript inspector UI |
| `.github/workflows/release.yml` | Tag-triggered release: builds client, native libs, CLI; publishes to GitHub Releases + pub.dev via OIDC |

Read `dart/sqlite_spect/CONTRIBUTING.md` before touching platform, build, or
release concerns — it documents platform support status and known gaps.

## Build order (critical gotcha)

`rust-embed` embeds `client/sqlite_spect_client/dist` into the Rust binary at
compile time (`crates/sqlite_spect_core/src/server.rs`). The client `dist/`
and native libs are not committed — build them locally, web client first:

```bash
(cd client/sqlite_spect_client && npm ci && npm run build)  # BEFORE any cargo build
cargo build
```

## Commands

```bash
# Web client (from client/sqlite_spect_client/)
npm run build        # tsc -b && vite build
npm run lint         # oxlint (not eslint)
npm run dev

# Rust (from repo root)
cargo test                                                     # tests are inline #[cfg(test)] modules
cargo build -p sqlite_spect_core --features cli --bin sqlite_spect
./target/debug/sqlite_spect attach                             # test the attach flow locally

# Dart plugin (from dart/sqlite_spect/)
flutter analyze
flutter test

# Native libs for the plugin (from dart/sqlite_spect/)
./scripts/build_android.sh              # requires NDK_HOME; DEV=1 for dev profile
./scripts/build_ios.sh
./scripts/verify_release.sh android|ios # proves native lib is excluded from release builds
```

## Manual live testing (end-to-end)

Full flow against an Android emulator, no `flutter run` needed (this Flutter
version has no `--no-resident`, so a resident `flutter run` would tie the app
to your terminal):

```bash
# 0. Rebuild the web client first (see build order above), then force the
#    crates to re-embed it — rust-embed bakes dist/ in at compile time and
#    cargo does NOT rebuild when only embedded assets change:
touch crates/sqlite_spect_core/src/server.rs

# 1. Native libs (needs NDK; NDKs live in ~/Library/Android/sdk/ndk/)
cd dart/sqlite_spect && NDK_HOME=<ndk-path> ./scripts/build_android.sh && cd ../..

# 2. Example app → install → launch detached
cd dart/sqlite_spect/example && flutter build apk --debug
adb install -r build/app/outputs/flutter-apk/app-debug.apk
adb shell monkey -p com.sqlite_spect.example -c android.intent.category.LAUNCHER 1

# 3. Attach the inspector (adb-forwards port 8123, probes the app)
cd ../../.. && cargo build -p sqlite_spect_core --features cli --bin sqlite_spect
./target/debug/sqlite_spect attach --no-browser   # then browse http://127.0.0.1:8123

# Verify the app started the inspector:
adb logcat -d | grep sqlite_spect   # → "sqlite_spect at http://127.0.0.1:8123"
```

Quick client-only loop (no emulator/app needed) — the debug CLI serves the
embedded web client against a local SQLite file:

```bash
./target/debug/sqlite_spect serve /path/to/db.sqlite   # http://127.0.0.1:8123
```

## Architecture rules

- **No method channels.** The plugin is FFI-only (`ffiPlugin: true`); platform
  shells (Kotlin/Swift) exist for discovery but never proxy calls.
- **FFI C ABI contract** (`crates/sqlite_spect_ffi/src/lib.rs`): every export
  wraps its body in `catch_unwind`; string args are null-terminated UTF-8 owned
  by the caller; string returns are callee-owned and must be freed via
  `inspector_free_string`. ABI changes must be mirrored in the hand-written
  bindings at `dart/sqlite_spect/lib/src/ffi_bindings.dart`.
- **Transport**: JSON-RPC 2.0 over WebSocket, dispatched in
  `crates/sqlite_spect_core/src/rpc.rs` (spec codes -326xx; FFI-layer errors
  use negative custom codes).
- Client changes only appear after rebuilding `dist/` and then the Rust binary.

## Conventions

- Rust: edition 2021; `tracing` for logs (`paranoid-android` on Android,
  `tracing-oslog` on iOS, both in the ffi crate).
- Dart: `flutter_lints` 6.
- Client: oxlint for linting, TypeScript checked via `tsc -b`.
- Release profile is `strip = true`, `lto = "fat"`, `codegen-units = 1` —
  native libs ship inside the pub package, keep them lean.

## Release

`./release.sh X.Y.Z ["one-line note"]` from the repo root drives the whole
release. Steps, including the gotchas hit during the v0.2.1 release:

```bash
# 1. Pre-flight: commit AND push all work — the script dies if the tree is
#    dirty or main is out of sync with origin/main.
git push origin main

# 2. If CHANGELOG.md has an "## Unreleased" section, fold its bullets into
#    the release note and delete the section first — release.sh blindly
#    prepends the new "## X.Y.Z" entry on top, which would strand
#    "## Unreleased" below the new version.

# 3. Release (the script prompts for confirmation; pipe "y" when scripted).
#    It bumps dart/sqlite_spect/pubspec.yaml, prepends a CHANGELOG entry,
#    commits "chore: release vX.Y.Z", tags vX.Y.Z, and pushes main + tag.
printf 'y\n' | ./release.sh 0.2.1 "One-line release note."

# 4. Watch CI build + publish (no manual steps, no stored credentials —
#    pub.dev publishing uses OIDC).
gh run list --workflow release.yml --limit 1     # grab the run id
gh run watch <run-id> --exit-status

# 5. Verify the three outcomes.
gh release view vX.Y.Z                           # GitHub release + CLI binaries
curl -s https://pub.dev/api/packages/sqlite_spect | jq '.latest.version'  # pub.dev
```

Notes: version lives in `dart/sqlite_spect/pubspec.yaml` only (Rust crates
are versioned independently); CI builds the web client, Android/iOS native
libs, and per-OS CLI binaries, cuts the GitHub release with SHA256SUMS, and
publishes the package. The script also refuses to run if the tag already
exists locally or on origin.

## Known gaps (intentional, don't "fix" silently)

- Editing UI is temporarily disabled client-side
  (`client/sqlite_spect_client/src/editing.ts`, `EDITING_ENABLED = false`)
  pending a data-mutation bug investigation. The server RPCs and the SQL
  Runner are intentionally left enabled — don't "complete" the disable by
  touching them.
- iOS `attach`: `cmd_attach_ios` in `crates/sqlite_spect_core/src/attach.rs`
  is a stub that exits with an error.
- Probe streaming: `probe.tail` / `probe.untail` in `rpc.rs` are stubbed with
  TODOs; queries are not yet streamed to the browser UI.
- macOS/Linux/Windows: plugin FFI can build, but CLI discovery/attach is
  unimplemented.
