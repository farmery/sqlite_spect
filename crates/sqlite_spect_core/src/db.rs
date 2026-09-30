use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

// --- Public config (comes from Inspector.start() via FFI) ---

pub struct DbConfig {
    pub id: String,
    pub path: String,
    pub read_only: bool,
}

// --- Error type ---

#[derive(Debug)]
pub enum DbError {
    UnknownDb,
    ReadOnly,
    Sql(String),
    Internal(String),
    InvalidParams(String),
}

// --- Internal handle: one per registered DB ---
struct DbHandle {
    config: DbConfig,
    // Mutex because rusqlite::Connection is Send but not Sync.
    // All access is serialised; fine for a dev tool.
    conn: Mutex<Connection>,
}

// --- Registry: the single shared object passed to rpc.rs ---
#[derive(Clone)]
pub struct DbRegistry(Arc<HashMap<String, Arc<DbHandle>>>);

impl DbRegistry {
    pub fn open(configs: Vec<DbConfig>) -> Result<Self, DbError> {
        let mut map = HashMap::new();

        for cfg in configs {
            let flags = if cfg.read_only {
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI
            };

            let conn = Connection::open_with_flags(&cfg.path, flags)
                .map_err(|e| DbError::Internal(e.to_string()))?;

            // Warn if not WAL — agent still works but reads may briefly block.
            let mode: String = conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap_or_default();
            if mode != "wal" {
                tracing::warn!(
                    db = %cfg.id,
                    journal_mode = %mode,
                    "DB is not in WAL mode — reads may block briefly during app writes"
                );
            }

            // Force full fsync after every write commit so WAL frames are fully
            // on disk before Flutter's sqlite3 (a separate binary) reads them.
            // Without this, SQLITE_IOERR_SHORT_READ (522) occurs when the app's
            // watch fires immediately after an inspector write.
            if !cfg.read_only {
                conn.pragma_update(None, "synchronous", "FULL")
                    .map_err(|e| DbError::Internal(e.to_string()))?;
            }

            tracing::info!(db = %cfg.id, path = %cfg.path, read_only = cfg.read_only, "opened database");
            map.insert(cfg.id.clone(), Arc::new(DbHandle { config: cfg, conn: Mutex::new(conn) }));
        }

        Ok(Self(Arc::new(map)))
    }

    fn get(&self, db_id: &str) -> Result<&Arc<DbHandle>, DbError> {
        self.0.get(db_id).ok_or(DbError::UnknownDb)
    }

    // --- db.list ---

    pub fn list(&self) -> Value {
        let entries: Vec<Value> = self.0.values().map(|h| {
            let mut caps = vec!["read", "liveMutations"];
            if !h.config.read_only {
                caps.push("write");
            }
            json!({
                "id":           h.config.id,
                "path":         h.config.path,
                "capabilities": caps,
            })
        }).collect();

        Value::Array(entries)
    }

    // --- db.schema ---

    pub fn schema(&self, db_id: &str) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();

        let objects = query_all(
            &conn,
            "SELECT type, name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            [],
        )?;

        let mut tables   = vec![];
        let mut indexes  = vec![];
        let mut triggers = vec![];
        let mut views    = vec![];

        for obj in objects {
            let kind = obj[0].as_str().unwrap_or("");
            let entry = json!({ "name": obj[1], "sql": obj[2] });
            match kind {
                "table"   => tables.push(entry),
                "index"   => indexes.push(entry),
                "trigger" => triggers.push(entry),
                "view"    => views.push(entry),
                _ => {}
            }
        }

