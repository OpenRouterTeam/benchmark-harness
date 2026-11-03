//! Datasource backends reached through `/api/datasources/uid/<uid>/resources/...` and
//! `/api/datasources/proxy/uid/<uid>/...`: Prometheus, Loki, Tempo, Pyroscope and
//! Alertmanager, plus the unified `/api/ds/query`.
//!
//! Log and trace timestamps are stored at scenario time. When a request's time range is
//! anchored on a different clock (the MCP server uses the wall clock for "now-1h"), the data
//! is shifted so the scenario's "now" lines up with the request's end.

use std::collections::BTreeMap;

use axum::{
    http::{Method, StatusCode, header},
    response::Response,
};
use mock_core::http::json;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    Ctx, Req, logql, not_found, ok, parse_rfc3339, parse_time,
    promql::{self, Labels, Series},
    rfc3339, text,
};

pub(crate) fn proxy(ctx: &mut Ctx, req: &Req, uid: &str, rest: &[&str]) -> Response {
    let Some(ds) = ctx.get("datasources", uid) else {
        return not_found("Data source not found");
    };
    let path = format!("/{}", rest.join("/"));
    match ds["type"].as_str().unwrap_or_default() {
        "prometheus" => prometheus(ctx, req, uid, &path),
        "loki" => loki(ctx, req, uid, &path),
        "tempo" => tempo(ctx, req, uid, &path),
        "grafana-pyroscope-datasource" => pyroscope(ctx, req, uid, &path),
        "alertmanager" => match path.as_str() {
            "/api/v2/status" => ok(json!({
                "cluster": {"peers": [], "status": "ready", "name": "01JB0ZK4RX8Y2V7M3E5Q9T6WND"},
                "config": {"original": "global:\n  resolve_timeout: 5m\nroute:\n  receiver: default\nreceivers:\n- name: default\n"},
                "uptime": rfc3339(ctx.now - 86_400.0 * 21.0),
                "versionInfo": {"branch": "HEAD", "buildDate": "20250307-15:05:18", "buildUser": "root@7e5f4c1b3d9a", "goVersion": "go1.23.7", "revision": "a3f4d2b", "version": "0.28.1"},
            })),
            "/api/v2/alerts" | "/api/v2/silences" => ok(json!([])),
            _ => text(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                "404 page not found\n",
            ),
        },
        other => json(
            StatusCode::BAD_GATEWAY,
            json!({"message": format!("Plugin health check failed: datasource type {other} does not support resource calls"), "traceID": ""}),
        ),
    }
}

/// Seconds to add to stored timestamps so data lines up with a request ending at `end`.
fn shift(ctx: &Ctx, end: Option<f64>) -> f64 {
    match end {
        Some(e) if (e - ctx.now).abs() > 6.0 * 3600.0 => e - ctx.now,
        _ => 0.0,
    }
}

fn fmt_num(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        }
    } else {
        format!("{v}")
    }
}

