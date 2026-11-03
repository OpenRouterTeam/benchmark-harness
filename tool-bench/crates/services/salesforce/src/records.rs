//! Record writes: type coercion and validation per field, system and derived fields
//! (`Name`, opportunity stage math, case/task closing, activity dates), soft deletes.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use mock_core::{store::Store, world::ServiceState};
use serde_json::{Map, Value, json};

use crate::{
    db::{Db, SfError, builtin_rows, current_user_id},
    schema::{Field, Object, STAGES, Schema, Ty},
    soql::person_name,
    util::{
        DAY_MS, civil_from_days, format_date, format_datetime, minted_id, normalize_id, parse_date,
        parse_datetime,
    },
};

/// Objects whose rows are written only through the Tooling API.
fn tooling_only_writes(name: &str) -> bool {
    matches!(
        name,
        "ApexClass" | "ApexTrigger" | "ApexLog" | "TraceFlag" | "DebugLevel"
    )
}

pub fn check_writable(o: &Object, tooling: bool, op: &str) -> Result<(), SfError> {
    let allowed = match op {
        "insert" => o.createable,
        "update" => o.updateable,
        _ => o.deletable,
    } && (tooling || !tooling_only_writes(&o.name))
        && o.name != "ApexLog";
    if allowed {
        Ok(())
    } else {
        let verb = match op {
            "insert" => "inserted",
            "update" => "updated",
            _ => "deleted",
        };
        Err(SfError::bad(
            "INVALID_TYPE_FOR_OPERATION",
            format!("entity type cannot be {verb}: {}", o.label),
        ))
    }
}

fn type_error(f: &Field, value: &Value) -> SfError {
    let shown = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let kind = match f.ty {
        Ty::Date => "date",
        Ty::DateTime => "datetime",
        Ty::Boolean => "boolean",
        Ty::Int => "int",
        t if t.is_numeric() => "double",
        _ => "string",
    };
    SfError::bad(
        "JSON_PARSER_ERROR",
        format!(
            "Cannot deserialize instance of {kind} from VALUE_STRING value {shown} or request may be missing a required field"
        ),
    )
}

/// The stored form of `value` for `f`, or the error Salesforce returns.
pub fn coerce(db: &Db, f: &Field, value: &Value) -> Result<Value, SfError> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let as_text = || match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    Ok(match f.ty {
        Ty::Boolean => match value {
            Value::Bool(b) => json!(b),
            Value::String(s) if s.eq_ignore_ascii_case("true") => json!(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => json!(false),
            other => return Err(type_error(f, other)),
        },
        Ty::Int => match value {
            Value::Number(n) if n.as_f64().is_some_and(|x| x.fract() == 0.0) => {
                json!(n.as_f64().unwrap_or(0.0) as i64)
            }
            Value::String(s) if s.trim().parse::<i64>().is_ok() => {
                json!(s.trim().parse::<i64>().expect("checked"))
            }
            other => return Err(type_error(f, other)),
        },
        t if t.is_numeric() => match value {
            Value::Number(n) => Value::Number(n.clone()),
            Value::String(s) if s.trim().parse::<f64>().is_ok() => {
                json!(s.trim().parse::<f64>().expect("checked"))
            }
            other => return Err(type_error(f, other)),
        },
        Ty::Date => {
            let s = as_text().ok_or_else(|| type_error(f, value))?;
            let d = parse_date(s.get(..10).unwrap_or(&s)).ok_or_else(|| type_error(f, value))?;
            json!(format_date(d))
        }
        Ty::DateTime => {
            let s = as_text().ok_or_else(|| type_error(f, value))?;
            let ms = parse_datetime(&s)
                .or_else(|| parse_date(&s).map(|d| d * DAY_MS))
                .ok_or_else(|| type_error(f, value))?;
            json!(format_datetime(ms))
        }
        Ty::Id | Ty::Reference => {
            let s = as_text().ok_or_else(|| type_error(f, value))?;
            let Some(id) = normalize_id(&s) else {
                return Err(SfError::bad(
                    "MALFORMED_ID",
                    format!("{}: id value of incorrect type: {s}", f.label),
                )
                .with_fields(&[&f.name]));
            };
            let target = db.schema.by_prefix(&id).map(|o| o.name.clone());
            if !target
                .as_ref()
                .is_some_and(|t| f.refs.iter().any(|r| r == t))
            {
                return Err(SfError::bad(
                    "FIELD_INTEGRITY_EXCEPTION",
                    format!("{}: id value of incorrect type: {s}", f.label),
                )
                .with_fields(&[&f.name]));
            }
            match db.find(&id) {
                None => {
                    return Err(SfError::bad(
                        "INSUFFICIENT_ACCESS_ON_CROSS_REFERENCE_ENTITY",
                        format!("insufficient access rights on cross-reference id: {s}"),
                    )
                    .with_fields(&[&f.name]));
                }
                Some((_, r)) if r["IsDeleted"] == true => {
                    return Err(SfError::bad("ENTITY_IS_DELETED", "entity is deleted")
                        .with_fields(&[&f.name]));
                }
                Some(_) => json!(id),
            }
        }
        _ => {
            let s = as_text().ok_or_else(|| type_error(f, value))?;
            if s.chars().count() > f.length as usize && f.length > 0 {
                return Err(SfError::bad(
                    "STRING_TOO_LONG",
                    format!(
                        "{}: data value too large: {s} (max length={})",
                        f.label, f.length
                    ),
                )
                .with_fields(&[&f.name]));
            }
            if f.ty == Ty::Email && !s.is_empty() {
                let ok = s.split_once('@').is_some_and(|(a, b)| {
                    !a.is_empty()
                        && b.contains('.')
                        && !b.starts_with('.')
                        && !b.ends_with('.')
                        && !s.contains(' ')
                });
                if !ok {
                    return Err(SfError::bad(
                        "INVALID_EMAIL_ADDRESS",
                        format!("{}: invalid email address: {s}", f.label),
                    )
                    .with_fields(&[&f.name]));
                }
            }
            json!(s)
        }
    })
}

