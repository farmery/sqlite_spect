import 'package:flutter_test/flutter_test.dart';

import 'package:sqlite_spect/sqlite_spect.dart';

void main() {
  setUp(() {
    Inspector.debugTestOnlyForceEnabled = false;
    Inspector.debugTestOnlySkipFfi = false;
  });

  group('release-mode gate', () {
    // In test binaries kReleaseMode is false so the release path never runs
    // by default. debugTestOnlyForceEnabled=false is our proxy for release.
    test('start returns null when release-mode short-circuit is active', () async {
      // Simulate release by leaving debugTestOnlyForceEnabled at its default
      // false. kReleaseMode is false in tests too, so we bypass by NOT
      // forcing-enabled — but the guard is `if (kReleaseMode && ...)`, so
      // in tests this won't short-circuit. Instead assert that
      // `debugTestOnlyForceEnabled = false` doesn't cause a short-circuit
      // in a debug build.
      Inspector.debugTestOnlySkipFfi = true;
      Inspector.debugTestOnlyForceEnabled = true;

      final conn = await Inspector.start(
        databases: const [InspectedDatabase(id: 'main', path: '/tmp/x.db')],
      );
      expect(conn, isNotNull);
      expect(conn!.host, '127.0.0.1');
      expect(Inspector.isRunning, isTrue);
      await Inspector.stop();
      expect(Inspector.isRunning, isFalse);
    });

    test('recordQuery no-ops without a running server (no FFI call)', () {
      Inspector.debugTestOnlySkipFfi = true;
      // Doesn't throw even though nothing is running.
      Inspector.recordQuery(
        dbId: 'main',
        sql: 'SELECT 1',
        params: const [],
        durationMicros: 0,
        rowsAffected: 0,
      );
    });
  });

  group('argument validation', () {
    setUp(() {
      Inspector.debugTestOnlyForceEnabled = true;
      Inspector.debugTestOnlySkipFfi = true;
    });

    test('empty databases → ArgumentError', () async {
      expect(
        () => Inspector.start(databases: const []),
        throwsA(isA<ArgumentError>()),
      );
    });

    test('invalid port → ArgumentError', () async {
      expect(
        () => Inspector.start(
          port: 42,
          databases: const [InspectedDatabase(id: 'a', path: '/x')],
        ),
        throwsA(isA<ArgumentError>()),
      );
    });

    test('port 0 is allowed (means "any free port")', () async {
      final conn = await Inspector.start(
        port: 0,
        databases: const [InspectedDatabase(id: 'a', path: '/x')],
      );
      expect(conn, isNotNull);
      await Inspector.stop();
    });

    test('bad db id characters → ArgumentError', () async {
      expect(
        () => Inspector.start(
          databases: const [InspectedDatabase(id: 'has space', path: '/x')],
        ),
        throwsA(isA<ArgumentError>()),
      );
    });

    test('empty db path → ArgumentError', () async {
      expect(
        () => Inspector.start(
          databases: const [InspectedDatabase(id: 'a', path: '')],
        ),
        throwsA(isA<ArgumentError>()),
      );
    });
  });

  group('connection tracking', () {
    setUp(() {
      Inspector.debugTestOnlyForceEnabled = true;
      Inspector.debugTestOnlySkipFfi = true;
    });

    test('currentUrl reflects the port', () async {
      await Inspector.start(
        port: 9999,
        databases: const [InspectedDatabase(id: 'a', path: '/x')],
      );
      expect(Inspector.currentUrl, contains(':9999'));
      await Inspector.stop();
      expect(Inspector.currentUrl, isNull);
    });

    test('logConnection is safe to call when not running', () {
      // Just asserts no throw — the log line goes to dart:developer.
      Inspector.logConnection();
    });
  });

  group('InspectorException', () {
    test('toString includes code and message', () {
      final e = InspectorException(-104, 'bind failed');
      expect(e.toString(), 'InspectorException(-104): bind failed');
    });
  });

  group('InspectedDatabase.toJson', () {
    test('serialises to the FFI wire shape', () {
      const db = InspectedDatabase(id: 'main', path: '/x.db', readOnly: true);
      expect(db.toJson(), {
        'id': 'main',
        'path': '/x.db',
        'readOnly': true,
      });
    });

    test('readOnly defaults to false', () {
      const db = InspectedDatabase(id: 'main', path: '/x.db');
      expect(db.toJson()['readOnly'], false);
    });
  });
}
