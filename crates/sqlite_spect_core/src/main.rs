// Standalone CLI entry point.
//
// Subcommands:
//   sqlite_spect <db-path> [--port N] [-v]
//   sqlite_spect serve <db-path> [--port N] [-v]
//   sqlite_spect list
//   sqlite_spect url [--pid N]
//   sqlite_spect attach [--platform <android|ios>] [--port N] [--serial <id>]

use sqlite_spect_core::db::{DbConfig, DbRegistry};
use sqlite_spect_core::server::{bind, start_server_with_shutdown};
use sqlite_spect_core::{banner, host_platform, mdns, DEFAULT_PORT};
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
            let (db_path, port, verbose) = match parse_serve_args(&args[2..]) {
                Some(x) => x,
                None => usage_and_exit(),
            };
            run_serve(db_path, port, verbose).await;
        }
        "list" => {
            init_tracing(false);
            attach::cmd_list().await;
        }
        "url" => {
            init_tracing(false);
            attach::cmd_url(&args[2..]).await;
        }
        "attach" => {
            init_tracing(false);
            attach::cmd_attach(&args[2..]).await;
        }
        "" | "-h" | "--help" | "help" => usage_and_exit(),
        _ => {
            let (db_path, port, verbose) = match parse_serve_args(&args[1..]) {
                Some(x) => x,
                None => usage_and_exit(),
            };
            run_serve(db_path, port, verbose).await;
        }
    }
}

async fn run_serve(db_path: String, port: u16, verbose: bool) {
    init_tracing(verbose);
    install_panic_hook();

    // Derive a friendly id from the filename: "/…/oia.db" → "oia".
    let id = Path::new(&db_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();

    let config = DbConfig {
        id: id.clone(),
        path: db_path.clone(),
        read_only: false,
    };

    let registry = match DbRegistry::open(vec![config]) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(db = %db_path, error = ?e, "failed to open database");
            exit(1);
        }
    };

    let (listener, actual_port) = match bind(port) {
        Ok(x) => x,
        Err(e) => {
            tracing::error!(port, error = %e, "failed to bind — is another process using this port?");
            exit(1);
        }
    };

    let db_ids = registry.db_ids();
    banner::emit(&db_ids);

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

fn parse_serve_args(args: &[String]) -> Option<(String, u16, bool)> {
    let mut db_path: Option<String> = None;
    let mut port: u16 = DEFAULT_PORT;
    let mut verbose = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port = args.get(i)?.parse().ok()?;
            }
            "-v" | "--verbose" => verbose = true,
            "--network" => {} // legacy no-op
            other if !other.starts_with("--") && !other.starts_with('-') => {
                db_path = Some(other.to_string());
            }
            _ => return None,
        }
        i += 1;
    }

    Some((db_path?, port, verbose))
}

fn usage_and_exit() -> ! {
    eprintln!(
        "usage:\n\
         \x20 sqlite_spect <db-path> [--port N] [-v]\n\
         \x20 sqlite_spect serve <db-path> [--port N] [-v]\n\
         \x20 sqlite_spect list\n\
         \x20 sqlite_spect url [--pid N]\n\
         \x20 sqlite_spect attach [--watch] [--no-browser] [--timeout S]"
    );
    exit(2);
}

