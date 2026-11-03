//! CloudWatch Logs (JSON 1.1, `X-Amz-Target: Logs_20140328.<Op>`). Times are epoch ms.
//!
//! - `log_groups` keyed by name: `logGroupName`, `creationTime`, `retentionInDays`,
//!   `storedBytes`, `kmsKeyId`, `logGroupClass`, `tags` (`{k: v}`).
//! - `log_streams` keyed by `"<group>|<stream>"`: `logGroupName`, `logStreamName`,
//!   `creationTime`, `events` (`[{timestamp, message}]`).
//! - `query_definitions` keyed by id; `anomaly_detectors` keyed by ARN; `anomalies`;
//!   `metric_filters` keyed by `"<group>|<name>"`; `queries` (Insights runs, by queryId).

use axum::{http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::{
    Ctx, Req,
    insights::{self, Event},
    util::{json_response, s, value_time},
};

const CT: &str = "application/x-amz-json-1.1";

type Out = Result<Value, (StatusCode, &'static str, String)>;

fn not_found(msg: &str) -> (StatusCode, &'static str, String) {
    (
        StatusCode::BAD_REQUEST,
        "ResourceNotFoundException",
        msg.to_owned(),
    )
}

fn invalid(msg: impl Into<String>) -> (StatusCode, &'static str, String) {
    (
        StatusCode::BAD_REQUEST,
        "InvalidParameterException",
        msg.into(),
    )
}

const NO_GROUP: &str = "The specified log group does not exist.";
const NO_STREAM: &str = "The specified log stream does not exist.";

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let op = req.target_op().unwrap_or_default();
    let input = req.body_json();
    let result = match op.as_str() {
        "DescribeLogGroups" => describe_log_groups(ctx, &input),
        "CreateLogGroup" => create_log_group(ctx, &input),
        "DeleteLogGroup" => delete_log_group(ctx, &input),
        "PutRetentionPolicy" => put_retention(ctx, &input),
        "DeleteRetentionPolicy" => delete_retention(ctx, &input),
        "DescribeLogStreams" => describe_log_streams(ctx, &input),
        "CreateLogStream" => create_log_stream(ctx, &input),
        "DeleteLogStream" => delete_log_stream(ctx, &input),
        "PutLogEvents" => put_log_events(ctx, &input),
        "GetLogEvents" => get_log_events(ctx, &input),
        "FilterLogEvents" => filter_log_events(ctx, &input),
        "StartQuery" => start_query(ctx, &input),
        "GetQueryResults" => get_query_results(ctx, &input),
        "StopQuery" => stop_query(ctx, &input),
        "DescribeQueries" => describe_queries(ctx, &input),
        "DescribeQueryDefinitions" => describe_query_definitions(ctx, &input),
        "PutQueryDefinition" => put_query_definition(ctx, &input),
        "DeleteQueryDefinition" => delete_query_definition(ctx, &input),
        "ListLogAnomalyDetectors" => list_anomaly_detectors(ctx, &input),
        "ListAnomalies" => list_anomalies(ctx, &input),
        "DescribeIndexPolicies" => describe_index_policies(ctx, &input),
        "DescribeFieldIndexes" => Ok(json!({"fieldIndexes": []})),
        "DescribeAccountPolicies" => Ok(
            json!({"accountPolicies": ctx.docs("account_policies").into_iter().map(|(_, v)| v).collect::<Vec<_>>()}),
        ),
        "DescribeMetricFilters" => describe_metric_filters(ctx, &input),
        "PutMetricFilter" => put_metric_filter(ctx, &input),
        "DescribeSubscriptionFilters" => {
            group_of(ctx, &input).map(|_| json!({"subscriptionFilters": []}))
        }
        "ListTagsLogGroup" => group_of(ctx, &input)
            .map(|g| json!({"tags": g.get("tags").cloned().unwrap_or(json!({}))})),
        "ListTagsForResource" => list_tags_for_resource(ctx, &input),
        "TagLogGroup" => tag_group(ctx, &input, "tags"),
        "TagResource" => tag_resource(ctx, &input),
        "UntagLogGroup" | "UntagResource" => untag(ctx, &input),
        "GetLogGroupFields" => get_log_group_fields(ctx, &input),
        "DescribeDestinations" => Ok(json!({"destinations": []})),
        "DescribeExportTasks" => Ok(json!({"exportTasks": []})),
        "DescribeResourcePolicies" => Ok(json!({"resourcePolicies": []})),
        "DescribeDeliveries" => Ok(json!({"deliveries": []})),
        "ListLogGroupsForQuery" => Ok(json!({"logGroupIdentifiers": []})),
        "" => Err((
            StatusCode::BAD_REQUEST,
            "UnknownOperationException",
            String::new(),
        )),
        _ => Ok(json!({})),
    };
    match result {
        Ok(v) => json_response(StatusCode::OK, CT, &v),
        Err((status, code, msg)) => {
            json_response(status, CT, &json!({"__type": code, "message": msg}))
        }
    }
}

fn ms(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(|x| if x < 100_000_000_000 { x * 1000 } else { x }),
        other => value_time(other).map(|t| t * 1000),
    }
}

