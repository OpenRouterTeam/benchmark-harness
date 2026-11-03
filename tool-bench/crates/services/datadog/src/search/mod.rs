//! v2 event search: logs (`POST /api/v2/logs/events/search`), spans
//! (`POST /api/v2/spans/events/search`), RUM (`GET /api/v2/rum/events`), plus RUM applications.

pub mod query;

use axum::{
    body::Bytes,
    extract::{RawQuery, State},
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use self::query::{Query, Searchable};
use crate::{
    BASE_URL, NAME,
    support::{OK, decode_cursor, docs, encode_cursor, errors, parse_query, path},
    timeparse::{bound_ms, rfc3339_ms},
};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 1000;

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Logs,
    Spans,
    Rum,
}

impl Family {
    fn collection(self) -> &'static str {
        match self {
            Family::Logs => "logs",
            Family::Spans => "spans",
            Family::Rum => "rum_events",
        }
    }

    fn timestamp_field(self) -> &'static str {
        match self {
            Family::Spans => "start_timestamp",
            Family::Logs | Family::Rum => "timestamp",
        }
    }

    fn request_prefix(self) -> &'static str {
        match self {
            Family::Logs => "pddv1ChZ",
            Family::Spans => "spdv1ChZ",
            Family::Rum => "rddv1ChZ",
        }
    }
}

struct Event<'a> {
    doc: &'a Value,
    family: Family,
}

fn push_flat(out: &mut Vec<Value>, v: &Value) {
    match v {
        Value::Array(items) => out.extend(items.iter().cloned()),
        Value::Null => {}
        other => out.push(other.clone()),
    }
}

impl Searchable for Event<'_> {
    fn field_values(&self, key: &str) -> Vec<Value> {
        let attrs = &self.doc["attributes"];
        let mut out = Vec::new();
        if let Some(attr) = key.strip_prefix('@') {
            for bag in ["attributes", "custom"] {
                if let Some(v) = path(&attrs[bag], attr) {
                    push_flat(&mut out, v);
                }
            }
            return out;
        }
        let key = match (self.family, key) {
            (Family::Spans, "operation" | "operation_name") => "operation_name",
            (Family::Spans, "resource") => "resource_name",
            (_, other) => other,
        };
        if let Some(v) = attrs.get(key).filter(|v| !v.is_object()) {
            push_flat(&mut out, v);
        } else if self.family == Family::Spans
            && let Some(v) = path(&attrs["custom"], key).or_else(|| path(&attrs["attributes"], key))
        {
            push_flat(&mut out, v);
        }
        if key == "id" {
            out.push(self.doc["id"].clone());
        }
        let prefix = format!("{key}:");
        for tag in attrs["tags"].as_array().into_iter().flatten() {
            if let Some(value) = tag.as_str().and_then(|t| t.strip_prefix(&prefix)) {
                out.push(Value::String(value.to_owned()));
            }
        }
        out
    }

    fn text(&self) -> String {
        let attrs = &self.doc["attributes"];
        let parts: Vec<&str> = match self.family {
            Family::Logs => vec![attrs["message"].as_str().unwrap_or_default()],
            Family::Spans => vec![
                attrs["resource_name"].as_str().unwrap_or_default(),
                attrs["custom"]["error"]["message"]
                    .as_str()
                    .unwrap_or_default(),
            ],
            Family::Rum => {
                let a = &attrs["attributes"];
                vec![
                    a["error"]["message"].as_str().unwrap_or_default(),
                    a["view"]["name"].as_str().unwrap_or_default(),
                    a["view"]["url"].as_str().unwrap_or_default(),
                    a["action"]["target"]["name"].as_str().unwrap_or_default(),
                ]
            }
        };
        parts.join(" ")
    }
}

struct SearchRequest {
    query: String,
    from: Option<String>,
    to: Option<String>,
    descending: bool,
    limit: usize,
    cursor: Option<String>,
}

fn run(world: &SharedWorld, family: Family, req: SearchRequest) -> Response {
    let mut guard = world.lock().expect("world lock");
    let now_ms = guard.clock.now_unix() * 1000;
    let parsed: Query = match query::parse(&req.query) {
        Ok(q) => q,
        Err(e) => return errors::bad_request(&format!("Invalid query: {e}")),
    };
    let from = match req.from.as_deref().map(|f| bound_ms(f, now_ms)) {
        None => now_ms - 15 * 60 * 1000,
        Some(Some(ms)) => ms,
        Some(None) => return errors::bad_request("Invalid 'from' time"),
    };
    let to = match req.to.as_deref().map(|t| bound_ms(t, now_ms)) {
        None => now_ms,
        Some(Some(ms)) => ms,
        Some(None) => return errors::bad_request("Invalid 'to' time"),
    };
    let offset = match req.cursor.as_deref().filter(|c| !c.is_empty()) {
        None => 0,
        Some(c) => match decode_cursor(c) {
            Some(o) => o,
            None => return errors::bad_request("Invalid cursor"),
        },
    };

    let ts_field = family.timestamp_field();
    let mut matched: Vec<(i64, Value)> = docs(&guard, family.collection())
        .into_iter()
        .filter_map(|doc| {
            let ts = doc["attributes"][ts_field].as_str().and_then(rfc3339_ms)?;
            (ts >= from && ts < to && parsed.matches(&Event { doc: &doc, family }))
                .then_some((ts, doc))
        })
        .collect();
    if req.descending {
        matched.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    } else {
        matched.sort_by_key(|(ts, _)| *ts);
    }
    let total = matched.len();
    let page: Vec<Value> = matched
        .into_iter()
        .skip(offset)
        .take(req.limit)
        .map(|(_, d)| d)
        .collect();
    let next = offset + page.len();

    let request_id =
        guard
            .service_mut(NAME)
            .ids
            .next_prefixed("request_id", family.request_prefix(), 40);
    let mut meta = json_value!({
        "elapsed": 37 + page.len() as u64,
        "request_id": request_id,
        "status": "done",
    });
    let mut body = json_value!({ "data": page });
    if next < total {
        let cursor = encode_cursor(next);
        meta["page"] = json_value!({ "after": cursor });
        let next_url = match family {
            Family::Logs => format!("{BASE_URL}/api/v2/logs/events?page[cursor]={cursor}"),
            Family::Spans => format!("{BASE_URL}/api/v2/spans/events?page[cursor]={cursor}"),
            Family::Rum => format!("{BASE_URL}/api/v2/rum/events?page[cursor]={cursor}"),
        };
        body["links"] = json_value!({ "next": next_url });
    }
    body["meta"] = meta;
    json(OK, body)
}