fn labels_json(l: &BTreeMap<String, String>) -> Value {
    Value::Object(l.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

// ───────────────────────────── Prometheus ─────────────────────────────

fn prom_error(code: StatusCode, typ: &str, msg: &str) -> Response {
    json(
        code,
        json!({"status": "error", "errorType": typ, "error": msg}),
    )
}

fn prom_series(ctx: &Ctx, uid: &str) -> Vec<Series> {
    ctx.coll("prom_series")
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| s["datasource"] == uid)
        .map(|s| Series {
            labels: s["labels"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
            value: s["value"].as_f64().unwrap_or(0.0),
            rate: s["rate"].as_f64(),
        })
        .collect()
}

fn match_sets(params: &[(String, String)]) -> Result<Vec<Vec<promql::Matcher>>, String> {
    let mut out = Vec::new();
    for (k, v) in params {
        if k == "match[]" {
            let e = promql::parse(v)?;
            let mut sels = Vec::new();
            promql::selectors(&e, &mut sels);
            out.extend(sels);
        }
    }
    Ok(out)
}

fn prometheus(ctx: &mut Ctx, req: &Req, uid: &str, path: &str) -> Response {
    let params = req.params();
    let p = |k: &str| params.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    let series = prom_series(ctx, uid);
    let selected = |sets: &[Vec<promql::Matcher>]| -> Vec<Series> {
        series
            .iter()
            .filter(|s| {
                sets.is_empty()
                    || sets
                        .iter()
                        .any(|ms| ms.iter().all(|m| m.matches(&s.labels)))
            })
            .cloned()
            .collect()
    };
    match path {
        "/api/v1/query" => {
            let Some(q) = p("query") else {
                return prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    "invalid parameter \"query\": unknown position: parse error: no expression found in input",
                );
            };
            let t = match p("time") {
                Some(t) => match parse_time(t) {
                    Some(t) => t,
                    None => {
                        return prom_error(
                            StatusCode::BAD_REQUEST,
                            "bad_data",
                            &format!(
                                "invalid parameter \"time\": cannot parse {t:?} to a valid timestamp"
                            ),
                        );
                    }
                },
                None => ctx.now,
            };
            let expr = match promql::parse(q) {
                Ok(e) => e,
                Err(e) => {
                    return prom_error(
                        StatusCode::BAD_REQUEST,
                        "bad_data",
                        &format!("invalid parameter \"query\": 1:1: parse error: {e}"),
                    );
                }
            };
            match promql::eval(
                &expr,
                &promql::Ctx {
                    series: &series,
                    time: t,
                },
            ) {
                Ok(promql::Value::Scalar(v)) => ok(
                    json!({"status": "success", "data": {"resultType": "scalar", "result": [t, fmt_num(v)]}}),
                ),
                Ok(promql::Value::Str(s)) => ok(
                    json!({"status": "success", "data": {"resultType": "string", "result": [t, s]}}),
                ),
                Ok(promql::Value::Vector(v)) => ok(
                    json!({"status": "success", "data": {"resultType": "vector", "result": v.into_iter().map(|(l, x)| json!({"metric": labels_json(&l), "value": [t, fmt_num(x)]})).collect::<Vec<_>>()}}),
                ),
                Ok(promql::Value::Range(s, d)) => ok(
                    json!({"status": "success", "data": {"resultType": "matrix", "result": s.into_iter().map(|s| {
                    let n = ((d / 15.0).floor() as i64).max(1);
                    let values: Vec<Value> = (0..n).map(|i| json!([t - (n - 1 - i) as f64 * 15.0, fmt_num(s.value)])).collect();
                    json!({"metric": labels_json(&s.labels), "values": values})
                }).collect::<Vec<_>>()}}),
                ),
                Err(e) => prom_error(StatusCode::BAD_REQUEST, "bad_data", &e),
            }
        }
        "/api/v1/query_range" => {
            let (Some(q), Some(start), Some(end), Some(step)) = (
                p("query"),
                p("start").and_then(parse_time),
                p("end").and_then(parse_time),
                p("step"),
            ) else {
                return prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    "invalid parameter \"start\": cannot parse \"\" to a valid timestamp",
                );
            };
            let step = step
                .parse::<f64>()
                .ok()
                .or_else(|| parse_duration(step))
                .unwrap_or(0.0);
            if step <= 0.0 {
                return prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    "zero or negative query resolution step widths are not accepted. Try a positive integer",
                );
            }
            if end < start {
                return prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    "end timestamp must not be before start time",
                );
            }
            if (end - start) / step > 11_000.0 {
                return prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    "exceeded maximum resolution of 11,000 points per timeseries. Try decreasing the query resolution (?step=XX)",
                );
            }
            let expr = match promql::parse(q) {
                Ok(e) => e,
                Err(e) => {
                    return prom_error(
                        StatusCode::BAD_REQUEST,
                        "bad_data",
                        &format!("invalid parameter \"query\": 1:1: parse error: {e}"),
                    );
                }
            };
            let mut out: Vec<(Labels, Vec<Value>)> = Vec::new();
            let mut t = start;
            while t <= end + 1e-9 {
                let v = match promql::eval(
                    &expr,
                    &promql::Ctx {
                        series: &series,
                        time: t,
                    },
                ) {
                    Ok(promql::Value::Scalar(x)) => vec![(Labels::new(), x)],
                    Ok(promql::Value::Vector(v)) => v,
                    Ok(_) => {
                        return prom_error(
                            StatusCode::BAD_REQUEST,
                            "bad_data",
                            "invalid expression type \"range vector\" for range query, must be Scalar or instant Vector",
                        );
                    }
                    Err(e) => return prom_error(StatusCode::BAD_REQUEST, "bad_data", &e),
                };
                for (l, x) in v {
                    let point = json!([(t * 1000.0).round() / 1000.0, fmt_num(x)]);
                    match out.iter_mut().find(|(k, _)| *k == l) {
                        Some((_, pts)) => pts.push(point),
                        None => out.push((l, vec![point])),
                    }
                }
                t += step;
            }
            ok(
                json!({"status": "success", "data": {"resultType": "matrix", "result": out.into_iter().map(|(l, v)| json!({"metric": labels_json(&l), "values": v})).collect::<Vec<_>>()}}),
            )
        }
        "/api/v1/labels" => match match_sets(&params) {
            Ok(sets) => {
                let mut names: Vec<String> = selected(&sets)
                    .into_iter()
                    .flat_map(|s| s.labels.into_keys())
                    .collect();
                names.sort();
                names.dedup();
                ok(json!({"status": "success", "data": names}))
            }
            Err(e) => prom_error(
                StatusCode::BAD_REQUEST,
                "bad_data",
                &format!("invalid parameter \"match[]\": {e}"),
            ),
        },
        "/api/v1/series" => match match_sets(&params) {
            Ok(sets) if !sets.is_empty() => {
                let mut list: Vec<Value> = selected(&sets)
                    .into_iter()
                    .map(|s| labels_json(&s.labels))
                    .collect();
                list.dedup();
                ok(json!({"status": "success", "data": list}))
            }
            Ok(_) => prom_error(
                StatusCode::BAD_REQUEST,
                "bad_data",
                "no match[] parameter provided",
            ),
            Err(e) => prom_error(
                StatusCode::BAD_REQUEST,
                "bad_data",
                &format!("invalid parameter \"match[]\": {e}"),
            ),
        },
        "/api/v1/metadata" => {
            let metric = p("metric").filter(|m| !m.is_empty());
            let limit = p("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(-1);
            let mut data = Map::new();
            for (_, m) in ctx.coll("prom_metadata") {
                if m["datasource"] != uid || metric.is_some_and(|x| m["metric"] != x) {
                    continue;
                }
                if limit >= 0 && data.len() as i64 >= limit {
                    break;
                }
                data.insert(
                    m["metric"].as_str().unwrap_or_default().to_owned(),
                    json!([{"type": m["type"], "help": m["help"], "unit": m["unit"].as_str().unwrap_or("")}]),
                );
            }
            ok(json!({"status": "success", "data": data}))
        }
        "/api/v1/status/buildinfo" => ok(
            json!({"status": "success", "data": {"version": "2.55.1", "revision": "6d7569113f1ca814f1e149f74176656540043b8d", "branch": "HEAD", "buildUser": "root@1f2a3b4c5d6e", "buildDate": "20241105-12:07:39", "goVersion": "go1.23.2"}}),
        ),
        "/api/v1/query_exemplars" => ok(json!({"status": "success", "data": []})),
        "/api/v1/rules" => ok(json!({"status": "success", "data": {"groups": []}})),
        "/api/v1/alerts" => ok(json!({"status": "success", "data": {"alerts": []}})),
        "/api/v1/targets/metadata" => ok(json!({"status": "success", "data": []})),
        p if p.starts_with("/api/v1/label/") && p.ends_with("/values") => {
            let name = crate::url_decode(&p["/api/v1/label/".len()..p.len() - "/values".len()]);
            match match_sets(&params) {
                Ok(sets) => {
                    let mut values: Vec<String> = selected(&sets)
                        .into_iter()
                        .filter_map(|s| s.labels.get(&name).cloned())
                        .collect();
                    values.sort();
                    values.dedup();
                    ok(json!({"status": "success", "data": values}))
                }
                Err(e) => prom_error(
                    StatusCode::BAD_REQUEST,
                    "bad_data",
                    &format!("invalid parameter \"match[]\": {e}"),
                ),
            }
        }
        _ => text(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "404 page not found\n",
        ),
    }
}

fn parse_duration(s: &str) -> Option<f64> {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.')?);
    let n: f64 = num.parse().ok()?;
    Some(
        n * match unit {
            "ms" => 0.001,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            "d" => 86400.0,
            _ => return None,
        },
    )
}

// ───────────────────────────── Loki ─────────────────────────────

