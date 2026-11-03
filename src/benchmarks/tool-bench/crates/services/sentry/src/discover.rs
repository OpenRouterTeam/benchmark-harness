//! The events (Discover / Explore) API over the dataset: `/events/`, `/events/validate/`,
//! `/events-stats/`, trace-item attributes, and traces.

use std::collections::BTreeMap;

use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Uri},
    response::Response,
};
use mock_core::world::{SharedWorld, WorldState};
use serde_json::{Map, Value, json};

use crate::{
    issues::{event_tags, issue_by_id_any},
    org::{find_project, org_exists},
    search::{self, Term},
    util::{
        Query, bad_request, docs, not_found, now, ok, page, paginate, str_of, time_window,
        value_time, with_link,
    },
};

/// The flat Discover row of an error event.
pub fn error_row(world: &WorldState, e: &Value) -> Map<String, Value> {
    let mut row = Map::new();
    let project = e["project"].as_str().unwrap_or("");
    let p = find_project(world, project);
    let issue = issue_by_id_any(world, &str_of(&e["groupID"]));
    for t in event_tags(world, e) {
        if let (Some(k), Some(v)) = (t["key"].as_str(), t["value"].as_str()) {
            row.insert(k.to_owned(), json!(v));
            row.insert(format!("tags[{k}]"), json!(v));
        }
    }
    let exc = &e["exception"];
    let mut set = |k: &str, v: Value| {
        row.insert(k.to_owned(), v);
    };
    set("id", e["eventID"].clone());
    set("event.id", e["eventID"].clone());
    set(
        "event.type",
        e.get("type").cloned().unwrap_or(json!("error")),
    );
    set("title", e["title"].clone());
    set(
        "message",
        e.get("message")
            .cloned()
            .filter(|m| !m.is_null())
            .unwrap_or_else(|| e["title"].clone()),
    );
    set("timestamp", e["dateCreated"].clone());
    set("level", e.get("level").cloned().unwrap_or(json!("error")));
    set("culprit", e.get("culprit").cloned().unwrap_or(Value::Null));
    set(
        "platform",
        e.get("platform").cloned().unwrap_or(Value::Null),
    );
    set("project", json!(project));
    set("project.name", json!(project));
    set(
        "project.id",
        p.map_or(Value::Null, |p| {
            str_of(&p["id"])
                .parse::<u64>()
                .map_or(json!(str_of(&p["id"])), Value::from)
        }),
    );
    set("issue", issue.map_or(Value::Null, |i| i["shortId"].clone()));
    set(
        "issue.id",
        str_of(&e["groupID"])
            .parse::<u64>()
            .map_or(Value::Null, Value::from),
    );
    set("release", e.get("release").cloned().unwrap_or(Value::Null));
    set(
        "environment",
        e.get("environment").cloned().unwrap_or(Value::Null),
    );
    set("trace", e.get("trace").cloned().unwrap_or(Value::Null));
    set(
        "transaction",
        e.get("transaction").cloned().unwrap_or(Value::Null),
    );
    if exc.is_object() {
        set("error.type", json!([exc["type"].clone()]));
        set("error.value", json!([exc["value"].clone()]));
        set(
            "error.handled",
            json!([exc["mechanism"]["handled"].as_bool().unwrap_or(false)]),
        );
        set(
            "error.unhandled",
            json!(!exc["mechanism"]["handled"].as_bool().unwrap_or(false)),
        );
        if let Some(top) = exc["frames"].as_array().and_then(|f| f.last()) {
            set("stack.filename", top["filename"].clone());
            set("stack.function", top["function"].clone());
        }
    }
    let user = &e["user"];
    for (k, f) in [
        ("user.id", "id"),
        ("user.email", "email"),
        ("user.username", "username"),
        ("user.ip", "ip_address"),
    ] {
        set(k, user.get(f).cloned().unwrap_or(Value::Null));
    }
    set(
        "user.display",
        user["email"]
            .as_str()
            .or_else(|| user["username"].as_str())
            .or_else(|| user["id"].as_str())
            .or_else(|| user["ip_address"].as_str())
            .map_or(Value::Null, |s| json!(s)),
    );
    set("http.url", e["request"]["url"].clone());
    set("url", e["request"]["url"].clone());
    set("http.method", e["request"]["method"].clone());
    row
}