fn group_arn(ctx: &Ctx, name: &str) -> String {
    format!(
        "arn:aws:logs:{}:{}:log-group:{name}",
        ctx.region, ctx.account
    )
}

/// Accepts `logGroupName` or `logGroupIdentifier` (name or ARN).
fn group_name(input: &Value) -> String {
    let ident = s(input, "logGroupIdentifier");
    if !ident.is_empty() {
        return name_from_ident(&ident);
    }
    s(input, "logGroupName")
}

fn name_from_ident(ident: &str) -> String {
    match ident.split_once(":log-group:") {
        Some((_, rest)) => rest.trim_end_matches(":*").to_owned(),
        None => ident.to_owned(),
    }
}

fn group_of(ctx: &Ctx, input: &Value) -> Result<Value, (StatusCode, &'static str, String)> {
    let name = group_name(input);
    ctx.get("log_groups", &name)
        .filter(|g| ctx.in_region(g))
        .ok_or_else(|| not_found(NO_GROUP))
}

fn streams_of(ctx: &Ctx, group: &str) -> Vec<Value> {
    ctx.docs("log_streams")
        .into_iter()
        .map(|(_, v)| v)
        .filter(|st| s(st, "logGroupName") == group)
        .collect()
}

fn events_of(stream: &Value) -> Vec<(i64, String, i64)> {
    let mut evs: Vec<(i64, String, i64)> = stream
        .get("events")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let t = ms(e.get("timestamp")?)?;
                    let ing = e.get("ingestionTime").and_then(ms).unwrap_or(t + 312);
                    Some((t, s(e, "message"), ing))
                })
                .collect()
        })
        .unwrap_or_default();
    evs.sort_by_key(|e| e.0);
    evs
}

fn stored_bytes(ctx: &Ctx, g: &Value) -> i64 {
    g.get("storedBytes")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            streams_of(ctx, &s(g, "logGroupName"))
                .iter()
                .flat_map(events_of)
                .map(|e| e.1.len() as i64 + 26)
                .sum()
        })
}

fn group_out(ctx: &Ctx, g: &Value) -> Value {
    let name = s(g, "logGroupName");
    let mut out = Map::new();
    out.insert("logGroupName".into(), json!(name));
    out.insert(
        "creationTime".into(),
        json!(g.get("creationTime").and_then(ms).unwrap_or(0)),
    );
    if let Some(r) = g.get("retentionInDays").filter(|v| !v.is_null()) {
        out.insert("retentionInDays".into(), r.clone());
    }
    out.insert(
        "metricFilterCount".into(),
        json!(
            ctx.docs("metric_filters")
                .iter()
                .filter(|(_, f)| s(f, "logGroupName") == name)
                .count()
        ),
    );
    out.insert("arn".into(), json!(format!("{}:*", group_arn(ctx, &name))));
    out.insert("storedBytes".into(), json!(stored_bytes(ctx, g)));
    for k in ["kmsKeyId", "dataProtectionStatus", "inheritedProperties"] {
        if let Some(v) = g.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), v.clone());
        }
    }
    out.insert(
        "logGroupClass".into(),
        g.get("logGroupClass").cloned().unwrap_or(json!("STANDARD")),
    );
    out.insert("logGroupArn".into(), json!(group_arn(ctx, &name)));
    Value::Object(out)
}

fn page<T: Clone>(
    items: &[T],
    input: &Value,
    default: usize,
    max: usize,
) -> (Vec<T>, Option<String>) {
    let limit = input
        .get("limit")
        .or_else(|| input.get("maxResults"))
        .and_then(Value::as_u64)
        .map(|l| l as usize)
        .unwrap_or(default)
        .clamp(1, max);
    let start = input
        .get("nextToken")
        .and_then(Value::as_str)
        .and_then(|t| t.trim_start_matches("tok-").parse::<usize>().ok())
        .unwrap_or(0);
    let slice: Vec<T> = items.iter().skip(start).take(limit).cloned().collect();
    let next = (start + limit < items.len()).then(|| format!("tok-{}", start + limit));
    (slice, next)
}