fn limit_from(v: Option<&Value>) -> Result<usize, Response> {
    match v {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(v) => v
            .as_f64()
            .filter(|n| *n >= 0.0 && n.fract() == 0.0)
            .map(|n| (n as usize).clamp(1, MAX_LIMIT))
            .ok_or_else(|| errors::bad_request("Invalid page limit")),
    }
}

fn string_or_number(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_body(body: &Bytes) -> Result<Value, Response> {
    if body.is_empty() {
        return Ok(json_value!({}));
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| errors::bad_request("Invalid JSON body"))
}

fn sort_descending(sort: Option<&Value>) -> Result<bool, Response> {
    match sort.and_then(Value::as_str) {
        None | Some("timestamp") => Ok(false),
        Some("-timestamp") => Ok(true),
        Some(other) => Err(errors::bad_request(&format!(
            "Invalid sort value '{other}', expected one of timestamp, -timestamp"
        ))),
    }
}

/// `POST /api/v2/logs/events/search` (`LogsListRequest`).
pub async fn logs_search(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let filter = &body["filter"];
    let req = match (
        limit_from(body["page"].get("limit")),
        sort_descending(body.get("sort")),
    ) {
        (Ok(limit), Ok(descending)) => SearchRequest {
            query: filter["query"].as_str().unwrap_or("*").to_owned(),
            from: filter.get("from").and_then(string_or_number),
            to: filter.get("to").and_then(string_or_number),
            descending,
            limit,
            cursor: body["page"]["cursor"].as_str().map(str::to_owned),
        },
        (Err(r), _) | (_, Err(r)) => return r,
    };
    run(&world, Family::Logs, req)
}

/// `POST /api/v2/spans/events/search` (`SpansListRequest`).
pub async fn spans_search(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let body = match parse_body(&body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let data = &body["data"];
    if !data.is_object() {
        return errors::bad_request("Missing required field 'data'");
    }
    if let Some(t) = data.get("type").and_then(Value::as_str)
        && t != "search_request"
    {
        return errors::bad_request(&format!("Invalid type '{t}', expected 'search_request'"));
    }
    let attrs = &data["attributes"];
    let filter = &attrs["filter"];
    let req = match (
        limit_from(attrs["page"].get("limit")),
        sort_descending(attrs.get("sort")),
    ) {
        (Ok(limit), Ok(descending)) => SearchRequest {
            query: filter["query"].as_str().unwrap_or("*").to_owned(),
            from: filter.get("from").and_then(string_or_number),
            to: filter.get("to").and_then(string_or_number),
            descending,
            limit,
            cursor: attrs["page"]["cursor"].as_str().map(str::to_owned),
        },
        (Err(r), _) | (_, Err(r)) => return r,
    };
    run(&world, Family::Spans, req)
}

/// `GET /api/v2/rum/events`.
pub async fn rum_list(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let limit = match params.get("page[limit]") {
        None => DEFAULT_LIMIT,
        Some(v) => match v.parse::<usize>() {
            Ok(n) => n.clamp(1, MAX_LIMIT),
            Err(_) => return errors::bad_request("Invalid value for parameter 'page[limit]'"),
        },
    };
    let descending = match sort_descending(
        params
            .get("sort")
            .map(|s| Value::String(s.clone()))
            .as_ref(),
    ) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let req = SearchRequest {
        query: params
            .get("filter[query]")
            .cloned()
            .unwrap_or_else(|| "*".to_owned()),
        from: params.get("filter[from]").cloned(),
        to: params.get("filter[to]").cloned(),
        descending,
        limit,
        cursor: params.get("page[cursor]").cloned(),
    };
    run(&world, Family::Rum, req)
}

/// `GET /api/v2/rum/applications`.
pub async fn rum_applications(State(world): State<SharedWorld>) -> Response {
    let world = world.lock().expect("world lock");
    json(
        OK,
        json_value!({ "data": docs(&world, "rum_applications") }),
    )
}
