//! A project's Postgres database, stored in the service store:
//!
//! - `tables`: one [`TableDef`] per table, keyed `<ref>.<schema>.<name>`
//! - `db.<ref>.<schema>.<name>`: that table's rows, keyed by primary key (`,`-joined)
//! - `extensions`: installed-extension overrides, keyed `<ref>.<name>`
//!
//! [`load`] builds an in-memory [`Db`]; [`store`] writes a changed one back.

use mock_core::{ids::IdGen, store::Store};
use serde_json::{Map, Value, json};

use crate::sql::{
    ColumnInfo, Db, Table, TableDef,
    exec::{size_pretty, table_bytes},
    value::{cast, data_type_name, rfc3339_to_tstz, to_text},
};

/// Extensions available on every project: (name, default_version, installed by default, comment).
const EXTENSIONS: [(&str, &str, Option<&str>, &str); 16] = [
    (
        "plpgsql",
        "1.0",
        Some("pg_catalog"),
        "PL/pgSQL procedural language",
    ),
    (
        "pgcrypto",
        "1.3",
        Some("extensions"),
        "cryptographic functions",
    ),
    (
        "uuid-ossp",
        "1.1",
        Some("extensions"),
        "generate universally unique identifiers (UUIDs)",
    ),
    (
        "pg_stat_statements",
        "1.10",
        Some("extensions"),
        "track planning and execution statistics of all SQL statements executed",
    ),
    (
        "pg_graphql",
        "1.5.11",
        Some("graphql"),
        "pg_graphql: GraphQL support",
    ),
    (
        "supabase_vault",
        "0.3.1",
        Some("vault"),
        "Supabase Vault Extension",
    ),
    (
        "pg_trgm",
        "1.6",
        None,
        "text similarity measurement and index searching based on trigrams",
    ),
    (
        "vector",
        "0.8.0",
        None,
        "vector data type and ivfflat and hnsw access methods",
    ),
    (
        "postgis",
        "3.3.7",
        None,
        "PostGIS geometry and geography spatial types and functions",
    ),
    ("pg_cron", "1.6", None, "Job scheduler for PostgreSQL"),
    ("pg_net", "0.14.0", None, "Async HTTP"),
    (
        "citext",
        "1.6",
        None,
        "data type for case-insensitive character strings",
    ),
    (
        "hstore",
        "1.8",
        None,
        "data type for storing sets of (key, value) pairs",
    ),
    (
        "unaccent",
        "1.1",
        None,
        "text search dictionary that removes accents",
    ),
    ("pgjwt", "0.2.0", None, "JSON Web Token API for Postgresql"),
    (
        "pg_stat_monitor",
        "2.1",
        None,
        "The pg_stat_monitor is a PostgreSQL Query Performance Monitoring tool, based on PostgreSQL contrib module pg_stat_statements.",
    ),
];

fn rows_collection(r: &str, schema: &str, name: &str) -> String {
    format!("db.{r}.{schema}.{name}")
}