        Ok(json!({ "tables": tables, "indexes": indexes, "triggers": triggers, "views": views }))
    }

    // --- db.tableInfo ---

    pub fn table_info(&self, db_id: &str, table: &str) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();

        let columns = conn
            .prepare(&format!("PRAGMA table_info(\"{}\")", table))
            .and_then(|mut s| {
                s.query_map([], |row| {
                    Ok(json!({
                        "cid":      row.get::<_, i64>(0)?,
                        "name":     row.get::<_, String>(1)?,
                        "type":     row.get::<_, String>(2)?,
                        "notNull":  row.get::<_, bool>(3)?,
                        "default":  row.get::<_, Option<String>>(4)?,
                        "pk":       row.get::<_, i64>(5)?,
                    }))
                })
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            })
            .map_err(|e| DbError::Sql(e.to_string()))?;

        let index_list: Vec<(String, bool)> = conn
            .prepare(&format!("PRAGMA index_list({})", quote_ident(table)))
            .and_then(|mut s| {
                s.query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, bool>(2)?)))
                    .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            })
            .map_err(|e| DbError::Sql(e.to_string()))?;

        let mut indexes = vec![];
        for (name, unique) in &index_list {
            let info = conn
                .prepare(&format!("PRAGMA index_info({})", quote_ident(name)))
                .and_then(|mut s| {
                    s.query_map([], |row| {
                        Ok(json!({ "seqno": row.get::<_, i64>(0)?, "name": row.get::<_, String>(2)? }))
                    })
                    .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
                })
                .map_err(|e| DbError::Sql(e.to_string()))?;
            indexes.push(json!({ "name": name, "columns": info, "unique": unique }));
        }

        let foreign_keys = conn
            .prepare(&format!("PRAGMA foreign_key_list(\"{}\")", table))
            .and_then(|mut s| {
                s.query_map([], |row| {
                    Ok(json!({
                        "id":       row.get::<_, i64>(0)?,
                        "table":    row.get::<_, String>(2)?,
                        "from":     row.get::<_, String>(3)?,
                        "to":       row.get::<_, Option<String>>(4)?,
                        "onUpdate": row.get::<_, String>(5)?,
                        "onDelete": row.get::<_, String>(6)?,
                    }))
                })
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            })
            .map_err(|e| DbError::Sql(e.to_string()))?;

        Ok(json!({ "columns": columns, "indexes": indexes, "foreignKeys": foreign_keys }))
    }

    // --- db.query ---

    pub fn query(
        &self,
        db_id: &str,
        sql: &str,
        params: Option<Value>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        // Append LIMIT/OFFSET so the caller's SQL stays unmodified.
        let paged = format!("SELECT * FROM ({}) LIMIT {} OFFSET {}", sql, limit + 1, offset);

        let sql_params = json_params(params);
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            sql_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let started = Instant::now();

        let mut stmt = conn.prepare(&paged).map_err(|e| DbError::Sql(e.to_string()))?;

        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let col_count = col_names.len();

        let rows: Vec<Vec<Value>> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let mut cells = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    cells.push(sql_value_to_json(
                        row.get::<_, rusqlite::types::Value>(i)?,
                    ));
                }
                Ok(cells)
            })
            .and_then(|r| r.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|e| DbError::Sql(e.to_string()))?;

        let duration_micros = started.elapsed().as_micros() as i64;

        // We fetched limit+1 rows; if we got that many there are more pages.
        let has_more = rows.len() > limit as usize;
        let rows: Vec<Value> = rows
            .into_iter()
            .take(limit as usize)
            .map(Value::Array)
            .collect();

        Ok(json!({
            "columns": col_names,
            "rows": rows,
            "hasMore": has_more,
            "durationMicros": duration_micros,
        }))
    }

    // --- db.execute ---

    pub fn execute(
        &self,
        db_id: &str,
        sql: &str,
        params: Option<Value>,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        if handle.config.read_only {
            return Err(DbError::ReadOnly);
        }

        let conn = handle.conn.lock().unwrap();
        let sql_params = json_params(params);
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            sql_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let started = Instant::now();
        let rows_affected = conn
            .execute(sql, param_refs.as_slice())
            .map_err(|e| DbError::Sql(e.to_string()))?;
        let duration_micros = started.elapsed().as_micros() as i64;

        let last_insert_rowid = conn.last_insert_rowid();

        Ok(json!({
            "rowsAffected":     rows_affected,
            "lastInsertRowid":  last_insert_rowid,
            "durationMicros":   duration_micros,
        }))
    }

    // --- db.tableRows ---

    pub fn table_rows(
        &self,
        db_id: &str,
        table: &str,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rk = resolve_row_key(&conn, table)?;

        let started = Instant::now();

        let (sql, rk_json) = match &rk {
            RowKeyKind::Pk(cols) => (
                format!(
                    "SELECT * FROM {} LIMIT {} OFFSET {}",
                    quote_ident(table),
                    limit + 1,
                    offset
                ),
                json!({ "kind": "pk", "cols": cols }),
            ),
            RowKeyKind::Rowid { alias } => (
                format!(
                    "SELECT rowid AS {}, * FROM {} LIMIT {} OFFSET {}",
                    quote_ident(alias),
                    quote_ident(table),
                    limit + 1,
                    offset
                ),
                json!({ "kind": "rowid", "alias": alias }),
            ),
            RowKeyKind::None => (
                format!(
                    "SELECT * FROM {} LIMIT {} OFFSET {}",
                    quote_ident(table),
                    limit + 1,
                    offset
                ),
                json!({ "kind": "none" }),
            ),
        };

        let mut stmt = conn.prepare(&sql).map_err(|e| DbError::Sql(e.to_string()))?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let col_count = col_names.len();

        let rows: Vec<Vec<Value>> = stmt
            .query_map([], |row| {
                let mut cells = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    cells.push(sql_value_to_json(row.get::<_, rusqlite::types::Value>(i)?));
                }
                Ok(cells)
            })
            .and_then(|r| r.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|e| DbError::Sql(e.to_string()))?;

        let duration_micros = started.elapsed().as_micros() as i64;

        let has_more = rows.len() > limit as usize;
        let rows: Vec<Value> = rows
            .into_iter()
            .take(limit as usize)
            .map(Value::Array)
            .collect();

        Ok(json!({
            "columns":       col_names,
            "rows":          rows,
            "hasMore":       has_more,
            "durationMicros": duration_micros,
            "rowKey":        rk_json,
        }))
    }

    // --- db.updateRow ---

    pub fn update_row(
        &self,
        db_id: &str,
        table: &str,
        row_key: &Value,
        updates: &Value,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        if handle.config.read_only {
            return Err(DbError::ReadOnly);
        }
        let mut conn = handle.conn.lock().unwrap();

        let updates_map = updates
            .as_object()
            .ok_or_else(|| DbError::InvalidParams("updates must be an object".into()))?;
        if updates_map.is_empty() {
            return Err(DbError::InvalidParams("updates must not be empty".into()));
        }

        // Validate all update columns exist.
        let cols = column_set(&conn, table)?;
        for col in updates_map.keys() {
            if !cols.contains(col.as_str()) {
                return Err(DbError::InvalidParams(format!(
                    "column '{}' does not exist in table '{}'",
                    col, table
                )));
            }
        }

        let rk_kind = resolve_row_key(&conn, table)?;
        let rk_map = row_key
            .as_object()
            .ok_or_else(|| DbError::InvalidParams("rowKey must be an object".into()))?;

        let (where_clause, where_vals) =
            build_where(&rk_kind, rk_map, table)?;

        let set_parts: Vec<String> = updates_map
            .keys()
            .map(|c| format!("{} = ?", quote_ident(c)))
            .collect();
        let set_vals: Vec<rusqlite::types::Value> = updates_map
            .values()
            .cloned()
            .map(json_to_sql)
            .collect();

        let sql = format!(
            "UPDATE {} SET {} WHERE {}",
            quote_ident(table),
            set_parts.join(", "),
            where_clause
        );

        let all_params: Vec<rusqlite::types::Value> =
            set_vals.into_iter().chain(where_vals).collect();
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            all_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let started = Instant::now();
        let tx = conn.transaction().map_err(|e| DbError::Sql(e.to_string()))?;
        let rows_affected = tx
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| DbError::Sql(e.to_string()))?;
        tx.commit().map_err(|e| DbError::Sql(e.to_string()))?;
        let duration_micros = started.elapsed().as_micros() as i64;

        Ok(json!({ "rowsAffected": rows_affected, "durationMicros": duration_micros }))
    }

    // --- db.insertRow ---

    pub fn insert_row(
        &self,
        db_id: &str,
        table: &str,
        values: &Value,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        if handle.config.read_only {
            return Err(DbError::ReadOnly);
        }
        let mut conn = handle.conn.lock().unwrap();

        let values_map = values
            .as_object()
            .ok_or_else(|| DbError::InvalidParams("values must be an object".into()))?;

        let started = Instant::now();
        let rows_affected;

        if values_map.is_empty() {
            let sql = format!("INSERT INTO {} DEFAULT VALUES", quote_ident(table));
            let tx = conn.transaction().map_err(|e| DbError::Sql(e.to_string()))?;
            rows_affected = tx
                .execute(&sql, [])
                .map_err(|e| DbError::Sql(e.to_string()))?;
            tx.commit().map_err(|e| DbError::Sql(e.to_string()))?;
        } else {
            let cols = column_set(&conn, table)?;
            for col in values_map.keys() {
                if !cols.contains(col.as_str()) {
                    return Err(DbError::InvalidParams(format!(
                        "column '{}' does not exist in table '{}'",
                        col, table
                    )));
                }
            }

            let col_parts: Vec<String> =
                values_map.keys().map(|c| quote_ident(c)).collect();
            let placeholders: Vec<&str> = col_parts.iter().map(|_| "?").collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_ident(table),
                col_parts.join(", "),
                placeholders.join(", ")
            );

            let all_params: Vec<rusqlite::types::Value> =
                values_map.values().cloned().map(json_to_sql).collect();
            let param_refs: Vec<&dyn rusqlite::ToSql> =
                all_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

            let tx = conn.transaction().map_err(|e| DbError::Sql(e.to_string()))?;
            rows_affected = tx
                .execute(&sql, param_refs.as_slice())
                .map_err(|e| DbError::Sql(e.to_string()))?;
            tx.commit().map_err(|e| DbError::Sql(e.to_string()))?;
        }

        let duration_micros = started.elapsed().as_micros() as i64;
        let last_insert_rowid = conn.last_insert_rowid();

        let mut result = json!({
            "rowsAffected": rows_affected,
            "durationMicros": duration_micros,
        });
        if rows_affected > 0 {
            result["lastInsertRowid"] = json!(last_insert_rowid);
        }
        Ok(result)
    }

    // --- db.deleteRow ---

    pub fn delete_row(
        &self,
        db_id: &str,
        table: &str,
        row_key: &Value,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        if handle.config.read_only {
            return Err(DbError::ReadOnly);
        }
        let mut conn = handle.conn.lock().unwrap();

        let rk_kind = resolve_row_key(&conn, table)?;
        let rk_map = row_key
            .as_object()
            .ok_or_else(|| DbError::InvalidParams("rowKey must be an object".into()))?;

        let (where_clause, where_vals) = build_where(&rk_kind, rk_map, table)?;
        let sql = format!("DELETE FROM {} WHERE {}", quote_ident(table), where_clause);

        let all_params: Vec<rusqlite::types::Value> = where_vals;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            all_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let started = Instant::now();
        let tx = conn.transaction().map_err(|e| DbError::Sql(e.to_string()))?;
        let rows_affected = tx
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| DbError::Sql(e.to_string()))?;
        tx.commit().map_err(|e| DbError::Sql(e.to_string()))?;
        let duration_micros = started.elapsed().as_micros() as i64;

        Ok(json!({ "rowsAffected": rows_affected, "durationMicros": duration_micros }))
    }

    // --- db.clearTable ---

    pub fn clear_table(
        &self,
        db_id: &str,
        table: &str,
        confirm: &str,
    ) -> Result<Value, DbError> {
        let handle = self.get(db_id)?;
        if handle.config.read_only {
            return Err(DbError::ReadOnly);
        }
        if confirm.is_empty() {
            return Err(DbError::InvalidParams("confirm must not be empty".into()));
        }
        if confirm != table {
            return Err(DbError::InvalidParams(
                "confirm must equal table name".into(),
            ));
        }
        let mut conn = handle.conn.lock().unwrap();
        let sql = format!("DELETE FROM {}", quote_ident(table));

        let started = Instant::now();
        let tx = conn.transaction().map_err(|e| DbError::Sql(e.to_string()))?;
        let rows_affected = tx
            .execute(&sql, [])
            .map_err(|e| DbError::Sql(e.to_string()))?;
        tx.commit().map_err(|e| DbError::Sql(e.to_string()))?;
        let duration_micros = started.elapsed().as_micros() as i64;

        Ok(json!({ "rowsAffected": rows_affected, "durationMicros": duration_micros }))
    }

    // --- Mutation polling (called by the polling task in Phase 6) ---

    pub fn data_version(&self, db_id: &str) -> Result<i64, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();
        conn.pragma_query_value(None, "data_version", |r| r.get(0))
            .map_err(|e| DbError::Internal(e.to_string()))
    }

    pub fn schema_version(&self, db_id: &str) -> Result<i64, DbError> {
        let handle = self.get(db_id)?;
        let conn = handle.conn.lock().unwrap();
        conn.pragma_query_value(None, "schema_version", |r| r.get(0))
            .map_err(|e| DbError::Internal(e.to_string()))
    }

    pub fn db_ids(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

// --- Helpers ---

// Converts a JSON array of params into rusqlite-compatible values.
fn json_params(params: Option<Value>) -> Vec<rusqlite::types::Value> {
    match params {
        Some(Value::Array(arr)) => arr.into_iter().map(json_to_sql).collect(),
        _ => vec![],
    }
}

fn json_to_sql(v: Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(b as i64),
        Value::Number(n) => n
            .as_i64()
            .map(rusqlite::types::Value::Integer)
            .unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => rusqlite::types::Value::Text(s),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

fn sql_value_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(s) => json!(s),
        rusqlite::types::Value::Blob(b) => json!(format!("<blob {} bytes>", b.len())),
    }
}

