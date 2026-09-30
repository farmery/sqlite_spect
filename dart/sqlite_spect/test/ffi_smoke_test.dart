// End-to-end FFI smoke test on macOS: loads the freshly-built cdylib from
// the workspace's target/release directory, spins up the inspector, hits the
// HTTP endpoint over loopback, and shuts down.
//
// Skipped on non-macOS hosts. Requires `cargo build --release -p
// sqlite_spect_ffi` to have been run at least once.

@Tags(['integration'])
library;

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';
import 'package:flutter_test/flutter_test.dart';

// Direct FFI typedefs — bypass the loader in ffi_bindings.dart because it
// looks for the dylib in the app bundle path, which doesn't exist in a
// vanilla `dart test` invocation.
typedef _StartNative = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StartDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StopNative = Pointer<Utf8> Function();
typedef _StopDart = Pointer<Utf8> Function();
typedef _FreeNative = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

void main() {
  if (!Platform.isMacOS) {
    return;
  }

  final workspaceRoot = Directory.current.path.contains('dart/sqlite_spect')
      ? Directory.current.parent.parent
      : Directory.current;
  final dylibPath =
      '${workspaceRoot.path}/target/release/libsqlite_spect_ffi.dylib';

  test('cdylib exists', skip: !File(dylibPath).existsSync(), () {
    expect(File(dylibPath).existsSync(), isTrue,
        reason: 'run: cargo build --release -p sqlite_spect_ffi');
  });

  test('start → HTTP GET / → stop against real cdylib', () async {
    if (!File(dylibPath).existsSync()) {
      markTestSkipped('dylib not built — skipping');
      return;
    }

    final lib = DynamicLibrary.open(dylibPath);
    final start = lib
        .lookup<NativeFunction<_StartNative>>('inspector_start')
        .asFunction<_StartDart>();
    final stop = lib
        .lookup<NativeFunction<_StopNative>>('inspector_stop')
        .asFunction<_StopDart>();
    final free = lib
        .lookup<NativeFunction<_FreeNative>>('inspector_free_string')
        .asFunction<_FreeDart>();

    // Create a temp sqlite file the engine can open.
    final tmpDir = Directory.systemTemp.createTempSync('sqlite_spect_ffi');
    final dbFile = File('${tmpDir.path}/test.db');
    // Empty file is a valid sqlite db (rusqlite creates on open with
    // SQLITE_OPEN_URI + read/write).
    dbFile.createSync();

    final config = jsonEncode({
      'port': 0,
      'platform': 'macos',
      'databases': [
        {'id': 'main', 'path': dbFile.path},
      ],
    });

    final cfgPtr = config.toNativeUtf8();
    late Map<String, dynamic> startResp;
    try {
      final resultPtr = start(cfgPtr);
      try {
        startResp = jsonDecode(resultPtr.toDartString()) as Map<String, dynamic>;
      } finally {
        free(resultPtr);
      }
    } finally {
      calloc.free(cfgPtr);
    }

    expect(startResp['ok'], isTrue, reason: startResp.toString());
    final port = startResp['port'] as int;
    expect(port, greaterThan(0));

    // Hit HTTP over loopback — assets bundle is embedded, so `/` should
    // return HTML or a "Client bundle not embedded" 500 (both prove the
    // server bound and is serving).
    final client = HttpClient();
    try {
      final req = await client.getUrl(Uri.parse('http://127.0.0.1:$port/'));
      final resp = await req.close();
      // Either 200 (bundle embedded) or 500 (bundle missing) — both prove
      // the axum server is answering.
      expect([HttpStatus.ok, HttpStatus.internalServerError],
          contains(resp.statusCode));
      await resp.drain<void>();
    } finally {
      client.close();
    }

    // Stop and confirm the port frees.
    final stopPtr = stop();
    try {
      final stopResp = jsonDecode(stopPtr.toDartString());
      expect(stopResp['ok'], isTrue);
    } finally {
      free(stopPtr);
    }

    tmpDir.deleteSync(recursive: true);
  });
}