/// Spans, logs, and other trace items are stored flat already; tags merge in.
fn item_row(doc: &Value) -> Map<String, Value> {
    let mut row = doc.as_object().cloned().unwrap_or_default();
    if let Some(tags) = doc["tags"].as_object() {
        for (k, v) in tags {
            row.entry(k.clone()).or_insert_with(|| v.clone());
            row.insert(format!("tags[{k}]"), v.clone());
        }
    }
    if let Some(attrs) = doc["attributes"].as_object() {
        for (k, v) in attrs {
            row.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if !row.contains_key("id")
        && let Some(id) = doc.get("span_id").or_else(|| doc.get("item_id"))
    {
        row.insert("id".into(), id.clone());
    }
    row
}

pub fn row_matches(row: &Map<String, Value>, terms: &[Term], now: i64) -> bool {
    let lookup = |k: &str| {
        let k = if k == "issue.short_id" { "issue" } else { k };
        row.get(k)
            .cloned()
            .or_else(|| row.get(&format!("tags[{k}]")).cloned())
    };
    let text = |t: &str| {
        let needle = t.to_lowercase();
        [
            "message",
            "title",
            "span.description",
            "log.body",
            "body",
            "transaction",
        ]
        .iter()
        .any(|k| {
            row.get(*k).and_then(Value::as_str).is_some_and(|s| {
                if needle.contains('*') {
                    search::glob(&format!("*{needle}*"), s)
                } else {
                    s.to_lowercase().contains(&needle)
                }
            })
        })
    };
    let custom = |term: &Term| -> Option<bool> {
        if let Term::Filter {
            negated,
            key,
            values,
            ..
        } = term
            && (key == "issue" || key == "issue.short_id")
        {
            let actual = row.get("issue").and_then(Value::as_str).unwrap_or("");
            let hit = values.iter().any(|v| v.eq_ignore_ascii_case(actual));
            return Some(hit != *negated);
        }
        None
    };
    search::matches(terms, now, &lookup, &text, &custom)
}

const FUNCTIONS: [&str; 22] = [
    "count",
    "count_unique",
    "count_if",
    "last_seen",
    "first_seen",
    "min",
    "max",
    "avg",
    "sum",
    "p50",
    "p75",
    "p90",
    "p95",
    "p99",
    "p100",
    "epm",
    "eps",
    "failure_rate",
    "failure_count",
    "count_miserable",
    "apdex",
    "any",
];

fn parse_function(field: &str) -> Option<(&str, &str)> {
    let open = field.find('(')?;
    let name = &field[..open];
    let arg = field[open + 1..].strip_suffix(')')?;
    Some((name, arg.trim()))
}

fn is_aggregate(field: &str) -> bool {
    parse_function(field).is_some()
}

fn field_type(field: &str) -> &'static str {
    if let Some((name, arg)) = parse_function(field) {
        return match name {
            "count" | "count_unique" | "count_if" | "failure_count" | "count_miserable" => {
                "integer"
            }
            "last_seen" | "first_seen" => "date",
            "epm" | "eps" | "failure_rate" | "apdex" => "number",
            _ if arg.contains("duration") || arg == "transaction.duration" => "duration",
            _ if arg == "timestamp" => "date",
            _ => "number",
        };
    }
    match field {
        "timestamp" | "first_seen" | "last_seen" => "date",
        "span.duration" | "transaction.duration" | "span.self_time" => "duration",
        "issue.id" | "project.id" | "value" => "integer",
        "error.handled" | "error.unhandled" | "is_transaction" => "boolean",
        "error.type" | "error.value" | "stack.filename" => "array",
        _ => "string",
    }
}

fn numbers(rows: &[&Map<String, Value>], arg: &str) -> Vec<f64> {
    rows.iter()
        .filter_map(|r| match r.get(arg) {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        })
        .collect()
}

fn percentile(mut v: Vec<f64>, p: f64) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let idx = ((p * (v.len() - 1) as f64).round() as usize).min(v.len() - 1);
    json!(v[idx])
}

