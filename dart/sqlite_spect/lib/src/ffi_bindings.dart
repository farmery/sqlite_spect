// Hand-written dart:ffi bindings for the sqlite_spect_ffi cdylib. See
// spec §5 for the C ABI and §10.4 for the DynamicLibrary loader rationale.

import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

typedef _InspectorStartNative = Pointer<Utf8> Function(Pointer<Utf8>);
typedef InspectorStartDart = Pointer<Utf8> Function(Pointer<Utf8>);

typedef _InspectorStopNative = Pointer<Utf8> Function();
typedef InspectorStopDart = Pointer<Utf8> Function();

typedef _InspectorRecordQueryNative = Void Function(Pointer<Utf8>);
typedef InspectorRecordQueryDart = void Function(Pointer<Utf8>);

typedef _InspectorFreeStringNative = Void Function(Pointer<Utf8>);
typedef InspectorFreeStringDart = void Function(Pointer<Utf8>);

class InspectorFfi {
  final InspectorStartDart start;
  final InspectorStopDart stop;
  final InspectorRecordQueryDart recordQuery;
  final InspectorFreeStringDart freeString;

  InspectorFfi._({
    required this.start,
    required this.stop,
    required this.recordQuery,
    required this.freeString,
  });

  static InspectorFfi? _cached;

  /// Resolve exports from the platform-appropriate shared library exactly
  /// once. Throws [UnsupportedError] on platforms we don't ship binaries
  /// for.
  static InspectorFfi load() {
    final cached = _cached;
    if (cached != null) return cached;

    final lib = _openLib();
    final fresh = InspectorFfi._(
      start: lib
          .lookup<NativeFunction<_InspectorStartNative>>('inspector_start')
          .asFunction<InspectorStartDart>(),
      stop: lib
          .lookup<NativeFunction<_InspectorStopNative>>('inspector_stop')
          .asFunction<InspectorStopDart>(),
      recordQuery: lib
          .lookup<NativeFunction<_InspectorRecordQueryNative>>(
              'inspector_record_query')
          .asFunction<InspectorRecordQueryDart>(),
      freeString: lib
          .lookup<NativeFunction<_InspectorFreeStringNative>>(
              'inspector_free_string')
          .asFunction<InspectorFreeStringDart>(),
    );
    _cached = fresh;
    return fresh;
  }

  static DynamicLibrary _openLib() {
    if (Platform.isAndroid) {
      return DynamicLibrary.open('libsqlite_spect_ffi.so');
    }
    if (Platform.isIOS) {
      // iOS statically links the .a into the app binary — resolve via the
      // process image itself.
      return DynamicLibrary.process();
    }
    // if (Platform.isMacOS) {
    //   return DynamicLibrary.open('libsqlite_spect_ffi.dylib');
    // }
    // if (Platform.isLinux) {
    //   return DynamicLibrary.open('libsqlite_spect_ffi.so');
    // }
    // if (Platform.isWindows) {
    //   return DynamicLibrary.open('sqlite_spect_ffi.dll');
    // }
    throw UnsupportedError(
      'sqlite_spect: platform ${Platform.operatingSystem} is not supported',
    );
  }
}
