//! Connection and database tools (`connect`, `find`, `aggregate`, `insert-many`, ...).

use serde_json::{Map, Value, json};

use crate::agg::{self, Source, WriteStage};
use crate::ejson::{self, bson_size, id_display, id_key, normalize, shell_repr};
use crate::query;
use crate::update;
use crate::{Ctx, error_running, fail, fail_blocks, ok, str_arg, text};

pub const PRECONFIGURED: &str = "preconfigured";
const MAX_DOCUMENTS_PER_QUERY: usize = 100;
const MAX_BYTES_PER_QUERY: usize = 16_777_216;
const ONE_MB: usize = 1_048_576;
const SYSTEM_DBS: [&str; 3] = ["admin", "config", "local"];
pub const DEFAULT_HOST: &str = "ac-octo-shard-00-00.x7kq2.mongodb.net";

const CONNECT_HINT: &str = "Please use one of the following tools: \"atlas-connect-cluster\", \"connect\" to connect to a MongoDB instance or update the MCP server configuration to include a connection string. Note to LLM: prefer using the \"atlas-connect-cluster\" tool to connect to an Atlas cluster over using a connection string. Make sure to ask the user to specify a cluster name they want to connect to or ask them if they want to use the \"list-clusters\" tool to list all their clusters. Do not invent cluster names or connection strings unless the user has explicitly specified them. If they've previously connected to MongoDB using MCP, you can ask them if they want to reconnect using the same cluster/connection.";

const LIMIT_TEXT_MAX_DOCS: &str = "the server's configured maximum number of documents";
const LIMIT_TEXT_MAX_BYTES: &str = "the server's configured maximum response size";
const LIMIT_TEXT_TOOL_BYTES: &str = "the responseBytesLimit parameter";

pub fn data_coll(db: &str, coll: &str) -> String {
    format!("db.{db}.{coll}")
}

fn ns_key(db: &str, coll: &str) -> String {
    format!("{db}.{coll}")
}

fn id_index() -> Value {
    json!({"v": 2, "key": {"_id": 1}, "name": "_id_"})
}

/// Seeds connections and the namespace catalog the first time any tool runs.
pub fn ensure_initialized(ctx: &mut Ctx) {
    if ctx.coll("_meta").is_some_and(|c| c.contains("init")) {
        return;
    }
    let snapshot = ctx.svc.store.snapshot();
    let mut found: Vec<(String, String)> = Vec::new();
    if let Some(m) = snapshot.as_object() {
        for name in m.keys() {
            if let Some(rest) = name.strip_prefix("db.")
                && let Some((db, coll)) = rest.split_once('.')
            {
                found.push((db.to_owned(), coll.to_owned()));
            }
        }
    }
    let existing = ctx.coll_mut("namespaces").clone();
    let mut namespaces = mock_core::store::Collection::default();
    let mut add = |key: String, db: &str, coll: &str, base: Option<&Value>| {
        let mut doc = base.cloned().unwrap_or_else(|| json!({}));
        doc["database"] = json!(db);
        doc["collection"] = json!(coll);
        let mut indexes: Vec<Value> = doc
            .get("indexes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !indexes.iter().any(|i| i["name"] == "_id_") {
            indexes.insert(0, id_index());
        }
        for idx in &mut indexes {
            if idx.get("v").is_none() {
                let mut with_v = Map::new();
                with_v.insert("v".to_owned(), json!(2));
                for (k, v) in idx.as_object().into_iter().flatten() {
                    with_v.insert(k.clone(), v.clone());
                }
                *idx = Value::Object(with_v);
            }
        }
        doc["indexes"] = Value::Array(indexes);
        if doc.get("searchIndexes").is_none() {
            doc["searchIndexes"] = json!([]);
        }
        namespaces.insert(key, doc);
    };
    for (key, doc) in existing.iter() {
        let (db, coll) = key.split_once('.').unwrap_or((key, ""));
        add(key.to_owned(), db, coll, Some(doc));
    }
    for (db, coll) in &found {
        let key = ns_key(db, coll);
        if !existing.contains(&key) {
            add(key, db, coll, None);
        }
    }
    *ctx.coll_mut("namespaces") = namespaces;
    let created = ctx.now_iso();
    if !ctx
        .coll("connections")
        .is_some_and(|c| c.contains(PRECONFIGURED))
    {
        let host = deployment_host(ctx);
        ctx.coll_mut("connections").insert(
            PRECONFIGURED,
            json!({
                "connectionId": PRECONFIGURED,
                "name": PRECONFIGURED,
                "source": "preconfigured",
                "state": "disconnected",
                "connectionString": format!("mongodb+srv://{host}/"),
                "createdAt": created,
                "lastUsedAt": created,
            }),
        );
    }
    ctx.coll_mut("_meta")
        .insert("init", json!({ "initializedAt": created }));
}

