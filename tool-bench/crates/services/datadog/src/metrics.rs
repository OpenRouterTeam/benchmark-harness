//! v1 timeseries query (`GET /api/v1/query?from&to&query`), computed from the `metrics`
//! collection: `{metric, tags, unit?, interval, points: [[epoch_s, value], ...]}`.
//!
//! Supported syntax: `[space_aggr:]metric{scope}[ by {tag,..}][.rollup(method[, secs])]`,
//! several queries separated by commas. Space aggregators: avg, sum, min, max, count and
//! percentiles (`p50`..`p99`). Scope: `*`, `tag:value` terms joined by `,` or `AND`, `!` to
//! negate, `*` wildcards in values.

use std::collections::BTreeMap;

use axum::{
    extract::{RawQuery, State},
    response::Response,
};
use mock_core::{http::json, world::SharedWorld};
use serde_json::{Value, json as json_value};

use crate::{
    search::query::glob,
    support::{OK, docs, errors, int_param, parse_query},
};

/// Datadog's rollup intervals (seconds), used when a window holds more than ~300 points.
const AUTO_INTERVALS: [i64; 13] = [
    1, 60, 120, 300, 600, 1200, 1800, 3600, 7200, 14400, 28800, 43200, 86400,
];
const MAX_POINTS: i64 = 300;

#[derive(Debug, Clone, PartialEq)]
struct MetricQuery {
    text: String,
    aggr: String,
    metric: String,
    scope: Vec<String>,
    group_by: Vec<String>,
    rollup: Option<(String, Option<i64>)>,
}

/// Splits on commas outside `{}` and `()`.
fn split_top(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '{' | '(' => depth += 1,
            '}' | ')' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            parts.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    parts
}

fn is_aggr(a: &str) -> bool {
    matches!(a, "avg" | "sum" | "min" | "max" | "count")
        || (a.len() == 3 && a.starts_with('p') && a[1..].parse::<u8>().is_ok_and(|p| p <= 99))
}

fn parse_one(text: &str) -> Result<MetricQuery, String> {
    let text = text.trim();
    let open = text
        .find('{')
        .ok_or_else(|| format!("missing scope in '{text}' (use {{*}} for all)"))?;
    let close = open
        + text[open..]
            .find('}')
            .ok_or_else(|| format!("unclosed scope in '{text}'"))?;
    let head = &text[..open];
    let (aggr, metric) = match head.split_once(':') {
        Some((a, m)) => (a.trim().to_lowercase(), m.trim()),
        None => ("avg".to_owned(), head.trim()),
    };
    if !is_aggr(&aggr) {
        return Err(format!("unknown aggregator '{aggr}'"));
    }
    if metric.is_empty()
        || !metric
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return Err(format!("invalid metric name '{metric}'"));
    }
    let scope: Vec<String> = text[open + 1..close]
        .replace(" AND ", ",")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s != "*")
        .collect();
    if scope.iter().any(|s| s.contains(" OR ") || s.contains(' ')) {
        return Err("unsupported scope expression".to_owned());
    }

    let mut rest = text[close + 1..].trim();
    let mut group_by = Vec::new();
    if let Some(after) = rest.strip_prefix("by") {
        let after = after.trim_start();
        let inner_start = after
            .strip_prefix('{')
            .ok_or_else(|| "expected '{' after 'by'".to_owned())?;
        let end = inner_start
            .find('}')
            .ok_or_else(|| "unclosed group by".to_owned())?;
        group_by = inner_start[..end]
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        rest = inner_start[end + 1..].trim();
    }

    let mut rollup = None;
    while !rest.is_empty() {
        let Some(call) = rest.strip_prefix('.') else {
            return Err(format!("unexpected '{rest}'"));
        };
        let open = call
            .find('(')
            .ok_or_else(|| format!("unexpected '{rest}'"))?;
        let close = call
            .find(')')
            .ok_or_else(|| "unclosed function call".to_owned())?;
        let name = &call[..open];
        let args: Vec<&str> = call[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .collect();
        match name {
            "rollup" => {
                let (method, secs) = match args.as_slice() {
                    [m] if m.parse::<i64>().is_err() => (m.to_string(), None),
                    [s] => (
                        "avg".to_owned(),
                        Some(s.parse::<i64>().map_err(|e| e.to_string())?),
                    ),
                    [m, s] => (
                        m.to_string(),
                        Some(
                            s.parse::<i64>()
                                .map_err(|_| format!("invalid rollup interval '{s}'"))?,
                        ),
                    ),
                    _ => return Err("rollup takes a method and an optional interval".to_owned()),
                };
                if !matches!(method.as_str(), "avg" | "sum" | "min" | "max" | "count") {
                    return Err(format!("unknown rollup method '{method}'"));
                }
                if secs.is_some_and(|s| s <= 0) {
                    return Err("rollup interval must be positive".to_owned());
                }
                rollup = Some((method, secs));
            }
            "as_count" | "as_rate" | "fill" => {}
            other => return Err(format!("unsupported function '{other}'")),
        }
        rest = call[close + 1..].trim();
    }
    Ok(MetricQuery {
        text: text.to_owned(),
        aggr,
        metric: metric.to_owned(),
        scope,
        group_by,
        rollup,
    })
}