fn with_next(mut v: Value, next: Option<String>) -> Value {
    if let Some(n) = next {
        v["nextToken"] = json!(n);
    }
    v
}

fn describe_log_groups(ctx: &mut Ctx, input: &Value) -> Out {
    let prefix = s(input, "logGroupNamePrefix");
    let pattern = s(input, "logGroupNamePattern").to_lowercase();
    let class = s(input, "logGroupClass");
    let idents: Vec<String> = input
        .get("logGroupIdentifiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(name_from_ident)
                .collect()
        })
        .unwrap_or_default();
    let mut groups: Vec<Value> = ctx
        .docs("log_groups")
        .into_iter()
        .map(|(_, g)| g)
        .filter(|g| ctx.in_region(g))
        .filter(|g| s(g, "logGroupName").starts_with(&prefix))
        .filter(|g| pattern.is_empty() || s(g, "logGroupName").to_lowercase().contains(&pattern))
        .filter(|g| {
            class.is_empty()
                || g.get("logGroupClass")
                    .and_then(Value::as_str)
                    .unwrap_or("STANDARD")
                    == class
        })
        .filter(|g| idents.is_empty() || idents.contains(&s(g, "logGroupName")))
        .collect();
    groups.sort_by_key(|g| s(g, "logGroupName"));
    let (slice, next) = page(&groups, input, 50, 50);
    let out: Vec<Value> = slice.iter().map(|g| group_out(ctx, g)).collect();
    Ok(with_next(json!({"logGroups": out}), next))
}

const RETENTION: [i64; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

fn create_log_group(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "logGroupName");
    if name.is_empty() || name.len() > 512 {
        return Err(invalid(
            "1 validation error detected: Value at 'logGroupName' failed to satisfy constraint: Member must have length greater than or equal to 1",
        ));
    }
    if ctx.get("log_groups", &name).is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            "ResourceAlreadyExistsException",
            "The specified log group already exists".into(),
        ));
    }
    let mut g = json!({
        "logGroupName": name,
        "creationTime": ctx.now * 1000,
        "logGroupClass": input.get("logGroupClass").cloned().unwrap_or(json!("STANDARD")),
        "tags": input.get("tags").cloned().unwrap_or(json!({})),
        "_region": ctx.region,
    });
    if let Some(k) = input.get("kmsKeyId") {
        g["kmsKeyId"] = k.clone();
    }
    ctx.put("log_groups", &name, g);
    Ok(json!({}))
}

fn delete_log_group(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let name = s(&g, "logGroupName");
    ctx.remove("log_groups", &name);
    let streams: Vec<String> = ctx
        .docs("log_streams")
        .into_iter()
        .filter(|(_, st)| s(st, "logGroupName") == name)
        .map(|(k, _)| k)
        .collect();
    for k in streams {
        ctx.remove("log_streams", &k);
    }
    Ok(json!({}))
}

fn put_retention(ctx: &mut Ctx, input: &Value) -> Out {
    let days = input
        .get("retentionInDays")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if !RETENTION.contains(&days) {
        return Err(invalid(format!(
            "1 validation error detected: Value '{days}' at 'retentionInDays' failed to satisfy constraint: Member must satisfy enum value set: [{}]",
            RETENTION
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let g = group_of(ctx, input)?;
    let name = s(&g, "logGroupName");
    ctx.get_mut("log_groups", &name).expect("exists")["retentionInDays"] = json!(days);
    Ok(json!({}))
}

fn delete_retention(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let name = s(&g, "logGroupName");
    if let Some(o) = ctx
        .get_mut("log_groups", &name)
        .and_then(Value::as_object_mut)
    {
        o.shift_remove("retentionInDays");
    }
    Ok(json!({}))
}

fn stream_out(ctx: &Ctx, st: &Value) -> Value {
    let evs = events_of(st);
    let mut out = json!({
        "logStreamName": s(st, "logStreamName"),
        "creationTime": st.get("creationTime").and_then(ms).or_else(|| evs.first().map(|e| e.0)).unwrap_or(0),
        "arn": format!("{}:log-stream:{}", group_arn(ctx, &s(st, "logGroupName")), s(st, "logStreamName")),
        "storedBytes": 0,
    });
    if let (Some(first), Some(last)) = (evs.first(), evs.last()) {
        out["firstEventTimestamp"] = json!(first.0);
        out["lastEventTimestamp"] = json!(last.0);
        out["lastIngestionTime"] = json!(last.2);
        out["uploadSequenceToken"] = json!(format!("4963{:0>52}", evs.len()));
    }
    out
}

fn describe_log_streams(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let prefix = s(input, "logStreamNamePrefix");
    let order_by_time = s(input, "orderBy") == "LastEventTime";
    if order_by_time && !prefix.is_empty() {
        return Err(invalid(
            "Cannot order by LastEventTime with a logStreamNamePrefix.",
        ));
    }
    let mut streams: Vec<Value> = streams_of(ctx, &s(&g, "logGroupName"))
        .into_iter()
        .filter(|st| s(st, "logStreamName").starts_with(&prefix))
        .map(|st| stream_out(ctx, &st))
        .collect();
    if order_by_time {
        streams.sort_by_key(|st| {
            st.get("lastEventTimestamp")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        });
    } else {
        streams.sort_by_key(|st| s(st, "logStreamName"));
    }
    if input
        .get("descending")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        streams.reverse();
    }
    let (slice, next) = page(&streams, input, 50, 50);
    Ok(with_next(json!({"logStreams": slice}), next))
}

fn create_log_stream(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let group = s(&g, "logGroupName");
    let stream = s(input, "logStreamName");
    let key = format!("{group}|{stream}");
    if ctx.get("log_streams", &key).is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            "ResourceAlreadyExistsException",
            "The specified log stream already exists".into(),
        ));
    }
    ctx.put(
        "log_streams",
        &key,
        json!({"logGroupName": group, "logStreamName": stream, "creationTime": ctx.now * 1000, "events": []}),
    );
    Ok(json!({}))
}

