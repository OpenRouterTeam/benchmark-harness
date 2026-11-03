//! Aggregations: `terms`, `avg`, `sum`, `min`, `max`, `value_count`, `cardinality`,
//! `stats`, `percentiles`, `date_histogram`, `histogram`, `range`, `filter`, `filters`,
//! `missing` and `top_hits`, with sub-aggregations on bucket aggregations.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{
    dsl::{Ctx, eval},
    error::EsError,
    model::{
        Doc, as_f64, field_values, format_millis, interval_millis, is_integer, is_numeric, num,
        parse_date_millis, scalar_string,
    },
};

const BUCKET_KINDS: [&str; 7] = [
    "terms",
    "date_histogram",
    "histogram",
    "range",
    "filter",
    "filters",
    "missing",
];

pub fn run(aggs: &Value, docs: &[&Doc], ctx: &Ctx) -> Result<Map<String, Value>, EsError> {
    let obj = aggs
        .as_object()
        .ok_or_else(|| EsError::parsing("Expected [START_OBJECT] under [aggs]"))?;
    let mut out = Map::new();
    for (name, spec) in obj {
        out.insert(name.clone(), one(name, spec, docs, ctx)?);
    }
    Ok(out)
}

fn one(name: &str, spec: &Value, docs: &[&Doc], ctx: &Ctx) -> Result<Value, EsError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| EsError::parsing(format!("Expected [START_OBJECT] under [{name}]")))?;
    let sub = obj.get("aggs").or_else(|| obj.get("aggregations"));
    let kinds: Vec<(&String, &Value)> = obj
        .iter()
        .filter(|(k, _)| *k != "aggs" && *k != "aggregations" && *k != "meta")
        .collect();
    let Some((kind, body)) = kinds.first() else {
        return Err(EsError::parsing(format!(
            "Missing definition for aggregation [{name}]"
        )));
    };
    if kinds.len() > 1 {
        return Err(EsError::parsing(format!(
            "Found two aggregation type definitions in [{name}]: [{}] and [{}]",
            kinds[0].0, kinds[1].0
        )));
    }
    if sub.is_some() && !BUCKET_KINDS.contains(&kind.as_str()) {
        return Err(EsError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "aggregation_initialization_exception",
            format!("Aggregator [{name}] of type [{kind}] cannot accept sub-aggregations"),
        ));
    }
    let field = body.get("field").and_then(Value::as_str);
    let needs_field = |kind: &str| -> Result<&str, EsError> {
        field.ok_or_else(|| {
            EsError::illegal(format!(
                "Required one of fields [field, script], but none were specified. ({kind} [{name}])"
            ))
        })
    };
    let with_sub = |mut bucket: Map<String, Value>, members: &[&Doc]| -> Result<Value, EsError> {
        if let Some(s) = sub {
            for (k, v) in run(s, members, ctx)? {
                bucket.insert(k, v);
            }
        }
        Ok(Value::Object(bucket))
    };

    match kind.as_str() {
        "avg" | "sum" | "min" | "max" => {
            let f = needs_field(kind)?;
            check_numeric(f, kind, ctx)?;
            let vals = numbers(docs, f, ctx);
            let v = match kind.as_str() {
                "sum" => Some(vals.iter().sum::<f64>()),
                "avg" => (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64),
                "min" => vals.iter().copied().reduce(f64::min),
                _ => vals.iter().copied().reduce(f64::max),
            };
            let mut out = Map::new();
            out.insert("value".into(), v.map_or(Value::Null, |x| json!(round(x))));
            if ctx.ty(f).as_deref() == Some("date")
                && let Some(x) = v
            {
                out.insert("value_as_string".into(), json!(format_millis(x as i64)));
            }
            Ok(Value::Object(out))
        }
        "value_count" => {
            let f = needs_field(kind)?;
            let n: usize = docs.iter().map(|d| field_values(&d.source, f).len()).sum();
            Ok(json!({ "value": n }))
        }
        "cardinality" => {
            let f = needs_field(kind)?;
            check_aggregatable(f, ctx)?;
            let set: BTreeSet<String> = docs
                .iter()
                .flat_map(|d| field_values(&d.source, f))
                .map(|v| scalar_string(&v))
                .collect();
            Ok(json!({ "value": set.len() }))
        }
        "stats" | "extended_stats" => {
            let f = needs_field(kind)?;
            check_numeric(f, kind, ctx)?;
            let vals = numbers(docs, f, ctx);
            let n = vals.len();
            let sum: f64 = vals.iter().sum();
            if n == 0 {
                return Ok(
                    json!({ "count": 0, "min": null, "max": null, "avg": null, "sum": 0.0 }),
                );
            }
            Ok(json!({
                "count": n,
                "min": round(vals.iter().copied().fold(f64::MAX, f64::min)),
                "max": round(vals.iter().copied().fold(f64::MIN, f64::max)),
                "avg": round(sum / n as f64),
                "sum": round(sum),
            }))
        }
        "percentiles" => {
            let f = needs_field(kind)?;
            check_numeric(f, kind, ctx)?;
            let mut vals = numbers(docs, f, ctx);
            vals.sort_by(f64::total_cmp);
            let percents: Vec<f64> = body
                .get("percents")
                .and_then(Value::as_array)
                .map(|p| p.iter().filter_map(Value::as_f64).collect())
                .unwrap_or_else(|| vec![1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0]);
            let mut values = Map::new();
            for p in percents {
                let v = percentile(&vals, p);
                values.insert(
                    format!("{p:.1}"),
                    v.map_or(Value::Null, |x| json!(round(x))),
                );
            }
            Ok(json!({ "values": values }))
        }
        "terms" => {
            let f = needs_field(kind)?;
            check_aggregatable(f, ctx)?;
            let size = body.get("size").and_then(Value::as_u64).unwrap_or(10) as usize;
            let min_doc_count = body
                .get("min_doc_count")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            let ty = ctx.ty(f);
            // (key, members) in first-seen order.
            let mut groups: Vec<(Value, Vec<&Doc>)> = Vec::new();
            for d in docs {
                let mut seen = BTreeSet::new();
                for v in field_values(&d.source, f) {
                    let key = term_key(&v, ty.as_deref());
                    if !seen.insert(key.to_string()) {
                        continue;
                    }
                    match groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, m)) => m.push(d),
                        None => groups.push((key, vec![d])),
                    }
                }
            }
            if let Some(missing) = body.get("missing") {
                let members: Vec<&Doc> = docs
                    .iter()
                    .filter(|d| field_values(&d.source, f).is_empty())
                    .copied()
                    .collect();
                if !members.is_empty() {
                    groups.push((missing.clone(), members));
                }
            }
            groups.retain(|(_, m)| m.len() >= min_doc_count);
            sort_terms(&mut groups, body.get("order"))?;
            let total: usize = groups.iter().map(|(_, m)| m.len()).sum();
            let mut buckets = Vec::new();
            for (key, members) in groups.iter().take(size) {
                let mut b = Map::new();
                b.insert("key".into(), key.clone());
                if ty.as_deref() == Some("boolean") {
                    b.insert(
                        "key_as_string".into(),
                        json!(if key == &json!(1) { "true" } else { "false" }),
                    );
                }
                if ty.as_deref() == Some("date")
                    && let Some(ms) = key.as_i64()
                {
                    b.insert("key_as_string".into(), json!(format_millis(ms)));
                }
                b.insert("doc_count".into(), json!(members.len()));
                buckets.push(with_sub(b, members)?);
            }
            let shown: usize = groups.iter().take(size).map(|(_, m)| m.len()).sum();
            Ok(json!({
                "doc_count_error_upper_bound": 0,
                "sum_other_doc_count": total - shown,
                "buckets": buckets,
            }))
        }
        "date_histogram" | "histogram" => {
            let f = needs_field(kind)?;
            let is_date = kind.as_str() == "date_histogram";
            let interval = if is_date {
                let spec = body
                    .get("fixed_interval")
                    .or_else(|| body.get("calendar_interval"))
                    .or_else(|| body.get("interval"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        EsError::illegal(
                            "Invalid interval specified, must be non-null and non-empty",
                        )
                    })?;
                interval_millis(spec)
                    .ok_or_else(|| EsError::illegal(format!("failed to parse setting [date_histogram.fixedInterval] with value [{spec}] as a time value: unit is missing or unrecognized")))? as f64
            } else {
                body.get("interval")
                    .and_then(Value::as_f64)
                    .filter(|i| *i > 0.0)
                    .ok_or_else(|| {
                        EsError::illegal(
                            "[interval] must be >0 for histogram aggregation [".to_owned()
                                + name
                                + "]",
                        )
                    })?
            };
            let min_doc_count = body
                .get("min_doc_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let mut keyed: Vec<(f64, Vec<&Doc>)> = Vec::new();
            for d in docs {
                let Some(v) = field_values(&d.source, f).into_iter().next() else {
                    continue;
                };
                let x = if is_date {
                    parse_date_millis(&v).map(|m| m as f64)
                } else {
                    as_f64(&v)
                };
                let Some(x) = x else { continue };
                let key = (x / interval).floor() * interval;
                match keyed.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, m)) => m.push(d),
                    None => keyed.push((key, vec![d])),
                }
            }
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Fill empty buckets between the first and last (min_doc_count 0).
            let mut filled: Vec<(f64, Vec<&Doc>)> = Vec::new();
            if let (Some(first), Some(last)) =
                (keyed.first().map(|k| k.0), keyed.last().map(|k| k.0))
            {
                let mut k = first;
                while k <= last {
                    let members = keyed
                        .iter()
                        .find(|(x, _)| *x == k)
                        .map(|(_, m)| m.clone())
                        .unwrap_or_default();
                    filled.push((k, members));
                    k += interval;
                }
            }
            let mut buckets = Vec::new();
            for (key, members) in filled.iter().filter(|(_, m)| m.len() >= min_doc_count) {
                let mut b = Map::new();
                if is_date {
                    b.insert("key_as_string".into(), json!(format_millis(*key as i64)));
                    b.insert("key".into(), json!(*key as i64));
                } else {
                    b.insert("key".into(), json!(key));
                }
                b.insert("doc_count".into(), json!(members.len()));
                buckets.push(with_sub(b, members)?);
            }
            Ok(json!({ "buckets": buckets }))
        }
        "range" => {
            let f = needs_field(kind)?;
            let ranges = body
                .get("ranges")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    EsError::illegal(format!(
                        "No [ranges] specified for the [{name}] aggregation"
                    ))
                })?;
            let mut buckets = Vec::new();
            for r in ranges {
                let from = r.get("from").and_then(as_f64_opt);
                let to = r.get("to").and_then(as_f64_opt);
                let members: Vec<&Doc> = docs
                    .iter()
                    .filter(|d| {
                        field_values(&d.source, f)
                            .iter()
                            .filter_map(as_f64)
                            .any(|x| from.is_none_or(|lo| x >= lo) && to.is_none_or(|hi| x < hi))
                    })
                    .copied()
                    .collect();
                let key = r
                    .get("key")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "{}-{}",
                            from.map_or("*".to_owned(), |v| format!("{v:.1}")),
                            to.map_or("*".to_owned(), |v| format!("{v:.1}"))
                        )
                    });
                let mut b = Map::new();
                b.insert("key".into(), json!(key));
                if let Some(v) = from {
                    b.insert("from".into(), json!(v));
                }
                if let Some(v) = to {
                    b.insert("to".into(), json!(v));
                }
                b.insert("doc_count".into(), json!(members.len()));
                buckets.push(with_sub(b, &members)?);
            }
            Ok(json!({ "buckets": buckets }))
        }
        "filter" => {
            let members = filter_docs(body, docs, ctx)?;
            let mut b = Map::new();
            b.insert("doc_count".into(), json!(members.len()));
            with_sub(b, &members)
        }
        "filters" => {
            let filters = body.get("filters").ok_or_else(|| {
                EsError::parsing(format!("[filters] cannot be empty in [{name}]"))
            })?;
            match filters {
                Value::Object(named) => {
                    let mut buckets = Map::new();
                    for (k, q) in named {
                        let members = filter_docs(q, docs, ctx)?;
                        let mut b = Map::new();
                        b.insert("doc_count".into(), json!(members.len()));
                        buckets.insert(k.clone(), with_sub(b, &members)?);
                    }
                    Ok(json!({ "buckets": buckets }))
                }
                Value::Array(list) => {
                    let mut buckets = Vec::new();
                    for q in list {
                        let members = filter_docs(q, docs, ctx)?;
                        let mut b = Map::new();
                        b.insert("doc_count".into(), json!(members.len()));
                        buckets.push(with_sub(b, &members)?);
                    }
                    Ok(json!({ "buckets": buckets }))
                }
                _ => Err(EsError::parsing("[filters] must be an object or array")),
            }
        }
        "missing" => {
            let f = needs_field(kind)?;
            let members: Vec<&Doc> = docs
                .iter()
                .filter(|d| field_values(&d.source, f).is_empty())
                .copied()
                .collect();
            let mut b = Map::new();
            b.insert("doc_count".into(), json!(members.len()));
            with_sub(b, &members)
        }
        "top_hits" => {
            let size = body.get("size").and_then(Value::as_u64).unwrap_or(3) as usize;
            let mut members: Vec<&Doc> = docs.to_vec();
            if let Some(sort) = body.get("sort") {
                let scored: Vec<(&Doc, f64)> = members.iter().map(|d| (*d, 1.0)).collect();
                members = crate::search::sort_docs(scored, sort, ctx)?
                    .into_iter()
                    .map(|(d, _, _)| d)
                    .collect();
            }
            let hits: Vec<Value> = members
                .iter()
                .take(size)
                .map(|d| json!({ "_index": d.index, "_id": d.id, "_score": 1.0, "_source": crate::search::filter_source(&d.source, body.get("_source")) }))
                .collect();
            Ok(
                json!({ "hits": { "total": { "value": docs.len(), "relation": "eq" }, "max_score": 1.0, "hits": hits } }),
            )
        }
        other => Err(EsError::parsing(format!(
            "Unknown aggregation type [{other}] did you mean [terms]?"
        ))),
    }
}

