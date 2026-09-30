import 'dart:convert';
import 'dart:developer' as developer;

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';

import 'connection.dart';
import 'exception.dart';
import 'ffi_bindings.dart';
import 'inspected_database.dart';

bool _isReleaseGate() => kReleaseMode;

class Inspector {
  Inspector._();

  static InspectorConnection? _current;
  static String? _platformOverride;

  // ---- Test hooks -----------------------------------------------------------

  @visibleForTesting
  static bool debugTestOnlyForceEnabled = false;

  @visibleForTesting
  static bool debugTestOnlySkipFfi = false;

  static bool get isRunning {
    if (_isReleaseGate() && !debugTestOnlyForceEnabled) return false;
    return _current != null;
  }

  static String? get currentUrl => _current?.url;

  static InspectorConnection? get currentConnection => _current;

  /// Start the inspector server. In release mode this is a no-op and returns `null`.
  ///
  /// Hot-reload safe: if a previous instance is still running, this call
  /// transparently stops it and re-starts with the new config.
  static Future<InspectorConnection?> start({
    int port = 8123,
    required List<InspectedDatabase> databases,
    String? logLevel,
  }) async {
    if (_isReleaseGate() && !debugTestOnlyForceEnabled) return null;

    _validateArgs(port: port, databases: databases);

    try {
      return await _startOnce(
        port: port,
        databases: databases,
        logLevel: logLevel,
      );
    } on InspectorException catch (e) {
      if (e.code == -100) {
        await stop();
        return await _startOnce(
          port: port,
          databases: databases,
          logLevel: logLevel,
        );
      }
      rethrow;
    }
  }

  /// Stop the inspector server. Idempotent — safe to call when not running.
  static Future<void> stop() async {
    if (_isReleaseGate() && !debugTestOnlyForceEnabled) return;

    _current = null;

    if (debugTestOnlySkipFfi) return;

    final ffi = InspectorFfi.load();
    final resultPtr = ffi.stop();
    try {
      final _ = jsonDecode(resultPtr.toDartString());
    } finally {
      ffi.freeString(resultPtr);
    }
  }

  /// Record a query executed by the app's own SQLite client. Fire-and-forget.
  /// May be called from any isolate.
  static void recordQuery({
    required String dbId,
    required String sql,
    required List<Object?> params,
    required int durationMicros,
    required int rowsAffected,
  }) {
    if (_isReleaseGate() && !debugTestOnlyForceEnabled) return;
    if (debugTestOnlySkipFfi) return;

    final payload = jsonEncode({
      'dbId': dbId,
      'sql': sql,
      'params': params,
      'durationMicros': durationMicros,
      'rowsAffected': rowsAffected,
    });
    final ffi = InspectorFfi.load();
    final ptr = payload.toNativeUtf8();
    try {
      ffi.recordQuery(ptr);
    } finally {
      calloc.free(ptr);
    }
  }

  /// Re-emit the connection banner to the log. Useful when the dev scrolled
  /// past the initial print.
  static void logConnection() {
    if (_isReleaseGate() && !debugTestOnlyForceEnabled) return;
    final c = _current;
    if (c == null) {
      developer.log(
        'sqlite_spect: not running (call Inspector.start first)',
        name: 'sqlite_spect',
      );
      return;
    }
    developer.log(
      '\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n'
      '  sqlite_spect\n'
      '    ${c.url}\n'
      '\n  Tip: run  sqlite_spect attach  on your Mac to auto-open.\n'
      '━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━',
      name: 'sqlite_spect',
    );
  }

  @visibleForTesting
  static void debugSetPlatformOverride(String? platform) {
    _platformOverride = platform;
  }

  // ---- Internals ------------------------------------------------------------

  static void _validateArgs({
    required int port,
    required List<InspectedDatabase> databases,
  }) {
    if (port != 0 && (port < 1024 || port > 65535)) {
      throw ArgumentError.value(
        port,
        'port',
        'must be 0 (any free) or 1024..65535',
      );
    }
    if (databases.isEmpty) {
      throw ArgumentError.value(databases, 'databases', 'must not be empty');
    }
    for (final db in databases) {
      if (db.id.isEmpty) {
        throw ArgumentError('database.id must not be empty');
      }
      if (!RegExp(r'^[A-Za-z0-9_-]+$').hasMatch(db.id)) {
        throw ArgumentError.value(
          db.id,
          'database.id',
          r'must match ^[A-Za-z0-9_-]+$',
        );
      }
      if (db.path.isEmpty) {
        throw ArgumentError('database.path must not be empty');
      }
    }
  }

  static Future<InspectorConnection> _startOnce({
    required int port,
    required List<InspectedDatabase> databases,
    String? logLevel,
  }) async {
    if (debugTestOnlySkipFfi) {
      final conn = InspectorConnection(
        host: '127.0.0.1',
        port: port == 0 ? 8123 : port,
      );
      _current = conn;
      return conn;
    }

    final config = <String, dynamic>{
      'port': port,
      if (_platformOverride != null) 'platform': _platformOverride,
      if (logLevel != null) 'logLevel': logLevel,
      'databases': databases.map((d) => d.toJson()).toList(),
      // TODO: 'authKey': authKey, when auth is implemented
    };
    final ffi = InspectorFfi.load();
    final cfgPtr = jsonEncode(config).toNativeUtf8();
    try {
      final resultPtr = ffi.start(cfgPtr);
      try {
        final decoded = jsonDecode(resultPtr.toDartString());
        if (decoded is! Map || decoded['ok'] != true) {
          final err = (decoded as Map?)?['error'] as Map?;
          final code = err?['code'] as int? ?? -1;
          final msg = err?['message'] as String? ?? 'unknown FFI error';
          throw InspectorException(code, msg);
        }
        final actualPort =
            (decoded['port'] as num?)?.toInt() ?? (port == 0 ? 8123 : port);
        final conn = InspectorConnection(host: '127.0.0.1', port: actualPort);
        _current = conn;
        return conn;
      } finally {
        ffi.freeString(resultPtr);
      }
    } finally {
      calloc.free(cfgPtr);
    }
  }
}