// ============================================================
// Row identity helpers
// ============================================================

enum RowKeyKind {
    Pk(Vec<String>),
    Rowid { alias: String },
    None,
}

fn resolve_row_key(conn: &Connection, table: &str) -> Result<RowKeyKind, DbError> {
    // 1. Collect PK columns from PRAGMA table_info (pk > 0), ordered by pk value.
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote_ident(table)))
        .map_err(|e| DbError::Sql(e.to_string()))?;

    let mut pk_cols: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?)))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|e| DbError::Sql(e.to_string()))?;

    pk_cols.retain(|(pk, _)| *pk > 0);

    if !pk_cols.is_empty() {
        pk_cols.sort_by_key(|(pk, _)| *pk);
        return Ok(RowKeyKind::Pk(
            pk_cols.into_iter().map(|(_, name)| name).collect(),
        ));
    }

    // 2. Check if WITHOUT ROWID (substring search on CREATE SQL — no PRAGMA for this).
    let create_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name = ?",
            [table],
            |row| row.get(0),
        )
        .ok();

    if let Some(s) = create_sql {
        if s.to_uppercase().contains("WITHOUT ROWID") {
            return Ok(RowKeyKind::None);
        }
    }

    // 3. Has an implicit rowid. Pick a stable alias that doesn't collide with existing columns.
    let cols = column_set(conn, table)?;
    let alias = find_rowid_alias(&cols);
    Ok(RowKeyKind::Rowid { alias })
}

fn find_rowid_alias(cols: &HashSet<String>) -> String {
    if !cols.contains("__rowid") {
        return "__rowid".to_string();
    }
    let mut i = 0u32;
    loop {
        let candidate = format!("__inspector_rowid_{}", i);
        if !cols.contains(&candidate) {
            return candidate;
        }
        i += 1;
    }
}

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn column_set(conn: &Connection, table: &str) -> Result<HashSet<String>, DbError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote_ident(table)))
        .map_err(|e| DbError::Sql(e.to_string()))?;
    let cols: HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|e| DbError::Sql(e.to_string()))?
        .into_iter()
        .collect();
    Ok(cols)
}

// Builds a SQL WHERE clause + bound params from an incoming rowKey object,
// validating it against the resolved RowKeyKind.
fn build_where(
    rk_kind: &RowKeyKind,
    rk_map: &serde_json::Map<String, Value>,
    table: &str,
) -> Result<(String, Vec<rusqlite::types::Value>), DbError> {
    match rk_kind {
        RowKeyKind::Pk(pk_cols) => {
            for col in pk_cols {
                if !rk_map.contains_key(col) {
                    return Err(DbError::InvalidParams(format!(
                        "rowKey missing required PK column '{}'",
                        col
                    )));
                }
            }
            for key in rk_map.keys() {
                if !pk_cols.contains(key) {
                    return Err(DbError::InvalidParams(format!(
                        "rowKey column '{}' is not a PK column of '{}'",
                        key, table
                    )));
                }
            }
            let parts: Vec<String> =
                pk_cols.iter().map(|c| format!("{} = ?", quote_ident(c))).collect();
            let vals: Vec<rusqlite::types::Value> =
                pk_cols.iter().map(|c| json_to_sql(rk_map[c].clone())).collect();
            Ok((parts.join(" AND "), vals))
        }
        RowKeyKind::Rowid { alias } => {
            if rk_map.len() != 1 || !rk_map.contains_key(alias) {
                return Err(DbError::InvalidParams(format!(
                    "rowKey must have exactly one key '{}' for a rowid table",
                    alias
                )));
            }
            Ok(("rowid = ?".to_string(), vec![json_to_sql(rk_map[alias].clone())]))
        }
        RowKeyKind::None => Err(DbError::InvalidParams(
            "table has no primary key or rowid — editing is disabled".into(),
        )),
    }
}

// ============================================================
// Subscription registry + on-demand polling (Phase 6)
// ============================================================