fn delete_log_stream(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let key = format!("{}|{}", s(&g, "logGroupName"), s(input, "logStreamName"));
    ctx.remove("log_streams", &key)
        .ok_or_else(|| not_found(NO_STREAM))?;
    Ok(json!({}))
}

fn put_log_events(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let key = format!("{}|{}", s(&g, "logGroupName"), s(input, "logStreamName"));
    let now_ms = ctx.now * 1000;
    let st = ctx
        .get_mut("log_streams", &key)
        .ok_or_else(|| not_found(NO_STREAM))?;
    let new: Vec<Value> = input
        .get("logEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| json!({"timestamp": e["timestamp"], "message": e["message"], "ingestionTime": now_ms}))
        .collect();
    match st.get_mut("events").and_then(Value::as_array_mut) {
        Some(list) => list.extend(new),
        None => st["events"] = Value::Array(new),
    }
    Ok(json!({"nextSequenceToken": format!("4963{:0>52}", now_ms)}))
}

fn get_log_events(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let key = format!("{}|{}", s(&g, "logGroupName"), s(input, "logStreamName"));
    let st = ctx
        .get("log_streams", &key)
        .ok_or_else(|| not_found(NO_STREAM))?;
    let start = input.get("startTime").and_then(Value::as_i64);
    let end = input.get("endTime").and_then(Value::as_i64);
    let evs: Vec<(i64, String, i64)> = events_of(&st)
        .into_iter()
        .filter(|e| start.is_none_or(|s| e.0 >= s) && end.is_none_or(|x| e.0 < x))
        .collect();
    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(10_000) as usize;
    let from_head = input
        .get("startFromHead")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let token = s(input, "nextToken");
    let (lo, hi) = if let Some(f) = token.strip_prefix("f/") {
        let lo: usize = f.parse().unwrap_or(0);
        (lo, (lo + limit).min(evs.len()))
    } else if let Some(b) = token.strip_prefix("b/") {
        let hi: usize = b.parse().unwrap_or(0);
        (hi.saturating_sub(limit), hi)
    } else if from_head {
        (0, limit.min(evs.len()))
    } else {
        (evs.len().saturating_sub(limit), evs.len())
    };
    let events: Vec<Value> = evs[lo..hi]
        .iter()
        .map(|(t, m, i)| json!({"timestamp": t, "message": m, "ingestionTime": i}))
        .collect();
    Ok(json!({
        "events": events,
        "nextForwardToken": format!("f/{hi:0>56}"),
        "nextBackwardToken": format!("b/{lo:0>56}"),
    }))
}

/// CloudWatch Logs filter patterns: terms (all required), `"quoted phrases"`, `?term`
/// (any of), `-term` (exclude), and JSON selectors `{ $.level = "ERROR" }`.
fn filter_matches(pattern: &str, message: &str) -> bool {
    let p = pattern.trim();
    if p.is_empty() || p == "\"\"" {
        return true;
    }
    if p.starts_with('{') && p.ends_with('}') {
        let Ok(v) = serde_json::from_str::<Value>(message.trim()) else {
            return false;
        };
        let inner = &p[1..p.len() - 1];
        let re =
            regex::Regex::new(r#"\$\.([\w.\[\]]+)\s*(=|!=|>=|<=|>|<)\s*("([^"]*)"|[^\s&|)]+)"#)
                .expect("valid regex");
        let conj_or = inner.contains("||");
        let mut results = Vec::new();
        for c in re.captures_iter(inner) {
            let ptr = format!("/{}", c[1].replace(['.', '['], "/").replace(']', ""));
            let have = v.pointer(&ptr);
            let want = c
                .get(4)
                .map(|m| m.as_str().to_owned())
                .unwrap_or_else(|| c[3].to_owned());
            let ok = match have {
                None => &c[2] == "!=",
                Some(h) => {
                    let ht = match h {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let num = ht.parse::<f64>().ok().zip(want.parse::<f64>().ok());
                    match (&c[2], num) {
                        ("=", _) => crate::util::glob_match(&want, &ht),
                        ("!=", _) => !crate::util::glob_match(&want, &ht),
                        (">", Some((a, b))) => a > b,
                        ("<", Some((a, b))) => a < b,
                        (">=", Some((a, b))) => a >= b,
                        ("<=", Some((a, b))) => a <= b,
                        _ => false,
                    }
                }
            };
            results.push(ok);
        }
        return if conj_or {
            results.iter().any(|r| *r)
        } else {
            !results.is_empty() && results.iter().all(|r| *r)
        };
    }
    if p.starts_with('[') {
        return true;
    }
    let mut terms = Vec::new();
    let mut rest = p;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let (prefix, body) = match rest.chars().next() {
            Some('?') => ('?', &rest[1..]),
            Some('-') => ('-', &rest[1..]),
            _ => (' ', rest),
        };
        let (term, remaining) = if let Some(stripped) = body.strip_prefix('"') {
            match stripped.find('"') {
                Some(end) => (stripped[..end].to_owned(), &stripped[end + 1..]),
                None => (stripped.to_owned(), ""),
            }
        } else {
            match body.find(char::is_whitespace) {
                Some(end) => (body[..end].to_owned(), &body[end..]),
                None => (body.to_owned(), ""),
            }
        };
        terms.push((prefix, term));
        rest = remaining;
    }
    let any: Vec<&String> = terms
        .iter()
        .filter(|(p, _)| *p == '?')
        .map(|(_, t)| t)
        .collect();
    if !any.is_empty() {
        return any.iter().any(|t| message.contains(t.as_str()));
    }
    terms.iter().all(|(prefix, t)| match prefix {
        '-' => !message.contains(t.as_str()),
        _ => message.contains(t.as_str()),
    })
}

fn filter_log_events(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let group = s(&g, "logGroupName");
    let names: Vec<String> = input
        .get("logStreamNames")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let prefix = s(input, "logStreamNamePrefix");
    let start = input.get("startTime").and_then(Value::as_i64);
    let end = input.get("endTime").and_then(Value::as_i64);
    let pattern = s(input, "filterPattern");
    let mut events: Vec<Value> = Vec::new();
    for st in streams_of(ctx, &group) {
        let name = s(&st, "logStreamName");
        if (!names.is_empty() && !names.contains(&name)) || !name.starts_with(&prefix) {
            continue;
        }
        for (i, (t, m, ing)) in events_of(&st).into_iter().enumerate() {
            if start.is_some_and(|s| t < s)
                || end.is_some_and(|e| t >= e)
                || !filter_matches(&pattern, &m)
            {
                continue;
            }
            events.push(json!({
                "logStreamName": name,
                "timestamp": t,
                "message": m,
                "ingestionTime": ing,
                "eventId": format!("{t:0>20}{i:0>16}"),
            }));
        }
    }
    events.sort_by_key(|e| e["timestamp"].as_i64());
    let (slice, next) = page(&events, input, 10_000, 10_000);
    Ok(with_next(
        json!({"events": slice, "searchedLogStreams": []}),
        next,
    ))
}

fn start_query(ctx: &mut Ctx, input: &Value) -> Out {
    let mut groups: Vec<String> = Vec::new();
    for key in ["logGroupNames", "logGroupIdentifiers"] {
        if let Some(a) = input.get(key).and_then(Value::as_array) {
            groups.extend(a.iter().filter_map(Value::as_str).map(name_from_ident));
        }
    }
    if let Some(n) = input.get("logGroupName").and_then(Value::as_str) {
        groups.push(n.to_owned());
    }
    let text = s(input, "queryString");
    let source_re = regex::Regex::new(r"(?i)^\s*SOURCE\s+(.+?)\s*\|").expect("valid regex");
    if groups.is_empty()
        && let Some(c) = source_re.captures(&text)
    {
        groups.extend(
            c[1].split(',')
                .map(|g| g.trim().trim_matches(['"', '\'', '`']).to_owned()),
        );
    }
    if groups.is_empty() {
        return Err(invalid("Query must specify at least one log group."));
    }
    for g in &groups {
        if ctx
            .get("log_groups", g)
            .filter(|d| ctx.in_region(d))
            .is_none()
        {
            return Err(not_found(&format!(
                "Log group '{g}' does not exist for account ID '{}' (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException)",
                ctx.account
            )));
        }
    }
    let start = input
        .get("startTime")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("startTime is required"))?;
    let end = input
        .get("endTime")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("endTime is required"))?;
    if end < start {
        return Err(invalid("End time should not be before the start time"));
    }
    let query_text = source_re.replace(&text, "").into_owned();
    let mut events = Vec::new();
    for g in &groups {
        for st in streams_of(ctx, g) {
            for (t, m, ing) in events_of(&st) {
                if t >= start * 1000 && t <= end * 1000 {
                    events.push(Event {
                        timestamp_ms: t,
                        ingestion_ms: ing,
                        message: m,
                        stream: s(&st, "logStreamName"),
                        group: g.clone(),
                    });
                }
            }
        }
    }
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l as usize)
        .or(Some(10_000));
    let outcome = insights::run(
        &insights::Query {
            text: &query_text,
            limit,
            account: &ctx.account,
        },
        &events,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, "MalformedQueryException", e))?;
    let id = ctx.svc.ids.next_uuid("logs-query").to_string();
    let results: Vec<Value> = outcome
        .rows
        .iter()
        .map(|r| {
            Value::Array(
                r.iter()
                    .map(|(f, v)| json!({"field": f, "value": v}))
                    .collect(),
            )
        })
        .collect();
    ctx.put(
        "queries",
        &id,
        json!({
            "queryId": id,
            "queryString": text,
            "status": "Complete",
            "createTime": ctx.now * 1000,
            "logGroupName": groups.first(),
            "results": results,
            "statistics": {
                "recordsMatched": outcome.records_matched as f64,
                "recordsScanned": outcome.records_scanned as f64,
                "estimatedRecordsSkipped": 0.0,
                "bytesScanned": outcome.bytes_scanned as f64,
                "estimatedBytesSkipped": 0.0,
                "logGroupsScanned": groups.len() as f64,
            },
        }),
    );
    Ok(json!({"queryId": id}))
}

