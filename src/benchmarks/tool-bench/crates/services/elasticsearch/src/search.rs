//! `_search` and `_count`: request validation, scoring, sorting, paging, `_source`
//! filtering, `fields`, `track_total_hits` and aggregations.

use std::cmp::Ordering;

use serde_json::{Map, Value, json};

use crate::{
    aggs,
    dsl::{Ctx, eval},
    error::EsError,
    model::{Doc, field_values, is_numeric, parse_date_millis, scalar_string, wildcard_match},
};

const KNOWN_KEYS: [&str; 22] = [
    "query",
    "size",
    "from",
    "sort",
    "_source",
    "aggs",
    "aggregations",
    "track_total_hits",
    "track_scores",
    "fields",
    "docvalue_fields",
    "stored_fields",
    "highlight",
    "timeout",
    "post_filter",
    "min_score",
    "explain",
    "version",
    "seq_no_primary_term",
    "terminate_after",
    "search_after",
    "runtime_mappings",
];

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Object(_) => "START_OBJECT",
        Value::Array(_) => "START_ARRAY",
        Value::String(_) => "VALUE_STRING",
        Value::Number(_) => "VALUE_NUMBER",
        Value::Bool(_) => "VALUE_BOOLEAN",
        Value::Null => "VALUE_NULL",
    }
}

fn matching<'a>(query: Option<&Value>, ctx: &Ctx<'a>) -> Result<Vec<(&'a Doc, f64)>, EsError> {
    let empty = Doc {
        index: String::new(),
        id: String::new(),
        source: json!({}),
    };
    if let Some(q) = query {
        eval(q, &empty, ctx)?;
    }
    let mut out = Vec::new();
    for doc in &ctx.corpus {
        let hit = match query {
            Some(q) => eval(q, doc, ctx)?,
            None => Some(1.0),
        };
        if let Some(score) = hit {
            out.push((*doc, score));
        }
    }
    Ok(out)
}

pub fn count(body: &Map<String, Value>, ctx: &Ctx) -> Result<Value, EsError> {
    for key in body.keys() {
        if key != "query" {
            return Err(EsError::parsing(format!(
                "request does not support [{key}]"
            )));
        }
    }
    let n = matching(body.get("query"), ctx)?.len();
    Ok(json!({ "count": n, "_shards": shards(ctx.indices.len()) }))
}

fn shards(n: usize) -> Value {
    json!({ "total": n, "successful": n, "skipped": 0, "failed": 0 })
}

