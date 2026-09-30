// mDNS advertisement for the inspector server.

use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::time::Duration;

// RFC 6335 caps the service name label at 15 bytes (excluding the leading
// underscore). `sqlite-insp` fits and is still uniquely ours; `attach`
// browses the same string on the client side.
pub const SERVICE_TYPE: &str = "_sqlite-spect._tcp.local.";

/// Handle to an active mDNS registration. Dropping this unregisters the
/// service and shuts down the daemon thread cleanly.
pub struct MdnsHandle {
    fullname: String,
    // Option so Drop can move the daemon out and call shutdown() — which
    // needs `self` by value.
    daemon: Option<ServiceDaemon>,
}

impl MdnsHandle {
    /// Explicitly unregister the service. The daemon keeps running until the
    /// handle is dropped. `Drop` calls this automatically.
    pub fn unregister(&self) {
        if let Some(d) = &self.daemon {
            match d.unregister(&self.fullname) {
                Ok(rx) => {
                    // Drain the response so the daemon doesn't log
                    // "unregister: failed to send response: sending on a
                    // closed channel" when its worker tries to reply.
                    let _ = rx.recv_timeout(Duration::from_millis(200));
                }
                Err(e) => tracing::debug!(error = %e, "mdns unregister failed"),
            }
        }
    }
}

impl Drop for MdnsHandle {
    fn drop(&mut self) {
        self.unregister();
        // Shut the daemon thread down and drain its exit response —
        // otherwise its internal thread survives and, when finally
        // dropped, logs "exit: failed to send response of shutdown" at
        // ERROR because our side already dropped the receiver.
        if let Some(daemon) = self.daemon.take() {
            if let Ok(rx) = daemon.shutdown() {
                let _ = rx.recv_timeout(Duration::from_millis(200));
            }
        }
    }
}

/// Register the inspector on the local network. Returns `None` if mDNS is unavailable.
pub fn register(
    port: u16,
    platform: &str,
    db_ids: &[String],
) -> Option<MdnsHandle> {
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mdns daemon start failed — discovery via mDNS is unavailable");
            return None;
        }
    };

    let pid = std::process::id();
    let hostname = hostname_or_default();
    // Bonjour requires the host string to end with '.local.' — normalise.
    let host_local = format!("{}.local.", hostname.trim_end_matches('.'));

    let instance = format!("sqlite-inspector-{}", pid);
    let mut props: HashMap<String, String> = HashMap::new();
    props.insert("pid".into(), pid.to_string());
    props.insert("platform".into(), platform.into());
    props.insert("dbs".into(), db_ids.join(","));

    // Empty IP set → let the daemon fill it in via enable_addr_auto below.
    let ips: &[std::net::IpAddr] = &[];
    let service = match ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_local,
        ips,
        port,
        Some(props),
    ) {
        Ok(s) => s.enable_addr_auto(),
        Err(e) => {
            tracing::warn!(error = %e, "mdns ServiceInfo build failed");
            return None;
        }
    };

    let fullname = service.get_fullname().to_string();

    if let Err(e) = daemon.register(service) {
        tracing::warn!(error = %e, "mdns register failed — discovery via mDNS is unavailable");
        return None;
    }

    tracing::debug!(fullname = %fullname, port, "mdns registered");

    Some(MdnsHandle {
        fullname,
        daemon: Some(daemon),
    })
}

fn hostname_or_default() -> String {
    // hostname isn't strictly critical — the client resolves via IPs from
    // the ServiceInfo's address set. But populating it correctly aids
    // diagnostics in tools like `dns-sd -B`.
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "sqlite-inspector".to_string())
}