fn deployment(ctx: &Ctx) -> Value {
    ctx.coll("deployment")
        .and_then(|c| c.get("deployment"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

pub fn deployment_host(ctx: &Ctx) -> String {
    deployment(ctx)["hosts"][0]
        .as_str()
        .unwrap_or(DEFAULT_HOST)
        .to_owned()
}

fn server_version(ctx: &Ctx) -> String {
    deployment(ctx)["version"]
        .as_str()
        .unwrap_or("8.0.15")
        .to_owned()
}

pub fn client_ip(ctx: &Ctx) -> String {
    deployment(ctx)["clientIp"]
        .as_str()
        .unwrap_or("198.51.100.23")
        .to_owned()
}

/// Lowercase alphanumerics joined by single hyphens, max 40 characters.
pub fn slugify(value: &str) -> String {
    let mut out = String::new();
    for c in value.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let mut s: String = trimmed.chars().take(40).collect();
    while s.ends_with('-') {
        s.pop();
    }
    s
}

pub fn suffix(ctx: &mut Ctx) -> String {
    let id = ctx.svc.ids.next_uuid("connection-suffix");
    format!("{:02x}{:02x}", id.as_bytes()[0], id.as_bytes()[1])
}

fn connection_hint_blocks(first: String) -> Value {
    fail_blocks(vec![text(first), text(CONNECT_HINT)])
}

/// Resolves `connectionId` to a live connection or returns the server's error result.
fn resolve(ctx: &mut Ctx, args: &Value) -> Result<(), Value> {
    let id = str_arg(args, "connectionId").unwrap_or_default().to_owned();
    let now = ctx.now_iso();
    let Some(conn) = ctx.coll_mut("connections").get_mut(&id) else {
        return Err(connection_hint_blocks(format!(
            "Connection \"{id}\" does not exist or has expired. Call the \"list-connections\" tool to see the active connections, or establish a new one and retry with the connectionId it returns."
        )));
    };
    conn["lastUsedAt"] = json!(now);
    match conn["state"].as_str() {
        Some("connecting") | Some("errored") => Err(connection_hint_blocks(
            "You need to connect to a MongoDB instance before you can access its data.".to_owned(),
        )),
        Some("disconnected") if conn["source"] == "explicit" => Err(connection_hint_blocks(
            "You need to connect to a MongoDB instance before you can access its data.".to_owned(),
        )),
        _ => {
            conn["state"] = json!("connected");
            Ok(())
        }
    }
}

fn namespace_exists(ctx: &Ctx, db: &str, coll: &str) -> bool {
    ctx.coll("namespaces")
        .is_some_and(|c| c.contains(&ns_key(db, coll)))
}

fn ensure_namespace(ctx: &mut Ctx, db: &str, coll: &str) {
    if !namespace_exists(ctx, db, coll) {
        ctx.coll_mut("namespaces").insert(
            ns_key(db, coll),
            json!({"database": db, "collection": coll, "indexes": [id_index()], "searchIndexes": []}),
        );
        ctx.coll_mut(&data_coll(db, coll));
    }
}

fn collections_of(ctx: &Ctx, db: &str) -> Vec<String> {
    ctx.coll("namespaces")
        .map(|c| {
            c.values()
                .filter(|v| v["database"] == db)
                .filter_map(|v| v["collection"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn databases(ctx: &Ctx) -> Vec<String> {
    let mut dbs: Vec<String> = ctx
        .coll("namespaces")
        .map(|c| {
            c.values()
                .filter_map(|v| v["database"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for s in SYSTEM_DBS {
        dbs.push(s.to_owned());
    }
    dbs.sort();
    dbs.dedup();
    dbs
}

struct DbSource<'a> {
    ctx: &'a Ctx<'a>,
    db: String,
}

impl Source for DbSource<'_> {
    fn docs(&self, collection: &str) -> Vec<Value> {
        self.ctx.docs(&data_coll(&self.db, collection))
    }
    fn now_ms(&self) -> i64 {
        self.ctx.now * 1000
    }
}

fn run_pipeline(
    ctx: &Ctx,
    db: &str,
    coll: Option<&str>,
    pipeline: &[Value],
) -> Result<(Vec<Value>, Option<WriteStage>), String> {
    let src = DbSource {
        ctx,
        db: db.to_owned(),
    };
    let input = coll
        .map(|c| ctx.docs(&data_coll(db, c)))
        .unwrap_or_default();
    agg::run(input, pipeline, &src)
}

fn arg_obj(args: &Value, key: &str) -> Value {
    match args.get(key) {
        Some(v) if !v.is_null() => normalize(v.clone()),
        _ => json!({}),
    }
}

fn indexes_of(ctx: &Ctx, db: &str, coll: &str) -> Vec<Value> {
    ctx.coll("namespaces")
        .and_then(|c| c.get(&ns_key(db, coll)))
        .and_then(|v| v["indexes"].as_array().cloned())
        .unwrap_or_default()
}

/// E11000 check against unique indexes (and `_id`).
fn duplicate_key(
    ctx: &Ctx,
    db: &str,
    coll: &str,
    doc: &Value,
    skip_key: Option<&str>,
) -> Option<String> {
    let data = ctx.coll(&data_coll(db, coll))?;
    for idx in indexes_of(ctx, db, coll) {
        let unique = idx["name"] == "_id_" || idx["unique"] == true;
        if !unique {
            continue;
        }
        let Some(keys) = idx["key"].as_object() else {
            continue;
        };
        let fields: Vec<&String> = keys.keys().collect();
        let mine: Vec<Value> = fields
            .iter()
            .map(|f| query::get_path(doc, f).cloned().unwrap_or(Value::Null))
            .collect();
        if idx["sparse"] == true && mine.iter().all(Value::is_null) {
            continue;
        }
        let clash = data.iter().any(|(k, other)| {
            Some(k) != skip_key
                && fields.iter().zip(&mine).all(|(f, v)| {
                    ejson::eq_values(query::get_path(other, f).unwrap_or(&Value::Null), v)
                })
        });
        if clash {
            let dup: Vec<String> = fields
                .iter()
                .zip(&mine)
                .map(|(f, v)| format!("{f}: {}", shell_repr(v)))
                .collect();
            return Some(format!(
                "E11000 duplicate key error collection: {db}.{coll} index: {} dup key: {{ {} }}",
                idx["name"].as_str().unwrap_or(""),
                dup.join(", ")
            ));
        }
    }
    None
}

/// Collects documents until the byte budget is reached, like `collectCursorUntilMaxBytesLimit`.
fn cap_bytes(docs: Vec<Value>, tool_limit: Option<f64>) -> (Vec<Value>, Option<&'static str>) {
    let (limit, capped_by) = match tool_limit {
        Some(l) if l > 0.0 => {
            let l = l as usize;
            if MAX_BYTES_PER_QUERY < l {
                (MAX_BYTES_PER_QUERY, LIMIT_TEXT_MAX_BYTES)
            } else {
                (l, LIMIT_TEXT_TOOL_BYTES)
            }
        }
        _ => (MAX_BYTES_PER_QUERY, LIMIT_TEXT_MAX_BYTES),
    };
    let mut total = 0;
    let mut out = Vec::new();
    for d in docs {
        let size = bson_size(&d);
        if total + size >= limit {
            return (out, Some(capped_by));
        }
        total += size;
        out.push(d);
    }
    (out, None)
}

fn limit_key(text: &str) -> &'static str {
    match text {
        LIMIT_TEXT_MAX_DOCS => "config.maxDocumentsPerQuery",
        LIMIT_TEXT_MAX_BYTES => "config.maxBytesPerQuery",
        _ => "tool.responseBytesLimit",
    }
}

pub fn call(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    match name {
        "connect" => return connect(ctx, args),
        "list-connections" => return list_connections(ctx),
        "disconnect" => return disconnect(ctx, args),
        _ => {}
    }
    if let Err(e) = resolve(ctx, args) {
        return e;
    }
    let db = str_arg(args, "database").unwrap_or_default().to_owned();
    let coll = str_arg(args, "collection").unwrap_or_default().to_owned();
    let result = match name {
        "find" => find(ctx, &db, &coll, args),
        "aggregate" => aggregate(ctx, &db, Some(&coll), args),
        "aggregate-db" => aggregate(ctx, &db, None, args),
        "count" => count(ctx, &db, &coll, args),
        "insert-many" => insert_many(ctx, &db, &coll, args),
        "update-many" => update_many(ctx, &db, &coll, args),
        "delete-many" => delete_many(ctx, &db, &coll, args),
        "create-collection" => create_collection(ctx, &db, &coll),
        "drop-collection" => drop_collection(ctx, &db, &coll),
        "drop-database" => drop_database(ctx, &db),
        "rename-collection" => rename_collection(ctx, &db, &coll, args),
        "list-databases" => list_databases(ctx),
        "list-collections" => list_collections(ctx, &db),
        "collection-indexes" => collection_indexes(ctx, &db, &coll),
        "create-index" => create_index(ctx, &db, &coll, args),
        "drop-index" => drop_index(ctx, &db, &coll, args),
        "collection-schema" => collection_schema(ctx, &db, &coll, args),
        "collection-storage-size" => storage_size(ctx, &db, &coll),
        "db-stats" => db_stats(ctx, &db),
        "explain" => explain(ctx, &db, &coll, args),
        "export" => export(ctx, &db, &coll, args),
        "mongodb-logs" => logs(ctx, args),
        _ => Err(format!("{name} is not implemented")),
    };
    result.unwrap_or_else(|e| error_running(name, &e))
}

fn connect(ctx: &mut Ctx, args: &Value) -> Value {
    let cs = str_arg(args, "connectionString")
        .unwrap_or_default()
        .to_owned();
    if str_arg(args, "connectionName") == Some(PRECONFIGURED) {
        return fail(
            "MCP error -32602: Input validation error: Invalid arguments for tool connect: [\n  {\n    \"code\": \"custom\",\n    \"path\": [\n      \"connectionName\"\n    ],\n    \"message\": \"\\\"preconfigured\\\" is a reserved connection name\"\n  }\n]",
        );
    }
    let rest = if let Some(r) = cs.strip_prefix("mongodb+srv://") {
        (r, true)
    } else if let Some(r) = cs.strip_prefix("mongodb://") {
        (r, false)
    } else {
        return error_running(
            "connect",
            "Invalid scheme, expected connection string to start with \"mongodb://\" or \"mongodb+srv://\"",
        );
    };
    let (authority, srv) = rest;
    let hosts_part = authority.split('/').next().unwrap_or("");
    let hosts_part = hosts_part.rsplit('@').next().unwrap_or("");
    let hosts: Vec<&str> = hosts_part.split(',').filter(|h| !h.is_empty()).collect();
    let Some(first) = hosts
        .first()
        .map(|h| h.split(':').next().unwrap_or(*h).to_owned())
    else {
        return error_running("connect", "Invalid connection string \"<mongodb uri>\"");
    };
    let known: Vec<String> = deployment(ctx)["hosts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let cluster_hosts: Vec<String> = ctx
        .docs("clusters")
        .iter()
        .flat_map(|c| {
            ["standard", "standardSrv"]
                .iter()
                .filter_map(|k| c["connectionStrings"][*k].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect();
    let reachable = known.is_empty() && cluster_hosts.is_empty()
        || known.iter().any(|h| {
            hosts
                .iter()
                .any(|x| x.split(':').next() == Some(h.as_str()))
        })
        || cluster_hosts.iter().any(|c| c.contains(&first))
        || first == "localhost"
        || first == "127.0.0.1";
    if !reachable {
        let msg = if srv {
            format!("querySrv ENOTFOUND _mongodb._tcp.{first}")
        } else {
            format!("getaddrinfo ENOTFOUND {first}")
        };
        return error_running("connect", &msg);
    }
    let label = str_arg(args, "connectionName").map_or(first.clone(), str::to_owned);
    let slug = slugify(&label);
    let slug = if slug.is_empty() {
        "connection".to_owned()
    } else {
        slug
    };
    let entry_name = format!("{slug}-{}", suffix(ctx));
    let id = ctx.uuid("connection");
    let now = ctx.now_iso();
    let host_type = if first.ends_with(".mongodb.net") {
        "atlas"
    } else if first == "localhost" || first == "127.0.0.1" {
        "local"
    } else {
        "unknown"
    };
    ctx.coll_mut("connections").insert(
        id.clone(),
        json!({
            "connectionId": id,
            "name": entry_name,
            "source": "explicit",
            "state": "connected",
            "description": format!("MongoDB connection (host type: {host_type}, auth: scram)"),
            "createdAt": now,
            "lastUsedAt": now,
        }),
    );
    ok(
        vec![text(format!(
            "Successfully connected to MongoDB. Your connectionId is \"{id}\" — pass it as the connectionId argument to all MongoDB tool calls that should run against this connection."
        ))],
        Some(json!({ "connectionId": id })),
    )
}

pub fn describe_connection(ctx: &Ctx, conn: &Value) -> String {
    if let Some(a) = conn.get("atlasCluster") {
        return format!(
            "Atlas cluster \"{}\" (project {})",
            a["clusterName"].as_str().unwrap_or(""),
            a["projectId"].as_str().unwrap_or("")
        );
    }
    if conn["source"] == "preconfigured" {
        if conn["state"] == "disconnected" {
            return "Configured connection string (not yet dialed)".to_owned();
        }
        let host = deployment_host(ctx);
        let host_type = if host.ends_with(".mongodb.net") {
            "atlas"
        } else {
            "unknown"
        };
        return format!("MongoDB connection (host type: {host_type}, auth: scram)");
    }
    conn["description"]
        .as_str()
        .unwrap_or("MongoDB connection")
        .to_owned()
}

fn list_connections(ctx: &mut Ctx) -> Value {
    let conns: Vec<Value> = ctx
        .docs("connections")
        .iter()
        .map(|c| {
            json!({
                "connectionId": c["connectionId"],
                "name": c["name"],
                "source": c["source"],
                "state": c["state"],
                "description": describe_connection(ctx, c),
                "createdAt": c["createdAt"],
                "lastUsedAt": c["lastUsedAt"],
            })
        })
        .collect();
    let body = if conns.is_empty() {
        "There are no active connections. Use one of the following tools to establish one: \"atlas-connect-cluster\", \"connect\".".to_owned()
    } else {
        format!(
            "Active connections:\n{}",
            conns
                .iter()
                .map(|c| format!(
                    "- \"{}\" (connectionId: \"{}\", {}): {}",
                    c["name"].as_str().unwrap_or(""),
                    c["connectionId"].as_str().unwrap_or(""),
                    c["state"].as_str().unwrap_or("unknown"),
                    c["description"].as_str().unwrap_or("")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    ok(vec![text(body)], Some(json!({ "connections": conns })))
}

fn disconnect(ctx: &mut Ctx, args: &Value) -> Value {
    let id = str_arg(args, "connectionId").unwrap_or_default().to_owned();
    if !ctx.coll("connections").is_some_and(|c| c.contains(&id)) {
        return connection_hint_blocks(format!(
            "Connection \"{id}\" does not exist or has expired. Call the \"list-connections\" tool to see the active connections, or establish a new one and retry with the connectionId it returns."
        ));
    }
    if id == PRECONFIGURED {
        if let Some(c) = ctx.coll_mut("connections").get_mut(&id) {
            c["state"] = json!("disconnected");
        }
        return ok(
            vec![text(
                "Closed the \"preconfigured\" connection. It remains available and will reconnect automatically on next use.",
            )],
            Some(json!({ "outcome": "disconnected" })),
        );
    }
    let removed = ctx.coll_mut("connections").remove(&id);
    if let Some(user) = removed
        .as_ref()
        .and_then(|c| c.get("temporaryUser"))
        .and_then(Value::as_str)
    {
        ctx.coll_mut("db_users").remove(user);
    }
    ok(
        vec![text(format!(
            "Disconnected. The connectionId \"{id}\" is no longer valid; use the connect tools to establish a new connection if needed."
        ))],
        Some(json!({ "outcome": "removed" })),
    )
}

fn find(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let filter = arg_obj(args, "filter");
    let projection = arg_obj(args, "projection");
    let sort = arg_obj(args, "sort");
    let limit = args
        .get("limit")
        .and_then(Value::as_f64)
        .unwrap_or(10.0)
        .abs() as usize;
    let mut matched = Vec::new();
    for d in ctx.docs(&data_coll(db, coll)) {
        if query::matches(&d, &filter)? {
            matched.push(d);
        }
    }
    let total = matched.len();
    query::sort_docs(&mut matched, &sort)?;
    let (effective, mut limits) = if limit == 0 {
        (usize::MAX, Vec::new())
    } else if MAX_DOCUMENTS_PER_QUERY < limit {
        (MAX_DOCUMENTS_PER_QUERY, vec![LIMIT_TEXT_MAX_DOCS])
    } else {
        (limit, Vec::new())
    };
    let projected: Vec<Value> = matched
        .into_iter()
        .take(effective)
        .map(|d| query::project(&d, &projection))
        .collect::<Result<_, _>>()?;
    let (docs, capped) = cap_bytes(
        projected,
        args.get("responseBytesLimit")
            .and_then(Value::as_f64)
            .or(Some(ONE_MB as f64)),
    );
    if let Some(c) = capped {
        limits.push(c);
    }
    let mut message = format!(
        "Query on collection \"{coll}\" resulted in {total} documents. Returning {} documents",
        docs.len()
    );
    if limits.is_empty() {
        message.push('.');
    } else {
        message.push_str(&format!(" while respecting the applied limits of {}. If the entire query result is required, use the \"export\" tool to retrieve the full result set.", limits.join(", ")));
    }
    let data = if docs.is_empty() {
        vec![]
    } else {
        vec![Value::Array(docs.clone()).to_string()]
    };
    let content = ctx.untrusted(&message, data);
    Ok(ok(
        content,
        Some(json!({
            "documents": docs,
            "queryResultsCount": total,
            "appliedLimits": limits.iter().map(|l| limit_key(l)).collect::<Vec<_>>(),
        })),
    ))
}

fn search_stage(pipeline: &[Value]) -> bool {
    pipeline.iter().any(|s| {
        s.get("$search").is_some()
            || s.get("$vectorSearch").is_some()
            || s.get("$searchMeta").is_some()
    })
}

const SEARCH_UNSUPPORTED: &str = "The connected MongoDB deployment does not support vector search indexes. Either connect to a MongoDB Atlas cluster or use the Atlas CLI to create and manage a local Atlas deployment.";

fn aggregate(ctx: &mut Ctx, db: &str, coll: Option<&str>, args: &Value) -> Result<Value, String> {
    let pipeline: Vec<Value> = args
        .get("pipeline")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(normalize)
        .collect();
    let tool = if coll.is_some() {
        "aggregate"
    } else {
        "aggregate-db"
    };
    if coll.is_none() {
        const DB_STAGES: [&str; 5] = [
            "$changeStream",
            "$currentOp",
            "$documents",
            "$listLocalSessions",
            "$queryStats",
        ];
        let first_ok = pipeline
            .first()
            .and_then(Value::as_object)
            .is_some_and(|m| m.keys().any(|k| DB_STAGES.contains(&k.as_str())));
        if !first_ok {
            return Err(format!(
                "The first stage of the pipeline must be a database-level aggregation stage (one of {})",
                DB_STAGES.join(", ")
            ));
        }
    }
    if search_stage(&pipeline) {
        return Ok(fail(SEARCH_UNSUPPORTED));
    }
    let (docs, write) = run_pipeline(ctx, db, coll, &pipeline)?;
    if let Some(stage) = write {
        perform_write(ctx, db, &stage, docs)?;
        let content = ctx.untrusted("The aggregation pipeline executed successfully.", vec![]);
        let mut structured = json!({ "documents": [], "appliedLimits": [] });
        if coll.is_some() {
            structured["count"] = json!("indeterminate");
        }
        return Ok(ok(content, Some(structured)));
    }
    let total = docs.len();
    let mut limits = Vec::new();
    if total > MAX_DOCUMENTS_PER_QUERY {
        limits.push(LIMIT_TEXT_MAX_DOCS);
    }
    let capped_docs: Vec<Value> = docs.into_iter().take(MAX_DOCUMENTS_PER_QUERY).collect();
    let (out, capped) = cap_bytes(
        capped_docs,
        args.get("responseBytesLimit")
            .and_then(Value::as_f64)
            .or(Some(ONE_MB as f64)),
    );
    if let Some(c) = capped {
        limits.push(c);
    }
    let mut message = format!("The aggregation resulted in {total} documents.");
    if out.len() != total || !limits.is_empty() {
        message.push_str(&format!(" Returning {} documents", out.len()));
        if !limits.is_empty() {
            message.push_str(&format!(
                " while respecting the applied limits of {}",
                limits.join(", ")
            ));
            if tool == "aggregate" {
                message.push_str(". If the entire aggregation result is required, use the \"export\" tool to retrieve the full result set");
            }
        }
        message.push('.');
    }
    let data = if out.is_empty() {
        vec![]
    } else {
        vec![Value::Array(out.clone()).to_string()]
    };
    let content = ctx.untrusted(&message, data);
    let applied: Vec<&str> = limits.iter().map(|l| limit_key(l)).collect();
    let structured = if coll.is_some() {
        json!({ "documents": out, "count": total, "appliedLimits": applied })
    } else {
        json!({ "documents": out, "aggResultsCount": total, "appliedLimits": applied })
    };
    Ok(ok(content, Some(structured)))
}

fn perform_write(
    ctx: &mut Ctx,
    db: &str,
    stage: &WriteStage,
    docs: Vec<Value>,
) -> Result<(), String> {
    let (target_db, coll, replace) = match stage {
        WriteStage::Out { db: d, coll } => (
            d.clone().unwrap_or_else(|| db.to_owned()),
            coll.clone(),
            true,
        ),
        WriteStage::Merge { db: d, coll } => (
            d.clone().unwrap_or_else(|| db.to_owned()),
            coll.clone(),
            false,
        ),
    };
    ensure_namespace(ctx, &target_db, &coll);
    let name = data_coll(&target_db, &coll);
    if replace {
        let keys: Vec<String> = ctx
            .coll(&name)
            .map(|c| c.iter().map(|(k, _)| k.to_owned()).collect())
            .unwrap_or_default();
        for k in keys {
            ctx.coll_mut(&name).remove(&k);
        }
    }
    for mut d in docs {
        if d.get("_id").is_none() {
            let id = ctx.object_id("objectid");
            let mut m = Map::new();
            m.insert("_id".to_owned(), json!({ "$oid": id }));
            for (k, v) in d.as_object().into_iter().flatten() {
                m.insert(k.clone(), v.clone());
            }
            d = Value::Object(m);
        }
        let key = id_key(&d["_id"]);
        let merged = match (replace, ctx.coll(&name).and_then(|c| c.get(&key)).cloned()) {
            (false, Some(Value::Object(mut existing))) => {
                for (k, v) in d.as_object().into_iter().flatten() {
                    existing.insert(k.clone(), v.clone());
                }
                Value::Object(existing)
            }
            _ => d,
        };
        ctx.coll_mut(&name).insert(key, merged);
    }
    Ok(())
}

fn count(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let has_query = args.get("query").is_some_and(|q| !q.is_null());
    let filter = arg_obj(args, "query");
    let mut n = 0;
    for d in ctx.docs(&data_coll(db, coll)) {
        if query::matches(&d, &filter)? {
            n += 1;
        }
    }
    Ok(ok(
        vec![text(format!(
            "Found {n} documents in the collection \"{coll}\"{}.",
            if has_query {
                " that matched the query"
            } else {
                ""
            }
        ))],
        Some(json!({ "count": n })),
    ))
}

fn insert_many(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let docs = args
        .get("documents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if docs.is_empty() {
        return Err("Invalid BulkOperation, Batch cannot be empty".to_owned());
    }
    ensure_namespace(ctx, db, coll);
    let name = data_coll(db, coll);
    let mut inserted = Vec::new();
    for doc in docs {
        let doc = normalize(doc);
        let Value::Object(fields) = doc else {
            return Err("document must be an object".to_owned());
        };
        let mut m = Map::new();
        let id = match fields.get("_id") {
            Some(id) => id.clone(),
            None => json!({ "$oid": ctx.object_id("objectid") }),
        };
        m.insert("_id".to_owned(), id.clone());
        for (k, v) in fields {
            if k != "_id" {
                m.insert(k, v);
            }
        }
        let doc = Value::Object(m);
        if let Some(e) = duplicate_key(ctx, db, coll, &doc, None) {
            return Err(e);
        }
        ctx.coll_mut(&name).insert(id_key(&id), doc);
        inserted.push(id);
    }
    let shown: Vec<String> = inserted.iter().map(id_display).collect();
    let content = ctx.untrusted(
        "Documents were inserted successfully.",
        vec![
            format!(
                "Inserted `{}` document(s) into {db}.{coll}.",
                inserted.len()
            ),
            format!("Inserted IDs: {}", shown.join(", ")),
        ],
    );
    let structured_ids: Vec<Value> = inserted
        .iter()
        .map(|id| match ejson::oid_hex(id) {
            Some(h) => json!(h),
            None => id.clone(),
        })
        .collect();
    Ok(ok(
        content,
        Some(
            json!({ "database": db, "collection": coll, "insertedCount": inserted.len(), "insertedIds": structured_ids }),
        ),
    ))
}

fn update_many(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let filter = arg_obj(args, "filter");
    let upd = normalize(args.get("update").cloned().unwrap_or(Value::Null));
    let upsert = args.get("upsert").and_then(Value::as_bool).unwrap_or(false);
    let name = data_coll(db, coll);
    let entries: Vec<(String, Value)> = ctx
        .coll(&name)
        .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
        .unwrap_or_default();
    // Validate the update document even when nothing matches.
    {
        let src = DbSource {
            ctx,
            db: db.to_owned(),
        };
        let mut probe = json!({"_id": 0});
        update::apply(&mut probe, &upd, false, &src)?;
    }
    let mut matched = 0;
    let mut modified = 0;
    let mut changes = Vec::new();
    for (key, doc) in entries {
        if !query::matches(&doc, &filter)? {
            continue;
        }
        matched += 1;
        let mut new_doc = doc.clone();
        {
            let src = DbSource {
                ctx,
                db: db.to_owned(),
            };
            update::apply(&mut new_doc, &upd, false, &src)?;
        }
        if new_doc != doc {
            changes.push((key, new_doc));
        }
    }
    for (key, doc) in &changes {
        if let Some(e) = duplicate_key(ctx, db, coll, doc, Some(key)) {
            return Err(format!(
                "Plan executor error during update :: caused by :: {e}"
            ));
        }
    }
    for (key, doc) in changes {
        // `insert` on an existing key keeps the document's position.
        ctx.coll_mut(&name).insert(key, doc);
        modified += 1;
    }
    let mut upserted_id = None;
    if matched == 0 && upsert {
        ensure_namespace(ctx, db, coll);
        let mut doc = update::upsert_seed(&filter);
        {
            let src = DbSource {
                ctx,
                db: db.to_owned(),
            };
            update::apply(&mut doc, &upd, true, &src)?;
        }
        let id = doc
            .get("_id")
            .cloned()
            .unwrap_or_else(|| json!({"$oid": ctx.object_id("objectid")}));
        let mut m = Map::new();
        m.insert("_id".to_owned(), id.clone());
        for (k, v) in doc.as_object().into_iter().flatten() {
            if k != "_id" {
                m.insert(k.clone(), v.clone());
            }
        }
        let doc = Value::Object(m);
        if let Some(e) = duplicate_key(ctx, db, coll, &doc, None) {
            return Err(e);
        }
        ctx.coll_mut(&name).insert(id_key(&id), doc);
        upserted_id = Some(id_display(&id));
    }
    let upserted = usize::from(upserted_id.is_some());
    let message = if matched == 0 && modified == 0 && upserted == 0 {
        "No documents matched the filter.".to_owned()
    } else {
        let mut m = format!("Matched {matched} document(s).");
        if modified > 0 {
            m.push_str(&format!(" Modified {modified} document(s)."));
        }
        if let Some(id) = &upserted_id {
            m.push_str(&format!(" Upserted 1 document with id: {id}."));
        }
        m
    };
    let mut structured = json!({ "database": db, "collection": coll, "matchedCount": matched, "modifiedCount": modified, "upsertedCount": upserted });
    if let Some(id) = upserted_id {
        structured["upsertedId"] = json!(id);
    }
    Ok(ok(vec![text(message)], Some(structured)))
}

fn delete_many(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let filter = arg_obj(args, "filter");
    let name = data_coll(db, coll);
    let mut doomed = Vec::new();
    for (k, d) in ctx
        .coll(&name)
        .map(|c| {
            c.iter()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    {
        if query::matches(&d, &filter)? {
            doomed.push(k);
        }
    }
    for k in &doomed {
        ctx.coll_mut(&name).remove(k);
    }
    Ok(ok(
        vec![text(format!(
            "Deleted `{}` document(s) from the requested collection.",
            doomed.len()
        ))],
        Some(json!({ "database": db, "collection": coll, "deletedCount": doomed.len() })),
    ))
}

fn create_collection(ctx: &mut Ctx, db: &str, coll: &str) -> Result<Value, String> {
    if coll.is_empty() || coll.contains('$') {
        return Err(format!("Invalid collection name: {coll}"));
    }
    if namespace_exists(ctx, db, coll) {
        return Err(format!("Collection {db}.{coll} already exists."));
    }
    ensure_namespace(ctx, db, coll);
    Ok(ok(
        vec![text(format!(
            "Collection \"{coll}\" created in database \"{db}\"."
        ))],
        Some(json!({ "database": db, "collection": coll, "created": true })),
    ))
}

fn clear(ctx: &mut Ctx, name: &str) {
    let keys: Vec<String> = ctx
        .coll(name)
        .map(|c| c.iter().map(|(k, _)| k.to_owned()).collect())
        .unwrap_or_default();
    for k in keys {
        ctx.coll_mut(name).remove(&k);
    }
}

fn drop_collection(ctx: &mut Ctx, db: &str, coll: &str) -> Result<Value, String> {
    clear(ctx, &data_coll(db, coll));
    ctx.coll_mut("namespaces").remove(&ns_key(db, coll));
    Ok(ok(
        vec![text(
            "Successfully dropped the requested collection from the requested database.",
        )],
        Some(json!({ "database": db, "collection": coll, "dropped": true })),
    ))
}

fn drop_database(ctx: &mut Ctx, db: &str) -> Result<Value, String> {
    for coll in collections_of(ctx, db) {
        clear(ctx, &data_coll(db, &coll));
        ctx.coll_mut("namespaces").remove(&ns_key(db, &coll));
    }
    Ok(ok(
        vec![text("Successfully dropped the requested database.")],
        Some(json!({ "database": db, "dropped": true })),
    ))
}

fn rename_collection(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let new_name = str_arg(args, "newName").unwrap_or_default().to_owned();
    let drop_target = args
        .get("dropTarget")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let structured = |renamed: bool| json!({ "database": db, "oldCollection": coll, "newCollection": new_name, "renamed": renamed });
    if !namespace_exists(ctx, db, coll) {
        return Ok(json!({
            "content": [text("Cannot rename the requested collection because it doesn't exist.")],
            "structuredContent": structured(false),
            "isError": true,
        }));
    }
    if namespace_exists(ctx, db, &new_name) {
        if !drop_target {
            return Ok(json!({
                "content": [text("Cannot rename the requested collection because the target collection already exists. If you want to overwrite it, set the \"dropTarget\" argument to true.")],
                "structuredContent": structured(false),
                "isError": true,
            }));
        }
        clear(ctx, &data_coll(db, &new_name));
        ctx.coll_mut("namespaces").remove(&ns_key(db, &new_name));
    }
    let old_name = data_coll(db, coll);
    let docs: Vec<(String, Value)> = ctx
        .coll(&old_name)
        .map(|c| c.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect())
        .unwrap_or_default();
    clear(ctx, &old_name);
    let target = data_coll(db, &new_name);
    for (k, v) in docs {
        ctx.coll_mut(&target).insert(k, v);
    }
    if let Some(mut meta) = ctx.coll_mut("namespaces").remove(&ns_key(db, coll)) {
        meta["collection"] = json!(new_name);
        ctx.coll_mut("namespaces")
            .insert(ns_key(db, &new_name), meta);
    }
    Ok(ok(
        vec![text(
            "The collection was renamed successfully in the requested database.",
        )],
        Some(structured(true)),
    ))
}

fn db_data_size(ctx: &Ctx, db: &str) -> (usize, usize, usize) {
    let mut objects = 0;
    let mut size = 0;
    let colls = collections_of(ctx, db);
    for c in &colls {
        for d in ctx.docs(&data_coll(db, c)) {
            objects += 1;
            size += bson_size(&d);
        }
    }
    (colls.len(), objects, size)
}

fn size_on_disk(ctx: &Ctx, db: &str) -> u64 {
    match db {
        "admin" => 364_544,
        "config" => 0,
        "local" => 4_571_136_000,
        _ => {
            let (colls, _, size) = db_data_size(ctx, db);
            let indexes: usize = collections_of(ctx, db)
                .iter()
                .map(|c| indexes_of(ctx, db, c).len())
                .sum();
            (colls as u64 * 20_480
                + indexes as u64 * 20_480
                + (size as u64 / 3).div_ceil(4096) * 4096)
                .max(8192)
        }
    }
}

fn list_databases(ctx: &mut Ctx) -> Result<Value, String> {
    let dbs: Vec<Value> = databases(ctx)
        .iter()
        .filter(|d| *d != "config")
        .map(|d| json!({ "name": d, "size": size_on_disk(ctx, d) }))
        .collect();
    let content = ctx.untrusted(
        &format!("Found {} databases:", dbs.len()),
        vec![Value::Array(dbs.clone()).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "databases": dbs, "totalCount": dbs.len() })),
    ))
}

fn list_collections(ctx: &mut Ctx, db: &str) -> Result<Value, String> {
    let colls: Vec<Value> = collections_of(ctx, db)
        .into_iter()
        .map(|c| json!({ "name": c }))
        .collect();
    if colls.is_empty() {
        return Ok(ok(
            vec![text(
                "Found 0 collections for the requested database. To create a collection, use the \"create-collection\" tool.",
            )],
            Some(json!({ "collections": [], "totalCount": 0 })),
        ));
    }
    let content = ctx.untrusted(
        &format!(
            "Found {} collections in the requested database.",
            colls.len()
        ),
        vec![json!({ "database": db, "collections": colls }).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "collections": colls, "totalCount": colls.len() })),
    ))
}

fn collection_indexes(ctx: &mut Ctx, db: &str, coll: &str) -> Result<Value, String> {
    if !namespace_exists(ctx, db, coll) {
        return Ok(fail(
            "The indexes for the requested namespace cannot be determined because the collection does not exist.",
        ));
    }
    let classic: Vec<Value> = indexes_of(ctx, db, coll)
        .iter()
        .map(|i| json!({ "name": i["name"], "key": i["key"] }))
        .collect();
    let content = ctx.untrusted(
        &format!(
            "Found {} classic indexes in the requested collection:",
            classic.len()
        ),
        vec![Value::Array(classic.clone()).to_string()],
    );
    Ok(ok(
        content,
        Some(
            json!({ "classicIndexes": classic, "searchIndexes": [], "classicIndexesCount": classic.len(), "searchIndexesCount": 0 }),
        ),
    ))
}

fn index_name(keys: &Map<String, Value>) -> String {
    keys.iter()
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}_{s}"),
            v => format!("{k}_{}", ejson::num(ejson::as_f64(v).unwrap_or(1.0))),
        })
        .collect::<Vec<_>>()
        .join("_")
}

fn create_index(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let Some(def) = args
        .get("definition")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
    else {
        return Err("Index definition not provided. Expected one of the following: `classic`, `vectorSearch`".to_owned());
    };
    let kind = def["type"].as_str().unwrap_or("classic");
    if kind != "classic" {
        return Ok(fail(SEARCH_UNSUPPORTED));
    }
    let keys = def["keys"]
        .as_object()
        .cloned()
        .ok_or("Index keys must be an object")?;
    if keys.is_empty() {
        return Err("Index keys cannot be an empty field.".to_owned());
    }
    let name = str_arg(args, "name").map_or_else(|| index_name(&keys), str::to_owned);
    ensure_namespace(ctx, db, coll);
    let existing = indexes_of(ctx, db, coll);
    if let Some(same) = existing.iter().find(|i| i["name"] == name.as_str()) {
        if same["key"] != Value::Object(keys.clone()) {
            return Err(format!(
                "An existing index has the same name as the requested index. When index names are not specified, they are auto generated and can cause conflicts. Please refer to our documentation. Requested index: {{ v: 2, key: {}, name: \"{name}\" }}, existing index: {{ v: 2, key: {}, name: \"{name}\" }}",
                shell_repr(&Value::Object(keys)),
                shell_repr(&same["key"])
            ));
        }
    } else if let Some(other) = existing
        .iter()
        .find(|i| i["key"] == Value::Object(keys.clone()))
    {
        return Err(format!(
            "Index already exists with a different name: {}",
            other["name"].as_str().unwrap_or("")
        ));
    } else if let Some(meta) = ctx.coll_mut("namespaces").get_mut(&ns_key(db, coll))
        && let Some(arr) = meta["indexes"].as_array_mut()
    {
        arr.push(json!({ "v": 2, "key": keys, "name": name }));
    }
    Ok(ok(
        vec![text(format!(
            "Created the index \"{name}\" on collection \"{coll}\" in database \"{db}\"."
        ))],
        Some(
            json!({ "database": db, "collection": coll, "indexName": name, "indexType": "classic" }),
        ),
    ))
}

fn drop_index(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let index = str_arg(args, "indexName").unwrap_or_default().to_owned();
    if str_arg(args, "type") == Some("search") {
        return Ok(fail(
            "Atlas Search is not supported in the current cluster.",
        ));
    }
    if !namespace_exists(ctx, db, coll) {
        return Err(format!("ns not found {db}.{coll}"));
    }
    if index == "_id_" {
        return Err("cannot drop _id index".to_owned());
    }
    let meta = ctx
        .coll_mut("namespaces")
        .get_mut(&ns_key(db, coll))
        .expect("namespace exists");
    let arr = meta["indexes"].as_array_mut().expect("indexes array");
    let Some(pos) = arr.iter().position(|i| i["name"] == index.as_str()) else {
        return Err(format!("index not found with name [{index}]"));
    };
    arr.remove(pos);
    let content = ctx.untrusted(
        "Successfully dropped the index from the provided namespace.",
        vec![json!({ "indexName": index, "namespace": format!("{db}.{coll}") }).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "database": db, "collection": coll, "indexName": index, "dropped": true })),
    ))
}

fn bson_type_name(v: &Value) -> &'static str {
    match ejson::type_name(v) {
        "string" => "String",
        "int" => "Int32",
        "long" => "Long",
        "double" => "Double",
        "bool" => "Boolean",
        "date" => "Date",
        "objectId" => "ObjectId",
        "object" => "Document",
        "array" => "Array",
        "null" => "Null",
        "decimal" => "Decimal128",
        "regex" => "RegExp",
        "binData" => "Binary",
        "timestamp" => "Timestamp",
        _ => "String",
    }
}

/// `getSimplifiedSchema` from mongodb-schema: per field, its observed types (most common first).
fn simplified_schema(docs: &[Value]) -> Map<String, Value> {
    let mut fields: Vec<(String, Vec<Value>)> = Vec::new();
    for d in docs {
        if let Value::Object(m) = d {
            for (k, v) in m {
                match fields.iter_mut().find(|(f, _)| f == k) {
                    Some((_, vals)) => vals.push(v.clone()),
                    None => fields.push((k.clone(), vec![v.clone()])),
                }
            }
        }
    }
    let mut out = Map::new();
    for (k, vals) in fields {
        out.insert(k, json!({ "types": simplified_types(&vals) }));
    }
    out
}

fn simplified_types(vals: &[Value]) -> Vec<Value> {
    let mut groups: Vec<(&'static str, Vec<&Value>)> = Vec::new();
    for v in vals {
        let t = bson_type_name(v);
        match groups.iter_mut().find(|(n, _)| *n == t) {
            Some((_, g)) => g.push(v),
            None => groups.push((t, vec![v])),
        }
    }
    groups.sort_by_key(|a| std::cmp::Reverse(a.1.len()));
    groups
        .into_iter()
        .map(|(t, g)| match t {
            "Document" => {
                let docs: Vec<Value> = g.into_iter().cloned().collect();
                json!({ "bsonType": "Document", "fields": simplified_schema(&docs) })
            }
            "Array" => {
                let items: Vec<Value> = g
                    .into_iter()
                    .flat_map(|a| a.as_array().cloned().unwrap_or_default())
                    .collect();
                json!({ "bsonType": "Array", "types": simplified_types(&items) })
            }
            other => json!({ "bsonType": other }),
        })
        .collect()
}

fn collection_schema(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let sample = args
        .get("sampleSize")
        .and_then(Value::as_f64)
        .unwrap_or(50.0)
        .clamp(0.0, 50_000.0) as usize;
    let docs: Vec<Value> = ctx
        .docs(&data_coll(db, coll))
        .into_iter()
        .take(sample)
        .collect();
    let (docs, _) = cap_bytes(
        docs,
        args.get("responseBytesLimit")
            .and_then(Value::as_f64)
            .or(Some(ONE_MB as f64)),
    );
    let schema = simplified_schema(&docs);
    if schema.is_empty() {
        return Ok(ok(
            vec![text(
                "Could not deduce the schema for the requested namespace. This may be because it doesn't exist or is empty.",
            )],
            Some(json!({ "schema": {}, "fieldsCount": 0 })),
        ));
    }
    let n = schema.len();
    let content = ctx.untrusted(
        &format!("Found {n} fields in the sampled schema. Note that this schema is inferred from a sample and may not represent the full schema of the collection."),
        vec![json!({ "database": db, "collection": coll, "schema": schema }).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "schema": schema, "fieldsCount": n })),
    ))
}

fn storage_size(ctx: &mut Ctx, db: &str, coll: &str) -> Result<Value, String> {
    if !namespace_exists(ctx, db, coll) {
        return Ok(fail(
            "The size of the requested namespace cannot be determined because the collection does not exist.",
        ));
    }
    let size: usize = ctx.docs(&data_coll(db, coll)).iter().map(bson_size).sum();
    let v = size as f64;
    let (value, units) = if v > 1_073_741_824.0 {
        (v / 1_073_741_824.0, "GB")
    } else if v > 1_048_576.0 {
        (v / 1_048_576.0, "MB")
    } else if v > 1024.0 {
        (v / 1024.0, "KB")
    } else {
        (v, "bytes")
    };
    Ok(ok(
        vec![text(format!(
            "The size of the requested namespace is `{value:.2} {units}`"
        ))],
        Some(json!({ "size": ejson::num(value), "units": units })),
    ))
}

fn db_stats(ctx: &mut Ctx, db: &str) -> Result<Value, String> {
    let (colls, objects, data_size) = db_data_size(ctx, db);
    let indexes: usize = collections_of(ctx, db)
        .iter()
        .map(|c| indexes_of(ctx, db, c).len())
        .sum();
    let storage = if colls == 0 {
        0
    } else {
        size_on_disk(ctx, db) as usize - indexes * 20_480
    };
    let index_size = indexes * 20_480;
    let stats = json!({
        "db": db,
        "collections": colls,
        "views": 0,
        "objects": objects,
        "avgObjSize": if objects == 0 { json!(0) } else { ejson::num(data_size as f64 / objects as f64) },
        "dataSize": data_size,
        "storageSize": storage,
        "indexes": indexes,
        "indexSize": index_size,
        "totalSize": storage + index_size,
        "scaleFactor": 1,
        "fsUsedSize": 4_385_628_160_u64,
        "fsTotalSize": 10_726_932_480_u64,
        "ok": 1,
    });
    let content = ctx.untrusted(
        "Statistics for database:",
        vec![json!({ "database": db, "stats": stats }).to_string()],
    );
    Ok(ok(content, Some(json!({ "stats": stats }))))
}

fn winning_plan(ctx: &Ctx, db: &str, coll: &str, filter: &Value) -> Value {
    let fields: Vec<&String> = filter
        .as_object()
        .map(|m| m.keys().filter(|k| !k.starts_with('$')).collect())
        .unwrap_or_default();
    let index = indexes_of(ctx, db, coll).into_iter().find(|i| {
        i["key"]
            .as_object()
            .and_then(|k| k.keys().next())
            .is_some_and(|first| fields.contains(&first))
    });
    match index {
        Some(idx) => {
            let key = idx["key"].clone();
            let bounds: Map<String, Value> = key
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| {
                    let b = match filter.get(k) {
                        Some(v) if !v.is_object() || ejson::is_wrapper(v) => {
                            format!("[{}, {}]", shell_repr(v), shell_repr(v))
                        }
                        _ => "[MinKey, MaxKey]".to_owned(),
                    };
                    (k.clone(), json!([b]))
                })
                .collect();
            json!({
                "isCached": false,
                "stage": "FETCH",
                "inputStage": {
                    "stage": "IXSCAN",
                    "keyPattern": key,
                    "indexName": idx["name"],
                    "isMultiKey": false,
                    "multiKeyPaths": {},
                    "isUnique": idx["unique"] == true || idx["name"] == "_id_",
                    "isSparse": false,
                    "isPartial": false,
                    "indexVersion": 2,
                    "direction": "forward",
                    "indexBounds": bounds,
                },
            })
        }
        None => json!({
            "isCached": false,
            "stage": "COLLSCAN",
            "filter": filter,
            "direction": "forward",
        }),
    }
}

fn explain(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let Some(method) = args
        .get("method")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
    else {
        return Err(
            "No method provided. Expected one of the following: `aggregate`, `find`, or `count`"
                .to_owned(),
        );
    };
    let verbosity = str_arg(args, "verbosity")
        .unwrap_or("queryPlanner")
        .to_owned();
    let m = method["name"].as_str().unwrap_or("find").to_owned();
    let margs = method
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let (filter, command, pipeline) = match m.as_str() {
        "aggregate" => {
            let p: Vec<Value> = margs["pipeline"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(normalize)
                .collect();
            let first_match = p
                .first()
                .and_then(|s| s.get("$match"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            (
                first_match,
                json!({ "aggregate": coll, "pipeline": p, "explain": true, "cursor": {}, "$db": db }),
                Some(p),
            )
        }
        "count" => {
            let q = normalize(margs.get("query").cloned().unwrap_or_else(|| json!({})));
            (
                q.clone(),
                json!({ "count": coll, "query": q, "$db": db }),
                None,
            )
        }
        _ => {
            let f = normalize(margs.get("filter").cloned().unwrap_or_else(|| json!({})));
            let mut cmd = json!({ "find": coll, "filter": f });
            for k in ["projection", "sort", "limit"] {
                if let Some(v) = margs.get(k).filter(|v| !v.is_null()) {
                    cmd[k] = v.clone();
                }
            }
            cmd["$db"] = json!(db);
            (f, cmd, None)
        }
    };
    let docs = ctx.docs(&data_coll(db, coll));
    let mut n = 0;
    for d in &docs {
        if query::matches(d, &filter)? {
            n += 1;
        }
    }
    let plan = winning_plan(ctx, db, coll, &filter);
    let ixscan = plan["stage"] == "FETCH";
    let mut query_planner = json!({
        "namespace": format!("{db}.{coll}"),
        "parsedQuery": filter,
        "indexFilterSet": false,
        "planCacheShapeHash": "A1B2C3D4",
        "planCacheKey": "E5F6A7B8",
        "optimizationTimeMillis": 0,
        "maxIndexedOrSolutionsReached": false,
        "maxIndexedAndSolutionsReached": false,
        "maxScansToExplodeReached": false,
        "prunedSimilarIndexes": false,
        "winningPlan": plan,
        "rejectedPlans": [],
    });
    if m == "count" {
        query_planner["winningPlan"] = json!({ "isCached": false, "stage": "COUNT", "inputStage": query_planner["winningPlan"].clone() });
    }
    let mut result = Map::new();
    result.insert("explainVersion".to_owned(), json!("1"));
    let stats = json!({
        "executionSuccess": true,
        "nReturned": n,
        "executionTimeMillis": 0,
        "totalKeysExamined": if ixscan { n } else { 0 },
        "totalDocsExamined": if ixscan { n } else { docs.len() },
    });
    let detailed = verbosity == "executionStats" || verbosity == "allPlansExecution";
    match &pipeline {
        Some(p)
            if p.iter().skip(1).any(|s| {
                s.get("$group").is_some()
                    || s.get("$lookup").is_some()
                    || s.get("$unwind").is_some()
            }) =>
        {
            let mut cursor = json!({ "queryPlanner": query_planner });
            if detailed {
                cursor["executionStats"] = stats.clone();
            }
            let mut stages = vec![json!({ "$cursor": cursor })];
            for s in p.iter().skip(usize::from(
                p.first().and_then(|s| s.get("$match")).is_some(),
            )) {
                stages.push(s.clone());
            }
            result.insert("stages".to_owned(), Value::Array(stages));
        }
        _ => {
            result.insert("queryPlanner".to_owned(), query_planner);
            if detailed {
                result.insert("executionStats".to_owned(), stats);
            }
        }
    }
    result.insert("command".to_owned(), command);
    result.insert(
        "serverInfo".to_owned(),
        json!({ "host": deployment_host(ctx), "port": 27017, "version": server_version(ctx), "gitVersion": "7b4a0c7f1d5b0e1f1c4b9a2e3d8f6a5b4c3d2e1f" }),
    );
    result.insert("ok".to_owned(), json!(1));
    let result = Value::Object(result);
    let content = ctx.untrusted(
        &format!("Here is some information about the winning plan chosen by the query optimizer for running the given `{m}` operation on the requested namespace. The execution plan was run with the following verbosity: \"{verbosity}\". This information can be used to understand how the query was executed and to optimize the query performance."),
        vec![json!({ "database": db, "collection": coll, "plan": result }).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "explainResult": result, "method": m, "verbosity": verbosity })),
    ))
}

fn export(ctx: &mut Ctx, db: &str, coll: &str, args: &Value) -> Result<Value, String> {
    let Some(target) = args
        .get("exportTarget")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
    else {
        return Err(
            "Export target not provided. Expected one of the following: `aggregate`, `find`"
                .to_owned(),
        );
    };
    let targs = target
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let docs = if target["name"] == "aggregate" {
        let p: Vec<Value> = targs["pipeline"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(normalize)
            .collect();
        if p.iter()
            .any(|s| s.get("$out").is_some() || s.get("$merge").is_some())
        {
            return Err("The export tool can not run pipelines with $out or $merge stages. Use the aggregate tool to run a pipeline that writes to a collection.".to_owned());
        }
        run_pipeline(ctx, db, Some(coll), &p)?.0
    } else {
        let filter = normalize(targs.get("filter").cloned().unwrap_or_else(|| json!({})));
        let mut out = Vec::new();
        for d in ctx.docs(&data_coll(db, coll)) {
            if query::matches(&d, &filter)? {
                out.push(d);
            }
        }
        query::sort_docs(&mut out, targs.get("sort").unwrap_or(&Value::Null))?;
        if let Some(l) = targs
            .get("limit")
            .and_then(Value::as_f64)
            .filter(|l| *l > 0.0)
        {
            out.truncate(l as usize);
        }
        let projection = targs.get("projection").cloned().unwrap_or(Value::Null);
        out.iter()
            .map(|d| query::project(d, &projection))
            .collect::<Result<_, _>>()?
    };
    let name = format!("{}.json", ctx.object_id("export"));
    let uri = format!("exported-data://{name}");
    let title = str_arg(args, "exportTitle").unwrap_or("").to_owned();
    ctx.coll_mut("exports").insert(
        name.clone(),
        json!({ "exportName": name, "exportTitle": title, "exportURI": uri, "namespace": format!("{db}.{coll}"), "documents": docs.len(), "format": str_arg(args, "jsonExportFormat").unwrap_or("relaxed") }),
    );
    Ok(ok(
        vec![
            text(format!(
                "Data for namespace {db}.{coll} is being exported and will be made available under resource URI - \"{uri}\"."
            )),
            json!({ "type": "resource_link", "name": name, "uri": uri, "description": "Resource URI for fetching exported data once it is ready.", "mimeType": "application/json" }),
        ],
        None,
    ))
}

fn default_log(ctx: &Ctx) -> Vec<String> {
    let host = deployment_host(ctx);
    let version = server_version(ctx);
    let t = |s: i64| ejson::iso_millis((ctx.now - 86_400 + s) * 1000).replace('Z', "+00:00");
    vec![
        json!({"t": {"$date": t(0)}, "s": "I", "c": "CONTROL", "id": 23285, "ctx": "main", "msg": "Automatically disabling TLS 1.0 and TLS 1.1, to force-enable TLS 1.1 specify --sslDisabledProtocols 'TLS1_0'; to force-enable TLS 1.0 specify --sslDisabledProtocols 'none'"}).to_string(),
        json!({"t": {"$date": t(1)}, "s": "I", "c": "CONTROL", "id": 4615611, "ctx": "initandlisten", "msg": "MongoDB starting", "attr": {"pid": 4128, "port": 27017, "dbPath": "/srv/mongodb/atlas-data", "architecture": "64-bit", "host": host}}).to_string(),
        json!({"t": {"$date": t(1)}, "s": "I", "c": "CONTROL", "id": 23403, "ctx": "initandlisten", "msg": "Build Info", "attr": {"buildInfo": {"version": version, "gitVersion": "7b4a0c7f1d5b0e1f1c4b9a2e3d8f6a5b4c3d2e1f", "modules": ["enterprise"]}}}).to_string(),
        json!({"t": {"$date": t(4)}, "s": "I", "c": "REPL", "id": 21358, "ctx": "ReplCoord-0", "msg": "Replica set state transition", "attr": {"newState": "PRIMARY", "oldState": "SECONDARY"}}).to_string(),
        json!({"t": {"$date": t(5)}, "s": "I", "c": "NETWORK", "id": 23016, "ctx": "listener", "msg": "Waiting for connections", "attr": {"port": 27017, "ssl": "on"}}).to_string(),
    ]
}

fn logs(ctx: &mut Ctx, args: &Value) -> Result<Value, String> {
    let kind = str_arg(args, "type").unwrap_or("global");
    let limit = args
        .get("limit")
        .and_then(Value::as_f64)
        .unwrap_or(50.0)
        .clamp(1.0, 1024.0) as usize;
    let dep = deployment(ctx);
    let lines: Vec<String> = match kind {
        "startupWarnings" => dep["startupWarnings"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|l| {
                        if let Value::String(s) = l {
                            s.clone()
                        } else {
                            l.to_string()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => dep["logs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|l| {
                        if let Value::String(s) = l {
                            s.clone()
                        } else {
                            l.to_string()
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| default_log(ctx)),
    };
    let total = lines.len();
    let shown: Vec<String> = lines
        .into_iter()
        .take(limit)
        .map(|l| l.trim_end().to_owned())
        .collect();
    let mut message = format!("Found: {total} messages");
    if total > limit {
        message.push_str(&format!(" (showing only the first {limit})"));
    }
    let content = ctx.untrusted(&message, vec![shown.join("\n")]);
    Ok(ok(
        content,
        Some(json!({ "logs": shown, "totalLinesWritten": total, "shownCount": shown.len() })),
    ))
}
