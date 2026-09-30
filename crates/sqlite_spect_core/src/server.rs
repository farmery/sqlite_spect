use crate::db::{DbRegistry, MutationSubs};
use crate::rpc;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::Embed;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Embed)]
#[folder = "../../client/sqlite_spect_client/dist"]
struct WebAsset;

#[derive(Clone)]
struct AppState {
    registry: DbRegistry,
    subs: MutationSubs,
}

/// Bind a TCP listener on 0.0.0.0:port. Returns a std::net listener (set
/// non-blocking so `TcpListener::from_std` inside the server's runtime works)
/// plus the actual port (may differ from the requested port if `port` was 0).
///
/// Runs synchronously — no runtime needed — so bind failures surface on the
/// calling thread before we spawn anything. Using std::net (not
/// tokio::net::TcpListener) is important: a tokio listener is bound to the
/// runtime that created it and cannot be moved to a different runtime.
pub fn bind(port: u16) -> Result<(std::net::TcpListener, u16), std::io::Error> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = std::net::TcpListener::bind(&addr)?;
    listener.set_nonblocking(true)?;
    let actual = listener.local_addr()?.port();
    Ok((listener, actual))
}

/// Run the inspector server on `listener` until `shutdown_rx` fires, then
/// close every open WebSocket and return.
///
/// Takes a `std::net::TcpListener` and converts internally with
/// `tokio::net::TcpListener::from_std`, which binds the listener to the
/// current tokio runtime. This lets the CLI and FFI bind synchronously on
/// their own thread and hand the listener off to whichever runtime will
/// drive the server.
pub async fn start_server_with_shutdown(
    listener: std::net::TcpListener,
    registry: DbRegistry,
    shutdown_rx: oneshot::Receiver<()>,
) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "failed to promote std listener to tokio");
            return;
        }
    };

    let subs = MutationSubs::new(registry.clone());

    let state = AppState {
        registry,
        subs,
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/assets/*path", get(asset_handler))
        .route("/ws", get(ws_handler))
        .route("/*path", get(static_handler))
        .with_state(state);

    let shutdown = async move {
        // If the sender was dropped without sending, treat it as a shutdown
        // signal too — same as an explicit stop.
        let _ = shutdown_rx.await;
    };

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!(error = %e, "server exited with error");
    }
}

/// Simplified entry point for the CLI: binds and runs until the process is killed.
pub async fn start_server(port: u16, registry: DbRegistry) {
    let listener = match bind(port) {
        Ok((l, _)) => l,
        Err(e) => {
            tracing::error!(
                port,
                error = %e,
                "failed to bind — is another process using this port?"
            );
            std::process::exit(1);
        }
    };
    tracing::info!(port, "listening");

    let (_tx, rx) = oneshot::channel();
    start_server_with_shutdown(listener, registry, rx).await;
}

// --- Static file handlers ---

async fn index_handler() -> Response {
    match WebAsset::get("index.html") {
        Some(file) => Html(String::from_utf8_lossy(&file.data).into_owned()).into_response(),
        None => {
            tracing::error!(
                "client bundle not embedded — run: (cd client/sqlite_spect_client && npm run build) then rebuild the CLI"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Client bundle not embedded.\n\
                 Run:  cd client/sqlite_spect_client && npm run build\n\
                 Then: cargo build --features cli",
            )
                .into_response()
        }
    }
}

async fn asset_handler(Path(path): Path<String>) -> Response {
    serve_embedded(format!("assets/{}", path))
}

async fn static_handler(Path(path): Path<String>) -> Response {
    serve_embedded(path)
}

fn serve_embedded(path: String) -> Response {
    match WebAsset::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// --- WebSocket handler ---

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    // TODO: auth_key check goes here when implemented
    ws.on_upgrade(move |socket| handle_socket(socket, state.registry, state.subs))
}

async fn handle_socket(mut socket: WebSocket, registry: DbRegistry, subs: MutationSubs) {
    // Short id for correlating log lines from one connection.
    let sock_id: String = Uuid::new_v4().simple().to_string()[..8].to_string();
    tracing::info!(sock = %sock_id, "ws connected");

    let (tx, mut rx) = mpsc::channel::<String>(32);

    loop {
        tokio::select! {
            Some(notification) = rx.recv() => {
                if socket.send(Message::Text(notification.into())).await.is_err() {
                    tracing::debug!(sock = %sock_id, "ws send failed while pushing notification");
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let ctx = rpc::Ctx { registry: &registry, subs: &subs, tx: &tx };
                        let response = rpc::dispatch(&text, &ctx, &sock_id).await;
                        if socket.send(Message::Text(response.into())).await.is_err() {
                            tracing::debug!(sock = %sock_id, "ws send failed while responding");
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::debug!(sock = %sock_id, error = %e, "ws recv error");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    tracing::info!(sock = %sock_id, "ws disconnected");
    // Socket is closing — drop any subscriptions this connection owned.
    // This also triggers poller abort if we were the last subscriber for a DB.
    subs.remove_by_tx(&tx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DbConfig, DbRegistry};
    use std::time::Duration;
    use tempfile::NamedTempFile;
    use tokio::time::timeout;

    fn setup_registry() -> (NamedTempFile, DbRegistry) {
        let file = NamedTempFile::new().unwrap();
        {
            let c = rusqlite::Connection::open(file.path()).unwrap();
            c.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        }
        let reg = DbRegistry::open(vec![DbConfig {
            id: "t".into(),
            path: file.path().to_str().unwrap().into(),
            read_only: false,
        }])
        .unwrap();
        (file, reg)
    }

    #[tokio::test]
    async fn shutdown_signal_stops_server_within_1s() {
        let (_file, registry) = setup_registry();
        let (listener, port) = bind(0).unwrap();

        let (tx, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            start_server_with_shutdown(listener, registry, rx).await;
        });

        // Give the server a moment to start accepting.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Sanity: port is bound.
        assert!(port > 0);

        // Signal shutdown; assert the task exits promptly.
        tx.send(()).unwrap();
        timeout(Duration::from_secs(1), handle).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dropped_shutdown_sender_also_stops_server() {
        // Spec §12.6-ish edge case: if a caller drops the shutdown_tx without
        // sending, the receiver resolves to Err(_) and we still shut down.
        let (_file, registry) = setup_registry();
        let (listener, _port) = bind(0).unwrap();

        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            start_server_with_shutdown(listener, registry, rx).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(tx);
        timeout(Duration::from_secs(1), handle).await.unwrap().unwrap();
    }

    #[test]
    fn bind_on_port_zero_returns_actual_port() {
        let (_listener, port) = bind(0).unwrap();
        assert!(port > 0);
    }
}
