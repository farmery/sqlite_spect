use crate::db::{DbRegistry, MutationSubs};
use crate::rpc;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rust_embed::Embed;
use std::net::{IpAddr, Ipv4Addr};
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

/// Bind on 0.0.0.0:port — the address the app-embedded server uses (adb
/// forward and mDNS both need to reach it). Returns a std::net listener
/// (set non-blocking so `TcpListener::from_std` inside the server's runtime
/// works) plus the actual port (may differ from the requested port if `port`
/// was 0).
///
/// Runs synchronously — no runtime needed — so bind failures surface on the
/// calling thread before we spawn anything. Using std::net (not
/// tokio::net::TcpListener) is important: a tokio listener is bound to the
/// runtime that created it and cannot be moved to a different runtime.
pub fn bind(port: u16) -> Result<(std::net::TcpListener, u16), std::io::Error> {
    bind_on(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
}

/// Bind on a specific address — the CLI `serve` uses loopback by default.
pub fn bind_on(ip: IpAddr, port: u16) -> Result<(std::net::TcpListener, u16), std::io::Error> {
    let addr = std::net::SocketAddr::new(ip, port);
    let listener = std::net::TcpListener::bind(addr)?;
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

    let state = AppState { registry, subs };

    let app = build_router(state);
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

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/assets/*path", get(asset_handler))
        .route("/ws", get(ws_handler))
        .route("/rpc", post(rpc_handler))
        .route("/*path", get(static_handler))
        .with_state(state)
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

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
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
                        let ctx = rpc::Ctx { registry: &registry, subs: &subs, tx: &tx, transport: rpc::Transport::Ws };
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

// --- HTTP JSON-RPC handler ---

/// `POST /rpc` — same JSON-RPC dialect as `/ws`, for curl/scripts/agents.
/// Push methods (subscriptions) are rejected with -32010 because HTTP has
/// no server→client channel; batching and id-less notifications likewise.
/// Content-Type is deliberately not enforced so plain `curl -d` works.
async fn rpc_handler(State(state): State<AppState>, body: String) -> Response {
    // Pre-dispatch shape checks: parse once as a Value, reject what only
    // makes sense over WebSocket, then hand the original body to dispatch
    // so envelopes stay byte-identical to /ws.
    let pre: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"body is not valid JSON\"}",
            )
                .into_response()
        }
    };
    if pre.is_array() {
        return json_ok(rpc::error_response(
            serde_json::Value::Null,
            rpc::ERR_INVALID_REQUEST,
            "batching not supported over HTTP",
        ));
    }
    if pre.get("id").is_none() {
        return json_ok(rpc::error_response(
            serde_json::Value::Null,
            rpc::ERR_INVALID_REQUEST,
            "id required over HTTP (notifications need a push channel)",
        ));
    }

    // Dummy notification channel — never used: push methods are rejected
    // for Transport::Http before they could register it.
    let (tx, _rx) = mpsc::channel::<String>(1);
    let ctx = rpc::Ctx {
        registry: &state.registry,
        subs: &state.subs,
        tx: &tx,
        transport: rpc::Transport::Http,
    };
    let response = rpc::dispatch(&body, &ctx, "http").await;
    json_ok(response)
}

fn json_ok(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DbConfig, DbRegistry};
    use axum::body::Body;
    use axum::http::Request;
    use std::time::Duration;
    use tempfile::NamedTempFile;
    use tokio::time::timeout;
    use tower::ServiceExt; // for Router::oneshot

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

    fn test_state(registry: DbRegistry) -> AppState {
        let subs = MutationSubs::new(registry.clone());
        AppState { registry, subs }
    }

    async fn post_rpc(app: &Router, body: &str, headers: &[(&str, &str)]) -> Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/rpc")
            .header("content-type", "application/json");
        for &(k, v) in headers {
            builder = builder.header(k, v);
        }
        app.clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn rpc_serves_db_list() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":1,"method":"db.list"}"#,
            &[],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_string(res).await;
        assert!(body.contains(r#""id":1"#));
        assert!(body.contains("t")); // the registered db id
    }

    #[tokio::test]
    async fn rpc_server_info_lists_methods() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":1,"method":"server.info"}"#,
            &[],
        )
        .await;
        let body = body_string(res).await;
        assert!(body.contains("\"methods\""));
        assert!(body.contains("\"db.query\""));
        assert!(body.contains("\"server.info\""));
        assert!(body.contains("\"http\""));
    }

    #[tokio::test]
    async fn rpc_rejects_push_methods_with_32010() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":7,"method":"db.subscribe","params":{"db":"t"}}"#,
            &[],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_string(res).await;
        assert!(body.contains("-32010"), "got: {body}");
        assert!(body.contains("WebSocket"));
    }

    #[tokio::test]
    async fn rpc_rejects_batches_and_missing_id() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = post_rpc(
            &app,
            r#"[{"jsonrpc":"2.0","id":1,"method":"db.list"}]"#,
            &[],
        )
        .await;
        let body = body_string(res).await;
        assert!(body.contains("batching not supported"), "got: {body}");

        let res = post_rpc(&app, r#"{"jsonrpc":"2.0","method":"db.list"}"#, &[]).await;
        let body = body_string(res).await;
        assert!(body.contains("id required"), "got: {body}");
    }

    #[tokio::test]
    async fn rpc_rejects_malformed_json_with_400() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = post_rpc(&app, "not json", &[]).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rpc_get_returns_405() {
        let (_file, registry) = setup_registry();
        let app = build_router(test_state(registry));

        let res = app
            .oneshot(Request::builder().uri("/rpc").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // WebSocketUpgrade extraction requires hyper's OnUpgrade extension, which
    // only exists for requests that came through a real server — oneshot
    // can't produce it. So the ws route gets checked over a real TCP
    // connection.
    #[tokio::test]
    async fn ws_upgrade_succeeds_over_real_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (_file, registry) = setup_registry();
        let (listener, port) = bind(0).unwrap();
        let (shutdown_tx, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            start_server_with_shutdown(listener, registry, rx).await;
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Upgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "got: {head}");

        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), handle).await.unwrap().unwrap();
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

    #[test]
    fn bind_on_loopback_reports_loopback() {
        let (listener, _port) = bind_on(IpAddr::from([127, 0, 0, 1]), 0).unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            IpAddr::from([127, 0, 0, 1])
        );
    }
}
