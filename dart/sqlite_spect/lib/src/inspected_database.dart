/// Descriptor for one SQLite database file the inspector should expose to
/// the browser client.
///
/// [id] appears in URLs and RPC calls; must be `^[a-zA-Z0-9_-]+$` — see spec
/// §8.1. [path] is the on-disk sqlite file path (absolute paths are safest
/// on mobile). [readOnly] forces the engine to open with SQLITE_OPEN_READ_ONLY,
/// which also removes the "write" capability from the browser client.
class InspectedDatabase {
  final String id;
  final String path;
  final bool readOnly;

  const InspectedDatabase({
    required this.id,
    required this.path,
    this.readOnly = false,
  });

  Map<String, dynamic> toJson() => {
        'id': id,
        'path': path,
        'readOnly': readOnly,
      };
}