fn as_f64_opt(v: &Value) -> Option<f64> {
    as_f64(v)
}

fn filter_docs<'a>(q: &Value, docs: &[&'a Doc], ctx: &Ctx) -> Result<Vec<&'a Doc>, EsError> {
    let mut out = Vec::new();
    for d in docs {
        if eval(q, d, ctx)?.is_some() {
            out.push(*d);
        }
    }
    Ok(out)
}

fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn numbers(docs: &[&Doc], field: &str, ctx: &Ctx) -> Vec<f64> {
    let is_date = ctx.ty(field).as_deref() == Some("date");
    docs.iter()
        .flat_map(|d| field_values(&d.source, field))
        .filter_map(|v| {
            if is_date {
                parse_date_millis(&v).map(|m| m as f64)
            } else {
                as_f64(&v)
            }
        })
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64))
}

fn term_key(v: &Value, ty: Option<&str>) -> Value {
    match ty {
        Some("boolean") => json!(if matches!(v, Value::Bool(true)) || v == "true" {
            1
        } else {
            0
        }),
        Some("date") => parse_date_millis(v).map_or(Value::Null, |m| json!(m)),
        Some(t) if is_integer(t) => as_f64(v).map_or(Value::Null, |f| json!(f as i64)),
        Some(t) if is_numeric(t) => as_f64(v).map_or(Value::Null, |f| json!(f)),
        _ => match v {
            Value::Number(n) => num(n.as_f64().unwrap_or_default()),
            other => Value::String(scalar_string(other)),
        },
    }
}