fn loki_streams(ctx: &Ctx, uid: &str, offset: f64) -> Vec<logql::Stream> {
    ctx.coll("loki_streams")
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| s["datasource"] == uid)
        .map(|s| logql::Stream {
            labels: s["labels"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
            entries: s["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    let ts = e[0].as_str().and_then(parse_rfc3339)?;
                    Some(logql::Entry {
                        ts: ts + offset,
                        line: e[1].as_str().unwrap_or_default().to_owned(),
                    })
                })
                .collect(),
        })
        .collect()
}

fn loki_error(msg: &str) -> Response {
    text(
        StatusCode::BAD_REQUEST,
        "text/plain; charset=utf-8",
        format!("{msg}\n"),
    )
}

fn ns(t: f64) -> String {
    format!("{}", (t * 1e9).round() as i128)
}

fn loki_stats(entries: usize, bytes: usize) -> Value {
    json!({"summary": {"bytesProcessedPerSecond": bytes * 40, "linesProcessedPerSecond": entries * 40, "totalBytesProcessed": bytes, "totalLinesProcessed": entries, "execTime": 0.025, "queueTime": 0.0004, "subqueries": 0, "totalEntriesReturned": entries, "splits": 1, "shards": 1, "totalPostFilterLines": entries, "totalStructuredMetadataBytesProcessed": 0}})
}

fn loki(ctx: &mut Ctx, req: &Req, uid: &str, path: &str) -> Response {
    let params = req.params();
    let p = |k: &str| params.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    let end = p("end").and_then(parse_time).unwrap_or(ctx.now);
    let start = p("start").and_then(parse_time).unwrap_or(end - 3600.0);
    let offset = shift(ctx, Some(end));
    let streams = loki_streams(ctx, uid, offset);
    match path {
        "/loki/api/v1/query_range" | "/loki/api/v1/query" => {
            let Some(q) = p("query") else {
                return loki_error("parse error : syntax error: unexpected $end");
            };
            let query = match logql::parse(q) {
                Ok(q) => q,
                Err(e) => return loki_error(&e),
            };
            let instant = path.ends_with("/query");
            match query {
                logql::Query::Log(lq) => {
                    if instant {
                        return loki_error(
                            "log queries are not supported as an instant query type, please change your query to a range query type",
                        );
                    }
                    let limit = p("limit")
                        .and_then(|l| l.parse::<usize>().ok())
                        .unwrap_or(100);
                    let backward =
                        p("direction").is_none_or(|d| d.eq_ignore_ascii_case("backward"));
                    let mut hits: Vec<(Labels, f64, String)> = Vec::new();
                    for s in streams.iter().filter(|s| lq.selects(&s.labels)) {
                        for e in &s.entries {
                            if e.ts < start || e.ts >= end {
                                continue;
                            }
                            if let Some(mut l) = lq.pipeline(&s.labels, &e.line) {
                                if !lq.has_parser() {
                                    l = s.labels.clone();
                                }
                                hits.push((l, e.ts, e.line.clone()));
                            }
                        }
                    }
                    hits.sort_by(|a, b| {
                        if backward {
                            b.1.total_cmp(&a.1)
                        } else {
                            a.1.total_cmp(&b.1)
                        }
                    });
                    hits.truncate(limit);
                    let bytes: usize = hits.iter().map(|h| h.2.len()).sum();
                    let n = hits.len();
                    let mut out: Vec<(Labels, Vec<Value>)> = Vec::new();
                    for (l, ts, line) in hits {
                        let v = json!([ns(ts), line]);
                        match out.iter_mut().find(|(k, _)| *k == l) {
                            Some((_, vals)) => vals.push(v),
                            None => out.push((l, vec![v])),
                        }
                    }
                    ok(
                        json!({"status": "success", "data": {"resultType": "streams", "result": out.into_iter().map(|(l, v)| json!({"stream": labels_json(&l), "values": v})).collect::<Vec<_>>(), "stats": loki_stats(n, bytes)}}),
                    )
                }
                logql::Query::Metric(mq) => {
                    if instant {
                        let t = p("time").and_then(parse_time).unwrap_or(end);
                        let off = shift(ctx, Some(t));
                        let streams = loki_streams(ctx, uid, off);
                        let v = logql::eval_metric(&mq, &streams, t);
                        return ok(
                            json!({"status": "success", "data": {"resultType": "vector", "result": v.into_iter().map(|(l, x)| json!({"metric": labels_json(&l), "value": [t, fmt_num(x)]})).collect::<Vec<_>>(), "stats": loki_stats(0, 0)}}),
                        );
                    }
                    let step = p("step")
                        .and_then(|s| s.parse::<f64>().ok().or_else(|| parse_duration(s)))
                        .unwrap_or_else(|| ((end - start) / 250.0).ceil().max(1.0));
                    if (end - start) / step > 11_000.0 {
                        return loki_error(
                            "exceeded maximum resolution of 11,000 points per timeseries. Try increasing the value of the step parameter",
                        );
                    }
                    let mut out: Vec<(Labels, Vec<Value>)> = Vec::new();
                    let mut t = (start / step).ceil() * step;
                    while t <= end {
                        for (l, x) in logql::eval_metric(&mq, &streams, t) {
                            let point = json!([t, fmt_num(x)]);
                            match out.iter_mut().find(|(k, _)| *k == l) {
                                Some((_, pts)) => pts.push(point),
                                None => out.push((l, vec![point])),
                            }
                        }
                        t += step;
                    }
                    ok(
                        json!({"status": "success", "data": {"resultType": "matrix", "result": out.into_iter().map(|(l, v)| json!({"metric": labels_json(&l), "values": v})).collect::<Vec<_>>(), "stats": loki_stats(0, 0)}}),
                    )
                }
            }
        }
        "/loki/api/v1/labels" | "/loki/api/v1/label" => {
            let sel = match p("query").filter(|q| !q.is_empty()).map(logql::parse) {
                Some(Ok(logql::Query::Log(l))) => Some(l),
                Some(Ok(_)) => {
                    return loki_error("parse error : only stream selectors are supported");
                }
                Some(Err(e)) => return loki_error(&e),
                None => None,
            };
            let mut names: Vec<String> = streams
                .iter()
                .filter(|s| {
                    sel.as_ref().is_none_or(|q| q.selects(&s.labels))
                        && s.entries.iter().any(|e| e.ts >= start && e.ts < end)
                })
                .flat_map(|s| s.labels.keys().cloned())
                .collect();
            names.sort();
            names.dedup();
            ok(json!({"status": "success", "data": names}))
        }
        "/loki/api/v1/index/stats" => {
            let sel = match p("query").map(logql::parse) {
                Some(Ok(logql::Query::Log(l))) => l,
                Some(Err(e)) => return loki_error(&e),
                _ => return loki_error("parse error : syntax error: unexpected $end"),
            };
            let mut streams_n = 0;
            let mut entries = 0;
            let mut bytes = 0;
            for s in streams.iter().filter(|s| sel.selects(&s.labels)) {
                let in_range: Vec<&logql::Entry> = s
                    .entries
                    .iter()
                    .filter(|e| e.ts >= start && e.ts < end)
                    .collect();
                if in_range.is_empty() {
                    continue;
                }
                streams_n += 1;
                entries += in_range.len();
                bytes += in_range.iter().map(|e| e.line.len()).sum::<usize>();
            }
            ok(
                json!({"streams": streams_n, "chunks": streams_n * 2, "entries": entries, "bytes": bytes}),
            )
        }
        "/loki/api/v1/patterns" => {
            let sel = match p("query").map(logql::parse) {
                Some(Ok(logql::Query::Log(l))) => l,
                Some(Err(e)) => return loki_error(&e),
                _ => return loki_error("parse error : syntax error: unexpected $end"),
            };
            let digits =
                Regex::new(r"\b[0-9a-f]{8,}\b|\b\d+(\.\d+)?(ms|s|m|h)?\b|\x22[^\x22]*\x22")
                    .expect("regex");
            let mut patterns: Vec<(String, Vec<f64>)> = Vec::new();
            for s in streams.iter().filter(|s| sel.selects(&s.labels)) {
                for e in s.entries.iter().filter(|e| e.ts >= start && e.ts < end) {
                    let pat = digits.replace_all(&e.line, "<_>").into_owned();
                    match patterns.iter_mut().find(|(p, _)| *p == pat) {
                        Some((_, ts)) => ts.push(e.ts),
                        None => patterns.push((pat, vec![e.ts])),
                    }
                }
            }
            let step = p("step").and_then(parse_duration).unwrap_or(60.0);
            let data: Vec<Value> = patterns
                .into_iter()
                .map(|(pat, ts)| {
                    let mut buckets: BTreeMap<i64, i64> = BTreeMap::new();
                    for t in ts {
                        *buckets.entry(((t / step).floor() * step) as i64).or_insert(0) += 1;
                    }
                    json!({"pattern": pat, "samples": buckets.into_iter().map(|(t, c)| json!([t, c])).collect::<Vec<_>>()})
                })
                .collect();
            ok(json!({"status": "success", "data": data}))
        }
        p if p.starts_with("/loki/api/v1/label/") && p.ends_with("/values") => {
            let name =
                crate::url_decode(&p["/loki/api/v1/label/".len()..p.len() - "/values".len()]);
            let sel = match req.q("query").filter(|q| !q.is_empty()).map(logql::parse) {
                Some(Ok(logql::Query::Log(l))) => Some(l),
                Some(Err(e)) => return loki_error(&e),
                _ => None,
            };
            let mut values: Vec<String> = streams
                .iter()
                .filter(|s| {
                    sel.as_ref().is_none_or(|q| q.selects(&s.labels))
                        && s.entries.iter().any(|e| e.ts >= start && e.ts < end)
                })
                .filter_map(|s| s.labels.get(&name).cloned())
                .collect();
            values.sort();
            values.dedup();
            ok(json!({"status": "success", "data": values}))
        }
        "/loki/api/v1/detected_labels" => ok(json!({"detectedLabels": []})),
        "/loki/api/v1/detected_fields" => ok(json!({"fields": [], "limit": 1000})),
        _ => text(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "404 page not found\n",
        ),
    }
}