fn aggregate(
    field: &str,
    rows: &[&Map<String, Value>],
    period_secs: i64,
    now: i64,
) -> Result<Value, String> {
    let (name, arg) = parse_function(field).ok_or_else(|| format!("{field} is not a function"))?;
    if !FUNCTIONS.contains(&name) {
        return Err(format!("{name} is not a valid function"));
    }
    let times: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.get("timestamp").and_then(value_time))
        .collect();
    let fmt = |t: Option<&i64>| t.map_or(Value::Null, |t| json!(mock_core::clock::format_unix(*t)));
    Ok(match name {
        "count" => json!(rows.len()),
        "count_unique" => {
            let mut seen: Vec<String> = rows
                .iter()
                .filter_map(|r| r.get(arg).filter(|v| !v.is_null()).map(str_of))
                .collect();
            seen.sort();
            seen.dedup();
            json!(seen.len())
        }
        "count_if" => {
            let parts: Vec<&str> = arg.split(',').map(str::trim).collect();
            let (f, cond, val) = (parts.first(), parts.get(1), parts.get(2));
            let n = rows
                .iter()
                .filter(|r| {
                    let actual = r.get(*f.unwrap_or(&"")).cloned().unwrap_or(Value::Null);
                    let op = match cond.copied() {
                        Some("greater") => search::Op::Gt,
                        Some("less") => search::Op::Lt,
                        Some("greaterOrEquals") => search::Op::Gte,
                        Some("lessOrEquals") => search::Op::Lte,
                        _ => search::Op::Eq,
                    };
                    let hit = search::compare(&actual, op, val.copied().unwrap_or(""), now);
                    if cond.copied() == Some("notEquals") {
                        !hit
                    } else {
                        hit
                    }
                })
                .count();
            json!(n)
        }
        "last_seen" => fmt(times.iter().max()),
        "first_seen" => fmt(times.iter().min()),
        "min" | "max" if arg == "timestamp" => fmt(if name == "min" {
            times.iter().min()
        } else {
            times.iter().max()
        }),
        "min" => numbers(rows, arg)
            .into_iter()
            .reduce(f64::min)
            .map_or(Value::Null, |v| json!(v)),
        "max" => numbers(rows, arg)
            .into_iter()
            .reduce(f64::max)
            .map_or(Value::Null, |v| json!(v)),
        "sum" => json!(numbers(rows, arg).iter().sum::<f64>()),
        "avg" => {
            let v = numbers(rows, arg);
            if v.is_empty() {
                Value::Null
            } else {
                json!(v.iter().sum::<f64>() / v.len() as f64)
            }
        }
        "p50" | "p75" | "p90" | "p95" | "p99" | "p100" => {
            let p: f64 = name[1..].parse::<f64>().unwrap_or(50.0) / 100.0;
            let arg = if arg.is_empty() { "span.duration" } else { arg };
            percentile(numbers(rows, arg), p)
        }
        "epm" => json!(rows.len() as f64 / (period_secs.max(60) as f64 / 60.0)),
        "eps" => json!(rows.len() as f64 / period_secs.max(1) as f64),
        "failure_count" => json!(
            rows.iter()
                .filter(|r| r
                    .get("span.status")
                    .or_else(|| r.get("transaction.status"))
                    .and_then(Value::as_str)
                    .is_some_and(|s| !matches!(s, "ok" | "cancelled" | "unknown")))
                .count()
        ),
        "failure_rate" => {
            let failed = rows
                .iter()
                .filter(|r| {
                    r.get("span.status")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !matches!(s, "ok" | "cancelled" | "unknown"))
                })
                .count();
            json!(if rows.is_empty() {
                0.0
            } else {
                failed as f64 / rows.len() as f64
            })
        }
        "any" => rows
            .first()
            .and_then(|r| r.get(arg).cloned())
            .unwrap_or(Value::Null),
        _ => json!(0),
    })
}

