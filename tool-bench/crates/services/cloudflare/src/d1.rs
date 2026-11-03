//! D1 (`/accounts/{id}/d1/database`). Databases are keyed by uuid; each table is a
//! `d1_tables` document `<uuid>/<table>` holding its `CREATE TABLE` SQL and its rows.
//! Queries run on real SQLite (D1's engine): the tables are loaded into an in-memory
//! database, the statements run, and on any write the tables are written back.

use mock_core::world::WorldState;
use rusqlite::{
    Connection,
    fallible_iterator::FallibleIterator,
    types::{Value as Sql, ValueRef},
};
use serde_json::{Map, Value, json};

use crate::api::{
    ApiRequest, ApiResponse, ApiResult, account, all, fail, get, method_not_allowed, no_route, now,
    ok, paged, put, remove, str_field, uuid_id, values,
};

pub fn route(w: &mut WorldState, req: &ApiRequest, segs: &[&str]) -> ApiResult {
    let account_id = segs[1];
    account(w, req, account_id)?;
    let m = req.method.as_str();
    match &segs[2..] {
        ["d1", "database"] => match m {
            "GET" => {
                let items: Vec<Value> = databases(w, account_id)
                    .into_iter()
                    .filter(|d| {
                        req.q("name")
                            .is_none_or(|n| str_field(d, "name").contains(n))
                    })
                    .map(|d| list_view(&d))
                    .collect();
                paged(req, items, 1000, 10_000)
            }
            "POST" => create(w, req, account_id),
            _ => Err(method_not_allowed()),
        },
        ["d1", "database", id, rest @ ..] => {
            let Some(db) =
                get(w, "d1_databases", id).filter(|d| str_field(d, "account_id") == account_id)
            else {
                return Err(fail(
                    404,
                    &[(7404, &format!("The database {id} could not be found"))],
                ));
            };
            match (m, rest) {
                ("GET", []) => ok(detail_view(w, &db)),
                ("DELETE", []) => {
                    remove(w, "d1_databases", id);
                    for (key, t) in all(w, "d1_tables") {
                        if str_field(&t, "database_id") == *id {
                            remove(w, "d1_tables", &key);
                        }
                    }
                    ok(Value::Null)
                }
                ("POST", ["query"]) => query(w, req, id, false),
                ("POST", ["raw"]) => query(w, req, id, true),
                (_, [] | ["query"] | ["raw"]) => Err(method_not_allowed()),
                _ => Err(no_route(&req.path)),
            }
        }
        _ => Err(no_route(&req.path)),
    }
}

fn databases(w: &WorldState, account_id: &str) -> Vec<Value> {
    values(w, "d1_databases")
        .into_iter()
        .filter(|d| str_field(d, "account_id") == account_id)
        .collect()
}

fn tables(w: &WorldState, db_id: &str) -> Vec<Value> {
    values(w, "d1_tables")
        .into_iter()
        .filter(|t| str_field(t, "database_id") == db_id)
        .collect()
}

fn list_view(d: &Value) -> Value {
    json!({
        "uuid": d["uuid"],
        "name": d["name"],
        "version": d.get("version").cloned().unwrap_or(json!("production")),
        "created_at": d["created_at"],
    })
}

fn file_size(w: &WorldState, db_id: &str) -> usize {
    16_384
        + tables(w, db_id)
            .iter()
            .map(|t| 4096 + t["rows"].to_string().len())
            .sum::<usize>()
}

fn detail_view(w: &WorldState, d: &Value) -> Value {
    let id = str_field(d, "uuid");
    json!({
        "uuid": d["uuid"],
        "name": d["name"],
        "version": d.get("version").cloned().unwrap_or(json!("production")),
        "created_at": d["created_at"],
        "file_size": file_size(w, id),
        "num_tables": tables(w, id).len(),
        "running_in_region": d.get("running_in_region").cloned().unwrap_or(json!("ENAM")),
        "read_replication": { "mode": "disabled" },
    })
}