/// Loads project `r`'s database. Tables without a definition get one inferred from rows.
pub fn load(store: &Store, ids: &IdGen, r: &str, now_rfc3339: &str) -> Db {
    let prefix = format!("{r}.");
    let mut tables = Vec::new();
    let mut max_oid = 16_384;
    if let Some(defs) = store.collection("tables") {
        for (key, doc) in defs.iter() {
            if !key.starts_with(&prefix) {
                continue;
            }
            let Ok(def) = serde_json::from_value::<TableDef>(doc.clone()) else {
                continue;
            };
            max_oid = max_oid.max(def.id);
            let rows = store
                .collection(&rows_collection(r, &def.schema, &def.name))
                .map(|c| {
                    c.values()
                        .map(|row| {
                            def.columns
                                .iter()
                                .map(|col| {
                                    normalize(
                                        row.get(&col.name).cloned().unwrap_or(Value::Null),
                                        col,
                                    )
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default();
            tables.push(Table { def, rows });
        }
    }
    let extensions = EXTENSIONS
        .iter()
        .map(|(name, version, schema, comment)| {
            let mut ext = json!({
                "name": name,
                "schema": schema,
                "default_version": version,
                "installed_version": schema.map(|_| *version),
                "comment": comment,
            });
            if let Some(over) = store
                .collection("extensions")
                .and_then(|c| c.get(&format!("{r}.{name}")))
            {
                ext["installed_version"] = over
                    .get("installed_version")
                    .cloned()
                    .unwrap_or(Value::Null);
                ext["schema"] = over.get("schema").cloned().unwrap_or(Value::Null);
            }
            ext
        })
        .collect();
    Db {
        tables,
        extensions,
        now: rfc3339_to_tstz(now_rfc3339),
        ids: ids.clone(),
        next_oid: max_oid + 1,
        dirty: false,
    }
}

/// Dataset values in their column's canonical form (numbers stay numbers, etc).
fn normalize(v: Value, col: &ColumnInfo) -> Value {
    if v.is_null() || !col.enums.is_empty() {
        return v;
    }
    match col.format.as_str() {
        "timestamptz" | "timestamp" | "date" | "int8" | "int4" | "int2" | "bool" => {
            cast(&v, &col.format).unwrap_or(v)
        }
        _ => v,
    }
}

fn row_key(def: &TableDef, row: &[Value], index: usize) -> String {
    if def.primary_keys.is_empty() {
        return (index + 1).to_string();
    }
    def.primary_keys
        .iter()
        .map(|k| {
            def.column_index(k)
                .and_then(|i| to_text(&row[i]))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Writes `db` back as project `r`'s database (replacing what was there).
pub fn store(store: &mut Store, ids: &mut IdGen, r: &str, db: &Db) {
    let prefix = format!("{r}.");
    let old: Vec<String> = store
        .collection("tables")
        .map(|c| {
            c.iter()
                .map(|(k, _)| k.to_owned())
                .filter(|k| k.starts_with(&prefix))
                .collect()
        })
        .unwrap_or_default();
    for key in &old {
        let still = db
            .tables
            .iter()
            .any(|t| format!("{r}.{}", t.def.qualified()) == *key);
        if !still {
            store.collection_mut("tables").remove(key);
            let rows = store.collection_mut(&format!("db.{key}"));
            let keys: Vec<String> = rows.iter().map(|(k, _)| k.to_owned()).collect();
            for k in keys {
                rows.remove(&k);
            }
        }
    }
    for t in &db.tables {
        let key = format!("{r}.{}", t.def.qualified());
        store.collection_mut("tables").insert(
            key,
            serde_json::to_value(&t.def).expect("table definition serializes"),
        );
        let coll = store.collection_mut(&rows_collection(r, &t.def.schema, &t.def.name));
        let keys: Vec<String> = coll.iter().map(|(k, _)| k.to_owned()).collect();
        for k in keys {
            coll.remove(&k);
        }
        for (i, row) in t.rows.iter().enumerate() {
            let mut obj = Map::new();
            for (c, v) in t.def.columns.iter().zip(row) {
                obj.insert(c.name.clone(), v.clone());
            }
            coll.insert(row_key(&t.def, row, i), Value::Object(obj));
        }
    }
    for ext in &db.extensions {
        let name = ext["name"].as_str().unwrap_or_default();
        let default_installed = EXTENSIONS
            .iter()
            .find(|e| e.0 == name)
            .is_some_and(|e| e.2.is_some());
        let installed = !ext["installed_version"].is_null();
        let key = format!("{r}.{name}");
        if installed != default_installed {
            store.collection_mut("extensions").insert(
                key,
                json!({ "installed_version": ext["installed_version"], "schema": ext["schema"] }),
            );
        } else {
            store.collection_mut("extensions").remove(&key);
        }
    }
    *ids = db.ids.clone();
}

/// Rows for the server's `list_tables` introspection query.
pub fn introspect_tables(db: &Db, parameters: &[Value], exclude: bool) -> Vec<Value> {
    let schemas: Vec<String> = parameters.iter().filter_map(to_text).collect();
    db.tables
        .iter()
        .filter(|t| schemas.contains(&t.def.schema) != exclude)
        .map(|t| table_row(db, &t.def, t.rows.len()))
        .collect()
}

fn table_row(db: &Db, def: &TableDef, row_count: usize) -> Value {
    let columns: Vec<Value> = def
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let unique = c.unique
                || def
                    .indexes
                    .iter()
                    .any(|ix| ix.unique && ix.columns.len() == 1 && ix.columns[0] == c.name);
            json!({
                "table_id": def.id,
                "schema": def.schema,
                "table": def.name,
                "id": format!("{}.{}", def.id, i + 1),
                "ordinal_position": i + 1,
                "name": c.name,
                "default_value": column_default(c),
                "data_type": if c.enums.is_empty() { data_type_name(&c.format) } else { "USER-DEFINED" },
                "format": c.format,
                "is_identity": c.identity.is_some(),
                "identity_generation": c.identity,
                "is_generated": false,
                "is_nullable": c.nullable,
                "is_updatable": true,
                "is_unique": unique,
                "enums": c.enums,
                "check": c.check,
                "comment": c.comment,
            })
        })
        .collect();
    let primary_keys: Vec<Value> = def
        .primary_keys
        .iter()
        .map(|k| json!({ "schema": def.schema, "table_name": def.name, "name": k, "table_id": def.id }))
        .collect();
    let qualified = def.qualified();
    let mut relationships = Vec::new();
    for t in &db.tables {
        for (i, fk) in t.def.foreign_keys.iter().enumerate() {
            if t.def.qualified() != qualified && fk.target != qualified {
                continue;
            }
            let (ts, tn) = fk
                .target
                .split_once('.')
                .unwrap_or(("public", fk.target.as_str()));
            relationships.push(json!({
                "id": t.def.id * 100 + i as i64 + 1,
                "constraint_name": fk.name,
                "source_schema": t.def.schema,
                "source_table_name": t.def.name,
                "source_columns": fk.columns,
                "target_table_schema": ts,
                "target_table_name": tn,
                "target_columns": fk.target_columns,
            }));
        }
    }
    let bytes = table_bytes(row_count);
    json!({
        "id": def.id,
        "schema": def.schema,
        "name": def.name,
        "rls_enabled": def.rls_enabled,
        "rls_forced": def.rls_forced,
        "replica_identity": "DEFAULT",
        "bytes": bytes,
        "size": size_pretty(bytes),
        "live_rows_estimate": row_count,
        "dead_rows_estimate": 0,
        "comment": def.comment,
        "primary_keys": primary_keys,
        "relationships": relationships,
        "columns": columns,
    })
}

/// `pg_get_expr` form of a column default.
fn column_default(c: &ColumnInfo) -> Value {
    match c.default.as_deref() {
        None => Value::Null,
        Some("nextval") => Value::String(format!("nextval('{}_seq'::regclass)", c.name)),
        Some(d) => {
            let lower = d.to_lowercase();
            let text = if lower == "now()" || lower == "current_timestamp" {
                "now()".to_owned()
            } else if d.starts_with('\'') && !d.contains("::") {
                format!("{d}::{}", c.format)
            } else {
                d.to_owned()
            };
            Value::String(text)
        }
    }
}

/// Rows for the server's `list_extensions` query.
pub fn introspect_extensions(db: &Db) -> Vec<Value> {
    db.extensions
        .iter()
        .map(|e| {
            json!({
                "name": e["name"],
                "schema": e["schema"],
                "default_version": e["default_version"],
                "installed_version": e["installed_version"],
                "comment": e["comment"],
            })
        })
        .collect()
}

/// Computed advisor lints (RLS and unindexed foreign keys) for `type` security|performance.
pub fn computed_lints(db: &Db, r: &str, kind: &str) -> Vec<Value> {
    let mut lints = Vec::new();
    for t in &db.tables {
        let d = &t.def;
        if kind == "security" && d.schema == "public" {
            if !d.rls_enabled {
                lints.push(json!({
                    "name": "rls_disabled_in_public",
                    "title": "RLS Disabled in Public",
                    "level": "ERROR",
                    "facing": "EXTERNAL",
                    "categories": ["SECURITY"],
                    "description": "Detects cases where row level security (RLS) has not been enabled on tables in schemas exposed to PostgREST",
                    "detail": format!("Table `{}.{}` is public, but RLS has not been enabled.", d.schema, d.name),
                    "remediation": "https://supabase.com/docs/guides/database/database-linter?lint=0013_rls_disabled_in_public",
                    "metadata": { "name": d.name, "type": "table", "schema": d.schema },
                    "cache_key": format!("rls_disabled_in_public_{}_{}", d.schema, d.name),
                }));
            } else if d.policies.is_empty() {
                lints.push(json!({
                    "name": "rls_enabled_no_policy",
                    "title": "RLS Enabled No Policy",
                    "level": "INFO",
                    "facing": "EXTERNAL",
                    "categories": ["SECURITY"],
                    "description": "Detects cases where row level security (RLS) has been enabled on a table but no RLS policies have been created.",
                    "detail": format!("Table `{}.{}` has RLS enabled, but no policies exist", d.schema, d.name),
                    "remediation": "https://supabase.com/docs/guides/database/database-linter?lint=0008_rls_enabled_no_policy",
                    "metadata": { "name": d.name, "type": "table", "schema": d.schema },
                    "cache_key": format!("rls_enabled_no_policy_{}_{}", d.schema, d.name),
                }));
            }
        }
        if kind == "performance" {
            for fk in &d.foreign_keys {
                let indexed = d.primary_keys.first() == fk.columns.first()
                    || d.indexes
                        .iter()
                        .any(|ix| ix.columns.first() == fk.columns.first());
                if !indexed {
                    lints.push(json!({
                        "name": "unindexed_foreign_keys",
                        "title": "Unindexed foreign keys",
                        "level": "INFO",
                        "facing": "EXTERNAL",
                        "categories": ["PERFORMANCE"],
                        "description": "Identifies foreign key constraints without a covering index, which can impact database performance.",
                        "detail": format!("Table `{}.{}` has a foreign key `{}` without a covering index. This can lead to suboptimal query performance.", d.schema, d.name, fk.name),
                        "remediation": "https://supabase.com/docs/guides/database/database-linter?lint=0001_unindexed_foreign_keys",
                        "metadata": { "name": d.name, "type": "table", "schema": d.schema, "fkey_name": fk.name, "fkey_columns": fk.columns },
                        "cache_key": format!("unindexed_foreign_keys_{}_{}_{}", d.schema, d.name, fk.name),
                    }));
                }
            }
        }
    }
    let _ = r;
    lints
}

fn ts_type(c: &ColumnInfo) -> String {
    if !c.enums.is_empty() {
        return format!("Database[\"public\"][\"Enums\"][\"{}\"]", c.format);
    }
    let base = c.format.trim_start_matches('_');
    let t = match base {
        "int8" | "int4" | "int2" | "numeric" | "float4" | "float8" => "number",
        "bool" => "boolean",
        "json" | "jsonb" => "Json",
        _ => "string",
    };
    if c.format.starts_with('_') {
        format!("{t}[]")
    } else {
        t.to_owned()
    }
}

/// `supabase gen types typescript` output for the project's non-system schemas.
pub fn typescript(db: &Db) -> String {
    let mut schemas: Vec<String> = Vec::new();
    for t in &db.tables {
        if !schemas.contains(&t.def.schema) {
            schemas.push(t.def.schema.clone());
        }
    }
    if !schemas.contains(&"public".to_owned()) {
        schemas.insert(0, "public".to_owned());
    }
    let mut out = String::from(
        "export type Json =\n  | string\n  | number\n  | boolean\n  | null\n  | { [key: string]: Json | undefined }\n  | Json[]\n\nexport type Database = {\n",
    );
    for schema in &schemas {
        out.push_str(&format!("  {schema}: {{\n    Tables: {{\n"));
        let tables: Vec<&Table> = db
            .tables
            .iter()
            .filter(|t| &t.def.schema == schema)
            .collect();
        if tables.is_empty() {
            out.push_str("      [_ in never]: never\n");
        }
        for t in &tables {
            let d = &t.def;
            out.push_str(&format!("      {}: {{\n", d.name));
            for (section, optional_all) in [
                ("Row", None),
                ("Insert", Some(false)),
                ("Update", Some(true)),
            ] {
                out.push_str(&format!("        {section}: {{\n"));
                for c in &d.columns {
                    let nullable = if c.nullable { " | null" } else { "" };
                    let optional = match optional_all {
                        None => false,
                        Some(true) => true,
                        Some(false) => c.nullable || c.default.is_some() || c.identity.is_some(),
                    };
                    out.push_str(&format!(
                        "          {}{}: {}{}\n",
                        c.name,
                        if optional { "?" } else { "" },
                        ts_type(c),
                        nullable
                    ));
                }
                out.push_str("        }\n");
            }
            out.push_str("        Relationships: [");
            if d.foreign_keys.is_empty() {
                out.push_str("]\n");
            } else {
                out.push('\n');
                for fk in &d.foreign_keys {
                    let target = fk
                        .target
                        .split_once('.')
                        .map_or(fk.target.as_str(), |(_, n)| n);
                    out.push_str(&format!(
                        "          {{\n            foreignKeyName: \"{}\"\n            columns: [{}]\n            isOneToOne: false\n            referencedRelation: \"{target}\"\n            referencedColumns: [{}]\n          }},\n",
                        fk.name,
                        fk.columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
                        fk.target_columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
                    ));
                }
                out.push_str("        ]\n");
            }
            out.push_str("      }\n");
        }
        out.push_str("    }\n    Views: {\n      [_ in never]: never\n    }\n    Functions: {\n      [_ in never]: never\n    }\n    Enums: {\n");
        let mut enums: Vec<(&str, &Vec<String>)> = Vec::new();
        for t in &tables {
            for c in &t.def.columns {
                if !c.enums.is_empty() && !enums.iter().any(|(n, _)| *n == c.format) {
                    enums.push((&c.format, &c.enums));
                }
            }
        }
        if enums.is_empty() {
            out.push_str("      [_ in never]: never\n");
        }
        for (name, values) in enums {
            out.push_str(&format!(
                "      {name}:\n{}\n",
                values
                    .iter()
                    .map(|v| format!("        | \"{v}\""))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        out.push_str("    }\n    CompositeTypes: {\n      [_ in never]: never\n    }\n  }\n");
    }
    out.push_str("}\n");
    out
}