/// Group-by key values and the rows in the group.
type Group<'a> = (Vec<Value>, Vec<&'a Map<String, Value>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dataset {
    Errors,
    Spans,
    Logs,
    Empty,
}

fn dataset_of(name: Option<&str>) -> Result<Dataset, String> {
    match name.unwrap_or("discover") {
        "errors" | "discover" | "events" | "issuePlatform" => Ok(Dataset::Errors),
        "spans" | "spansIndexed" | "transactions" => Ok(Dataset::Spans),
        "logs" | "ourlogs" => Ok(Dataset::Logs),
        "tracemetrics" | "metrics" | "profiles" | "profile_functions" | "uptime_results" => {
            Ok(Dataset::Empty)
        }
        other => Err(format!("{other} is not a valid dataset")),
    }
}

/// Rows of a dataset within the request's window and project/environment filters.
fn dataset_rows(
    world: &WorldState,
    dataset: Dataset,
    q: &Query,
    now: i64,
) -> Result<(Vec<Map<String, Value>>, i64), Response> {
    let (from, to) = time_window(q, now, "14d")?;
    let projects: Vec<String> = q
        .all("project")
        .into_iter()
        .filter(|p| *p != "-1")
        .map(str::to_owned)
        .collect();
    let environments = q.all("environment");
    let rows: Vec<Map<String, Value>> = match dataset {
        Dataset::Errors => docs(world, "events")
            .into_iter()
            .map(|e| error_row(world, e))
            .collect(),
        Dataset::Spans => docs(world, "spans").into_iter().map(item_row).collect(),
        Dataset::Logs => docs(world, "logs").into_iter().map(item_row).collect(),
        Dataset::Empty => Vec::new(),
    };
    let rows = rows
        .into_iter()
        .filter(|r| {
            let t = r.get("timestamp").and_then(value_time).unwrap_or(0);
            t >= from && t <= to
        })
        .filter(|r| {
            projects.is_empty() || {
                let pid = r
                    .get("project.id")
                    .map(str_of)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        r.get("project")
                            .and_then(Value::as_str)
                            .and_then(|slug| find_project(world, slug))
                            .map(|p| str_of(&p["id"]))
                    })
                    .unwrap_or_default();
                projects.contains(&pid)
            }
        })
        .filter(|r| {
            environments.is_empty()
                || r.get("environment")
                    .and_then(Value::as_str)
                    .is_some_and(|e| environments.contains(&e))
        })
        .collect();
    Ok((rows, to - from))
}

fn sort_rows(rows: &mut [Map<String, Value>], sort: &str) {
    let (desc, key) = match sort.strip_prefix('-') {
        Some(k) => (true, k),
        None => (false, sort),
    };
    rows.sort_by(|a, b| {
        let va = a.get(key).cloned().unwrap_or(Value::Null);
        let vb = b.get(key).cloned().unwrap_or(Value::Null);
        let ord = match (&va, &vb) {
            (Value::Number(x), Value::Number(y)) => x
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&y.as_f64().unwrap_or(0.0)),
            _ => match (
                value_time(&va).filter(|_| va.is_string()),
                value_time(&vb).filter(|_| vb.is_string()),
            ) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => str_of(&va).cmp(&str_of(&vb)),
            },
        };
        if desc { ord.reverse() } else { ord }
    });
}

