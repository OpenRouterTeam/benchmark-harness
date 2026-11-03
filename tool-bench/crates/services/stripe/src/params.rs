//! Request parameters. MCP calls pass JSON; the REST face passes form-encoded bodies and
//! queries (`metadata[plan]=pro`, `expand[]=customer`, `items[0][price]=price_x`), which are
//! decoded into the same JSON shape. Stripe coerces leniently (every value was once a form
//! string), so the accessors accept strings for numbers and booleans.

use serde_json::{Map, Value, json};

use crate::error::{Res, StripeError};

#[derive(Debug, Clone, Default)]
pub struct Params {
    map: Map<String, Value>,
}

impl Params {
    pub fn new(map: Map<String, Value>) -> Self {
        Self { map }
    }

    pub fn from_value(value: &Value) -> Self {
        Self::new(value.as_object().cloned().unwrap_or_default())
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    pub fn raw(&self, key: &str) -> Option<&Value> {
        self.map.get(key).filter(|v| !v.is_null())
    }

    pub fn has(&self, key: &str) -> bool {
        self.raw(key).is_some()
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key)
    }

    pub fn insert(&mut self, key: &str, value: Value) {
        self.map.insert(key.to_owned(), value);
    }

    pub fn str(&self, key: &str) -> Option<String> {
        self.raw(key).and_then(scalar_string)
    }

    pub fn req_str(&self, key: &str) -> Res<String> {
        match self.str(key) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(StripeError::missing_param(key)),
        }
    }

    pub fn int(&self, key: &str) -> Res<Option<i64>> {
        match self.raw(key) {
            None => Ok(None),
            Some(v) => as_int(v)
                .map(Some)
                .ok_or_else(|| StripeError::invalid_integer(key, &display(v))),
        }
    }

    pub fn float(&self, key: &str) -> Res<Option<f64>> {
        match self.raw(key) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(Value::String(s)) => s.trim().parse::<f64>().map(Some).map_err(|_| {
                StripeError::invalid(format!("Invalid decimal: {s}"))
                    .code("parameter_invalid_decimal")
                    .param(key)
            }),
            Some(v) => {
                Err(StripeError::invalid(format!("Invalid decimal: {}", display(v))).param(key))
            }
        }
    }

    pub fn bool(&self, key: &str) -> Res<Option<bool>> {
        match self.raw(key) {
            None => Ok(None),
            Some(v) => as_bool(v)
                .map(Some)
                .ok_or_else(|| StripeError::invalid_boolean(key, &display(v))),
        }
    }

    pub fn obj(&self, key: &str) -> Option<Map<String, Value>> {
        self.raw(key).and_then(|v| v.as_object().cloned())
    }

    /// Array parameters: JSON arrays, form-decoded index maps, or a single scalar.
    pub fn list(&self, key: &str) -> Vec<Value> {
        self.raw(key).map(as_list).unwrap_or_default()
    }

    pub fn expand(&self) -> Vec<String> {
        self.list("expand")
            .iter()
            .filter_map(scalar_string)
            .filter(|s| !s.is_empty())
            .collect()
    }
}

pub fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn display(v: &Value) -> String {
    scalar_string(v).unwrap_or_else(|| v.to_string())
}

