//! C ABI over the sqlite_spect engine, called from Dart via `dart:ffi`.
//!
//! - Every exported fn wraps its body in `std::panic::catch_unwind`.
//! - String arguments: null-terminated UTF-8, owned by the caller.
//! - String returns: `*mut c_char`, owned by the callee — caller MUST free via `inspector_free_string`.

use serde::Deserialize;
use serde_json::{json, Value};
use sqlite_spect_core::db::{DbConfig, DbRegistry};
use sqlite_spect_core::server::{bind, start_server_with_shutdown};
use sqlite_spect_core::{banner, host_platform, mdns};
use std::ffi::{c_char, CStr, CString};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use tokio::sync::oneshot;

// Error codes — negative to avoid collision with JSON-RPC codes.
const ERR_ALREADY_RUNNING: i32 = -100;
const ERR_MALFORMED_JSON: i32 = -101;
const ERR_INVALID_CONFIG: i32 = -102;
const ERR_DB_OPEN_FAILED: i32 = -103;
const ERR_BIND_FAILED: i32 = -104;
const ERR_PANIC: i32 = -999;

struct ServerHandle {
    shutdown_tx: oneshot::Sender<()>,
    thread: JoinHandle<()>,
    _mdns: Option<mdns::MdnsHandle>,
}

fn server_slot() -> &'static Mutex<Option<ServerHandle>> {
    static SLOT: OnceLock<Mutex<Option<ServerHandle>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

#[derive(Deserialize, Debug)]
struct StartConfig {
    port: u16,
    #[serde(default)]
    platform: Option<String>,
    databases: Vec<StartDb>,
    #[serde(default, rename = "logLevel")]
    log_level: Option<String>,
}

#[derive(Deserialize, Debug)]
struct StartDb {
    id: String,
    path: String,
    #[serde(default, rename = "readOnly")]
    read_only: bool,
}

// -----------------------------------------------------------------------------
// Exports
// -----------------------------------------------------------------------------

/// Start the inspector server.
/// Returns `{"ok": true, "port": N}` or `{"ok": false, "error": {"code": N, "message": "..."}}`.
#[no_mangle]
pub extern "C" fn inspector_start(config_json: *const c_char) -> *mut c_char {
    let response = std::panic::catch_unwind(|| start_inner(config_json))
        .unwrap_or_else(|_| json!({"ok": false, "error": {"code": ERR_PANIC, "message": "panic in FFI (start)"}}));
    to_c_string(&response)
}

/// Stop the inspector server. Idempotent — no-op if not running. Always
/// returns `{"ok": true}`.
#[no_mangle]
pub extern "C" fn inspector_stop() -> *mut c_char {
    let response = std::panic::catch_unwind(stop_inner)
        .unwrap_or_else(|_| json!({"ok": true}));
    to_c_string(&response)
}

/// Record a query executed by the app's own SQLite client. Fire-and-forget. No-op today.
#[no_mangle]
pub extern "C" fn inspector_record_query(_payload_json: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        if server_slot().lock().ok().map(|g| g.is_none()).unwrap_or(true) {
            return;
        }
        // TODO: forward payload to probe channel
    });
}

/// Free a `*mut c_char` previously returned by any inspector_* fn. Passing
/// null is safe.
#[no_mangle]
pub extern "C" fn inspector_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: contract requires callers only pass pointers we handed them
    // via CString::into_raw.
    let _ = std::panic::catch_unwind(|| unsafe {
        let _ = CString::from_raw(s);
    });
}

// -----------------------------------------------------------------------------
// Implementation
// -----------------------------------------------------------------------------