/// Runs a Discover-style query: selects `fields` (grouping by the non-aggregates when any
/// aggregate is present), filters by `query`, sorts by `sort`.
fn run_query(
    rows: Vec<Map<String, Value>>,
    fields: &[String],
    query: &str,
    sort: Option<&str>,
    period: i64,
    now: i64,
) -> Result<Vec<Map<String, Value>>, String> {
    let terms = search::parse(query);
    let matched: Vec<Map<String, Value>> = rows
        .into_iter()
        .filter(|r| row_matches(r, &terms, now))
        .collect();
    let aggregates: Vec<&String> = fields.iter().filter(|f| is_aggregate(f)).collect();
    let group_by: Vec<&String> = fields.iter().filter(|f| !is_aggregate(f)).collect();
    let mut out: Vec<Map<String, Value>> = if aggregates.is_empty() {
        matched
            .iter()
            .map(|r| {
                fields
                    .iter()
                    .map(|f| (f.clone(), r.get(f).cloned().unwrap_or(Value::Null)))
                    .collect()
            })
            .collect()
    } else {
        let mut groups: Vec<Group> = Vec::new();
        for r in &matched {
            let key: Vec<Value> = group_by
                .iter()
                .map(|f| r.get(*f).cloned().unwrap_or(Value::Null))
                .collect();
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push(r),
                None => groups.push((key, vec![r])),
            }
        }
        if groups.is_empty() && group_by.is_empty() {
            groups.push((Vec::new(), Vec::new()));
        }
        let mut out = Vec::new();
        for (key, members) in groups {
            let mut row = Map::new();
            for (f, v) in group_by.iter().zip(key) {
                row.insert((*f).clone(), v);
            }
            for f in &aggregates {
                row.insert((*f).clone(), aggregate(f, &members, period, now)?);
            }
            // Keep the selected order.
            let ordered: Map<String, Value> = fields
                .iter()
                .map(|f| (f.clone(), row.get(f).cloned().unwrap_or(Value::Null)))
                .collect();
            out.push(ordered);
        }
        out
    };
    if let Some(sort) = sort.filter(|s| !s.is_empty()) {
        let key = sort.trim_start_matches('-');
        if !fields.iter().any(|f| f == key) {
            return Err("orderby must also be in the selected columns or groupby".into());
        }
        sort_rows(&mut out, sort);
    }
    Ok(out)
}

fn validate_fields(fields: &[String]) -> Result<(), String> {
    for f in fields {
        if let Some((name, _)) = parse_function(f)
            && !FUNCTIONS.contains(&name)
        {
            return Err(format!("{name} is not a valid function"));
        }
    }
    Ok(())
}

pub async fn events(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let dataset = match dataset_of(q.get("dataset")) {
        Ok(d) => d,
        Err(e) => return bad_request(&e),
    };
    let fields: Vec<String> = q.all("field").into_iter().map(str::to_owned).collect();
    if fields.is_empty() {
        return bad_request("No columns selected");
    }
    if let Err(e) = validate_fields(&fields) {
        return bad_request(&e);
    }
    let now = now(&world);
    let (rows, period) = match dataset_rows(&world, dataset, &q, now) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let sort = q.get("sort").or_else(|| q.get("orderby"));
    let result = match run_query(
        rows,
        &fields,
        q.get("query").unwrap_or(""),
        sort,
        period,
        now,
    ) {
        Ok(r) => r,
        Err(e) => return bad_request(&e),
    };
    let meta_fields: Map<String, Value> = fields
        .iter()
        .map(|f| (f.clone(), json!(field_type(f))))
        .collect();
    let units: Map<String, Value> = fields
        .iter()
        .map(|f| {
            (
                f.clone(),
                if field_type(f) == "duration" {
                    json!("millisecond")
                } else {
                    Value::Null
                },
            )
        })
        .collect();
    let page = page(
        result.into_iter().map(Value::Object).collect(),
        &q,
        &uri,
        &headers,
        50,
        100,
    );
    let body = json!({
        "data": page.items,
        "meta": {
            "fields": meta_fields,
            "units": units,
            "isMetricsData": false,
            "isMetricsExtractedData": false,
            "tips": { "query": null, "columns": null },
            "datasetReason": "unchanged",
            "dataset": q.get("dataset").unwrap_or("discover"),
        }
    });
    with_link(ok(body), &page.link)
}

