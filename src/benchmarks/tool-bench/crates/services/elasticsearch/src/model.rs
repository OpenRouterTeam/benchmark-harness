//! Indices, documents, field types, values and dates shared by the query DSL,
//! aggregations and ES|QL.

use std::cmp::Ordering;

use mock_core::clock::format_unix;
use serde_json::{Map, Value, json};

use crate::error::EsError;

/// One index from the `indices` collection plus its documents (dataset order).
#[derive(Debug, Clone)]
pub struct Index {
    pub name: String,
    pub meta: Value,
    pub docs: Vec<Doc>,
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub index: String,
    pub id: String,
    pub source: Value,
}

impl Index {
    pub fn properties(&self) -> &Map<String, Value> {
        static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
        self.meta["mappings"]["properties"]
            .as_object()
            .unwrap_or_else(|| EMPTY.get_or_init(Map::new))
    }

    /// The mapped type of `field` (`a.b` may be an object path or a multi-field such as
    /// `message.keyword`).
    pub fn field_type(&self, field: &str) -> Option<String> {
        field_type_in(self.properties(), field)
    }

    /// Every leaf field path with its type, in mapping order (multi-fields included).
    pub fn leaf_fields(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        collect_fields(self.properties(), "", &mut out);
        out
    }
}

fn field_type_in(props: &Map<String, Value>, field: &str) -> Option<String> {
    if let Some(p) = props.get(field) {
        return Some(p["type"].as_str().unwrap_or("object").to_owned());
    }
    let (head, rest) = field.split_once('.')?;
    let p = props.get(head)?;
    if let Some(sub) = p["properties"].as_object() {
        return field_type_in(sub, rest);
    }
    p["fields"][rest]["type"].as_str().map(str::to_owned)
}

fn collect_fields(props: &Map<String, Value>, prefix: &str, out: &mut Vec<(String, String)>) {
    for (name, p) in props {
        let path = format!("{prefix}{name}");
        if let Some(sub) = p["properties"].as_object() {
            collect_fields(sub, &format!("{path}."), out);
            continue;
        }
        out.push((
            path.clone(),
            p["type"].as_str().unwrap_or("object").to_owned(),
        ));
        if let Some(fields) = p["fields"].as_object() {
            for (sub, sp) in fields {
                out.push((
                    format!("{path}.{sub}"),
                    sp["type"].as_str().unwrap_or("keyword").to_owned(),
                ));
            }
        }
    }
}

/// The type of `field` across `indices` (first index that maps it).
pub fn type_across(indices: &[&Index], field: &str) -> Option<String> {
    indices.iter().find_map(|i| i.field_type(field))
}

pub fn is_numeric(ty: &str) -> bool {
    matches!(
        ty,
        "long"
            | "integer"
            | "short"
            | "byte"
            | "double"
            | "float"
            | "half_float"
            | "scaled_float"
            | "unsigned_long"
    )
}

pub fn is_integer(ty: &str) -> bool {
    matches!(ty, "long" | "integer" | "short" | "byte" | "unsigned_long")
}

/// Values of `path` in `source`: a flat dotted key, a nested object path, or a
/// multi-field (`message.keyword` reads `message`). Arrays are flattened.
pub fn field_values(source: &Value, path: &str) -> Vec<Value> {
    let mut out = Vec::new();
    collect_values(source, path, &mut out);
    if out.is_empty()
        && let Some((base, _)) = path.rsplit_once('.')
    {
        collect_values(source, base, &mut out);
    }
    out
}

fn collect_values(source: &Value, path: &str, out: &mut Vec<Value>) {
    match source {
        Value::Object(map) => {
            if let Some(v) = map.get(path) {
                push_flat(v, out);
                return;
            }
            let mut split = path.splitn(2, '.');
            let head = split.next().unwrap_or_default();
            if let (Some(rest), Some(child)) = (split.next(), map.get(head)) {
                collect_values(child, rest, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_values(item, path, out);
            }
        }
        _ => {}
    }
}

fn push_flat(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| push_flat(i, out)),
        Value::Null => {}
        other => out.push(other.clone()),
    }
}

pub fn first_value(source: &Value, path: &str) -> Option<Value> {
    field_values(source, path).into_iter().next()
}

// ─────────────────────────────── dates ───────────────────────────────