fn get_query_results(ctx: &mut Ctx, input: &Value) -> Out {
    let id = s(input, "queryId");
    let q = ctx
        .get("queries", &id)
        .ok_or_else(|| not_found(&format!("Query does not exist: {id}")))?;
    Ok(json!({
        "queryLanguage": "CWLI",
        "results": q["results"],
        "statistics": q["statistics"],
        "status": q["status"],
    }))
}

fn stop_query(ctx: &mut Ctx, input: &Value) -> Out {
    let id = s(input, "queryId");
    let q = ctx
        .get("queries", &id)
        .ok_or_else(|| not_found(&format!("Query does not exist: {id}")))?;
    // Queries finish as soon as they start, so there is nothing left to stop.
    let _ = q;
    Ok(json!({"success": false}))
}

fn describe_queries(ctx: &mut Ctx, input: &Value) -> Out {
    let status = s(input, "status");
    let group = s(input, "logGroupName");
    let queries: Vec<Value> = ctx
        .docs("queries")
        .into_iter()
        .map(|(_, q)| q)
        .filter(|q| status.is_empty() || s(q, "status") == status)
        .filter(|q| group.is_empty() || s(q, "logGroupName") == group)
        .map(|q| {
            json!({
                "queryLanguage": "CWLI",
                "queryId": q["queryId"],
                "queryString": q["queryString"],
                "status": q["status"],
                "createTime": q["createTime"],
                "logGroupName": q["logGroupName"],
            })
        })
        .collect();
    let (slice, next) = page(&queries, input, 1000, 1000);
    Ok(with_next(json!({"queries": slice}), next))
}