const POLL_INTERVAL: Duration = Duration::from_millis(250);

struct Subscription {
    db_id: String,
    #[allow(dead_code)] // reserved for future per-table filtering (see arch §8.1)
    table: Option<String>,
    tx: mpsc::Sender<String>,
}

// Inner state under a single Mutex so add/remove decisions
// (spawn / abort a poller) happen atomically.
#[derive(Default)]
struct Inner {
    subs: HashMap<String, Subscription>,
    pollers: HashMap<String, JoinHandle<()>>, // db_id → poll task
}

// Owns the subscription registry AND the polling task lifecycle.
// A poller for a given DB exists iff that DB has ≥1 active subscriber.
#[derive(Clone)]
pub struct MutationSubs {
    inner: Arc<Mutex<Inner>>,
    registry: DbRegistry,
}

impl MutationSubs {
    pub fn new(registry: DbRegistry) -> Self {
        Self { inner: Arc::new(Mutex::new(Inner::default())), registry }
    }

    pub fn add(&self, db_id: String, table: Option<String>, tx: mpsc::Sender<String>) -> String {
        let id = Uuid::new_v4().to_string();
        let mut inner = self.inner.lock().unwrap();

        // First subscriber for this DB? Spawn its poller.
        // Capture the version baseline HERE (not inside the task) so any write
        // that happens after add() returns is visible against a known snapshot.
        // Reading the baseline inside the task would race with the caller.
        if !inner.pollers.contains_key(&db_id) {
            let baseline_data = self.registry.data_version(&db_id).unwrap_or(0);
            let baseline_schema = self.registry.schema_version(&db_id).unwrap_or(0);
            let handle = spawn_poller(self.clone(), db_id.clone(), baseline_data, baseline_schema);
            inner.pollers.insert(db_id.clone(), handle);
        }

        inner.subs.insert(id.clone(), Subscription { db_id, table, tx });
        id
    }

    pub fn remove(&self, sub_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(sub) = inner.subs.remove(sub_id) else { return; };
        gc_poller(&mut inner, &sub.db_id);
    }

    // Called from handle_socket when the WebSocket closes.
    // Sweeps every subscription that came from this socket.
    pub fn remove_by_tx(&self, tx: &mpsc::Sender<String>) {
        let mut inner = self.inner.lock().unwrap();
        let doomed: Vec<(String, String)> = inner
            .subs
            .iter()
            .filter(|(_, s)| s.tx.same_channel(tx))
            .map(|(id, s)| (id.clone(), s.db_id.clone()))
            .collect();

        for (id, _) in &doomed {
            inner.subs.remove(id);
        }
        for (_, db_id) in &doomed {
            gc_poller(&mut inner, db_id);
        }
    }

    // Returns cheap-to-clone tx handles for every subscriber to db_id.
    // Collected into a Vec so we release the lock before awaiting on send.
    fn senders_for(&self, db_id: &str) -> Vec<mpsc::Sender<String>> {
        self.inner
            .lock()
            .unwrap()
            .subs
            .values()
            .filter(|s| s.db_id == db_id)
            .map(|s| s.tx.clone())
            .collect()
    }
}

// Aborts the poller for `db_id` if no subscriptions remain for it.
// Caller must hold the Inner lock.
fn gc_poller(inner: &mut Inner, db_id: &str) {
    let still_watched = inner.subs.values().any(|s| s.db_id == db_id);
    if !still_watched {
        if let Some(handle) = inner.pollers.remove(db_id) {
            handle.abort();
        }
    }
}

// One task per DB. Polls data_version + schema_version at POLL_INTERVAL
// and broadcasts notifications. Task is aborted when the last subscriber leaves.
// data_version is a 32-bit wrapping int — compare with != not >.
// Baselines come from add() so the caller can rely on writes-after-subscribe being seen.
fn spawn_poller(
    subs: MutationSubs,
    db_id: String,
    mut last_data: i64,
    mut last_schema: i64,
) -> JoinHandle<()> {
    tracing::debug!(db = %db_id, "poller started");
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(POLL_INTERVAL);

        loop {
            tick.tick().await;

            match subs.registry.data_version(&db_id) {
                Ok(cur) if cur != last_data => {
                    last_data = cur;
                    broadcast(&subs, &db_id, "db.mutated",
                        json!({ "db": &db_id, "dataVersion": cur })).await;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(db = %db_id, error = ?e, "data_version read failed"),
            }

            match subs.registry.schema_version(&db_id) {
                Ok(cur) if cur != last_schema => {
                    last_schema = cur;
                    broadcast(&subs, &db_id, "db.schemaChanged",
                        json!({ "db": &db_id, "schemaVersion": cur })).await;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(db = %db_id, error = ?e, "schema_version read failed"),
            }
        }
    })
}

async fn broadcast(subs: &MutationSubs, db_id: &str, method: &str, params: Value) {
    let frame = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })).unwrap();

    let senders = subs.senders_for(db_id);
    let target_count = senders.len();
    let mut dropped = 0usize;
    for tx in senders {
        // Ignore send errors — a closed socket will be dropped on unsubscribe.
        // Count them so a persistent broadcast failure is visible in logs.
        if tx.send(frame.clone()).await.is_err() {
            dropped += 1;
        }
    }
    if dropped > 0 {
        tracing::debug!(db = %db_id, method, target_count, dropped, "broadcast: some subscribers unreachable");
    }
}

// ============================================================
// Helpers
// ============================================================

