use std::process::{Command, Stdio};

pub struct AdbInfo {
    program_path: String,
}

impl AdbInfo {

    pub fn is_available() -> Option<AdbInfo> {
        is_available_path().map(|program_path| AdbInfo { program_path })
    }
    
    pub fn forward(&self, port: u16, device_serial: Option<&str>) -> Result<(), String> {
        let port_str = port.to_string();
        let port_arg = format!("tcp:{}", port_str);
        let mut args: Vec<&str> = vec![];
        if let Some(serial) = device_serial {
            args.push("-s");
            args.push(serial);
        }
        args.push("forward");
        args.push(&port_arg);
        args.push(&port_arg);
        let output = Command::new(self.program_path.to_owned())
            .args(args)
            .output()
            .map_err(|e| format!("failed to spawn adb: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("adb forward exit code {}", output.status)
            } else {
                stderr
            });
        }
        Ok(())
    }

    pub fn devices(&self) -> Vec<String> {
        let output = match Command::new(&self.program_path).arg("devices").output() {
            Ok(o) if o.status.success() => o,
            _ => return vec![],
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .skip(1) // "List of devices attached"
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let serial = parts.next()?;
                let state = parts.next()?;
                if state == "device" {
                    Some(serial.to_string())
                } else {
                    None
                }
            })
            .collect()
    }
}

fn is_available_path() -> Option<String> {
    if let Ok(output) = std::process::Command::new("which").arg("adb").output() {
        if output.status.success() {
            if let Ok(path) = String::from_utf8(output.stdout) {
                let adb_path = path.trim().to_string();
                if !adb_path.is_empty() && test_adb(&adb_path) {
                    return Some(adb_path);
                }
            }
        }
    }
    // Check common Android SDK locations
    let home = std::env::var("HOME").ok();
    let mut candidates = vec![
        "/opt/android-sdk/platform-tools/adb".to_string(),
        "/usr/local/bin/adb".to_string(),
    ];
    if let Some(home_dir) = home {
        candidates.push(format!("{}/Android/sdk/platform-tools/adb", home_dir));
        candidates.push(format!(
            "{}/Library/Android/sdk/platform-tools/adb",
            home_dir
        ));
    }

    for adb_path in candidates {
        if std::path::Path::new(&adb_path).exists() && test_adb(&adb_path) {
            return Some(adb_path);
        }
    }

    None
}

fn test_adb(adb_path: &str) -> bool {
    match Command::new(adb_path)
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) => {
            eprintln!("Failed to run adb at {}: {}", adb_path, e);
            false
        }
    }
}
