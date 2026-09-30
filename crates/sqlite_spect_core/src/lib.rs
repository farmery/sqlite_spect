pub mod adb;
pub mod banner;
pub mod db;
pub mod mdns;
pub mod probe;
pub mod rpc;
pub mod server;

pub const DEFAULT_PORT: u16 = 8123;

/// A stable, short host-platform name used in banners, TXT records, and the
/// state file. Matches the `platform` enum in the FFI config JSON.
pub fn host_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    { "macos" }
    #[cfg(target_os = "linux")]
    { "linux" }
    #[cfg(target_os = "windows")]
    { "windows" }
    #[cfg(target_os = "ios")]
    { "ios" }
    #[cfg(target_os = "android")]
    { "android" }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "ios",
        target_os = "android",
    )))]
    { "unknown" }
}