// ───────────────────────────── Tempo ─────────────────────────────

struct Span {
    id: String,
    parent: String,
    name: String,
    service: String,
    start: f64,
    duration_ms: f64,
    status: String,
    kind: String,
    attrs: Map<String, Value>,
}

struct Trace {
    id: String,
    spans: Vec<Span>,
}

fn tempo_traces(ctx: &Ctx, uid: &str, offset: f64) -> Vec<Trace> {
    ctx.coll("tempo_traces")
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| t["datasource"] == uid)
        .map(|t| {
            let base = t["start"].as_str().and_then(parse_rfc3339).unwrap_or(0.0) + offset;
            Trace {
                id: t["traceID"].as_str().unwrap_or_default().to_owned(),
                spans: t["spans"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| Span {
                        id: s["spanID"].as_str().unwrap_or_default().to_owned(),
                        parent: s["parentSpanID"].as_str().unwrap_or_default().to_owned(),
                        name: s["name"].as_str().unwrap_or_default().to_owned(),
                        service: s["service"].as_str().unwrap_or_default().to_owned(),
                        start: base + s["offsetMs"].as_f64().unwrap_or(0.0) / 1000.0,
                        duration_ms: s["durationMs"].as_f64().unwrap_or(0.0),
                        status: s["status"].as_str().unwrap_or("unset").to_owned(),
                        kind: s["kind"].as_str().unwrap_or("server").to_owned(),
                        attrs: s["attributes"].as_object().cloned().unwrap_or_default(),
                    })
                    .collect(),
            }
        })
        .collect()
}

fn attr_value(v: &Value) -> Value {
    match v {
        Value::String(s) => json!({"stringValue": s}),
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Number(n) if n.is_i64() => json!({"intValue": n.to_string()}),
        Value::Number(n) => json!({"doubleValue": n.as_f64()}),
        other => json!({"stringValue": other.to_string()}),
    }
}

