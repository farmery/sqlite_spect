# sqlite_spect

In-app SQLite inspector for Flutter — browse, query, and edit your SQLite
databases live from a browser.

A Rust engine is embedded into your app via `dart:ffi`; it serves the
inspector UI (an embedded web client) over a loopback HTTP+WebSocket server.
A companion CLI discovers a running app (mDNS / adb) and opens the inspector
in your browser. **Debug and staging only** — the plugin no-ops in release
mode and is best added as a `dev_dependency`.

## Repository layout

| Path | What it is |
|------|------------|
| `dart/sqlite_spect/` | The Flutter plugin (published as `sqlite_spect` on pub.dev) — **start here: [its README][plugin] has the full usage guide** |
| `crates/sqlite_spect_core/` | Rust engine: HTTP server, embedded web client, SQLite access, CLI |
| `crates/sqlite_spect_ffi/` | C ABI (`cdylib`) over the engine, loaded by the plugin via `dart:ffi` |
| `client/sqlite_spect_client/` | React/Vite inspector UI, embedded into the Rust binary at compile time |
| `.github/workflows/release.yml` | Tag-triggered release: builds client, native libs per platform, CLI, publishes to GitHub Releases and pub.dev |

## Quick start

```yaml
# your app's pubspec.yaml
dev_dependencies:
  sqlite_spect: ^0.1.0
```

```dart
if (kDebugMode) {
  await Inspector.start(
    databases: [InspectedDatabase(id: 'my_database', path: dbPath)],
  );
}
```

Then run `sqlite_spect attach` with the [prebuilt CLI][releases] to open the
inspector in your browser. **Android is fully supported; iOS is in progress.**
Full instructions: [plugin README][plugin].

## Building from source

Native artifacts (web client bundle, `.so`/xcframework/dylib) are not
committed — see [CONTRIBUTING][contributing] for the build order (web client
first, then the per-platform native libraries).

[plugin]: dart/sqlite_spect/README.md
[releases]: https://github.com/farmery/sqlite_spect/releases
[contributing]: dart/sqlite_spect/CONTRIBUTING.md