fn tags_of(series: &Value) -> Vec<String> {
    series["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect()
}

fn scope_matches(scope: &[String], tags: &[String]) -> bool {
    scope.iter().all(|term| match term.strip_prefix('!') {
        Some(neg) => !tags.iter().any(|t| glob(neg, t)),
        None => tags.iter().any(|t| glob(term, t)),
    })
}

fn aggregate(method: &str, values: &[f64]) -> f64 {
    match method {
        "sum" => values.iter().sum(),
        "min" => values.iter().copied().fold(f64::INFINITY, f64::min),
        "max" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "count" => values.len() as f64,
        p if p.starts_with('p') && p.len() == 3 => {
            let pct: f64 = p[1..].parse().unwrap_or(50.0);
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
            sorted[rank.clamp(1, sorted.len()) - 1]
        }
        _ => values.iter().sum::<f64>() / values.len() as f64,
    }
}

fn unit(name: &str) -> Value {
    let (family, plural, short, scale) = match name {
        "nanosecond" => ("time", "nanoseconds", "ns", 1e-9),
        "microsecond" => ("time", "microseconds", "μs", 1e-6),
        "millisecond" => ("time", "milliseconds", "ms", 0.001),
        "second" => ("time", "seconds", "s", 1.0),
        "minute" => ("time", "minutes", "min", 60.0),
        "percent" => ("percentage", "percent", "%", 1.0),
        "byte" => ("bytes", "bytes", "B", 1.0),
        "kibibyte" => ("bytes", "kibibytes", "KiB", 1024.0),
        "mebibyte" => ("bytes", "mebibytes", "MiB", 1_048_576.0),
        "request" => ("network", "requests", "req", 1.0),
        "error" => ("general", "errors", "err", 1.0),
        "connection" => ("network", "connections", "conn", 1.0),
        other => ("general", other, other, 1.0),
    };
    json_value!({
        "family": family, "name": name, "plural": plural, "scale_factor": scale, "short_name": short,
    })
}

fn run_query(
    q: &MetricQuery,
    index: usize,
    all_series: &[Value],
    from: i64,
    to: i64,
) -> Vec<Value> {
    let matched: Vec<&Value> = all_series
        .iter()
        .filter(|s| s["metric"].as_str() == Some(q.metric.as_str()))
        .filter(|s| scope_matches(&q.scope, &tags_of(s)))
        .collect();
    if matched.is_empty() {
        return Vec::new();
    }
    let native = matched
        .iter()
        .filter_map(|s| s["interval"].as_i64())
        .max()
        .unwrap_or(60);
    let (method, interval) = match &q.rollup {
        Some((m, Some(secs))) => (m.clone(), (*secs).max(1)),
        rollup => {
            let method = rollup.as_ref().map_or("avg".to_owned(), |(m, _)| m.clone());
            let target = (to - from) / MAX_POINTS;
            let interval = if target > native {
                AUTO_INTERVALS
                    .iter()
                    .copied()
                    .find(|i| *i >= target)
                    .unwrap_or(86_400)
            } else {
                native
            };
            (method, interval)
        }
    };

    // group key -> (tag_set, bucket -> per-series rolled-up values)
    let mut groups: BTreeMap<Vec<String>, BTreeMap<i64, Vec<f64>>> = BTreeMap::new();
    let mut unit_name: Option<String> = None;
    for series in matched {
        let tags = tags_of(series);
        let mut key = Vec::new();
        for g in &q.group_by {
            match tags
                .iter()
                .find(|t| t.split_once(':').is_some_and(|(k, _)| k == g))
            {
                Some(t) => key.push(t.clone()),
                None => key.push(format!("{g}:N/A")),
            }
        }
        if unit_name.is_none() {
            unit_name = series["unit"].as_str().map(str::to_owned);
        }
        let mut buckets: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for point in series["points"].as_array().into_iter().flatten() {
            let (Some(ts), Some(v)) = (point[0].as_i64(), point[1].as_f64()) else {
                continue;
            };
            if ts < from || ts > to {
                continue;
            }
            buckets
                .entry(ts.div_euclid(interval) * interval)
                .or_default()
                .push(v);
        }
        let group = groups.entry(key).or_default();
        for (bucket, values) in buckets {
            group
                .entry(bucket)
                .or_default()
                .push(aggregate(&method, &values));
        }
    }

    let scope_text = |extra: &[String]| {
        let mut all = q.scope.clone();
        all.extend(extra.iter().cloned());
        if all.is_empty() {
            "*".to_owned()
        } else {
            all.join(",")
        }
    };
    groups
        .into_iter()
        .filter(|(_, points)| !points.is_empty())
        .map(|(tag_set, points)| {
            let pointlist: Vec<Value> = points
                .iter()
                .map(|(ts, values)| json_value!([(*ts * 1000) as f64, aggregate(&q.aggr, values)]))
                .collect();
            let first = *points.keys().next().expect("non-empty");
            let last = *points.keys().last().expect("non-empty");
            let scope = scope_text(&tag_set);
            json_value!({
                "unit": unit_name.as_deref().map_or(json_value!(null), |u| json_value!([unit(u), null])),
                "query_index": index,
                "aggr": q.aggr,
                "metric": q.metric,
                "tag_set": tag_set,
                "expression": format!("{}:{}{{{}}}", q.aggr, q.metric, scope),
                "scope": scope,
                "interval": interval,
                "length": pointlist.len(),
                "start": first * 1000,
                "end": last * 1000 + interval * 1000 - 1,
                "pointlist": pointlist,
                "display_name": q.metric,
                "attributes": {},
            })
        })
        .collect()
}

pub async fn query(State(world): State<SharedWorld>, RawQuery(raw): RawQuery) -> Response {
    let params = parse_query(raw.as_deref());
    let (from, to) = match (int_param(&params, "from"), int_param(&params, "to")) {
        (Ok(Some(f)), Ok(Some(t))) => (f, t),
        (Ok(None), _) => return errors::bad_request("Missing required parameter 'from'"),
        (_, Ok(None)) => return errors::bad_request("Missing required parameter 'to'"),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let Some(text) = params.get("query").filter(|q| !q.trim().is_empty()) else {
        return errors::bad_request("Missing required parameter 'query'");
    };
    if from >= to {
        return errors::bad_request("The 'from' parameter must be earlier than 'to'");
    }
    let queries = match split_top(text)
        .iter()
        .map(|q| parse_one(q))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(q) => q,
        Err(e) => return errors::bad_request(&format!("Error parsing query: {e}")),
    };
    let world = world.lock().expect("world lock");
    let all_series = docs(&world, "metrics");
    let series: Vec<Value> = queries
        .iter()
        .enumerate()
        .flat_map(|(i, q)| run_query(q, i, &all_series, from, to))
        .collect();
    let group_by: Vec<String> = queries
        .iter()
        .flat_map(|q| q.group_by.iter().cloned())
        .collect();
    json(
        OK,
        json_value!({
            "status": "ok",
            "res_type": "time_series",
            "resp_version": 1,
            "query": text,
            "from_date": from * 1000,
            "to_date": to * 1000,
            "series": series,
            "values": [],
            "times": [],
            "message": "",
            "group_by": group_by,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_queries() {
        let q = parse_one("max:checkout.api.latency.p95{env:prod,service:checkout-api} by {host}.rollup(max, 300)").unwrap();
        assert_eq!(q.aggr, "max");
        assert_eq!(q.metric, "checkout.api.latency.p95");
        assert_eq!(q.scope, ["env:prod", "service:checkout-api"]);
        assert_eq!(q.group_by, ["host"]);
        assert_eq!(q.rollup, Some(("max".to_owned(), Some(300))));
        let q = parse_one("system.cpu.user{*}").unwrap();
        assert_eq!(q.aggr, "avg");
        assert!(q.scope.is_empty());
        assert!(parse_one("avg:system.cpu.user").is_err());
        assert!(parse_one("median:x{*}").is_err());
        assert!(parse_one("avg:x{*}.ewma_3()").is_err());
        assert_eq!(split_top("a{x,y}, b{*}"), ["a{x,y}", "b{*}"]);
    }

    #[test]
    fn aggregates() {
        assert_eq!(aggregate("avg", &[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(aggregate("max", &[1.0, 5.0, 3.0]), 5.0);
        assert_eq!(aggregate("p50", &[1.0, 5.0, 3.0]), 3.0);
        assert_eq!(aggregate("count", &[1.0, 5.0]), 2.0);
    }
}