pub fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn as_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(items) => items.clone(),
        Value::Object(map) => {
            let mut indexed: Vec<(usize, Value)> = map
                .iter()
                .filter_map(|(k, v)| k.parse::<usize>().ok().map(|i| (i, v.clone())))
                .collect();
            indexed.sort_by_key(|(i, _)| *i);
            indexed.into_iter().map(|(_, v)| v).collect()
        }
        Value::String(s) if s.is_empty() => Vec::new(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Merges a `metadata` parameter into `existing`: `""` clears everything, a key set to
/// `""` (or null) is removed, other values are stored as strings.
pub fn apply_metadata(existing: &mut Value, param: Option<&Value>) {
    let Some(param) = param else { return };
    if !existing.is_object() {
        *existing = json!({});
    }
    let target = existing.as_object_mut().expect("object");
    match param {
        Value::String(s) if s.is_empty() => target.clear(),
        Value::Object(updates) => {
            for (k, v) in updates {
                match scalar_string(v) {
                    Some(s) if !s.is_empty() => {
                        target.insert(k.clone(), Value::String(s));
                    }
                    _ => {
                        target.remove(k);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Decodes an `application/x-www-form-urlencoded` string with Stripe's bracket notation.
pub fn decode_form(text: &str) -> Map<String, Value> {
    let mut root = Value::Object(Map::new());
    for pair in text.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(k);
        let value = percent_decode(v);
        let mut segments = Vec::new();
        let (head, mut rest) = match key.find('[') {
            Some(i) => (key[..i].to_owned(), &key[i..]),
            None => (key.clone(), ""),
        };
        segments.push(head);
        while let Some(stripped) = rest.strip_prefix('[') {
            match stripped.find(']') {
                Some(end) => {
                    segments.push(stripped[..end].to_owned());
                    rest = &stripped[end + 1..];
                }
                None => break,
            }
        }
        insert_path(&mut root, &segments, Value::String(value));
    }
    match normalize_arrays(root) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn insert_path(node: &mut Value, segments: &[String], value: Value) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    if first.is_empty() {
        if !node.is_array() {
            *node = Value::Array(Vec::new());
        }
        let arr = node.as_array_mut().expect("array");
        if rest.is_empty() {
            arr.push(value);
        } else {
            let mut child = Value::Object(Map::new());
            insert_path(&mut child, rest, value);
            arr.push(child);
        }
        return;
    }
    if !node.is_object() {
        *node = Value::Object(Map::new());
    }
    let map = node.as_object_mut().expect("object");
    if rest.is_empty() {
        map.insert(first.clone(), value);
    } else {
        let child = map
            .entry(first.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        insert_path(child, rest, value);
    }
}

/// Objects whose keys are all indices (`items[0][price]`) become arrays.
fn normalize_arrays(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let all_indices = !map.is_empty() && map.keys().all(|k| k.parse::<usize>().is_ok());
            let normalized: Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, normalize_arrays(v)))
                .collect();
            if all_indices {
                Value::Array(as_list(&Value::Object(normalized)))
            } else {
                Value::Object(normalized)
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_arrays).collect()),
        other => other,
    }
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_stripe_brackets() {
        let map = decode_form(
            "email=a%40b.com&metadata[plan]=pro&expand[]=customer&expand[]=latest_invoice&items[0][price]=price_1&items[0][quantity]=2&items[1][price]=price_2&name=Ada+Lovelace",
        );
        assert_eq!(
            Value::Object(map),
            json!({
                "email": "a@b.com",
                "metadata": {"plan": "pro"},
                "expand": ["customer", "latest_invoice"],
                "items": [{"price": "price_1", "quantity": "2"}, {"price": "price_2"}],
                "name": "Ada Lovelace"
            })
        );
    }

    #[test]
    fn coerces_scalars() {
        let p = Params::from_value(&json!({"limit": "3", "n": 4, "flag": "true", "bad": "x"}));
        assert_eq!(p.int("limit").unwrap(), Some(3));
        assert_eq!(p.int("n").unwrap(), Some(4));
        assert_eq!(p.bool("flag").unwrap(), Some(true));
        assert!(p.int("bad").is_err());
        assert_eq!(p.str("n").as_deref(), Some("4"));
    }

    #[test]
    fn metadata_merges_and_unsets() {
        let mut m = json!({"a": "1", "b": "2"});
        apply_metadata(&mut m, Some(&json!({"a": "", "c": 3})));
        assert_eq!(m, json!({"b": "2", "c": "3"}));
        apply_metadata(&mut m, Some(&json!("")));
        assert_eq!(m, json!({}));
    }
}
