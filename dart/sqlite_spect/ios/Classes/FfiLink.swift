// Force-links the Rust static library into the app binary.
//
// dart:ffi resolves the inspector_* symbols from the process image at
// runtime (dlsym), so no native code references them — and an unreferenced
// static archive is dead-stripped by the linker, leaving dlsym nothing to
// find. Calling through the real symbols here (from register(), which
// GeneratedPluginRegistrant always invokes) keeps libsqlite_spect_ffi.a's
// members in the binary.
//
// inspector_stop is the entry point called unconditionally: it is
// documented idempotent — a no-op returning {"ok": true} when no server is
// running — so calling it during plugin registration is harmless. The
// other symbols sit behind a runtime-opaque condition that never fires;
// the compiler can't prove that, so it must keep the references.

import Foundation

@_silgen_name("inspector_start")
private func spect_inspector_start(_ config: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?

@_silgen_name("inspector_stop")
private func spect_inspector_stop() -> UnsafeMutablePointer<CChar>?

@_silgen_name("inspector_record_query")
private func spect_inspector_record_query(_ payload: UnsafePointer<CChar>)

@_silgen_name("inspector_free_string")
private func spect_inspector_free_string(_ s: UnsafeMutablePointer<CChar>)

@inline(never)
func sqliteSpectForceLinkFfi() {
    if let result = spect_inspector_stop() {
        spect_inspector_free_string(result)
    }
    if ProcessInfo.processInfo.environment["SQLITE_SPECT_LINK_PROBE"] != nil {
        if let result = spect_inspector_start("") {
            spect_inspector_free_string(result)
        }
        spect_inspector_record_query("")
    }
}