fn sort_terms(groups: &mut [(Value, Vec<&Doc>)], order: Option<&Value>) -> Result<(), EsError> {
    let (by, dir) = match order {
        None => ("_count".to_owned(), "desc".to_owned()),
        Some(Value::Object(o)) => {
            let (k, v) = o
                .iter()
                .next()
                .ok_or_else(|| EsError::parsing("[order] must not be empty"))?;
            (k.clone(), v.as_str().unwrap_or("desc").to_lowercase())
        }
        Some(Value::Array(a)) => {
            let first = a.first().and_then(Value::as_object);
            match first.and_then(|o| o.iter().next()) {
                Some((k, v)) => (k.clone(), v.as_str().unwrap_or("desc").to_lowercase()),
                None => ("_count".to_owned(), "desc".to_owned()),
            }
        }
        Some(_) => return Err(EsError::parsing("[order] must be an object")),
    };
    let key_cmp = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        _ => scalar_string(a).cmp(&scalar_string(b)),
    };
    match by.as_str() {
        "_count" => groups.sort_by(|a, b| {
            let c = a.1.len().cmp(&b.1.len());
            let c = if dir == "asc" { c } else { c.reverse() };
            c.then_with(|| key_cmp(&a.0, &b.0))
        }),
        "_key" | "_term" => groups.sort_by(|a, b| {
            let c = key_cmp(&a.0, &b.0);
            if dir == "asc" { c } else { c.reverse() }
        }),
        other => {
            return Err(EsError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "aggregation_execution_exception",
                format!("Invalid aggregator order path [{other}]. Unknown aggregation [{other}]"),
            ));
        }
    }
    Ok(())
}

fn check_aggregatable(field: &str, ctx: &Ctx) -> Result<(), EsError> {
    if ctx.ty(field).as_deref() == Some("text") {
        let index = ctx
            .indices
            .first()
            .map(|i| i.name.as_str())
            .unwrap_or_default();
        return Err(EsError::illegal(format!(
            "Fielddata is disabled on [{field}] in [{index}]. Text fields are not optimised for operations that require per-document field data like aggregations and sorting, so these operations are disabled by default. Please use a keyword field instead. Alternatively, set fielddata=true on [{field}] in order to load field data by uninverting the inverted index. Note that this can use significant memory."
        )));
    }
    Ok(())
}

fn check_numeric(field: &str, kind: &str, ctx: &Ctx) -> Result<(), EsError> {
    match ctx.ty(field).as_deref() {
        Some(t) if is_numeric(t) || t == "date" || t == "boolean" => Ok(()),
        None => Ok(()),
        Some(t) => Err(EsError::illegal(format!(
            "Field [{field}] of type [{t}] is not supported for aggregation [{kind}]"
        ))),
    }
}