/// Validated, coerced input fields for an insert or update.
fn input_fields(
    db: &Db,
    o: &Object,
    input: &Map<String, Value>,
    insert: bool,
) -> Result<Vec<(Field, Value)>, SfError> {
    let mut out = Vec::new();
    let mut locked = Vec::new();
    for (key, value) in input {
        if key == "attributes" || (!insert && key.eq_ignore_ascii_case("Id")) {
            continue;
        }
        let Some(f) = o.field(key) else {
            return Err(SfError::bad(
                "INVALID_FIELD",
                format!("No such column '{key}' on sobject of type {}", o.name),
            ));
        };
        let allowed = if insert { f.createable } else { f.updateable };
        if !allowed {
            locked.push(f.name.clone());
            continue;
        }
        out.push((f.clone(), coerce(db, f, value)?));
    }
    if !locked.is_empty() {
        let names = locked.join(", ");
        return Err(SfError::bad(
            "INVALID_FIELD_FOR_INSERT_UPDATE",
            format!("Unable to create/update fields: {names}. Please check the security settings of this field and verify that it is read/write for your profile or permission set."),
        )
        .with_fields(&locked.iter().map(String::as_str).collect::<Vec<_>>()));
    }
    Ok(out)
}

fn missing_required(o: &Object, rec: &Map<String, Value>) -> Option<SfError> {
    let missing: Vec<&str> = o
        .fields
        .iter()
        .filter(|f| !f.nillable && f.createable && !f.defaulted)
        .filter(|f| rec.get(&f.name).is_none_or(|v| v.is_null() || v == ""))
        .map(|f| f.name.as_str())
        .collect();
    (!missing.is_empty()).then(|| {
        SfError::bad(
            "REQUIRED_FIELD_MISSING",
            format!("Required fields are missing: [{}]", missing.join(", ")),
        )
        .with_fields(&missing)
    })
}

fn stage_info(stage: &str) -> Option<&'static (&'static str, f64, &'static str, &'static str)> {
    STAGES.iter().find(|s| s.0 == stage)
}

