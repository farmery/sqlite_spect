// Standalone CLI entry point.
//
// Subcommands:
//   sqlite_spect serve <db-path> [--port N] [--host ADDR] [--json] [-v]
//   sqlite_spect attach [--platform <android|ios>] [--port N] [--serial <id>]
//                       [--pid N] [--watch] [--no-browser] [--timeout S] [--json]

use sqlite_spect_core::db::{DbConfig, DbRegistry};
use sqlite_spect_core::server::{bind_on, start_server_with_shutdown};
use sqlite_spect_core::{banner, host_platform, mdns, DEFAULT_PORT};
use std::net::IpAddr;
use std::path::Path;
use std::process::exit;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

mod attach;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    // First-arg dispatch. Legacy shape: `sqlite_spect <db-path>` is treated as `serve`.
    let subcommand = args.get(1).map(String::as_str).unwrap_or("");
    match subcommand {
        "serve" => {
            let serve_args = match parse_serve_args(&args[2..]) {
                Ok(x) => x,
                Err(msg) => usage_error(&msg),
            };
            run_serve(serve_args).await;
        }
        "attach" => {
            init_tracing(false);
            attach::cmd_attach(&args[2..]).await;
        }
        "-V" | "--version" => print_version(),
        "-h" | "--help" | "help" => print_help(),
        "" => {
            // Bare invocation: full usage on stderr with the conventional
            // nonzero exit; explicit --help prints the same text on stdout.
            eprintln!("{HELP}");
            exit(2);
        }
        _ => {
            let serve_args = match parse_serve_args(&args[1..]) {
                Ok(x) => x,
                Err(msg) => usage_error(&msg),
            };
            run_serve(serve_args).await;
        }
    }
}

struct ServeArgs {
    db_path: String,
    port: u16,
    verbose: bool,
    host: IpAddr,
    json: bool,
}

async fn run_serve(args: ServeArgs) {
    init_tracing(args.verbose);
    install_panic_hook();

    // Derive a friendly id from the filename: "/…/oia.db" → "oia".
    let id = Path::new(&args.db_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();

    let config = DbConfig {
        id: id.clone(),
        path: args.db_path.clone(),
        read_only: false,
    };

    let registry = match DbRegistry::open(vec![config]) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(db = %args.db_path, error = ?e, "failed to open database");
            exit(1);
        }
    };

    let (listener, actual_port) = match bind_on(args.host, args.port) {
        Ok(x) => x,
        Err(e) => {
            tracing::error!(port = args.port, error = %e, "failed to bind — is another process using this port?");
            exit(1);
        }
    };

    let db_ids = registry.db_ids();
    let display_url = format!("http://127.0.0.1:{}", actual_port);

    if args.json {
        // The machine-readable contract: stdout gets exactly one JSON
        // document; logs and the banner stay on stderr.
        println!(
            "{}",
            serde_json::json!({
                "url": format!("{}/", display_url),
                "port": actual_port,
                "host": args.host.to_string(),
                "databases": db_ids,
            })
        );
    } else {
        banner::emit(&db_ids);
    }
    tracing::info!("sqlite_spect at {}/", display_url);

    let _mdns_handle = mdns::register(actual_port, host_platform(), &db_ids);

    // Wire Ctrl-C to graceful shutdown so mdns cleanup runs.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown requested (Ctrl-C)");
        let _ = shutdown_tx.send(());
    });

    tracing::info!(port = actual_port, db_id = %id, "starting server");
    start_server_with_shutdown(listener, registry, shutdown_rx).await;
    tracing::info!("server stopped");
}

fn init_tracing(verbose: bool) {
    // mdns_sd=off suppresses that crate's benign multicast errors from spamming the CLI.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if verbose {
            EnvFilter::new("sqlite_spect_core=debug,info,mdns_sd=off")
        } else {
            EnvFilter::new("info,mdns_sd=off")
        }
    });

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default(info);
    }));
}

fn parse_serve_args(args: &[String]) -> Result<ServeArgs, String> {
    let mut db_path: Option<String> = None;
    let mut port: u16 = DEFAULT_PORT;
    let mut verbose = false;
    let mut host: IpAddr = IpAddr::from([127, 0, 0, 1]);
    let mut json = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => print_help(),
            "--port" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--port expects a number".to_string())?;
                port = value.parse().map_err(|_| {
                    format!("invalid --port value: '{value}' (expected 1-65535)")
                })?;
            }
            "--host" => {
                i += 1;
                let value = take_value(args, i, "--host")?;
                host = parse_host(&value)?;
            }
            "--json" => json = true,
            "-v" | "--verbose" => verbose = true,
            "--network" => {} // legacy no-op
            _ if arg.starts_with('-') => return Err(format!("unknown flag: {arg}")),
            _ if db_path.is_none() => db_path = Some(arg.to_string()),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
        i += 1;
    }

    Ok(ServeArgs {
        db_path: db_path.ok_or_else(|| "missing <db-path>".to_string())?,
        port,
        verbose,
        host,
        json,
    })
}

fn parse_host(value: &str) -> Result<IpAddr, String> {
    if value.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::from([127, 0, 0, 1]));
    }
    value
        .parse()
        .map_err(|_| format!("invalid --host value: '{value}' (expected an IP address)"))
}