pub async fn validate(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let dataset_name = q.get("dataset").unwrap_or("discover").to_owned();
    let dataset_ok = dataset_of(Some(&dataset_name));
    let fields: Vec<String> = q.all("field").into_iter().map(str::to_owned).collect();
    let field_results: Vec<Value> = fields
        .iter()
        .map(|f| {
            let err = parse_function(f)
                .filter(|(name, _)| !FUNCTIONS.contains(name))
                .map(|(name, _)| format!("{name} is not a valid function"));
            let ty = match field_type(f) {
                "integer" | "number" | "duration" => "number",
                "boolean" => "boolean",
                _ => "string",
            };
            json!({ "name": f, "valid": err.is_none(), "attrType": ty, "error": err })
        })
        .collect();
    let orderby: Vec<Value> = q
        .all("orderby")
        .into_iter()
        .map(|o| {
            let key = o.trim_start_matches('-');
            let valid = fields.iter().any(|f| f == key);
            json!({
                "name": o,
                "valid": valid,
                "attrType": null,
                "error": if valid { Value::Null } else { json!("orderby must also be in the selected columns or groupby") },
            })
        })
        .collect();
    let query_fields: Vec<Value> = search::parse(q.get("query").unwrap_or(""))
        .into_iter()
        .filter_map(|t| match t {
            Term::Filter { key, .. } | Term::Has { key, .. } => {
                Some(json!({ "name": key, "valid": true, "attrType": "string", "error": null }))
            }
            Term::Text { .. } => None,
        })
        .collect();
    let projects: Vec<Value> = q
        .all("project")
        .into_iter()
        .filter(|p| *p != "-1")
        .map(|p| {
            let found = find_project(&world, p).is_some();
            json!({ "valid": found, "error": if found { Value::Null } else { json!(format!("Project {p} does not exist")) } })
        })
        .collect();
    let envs = crate::org::environment_names(&world, &[]);
    let environment: Vec<Value> = q
        .all("environment")
        .into_iter()
        .map(|e| {
            let found = envs.iter().any(|x| x == e);
            json!({ "valid": found, "error": if found { Value::Null } else { json!(format!("Environment {e} does not exist")) } })
        })
        .collect();
    let all_valid = dataset_ok.is_ok()
        && field_results.iter().all(|f| f["valid"] == true)
        && orderby.iter().all(|o| o["valid"] == true)
        && projects.iter().all(|p| p["valid"] == true)
        && environment.iter().all(|e| e["valid"] == true);
    let body = json!({
        "valid": all_valid,
        "projects": projects,
        "dataset": [{ "name": dataset_name, "valid": dataset_ok.is_ok(), "error": dataset_ok.err() }],
        "environment": environment,
        "field": field_results,
        "query": { "valid": true, "error": null, "fields": query_fields },
        "orderby": orderby,
    });
    if all_valid {
        ok(body)
    } else {
        mock_core::http::json(axum::http::StatusCode::BAD_REQUEST, body)
    }
}

fn interval_for(period: i64) -> i64 {
    match period {
        p if p <= 86_400 => 3_600,
        p if p <= 7 * 86_400 => 3 * 3_600,
        p if p <= 30 * 86_400 => 12 * 3_600,
        _ => 86_400,
    }
}

pub async fn events_stats(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let dataset = match dataset_of(q.get("dataset")) {
        Ok(d) => d,
        Err(e) => return bad_request(&e),
    };
    let y_axis = q.get("yAxis").unwrap_or("count()").to_owned();
    if let Err(e) = validate_fields(std::slice::from_ref(&y_axis)) {
        return bad_request(&e);
    }
    let now = now(&world);
    let (rows, period) = match dataset_rows(&world, dataset, &q, now) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (from, to) = match time_window(&q, now, "14d") {
        Ok(w) => w,
        Err(r) => return r,
    };
    let terms = search::parse(q.get("query").unwrap_or(""));
    let rows: Vec<Map<String, Value>> = rows
        .into_iter()
        .filter(|r| row_matches(r, &terms, now))
        .collect();
    let interval = q
        .non_empty("interval")
        .and_then(crate::util::parse_period)
        .unwrap_or_else(|| interval_for(period))
        .max(60);
    let start = from - from.rem_euclid(interval);
    let mut data = Vec::new();
    let mut t = start;
    while t <= to {
        let bucket: Vec<&Map<String, Value>> = rows
            .iter()
            .filter(|r| {
                let ts = r.get("timestamp").and_then(value_time).unwrap_or(0);
                ts >= t && ts < t + interval
            })
            .collect();
        let value = match aggregate(&y_axis, &bucket, interval, now) {
            Ok(v) => v,
            Err(e) => return bad_request(&e),
        };
        let count = match value {
            Value::Number(n) => json!(n),
            _ => json!(0),
        };
        data.push(json!([t, [{ "count": count }]]));
        t += interval;
    }
    ok(json!({
        "data": data,
        "start": start,
        "end": to,
        "isMetricsData": false,
        "meta": { "fields": { (y_axis.clone()): field_type(&y_axis) }, "units": { (y_axis): null }, "isMetricsData": false, "tips": {}, "dataset": q.get("dataset").unwrap_or("discover") },
    }))
}