/// One `{ ... }` TraceQL condition evaluated against a span.
fn span_matches(cond: &str, span: &Span) -> Option<bool> {
    let cond = cond.trim();
    if cond.is_empty() || cond == "true" {
        return Some(true);
    }
    if let Some((a, b)) = split_top(cond, "||") {
        return Some(span_matches(&a, span)? || span_matches(&b, span)?);
    }
    if let Some((a, b)) = split_top(cond, "&&") {
        return Some(span_matches(&a, span)? && span_matches(&b, span)?);
    }
    let cond = cond.trim_start_matches('(').trim_end_matches(')').trim();
    let ops = ["!~", "=~", "!=", ">=", "<=", "=", ">", "<"];
    let (field, op, value) = ops
        .iter()
        .find_map(|op| cond.split_once(op).map(|(f, v)| (f.trim(), *op, v.trim())))?;
    let value = value.trim_matches('"');
    let actual: Option<Value> = match field {
        "name" | "span:name" => Some(json!(span.name)),
        "status" | "span:status" => Some(json!(span.status)),
        "kind" | "span:kind" => Some(json!(span.kind)),
        "duration" | "span:duration" => Some(json!(span.duration_ms)),
        "resource.service.name" | ".service.name" | "rootServiceName" | "trace:rootService" => {
            Some(json!(span.service))
        }
        f => {
            let key = f
                .trim_start_matches("span.")
                .trim_start_matches("resource.")
                .trim_start_matches('.');
            span.attrs.get(key).cloned()
        }
    };
    let Some(actual) = actual else {
        return Some(op == "!=");
    };
    let num = |s: &str| -> Option<f64> {
        s.parse::<f64>().ok().or_else(|| {
            let (n, u) = s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.')?);
            let n: f64 = n.parse().ok()?;
            Some(
                n * match u {
                    "ns" => 1e-6,
                    "us" | "µs" => 1e-3,
                    "ms" => 1.0,
                    "s" => 1000.0,
                    "m" => 60_000.0,
                    "h" => 3_600_000.0,
                    _ => return None,
                },
            )
        })
    };
    let a_str = match &actual {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    Some(match op {
        "=" => a_str == value,
        "!=" => a_str != value,
        "=~" => Regex::new(&format!("^(?:{value})$")).is_ok_and(|r| r.is_match(&a_str)),
        "!~" => Regex::new(&format!("^(?:{value})$")).is_ok_and(|r| !r.is_match(&a_str)),
        _ => {
            let a = actual.as_f64().or_else(|| a_str.parse().ok());
            let b = if field.contains("duration") {
                num(value)
            } else {
                value.parse().ok()
            };
            match (a, b) {
                (Some(a), Some(b)) => match op {
                    ">" => a > b,
                    ">=" => a >= b,
                    "<" => a < b,
                    _ => a <= b,
                },
                _ => false,
            }
        }
    })
}

fn split_top(s: &str, sep: &str) -> Option<(String, String)> {
    let mut depth = 0;
    let mut in_str = false;
    let b = s.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'"' => in_str = !in_str,
            b'(' if !in_str => depth += 1,
            b')' if !in_str => depth -= 1,
            _ => {}
        }
        if depth == 0 && !in_str && s[i..].starts_with(sep) {
            return Some((s[..i].to_owned(), s[i + sep.len()..].to_owned()));
        }
    }
    None
}

fn traceql_spans<'a>(q: &str, trace: &'a Trace) -> Result<Vec<&'a Span>, String> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(trace.spans.iter().collect());
    }
    let Some(inner) = q
        .split('|')
        .next()
        .map(str::trim)
        .and_then(|s| s.strip_prefix('{'))
        .and_then(|s| s.strip_suffix('}'))
    else {
        return Err(format!(
            "invalid TraceQL query: parse error at line 1, col 1: syntax error: unexpected IDENTIFIER: {q}"
        ));
    };
    let mut out = Vec::new();
    for s in &trace.spans {
        match span_matches(inner, s) {
            Some(true) => out.push(s),
            Some(false) => {}
            None => {
                return Err(format!(
                    "invalid TraceQL query: parse error at line 1, col 2: syntax error: unexpected {inner}"
                ));
            }
        }
    }
    Ok(out)
}

fn tempo_error(code: StatusCode, msg: &str) -> Response {
    text(code, "text/plain; charset=utf-8", format!("{msg}\n"))
}

