use crate::db::{DbError, DbRegistry, MutationSubs};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;
use tokio::sync::mpsc;

// Which connection a request arrived on. Push methods (subscriptions) only
// work over WebSocket — HTTP has no server→client channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Ws,
    Http,
}

// Everything a handler might need. Passed by ref through dispatch.
pub struct Ctx<'a> {
    pub registry: &'a DbRegistry,
    pub subs: &'a MutationSubs,
    pub tx: &'a mpsc::Sender<String>,
    pub transport: Transport,
}

// Parses params into T, returning early from the enclosing fn with a serialised error
// if params are absent or malformed. Must be defined before use.
macro_rules! try_params {
    ($params:expr, $id:expr, $T:ty) => {
        match $params {
            None => return error_response($id, ERR_INVALID_PARAMS, "params required"),
            Some(v) => match serde_json::from_value::<$T>(v) {
                Ok(p) => p,
                Err(e) => return error_response($id, ERR_INVALID_PARAMS, &e.to_string()),
            },
        }
    };
}

// Error codes from the spec
pub const ERR_INVALID_REQUEST: i32 = -32600;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INVALID_PARAMS: i32 = -32602;
#[allow(dead_code)]
const ERR_INTERNAL: i32 = -32603;
#[allow(dead_code)]
const ERR_READ_ONLY: i32 = -32000;
#[allow(dead_code)]
const ERR_UNKNOWN_DB: i32 = -32001;
#[allow(dead_code)]
const ERR_SQL: i32 = -32002;
const ERR_PUSH_OVER_HTTP: i32 = -32010;

/// Every JSON-RPC method this build dispatches. Served by `server.info`
// so scripts/agents can discover the API at runtime; keep in sync with
// the dispatch match below.
const METHODS: &[&str] = &[
    "server.info",
    "db.list",
    "db.schema",
    "db.tableInfo",
    "db.query",
    "db.execute",
    "db.tableRows",
    "db.updateRow",
    "db.insertRow",
    "db.deleteRow",
    "db.clearTable",
    "db.subscribe",
    "db.unsubscribe",
    "probe.tail",
    "probe.untail",
];

// --- Envelopes ---

#[derive(Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    params: Option<Value>,
    id: Option<Value>,
}

#[derive(Serialize)]
struct RpcSuccess {
    jsonrpc: &'static str,
    result: Value,
    id: Value,
}

#[derive(Serialize)]
struct RpcFailure {
    jsonrpc: &'static str,
    error: RpcErrorObject,
    id: Value,
}

#[derive(Serialize)]
struct RpcErrorObject {
    code: i32,
    message: String,
}

// --- Param shapes (one struct per method) ---

#[derive(Deserialize)]
struct DbParams {
    db: String,
}

#[derive(Deserialize)]
struct TableInfoParams {
    db: String,
    table: String,
}

#[derive(Deserialize)]
struct QueryParams {
    db: String,
    sql: String,
    params: Option<Value>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct ExecuteParams {
    db: String,
    sql: String,
    params: Option<Value>,
}

#[derive(Deserialize)]
struct SubscribeParams {
    db: String,
    table: Option<String>,
}

#[derive(Deserialize)]
struct UnsubscribeParams {
    #[serde(rename = "subscriptionId")]
    subscription_id: String,
}

#[derive(Deserialize)]
struct TailParams {
    db: Option<String>,
    #[serde(rename = "minDurationMicros")]
    min_duration_micros: Option<i64>,
}

#[derive(Deserialize)]
struct TableRowsParams {
    db: String,
    table: String,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct UpdateRowParams {
    db: String,
    table: String,
    #[serde(rename = "rowKey")]
    row_key: Value,
    updates: Value,
}

#[derive(Deserialize)]
struct InsertRowParams {
    db: String,
    table: String,
    values: Value,
}

#[derive(Deserialize)]
struct DeleteRowParams {
    db: String,
    table: String,
    #[serde(rename = "rowKey")]
    row_key: Value,
}

#[derive(Deserialize)]
struct ClearTableParams {
    db: String,
    table: String,
    confirm: String,
}

// --- Entry point ---

/// Called from handle_socket in server.rs for every incoming text frame.
/// `sock_id` is a short correlation string logged with every event on this connection.
pub async fn dispatch(raw: &str, ctx: &Ctx<'_>, sock_id: &str) -> String {
    let started = Instant::now();

    let req: RpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(sock = %sock_id, error = %e, "rpc frame not valid JSON-RPC");
            return error_response(Value::Null, ERR_INVALID_REQUEST, "invalid request");
        }
    };

    let id = req.id.unwrap_or(Value::Null);
    let method = req.method.clone();

    let response = match req.method.as_str() {
        "server.info"    => handle_server_info(id).await,
        "db.list"        => handle_db_list(id, ctx).await,
        "db.schema"      => handle_db_schema(id, req.params, ctx).await,
        "db.tableInfo"   => handle_db_table_info(id, req.params, ctx).await,
        "db.query"       => handle_db_query(id, req.params, ctx).await,
        "db.execute"     => handle_db_execute(id, req.params, ctx).await,
        "db.tableRows"   => handle_db_table_rows(id, req.params, ctx).await,
        "db.updateRow"   => handle_db_update_row(id, req.params, ctx).await,
        "db.insertRow"   => handle_db_insert_row(id, req.params, ctx).await,
        "db.deleteRow"   => handle_db_delete_row(id, req.params, ctx).await,
        "db.clearTable"  => handle_db_clear_table(id, req.params, ctx).await,
        "db.subscribe"   => handle_db_subscribe(id, req.params, ctx).await,
        "db.unsubscribe" => handle_db_unsubscribe(id, req.params, ctx).await,
        "probe.tail"     => handle_probe_tail(id, req.params, ctx).await,
        "probe.untail"   => handle_probe_untail(id, req.params, ctx).await,
        _                => error_response(id, ERR_METHOD_NOT_FOUND, "method not found"),
    };