fn describe_query_definitions(ctx: &mut Ctx, input: &Value) -> Out {
    let prefix = s(input, "queryDefinitionNamePrefix");
    let defs: Vec<Value> = ctx
        .docs("query_definitions")
        .into_iter()
        .map(|(k, mut d)| {
            if d.get("queryDefinitionId").is_none() {
                d["queryDefinitionId"] = json!(k);
            }
            if let Some(t) = d.get("lastModified").and_then(ms) {
                d["lastModified"] = json!(t);
            }
            d.as_object_mut()
                .map(|o| o.entry("queryLanguage").or_insert(json!("CWLI")));
            d.as_object_mut()
                .map(|o| o.entry("logGroupNames").or_insert(json!([])));
            d
        })
        .filter(|d| s(d, "name").starts_with(&prefix))
        .collect();
    let (slice, next) = page(&defs, input, 1000, 1000);
    Ok(with_next(json!({"queryDefinitions": slice}), next))
}

fn put_query_definition(ctx: &mut Ctx, input: &Value) -> Out {
    let id = match input.get("queryDefinitionId").and_then(Value::as_str) {
        Some(id) => id.to_owned(),
        None => ctx.svc.ids.next_uuid("query-definition").to_string(),
    };
    ctx.put(
        "query_definitions",
        &id,
        json!({
            "queryLanguage": input.get("queryLanguage").cloned().unwrap_or(json!("CWLI")),
            "queryDefinitionId": id,
            "name": s(input, "name"),
            "queryString": s(input, "queryString"),
            "lastModified": ctx.now * 1000,
            "logGroupNames": input.get("logGroupNames").cloned().unwrap_or(json!([])),
        }),
    );
    Ok(json!({"queryDefinitionId": id}))
}