/// Fields Salesforce computes from others.
fn derive(
    db: &Db,
    o: &Object,
    rec: &mut Map<String, Value>,
    prev: Option<&Map<String, Value>>,
    explicit: &BTreeSet<String>,
) {
    let now = format_datetime(db.now_ms);
    let get =
        |rec: &Map<String, Value>, k: &str| rec.get(k).and_then(Value::as_str).map(str::to_owned);
    match o.name.as_str() {
        "Contact" | "Lead" | "User" => {
            let name = person_name(&Value::Object(rec.clone()));
            rec.insert("Name".into(), json!(name));
        }
        "Opportunity" => {
            let stage = get(rec, "StageName").unwrap_or_default();
            let stage_changed = prev.is_none_or(|p| p.get("StageName") != rec.get("StageName"));
            if let Some(info) = stage_info(&stage) {
                if stage_changed && !explicit.contains("Probability") {
                    rec.insert("Probability".into(), json!(info.1));
                }
                if stage_changed && !explicit.contains("ForecastCategoryName") {
                    rec.insert("ForecastCategoryName".into(), json!(info.3));
                }
                rec.insert("IsClosed".into(), json!(stage.starts_with("Closed")));
                rec.insert("IsWon".into(), json!(stage == "Closed Won"));
            } else {
                rec.insert("IsClosed".into(), json!(false));
                rec.insert("IsWon".into(), json!(false));
            }
            let fc = match get(rec, "ForecastCategoryName").as_deref() {
                Some("Best Case") => "BestCase",
                Some("Commit") => "Forecast",
                Some("Closed") => "Closed",
                Some("Omitted") => "Omitted",
                _ => "Pipeline",
            };
            rec.insert("ForecastCategory".into(), json!(fc));
            let amount = rec.get("Amount").and_then(Value::as_f64);
            let prob = rec.get("Probability").and_then(Value::as_f64);
            rec.insert(
                "ExpectedRevenue".into(),
                match (amount, prob) {
                    (Some(a), Some(p)) => json!(a * p / 100.0),
                    _ => Value::Null,
                },
            );
            if let Some(d) = get(rec, "CloseDate").and_then(|s| parse_date(&s)) {
                let (y, m, _) = civil_from_days(d);
                let q = (m - 1) / 3 + 1;
                rec.insert("FiscalYear".into(), json!(y));
                rec.insert("FiscalQuarter".into(), json!(q));
                rec.insert("Fiscal".into(), json!(format!("{y} {q}")));
            }
        }
        "Case" => {
            let closed = get(rec, "Status").as_deref() == Some("Closed");
            let was_closed = prev.is_some_and(|p| p.get("IsClosed") == Some(&Value::Bool(true)));
            rec.insert("IsClosed".into(), json!(closed));
            if closed && !was_closed {
                rec.insert("ClosedDate".into(), json!(now));
            } else if !closed {
                rec.insert("ClosedDate".into(), Value::Null);
            }
        }
        "Task" => {
            let closed = get(rec, "Status").as_deref() == Some("Completed");
            let was_closed = prev.is_some_and(|p| p.get("IsClosed") == Some(&Value::Bool(true)));
            rec.insert("IsClosed".into(), json!(closed));
            if closed && !was_closed {
                rec.insert("CompletedDateTime".into(), json!(now));
            } else if !closed {
                rec.insert("CompletedDateTime".into(), Value::Null);
            }
            rec.insert(
                "IsHighPriority".into(),
                json!(get(rec, "Priority").as_deref() == Some("High")),
            );
            let account = |id: Option<String>| -> Option<String> {
                let id = id?;
                let (obj, r) = db.find(&id)?;
                if obj.name == "Account" {
                    Some(id)
                } else {
                    r.get("AccountId")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }
            };
            let acct = account(get(rec, "WhatId")).or_else(|| account(get(rec, "WhoId")));
            rec.insert("AccountId".into(), acct.map_or(Value::Null, Value::String));
        }
        "ApexClass" | "ApexTrigger" => {
            let body = get(rec, "Body").unwrap_or_default();
            rec.insert(
                "LengthWithoutComments".into(),
                json!(strip_comments(&body).chars().count()),
            );
            rec.insert("BodyCrc".into(), json!(f64::from(crc32(body.as_bytes()))));
            rec.insert("IsValid".into(), json!(true));
            if o.name == "ApexTrigger" {
                let header = body.split('{').next().unwrap_or("").to_lowercase();
                for (field, words) in [
                    ("UsageBeforeInsert", "before insert"),
                    ("UsageAfterInsert", "after insert"),
                    ("UsageBeforeUpdate", "before update"),
                    ("UsageAfterUpdate", "after update"),
                    ("UsageBeforeDelete", "before delete"),
                    ("UsageAfterDelete", "after delete"),
                    ("UsageAfterUndelete", "after undelete"),
                ] {
                    rec.insert(
                        field.into(),
                        json!(
                            header
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                .contains(words)
                        ),
                    );
                }
                rec.insert("UsageIsBulk".into(), json!(true));
            }
        }
        _ => {}
    }
}