fn start_inner(config_json: *const c_char) -> Value {
    if config_json.is_null() {
        return err(ERR_MALFORMED_JSON, "config_json is null");
    }

    // SAFETY: caller must pass a valid null-terminated UTF-8 pointer.
    let json_str = match unsafe { CStr::from_ptr(config_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return err(ERR_MALFORMED_JSON, "config_json is not valid UTF-8"),
    };

    let cfg: StartConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(e) => return err(ERR_MALFORMED_JSON, &format!("config JSON invalid: {}", e)),
    };

    if let Err(msg) = validate_config(&cfg) {
        return err(ERR_INVALID_CONFIG, &msg);
    }

    init_logging(cfg.log_level.as_deref());

    // Hold the lock for the whole check+populate so a concurrent call can't sneak in.
    let mut slot = match server_slot().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if slot.is_some() {
        return err(ERR_ALREADY_RUNNING, "inspector server is already running");
    }

    let db_configs: Vec<DbConfig> = cfg
        .databases
        .iter()
        .map(|d| DbConfig {
            id: d.id.clone(),
            path: d.path.clone(),
            read_only: d.read_only,
        })
        .collect();

    let registry = match DbRegistry::open(db_configs) {
        Ok(r) => r,
        Err(e) => {
            return err(
                ERR_DB_OPEN_FAILED,
                &format!("failed to open databases: {:?}", e),
            )
        }
    };

    tracing::debug!(requested_port = cfg.port, "attempting to bind port");
    let (listener, actual_port) = match bind(cfg.port) {
        Ok(x) => {
            tracing::debug!(actual_port = x.1, "port bound successfully");
            x
        }
        Err(e) => return err(ERR_BIND_FAILED, &format!("bind failed: {}", e)),
    };

    tracing::info!("sqlite_spect at http://127.0.0.1:{}", actual_port);

    let db_ids = registry.db_ids();
    banner::emit(&db_ids);

    let platform = cfg.platform.as_deref().unwrap_or(host_platform());
    let mdns_handle = mdns::register(actual_port, platform, &db_ids);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("sqlite_spect".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "failed to build server runtime");
                    return;
                }
            };
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.block_on(start_server_with_shutdown(
                    listener,
                    registry,
                    shutdown_rx,
                ));
            }));
            tracing::debug!("server thread exiting");
        })
        .expect("spawn server thread");

    *slot = Some(ServerHandle {
        shutdown_tx,
        thread,
        _mdns: mdns_handle,
    });

    json!({ "ok": true, "port": actual_port })
}

fn stop_inner() -> Value {
    let handle_opt = {
        let mut slot = match server_slot().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.take()
    };

    let Some(handle) = handle_opt else {
        return json!({ "ok": true });
    };

    let _ = handle.shutdown_tx.send(());

    let joined = join_with_timeout(handle.thread, std::time::Duration::from_secs(2));
    if !joined {
        tracing::warn!("server thread did not join within 2s; abandoning");
    }
    // _mdns drops here → mDNS unregistered.

    json!({ "ok": true })
}

fn join_with_timeout(handle: JoinHandle<()>, timeout: std::time::Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    rx.recv_timeout(timeout).is_ok()
}

fn validate_config(cfg: &StartConfig) -> Result<(), String> {
    if cfg.databases.is_empty() {
        return Err("databases must not be empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    for db in &cfg.databases {
        if db.id.trim().is_empty() {
            return Err("database id must be non-empty".into());
        }
        if !db
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "database id '{}' contains invalid characters (allowed: A-Z, a-z, 0-9, _, -)",
                db.id
            ));
        }
        if !seen.insert(&db.id) {
            return Err(format!("duplicate database id: {}", db.id));
        }
        if db.path.trim().is_empty() {
            return Err(format!("database '{}' has empty path", db.id));
        }
    }
    Ok(())
}

static LOG_INIT: OnceLock<()> = OnceLock::new();

fn init_logging(log_level: Option<&str>) {
    LOG_INIT.get_or_init(|| {
        install_platform_logger(log_level);
    });
}

#[cfg(target_os = "android")]
fn install_platform_logger(log_level: Option<&str>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = build_filter(log_level);
    // paranoid_android's `init()` doesn't accept a filter, so compose the
    // layer + filter through the registry instead. The tag is what appears
    // in `adb logcat -s sqlite_spect`.
    let layer = paranoid_android::layer("sqlite_spect");
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}