fn delete_query_definition(ctx: &mut Ctx, input: &Value) -> Out {
    let id = s(input, "queryDefinitionId");
    let existed = ctx.remove("query_definitions", &id).is_some();
    if !existed {
        return Err(not_found("The specified query definition does not exist."));
    }
    Ok(json!({"success": true}))
}

fn list_anomaly_detectors(ctx: &mut Ctx, input: &Value) -> Out {
    let filter = s(input, "filterLogGroupArn");
    let detectors: Vec<Value> = ctx
        .docs("anomaly_detectors")
        .into_iter()
        .map(|(k, mut d)| {
            if d.get("anomalyDetectorArn").is_none() {
                d["anomalyDetectorArn"] = json!(k);
            }
            for t in ["creationTimeStamp", "lastModifiedTimeStamp"] {
                if let Some(v) = d.get(t).and_then(ms) {
                    d[t] = json!(v);
                }
            }
            d
        })
        .filter(|d| {
            filter.is_empty()
                || d.get("logGroupArnList")
                    .and_then(Value::as_array)
                    .is_some_and(|a| {
                        a.iter().any(|x| {
                            x.as_str().is_some_and(|x| {
                                x.trim_end_matches(":*") == filter.trim_end_matches(":*")
                            })
                        })
                    })
        })
        .collect();
    let (slice, next) = page(&detectors, input, 50, 50);
    Ok(with_next(json!({"anomalyDetectors": slice}), next))
}