    // Detect error by looking for the top-level `,"error":{` marker. Fragile
    // but avoids re-parsing the response — good enough for debug logging.
    let is_err = response.contains(r#","error":{"code":"#);
    let duration_us = started.elapsed().as_micros() as u64;
    if is_err {
        tracing::debug!(sock = %sock_id, method = %method, duration_us, "rpc → error");
    } else {
        tracing::debug!(sock = %sock_id, method = %method, duration_us, "rpc → ok");
    }
    response
}

// --- Handlers ---

/// Capability probe for scripts/agents: version, platform, transports, and
/// the full method table in one call.
async fn handle_server_info(id: Value) -> String {
    ok_response(
        id,
        serde_json::json!({
            "version": crate::VERSION,
            "platform": crate::host_platform(),
            "transports": ["ws", "http"],
            "methods": METHODS,
        }),
    )
}

async fn handle_db_list(id: Value, ctx: &Ctx<'_>) -> String {
    ok_response(id, ctx.registry.list())
}

async fn handle_db_schema(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, DbParams);
    db_result(id, ctx.registry.schema(&p.db))
}

async fn handle_db_table_info(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, TableInfoParams);
    db_result(id, ctx.registry.table_info(&p.db, &p.table))
}

async fn handle_db_query(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, QueryParams);
    db_result(id, ctx.registry.query(&p.db, &p.sql, p.params, p.limit, p.offset))
}

async fn handle_db_execute(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, ExecuteParams);
    db_result(id, ctx.registry.execute(&p.db, &p.sql, p.params))
}

async fn handle_db_table_rows(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, TableRowsParams);
    db_result(id, ctx.registry.table_rows(&p.db, &p.table, p.limit, p.offset))
}

async fn handle_db_update_row(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, UpdateRowParams);
    db_result(id, ctx.registry.update_row(&p.db, &p.table, &p.row_key, &p.updates))
}

async fn handle_db_insert_row(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, InsertRowParams);
    db_result(id, ctx.registry.insert_row(&p.db, &p.table, &p.values))
}

async fn handle_db_delete_row(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, DeleteRowParams);
    db_result(id, ctx.registry.delete_row(&p.db, &p.table, &p.row_key))
}

async fn handle_db_clear_table(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    let p = try_params!(params, id, ClearTableParams);
    db_result(id, ctx.registry.clear_table(&p.db, &p.table, &p.confirm))
}

async fn handle_db_subscribe(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    if ctx.transport == Transport::Http {
        return error_response(id, ERR_PUSH_OVER_HTTP, "push methods require a WebSocket connection (/ws)");
    }
    let p = try_params!(params, id, SubscribeParams);
    let sub_id = ctx.subs.add(p.db, p.table, ctx.tx.clone());
    ok_response(id, serde_json::json!({ "subscriptionId": sub_id }))
}

async fn handle_db_unsubscribe(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    if ctx.transport == Transport::Http {
        return error_response(id, ERR_PUSH_OVER_HTTP, "push methods require a WebSocket connection (/ws)");
    }
    let p = try_params!(params, id, UnsubscribeParams);
    ctx.subs.remove(&p.subscription_id);
    ok_response(id, serde_json::json!({}))
}

async fn handle_probe_tail(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    if ctx.transport == Transport::Http {
        return error_response(id, ERR_PUSH_OVER_HTTP, "push methods require a WebSocket connection (/ws)");
    }
    let p = try_params!(params, id, TailParams);
    let _ = p; // TODO: register probe subscriber in probe.rs
    ok_response(id, serde_json::json!({ "subscriptionId": "stub" }))
}

async fn handle_probe_untail(id: Value, params: Option<Value>, ctx: &Ctx<'_>) -> String {
    if ctx.transport == Transport::Http {
        return error_response(id, ERR_PUSH_OVER_HTTP, "push methods require a WebSocket connection (/ws)");
    }
    let p = try_params!(params, id, UnsubscribeParams);
    let _ = p; // TODO: remove probe subscriber
    ok_response(id, serde_json::json!({}))
}

// --- Helpers ---

fn db_result(id: Value, result: Result<Value, DbError>) -> String {
    match result {
        Ok(v) => ok_response(id, v),
        Err(DbError::UnknownDb)          => error_response(id, ERR_UNKNOWN_DB, "unknown database id"),
        Err(DbError::ReadOnly)           => error_response(id, ERR_READ_ONLY, "database is read-only"),
        Err(DbError::Sql(msg))           => error_response(id, ERR_SQL, &msg),
        Err(DbError::Internal(msg))      => error_response(id, ERR_INTERNAL, &msg),
        Err(DbError::InvalidParams(msg)) => error_response(id, ERR_INVALID_PARAMS, &msg),
    }
}

fn ok_response(id: Value, result: Value) -> String {
    serde_json::to_string(&RpcSuccess { jsonrpc: "2.0", result, id }).unwrap()
}

// Also used by the HTTP transport in server.rs for pre-dispatch rejections
// (batching, missing id), so the envelopes stay identical everywhere.
pub fn error_response(id: Value, code: i32, message: &str) -> String {
    serde_json::to_string(&RpcFailure {
        jsonrpc: "2.0",
        error: RpcErrorObject { code, message: message.to_owned() },
        id,
    })
    .unwrap()
}
