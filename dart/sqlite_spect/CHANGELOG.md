## Unreleased

* Editing is a work in progress: data editing in the inspector UI (cell edits,
  add/delete row, clear table) is temporarily disabled while a bug is
  investigated. The SQL Runner still runs arbitrary statements and now warns
  that write statements may corrupt your database.

## 0.2.0

* No code changes — validates the automated release pipeline (GitHub Release + pub.dev OIDC publishing).

## 0.1.0

* Initial public release: in-app SQLite inspector for Flutter (debug/staging
  only) — browse, query, and edit SQLite databases live from a browser.
* Migrates the Android module to Flutter's built-in Kotlin (AGP 9+); the
  plugin no longer applies the legacy Kotlin Gradle Plugin.
* Raises the minimum supported SDK to Flutter 3.44 / Dart 3.12.
