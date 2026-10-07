pub mod adb;
pub mod banner;
pub mod db;
pub mod mdns;
pub mod probe;
pub mod rpc;
pub mod server;

pub const DEFAULT_PORT: u16 = 8123;

/// Engine version reported by `server.info` and the CLI's `--version`.
/// Release builds stamp SQLITE_SPECT_CLI_VERSION=<tag> (release.yml); local
/// builds fall back to the crate version, which is versioned independently
/// of the pub package.
pub const VERSION: &str = match option_env!("SQLITE_SPECT_CLI_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

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