fn create(w: &mut WorldState, req: &ApiRequest, account_id: &str) -> ApiResult {
    let body = req.json_object()?;
    let Some(name) = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    else {
        return Err(fail(
            400,
            &[(7400, "The request is malformed: name is required")],
        ));
    };
    if databases(w, account_id)
        .iter()
        .any(|d| str_field(d, "name") == name)
    {
        return Err(fail(
            400,
            &[(7502, "A database with that name already exists")],
        ));
    }
    let uuid = uuid_id(w, &["d1", account_id, name]);
    let doc = json!({
        "uuid": uuid,
        "account_id": account_id,
        "name": name,
        "version": "production",
        "created_at": now(w),
        "running_in_region": body.get("primary_location_hint").and_then(Value::as_str).map(str::to_uppercase).unwrap_or_else(|| "ENAM".to_owned()),
    });
    put(w, "d1_databases", &uuid, doc.clone());
    ok(detail_view(w, &doc))
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map(Sql::Integer)
            .unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

fn from_sql(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(b),
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn sql_error(e: &rusqlite::Error) -> ApiResponse {
    let message = match e {
        rusqlite::Error::SqliteFailure(f, msg) => {
            let code = match f.code {
                rusqlite::ErrorCode::ConstraintViolation => "SQLITE_CONSTRAINT",
                rusqlite::ErrorCode::TooBig => "SQLITE_TOOBIG",
                rusqlite::ErrorCode::ReadOnly => "SQLITE_READONLY",
                _ => "SQLITE_ERROR",
            };
            format!("{}: {code}", msg.clone().unwrap_or_else(|| f.to_string()))
        }
        rusqlite::Error::InvalidParameterCount(..) => {
            "Wrong number of parameter bindings for SQL query.".to_owned()
        }
        other => format!("{other}: SQLITE_ERROR"),
    };
    fail(400, &[(7500, &message)])
}

/// Replaces SQLite's `random()`/`randomblob()` with a generator seeded from the logical
/// clock, so queries never read real randomness.
fn deterministic_functions(conn: &Connection, seed: i64) -> Result<(), rusqlite::Error> {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use rusqlite::functions::FunctionFlags;
    let state = Arc::new(AtomicU64::new((seed as u64) ^ 0x9E37_79B9_7F4A_7C15));
    let next = move |state: &AtomicU64| {
        let mut x = state.load(Ordering::Relaxed).max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state.store(x, Ordering::Relaxed);
        x
    };
    let (s1, s2) = (state.clone(), state);
    conn.create_scalar_function("random", 0, FunctionFlags::SQLITE_UTF8, move |_| {
        Ok(next(&s1) as i64)
    })?;
    conn.create_scalar_function("randomblob", 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
        let n = ctx.get::<i64>(0)?.clamp(1, 1 << 20) as usize;
        Ok((0..n).map(|_| next(&s2) as u8).collect::<Vec<u8>>())
    })?;
    Ok(())
}

/// Pins SQLite's notion of "now" (`'now'` in date functions and the CURRENT_* keywords)
/// to the logical clock. Text inside other string literals is left alone.
fn freeze_clock(sql: &str, now: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while !rest.is_empty() {
        if rest.starts_with('\'') {
            let end = rest[1..].find('\'').map_or(rest.len(), |i| i + 2);
            let literal = &rest[..end];
            if literal.eq_ignore_ascii_case("'now'") {
                out.push_str(&format!("'{now}'"));
            } else {
                out.push_str(literal);
            }
            rest = &rest[end..];
            continue;
        }
        let upper = rest.to_ascii_uppercase();
        let keyword = ["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"]
            .into_iter()
            .find(|k| {
                upper.starts_with(k)
                    && !rest[k.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            });
        let at_boundary = !out.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
        match keyword {
            Some(k) if at_boundary => {
                let (date, time) = now.split_once(' ').unwrap_or((now, "00:00:00"));
                let value = match k {
                    "CURRENT_DATE" => date.to_owned(),
                    "CURRENT_TIME" => time.to_owned(),
                    _ => now.to_owned(),
                };
                out.push_str(&format!("'{value}'"));
                rest = &rest[k.len()..];
            }
            _ => {
                let ch = rest.chars().next().expect("non-empty");
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    out
}

/// Loads a database's tables into a fresh in-memory SQLite.
fn load(w: &WorldState, db: &Value) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_in_memory()?;
    deterministic_functions(&conn, w.clock.now_unix())?;
    for t in tables(w, str_field(db, "uuid")) {
        conn.execute_batch(str_field(&t, "sql"))?;
        for row in t
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(obj) = row.as_object() else { continue };
            let cols: Vec<String> = obj.keys().map(|k| quote(k)).collect();
            let marks: Vec<String> = (1..=obj.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote(str_field(&t, "name")),
                cols.join(", "),
                marks.join(", ")
            );
            let params: Vec<Sql> = obj.values().map(to_sql).collect();
            conn.execute(&sql, rusqlite::params_from_iter(params))?;
        }
    }
    for extra in db
        .get("extra_schema")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(sql) = extra.as_str() {
            conn.execute_batch(sql)?;
        }
    }
    Ok(conn)
}

/// Writes every table back into the store after a write.
fn save(w: &mut WorldState, mut db: Value, conn: &Connection) -> Result<(), rusqlite::Error> {
    let db_id = str_field(&db, "uuid").to_owned();
    let mut schema = conn.prepare(
        "SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY rowid",
    )?;
    let entries: Vec<(String, String, String)> = schema
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut new_tables = Vec::new();
    let mut extra = Vec::new();
    for (kind, name, sql) in entries {
        if kind != "table" {
            extra.push(json!(sql));
            continue;
        }
        let mut stmt = conn.prepare(&format!("SELECT * FROM {}", quote(&name)))?;
        let cols: Vec<String> = stmt.column_names().map(str::to_owned).collect();
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut obj = Map::new();
            for (i, c) in cols.iter().enumerate() {
                obj.insert(c.clone(), from_sql(row.get_ref(i)?));
            }
            out.push(Value::Object(obj));
        }
        new_tables.push((
            name.clone(),
            json!({ "database_id": db_id, "name": name, "sql": sql, "rows": out }),
        ));
    }
    for (key, t) in all(w, "d1_tables") {
        if str_field(&t, "database_id") == db_id {
            remove(w, "d1_tables", &key);
        }
    }
    for (name, doc) in new_tables {
        put(w, "d1_tables", &format!("{db_id}/{name}"), doc);
    }
    if extra.is_empty() {
        if let Some(o) = db.as_object_mut() {
            o.shift_remove("extra_schema");
        }
    } else {
        db["extra_schema"] = Value::Array(extra);
    }
    put(w, "d1_databases", &db_id, db);
    Ok(())
}

fn statements(req: &ApiRequest) -> Result<Vec<(String, Vec<Value>)>, ApiResponse> {
    let body = req.json()?;
    let one = |v: &Value| -> Option<(String, Vec<Value>)> {
        let sql = v.get("sql")?.as_str()?.to_owned();
        let params = v
            .get("params")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Some((sql, params))
    };
    let list = match &body {
        Value::Object(o) if o.contains_key("batch") => o["batch"]
            .as_array()
            .map(|b| b.iter().filter_map(one).collect::<Vec<_>>())
            .unwrap_or_default(),
        Value::Array(items) => items.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    };
    if list.is_empty() {
        return Err(fail(
            400,
            &[(7400, "The request is malformed: sql is required")],
        ));
    }
    Ok(list)
}

fn query(w: &mut WorldState, req: &ApiRequest, db_id: &str, raw: bool) -> ApiResult {
    let db = get(w, "d1_databases", db_id).unwrap_or(Value::Null);
    let stmts = statements(req)?;
    let conn = load(w, &db).map_err(|e| sql_error(&e))?;
    let mut results = Vec::new();
    let mut wrote = false;
    let sql_now = w
        .clock
        .now_rfc3339()
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_owned();
    for (sql, params) in &stmts {
        let sql = freeze_clock(sql, &sql_now);
        let mut batch = rusqlite::Batch::new(&conn, &sql);
        let mut any = false;
        loop {
            let next = batch.next().map_err(|e| sql_error(&e))?;
            let Some(mut stmt) = next else { break };
            any = true;
            if !params.is_empty() {
                if params.len() != stmt.parameter_count() {
                    return Err(fail(
                        400,
                        &[(7500, "Wrong number of parameter bindings for SQL query.")],
                    ));
                }
                for (i, p) in params.iter().enumerate() {
                    stmt.raw_bind_parameter(i + 1, to_sql(p))
                        .map_err(|e| sql_error(&e))?;
                }
            }
            let readonly = stmt.readonly();
            wrote |= !readonly;
            let cols: Vec<String> = stmt.column_names().map(str::to_owned).collect();
            let before = conn.total_changes();
            let mut rows_out = Vec::new();
            {
                let mut rows = stmt.raw_query();
                while let Some(row) = rows.next().map_err(|e| sql_error(&e))? {
                    let vals: Vec<Value> = (0..cols.len())
                        .map(|i| row.get_ref(i).map(from_sql).unwrap_or(Value::Null))
                        .collect();
                    rows_out.push(vals);
                }
            }
            let changes = if readonly {
                0
            } else {
                conn.total_changes() - before
            };
            let rows_read = rows_out.len() as u64 + changes;
            let result_rows = if raw {
                json!({ "columns": cols, "rows": rows_out })
            } else {
                Value::Array(
                    rows_out
                        .iter()
                        .map(|vals| {
                            Value::Object(cols.iter().cloned().zip(vals.iter().cloned()).collect())
                        })
                        .collect(),
                )
            };
            results.push(json!({
                "results": result_rows,
                "success": true,
                "meta": {
                    "served_by": "v3-prod",
                    "served_by_region": db.get("running_in_region").cloned().unwrap_or(json!("ENAM")),
                    "served_by_primary": true,
                    "timings": { "sql_duration_ms": 0.25 },
                    "duration": 0.25,
                    "changes": changes,
                    "last_row_id": conn.last_insert_rowid(),
                    "changed_db": !readonly,
                    "size_after": 0,
                    "rows_read": rows_read,
                    "rows_written": changes,
                },
            }));
        }
        if !any {
            return Err(fail(400, &[(7500, "No SQL statements detected.")]));
        }
    }
    if wrote {
        save(w, db, &conn).map_err(|e| sql_error(&e))?;
    }
    let size = file_size(w, db_id);
    for r in &mut results {
        r["meta"]["size_after"] = json!(size);
    }
    ok(Value::Array(results))
}

#[cfg(test)]
mod determinism_tests {
    use super::*;

    #[test]
    fn freezes_now_and_current_keywords_but_not_other_literals() {
        let sql = "SELECT datetime('now'), date('NOW','-1 day'), CURRENT_TIMESTAMP, current_date, 'now is the time', my_current_date FROM t";
        assert_eq!(
            freeze_clock(sql, "2026-03-14 09:00:00"),
            "SELECT datetime('2026-03-14 09:00:00'), date('2026-03-14 09:00:00','-1 day'), '2026-03-14 09:00:00', '2026-03-14', 'now is the time', my_current_date FROM t"
        );
    }

    #[test]
    fn random_is_seeded() {
        let draw = |seed| {
            let conn = Connection::open_in_memory().unwrap();
            deterministic_functions(&conn, seed).unwrap();
            conn.query_row("SELECT random(), hex(randomblob(4))", [], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
        };
        assert_eq!(draw(1_773_478_800), draw(1_773_478_800));
        assert_ne!(draw(1_773_478_800), draw(1_773_478_801));
    }
}
