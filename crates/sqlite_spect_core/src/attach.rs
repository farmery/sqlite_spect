// `sqlite_spect attach` — the CLI's single discovery surface.
//
// Two channels, selected by --platform:
//   android: adb forward to the device's loopback inspector, then open
//            http://127.0.0.1:<port> locally — adb is the tunnel, so the
//            device's IP is never needed.
//   ios:     mDNS network discovery. The inspector announces itself on the
//            LAN at startup (see mdns.rs) and we can't guess its IP, so we
//            browse for announcements. This finds ANY inspector on the
//            network — iOS devices on Wi-Fi, desktop `serve` instances,
//            other machines. The iOS simulator still needs simctl loopback
//            detection (TODO — spec §21).

use sqlite_spect_core::adb::AdbInfo;
use sqlite_spect_core::DEFAULT_PORT;
use std::net::IpAddr;
use std::process::exit;
use std::time::{Duration, Instant};

/// One inspector instance found via mDNS. `pid` and `platform` come from
/// the service's TXT records, address and port from resolution.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub url: String,
    pub port: u16,
    pub pid: u32,
    pub platform: String,
}

/// Per-instance accumulator while browsing. mDNS resolves one instance once
/// per interface/address family, so the same service fires multiple
/// `ServiceResolved` events with different address subsets — collect them
/// and pick the best address when the window closes.
struct MdnsAcc {
    pid: u32,
    platform: String,
    port: u16,
    ipv4: Option<IpAddr>,
    any: Option<IpAddr>,
}

/// Prefer IPv4: link-local IPv6 (fe80::…) makes awkward and often
/// unreachable URLs. IPv6 needs bracketing in URLs.
fn url_for(addr: IpAddr, port: u16) -> String {
    match addr {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

impl MdnsAcc {
    fn best_url(&self) -> String {
        url_for(
            self.ipv4.or(self.any).unwrap_or(IpAddr::from([127, 0, 0, 1])),
            self.port,
        )
    }
}

/// One-shot mDNS browse. Returns `None` when mDNS itself is unavailable
/// (blocked / no daemon) — callers treat that as "no hits this round".
fn try_discover_mdns(wait: Duration) -> Option<Vec<Discovered>> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let daemon = ServiceDaemon::new().ok()?;
    let receiver = daemon.browse(sqlite_spect_core::mdns::SERVICE_TYPE).ok()?;

    let deadline = Instant::now() + wait;
    let mut instances: std::collections::HashMap<String, MdnsAcc> =
        std::collections::HashMap::new();

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let props = info.get_properties();
                let pid = props
                    .get_property_val_str("pid")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let platform = props
                    .get_property_val_str("platform")
                    .unwrap_or("unknown")
                    .to_string();
                let port = info.get_port();

                let acc = instances
                    .entry(info.get_fullname().to_string())
                    .or_insert(MdnsAcc {
                        pid,
                        platform,
                        port,
                        ipv4: None,
                        any: None,
                    });
                for addr in info.get_addresses() {
                    if addr.is_ipv4() {
                        acc.ipv4.get_or_insert(*addr);
                    } else {
                        acc.any.get_or_insert(*addr);
                    }
                }
            }
            Ok(_other) => {}
            Err(_timeout_or_disc) => break,
        }
    }

    // Stop cleanly: shutdown() returns a receiver that we MUST drain,
    // otherwise the daemon logs "exit: failed to send response of shutdown:
    // sending on a closed channel" at ERROR when it can't hand back its
    // DaemonStatus. Consuming the response silences it at the source.
    if let Ok(rx) = daemon.shutdown() {
        let _ = rx.recv_timeout(Duration::from_millis(200));
    }
    Some(
        instances
            .into_values()
            .map(|acc| Discovered {
                url: acc.best_url(),
                port: acc.port,
                pid: acc.pid,
                platform: acc.platform,
            })
            .collect(),
    )
}

// ============================================================================
// attach
// ============================================================================

struct AttachArgs {
    port: u16,
    platform: String,
    serial: Option<String>,
    pid: Option<u32>,
    watch: bool,
    no_browser: bool,
    timeout_secs: u64,
    json: bool,
}

pub async fn cmd_attach(args: &[String]) {
    let parsed = match parse_attach_args(args) {
        Ok(p) => p,
        Err(msg) => super::usage_error(&msg),
    };
    if let Err(msg) = validate(&parsed) {
        super::usage_error(&msg);
    }

    match parsed.platform.as_str() {
        // parse_attach_args only lets "android"/"ios" through.
        "android" => cmd_attach_android(&parsed).await,
        _ => cmd_attach_ios(&parsed).await,
    }
}