/// Fetch the value that follows a `--flag`, erroring (with the flag name) if
/// it's missing. Index is the position of the value, one past the flag.
fn take_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i)
        .cloned()
        .ok_or_else(|| format!("{flag} expects a value"))
}

/// Full help text. Printed verbatim, so every flag the parsers accept must
/// appear here — the parsers reject unknown flags, so this list is the contract.
const HELP: &str = "\
sqlite_spect — SQLite inspector for Flutter apps

USAGE:
    sqlite_spect serve <db-path> [OPTIONS]
    sqlite_spect attach [OPTIONS]
    sqlite_spect <db-path> [OPTIONS]          (legacy shorthand for serve)

COMMANDS:
    serve    Open a local SQLite file and serve the inspector UI on this
             machine (http://127.0.0.1:<port>).
    attach   Connect to a running inspector and open it in a browser.
             Two channels, selected by --platform: android (adb forward,
             default) and ios (mDNS network discovery — finds any announced
             inspector on the LAN, including desktop serve instances).

JSON OUTPUT:
    With --json, stdout carries exactly one JSON document (errors go to
    stderr as JSON); logs never touch stdout. Exit codes are unchanged.

SERVE OPTIONS:
    --port <N>        Port to listen on (default 8123).
    --host <ADDR>     Address to bind (default 127.0.0.1; 0.0.0.0 exposes
                      the inspector to the network).
    --json            Print startup info (url, port, databases) as a single
                      JSON document on stdout.
    -v, --verbose     Enable debug logging.

ATTACH OPTIONS:
    --platform <P>    android (default): adb forward to the device's
                      inspector. ios: mDNS discovery on the local network.
    --port <N>        (android) Port to forward (default 8123). Must match
                      the port passed to Inspector.start() in the app.
    --serial <ID>     (android) Target a specific device (see: adb devices).
    --pid <N>         (ios) Pick a specific instance when several inspectors
                      are discovered on the network.
    --watch           (android) Keep the adb forward alive; re-open the
                      browser on reconnect.
    --no-browser      Don't open a browser; just print the URL.
    --timeout <SECS>  How long to wait for a device/instance (default 60).
    --json            Print the attach result as JSON. Not combinable
                      with --watch.

OPTIONS:
    -h, --help        Print this help.
    -V, --version     Print version.";

fn print_help() -> ! {
    println!("{HELP}");
    exit(0);
}

fn print_version() -> ! {
    println!("sqlite_spect {}", cli_version());
    exit(0);
}

/// release.yml sets SQLITE_SPECT_CLI_VERSION=<tag> at build time so release
/// binaries report the release they shipped in; local builds fall back to the
/// crate version, which is versioned independently of the pub package.
fn cli_version() -> &'static str {
    option_env!("SQLITE_SPECT_CLI_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// One-line parse error + pointer to --help, on stderr, with the
/// conventional usage-error exit code.
fn usage_error(message: &str) -> ! {
    eprintln!("sqlite_spect: {message}");
    eprintln!("Try 'sqlite_spect --help' for more information.");
    exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn serve_args_parses_path_port_verbose() {
        let a = parse_serve_args(&args(&["/tmp/app.db", "--port", "9000", "-v"])).unwrap();
        assert_eq!(a.db_path, "/tmp/app.db");
        assert_eq!(a.port, 9000);
        assert!(a.verbose);
    }

    #[test]
    fn serve_args_defaults() {
        let a = parse_serve_args(&args(&["/tmp/app.db"])).unwrap();
        assert_eq!(a.db_path, "/tmp/app.db");
        assert_eq!(a.port, DEFAULT_PORT);
        assert!(!a.verbose);
        assert_eq!(a.host, IpAddr::from([127, 0, 0, 1]));
        assert!(!a.json);
    }

    #[test]
    fn serve_args_parses_host_and_json() {
        let a =
            parse_serve_args(&args(&["/tmp/app.db", "--host", "0.0.0.0", "--json"])).unwrap();
        assert_eq!(a.host, IpAddr::from([0, 0, 0, 0]));
        assert!(a.json);

        let a = parse_serve_args(&args(&["/tmp/app.db", "--host", "localhost"])).unwrap();
        assert_eq!(a.host, IpAddr::from([127, 0, 0, 1]));
    }

    #[test]
    fn serve_args_rejects_bad_host() {
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--host", "not-an-ip"])).is_err());
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--host"])).is_err());
    }

    #[test]
    fn serve_args_rejects_unknown_flag() {
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--jsonx"])).is_err());
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--token", "x"])).is_err());
    }

    #[test]
    fn serve_args_rejects_missing_or_invalid_port() {
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--port"])).is_err());
        assert!(parse_serve_args(&args(&["/tmp/app.db", "--port", "abc"])).is_err());
    }

    #[test]
    fn serve_args_rejects_missing_path() {
        assert!(parse_serve_args(&args(&["--port", "9000"])).is_err());
    }

    #[test]
    fn serve_args_rejects_second_positional() {
        assert!(parse_serve_args(&args(&["one.db", "two.db"])).is_err());
    }
}
