# sqlite_spect

In-app SQLite inspector for Flutter — browse, query, and edit your SQLite
databases live from a browser.

**Debug and staging only.** The plugin no-ops in release mode and is best added
as a `dev_dependency` so it's excluded from release builds entirely.

## Quick start

### 1. Add the plugin

```yaml
# your app's pubspec.yaml
dev_dependencies:
  sqlite_spect: ^0.1.0
```

### 2. Initialize in your app

```dart
// anywhere near your db init (Make sure the db has been created and valid at this point)
if (kDebugMode) {
  await Inspector.start(
    databases: [InspectedDatabase(id: 'my_database', path: dbPath)],
  );
}
```

### 3. Download the CLI

Grab the prebuilt `sqlite_spect` binary for your OS from the
[GitHub releases page][releases]. Extract it and add to your `PATH`
(e.g., `/usr/local/bin/` on macOS/Linux, or anywhere on Windows).

### 4. Run the inspector

Once your app is running, and the inspector service is running, run the command below in your terminal to 
open up the inspector client on your browser.

```bash
sqlite_spect attach
```

> **Android only right now.** iOS is still in the works.

### Attach options

```bash
sqlite_spect attach [--watch] [--port PORT] [--serial SERIAL]
```

* `--watch` — (Android only) keep the adb forward alive; re-opens the browser on reconnect.
* `--port PORT` — override the default port (8123). **If you use this, pass the
  same port to `Inspector.start(port: PORT)` in your app.**
* `--serial SERIAL` — (Android only) target a specific device. Run `adb devices` to list serials.

Example:

```bash
sqlite_spect attach --watch --serial {SERIAL} --port 9000
```

And in your app:

```dart
await Inspector.start(
  port: 9000,
  databases: [InspectedDatabase(id: 'main', path: dbPath)],
);
```

## Isolate rules

* `Inspector.start` / `Inspector.stop` must be called from the root isolate.
* If your app opens SQLite in a background isolate,
  just register the DB *path* via `Inspector.start` from the main isolate.

## CLI installation options

### Prebuilt binaries (recommended)

Available at [GitHub releases][releases]:

| OS      | Arch    | Archive                                    |
|---------|---------|------------------------------------------|
| macOS   | ARM64   | `sqlite_spect-*-aarch64-apple-darwin.tar.gz` |
| macOS   | x86_64  | `sqlite_spect-*-x86_64-apple-darwin.tar.gz`  |
| Linux   | x86_64  | `sqlite_spect-*-x86_64-unknown-linux-gnu.tar.gz` |
| Windows | x86_64  | `sqlite_spect-*-x86_64-pc-windows-msvc.zip`  |

Extract, add to `PATH`. Each release ships `SHA256SUMS.txt` for verification.

[releases]: https://github.com/farmery/sqlite_spect/releases