/// Cross-flag rules, separate from parsing so they're unit-testable.
fn validate(a: &AttachArgs) -> Result<(), String> {
    if a.watch && a.json {
        // --watch streams progress; the JSON contract is "stdout is exactly
        // one document". NDJSON events are designed but not built yet.
        return Err("--json with --watch is not supported yet".into());
    }
    if a.watch && a.platform == "ios" {
        return Err("--watch is not supported with --platform ios yet".into());
    }
    Ok(())
}

fn parse_attach_args(args: &[String]) -> Result<AttachArgs, String> {
    let mut port: Option<u16> = None;
    let mut platform: Option<String> = None;
    let mut serial: Option<String> = None;
    let mut pid: Option<u32> = None;
    let mut watch = false;
    let mut no_browser = false;
    let mut timeout_secs: Option<u64> = None;
    let mut json = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => super::print_help(),
            "--port" => {
                i += 1;
                let value = super::take_value(args, i, "--port")?;
                port = Some(value.parse().map_err(|_| {
                    format!("invalid --port value: '{value}' (expected 1-65535)")
                })?);
            }
            "--platform" => {
                i += 1;
                let value = super::take_value(args, i, "--platform")?;
                if value != "android" && value != "ios" {
                    return Err(format!(
                        "unknown platform '{value}' (expected 'android' or 'ios')"
                    ));
                }
                platform = Some(value);
            }
            "--serial" => {
                i += 1;
                serial = Some(super::take_value(args, i, "--serial")?);
            }
            "--pid" => {
                i += 1;
                let value = super::take_value(args, i, "--pid")?;
                pid = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid --pid value: '{value}' (expected a process id)"))?,
                );
            }
            "--watch" => watch = true,
            "--no-browser" => no_browser = true,
            "--timeout" => {
                i += 1;
                let value = super::take_value(args, i, "--timeout")?;
                timeout_secs = Some(value.parse().map_err(|_| {
                    format!("invalid --timeout value: '{value}' (expected seconds)")
                })?);
            }
            "--json" => json = true,
            _ if arg.starts_with('-') => return Err(format!("unknown flag: {arg}")),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
        i += 1;
    }

    Ok(AttachArgs {
        port: port.unwrap_or(DEFAULT_PORT),
        platform: platform.unwrap_or_else(|| "android".to_string()),
        serial,
        pid,
        watch,
        no_browser,
        timeout_secs: timeout_secs.unwrap_or(60),
        json,
    })
}

// ============================================================================
// android channel: adb forward → loopback URL
// ============================================================================

async fn cmd_attach_android(args: &AttachArgs) {
    let Some(adb) = AdbInfo::is_available() else {
        eprintln!("✗ adb not found on PATH. Install Android SDK platform-tools.");
        exit(1);
    };

    let port = args.port;
    let device_filter = args.serial.as_deref();
    let url = format!("http://127.0.0.1:{}", port);
    let mut established_forward = false;

    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let mut opened_browser = false;

    loop {
        let devices = adb.devices();
        let target_device = if let Some(serial) = device_filter {
            if devices.iter().any(|d| d == serial) {
                Some(serial.to_string())
            } else {
                None
            }
        } else if devices.len() == 1 {
            Some(devices[0].clone())
        } else if devices.is_empty() {
            None
        } else {
            eprintln!("✗ Multiple devices found — pick one with --serial <serial>:");
            for dev in &devices {
                eprintln!("    --serial {}", dev);
            }
            exit(2);
        };

        if let Some(dev) = target_device {
            if !established_forward {
                print!("• adb forward tcp:{} tcp:{} (device {}) … ", port, port, dev);
                match adb.forward(port, Some(&dev)) {
                    Ok(()) => {
                        println!("ok");
                        established_forward = true;
                    }
                    Err(e) => {
                        println!("failed");
                        eprintln!("✗ adb forward failed: {}", e);
                        eprintln!(
                            "  This usually means another process on the Mac already\n\
                             \x20 owns localhost:{}. Run: adb forward --remove-all\n\
                             \x20 or: lsof -iTCP:{} -sTCP:LISTEN -t | xargs kill",
                            port, port
                        );
                        exit(2);
                    }
                }
            }

            if !opened_browser {
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({ "url": url, "port": port, "serial": dev, "via": "adb" })
                    );
                } else {
                    println!("✔ Connected to device: {}", dev);
                    println!("✔ URL: {}", url);
                }
                if !args.no_browser {
                    open_browser(&url);
                }
                opened_browser = true;
            }

            if !args.watch {
                return;
            }
        }

        if !args.watch && Instant::now() >= deadline {
            eprintln!("✗ No device found in {}s.", args.timeout_secs);
            eprintln!(
                "  Check that:\n\
                 \x20   • adb sees the device (run: adb devices)\n\
                 \x20   • The app is running with sqlite_spect enabled"
            );
            exit(1);
        }

        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