fn list_anomalies(ctx: &mut Ctx, input: &Value) -> Out {
    let det = s(input, "anomalyDetectorArn");
    let suppression = s(input, "suppressionState");
    let anomalies: Vec<Value> = ctx
        .docs("anomalies")
        .into_iter()
        .map(|(_, mut a)| {
            for t in ["firstSeen", "lastSeen"] {
                if let Some(v) = a.get(t).and_then(ms) {
                    a[t] = json!(v);
                }
            }
            a
        })
        .filter(|a| det.is_empty() || s(a, "anomalyDetectorArn") == det)
        .filter(|a| {
            suppression != "UNSUPPRESSED"
                || !a
                    .get("suppressed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .collect();
    let (slice, next) = page(&anomalies, input, 50, 50);
    Ok(with_next(json!({"anomalies": slice}), next))
}

fn describe_index_policies(ctx: &mut Ctx, input: &Value) -> Out {
    let idents: Vec<String> = input
        .get("logGroupIdentifiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(name_from_ident)
                .collect()
        })
        .unwrap_or_default();
    let mut policies = Vec::new();
    for name in idents {
        let g = ctx
            .get("log_groups", &name)
            .ok_or_else(|| not_found(NO_GROUP))?;
        if let Some(p) = g.get("indexPolicy").filter(|p| !p.is_null()) {
            policies.push(json!({
                "logGroupIdentifier": group_arn(ctx, &name),
                "lastUpdateTime": ctx.now * 1000,
                "policyDocument": match p { Value::String(s) => s.clone(), o => o.to_string() },
                "policyName": "toolbench-index-policy",
                "source": "LOG_GROUP",
            }));
        }
    }
    Ok(json!({"indexPolicies": policies}))
}

fn describe_metric_filters(ctx: &mut Ctx, input: &Value) -> Out {
    let group = group_name(input);
    if !group.is_empty() {
        group_of(ctx, input)?;
    }
    let prefix = s(input, "filterNamePrefix");
    let filters: Vec<Value> = ctx
        .docs("metric_filters")
        .into_iter()
        .map(|(_, f)| f)
        .filter(|f| group.is_empty() || s(f, "logGroupName") == group)
        .filter(|f| s(f, "filterName").starts_with(&prefix))
        .collect();
    Ok(json!({"metricFilters": filters}))
}

fn put_metric_filter(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let group = s(&g, "logGroupName");
    let name = s(input, "filterName");
    ctx.put(
        "metric_filters",
        &format!("{group}|{name}"),
        json!({
            "filterName": name,
            "filterPattern": s(input, "filterPattern"),
            "metricTransformations": input.get("metricTransformations").cloned().unwrap_or(json!([])),
            "creationTime": ctx.now * 1000,
            "logGroupName": group,
        }),
    );
    Ok(json!({}))
}

fn list_tags_for_resource(ctx: &mut Ctx, input: &Value) -> Out {
    let arn = s(input, "resourceArn");
    let name = name_from_ident(&arn);
    let g = ctx
        .get("log_groups", &name)
        .ok_or_else(|| not_found(NO_GROUP))?;
    Ok(json!({"tags": g.get("tags").cloned().unwrap_or(json!({}))}))
}

fn tag_group(ctx: &mut Ctx, input: &Value, field: &str) -> Out {
    let g = group_of(ctx, input)?;
    let name = s(&g, "logGroupName");
    let doc = ctx.get_mut("log_groups", &name).expect("exists");
    let mut tags = doc
        .get("tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (k, v) in input
        .get(field)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
    {
        tags.insert(k, v);
    }
    doc["tags"] = Value::Object(tags);
    Ok(json!({}))
}

fn tag_resource(ctx: &mut Ctx, input: &Value) -> Out {
    let name = name_from_ident(&s(input, "resourceArn"));
    tag_group(
        ctx,
        &json!({"logGroupName": name, "tags": input.get("tags")}),
        "tags",
    )
}

fn untag(ctx: &mut Ctx, input: &Value) -> Out {
    let name = if input.get("resourceArn").is_some() {
        name_from_ident(&s(input, "resourceArn"))
    } else {
        group_name(input)
    };
    let keys: Vec<String> = input
        .get("tags")
        .or_else(|| input.get("tagKeys"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let doc = ctx
        .get_mut("log_groups", &name)
        .ok_or_else(|| not_found(NO_GROUP))?;
    if let Some(t) = doc.get_mut("tags").and_then(Value::as_object_mut) {
        for k in keys {
            t.shift_remove(&k);
        }
    }
    Ok(json!({}))
}

fn get_log_group_fields(ctx: &mut Ctx, input: &Value) -> Out {
    let g = group_of(ctx, input)?;
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut total = 0usize;
    for st in streams_of(ctx, &s(&g, "logGroupName")) {
        for (_, m, _) in events_of(&st) {
            total += 1;
            let mut names = vec![
                "@timestamp".to_owned(),
                "@message".to_owned(),
                "@logStream".to_owned(),
                "@log".to_owned(),
                "@ingestionTime".to_owned(),
            ];
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(m.trim()) {
                names.extend(o.keys().cloned());
            }
            for n in names {
                match counts.iter_mut().find(|(k, _)| *k == n) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((n, 1)),
                }
            }
        }
    }
    let fields: Vec<Value> = counts
        .into_iter()
        .map(
            |(name, c)| json!({"name": name, "percent": (c * 100).checked_div(total).unwrap_or(0)}),
        )
        .collect();
    Ok(json!({"logGroupFields": fields}))
}

#[cfg(test)]
mod tests {
    use super::filter_matches;

    #[test]
    fn filter_patterns() {
        assert!(filter_matches("ERROR", "an ERROR happened"));
        assert!(!filter_matches("ERROR timeout", "an ERROR happened"));
        assert!(filter_matches(
            "\"payment failed\"",
            "payment failed for order 1"
        ));
        assert!(filter_matches("?WARN ?ERROR", "WARN disk"));
        assert!(filter_matches("ERROR -retry", "ERROR final"));
        assert!(!filter_matches("ERROR -retry", "ERROR will retry"));
        assert!(filter_matches(
            "{ $.level = \"ERROR\" }",
            "{\"level\":\"ERROR\"}"
        ));
        assert!(filter_matches("{ $.latency > 500 }", "{\"latency\":812}"));
        assert!(!filter_matches("{ $.latency > 500 }", "not json"));
    }
}