// Runs a query and returns all rows as Vec<Vec<Value>>.
fn query_all(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Vec<Value>>, DbError> {
    let mut stmt = conn.prepare(sql).map_err(|e| DbError::Sql(e.to_string()))?;
    let col_count = stmt.column_count();
    stmt.query_map(params, |row| {
        let mut cells = Vec::with_capacity(col_count);
        for i in 0..col_count {
            cells.push(sql_value_to_json(row.get::<_, rusqlite::types::Value>(i)?));
        }
        Ok(cells)
    })
    .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
    .map_err(|e| DbError::Sql(e.to_string()))
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    // ----- Helpers -----

    // Creates a temp file, initialises it with `schema_sql`, and opens a DbRegistry against it.
    fn setup(schema_sql: &str, read_only: bool) -> (NamedTempFile, DbRegistry) {
        let file = NamedTempFile::new().unwrap();
        {
            let c = Connection::open(file.path()).unwrap();
            c.execute_batch(schema_sql).unwrap();
        }
        let reg = DbRegistry::open(vec![DbConfig {
            id: "test".to_string(),
            path: file.path().to_str().unwrap().to_string(),
            read_only,
        }])
        .unwrap();
        (file, reg)
    }

    // Runs `sql` via a fresh, independent connection to `path`.
    // Simulates a write from "another process" (i.e. the app being inspected).
    fn external_exec(path: &Path, sql: &str) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(sql).unwrap();
    }

    // ============================================================
    // JSON <-> SQL conversion
    // ============================================================

    #[test]
    fn json_to_sql_roundtrip_scalars() {
        assert!(matches!(json_to_sql(Value::Null), rusqlite::types::Value::Null));
        assert!(matches!(
            json_to_sql(json!(true)),
            rusqlite::types::Value::Integer(1)
        ));
        assert!(matches!(
            json_to_sql(json!(false)),
            rusqlite::types::Value::Integer(0)
        ));
        assert!(matches!(
            json_to_sql(json!(42)),
            rusqlite::types::Value::Integer(42)
        ));
        match json_to_sql(json!(3.14)) {
            rusqlite::types::Value::Real(f) => assert!((f - 3.14).abs() < 1e-9),
            other => panic!("expected Real, got {:?}", other),
        }
        match json_to_sql(json!("hello")) {
            rusqlite::types::Value::Text(s) => assert_eq!(s, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn sql_value_to_json_scalars() {
        assert_eq!(sql_value_to_json(rusqlite::types::Value::Null), Value::Null);
        assert_eq!(sql_value_to_json(rusqlite::types::Value::Integer(42)), json!(42));
        assert_eq!(sql_value_to_json(rusqlite::types::Value::Real(3.14)), json!(3.14));
        assert_eq!(sql_value_to_json(rusqlite::types::Value::Text("x".into())), json!("x"));
        // Blob is intentionally placeholderised — we don't ship raw bytes over the wire.
        match sql_value_to_json(rusqlite::types::Value::Blob(vec![1, 2, 3])) {
            Value::String(s) => assert!(s.contains("3 bytes")),
            other => panic!("expected string placeholder, got {:?}", other),
        }
    }

    // ============================================================
    // DbRegistry::open
    // ============================================================

    #[test]
    fn open_missing_file_errors() {
        let r = DbRegistry::open(vec![DbConfig {
            id: "x".into(),
            path: "/tmp/definitely-not-a-real-path-xyz-999999.db".into(),
            read_only: true,
        }]);
        assert!(matches!(r, Err(DbError::Internal(_))));
    }

    #[test]
    fn open_read_only_flag_prevents_writes() {
        let (_f, reg) = setup("CREATE TABLE t (x INTEGER);", true);
        let err = reg
            .execute("test", "INSERT INTO t VALUES (1)", None)
            .unwrap_err();
        assert!(matches!(err, DbError::ReadOnly));
    }

    // ============================================================
    // db.list
    // ============================================================

    #[test]
    fn list_capabilities_for_writable_db() {
        let (_f, reg) = setup("CREATE TABLE t (x INTEGER);", false);
        let list = reg.list();
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let caps = arr[0]["capabilities"].as_array().unwrap();
        let cap_strs: Vec<&str> = caps.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(cap_strs.contains(&"read"));
        assert!(cap_strs.contains(&"write"));
        assert!(cap_strs.contains(&"liveMutations"));
    }

    #[test]
    fn list_omits_write_capability_when_read_only() {
        let (_f, reg) = setup("CREATE TABLE t (x INTEGER);", true);
        let list = reg.list();
        let caps = list.as_array().unwrap()[0]["capabilities"].as_array().unwrap();
        let cap_strs: Vec<&str> = caps.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(!cap_strs.contains(&"write"));
        assert!(cap_strs.contains(&"read"));
    }

    // ============================================================
    // db.schema
    // ============================================================

    #[test]
    fn schema_buckets_objects_by_type() {
        let (_f, reg) = setup(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             CREATE INDEX idx_users_name ON users(name);
             CREATE VIEW active AS SELECT * FROM users;
             CREATE TRIGGER trg AFTER INSERT ON users BEGIN SELECT 1; END;",
            false,
        );
        let s = reg.schema("test").unwrap();
        assert_eq!(s["tables"].as_array().unwrap().len(), 1);
        assert_eq!(s["indexes"].as_array().unwrap().len(), 1);
        assert_eq!(s["views"].as_array().unwrap().len(), 1);
        assert_eq!(s["triggers"].as_array().unwrap().len(), 1);
        assert_eq!(s["tables"][0]["name"], "users");
    }

    #[test]
    fn schema_excludes_sqlite_internal_objects() {
        // Composite PK causes SQLite to auto-create sqlite_autoindex_...
        let (_f, reg) = setup(
            "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY(a, b));",
            false,
        );
        let s = reg.schema("test").unwrap();
        let index_names: Vec<&str> = s["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["name"].as_str().unwrap())
            .collect();
        assert!(index_names.iter().all(|n| !n.starts_with("sqlite_")));
    }

    #[test]
    fn schema_unknown_db_returns_error() {
        let (_f, reg) = setup("CREATE TABLE t (x INTEGER);", false);
        let err = reg.schema("nope").unwrap_err();
        assert!(matches!(err, DbError::UnknownDb));
    }

    // ============================================================
    // db.tableInfo
    // ============================================================

    #[test]
    fn table_info_columns_have_correct_metadata() {
        let (_f, reg) = setup(
            "CREATE TABLE t (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                nickname TEXT DEFAULT 'anon'
            );",
            false,
        );
        let info = reg.table_info("test", "t").unwrap();
        let cols = info["columns"].as_array().unwrap();
        assert_eq!(cols.len(), 3);

        assert_eq!(cols[0]["name"], "id");
        assert_eq!(cols[0]["pk"], 1);
        assert_eq!(cols[1]["name"], "name");
        assert_eq!(cols[1]["notNull"], true);
        assert_eq!(cols[2]["default"], "'anon'");
    }

    #[test]
    fn table_info_composite_pk_marks_both_columns() {
        let (_f, reg) = setup(
            "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY(a, b));",
            false,
        );
        let info = reg.table_info("test", "t").unwrap();
        let pks: Vec<i64> = info["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["pk"].as_i64().unwrap())
            .collect();
        // Both columns should be marked pk (1 and 2), in some order.
        assert!(pks.contains(&1));
        assert!(pks.contains(&2));
    }

    #[test]
    fn table_info_returns_foreign_keys() {
        let (_f, reg) = setup(
            "CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER
                REFERENCES parent(id) ON DELETE CASCADE);",
            false,
        );
        let info = reg.table_info("test", "child").unwrap();
        let fks = info["foreignKeys"].as_array().unwrap();
        assert_eq!(fks.len(), 1);
        assert_eq!(fks[0]["table"], "parent");
        assert_eq!(fks[0]["from"], "parent_id");
        assert_eq!(fks[0]["to"], "id");
        assert_eq!(fks[0]["onDelete"], "CASCADE");
    }

    // ============================================================
    // db.query
    // ============================================================

    #[test]
    fn query_returns_columns_and_rows() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER, name TEXT);
             INSERT INTO t VALUES (1, 'a'), (2, 'b');",
            false,
        );
        let r = reg.query("test", "SELECT id, name FROM t ORDER BY id", None, None, None).unwrap();
        assert_eq!(r["columns"], json!(["id", "name"]));
        assert_eq!(r["rows"], json!([[1, "a"], [2, "b"]]));
        assert_eq!(r["hasMore"], false);
    }

    #[test]
    fn query_pagination_reports_hasmore() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER);
             INSERT INTO t VALUES (1), (2), (3), (4), (5);",
            false,
        );
        let r = reg
            .query("test", "SELECT id FROM t ORDER BY id", None, Some(2), Some(0))
            .unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 2);
        assert_eq!(r["hasMore"], true);
    }

    #[test]
    fn query_last_page_reports_no_more() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER);
             INSERT INTO t VALUES (1), (2), (3);",
            false,
        );
        let r = reg
            .query("test", "SELECT id FROM t ORDER BY id", None, Some(2), Some(2))
            .unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 1);
        assert_eq!(r["hasMore"], false);
    }

    #[test]
    fn query_binds_parameters() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER, name TEXT);
             INSERT INTO t VALUES (1, 'alice'), (2, 'bob');",
            false,
        );
        let r = reg
            .query("test", "SELECT name FROM t WHERE id = ?", Some(json!([2])), None, None)
            .unwrap();
        assert_eq!(r["rows"], json!([["bob"]]));
    }

    #[test]
    fn query_bad_sql_returns_sql_error() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let err = reg.query("test", "SELECT * FROM nonexistent", None, None, None).unwrap_err();
        assert!(matches!(err, DbError::Sql(_)));
    }

    // ============================================================
    // db.execute
    // ============================================================

    #[test]
    fn execute_insert_reports_rows_affected_and_last_rowid() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER PRIMARY KEY, x INTEGER);", false);
        let r = reg
            .execute("test", "INSERT INTO t (x) VALUES (?), (?)", Some(json!([10, 20])))
            .unwrap();
        assert_eq!(r["rowsAffected"], 2);
        assert_eq!(r["lastInsertRowid"], 2);
    }

    #[test]
    fn execute_update_matching_zero_rows_reports_zero() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER, x INTEGER);
             INSERT INTO t VALUES (1, 100);",
            false,
        );
        let r = reg
            .execute("test", "UPDATE t SET x = 0 WHERE id = 999", None)
            .unwrap();
        assert_eq!(r["rowsAffected"], 0);
    }

    // ============================================================
    // data_version / schema_version (visibility semantics)
    // ============================================================

    #[test]
    fn own_writes_do_not_bump_data_version() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let before = reg.data_version("test").unwrap();
        reg.execute("test", "INSERT INTO t VALUES (1)", None).unwrap();
        let after = reg.data_version("test").unwrap();
        // Same connection wrote; per SQLite spec its own data_version does not change.
        assert_eq!(before, after);
    }

    #[test]
    fn external_writes_bump_data_version() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let before = reg.data_version("test").unwrap();
        external_exec(file.path(), "INSERT INTO t VALUES (1)");
        let after = reg.data_version("test").unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn external_update_bumps_data_version() {
        let (file, reg) = setup(
            "CREATE TABLE t (id INTEGER, x INTEGER);
             INSERT INTO t VALUES (1, 0);",
            false,
        );
        let before = reg.data_version("test").unwrap();
        external_exec(file.path(), "UPDATE t SET x = 999 WHERE id = 1");
        assert_ne!(before, reg.data_version("test").unwrap());
    }

    #[test]
    fn external_delete_bumps_data_version() {
        let (file, reg) = setup(
            "CREATE TABLE t (id INTEGER);
             INSERT INTO t VALUES (1);",
            false,
        );
        let before = reg.data_version("test").unwrap();
        external_exec(file.path(), "DELETE FROM t WHERE id = 1");
        assert_ne!(before, reg.data_version("test").unwrap());
    }

    #[test]
    fn external_ddl_bumps_both_versions() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let data_before = reg.data_version("test").unwrap();
        let schema_before = reg.schema_version("test").unwrap();
        external_exec(file.path(), "CREATE INDEX i ON t(id)");
        assert_ne!(data_before, reg.data_version("test").unwrap());
        assert_ne!(schema_before, reg.schema_version("test").unwrap());
    }

    #[test]
    fn external_insert_does_not_bump_schema_version() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let before = reg.schema_version("test").unwrap();
        external_exec(file.path(), "INSERT INTO t VALUES (1)");
        assert_eq!(before, reg.schema_version("test").unwrap());
    }

    // ============================================================
    // MutationSubs — registry mechanics
    // ============================================================

    #[tokio::test]
    async fn add_returns_unique_ids() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, _rx) = mpsc::channel(4);
        let a = subs.add("test".into(), None, tx.clone());
        let b = subs.add("test".into(), None, tx);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn add_spawns_exactly_one_poller_per_db() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, _rx) = mpsc::channel(4);
        subs.add("test".into(), None, tx.clone());
        subs.add("test".into(), None, tx);
        assert_eq!(subs.inner.lock().unwrap().pollers.len(), 1);
    }

    #[tokio::test]
    async fn remove_last_sub_aborts_poller() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, _rx) = mpsc::channel(4);
        let id = subs.add("test".into(), None, tx);
        assert_eq!(subs.inner.lock().unwrap().pollers.len(), 1);
        subs.remove(&id);
        assert_eq!(subs.inner.lock().unwrap().pollers.len(), 0);
    }

    #[tokio::test]
    async fn remove_non_last_sub_keeps_poller() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, _rx) = mpsc::channel(4);
        let a = subs.add("test".into(), None, tx.clone());
        let _b = subs.add("test".into(), None, tx);
        subs.remove(&a);
        assert_eq!(subs.inner.lock().unwrap().pollers.len(), 1);
        assert_eq!(subs.inner.lock().unwrap().subs.len(), 1);
    }

    #[tokio::test]
    async fn remove_unknown_id_is_noop() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        subs.remove("does-not-exist"); // must not panic
    }

    #[tokio::test]
    async fn remove_by_tx_sweeps_all_subs_for_that_socket() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx_a, _rx_a) = mpsc::channel(4);
        let (tx_b, _rx_b) = mpsc::channel(4);
        subs.add("test".into(), None, tx_a.clone());
        subs.add("test".into(), None, tx_a.clone());
        subs.add("test".into(), None, tx_b);

        subs.remove_by_tx(&tx_a);

        // Only tx_b's single sub remains.
        assert_eq!(subs.inner.lock().unwrap().subs.len(), 1);
        // Poller still alive because tx_b is still subscribed.
        assert_eq!(subs.inner.lock().unwrap().pollers.len(), 1);
    }

    // ============================================================
    // Polling task (integration — real tokio, real timing)
    // ============================================================

    // POLL_INTERVAL is 250ms. Give it a full second of headroom for CI jitter.
    const RECV_TIMEOUT: Duration = Duration::from_millis(1000);

    #[tokio::test]
    async fn poller_broadcasts_db_mutated_on_external_insert() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, mut rx) = mpsc::channel(8);
        subs.add("test".into(), None, tx);

        external_exec(file.path(), "INSERT INTO t VALUES (1)");

        let frame = timeout(RECV_TIMEOUT, rx.recv()).await.unwrap().unwrap();
        let msg: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(msg["method"], "db.mutated");
        assert_eq!(msg["params"]["db"], "test");
    }

    #[tokio::test]
    async fn poller_broadcasts_schema_changed_on_external_ddl() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, mut rx) = mpsc::channel(8);
        subs.add("test".into(), None, tx);

        external_exec(file.path(), "CREATE INDEX i ON t(id)");

        // Order isn't guaranteed but both should arrive.
        let mut seen_mutated = false;
        let mut seen_schema = false;
        for _ in 0..2 {
            let frame = timeout(RECV_TIMEOUT, rx.recv()).await.unwrap().unwrap();
            let msg: Value = serde_json::from_str(&frame).unwrap();
            match msg["method"].as_str().unwrap() {
                "db.mutated" => seen_mutated = true,
                "db.schemaChanged" => seen_schema = true,
                other => panic!("unexpected method: {}", other),
            }
        }
        assert!(seen_mutated);
        assert!(seen_schema);
    }

    #[tokio::test]
    async fn poller_does_not_broadcast_for_own_writes() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg.clone());
        let (tx, mut rx) = mpsc::channel(8);
        subs.add("test".into(), None, tx);

        // Write via DbRegistry's own connection — data_version won't bump for its own reads.
        reg.execute("test", "INSERT INTO t VALUES (1)", None).unwrap();

        // Nothing should arrive within one poll cycle + slack.
        let result = timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(result.is_err(), "unexpected notification: {:?}", result);
    }

    #[tokio::test]
    async fn poller_stops_after_last_unsubscribe() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx, mut rx) = mpsc::channel(8);
        // Keep a tx clone alive so the channel doesn't close on unsubscribe —
        // otherwise rx.recv() would resolve immediately with None and defeat the timeout.
        let _tx_hold = tx.clone();
        let id = subs.add("test".into(), None, tx);

        subs.remove(&id);

        // Give abort() a moment to land, then write and confirm nothing arrives.
        tokio::time::sleep(Duration::from_millis(50)).await;
        external_exec(file.path(), "INSERT INTO t VALUES (1)");

        let result = timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(result.is_err(), "poller still running after unsubscribe");
    }

    #[tokio::test]
    async fn two_subscribers_both_receive_notification() {
        let (file, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let subs = MutationSubs::new(reg);
        let (tx1, mut rx1) = mpsc::channel(8);
        let (tx2, mut rx2) = mpsc::channel(8);
        subs.add("test".into(), None, tx1);
        subs.add("test".into(), None, tx2);

        external_exec(file.path(), "INSERT INTO t VALUES (1)");

        assert!(timeout(RECV_TIMEOUT, rx1.recv()).await.unwrap().is_some());
        assert!(timeout(RECV_TIMEOUT, rx2.recv()).await.unwrap().is_some());
    }

    // ============================================================
    // quote_ident
    // ============================================================

    #[test]
    fn identifier_quoting_handles_embedded_double_quotes() {
        assert_eq!(quote_ident("normal"), "\"normal\"");
        assert_eq!(quote_ident("has\"quote"), "\"has\"\"quote\"");
        assert_eq!(quote_ident("a\"b\"c"), "\"a\"\"b\"\"c\"");
    }

    // ============================================================
    // resolve_row_key
    // ============================================================

    #[test]
    fn resolve_row_key_returns_pk_for_declared_pk() {
        let (_f, _reg) = setup("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);", false);
        let c = Connection::open(_f.path()).unwrap();
        match resolve_row_key(&c, "t").unwrap() {
            RowKeyKind::Pk(cols) => assert_eq!(cols, vec!["id"]),
            _ => panic!("expected Pk"),
        }
    }

    #[test]
    fn resolve_row_key_returns_composite_pk_in_order() {
        let (_f, _reg) = setup(
            "CREATE TABLE t (b INTEGER, a INTEGER, PRIMARY KEY(a, b));",
            false,
        );
        let c = Connection::open(_f.path()).unwrap();
        match resolve_row_key(&c, "t").unwrap() {
            RowKeyKind::Pk(cols) => {
                // pk=1 → a, pk=2 → b
                assert_eq!(cols[0], "a");
                assert_eq!(cols[1], "b");
            }
            _ => panic!("expected Pk"),
        }
    }

    #[test]
    fn resolve_row_key_returns_rowid_for_no_pk_table() {
        let (_f, _reg) = setup("CREATE TABLE t (message TEXT, level TEXT);", false);
        let c = Connection::open(_f.path()).unwrap();
        match resolve_row_key(&c, "t").unwrap() {
            RowKeyKind::Rowid { alias } => assert_eq!(alias, "__rowid"),
            _ => panic!("expected Rowid"),
        }
    }

    #[test]
    fn resolve_row_key_uses_fallback_alias_when_column_named_rowid_exists() {
        let (_f, _reg) =
            setup("CREATE TABLE t (__rowid TEXT, name TEXT);", false);
        let c = Connection::open(_f.path()).unwrap();
        match resolve_row_key(&c, "t").unwrap() {
            RowKeyKind::Rowid { alias } => {
                assert_ne!(alias, "__rowid");
                assert!(alias.starts_with("__inspector_rowid_"));
            }
            _ => panic!("expected Rowid with fallback alias"),
        }
    }

    // ============================================================
    // db.tableRows
    // ============================================================

    #[test]
    fn table_rows_returns_rowkey_pk() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO t VALUES (1, 'a'), (2, 'b');",
            false,
        );
        let r = reg.table_rows("test", "t", None, None).unwrap();
        assert_eq!(r["rowKey"]["kind"], "pk");
        assert_eq!(r["rowKey"]["cols"], json!(["id"]));
        assert_eq!(r["rows"].as_array().unwrap().len(), 2);
        assert!(r["columns"].as_array().unwrap().iter().any(|c| c == "name"));
    }

    #[test]
    fn table_rows_prepends_rowid_alias_when_no_pk() {
        let (_f, reg) = setup(
            "CREATE TABLE t (message TEXT);
             INSERT INTO t VALUES ('hello');",
            false,
        );
        let r = reg.table_rows("test", "t", None, None).unwrap();
        assert_eq!(r["rowKey"]["kind"], "rowid");
        let alias = r["rowKey"]["alias"].as_str().unwrap();
        let cols = r["columns"].as_array().unwrap();
        assert_eq!(cols[0].as_str().unwrap(), alias);
    }

    #[test]
    fn table_rows_uses_fallback_alias_when_column_named_underscore_underscore_rowid_exists() {
        let (_f, reg) = setup(
            "CREATE TABLE t (__rowid TEXT, msg TEXT);
             INSERT INTO t VALUES ('x', 'hello');",
            false,
        );
        let r = reg.table_rows("test", "t", None, None).unwrap();
        assert_eq!(r["rowKey"]["kind"], "rowid");
        let alias = r["rowKey"]["alias"].as_str().unwrap();
        assert_ne!(alias, "__rowid");
        let cols = r["columns"].as_array().unwrap();
        assert_eq!(cols[0].as_str().unwrap(), alias);
    }

    #[test]
    fn table_rows_pagination_matches_query() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY);
             INSERT INTO t VALUES (1),(2),(3),(4),(5);",
            false,
        );
        let p1 = reg.table_rows("test", "t", Some(2), Some(0)).unwrap();
        assert_eq!(p1["rows"].as_array().unwrap().len(), 2);
        assert_eq!(p1["hasMore"], true);

        let p3 = reg.table_rows("test", "t", Some(2), Some(4)).unwrap();
        assert_eq!(p3["rows"].as_array().unwrap().len(), 1);
        assert_eq!(p3["hasMore"], false);
    }

    #[test]
    fn table_rows_respects_read_only_by_still_reading() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY);
             INSERT INTO t VALUES (42);",
            true,
        );
        let r = reg.table_rows("test", "t", None, None).unwrap();
        assert_eq!(r["rows"][0][0], 42);
    }

    // ============================================================
    // db.updateRow
    // ============================================================

    #[test]
    fn update_row_updates_target_only() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);
             INSERT INTO t VALUES (1, 'a'), (2, 'b');",
            false,
        );
        let r = reg
            .update_row("test", "t", &json!({"id": 1}), &json!({"val": "changed"}))
            .unwrap();
        assert_eq!(r["rowsAffected"], 1);
        // Row 2 untouched.
        let rows = reg.query("test", "SELECT val FROM t WHERE id=2", None, None, None).unwrap();
        assert_eq!(rows["rows"][0][0], "b");
    }

    #[test]
    fn update_row_composite_pk_targets_correct_row() {
        let (_f, reg) = setup(
            "CREATE TABLE t (a INTEGER, b INTEGER, val TEXT, PRIMARY KEY(a, b));
             INSERT INTO t VALUES (1, 1, 'aa'), (1, 2, 'ab');",
            false,
        );
        let r = reg
            .update_row(
                "test",
                "t",
                &json!({"a": 1, "b": 2}),
                &json!({"val": "updated"}),
            )
            .unwrap();
        assert_eq!(r["rowsAffected"], 1);
        let rows = reg
            .query("test", "SELECT val FROM t WHERE a=1 AND b=1", None, None, None)
            .unwrap();
        assert_eq!(rows["rows"][0][0], "aa");
    }

    #[test]
    fn update_row_zero_rows_when_pk_not_found_ok_with_zero() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);", false);
        let r = reg
            .update_row("test", "t", &json!({"id": 999}), &json!({"val": "x"}))
            .unwrap();
        assert_eq!(r["rowsAffected"], 0);
    }

    #[test]
    fn update_row_read_only_db_errors_with_read_only() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);
             INSERT INTO t VALUES (1, 'a');",
            true,
        );
        let err = reg
            .update_row("test", "t", &json!({"id": 1}), &json!({"val": "b"}))
            .unwrap_err();
        assert!(matches!(err, DbError::ReadOnly));
    }

    #[test]
    fn update_row_unknown_column_in_updates_returns_invalid_params() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);",
            false,
        );
        let err = reg
            .update_row(
                "test",
                "t",
                &json!({"id": 1}),
                &json!({"nonexistent": "x"}),
            )
            .unwrap_err();
        assert!(matches!(err, DbError::InvalidParams(_)));
    }

    #[test]
    fn update_row_missing_pk_column_returns_invalid_params() {
        let (_f, reg) = setup(
            "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY(a, b));",
            false,
        );
        // Only send 'a', missing 'b'
        let err = reg
            .update_row("test", "t", &json!({"a": 1}), &json!({"a": 2}))
            .unwrap_err();
        assert!(matches!(err, DbError::InvalidParams(_)));
    }

    #[test]
    fn update_row_null_value_writes_null() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);
             INSERT INTO t VALUES (1, 'x');",
            false,
        );
        reg.update_row("test", "t", &json!({"id": 1}), &json!({"val": null}))
            .unwrap();
        let rows = reg
            .query("test", "SELECT val FROM t WHERE id=1", None, None, None)
            .unwrap();
        assert_eq!(rows["rows"][0][0], Value::Null);
    }

    // ============================================================
    // db.insertRow
    // ============================================================

    #[test]
    fn insert_row_returns_last_insert_rowid() {
        let (_f, reg) =
            setup("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);", false);
        let r = reg
            .insert_row("test", "t", &json!({"id": 42, "val": "hello"}))
            .unwrap();
        assert_eq!(r["rowsAffected"], 1);
        assert_eq!(r["lastInsertRowid"], 42);
    }

    #[test]
    fn insert_row_empty_values_issues_default_values() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, val TEXT DEFAULT 'x');",
            false,
        );
        let r = reg.insert_row("test", "t", &json!({})).unwrap();
        assert_eq!(r["rowsAffected"], 1);
        let rows = reg.query("test", "SELECT val FROM t", None, None, None).unwrap();
        assert_eq!(rows["rows"][0][0], "x");
    }

    #[test]
    fn insert_row_missing_not_null_returns_sql_error() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT NOT NULL);",
            false,
        );
        let err = reg
            .insert_row("test", "t", &json!({"id": 1}))
            .unwrap_err();
        assert!(matches!(err, DbError::Sql(_)));
    }

    #[test]
    fn insert_row_read_only_errors() {
        let (_f, reg) =
            setup("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);", true);
        let err = reg
            .insert_row("test", "t", &json!({"id": 1, "val": "a"}))
            .unwrap_err();
        assert!(matches!(err, DbError::ReadOnly));
    }

    // ============================================================
    // db.deleteRow
    // ============================================================

    #[test]
    fn delete_row_deletes_target_only() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT);
             INSERT INTO t VALUES (1, 'a'), (2, 'b');",
            false,
        );
        let r = reg.delete_row("test", "t", &json!({"id": 1})).unwrap();
        assert_eq!(r["rowsAffected"], 1);
        let rows = reg.query("test", "SELECT * FROM t", None, None, None).unwrap();
        assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
        assert_eq!(rows["rows"][0][0], 2);
    }

    #[test]
    fn delete_row_missing_pk_column_returns_invalid_params() {
        let (_f, reg) = setup(
            "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY(a, b));
             INSERT INTO t VALUES (1, 1);",
            false,
        );
        let err = reg
            .delete_row("test", "t", &json!({"a": 1}))
            .unwrap_err();
        assert!(matches!(err, DbError::InvalidParams(_)));
    }

    // ============================================================
    // db.clearTable
    // ============================================================

    #[test]
    fn clear_table_deletes_all_rows_and_reports_count() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER);
             INSERT INTO t VALUES (1),(2),(3);",
            false,
        );
        let r = reg.clear_table("test", "t", "t").unwrap();
        assert_eq!(r["rowsAffected"], 3);
        let rows = reg.query("test", "SELECT * FROM t", None, None, None).unwrap();
        assert_eq!(rows["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn clear_table_confirm_mismatch_errors_with_invalid_params() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let err = reg.clear_table("test", "t", "wrong").unwrap_err();
        assert!(matches!(err, DbError::InvalidParams(_)));
    }

    #[test]
    fn clear_table_confirm_empty_errors() {
        let (_f, reg) = setup("CREATE TABLE t (id INTEGER);", false);
        let err = reg.clear_table("test", "t", "").unwrap_err();
        assert!(matches!(err, DbError::InvalidParams(_)));
    }

    #[test]
    fn clear_table_read_only_errors() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER);
             INSERT INTO t VALUES (1);",
            true,
        );
        let err = reg.clear_table("test", "t", "t").unwrap_err();
        assert!(matches!(err, DbError::ReadOnly));
    }

    // ============================================================
    // db.tableInfo — unique field on indexes
    // ============================================================

    #[test]
    fn table_info_index_entries_include_unique_field() {
        let (_f, reg) = setup(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, email TEXT, name TEXT);
             CREATE UNIQUE INDEX idx_email ON t(email);
             CREATE INDEX idx_name ON t(name);",
            false,
        );
        let info = reg.table_info("test", "t").unwrap();
        let indexes = info["indexes"].as_array().unwrap();

        let email_idx = indexes.iter().find(|i| i["name"] == "idx_email").unwrap();
        assert_eq!(email_idx["unique"], true);

        let name_idx = indexes.iter().find(|i| i["name"] == "idx_name").unwrap();
        assert_eq!(name_idx["unique"], false);
    }
}