pub fn search(body: &Map<String, Value>, ctx: &Ctx) -> Result<Value, EsError> {
    for (key, value) in body {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(EsError::parsing(format!(
                "Unknown key for a {} in [{key}].",
                kind_name(value)
            )));
        }
    }
    let int = |key: &str, default: i64| -> Result<i64, EsError> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v
                .as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| {
                    EsError::parsing(format!("[{key}] failed to parse field, expected a number"))
                }),
        }
    };
    let size = int("size", 10)?;
    let from = int("from", 0)?;
    if size < 0 {
        return Err(EsError::illegal(format!(
            "[size] parameter cannot be negative, found [{size}]"
        )));
    }
    if from < 0 {
        return Err(EsError::illegal(format!(
            "[from] parameter cannot be negative but was [{from}]"
        )));
    }
    if from + size > 10_000 {
        return Err(EsError::illegal(format!(
            "Result window is too large, from + size must be less than or equal to: [10000] but was [{}]. See the scroll api for a more efficient way to request large data sets. This limit can be set by changing the [index.max_result_window] index level setting.",
            from + size
        )));
    }

    let mut matched = matching(body.get("query"), ctx)?;
    if let Some(min) = body.get("min_score").and_then(Value::as_f64) {
        matched.retain(|(_, s)| *s >= min);
    }

    let aggregations = match body.get("aggs").or_else(|| body.get("aggregations")) {
        Some(a) => {
            let docs: Vec<&Doc> = matched.iter().map(|(d, _)| *d).collect();
            Some(aggs::run(a, &docs, ctx)?)
        }
        None => None,
    };

    if let Some(pf) = body.get("post_filter") {
        let mut kept = Vec::new();
        for (d, s) in matched {
            if eval(pf, d, ctx)?.is_some() {
                kept.push((d, s));
            }
        }
        matched = kept;
    }

    let sorted: Vec<(&Doc, f64, Option<Vec<Value>>)> = match body.get("sort") {
        Some(sort) => sort_docs(matched, sort, ctx)?
            .into_iter()
            .map(|(d, s, v)| (d, s, Some(v)))
            .collect(),
        None => {
            let mut m = matched;
            m.sort_by(|a, b| b.1.total_cmp(&a.1));
            m.into_iter().map(|(d, s)| (d, s, None)).collect()
        }
    };
    let total = sorted.len();
    let by_field = body.get("sort").is_some_and(|s| !sorts_by_score_only(s));
    let track_scores = body
        .get("track_scores")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let page: Vec<&(&Doc, f64, Option<Vec<Value>>)> = sorted
        .iter()
        .skip(from as usize)
        .take(size as usize)
        .collect();
    let fields: Vec<String> = body
        .get("fields")
        .and_then(Value::as_array)
        .map(|f| {
            f.iter()
                .filter_map(|x| {
                    x.as_str()
                        .map(str::to_owned)
                        .or_else(|| x["field"].as_str().map(str::to_owned))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut hits = Vec::new();
    for (doc, score, sort_values) in &page {
        let mut h = Map::new();
        h.insert("_index".into(), json!(doc.index));
        h.insert("_id".into(), json!(doc.id));
        h.insert(
            "_score".into(),
            if by_field && !track_scores {
                Value::Null
            } else {
                json!(score)
            },
        );
        if body.get("_source") != Some(&Value::Bool(false)) {
            h.insert(
                "_source".into(),
                filter_source(&doc.source, body.get("_source")),
            );
        }
        if !fields.is_empty() {
            let mut fm = Map::new();
            for pattern in &fields {
                let leaves: Vec<(String, String)> = ctx
                    .indices
                    .iter()
                    .find(|i| i.name == doc.index)
                    .map(|i| i.leaf_fields())
                    .unwrap_or_default();
                for (leaf, _) in leaves.iter().filter(|(l, _)| wildcard_match(pattern, l)) {
                    let vals = field_values(&doc.source, leaf);
                    if !vals.is_empty() {
                        fm.insert(leaf.clone(), Value::Array(vals));
                    }
                }
            }
            h.insert("fields".into(), Value::Object(fm));
        }
        if let Some(v) = sort_values {
            h.insert("sort".into(), Value::Array(v.clone()));
        }
        hits.push(Value::Object(h));
    }

    let max_score = if by_field && !track_scores {
        Value::Null
    } else {
        sorted
            .iter()
            .map(|(_, s, _)| *s)
            .reduce(f64::max)
            .map_or(Value::Null, |m| json!(m))
    };
    let mut hits_obj = Map::new();
    match body.get("track_total_hits") {
        Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            hits_obj.insert("total".into(), json!({ "value": total, "relation": "eq" }));
        }
        Some(Value::Number(n)) => {
            let cap = n.as_u64().unwrap_or(10_000) as usize;
            let (value, rel) = if total > cap {
                (cap, "gte")
            } else {
                (total, "eq")
            };
            hits_obj.insert("total".into(), json!({ "value": value, "relation": rel }));
        }
        _ => {
            let (value, rel) = if total > 10_000 {
                (10_000, "gte")
            } else {
                (total, "eq")
            };
            hits_obj.insert("total".into(), json!({ "value": value, "relation": rel }));
        }
    }
    hits_obj.insert("max_score".into(), max_score);
    hits_obj.insert("hits".into(), Value::Array(hits));

    let mut out = Map::new();
    out.insert("took".into(), json!(2 + ctx.corpus.len() / 40));
    out.insert("timed_out".into(), json!(false));
    out.insert("_shards".into(), shards(ctx.indices.len()));
    out.insert("hits".into(), Value::Object(hits_obj));
    if let Some(a) = aggregations {
        out.insert("aggregations".into(), Value::Object(a));
    }
    Ok(Value::Object(out))
}

fn sorts_by_score_only(sort: &Value) -> bool {
    let specs = match sort {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    specs.iter().all(|s| match s {
        Value::String(f) => f == "_score",
        Value::Object(o) => o.keys().all(|k| k == "_score"),
        _ => false,
    })
}

struct SortKey {
    field: String,
    desc: bool,
    missing_first: bool,
    unmapped_ok: bool,
}

fn parse_sort(sort: &Value) -> Result<Vec<SortKey>, EsError> {
    let specs = match sort {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let mut out = Vec::new();
    for s in specs {
        match s {
            Value::String(f) => {
                let (field, desc) = match f.split_once(':') {
                    Some((f, d)) => (f.to_owned(), d == "desc"),
                    None => (f.clone(), f == "_score"),
                };
                out.push(SortKey {
                    field,
                    desc,
                    missing_first: false,
                    unmapped_ok: false,
                });
            }
            Value::Object(o) => {
                for (field, spec) in o {
                    let (order, missing, unmapped) = match &spec {
                        Value::String(d) => (Some(d.as_str()), None, false),
                        Value::Object(so) => (
                            so.get("order").and_then(Value::as_str),
                            so.get("missing").and_then(Value::as_str),
                            so.contains_key("unmapped_type"),
                        ),
                        _ => {
                            return Err(EsError::parsing(format!(
                                "[sort] malformed for field [{field}]"
                            )));
                        }
                    };
                    let desc = match order {
                        Some(o) if o.eq_ignore_ascii_case("desc") => true,
                        Some(o) if o.eq_ignore_ascii_case("asc") => false,
                        Some(o) => {
                            return Err(EsError::illegal(format!("Unknown SortOrder [{o}]")));
                        }
                        None => field == "_score",
                    };
                    out.push(SortKey {
                        field: field.clone(),
                        desc,
                        missing_first: missing == Some("_first"),
                        unmapped_ok: unmapped,
                    });
                }
            }
            _ => return Err(EsError::parsing("[sort] must be a string, object or array")),
        }
    }
    Ok(out)
}

fn sort_value(doc: &Doc, score: f64, key: &SortKey, ty: Option<&str>) -> Value {
    match key.field.as_str() {
        "_score" => json!(score),
        "_doc" => Value::Null,
        "_id" => json!(doc.id),
        f => {
            let vals = field_values(&doc.source, f);
            let v = if key.desc {
                vals.iter().max_by(|a, b| cmp_values(a, b, ty))
            } else {
                vals.iter().min_by(|a, b| cmp_values(a, b, ty))
            };
            match (v, ty) {
                (Some(v), Some("date")) => parse_date_millis(v).map_or(Value::Null, |m| json!(m)),
                (Some(v), _) => v.clone(),
                (None, _) => Value::Null,
            }
        }
    }
}

fn cmp_values(a: &Value, b: &Value, ty: Option<&str>) -> Ordering {
    match ty {
        Some("date") => parse_date_millis(a).cmp(&parse_date_millis(b)),
        Some(t) if is_numeric(t) => a
            .as_f64()
            .unwrap_or(f64::NAN)
            .total_cmp(&b.as_f64().unwrap_or(f64::NAN)),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            _ => scalar_string(a).cmp(&scalar_string(b)),
        },
    }
}

/// A hit with its score and `sort` values.
pub type SortedHit<'a> = (&'a Doc, f64, Vec<Value>);

/// Sorts `(doc, score)` by a sort spec, returning each hit's `sort` values.
pub fn sort_docs<'a>(
    docs: Vec<(&'a Doc, f64)>,
    sort: &Value,
    ctx: &Ctx,
) -> Result<Vec<SortedHit<'a>>, EsError> {
    let keys = parse_sort(sort)?;
    let mut types = Vec::new();
    for k in &keys {
        if k.field.starts_with('_') {
            types.push(None);
            continue;
        }
        let ty = ctx.ty(&k.field);
        match ty.as_deref() {
            None if !k.unmapped_ok => {
                return Err(EsError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "query_shard_exception",
                    format!("No mapping found for [{}] in order to sort on", k.field),
                )
                .with(
                    "index",
                    json!(
                        ctx.indices
                            .first()
                            .map(|i| i.name.clone())
                            .unwrap_or_default()
                    ),
                ));
            }
            Some("text") => {
                let index = ctx
                    .indices
                    .first()
                    .map(|i| i.name.as_str())
                    .unwrap_or_default();
                return Err(EsError::illegal(format!(
                    "Fielddata is disabled on [{}] in [{index}]. Text fields are not optimised for operations that require per-document field data like aggregations and sorting, so these operations are disabled by default. Please use a keyword field instead. Alternatively, set fielddata=true on [{}] in order to load field data by uninverting the inverted index. Note that this can use significant memory.",
                    k.field, k.field
                )));
            }
            _ => types.push(ty),
        }
    }
    let mut rows: Vec<SortedHit<'a>> = docs
        .into_iter()
        .map(|(d, s)| {
            let vals = keys
                .iter()
                .zip(&types)
                .map(|(k, t)| sort_value(d, s, k, t.as_deref()))
                .collect();
            (d, s, vals)
        })
        .collect();
    rows.sort_by(|a, b| {
        for (i, k) in keys.iter().enumerate() {
            let (x, y) = (&a.2[i], &b.2[i]);
            let ord = match (x.is_null(), y.is_null()) {
                (true, true) => Ordering::Equal,
                (true, false) => {
                    if k.missing_first {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (false, true) => {
                    if k.missing_first {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                _ => {
                    let o = cmp_values(x, y, types[i].as_deref());
                    if k.desc { o.reverse() } else { o }
                }
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    Ok(rows)
}

/// `_source` filtering: `true`/`false`, a pattern, a list of patterns, or
/// `{includes, excludes}` (wildcards allowed, matched against dotted paths).
pub fn filter_source(source: &Value, spec: Option<&Value>) -> Value {
    let (includes, excludes): (Vec<String>, Vec<String>) = match spec {
        None | Some(Value::Bool(true)) | Some(Value::Null) => return source.clone(),
        Some(Value::Bool(false)) => return json!({}),
        Some(Value::String(s)) => (vec![s.clone()], Vec::new()),
        Some(Value::Array(a)) => (
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Vec::new(),
        ),
        Some(Value::Object(o)) => {
            let list = |k: &str, alt: &str| -> Vec<String> {
                match o.get(k).or_else(|| o.get(alt)) {
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    Some(Value::String(s)) => vec![s.clone()],
                    _ => Vec::new(),
                }
            };
            (list("includes", "include"), list("excludes", "exclude"))
        }
        Some(_) => return source.clone(),
    };
    filter_object(source, "", &includes, &excludes)
}

fn filter_object(v: &Value, prefix: &str, includes: &[String], excludes: &[String]) -> Value {
    let Value::Object(map) = v else {
        return v.clone();
    };
    let mut out = Map::new();
    for (k, child) in map {
        let path = format!("{prefix}{k}");
        if excludes.iter().any(|p| wildcard_match(p, &path)) {
            continue;
        }
        let included = includes.is_empty()
            || includes
                .iter()
                .any(|p| wildcard_match(p, &path) || wildcard_match(&format!("{p}.*"), &path));
        if included {
            out.insert(k.clone(), child.clone());
            continue;
        }
        let descend = includes
            .iter()
            .any(|p| p.starts_with(&format!("{path}.")) || p.contains('*'));
        if descend && child.is_object() {
            let sub = filter_object(child, &format!("{path}."), includes, excludes);
            if sub.as_object().is_some_and(|m| !m.is_empty()) {
                out.insert(k.clone(), sub);
            }
        }
    }
    Value::Object(out)
}