fn tempo(ctx: &mut Ctx, req: &Req, uid: &str, path: &str) -> Response {
    let p = |k: &str| req.q(k);
    let end = p("end").and_then(parse_time).unwrap_or(ctx.now);
    let start = p("start").and_then(parse_time).unwrap_or(end - 3600.0);
    let offset = shift(ctx, Some(end));
    let traces = tempo_traces(ctx, uid, offset);
    match path {
        "/api/search" => {
            let q = p("q").unwrap_or("{}");
            let limit = p("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(20);
            let spss = p("spss").and_then(|l| l.parse::<usize>().ok()).unwrap_or(3);
            let mut out = Vec::new();
            for t in &traces {
                let Some(root) = t
                    .spans
                    .iter()
                    .find(|s| s.parent.is_empty())
                    .or(t.spans.first())
                else {
                    continue;
                };
                if root.start < start || root.start > end {
                    continue;
                }
                let matched = match traceql_spans(q, t) {
                    Ok(m) => m,
                    Err(e) => return tempo_error(StatusCode::BAD_REQUEST, &e),
                };
                if matched.is_empty() {
                    continue;
                }
                let spans: Vec<Value> = matched
                    .iter()
                    .take(spss)
                    .map(|s| json!({"spanID": s.id, "name": s.name, "startTimeUnixNano": ns(s.start), "durationNanos": format!("{}", (s.duration_ms * 1e6).round() as i64), "attributes": [{"key": "service.name", "value": {"stringValue": s.service}}]}))
                    .collect();
                let span_set = json!({"spans": spans, "matched": matched.len()});
                out.push(json!({
                    "traceID": t.id, "rootServiceName": root.service, "rootTraceName": root.name, "startTimeUnixNano": ns(root.start),
                    "durationMs": root.duration_ms.round() as i64, "spanSet": span_set, "spanSets": [span_set],
                }));
            }
            out.sort_by(|a, b| {
                b["startTimeUnixNano"]
                    .as_str()
                    .cmp(&a["startTimeUnixNano"].as_str())
            });
            let inspected = traces.len();
            out.truncate(limit);
            ok(
                json!({"traces": out, "metrics": {"inspectedTraces": inspected, "inspectedBytes": (inspected * 4096).to_string(), "totalBlocks": 3, "completedJobs": 3, "totalJobs": 3, "totalBlockBytes": "1830204"}}),
            )
        }
        p if p.starts_with("/api/v2/traces/") && p != "/api/v2/traces/diff" => {
            let id = p.trim_start_matches("/api/v2/traces/").to_lowercase();
            let llm = req
                .headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|a| a.contains("vnd.grafana.llm"));
            let Some(t) = traces.iter().find(|t| {
                t.id.to_lowercase() == id
                    || t.id.trim_start_matches('0') == id.trim_start_matches('0')
            }) else {
                return ok(json!({"trace": {}}));
            };
            let mut services: Vec<(String, Vec<&Span>)> = Vec::new();
            for s in &t.spans {
                match services.iter_mut().find(|(n, _)| *n == s.service) {
                    Some((_, v)) => v.push(s),
                    None => services.push((s.service.clone(), vec![s])),
                }
            }
            let code = |s: &Span| match s.status.as_str() {
                "error" => "STATUS_CODE_ERROR",
                "ok" => "STATUS_CODE_OK",
                _ => "STATUS_CODE_UNSET",
            };
            if llm {
                let services: Vec<Value> = services
                    .into_iter()
                    .map(|(name, spans)| {
                        json!({
                            "serviceName": name, "resource": {"service.name": name},
                            "scopes": [{"name": "io.opentelemetry", "version": "1.40.0", "spans": spans.iter().map(|s| json!({
                                "spanId": s.id, "parentSpanId": s.parent, "name": s.name, "kind": format!("SPAN_KIND_{}", s.kind.to_uppercase()),
                                "startTimeUnixNano": ns(s.start), "endTimeUnixNano": ns(s.start + s.duration_ms / 1000.0),
                                "attributes": s.attrs, "status": {"code": code(s)}, "events": [],
                            })).collect::<Vec<_>>()}],
                        })
                    })
                    .collect();
                return ok(json!({"traceId": t.id, "services": services}));
            }
            let hex_to_b64 = |h: &str| -> String {
                let bytes: Vec<u8> = (0..h.len() / 2)
                    .filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
                    .collect();
                b64(&bytes)
            };
            let rs: Vec<Value> = services
                .into_iter()
                .map(|(name, spans)| {
                    json!({
                        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": name}}]},
                        "scopeSpans": [{"scope": {"name": "io.opentelemetry", "version": "1.40.0"}, "spans": spans.iter().map(|s| {
                            let mut v = json!({
                                "traceId": hex_to_b64(&t.id), "spanId": hex_to_b64(&s.id), "name": s.name, "kind": format!("SPAN_KIND_{}", s.kind.to_uppercase()),
                                "startTimeUnixNano": ns(s.start), "endTimeUnixNano": ns(s.start + s.duration_ms / 1000.0),
                                "attributes": s.attrs.iter().map(|(k, v)| json!({"key": k, "value": attr_value(v)})).collect::<Vec<_>>(),
                                "status": if s.status == "error" { json!({"code": "STATUS_CODE_ERROR"}) } else { json!({}) },
                            });
                            if !s.parent.is_empty() {
                                v["parentSpanId"] = json!(hex_to_b64(&s.parent));
                            }
                            v
                        }).collect::<Vec<_>>()}],
                    })
                })
                .collect();
            ok(json!({"trace": {"resourceSpans": rs}}))
        }
        "/api/v2/search/tags" => {
            let mut resource: Vec<String> = vec!["service.name".into()];
            let mut span: Vec<String> = traces
                .iter()
                .flat_map(|t| t.spans.iter().flat_map(|s| s.attrs.keys().cloned()))
                .collect();
            span.sort();
            span.dedup();
            resource.dedup();
            let scope = p("scope").unwrap_or("all");
            let mut scopes = Vec::new();
            if scope == "all" || scope == "resource" {
                scopes.push(json!({"name": "resource", "tags": resource}));
            }
            if scope == "all" || scope == "span" {
                scopes.push(json!({"name": "span", "tags": span}));
            }
            if scope == "all" || scope == "intrinsic" {
                scopes.push(json!({"name": "intrinsic", "tags": ["duration", "event:name", "kind", "name", "rootName", "rootServiceName", "span:duration", "span:kind", "span:name", "span:status", "span:statusMessage", "status", "statusMessage", "trace:duration", "trace:rootName", "trace:rootService"]}));
            }
            ok(
                json!({"scopes": scopes, "metrics": {"inspectedBytes": "18302", "totalBlocks": 3, "totalJobs": 3, "completedJobs": 3}}),
            )
        }
        p if p.starts_with("/api/v2/search/tag/") && p.ends_with("/values") => {
            let tag = crate::url_decode(&p["/api/v2/search/tag/".len()..p.len() - "/values".len()]);
            let key = tag
                .trim_start_matches("resource.")
                .trim_start_matches("span.")
                .trim_start_matches('.');
            let mut values: Vec<String> = Vec::new();
            for t in &traces {
                for s in &t.spans {
                    let v = match key {
                        "service.name" => Some(s.service.clone()),
                        "name" | "span:name" => Some(s.name.clone()),
                        "status" | "span:status" => Some(s.status.clone()),
                        "kind" | "span:kind" => Some(s.kind.clone()),
                        _ => s.attrs.get(key).map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        }),
                    };
                    values.extend(v);
                }
            }
            values.sort();
            values.dedup();
            ok(
                json!({"tagValues": values.into_iter().map(|v| json!({"type": "string", "value": v})).collect::<Vec<_>>(), "metrics": {"inspectedBytes": "18302", "totalBlocks": 3, "totalJobs": 3, "completedJobs": 3}}),
            )
        }
        "/api/metrics/query_range" | "/api/metrics/query" => {
            let q = p("q").unwrap_or_default();
            let filter = q.split('|').next().unwrap_or_default();
            let mut groups: BTreeMap<String, (usize, f64)> = BTreeMap::new();
            for t in &traces {
                let spans = match traceql_spans(filter, t) {
                    Ok(s) => s,
                    Err(e) => return tempo_error(StatusCode::BAD_REQUEST, &e),
                };
                for s in spans
                    .into_iter()
                    .filter(|s| s.start >= start && s.start <= end)
                {
                    let e = groups.entry(s.service.clone()).or_insert((0, 0.0));
                    e.0 += 1;
                    e.1 = e.1.max(s.duration_ms);
                }
            }
            let range = (end - start).max(1.0);
            let by_service =
                q.contains("by(resource.service.name)") || q.contains("by (resource.service.name)");
            let series: Vec<Value> = groups
                .into_iter()
                .map(|(svc, (n, _))| {
                    let labels = if by_service { json!([{"key": "resource.service.name", "value": {"stringValue": svc}}]) } else { json!([]) };
                    json!({"labels": labels, "samples": [{"timestampMs": format!("{}", (end * 1000.0) as i64), "value": n as f64 / range}], "promLabels": ""})
                })
                .collect();
            ok(
                json!({"series": series, "metrics": {"inspectedTraces": traces.len(), "inspectedBytes": "18302", "totalJobs": 3, "completedJobs": 3}}),
            )
        }
        "/api/echo" => text(StatusCode::OK, "text/plain; charset=utf-8", "echo"),
        "/api/status/buildinfo" => ok(
            json!({"version": "2.9.0", "revision": "3e3a0c4b1", "branch": "HEAD", "buildUser": "", "buildDate": "", "goVersion": "go1.24.4"}),
        ),
        _ => text(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "404 page not found\n",
        ),
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ───────────────────────────── Pyroscope (Connect, binary protobuf) ─────────────────────────────