pub fn strip_comments(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("//") {
            rest = r.find('\n').map_or("", |i| &r[i..]);
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.find("*/").map_or("", |i| &r[i + 2..]);
        } else {
            let c = rest.chars().next().expect("non-empty");
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Copies built-in rows into the store before the first write to that object.
fn materialize(store: &mut Store, object: &str) {
    if store.collection(object).is_none() {
        let rows = builtin_rows(object);
        let c = store.collection_mut(object);
        for r in rows {
            if let Some(id) = r["Id"].as_str() {
                c.insert(id, r.clone());
            }
        }
    }
}

fn next_id(state: &mut ServiceState, o: &Object) -> String {
    loop {
        let n = state.ids.next_int(&format!("id/{}", o.prefix), 1);
        let id = minted_id(&o.prefix, n);
        if !state
            .store
            .collection(&o.name)
            .is_some_and(|c| c.contains(&id))
        {
            return id;
        }
    }
}

/// Inserts a record. Returns its Id.
pub fn insert(
    state: &mut ServiceState,
    schema: &Schema,
    now_ms: i64,
    version: &str,
    tooling: bool,
    o: &Object,
    input: &Map<String, Value>,
) -> Result<String, SfError> {
    check_writable(o, tooling, "insert")?;
    let user = current_user_id(&state.store);
    let (mut rec, explicit) = {
        let db = Db {
            schema,
            store: &state.store,
            now_ms,
            version,
            tooling,
        };
        let fields = input_fields(&db, o, input, true)?;
        let explicit: BTreeSet<String> = fields.iter().map(|(f, _)| f.name.clone()).collect();
        let mut rec = Map::new();
        for f in &o.fields {
            let v = fields
                .iter()
                .find(|(ff, _)| ff.name == f.name)
                .map(|(_, v)| v.clone())
                .or_else(|| f.default_pick.clone().map(Value::String))
                .unwrap_or(if f.ty == Ty::Boolean {
                    json!(false)
                } else {
                    Value::Null
                });
            rec.insert(f.name.clone(), v);
        }
        (rec, explicit)
    };
    let now = format_datetime(now_ms);
    for (k, v) in [
        ("IsDeleted", json!(false)),
        ("CreatedDate", json!(now)),
        ("CreatedById", user.clone()),
        ("LastModifiedDate", json!(now)),
        ("LastModifiedById", user.clone()),
        ("SystemModstamp", json!(now)),
    ] {
        if rec.contains_key(k) {
            rec.insert(k.into(), v);
        }
    }
    if rec.contains_key("OwnerId") && rec["OwnerId"].is_null() {
        rec.insert("OwnerId".into(), user.clone());
    }
    if let Some(f) = o.fields.iter().find(|f| f.auto_number) {
        let max = state
            .store
            .collection(&o.name)
            .map(|c| {
                c.values()
                    .filter_map(|r| r[&f.name].as_str())
                    .filter_map(|s| {
                        s.trim_start_matches(|c: char| !c.is_ascii_digit())
                            .parse::<u64>()
                            .ok()
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        rec.insert(f.name.clone(), json!(format!("{:08}", max + 1)));
    }
    if o.name == "ApexClass" || o.name == "ApexTrigger" {
        let name = rec
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if let Some(existing) = state.store.collection(&o.name).and_then(|c| {
            c.values()
                .find(|r| {
                    r["Name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&name))
                })
                .cloned()
        }) {
            return Err(SfError::bad(
                "DUPLICATE_VALUE",
                format!(
                    "duplicate value found: Name duplicates value on record with id: {}",
                    existing["Id"].as_str().unwrap_or("")
                ),
            )
            .with_fields(&["Name"]));
        }
    }
    if let Some(e) = missing_required(o, &rec) {
        return Err(e);
    }
    {
        let db = Db {
            schema,
            store: &state.store,
            now_ms,
            version,
            tooling,
        };
        derive(&db, o, &mut rec, None, &explicit);
    }
    if o.name == "User" && rec.get("CommunityNickname").is_none_or(Value::is_null) {
        let nick = rec
            .get("Username")
            .and_then(Value::as_str)
            .and_then(|u| u.split('@').next())
            .unwrap_or("user")
            .to_owned();
        rec.insert("CommunityNickname".into(), json!(nick));
    }
    if o.name == "User" && rec.get("UserType").is_none_or(Value::is_null) {
        rec.insert("UserType".into(), json!("Standard"));
    }
    materialize(&mut state.store, &o.name);
    let id = next_id(state, o);
    rec.insert("Id".into(), json!(id));
    let done = Value::Object(rec);
    touch_activity(state, &done);
    state.store.collection_mut(&o.name).insert(id.clone(), done);
    Ok(id)
}

/// Updates a record by Id.
#[allow(clippy::too_many_arguments)]
pub fn update(
    state: &mut ServiceState,
    schema: &Schema,
    now_ms: i64,
    version: &str,
    tooling: bool,
    o: &Object,
    id: &str,
    input: &Map<String, Value>,
) -> Result<String, SfError> {
    check_writable(o, tooling, "update")?;
    let user = current_user_id(&state.store);
    let id = locate(state, schema, o, id)?;
    let rec = {
        let db = Db {
            schema,
            store: &state.store,
            now_ms,
            version,
            tooling,
        };
        let prev = db
            .get(&o.name, &id)
            .cloned()
            .ok_or_else(SfError::not_found)?;
        let fields = input_fields(&db, o, input, false)?;
        let explicit: BTreeSet<String> = fields.iter().map(|(f, _)| f.name.clone()).collect();
        let prev_map = prev.as_object().cloned().unwrap_or_default();
        let mut rec = prev_map.clone();
        for (f, v) in fields {
            if !f.nillable && v.is_null() && !f.defaulted {
                return Err(SfError::bad(
                    "REQUIRED_FIELD_MISSING",
                    format!("Required fields are missing: [{}]", f.name),
                )
                .with_fields(&[&f.name]));
            }
            rec.insert(f.name.clone(), v);
        }
        let now = format_datetime(now_ms);
        for (k, v) in [
            ("LastModifiedDate", json!(now)),
            ("LastModifiedById", user),
            ("SystemModstamp", json!(now)),
        ] {
            if rec.contains_key(k) || o.has_field(k) {
                rec.insert(k.into(), v);
            }
        }
        derive(&db, o, &mut rec, Some(&prev_map), &explicit);
        rec
    };
    materialize(&mut state.store, &o.name);
    let done = Value::Object(rec);
    touch_activity(state, &done);
    state.store.collection_mut(&o.name).insert(id.clone(), done);
    Ok(id)
}

/// The stored Id for `id` on `o`, or the REST error for a missing/deleted/foreign record.
pub fn locate(
    state: &ServiceState,
    schema: &Schema,
    o: &Object,
    id: &str,
) -> Result<String, SfError> {
    let Some(full) = normalize_id(id) else {
        return Err(SfError::bad("MALFORMED_ID", format!("malformed id {id}")));
    };
    if !full.starts_with(&o.prefix) {
        return Err(SfError::bad(
            "MALFORMED_ID",
            format!("{}: id value of incorrect type: {id}", o.label),
        ));
    }
    let db = Db {
        schema,
        store: &state.store,
        now_ms: 0,
        version: "",
        tooling: false,
    };
    match db.get(&o.name, &full) {
        None => Err(SfError::not_found()),
        Some(r) if r["IsDeleted"] == true => Err(SfError::new(
            StatusCode::NOT_FOUND,
            "ENTITY_IS_DELETED",
            "entity is deleted",
        )),
        Some(_) => Ok(full),
    }
}

/// Soft-deletes a record (and cascades to children with cascade-delete lookups).
pub fn delete(
    state: &mut ServiceState,
    schema: &Schema,
    now_ms: i64,
    tooling: bool,
    o: &Object,
    id: &str,
) -> Result<String, SfError> {
    check_writable(o, tooling, "delete")?;
    let id = locate(state, schema, o, id)?;
    if o.name == "TraceFlag" || o.name == "DebugLevel" || o.name == "FieldPermissions" {
        materialize(&mut state.store, &o.name);
        state.store.collection_mut(&o.name).remove(&id);
        return Ok(id);
    }
    let now = format_datetime(now_ms);
    let user = current_user_id(&state.store);
    let mut targets = vec![(o.name.clone(), id.clone())];
    for rel in o.children.iter().filter(|c| c.cascade) {
        if let Some(c) = state.store.collection(&rel.child) {
            for (k, r) in c.iter() {
                if r.get(&rel.field).and_then(Value::as_str) == Some(id.as_str())
                    && r["IsDeleted"] != true
                {
                    targets.push((rel.child.clone(), k.to_owned()));
                }
            }
        }
    }
    for (obj, key) in targets {
        if let Some(r) = state.store.collection_mut(&obj).get_mut(&key) {
            r["IsDeleted"] = json!(true);
            r["LastModifiedDate"] = json!(now);
            r["LastModifiedById"] = user.clone();
            r["SystemModstamp"] = json!(now);
        }
    }
    Ok(id)
}

/// Completed tasks move `LastActivityDate` on the records they relate to.
fn touch_activity(state: &mut ServiceState, rec: &Value) {
    if rec.get("Status").and_then(Value::as_str) != Some("Completed") || rec.get("WhoId").is_none()
    {
        return;
    }
    let Some(date) = rec
        .get("ActivityDate")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    for key in ["WhoId", "WhatId", "AccountId"] {
        let Some(id) = rec.get(key).and_then(Value::as_str) else {
            continue;
        };
        for obj in ["Contact", "Lead", "Account", "Opportunity"] {
            if let Some(r) = state.store.collection_mut(obj).get_mut(id)
                && r.get("LastActivityDate")
                    .and_then(Value::as_str)
                    .is_none_or(|d| d < date.as_str())
            {
                r["LastActivityDate"] = json!(date);
            }
        }
    }
}

/// Full record for `GET /sobjects/{type}/{id}`: attributes then every field, or `fields`.
pub fn full_record(
    db: &Db,
    o: &Object,
    rec: &Value,
    fields: Option<&[String]>,
) -> Result<Value, SfError> {
    let mut out = Map::new();
    out.insert("attributes".into(), db.attributes(&o.name, rec));
    match fields {
        Some(list) => {
            for name in list {
                let Some(f) = o.field(name.trim()) else {
                    return Err(SfError::bad(
                        "INVALID_FIELD",
                        format!(
                            "No such column '{}' on sobject of type {}",
                            name.trim(),
                            o.name
                        ),
                    ));
                };
                out.insert(
                    f.name.clone(),
                    crate::soql::field_value(rec, &o.name, &f.name),
                );
            }
        }
        None => {
            for f in &o.fields {
                out.insert(
                    f.name.clone(),
                    crate::soql::field_value(rec, &o.name, &f.name),
                );
            }
        }
    }
    Ok(Value::Object(out))
}

/// Upsert key lookup: matching live records of `o` where `field` equals `value`.
pub fn find_by_external(db: &Db, o: &Object, field: &Field, value: &str) -> Vec<String> {
    db.rows(&o.name)
        .into_iter()
        .filter(|r| r["IsDeleted"] != true)
        .filter(|r| match &r[&field.name] {
            Value::String(s) => s.eq_ignore_ascii_case(value),
            Value::Number(n) => n.to_string() == value,
            _ => false,
        })
        .filter_map(|r| r["Id"].as_str().map(str::to_owned))
        .collect()
}

pub fn today(now_ms: i64) -> String {
    format_date(now_ms.div_euclid(DAY_MS))
}
