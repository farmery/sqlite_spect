/// Error surfaced from the native FFI layer. `code` matches the negative
/// error codes in spec §5.3 (e.g. -100 already-running, -104 bind-failed).
class InspectorException implements Exception {
  final int code;
  final String message;

  InspectorException(this.code, this.message);

  @override
  String toString() => 'InspectorException($code): $message';
}