#[cfg(target_os = "ios")]
fn install_platform_logger(log_level: Option<&str>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = build_filter(log_level);
    let layer = tracing_oslog::OsLogger::new("sqlite_spect", "default");
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn install_platform_logger(log_level: Option<&str>) {
    let filter = build_filter(log_level);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn build_filter(log_level: Option<&str>) -> tracing_subscriber::EnvFilter {
    // Precedence: explicit config > RUST_LOG env > "info". `mdns_sd=off`
    // is appended so the crate's internal chatter (unregister replies,
    // multicast resends on cellular / IPv6 interfaces, etc.) doesn't
    // pollute `flutter logs` / logcat / Xcode console. Our own mdns
    // wrapper still emits a warn on register failure — that we keep.
    if let Some(level) = log_level {
        if !level.is_empty() {
            let with_mdns_off = format!("{},mdns_sd=off", level);
            if let Ok(f) = tracing_subscriber::EnvFilter::try_new(&with_mdns_off) {
                return f;
            }
            tracing::warn!(level, "unrecognized logLevel; falling back to RUST_LOG / info");
        }
    }
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,mdns_sd=off"))
}

// -----------------------------------------------------------------------------
// JSON helpers
// -----------------------------------------------------------------------------
fn err(code: i32, message: &str) -> Value {
    json!({
        "ok": false,
        "error": { "code": code, "message": message }
    })
}

fn to_c_string(v: &Value) -> *mut c_char {
    let s = v.to_string();
    // CString::new only fails on interior nulls — a JSON string produced by
    // serde_json can't contain one, so this unwrap is safe.
    CString::new(s).unwrap().into_raw()
}

// -----------------------------------------------------------------------------
// Tests — exercise the extern fns directly from Rust
// -----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    // Helper: turn a &str into a *const c_char callable by the extern fns.
    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    // Helper: consume the return pointer, decode it as JSON, and free it via
    // the public API — same lifecycle a Dart caller would use.
    fn take_response(ptr: *mut c_char) -> Value {
        assert!(!ptr.is_null());
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        inspector_free_string(ptr);
        serde_json::from_str(&s).unwrap()
    }

    fn temp_db() -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        let c = rusqlite::Connection::open(f.path()).unwrap();
        c.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        f
    }

    // The FFI keeps one global server slot, so tests that start/stop the
    // inspector must not overlap — hold this guard for the whole test body.
    static SUITE_LOCK: Mutex<()> = Mutex::new(());

    fn suite_lock() -> std::sync::MutexGuard<'static, ()> {
        SUITE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn ensure_stopped() {
        // Best-effort cleanup between tests. inspector_stop is idempotent.
        let ptr = inspector_stop();
        inspector_free_string(ptr);
    }

    #[test]
    fn malformed_json_returns_101() {
        let _suite = suite_lock();
        ensure_stopped();
        let c = cstr("not json");
        let resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["error"]["code"], ERR_MALFORMED_JSON);
    }

    #[test]
    fn empty_databases_returns_102() {
        let _suite = suite_lock();
        ensure_stopped();
        let payload = json!({"port": 0, "databases": []});
        let c = cstr(&payload.to_string());
        let resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["error"]["code"], ERR_INVALID_CONFIG);
    }

    #[test]
    fn nonexistent_db_returns_103() {
        let _suite = suite_lock();
        ensure_stopped();
        let payload = json!({
            "port": 0,
            "databases": [{"id": "x", "path": "/tmp/definitely-not-a-real-path-xyz-99999.db"}]
        });
        let c = cstr(&payload.to_string());
        let resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["error"]["code"], ERR_DB_OPEN_FAILED);
    }

    #[test]
    fn full_lifecycle_start_then_stop_releases_port() {
        let _suite = suite_lock();
        ensure_stopped();
        let db = temp_db();
        let payload = json!({
            "port": 0,
            "platform": "macos",
            "databases": [{"id": "t", "path": db.path().to_str().unwrap()}]
        });
        let c = cstr(&payload.to_string());
        let start_resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(start_resp["ok"], true, "start failed: {}", start_resp);
        let port = start_resp["port"].as_u64().unwrap();
        assert!(port > 0);

        // Second start on top of a running instance → -100.
        let dupe = take_response(inspector_start(c.as_ptr()));
        assert_eq!(dupe["error"]["code"], ERR_ALREADY_RUNNING);

        // Stop is idempotent + always succeeds.
        let stop_resp = take_response(inspector_stop());
        assert_eq!(stop_resp["ok"], true);
        let stop_again = take_response(inspector_stop());
        assert_eq!(stop_again["ok"], true);

        // After stop the port is free — re-bind proves it.
        // Give the OS a moment; TCP TIME_WAIT can hold briefly.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (_l, again) = bind(port as u16).expect("port should be free");
        assert_eq!(again, port as u16);
    }

    #[test]
    fn record_query_before_start_is_noop() {
        let _suite = suite_lock();
        ensure_stopped();
        let payload = json!({
            "dbId": "x",
            "sql": "SELECT 1",
            "params": [],
            "durationMicros": 0,
            "rowsAffected": 0
        });
        let c = cstr(&payload.to_string());
        // Must not panic, must not allocate a returned string (void return).
        inspector_record_query(c.as_ptr());
    }

    #[test]
    fn free_null_is_safe() {
        inspector_free_string(std::ptr::null_mut());
    }

    #[test]
    fn duplicate_db_ids_rejected() {
        let _suite = suite_lock();
        ensure_stopped();
        let db = temp_db();
        let payload = json!({
            "port": 0,

            "databases": [
                {"id": "a", "path": db.path().to_str().unwrap()},
                {"id": "a", "path": db.path().to_str().unwrap()}
            ]
        });
        let c = cstr(&payload.to_string());
        let resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(resp["error"]["code"], ERR_INVALID_CONFIG);
    }

    #[test]
    fn invalid_db_id_characters_rejected() {
        let _suite = suite_lock();
        ensure_stopped();
        let db = temp_db();
        let payload = json!({
            "port": 0,

            "databases": [{"id": "bad id!", "path": db.path().to_str().unwrap()}]
        });
        let c = cstr(&payload.to_string());
        let resp = take_response(inspector_start(c.as_ptr()));
        assert_eq!(resp["error"]["code"], ERR_INVALID_CONFIG);
    }
}