pub async fn trace_item_attributes(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    RawQuery(raw): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let q = Query::parse(raw.as_deref());
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let collection = match q.get("itemType").unwrap_or("spans") {
        "spans" => "spans",
        "logs" => "logs",
        _ => "",
    };
    let substring = q.get("substringMatch").map(str::to_lowercase);
    let wanted_type = q.get("attributeType");
    let mut attrs: BTreeMap<String, &'static str> = BTreeMap::new();
    for doc in docs(&world, collection) {
        for (k, v) in item_row(doc) {
            if k.starts_with("tags[") || k == "tags" || k == "attributes" {
                continue;
            }
            let ty = match v {
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                _ => "string",
            };
            attrs.entry(k).or_insert(ty);
        }
    }
    let out: Vec<Value> = attrs
        .into_iter()
        .filter(|(k, _)| {
            substring
                .as_ref()
                .is_none_or(|s| k.to_lowercase().contains(s))
        })
        .filter(|(_, ty)| wanted_type.is_none_or(|w| w == *ty))
        .map(|(k, ty)| {
            let source = if k.starts_with("sentry.")
                || matches!(
                    k.as_str(),
                    "span.op"
                        | "span.description"
                        | "span.duration"
                        | "transaction"
                        | "trace"
                        | "timestamp"
                        | "project"
                        | "id"
                        | "severity"
                        | "message"
                ) {
                "sentry"
            } else {
                "user"
            };
            json!({
                "key": k,
                "name": k,
                "attributeType": ty,
                "attributeSource": { "source_type": source },
            })
        })
        .collect();
    paginate(out, &q, &uri, &headers, 1000, 1000)
}

fn trace_spans<'a>(world: &'a WorldState, trace: &str) -> Vec<&'a Value> {
    docs(world, "spans")
        .into_iter()
        .filter(|s| {
            s["trace"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(trace))
        })
        .collect()
}

fn trace_errors<'a>(world: &'a WorldState, trace: &str) -> Vec<&'a Value> {
    docs(world, "events")
        .into_iter()
        .filter(|e| {
            e["trace"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(trace))
        })
        .collect()
}

