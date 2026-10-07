## 0.3.0

* Headless inspection and iOS support.
* Inspect without the browser: JSON-RPC over plain HTTP (POST /rpc); call server.info to see everything the server supports.
* iOS: Inspector.start no longer crashes, and attach --platform ios finds inspectors on the local network (same Wi-Fi required).
* Breaking: the url and list CLI commands are removed — attach --no-browser prints the URL.
* Breaking: serve listens on localhost only by default — pass --host 0.0.0.0 to expose it.
* --json machine-readable output for serve and attach.

## 0.2.1

* Data editing in the inspector UI is temporarily disabled (work in progress) while a bug is investigated; the SQL Runner still runs arbitrary statements and now warns that write statements may corrupt your database.

## 0.2.0

* No code changes — validates the automated release pipeline (GitHub Release + pub.dev OIDC publishing).

## 0.1.0

* Initial public release: in-app SQLite inspector for Flutter (debug/staging
  only) — browse, query, and edit SQLite databases live from a browser.
* Migrates the Android module to Flutter's built-in Kotlin (AGP 9+); the
  plugin no longer applies the legacy Kotlin Gradle Plugin.
* Raises the minimum supported SDK to Flutter 3.44 / Dart 3.12.
