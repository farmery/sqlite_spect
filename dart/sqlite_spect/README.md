# sqlite_spect

In-app SQLite inspector for Flutter — browse, query, and (once editing is
fixed) edit your SQLite databases live from a browser.

> **⚠️ Editing is a work in progress.** Data editing in the inspector UI
> (cell edits, add/delete row, clear table) is temporarily disabled while a
> bug is investigated. The SQL Runner still executes arbitrary statements —
> be aware that write statements may misbehave and could corrupt your
> database.

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

### Attach options

```bash
sqlite_spect attach [--platform PLATFORM] [--port PORT] [--serial SERIAL] [--pid PID] [--json] [--no-browser]
```

Two discovery channels, selected by `--platform`:

* `--platform android` *(default)* — connects through adb: forwards the
  device port to your machine and opens `http://127.0.0.1:8123`.
  Fully supported (emulator and physical devices).
* `--platform ios` — network discovery via mDNS: the inspector announces
  itself on the local network and `attach` finds it — no IP guessing
  needed. Verified on the iOS simulator; physical devices on the same
  Wi-Fi should work the same way (not yet hardware-tested).

Flags:

* `--watch` — (android only) keep the adb forward alive; re-opens the browser on reconnect.
* `--port PORT` — (android only) override the default port (8123). **If you use this, pass the
  same port to `Inspector.start(port: PORT)` in your app.**
* `--serial SERIAL` — (android only) target a specific device. Run `adb devices` to list serials.
* `--pid PID` — (ios only) pick a specific instance when several inspectors are discovered.
* `--json` — print the result as one JSON document (with `--no-browser`, for scripts).
* `--no-browser` — don't open a browser; just print the URL.

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

## Scripting / headless use

The server speaks JSON-RPC 2.0 over plain HTTP (`POST /rpc`) in addition to
the browser's WebSocket — so `curl` (or any HTTP client, LLM agents
included) can inspect databases without a browser:

```bash
# Discover what the server can do:
curl -s 127.0.0.1:8123/rpc -d \
  '{"jsonrpc":"2.0","id":1,"method":"server.info"}'

# List databases, then query one:
curl -s 127.0.0.1:8123/rpc -d \
  '{"jsonrpc":"2.0","id":1,"method":"db.list"}'
curl -s 127.0.0.1:8123/rpc -d \
  '{"jsonrpc":"2.0","id":2,"method":"db.query",
    "params":{"db":"main","sql":"SELECT * FROM users LIMIT 5"}}'
```

Notes: protocol-level errors come back as JSON-RPC error objects with HTTP
200. Batching and notifications (id-less requests) are rejected — they need
the WebSocket's push channel, as do `db.subscribe`/`db.unsubscribe`
(error `-32010`).

You can also inspect a local SQLite file with no app attached:

```bash
sqlite_spect serve /path/to/db.sqlite --json
# stdout: {"url":"http://127.0.0.1:8123/","port":8123,"databases":["db"],...}
```

`serve` binds to `127.0.0.1` by default; pass `--host 0.0.0.0` to expose it
to the network (debug tooling only — anyone reachable can read and write
the database).

### JSON-RPC method reference

| Method | Params | Notes |
|---|---|---|
| `server.info` | — | version, platform, transports, full method list |
| `db.list` | — | registered database ids |
| `db.schema` | `db` | tables/indexes/triggers/views |
| `db.tableInfo` | `db`, `table` | columns |
| `db.query` | `db`, `sql`, `params?`, `limit?`, `offset?` | read-only `SELECT` |
| `db.execute` | `db`, `sql`, `params?` | arbitrary statement |
| `db.tableRows` | `db`, `table`, `limit?`, `offset?` | paged rows |
| `db.updateRow` | `db`, `table`, `rowKey`, `updates` | |
| `db.insertRow` | `db`, `table`, `values` | |
| `db.deleteRow` | `db`, `table`, `rowKey` | |
| `db.clearTable` | `db`, `table`, `confirm` | |
| `db.subscribe` / `db.unsubscribe` | see above | WebSocket only |
| `probe.tail` / `probe.untail` | — | stubbed (not yet implemented) |

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