fn pb_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn pb_bytes(field: u64, data: &[u8], out: &mut Vec<u8>) {
    pb_varint((field << 3) | 2, out);
    pb_varint(data.len() as u64, out);
    out.extend_from_slice(data);
}

fn pyroscope(ctx: &mut Ctx, req: &Req, uid: &str, path: &str) -> Response {
    let profiles: Vec<Value> = ctx
        .coll("pyroscope_profiles")
        .into_iter()
        .map(|(_, p)| p)
        .filter(|p| p["datasource"] == uid)
        .collect();
    let proto = |body: Vec<u8>| text(StatusCode::OK, "application/proto", body);
    match (req.method.clone(), path) {
        (Method::POST, "/querier.v1.QuerierService/ProfileTypes") => {
            let mut out = Vec::new();
            for p in &profiles {
                let id = p["profileType"]
                    .as_str()
                    .unwrap_or("process_cpu:cpu:nanoseconds:cpu:nanoseconds");
                let parts: Vec<&str> = id.split(':').collect();
                let mut msg = Vec::new();
                pb_bytes(1, id.as_bytes(), &mut msg);
                // ProfileType { ID = 1; name = 2; sample_type = 4; sample_unit = 5; period_type = 6; period_unit = 7 }
                for (field, part) in [2u64, 4, 5, 6, 7].into_iter().zip(parts.iter()) {
                    pb_bytes(field, part.as_bytes(), &mut msg);
                }
                pb_bytes(1, &msg, &mut out);
            }
            proto(out)
        }
        (Method::POST, "/querier.v1.QuerierService/LabelNames") => {
            let mut names: Vec<String> = profiles
                .iter()
                .flat_map(|p| {
                    p["labels"]
                        .as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect();
            names.extend(["__name__".to_owned(), "service_name".to_owned()]);
            names.sort();
            names.dedup();
            let mut out = Vec::new();
            for n in names {
                pb_bytes(1, n.as_bytes(), &mut out);
            }
            proto(out)
        }
        (Method::POST, "/querier.v1.QuerierService/LabelValues") => {
            let name = pb_first_string(&req.body).unwrap_or_default();
            let mut values: Vec<String> = profiles
                .iter()
                .filter_map(|p| {
                    if name == "service_name" {
                        p["service"].as_str().map(str::to_owned)
                    } else {
                        p["labels"][&name].as_str().map(str::to_owned)
                    }
                })
                .collect();
            values.sort();
            values.dedup();
            let mut out = Vec::new();
            for v in values {
                pb_bytes(1, v.as_bytes(), &mut out);
            }
            proto(out)
        }
        (Method::POST, "/querier.v1.QuerierService/SelectMergeStacktraces") => {
            // FlameGraph { names = 1; levels = 2 (Level { values = 1 packed }); total = 3; max_self = 4 }
            let mut fg = Vec::new();
            let has = !profiles.is_empty();
            let names: &[&str] = if has {
                &[
                    "total",
                    "main.(*Server).handleCheckout",
                    "payments.(*Client).Charge",
                    "net/http.(*Transport).RoundTrip",
                    "encoding/json.Marshal",
                ]
            } else {
                &["total"]
            };
            for n in names {
                pb_bytes(1, n.as_bytes(), &mut fg);
            }
            let levels: &[&[u64]] = if has {
                &[
                    &[0, 1000, 0, 0],
                    &[0, 1000, 80, 1],
                    &[0, 740, 120, 2, 0, 180, 180, 4],
                    &[0, 620, 620, 3],
                ]
            } else {
                &[&[0, 0, 0, 0]]
            };
            for lv in levels {
                let mut packed = Vec::new();
                for v in *lv {
                    pb_varint(*v, &mut packed);
                }
                let mut level = Vec::new();
                pb_bytes(1, &packed, &mut level);
                pb_bytes(2, &level, &mut fg);
            }
            pb_varint(3 << 3, &mut fg);
            pb_varint(if has { 1000 } else { 0 }, &mut fg);
            pb_varint(4 << 3, &mut fg);
            pb_varint(if has { 620 } else { 0 }, &mut fg);
            let mut out = Vec::new();
            pb_bytes(1, &fg, &mut out);
            proto(out)
        }
        (Method::POST, "/querier.v1.QuerierService/SelectSeries") => {
            // SelectSeriesResponse { series = 1 (Series { labels = 1 (LabelPair { name = 1; value = 2 }); points = 2 (Point { value = 1 double; timestamp = 2 }) }) }
            let mut out = Vec::new();
            for p in &profiles {
                let mut series = Vec::new();
                let mut lp = Vec::new();
                pb_bytes(1, b"service_name", &mut lp);
                pb_bytes(
                    2,
                    p["service"].as_str().unwrap_or_default().as_bytes(),
                    &mut lp,
                );
                pb_bytes(1, &lp, &mut series);
                for i in 0..4u64 {
                    let mut pt = Vec::new();
                    pb_varint((1 << 3) | 1, &mut pt);
                    pt.extend_from_slice(&(1.0e9_f64 * (2.0 + i as f64 * 0.1)).to_le_bytes());
                    pb_varint(2 << 3, &mut pt);
                    pb_varint(
                        ((ctx.now as u64).saturating_sub(900 - i * 300)) * 1000,
                        &mut pt,
                    );
                    pb_bytes(2, &pt, &mut series);
                }
                pb_bytes(1, &series, &mut out);
            }
            proto(out)
        }
        (Method::GET, "/pyroscope/render") => {
            let query = req.q("query").unwrap_or_default();
            let profile = profiles
                .iter()
                .find(|p| query.contains(p["service"].as_str().unwrap_or("\u{0}")));
            match req.q("format") {
                Some("dot") => {
                    let body = profile.and_then(|p| p["dot"].as_str().map(str::to_owned)).unwrap_or_else(|| "digraph \"unnamed\" {\nnode [style=filled fillcolor=\"#f8f8f8\"]\n}\n".into());
                    text(StatusCode::OK, "text/plain; charset=utf-8", body)
                }
                _ => ok(profile.map(|p| p["flamebearer"].clone()).filter(|f| !f.is_null()).unwrap_or_else(|| json!({"version": 1, "flamebearer": {"names": ["total"], "levels": [[0, 0, 0, 0]], "numTicks": 0, "maxSelf": 0}, "metadata": {"format": "single", "spyName": "", "sampleRate": 100, "units": "samples", "name": ""}, "timeline": null}))),
            }
        }
        _ => json(
            StatusCode::NOT_FOUND,
            json!({"code": "unimplemented", "message": format!("{path} is not implemented")}),
        ),
    }
}

fn pb_first_string(buf: &[u8]) -> Option<String> {
    // LabelValuesRequest { name = 1 }
    if buf.len() < 2 || buf[0] != 0x0a {
        return None;
    }
    let len = buf[1] as usize;
    buf.get(2..2 + len)
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

// ───────────────────────────── /api/ds/query ─────────────────────────────

pub(crate) fn ds_query(ctx: &mut Ctx, req: &Req) -> Response {
    if req.method != Method::POST {
        return not_found("Not found");
    }
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let to = body["to"]
        .as_str()
        .and_then(|t| relative_or_ms(t, ctx.now))
        .unwrap_or(ctx.now);
    let from = body["from"]
        .as_str()
        .and_then(|t| relative_or_ms(t, ctx.now))
        .unwrap_or(to - 3600.0);
    let mut results = Map::new();
    for q in body["queries"].as_array().cloned().unwrap_or_default() {
        let ref_id = q["refId"].as_str().unwrap_or("A").to_owned();
        let uid = q["datasource"]["uid"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let Some(ds) = ctx.get("datasources", &uid) else {
            results.insert(ref_id, json!({"error": "data source not found", "errorSource": "downstream", "status": 400}));
            continue;
        };
        let typ = ds["type"].as_str().unwrap_or_default();
        let frames = match typ {
            "prometheus" => {
                let expr = q["expr"].as_str().unwrap_or_default();
                let series = prom_series(ctx, &uid);
                match promql::parse(expr).and_then(|e| promql::eval(&e, &promql::Ctx { series: &series, time: to })) {
                    Ok(promql::Value::Vector(v)) => v
                        .into_iter()
                        .map(|(l, x)| {
                            let n = 12;
                            let step = ((to - from) / n as f64).max(1.0);
                            let times: Vec<i64> = (0..=n).map(|i| ((from + step * i as f64) * 1000.0) as i64).collect();
                            json!({
                                "schema": {"refId": ref_id, "meta": {"type": "timeseries-multi", "typeVersion": [0, 1], "executedQueryString": format!("Expr: {expr}\nStep: {step}s")},
                                    "fields": [{"name": "Time", "type": "time", "typeInfo": {"frame": "time.Time"}, "config": {"interval": (step * 1000.0) as i64}},
                                               {"name": "Value", "type": "number", "typeInfo": {"frame": "float64"}, "labels": labels_json(&l), "config": {"displayNameFromDS": l.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect::<Vec<_>>().join(", ")}}]},
                                "data": {"values": [times.clone(), times.iter().map(|_| x).collect::<Vec<_>>()]},
                            })
                        })
                        .collect::<Vec<_>>(),
                    Ok(promql::Value::Scalar(x)) => vec![json!({"schema": {"refId": ref_id, "fields": [{"name": "Time", "type": "time"}, {"name": "Value", "type": "number"}]}, "data": {"values": [[(to * 1000.0) as i64], [x]]}})],
                    Ok(_) => vec![],
                    Err(e) => {
                        results.insert(ref_id, json!({"error": format!("bad_data: invalid parameter \"query\": {e}"), "errorSource": "downstream", "status": 400}));
                        continue;
                    }
                }
            }
            "loki" => {
                let expr = q["expr"].as_str().unwrap_or_default();
                let offset = shift(ctx, Some(to));
                let streams = loki_streams(ctx, &uid, offset);
                match logql::parse(expr) {
                    Ok(logql::Query::Log(lq)) => {
                        let mut ts = Vec::new();
                        let mut lines = Vec::new();
                        let mut labels = Vec::new();
                        for s in streams.iter().filter(|s| lq.selects(&s.labels)) {
                            for e in s.entries.iter().filter(|e| e.ts >= from && e.ts < to) {
                                if lq.pipeline(&s.labels, &e.line).is_some() {
                                    ts.push((e.ts * 1000.0) as i64);
                                    lines.push(e.line.clone());
                                    labels.push(labels_json(&s.labels));
                                }
                            }
                        }
                        vec![json!({"schema": {"refId": ref_id, "meta": {"type": "log-lines", "typeVersion": [0, 0]}, "fields": [{"name": "labels", "type": "other", "typeInfo": {"frame": "json.RawMessage"}}, {"name": "Time", "type": "time", "typeInfo": {"frame": "time.Time"}}, {"name": "Line", "type": "string", "typeInfo": {"frame": "string"}}]}, "data": {"values": [labels, ts, lines]}})]
                    }
                    Ok(logql::Query::Metric(mq)) => logql::eval_metric(&mq, &streams, to)
                        .into_iter()
                        .map(|(l, x)| json!({"schema": {"refId": ref_id, "fields": [{"name": "Time", "type": "time"}, {"name": "Value", "type": "number", "labels": labels_json(&l)}]}, "data": {"values": [[(to * 1000.0) as i64], [x]]}}))
                        .collect(),
                    Err(e) => {
                        results.insert(ref_id, json!({"error": e, "errorSource": "downstream", "status": 400}));
                        continue;
                    }
                }
            }
            _ => vec![],
        };
        results.insert(ref_id, json!({"status": 200, "frames": frames}));
    }
    ok(json!({"results": results}))
}

/// `now`, `now-1h`, epoch milliseconds.
fn relative_or_ms(s: &str, now: f64) -> Option<f64> {
    if let Ok(ms) = s.parse::<f64>() {
        return Some(ms / 1000.0);
    }
    let rest = s.strip_prefix("now")?;
    if rest.is_empty() {
        return Some(now);
    }
    let (sign, d) = rest.split_at(1);
    let secs = parse_duration(d)?;
    Some(if sign == "-" { now - secs } else { now + secs })
}
