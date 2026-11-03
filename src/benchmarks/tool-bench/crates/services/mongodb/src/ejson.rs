//! Documents are kept as relaxed Extended JSON (what the server prints): `{"$oid": ..}` for
//! ObjectIds, `{"$date": "<ISO>"}` for dates, plain JSON numbers for int/long/double. This
//! module normalises canonical input, compares values in BSON order, and formats output.

use std::cmp::Ordering;

use serde_json::{Map, Number, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Single-key objects that stand for one BSON scalar rather than a sub-document.
const WRAPPERS: [&str; 13] = [
    "$oid",
    "$date",
    "$numberDecimal",
    "$numberLong",
    "$numberInt",
    "$numberDouble",
    "$regularExpression",
    "$binary",
    "$timestamp",
    "$uuid",
    "$minKey",
    "$maxKey",
    "$symbol",
];

pub fn is_wrapper(v: &Value) -> bool {
    match v {
        Value::Object(m) => {
            m.len() <= 2
                && m.keys()
                    .next()
                    .is_some_and(|k| WRAPPERS.contains(&k.as_str()))
        }
        _ => false,
    }
}

/// Canonical EJSON (`$numberInt`, `$numberLong`, `$date: {$numberLong}`) to relaxed form.
pub fn normalize(v: Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(m) => {
            if m.len() == 1 {
                let (k, inner) = m.iter().next().expect("one entry");
                match (k.as_str(), inner) {
                    ("$numberInt" | "$numberLong", Value::String(s)) => {
                        if let Ok(n) = s.parse::<i64>() {
                            return Value::Number(n.into());
                        }
                    }
                    ("$numberDouble", Value::String(s)) => {
                        if let Ok(f) = s.parse::<f64>() {
                            return num(f);
                        }
                    }
                    ("$date", _) => {
                        if let Some(ms) = date_millis_raw(inner) {
                            return date(ms);
                        }
                    }
                    _ => {}
                }
            }
            Value::Object(m.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

fn date_millis_raw(inner: &Value) -> Option<i64> {
    match inner {
        Value::String(s) => parse_iso_millis(s),
        Value::Number(n) => n.as_i64(),
        Value::Object(m) => m
            .get("$numberLong")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok()),
        _ => None,
    }
}

pub fn parse_iso_millis(s: &str) -> Option<i64> {
    let full = if s.len() == 10 {
        format!("{s}T00:00:00Z")
    } else {
        s.to_owned()
    };
    let dt = OffsetDateTime::parse(&full, &Rfc3339).ok()?;
    Some((dt.unix_timestamp_nanos() / 1_000_000) as i64)
}

pub fn iso_millis(ms: i64) -> String {
    let dt = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

pub fn date(ms: i64) -> Value {
    json!({ "$date": iso_millis(ms) })
}

pub fn date_millis(v: &Value) -> Option<i64> {
    v.as_object()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.get("$date"))
        .and_then(date_millis_raw)
}

pub fn oid_hex(v: &Value) -> Option<&str> {
    v.as_object()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.get("$oid"))
        .and_then(Value::as_str)
}

/// JS-style number: integral values print without a fraction.
pub fn num(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
        Value::Number((f as i64).into())
    } else {
        Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Object(m) if m.len() == 1 => m
            .get("$numberDecimal")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok()),
        _ => None,
    }
}

pub fn is_number(v: &Value) -> bool {
    as_f64(v).is_some()
}

/// BSON comparison order of types.
pub fn type_rank(v: &Value) -> u8 {
    match v {
        Value::Null => 2,
        Value::Number(_) => 3,
        Value::String(_) => 4,
        Value::Array(_) => 6,
        Value::Bool(_) => 9,
        Value::Object(m) => match m.keys().next().map(String::as_str) {
            _ if !is_wrapper(v) => 5,
            Some("$minKey") => 0,
            Some("$numberDecimal" | "$numberLong" | "$numberInt" | "$numberDouble") => 3,
            Some("$symbol") => 4,
            Some("$binary" | "$uuid") => 7,
            Some("$oid") => 8,
            Some("$date") => 10,
            Some("$timestamp") => 11,
            Some("$regularExpression") => 12,
            Some("$maxKey") => 13,
            _ => 5,
        },
    }
}