/// Milliseconds since the epoch of an ISO 8601 date/time or epoch-millis number/string.
pub fn parse_date_millis(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => parse_date_str(s),
        _ => None,
    }
}

pub fn parse_date_str(s: &str) -> Option<i64> {
    let s = s.trim();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && s.len() > 8 {
        return s.parse().ok();
    }
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let mo: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut ms = days_from_civil(y, mo, d) * 86_400_000;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(ms);
    }
    let rest = rest.strip_prefix(['T', ' '])?;
    let rb = rest.as_bytes();
    if rb.len() < 5 {
        return None;
    }
    let h: i64 = rest.get(0..2)?.parse().ok()?;
    let mi: i64 = rest.get(3..5)?.parse().ok()?;
    let mut idx = 5;
    let mut sec = 0;
    if rb.get(5) == Some(&b':') {
        sec = rest.get(6..8)?.parse().ok()?;
        idx = 8;
    }
    let mut frac_ms = 0;
    if rb.get(idx) == Some(&b'.') {
        let digits: String = rest[idx + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        idx += 1 + digits.len();
        let padded = format!("{digits:0<3}");
        frac_ms = padded[..3].parse().unwrap_or(0);
    }
    ms += (h * 3600 + mi * 60 + sec) * 1000 + frac_ms;
    let zone = &rest[idx..];
    let offset_min = match zone {
        "" | "Z" | "z" => 0,
        z if z.len() >= 5 && (z.starts_with('+') || z.starts_with('-')) => {
            let sign = if z.starts_with('-') { -1 } else { 1 };
            let digits = z[1..].replace(':', "");
            let oh: i64 = digits.get(0..2)?.parse().ok()?;
            let om: i64 = digits.get(2..4)?.parse().ok()?;
            sign * (oh * 60 + om)
        }
        _ => return None,
    };
    Some(ms - offset_min * 60_000)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-10-30T10:15:00.000Z`.
pub fn format_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let base = format_unix(secs);
    format!("{}.{frac:03}Z", base.trim_end_matches('Z'))
}

/// Elasticsearch date math: `now`, `now-1h`, `now-7d/d`, `2026-10-01||+1M`, or a date.
pub fn date_math(expr: &str, now_ms: i64, round_up: bool) -> Option<i64> {
    let (anchor, ops) = if let Some(rest) = expr.strip_prefix("now") {
        (now_ms, rest)
    } else if let Some((date, rest)) = expr.split_once("||") {
        (parse_date_str(date)?, rest)
    } else {
        return parse_date_str(expr);
    };
    let mut t = anchor;
    let mut chars = ops.chars().peekable();
    while let Some(op) = chars.next() {
        match op {
            '+' | '-' => {
                let mut num = String::new();
                while let Some(c) = chars.peek().filter(|c| c.is_ascii_digit()) {
                    num.push(*c);
                    chars.next();
                }
                let n: i64 = if num.is_empty() { 1 } else { num.parse().ok()? };
                let unit = chars.next()?;
                let delta = unit_millis(unit)? * n;
                t = if op == '+' { t + delta } else { t - delta };
            }
            '/' => {
                let unit = chars.next()?;
                let size = unit_millis(unit)?;
                let floor = t.div_euclid(size) * size;
                t = if round_up { floor + size - 1 } else { floor };
            }
            _ => return None,
        }
    }
    Some(t)
}

fn unit_millis(unit: char) -> Option<i64> {
    Some(match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' | 'H' => 3_600_000,
        'd' => 86_400_000,
        'w' => 7 * 86_400_000,
        'M' => 30 * 86_400_000,
        'y' => 365 * 86_400_000,
        _ => return None,
    })
}

/// Interval lengths like `1h`, `15m`, `1d`, `hour`, `day`, `1w`.
pub fn interval_millis(s: &str) -> Option<i64> {
    let s = s.trim();
    let named = match s {
        "second" | "1s" => Some(1_000),
        "minute" | "1m" => Some(60_000),
        "hour" | "1h" => Some(3_600_000),
        "day" | "1d" => Some(86_400_000),
        "week" | "1w" => Some(7 * 86_400_000),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let unit = &s[digits.len()..];
    let n: i64 = digits.parse().ok()?;
    let per = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    Some(n * per)
}

// ────────────────────────────── comparisons ──────────────────────────────

/// A field value as a number, if it is (or parses as) one.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Compares a document value with a query value using the field type.
pub fn compare(
    doc: &Value,
    query: &Value,
    ty: Option<&str>,
    now_ms: i64,
    round_up: bool,
) -> Option<Ordering> {
    match ty {
        Some("date") | Some("date_nanos") => {
            let d = parse_date_millis(doc)?;
            let q = match query {
                Value::String(s) => date_math(s, now_ms, round_up)?,
                other => parse_date_millis(other)?,
            };
            Some(d.cmp(&q))
        }
        Some(t) if is_numeric(t) => as_f64(doc)?.partial_cmp(&as_f64(query)?),
        _ => match (doc, query) {
            (Value::Number(_), _) | (_, Value::Number(_)) => {
                as_f64(doc)?.partial_cmp(&as_f64(query)?)
            }
            (Value::String(a), Value::String(b)) => Some(a.as_str().cmp(b.as_str())),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            _ => None,
        },
    }
}

/// Equality for `term`-style matches on non-text fields.
pub fn value_eq(doc: &Value, query: &Value, ty: Option<&str>) -> bool {
    match ty {
        Some("boolean") => {
            let as_bool = |v: &Value| match v {
                Value::Bool(b) => Some(*b),
                Value::String(s) => s.parse().ok(),
                _ => None,
            };
            as_bool(doc).is_some_and(|d| Some(d) == as_bool(query))
        }
        Some("date") | Some("date_nanos") => {
            parse_date_millis(doc).is_some_and(|d| Some(d) == parse_date_millis(query))
        }
        Some(t) if is_numeric(t) => as_f64(doc).is_some_and(|d| Some(d) == as_f64(query)),
        _ => match (doc, query) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::String(a), other) | (other, Value::String(a)) => *a == scalar_string(other),
            (a, b) => a == b,
        },
    }
}

pub fn scalar_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Standard-analyzer tokens: lowercase alphanumeric runs.
pub fn analyze(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Glob match with `*` and `?`.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A JSON number for an `f64` that is integral when possible (`3.0` becomes `3`).
pub fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// Resolves a comma-separated index expression (wildcards, `_all`, aliases) to indices.
/// A concrete name that does not exist is an `index_not_found_exception`.
pub fn resolve<'a>(all: &'a [Index], expr: &str) -> Result<Vec<&'a Index>, EsError> {
    let mut out: Vec<&Index> = Vec::new();
    let push = |i: &'a Index, out: &mut Vec<&'a Index>| {
        if !out.iter().any(|o| o.name == i.name) {
            out.push(i);
        }
    };
    for part in expr.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(excl) = part.strip_prefix('-') {
            out.retain(|i| !wildcard_match(excl, &i.name));
            continue;
        }
        if part == "_all" || part == "*" {
            all.iter()
                .filter(|i| !i.name.starts_with('.'))
                .for_each(|i| push(i, &mut out));
            continue;
        }
        let matched: Vec<&Index> = all
            .iter()
            .filter(|i| {
                wildcard_match(part, &i.name)
                    || i.meta["aliases"].as_array().is_some_and(|a| {
                        a.iter()
                            .any(|x| x.as_str().is_some_and(|x| wildcard_match(part, x)))
                    })
            })
            .collect();
        if matched.is_empty() && !part.contains('*') {
            return Err(EsError::index_not_found(part));
        }
        matched.into_iter().for_each(|i| push(i, &mut out));
    }
    if expr.trim().is_empty() {
        all.iter()
            .filter(|i| !i.name.starts_with('.'))
            .for_each(|i| push(i, &mut out));
    }
    Ok(out)
}

/// [`resolve`] for data access: closed indices are skipped when matched by a wildcard
/// and are an `index_closed_exception` when named directly.
pub fn resolve_open<'a>(all: &'a [Index], expr: &str) -> Result<Vec<&'a Index>, EsError> {
    let found = resolve(all, expr)?;
    let concrete: Vec<&str> = expr.split(',').map(str::trim).collect();
    let mut out = Vec::new();
    for i in found {
        if i.meta["status"] == "close" {
            if concrete.contains(&i.name.as_str()) {
                return Err(EsError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "index_closed_exception",
                    "closed",
                )
                .with(
                    "index_uuid",
                    json!(i.meta["uuid"].as_str().unwrap_or("_na_")),
                )
                .with("index", json!(i.name)));
            }
            continue;
        }
        out.push(i);
    }
    Ok(out)
}