// ============================================================================
// ios channel: mDNS network discovery
// ============================================================================

/// Outcome of filtering discovered instances against an optional --pid.
enum Pick<'a> {
    None,
    One(&'a Discovered),
    Many(Vec<&'a Discovered>),
}

fn pick_mdns_target(hits: &[Discovered], pid: Option<u32>) -> Pick<'_> {
    let candidates: Vec<&Discovered> = match pid {
        Some(p) => hits.iter().filter(|h| h.pid == p).collect(),
        None => hits.iter().collect(),
    };
    match candidates.len() {
        0 => Pick::None,
        1 => Pick::One(candidates[0]),
        _ => Pick::Many(candidates),
    }
}

/// Rewrite `http://<local-ip>:<port>` to `http://127.0.0.1:<port>` so a
/// same-machine hit (e.g. a `serve` bound to loopback only) stays reachable.
fn rewrite_url_host(url: &str, local_ips: &[IpAddr]) -> String {
    for ip in local_ips {
        let host = format!("://{ip}:");
        if url.contains(&host) {
            return url.replace(&host, "://127.0.0.1:");
        }
    }
    url.to_string()
}

fn rewrite_same_host(mut hit: Discovered) -> Discovered {
    let local_ips: Vec<IpAddr> = if_addrs::get_if_addrs()
        .map(|addrs| addrs.iter().map(|a| a.ip()).collect())
        .unwrap_or_default();
    hit.url = rewrite_url_host(&hit.url, &local_ips);
    hit
}