pub async fn trace_meta(
    State(world): State<SharedWorld>,
    Path((org, trace)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let spans = trace_spans(&world, &trace);
    let mut by_op: BTreeMap<String, u64> = BTreeMap::new();
    for s in &spans {
        *by_op
            .entry(s["span.op"].as_str().unwrap_or("default").to_owned())
            .or_default() += 1;
    }
    let tx_children: Vec<Value> = spans
        .iter()
        .filter(|s| s["is_transaction"] == json!(true))
        .map(|tx| {
            let id = tx["span_id"].clone();
            let n = spans.iter().filter(|s| s["parent_span_id"] == id).count();
            json!({ "transaction.event_id": tx.get("event_id").cloned().unwrap_or(Value::Null), "count()": n })
        })
        .collect();
    let logs = docs(&world, "logs")
        .into_iter()
        .filter(|l| {
            l["trace"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(&trace))
        })
        .count();
    ok(json!({
        "logs": logs,
        "errors": trace_errors(&world, &trace).len(),
        "performance_issues": 0,
        "span_count": spans.len(),
        "transaction_child_count_map": tx_children,
        "span_count_map": by_op,
    }))
}

fn span_node(world: &WorldState, s: &Value, all: &[&Value], errors: &[&Value]) -> Value {
    let id = s["span_id"].clone();
    let children: Vec<Value> = all
        .iter()
        .filter(|c| c["parent_span_id"] == id)
        .map(|c| span_node(world, c, all, errors))
        .collect();
    let start = value_time(&s["timestamp"]).unwrap_or(0) as f64;
    let duration = s["span.duration"].as_f64().unwrap_or(0.0);
    let start = s["start_timestamp"]
        .as_f64()
        .unwrap_or(start - duration / 1000.0);
    let project = s["project"].as_str().unwrap_or("");
    let errs: Vec<Value> = errors
        .iter()
        .filter(|e| e["spanId"] == id)
        .map(|e| {
            json!({
                "event_id": e["eventID"].clone(),
                "issue_id": str_of(&e["groupID"]).parse::<u64>().unwrap_or(0),
                "project_slug": e["project"].clone(),
                "title": e["title"].clone(),
                "level": e.get("level").cloned().unwrap_or(json!("error")),
                "timestamp": value_time(&e["dateCreated"]).unwrap_or(0),
            })
        })
        .collect();
    json!({
        "children": children,
        "errors": errs,
        "occurrences": [],
        "event_id": s.get("event_id").cloned().unwrap_or_else(|| id.clone()),
        "span_id": id,
        "transaction_id": s.get("event_id").cloned().unwrap_or_else(|| s["span_id"].clone()),
        "project_id": find_project(world, project).map_or(json!(0), |p| json!(str_of(&p["id"]).parse::<u64>().unwrap_or(0))),
        "project_slug": project,
        "profile_id": null,
        "profiler_id": null,
        "parent_span_id": s.get("parent_span_id").cloned().unwrap_or(Value::Null),
        "start_timestamp": start,
        "end_timestamp": start + duration / 1000.0,
        "measurements": {},
        "duration": duration,
        "transaction": s.get("transaction").cloned().unwrap_or(Value::Null),
        "is_transaction": s.get("is_transaction").cloned().unwrap_or(json!(false)),
        "description": s.get("span.description").cloned().unwrap_or(Value::Null),
        "sdk_name": s.get("sdk.name").cloned().unwrap_or(Value::Null),
        "op": s.get("span.op").cloned().unwrap_or(Value::Null),
        "name": s.get("span.name").cloned().unwrap_or(Value::Null),
        "event_type": if s["is_transaction"] == json!(true) { "transaction" } else { "span" },
        "additional_attributes": s.get("attributes").cloned().unwrap_or(json!({})),
        "status": s.get("span.status").cloned().unwrap_or(Value::Null),
    })
}

pub async fn trace(
    State(world): State<SharedWorld>,
    Path((org, trace)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    if !org_exists(&world, &org) {
        return not_found();
    }
    let spans = trace_spans(&world, &trace);
    let errors = trace_errors(&world, &trace);
    let ids: Vec<Value> = spans.iter().map(|s| s["span_id"].clone()).collect();
    let mut roots: Vec<Value> = spans
        .iter()
        .filter(|s| s["parent_span_id"].is_null() || !ids.contains(&s["parent_span_id"]))
        .map(|s| span_node(&world, s, &spans, &errors))
        .collect();
    for e in errors.iter().filter(|e| !ids.contains(&e["spanId"])) {
        roots.push(json!({
            "id": e["eventID"].clone(),
            "issue_id": str_of(&e["groupID"]).parse::<u64>().unwrap_or(0),
            "project_id": find_project(&world, e["project"].as_str().unwrap_or("")).map_or(json!(0), |p| json!(str_of(&p["id"]).parse::<u64>().unwrap_or(0))),
            "project_slug": e["project"].clone(),
            "title": e["title"].clone(),
            "culprit": e.get("culprit").cloned().unwrap_or(json!("")),
            "type": "error",
            "timestamp": value_time(&e["dateCreated"]).unwrap_or(0),
        }));
    }
    ok(Value::Array(roots))
}
