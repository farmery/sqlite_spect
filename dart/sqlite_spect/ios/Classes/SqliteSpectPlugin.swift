// Empty plugin shell for Flutter iOS plugin discovery. All Dart↔native
// traffic goes through dart:ffi against the vendored xcframework — this
// Swift file exists only so the plugin registers.

import Flutter

public class SqliteSpectPlugin: NSObject, FlutterPlugin {
  public static func register(with registrar: FlutterPluginRegistrar) {
    // No method channels — this is an FFI plugin.
  }
}