async fn cmd_attach_ios(args: &AttachArgs) {
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);

    loop {
        let hits = try_discover_mdns(Duration::from_millis(600)).unwrap_or_default();
        match pick_mdns_target(&hits, args.pid) {
            Pick::One(hit) => {
                let hit = rewrite_same_host(hit.clone());
                if args.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "url": hit.url, "port": hit.port, "pid": hit.pid,
                            "platform": hit.platform, "via": "mdns",
                        })
                    );
                } else {
                    println!("✔ Found inspector (platform: {}, pid: {})", hit.platform, hit.pid);
                    println!("✔ URL: {}", hit.url);
                }
                if !args.no_browser {
                    open_browser(&hit.url);
                }
                return;
            }
            Pick::Many(hits) => {
                if args.json {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "error": "multiple instances found — use --pid <N>",
                            "instances": hits.iter().map(|h| serde_json::json!({
                                "url": h.url, "pid": h.pid, "port": h.port, "platform": h.platform,
                            })).collect::<Vec<_>>(),
                        })
                    );
                } else {
                    eprintln!("✗ Multiple inspectors found — pick one with --pid <N>:");
                    for h in &hits {
                        eprintln!("    --pid {}   (platform: {}, {})", h.pid, h.platform, h.url);
                    }
                }
                exit(2);
            }
            Pick::None => {}
        }

        if Instant::now() >= deadline {
            if let Some(pid) = args.pid {
                eprintln!("✗ No inspector with pid {pid} found.");
            } else {
                eprintln!("✗ No inspector found on the network in {}s.", args.timeout_secs);
            }
            eprintln!(
                "  Check that:\n\
                 \x20   • The app is running with sqlite_spect enabled\n\
                 \x20   • The device and this machine are on the same network\n\
                 \x20   \x20 (mDNS/multicast must not be blocked)"
            );
            exit(1);
        }

        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);

    let _ = std::process::Command::new(cmd)
        .args(&args)
        .spawn()
        .and_then(|mut c| c.wait());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn hit(pid: u32, url: &str) -> Discovered {
        Discovered {
            url: url.to_string(),
            port: 8123,
            pid,
            platform: "macos".into(),
        }
    }

    #[test]
    fn attach_args_defaults() {
        let a = parse_attach_args(&args(&[])).unwrap();
        assert_eq!(a.port, DEFAULT_PORT);
        assert_eq!(a.platform, "android");
        assert!(a.serial.is_none());
        assert!(a.pid.is_none());
        assert!(!a.watch);
        assert!(!a.no_browser);
        assert_eq!(a.timeout_secs, 60);
        assert!(!a.json);
    }

    #[test]
    fn attach_args_parse_all_flags() {
        let a = parse_attach_args(&args(&[
            "--port",
            "9000",
            "--serial",
            "emulator-5554",
            "--platform",
            "ios",
            "--pid",
            "4211",
            "--no-browser",
            "--timeout",
            "5",
        ]))
        .unwrap();
        assert_eq!(a.port, 9000);
        assert_eq!(a.platform, "ios");
        assert_eq!(a.serial.as_deref(), Some("emulator-5554"));
        assert_eq!(a.pid, Some(4211));
        assert!(!a.watch);
        assert!(a.no_browser);
        assert_eq!(a.timeout_secs, 5);
    }

    #[test]
    fn attach_args_json_flag_parses() {
        let a = parse_attach_args(&args(&["--json"])).unwrap();
        assert!(a.json);
    }

    #[test]
    fn attach_args_reject_unknown_flag_and_positional() {
        assert!(parse_attach_args(&args(&["--jsonx"])).is_err());
        assert!(parse_attach_args(&args(&["oops"])).is_err());
    }

    #[test]
    fn attach_args_reject_invalid_values() {
        assert!(parse_attach_args(&args(&["--timeout", "abc"])).is_err());
        assert!(parse_attach_args(&args(&["--timeout"])).is_err());
        assert!(parse_attach_args(&args(&["--port", "abc"])).is_err());
        assert!(parse_attach_args(&args(&["--port"])).is_err());
        assert!(parse_attach_args(&args(&["--serial"])).is_err());
        assert!(parse_attach_args(&args(&["--pid", "abc"])).is_err());
        assert!(parse_attach_args(&args(&["--pid"])).is_err());
        assert!(parse_attach_args(&args(&["--platform", "windows"])).is_err());
        assert!(parse_attach_args(&args(&["--platform"])).is_err());
    }

    #[test]
    fn validate_rejects_watch_json_and_watch_ios() {
        let mut a = parse_attach_args(&args(&["--watch", "--json"])).unwrap();
        assert!(validate(&a).is_err());

        a = parse_attach_args(&args(&["--watch", "--platform", "ios"])).unwrap();
        assert!(validate(&a).is_err());

        a = parse_attach_args(&args(&["--watch", "--platform", "android"])).unwrap();
        assert!(validate(&a).is_ok());
    }

    #[test]
    fn url_for_formats_ipv4_and_brackets_ipv6() {
        assert_eq!(
            url_for(IpAddr::from([192, 168, 1, 5]), 8123),
            "http://192.168.1.5:8123"
        );
        assert_eq!(
            url_for("fe80::1".parse().unwrap(), 8123),
            "http://[fe80::1]:8123"
        );
    }

    #[test]
    fn mdns_acc_prefers_ipv4_over_ipv6() {
        let acc = MdnsAcc {
            pid: 1,
            platform: "macos".into(),
            port: 8123,
            ipv4: Some(IpAddr::from([10, 0, 0, 5])),
            any: Some("fe80::1".parse().unwrap()),
        };
        assert_eq!(acc.best_url(), "http://10.0.0.5:8123");

        let acc = MdnsAcc {
            pid: 1,
            platform: "macos".into(),
            port: 8123,
            ipv4: None,
            any: Some("fe80::1".parse().unwrap()),
        };
        assert_eq!(acc.best_url(), "http://[fe80::1]:8123");
    }

    #[test]
    fn pick_returns_none_one_or_many() {
        assert!(matches!(pick_mdns_target(&[], None), Pick::None));

        let hits = vec![hit(1, "http://10.0.0.5:8123")];
        assert!(matches!(pick_mdns_target(&hits, None), Pick::One(_)));

        let hits = vec![hit(1, "http://10.0.0.5:8123"), hit(2, "http://10.0.0.9:8123")];
        assert!(matches!(pick_mdns_target(&hits, None), Pick::Many(_)));
    }

    #[test]
    fn pick_filters_by_pid() {
        let hits = vec![hit(1, "http://10.0.0.5:8123"), hit(2, "http://10.0.0.9:8123")];
        assert!(matches!(pick_mdns_target(&hits, Some(2)), Pick::One(h) if h.pid == 2));
        assert!(matches!(pick_mdns_target(&hits, Some(99)), Pick::None));
    }

    #[test]
    fn rewrite_url_host_targets_local_ips_only() {
        let local = vec![
            IpAddr::from([192, 168, 1, 42]),
            IpAddr::from([127, 0, 0, 1]),
        ];
        assert_eq!(
            rewrite_url_host("http://192.168.1.42:8123", &local),
            "http://127.0.0.1:8123"
        );
        // Foreign address stays untouched.
        assert_eq!(
            rewrite_url_host("http://10.0.0.5:8123", &local),
            "http://10.0.0.5:8123"
        );
        // Already loopback stays as-is.
        assert_eq!(
            rewrite_url_host("http://127.0.0.1:8123", &local),
            "http://127.0.0.1:8123"
        );
        assert_eq!(rewrite_url_host("http://10.0.0.5:8123", &[]), "http://10.0.0.5:8123");
    }
}