/// Human type name as `$type` reports it.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Number(n) => {
            if n.is_f64() {
                "double"
            } else if n.as_i64().is_some_and(|i| i32::try_from(i).is_ok()) {
                "int"
            } else {
                "long"
            }
        }
        Value::Object(m) => match m.keys().next().map(String::as_str) {
            _ if !is_wrapper(v) => "object",
            Some("$oid") => "objectId",
            Some("$date") => "date",
            Some("$numberDecimal") => "decimal",
            Some("$regularExpression") => "regex",
            Some("$binary" | "$uuid") => "binData",
            Some("$timestamp") => "timestamp",
            Some("$minKey") => "minKey",
            Some("$maxKey") => "maxKey",
            Some("$symbol") => "symbol",
            _ => "object",
        },
    }
}

pub fn cmp_values(a: &Value, b: &Value) -> Ordering {
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        _ if ra == 3 => as_f64(a)
            .unwrap_or(0.0)
            .partial_cmp(&as_f64(b).unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                let o = cmp_values(p, q);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        _ if ra == 10 => date_millis(a).cmp(&date_millis(b)),
        _ if ra == 8 => oid_hex(a).cmp(&oid_hex(b)),
        (Value::Object(x), Value::Object(y)) if ra == 5 => {
            for ((ka, va), (kb, vb)) in x.iter().zip(y) {
                let o = ka.cmp(kb).then_with(|| cmp_values(va, vb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => a.to_string().cmp(&b.to_string()),
    }
}

pub fn eq_values(a: &Value, b: &Value) -> bool {
    cmp_values(a, b) == Ordering::Equal
}

/// Store key for a document `_id`.
pub fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        v => oid_hex(v).map_or_else(|| v.to_string(), str::to_owned),
    }
}

/// `ObjectId.toString()`-style rendering of an `_id` for messages.
pub fn id_display(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        v => oid_hex(v).map_or_else(|| v.to_string(), str::to_owned),
    }
}

/// mongosh-style rendering of a value inside error messages, e.g. `ObjectId('…')`.
pub fn shell_repr(v: &Value) -> String {
    if let Some(h) = oid_hex(v) {
        return format!("ObjectId('{h}')");
    }
    if let Some(ms) = date_millis(v) {
        return format!("new Date({ms})");
    }
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Object(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}: {}", shell_repr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Array(items) => format!(
            "[ {} ]",
            items.iter().map(shell_repr).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

/// Approximate BSON size of a document (deterministic; used for stats and byte limits).
pub fn bson_size(v: &Value) -> usize {
    match v {
        Value::Object(m) if !is_wrapper(v) => {
            5 + m
                .iter()
                .map(|(k, v)| 2 + k.len() + bson_size(v))
                .sum::<usize>()
        }
        Value::Array(items) => {
            5 + items
                .iter()
                .enumerate()
                .map(|(i, v)| 2 + i.to_string().len() + bson_size(v))
                .sum::<usize>()
        }
        Value::String(s) => 5 + s.len(),
        Value::Number(n) => {
            if n.is_f64() || n.as_i64().is_some_and(|i| i32::try_from(i).is_err()) {
                8
            } else {
                4
            }
        }
        Value::Bool(_) => 1,
        Value::Null => 0,
        Value::Object(m) => match m.keys().next().map(String::as_str) {
            Some("$oid") => 12,
            Some("$date" | "$timestamp") => 8,
            Some("$numberDecimal") => 16,
            _ => 16,
        },
    }
}

/// Empty object helper.
pub fn obj() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_canonical_numbers_and_dates() {
        let v = normalize(json!({
            "n": {"$numberInt": "5"},
            "l": {"$numberLong": "9000000000"},
            "d": {"$date": {"$numberLong": "1772323200000"}},
            "o": {"$oid": "65f000000000000000000001"}
        }));
        assert_eq!(v["n"], 5);
        assert_eq!(v["l"], 9_000_000_000_i64);
        assert_eq!(v["d"], json!({"$date": "2026-03-01T00:00:00.000Z"}));
        assert_eq!(v["o"]["$oid"], "65f000000000000000000001");
    }

    #[test]
    fn orders_values_like_bson() {
        assert_eq!(cmp_values(&json!(1), &json!(1.0)), Ordering::Equal);
        assert_eq!(cmp_values(&json!(null), &json!(0)), Ordering::Less);
        assert_eq!(cmp_values(&json!(5), &json!("a")), Ordering::Less);
        assert_eq!(
            cmp_values(
                &json!({"$date": "2026-01-01T00:00:00Z"}),
                &json!({"$date": "2026-02-01T00:00:00.000Z"})
            ),
            Ordering::Less
        );
        assert_eq!(num(42.0), json!(42));
        assert_eq!(num(2.5), json!(2.5));
    }
}
