// `sqlite_spect` discovery subcommands: `list`, `url`, `attach`.
//
// Discovery channels
// android: adb forward cfg.port, then open 127.0.0.1:cfg.port
// ios simulator: use loopback (127.0.0.1:cfg.port, detected via simctl)
// ios physical: use mdns discovery
// (simctl / idevicesyslog channels for iOS are still TODO — spec §21.)

use sqlite_spect_core::adb::AdbInfo;
use sqlite_spect_core::DEFAULT_PORT;
use std::process::{exit, Command};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Discovered {
    pub url: String,
    pub port: u16,
    pub pid: u32,
    pub platform: String,
    pub source: &'static str,
    /// Device identifier (ADB serial for Android, UDID for iOS). None for mDNS.
    pub device: Option<String>,
}

/// Scan every discovery channel we know about and return the union.
pub fn discover_all(_device_filter: Option<&str>) -> Vec<Discovered> {
    let mut out = Vec::new();

    if let Some(mdns_hits) = try_discover_mdns(Duration::from_millis(600)) {
        out.extend(mdns_hits);
    }

    out
}

/// One-shot mDNS query. Returns `None` if mDNS is blocked / unavailable.
fn try_discover_mdns(wait: Duration) -> Option<Vec<Discovered>> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let daemon = ServiceDaemon::new().ok()?;
    let receiver = daemon.browse(sqlite_spect_core::mdns::SERVICE_TYPE).ok()?;

    let deadline = std::time::Instant::now() + wait;
    let mut hits = Vec::new();

    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
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
                let host_ip = info
                    .get_addresses()
                    .iter()
                    .next()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "127.0.0.1".into());
                let url = format!("http://{}:{}", host_ip, port);
                hits.push(Discovered {
                    url,
                    port,
                    pid,
                    platform,
                    source: "mdns",
                    device: None,
                });
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
    Some(hits)
}
// ============================================================================
// Commands
// ============================================================================

pub async fn cmd_list() {
    let hits = discover_all(None);
    if hits.is_empty() {
        println!("no running sqlite_spect instances found");
        return;
    }

    println!(
        "{:<8} {:<6} {:<10} {:<12} {:<18} {}",
        "PID", "PORT", "PLATFORM", "SOURCE", "DEVICE", "URL"
    );
    for d in &hits {
        println!(
            "{:<8} {:<6} {:<10} {:<12} {:<18} {}",
            d.pid,
            d.port,
            d.platform,
            d.source,
            d.device.as_deref().unwrap_or("-"),
            d.url
        );
    }
}

pub async fn cmd_url(args: &[String]) {
    let mut target_pid: Option<u32> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pid" => {
                i += 1;
                target_pid = args.get(i).and_then(|s| s.parse().ok());
            }
            _ => {}
        }
        i += 1;
    }

    let hits = discover_all(None);
    let picked = match target_pid {
        Some(pid) => hits.into_iter().find(|d| d.pid == pid),
        None => match hits.len() {
            0 => None,
            1 => Some(hits.into_iter().next().unwrap()),
            _ => {
                eprintln!("multiple instances found — use --pid <N> to disambiguate:");
                for d in &hits {
                    eprintln!("  pid={} port={} device={} {}",
                        d.pid, d.port, d.device.as_deref().unwrap_or("-"), d.url);
                }
                exit(2);
            }
        },
    };

    match picked {
        Some(d) => println!("{}", d.url),
        None => {
            eprintln!("no matching instance");
            exit(1);
        }
    }
}

pub async fn cmd_attach(args: &[String]) {
    let mut port: Option<u16> = None;
    let mut platform: Option<String> = None;
    let mut serial: Option<String> = None;
    let mut watch = false;
    let mut no_browser = false;
    let mut timeout_secs: u64 = 60;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port = args.get(i).and_then(|s| s.parse().ok());
            }
            "--platform" => {
                i += 1;
                platform = args.get(i).cloned();
            }
            "--serial" => {
                i += 1;
                serial = args.get(i).cloned();
            }
            "--watch" => watch = true,
            "--no-browser" => no_browser = true,
            "--timeout" => {
                i += 1;
                timeout_secs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(60);
            }
            _ => {}
        }
        i += 1;
    }

    let port = port.unwrap_or(DEFAULT_PORT);
    let platform = platform.unwrap_or_else(|| "android".to_string());

    match platform.as_str() {
        "android" => cmd_attach_android(port, serial, watch, no_browser, timeout_secs).await,
        "ios" => cmd_attach_ios(port, serial, watch, no_browser, timeout_secs).await,
        _ => {
            eprintln!("✗ Unknown platform: {}. Use 'android' or 'ios'.", platform);
            exit(1);
        }
    }
}

async fn cmd_attach_android(
    port: u16,
    serial: Option<String>,
    watch: bool,
    no_browser: bool,
    timeout_secs: u64,
) {
    let Some(adb) = AdbInfo::is_available() else {
        eprintln!("✗ adb not found on PATH. Install Android SDK platform-tools.");
        exit(1);
    };

    let device_filter = serial.as_deref();
    let url = format!("http://127.0.0.1:{}", port);
    let mut established_forward = false;

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
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
                println!("✔ Connected to device: {}", dev);
                println!("✔ URL: {}", url);
                if !no_browser {
                    open_browser(&url);
                }
                opened_browser = true;
            }

            if !watch {
                return;
            }
        }

        if !watch && std::time::Instant::now() >= deadline {
            eprintln!("✗ No device found in {}s.", timeout_secs);
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

async fn cmd_attach_ios(
    port: u16,
    serial: Option<String>,
    _watch: bool,
    no_browser: bool,
    _timeout_secs: u64,
) {
    // TODO: iOS implementation
    // For simulator: use simctl to detect, then use loopback
    // For physical device: use mdns discovery
    eprintln!("✗ iOS attach is not yet implemented");
    exit(1);
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

