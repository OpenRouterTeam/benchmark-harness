//! MQL query filters (`db.collection.find()` semantics) and dotted-path helpers.

use std::cmp::Ordering;

use regex::Regex;
use serde_json::{Map, Value};

use crate::agg;
use crate::ejson::{self, as_f64, cmp_values, eq_values, is_wrapper, type_name, type_rank};

pub type QResult<T> = Result<T, String>;

/// Leaf values at a dotted path, traversing arrays of sub-documents like MongoDB.
pub fn resolve(doc: &Value, path: &str) -> Vec<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut out = Vec::new();
    resolve_into(doc, &parts, &mut out);
    out
}

fn resolve_into(cur: &Value, parts: &[&str], out: &mut Vec<Value>) {
    let Some((head, rest)) = parts.split_first() else {
        out.push(cur.clone());
        return;
    };
    match cur {
        Value::Object(m) if !is_wrapper(cur) => {
            if let Some(next) = m.get(*head) {
                resolve_into(next, rest, out);
            }
        }
        Value::Array(items) => {
            if let Ok(i) = head.parse::<usize>()
                && let Some(next) = items.get(i)
            {
                resolve_into(next, rest, out);
            }
            for item in items {
                if item.is_object() && !is_wrapper(item) {
                    resolve_into(item, parts, out);
                }
            }
        }
        _ => {}
    }
}

/// Single value at a path without array traversal (numeric segments index arrays).
pub fn get_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = doc;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) if !is_wrapper(cur) => m.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

pub fn get_path_mut<'a>(doc: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut cur = doc;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get_mut(part)?,
            Value::Array(items) => items.get_mut(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Sets a dotted path, creating intermediate documents.
pub fn set_path(doc: &mut Value, path: &str, value: Value) -> QResult<()> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = doc;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        match cur {
            Value::Object(m) => {
                if last {
                    m.insert((*part).to_owned(), value);
                    return Ok(());
                }
                let entry = m
                    .entry((*part).to_owned())
                    .or_insert_with(|| Value::Object(Map::new()));
                if entry.is_null() {
                    *entry = Value::Object(Map::new());
                }
                cur = entry;
            }
            Value::Array(items) => {
                let Ok(idx) = part.parse::<usize>() else {
                    return Err(format!(
                        "Cannot create field '{part}' in element {{{}: {}}}",
                        parts[..i].join("."),
                        ejson::shell_repr(&Value::Array(items.clone()))
                    ));
                };
                while items.len() <= idx {
                    items.push(Value::Null);
                }
                if last {
                    items[idx] = value;
                    return Ok(());
                }
                if items[idx].is_null() {
                    items[idx] = Value::Object(Map::new());
                }
                cur = &mut items[idx];
            }
            other => {
                return Err(format!(
                    "Cannot create field '{part}' in element {{{}: {}}}",
                    parts[..i].join("."),
                    ejson::shell_repr(other)
                ));
            }
        }
    }
    Ok(())
}

/// Removes a dotted path; returns whether something was removed.
pub fn unset_path(doc: &mut Value, path: &str) -> bool {
    let (parent, leaf) = match path.rsplit_once('.') {
        Some((p, l)) => (get_path_mut(doc, p), l),
        None => (Some(doc), path),
    };
    match parent {
        Some(Value::Object(m)) => m.shift_remove(leaf).is_some(),
        Some(Value::Array(items)) => match leaf.parse::<usize>() {
            Ok(i) if i < items.len() => {
                items[i] = Value::Null;
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// Does `doc` match the query `filter`?
pub fn matches(doc: &Value, filter: &Value) -> QResult<bool> {
    let Some(filter) = filter.as_object() else {
        if filter.is_null() {
            return Ok(true);
        }
        return Err("query filter must be an object".to_owned());
    };
    for (key, cond) in filter {
        let ok = match key.as_str() {
            "$and" | "$or" | "$nor" => {
                let Some(items) = cond.as_array().filter(|a| !a.is_empty()) else {
                    return Err(format!("{key} must be a nonempty array"));
                };
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    if !item.is_object() {
                        return Err(format!("{key} argument's entries must be objects"));
                    }
                    results.push(matches(doc, item)?);
                }
                match key.as_str() {
                    "$and" => results.iter().all(|r| *r),
                    "$or" => results.iter().any(|r| *r),
                    _ => !results.iter().any(|r| *r),
                }
            }
            "$expr" => agg::truthy(&agg::eval_root(cond, doc)?),
            "$comment" => true,
            "$where" => {
                return Err("$where is not allowed in this context".to_owned());
            }
            "$text" => {
                return Err("text index required for $text query".to_owned());
            }
            k if k.starts_with('$') => return Err(format!("unknown top level operator: {k}")),
            path => field_matches(&resolve(doc, path), cond)?,
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn is_operator_object(cond: &Value) -> bool {
    match cond {
        Value::Object(m) if !is_wrapper(cond) => {
            m.keys().next().is_some_and(|k| k.starts_with('$'))
        }
        _ => false,
    }
}

/// Matches the leaf values at a path against a condition (equality or operator object).
pub fn field_matches(leaves: &[Value], cond: &Value) -> QResult<bool> {
    if is_operator_object(cond) {
        let ops = cond.as_object().expect("operator object");
        for (op, arg) in ops {
            if op == "$options" {
                continue;
            }
            if !apply_op(leaves, op, arg, ops)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(re) = regex_of(cond)? {
        return Ok(expanded(leaves).iter().any(|v| regex_hit(&re, v)));
    }
    Ok(eq_match(leaves, cond))
}

fn expanded(leaves: &[Value]) -> Vec<&Value> {
    let mut out = Vec::new();
    for leaf in leaves {
        out.push(leaf);
        if let Value::Array(items) = leaf {
            out.extend(items.iter());
        }
    }
    out
}

fn eq_match(leaves: &[Value], target: &Value) -> bool {
    if target.is_null() && (leaves.is_empty() || leaves.iter().any(Value::is_null)) {
        return true;
    }
    expanded(leaves).iter().any(|v| eq_values(v, target))
}

fn cmp_match(leaves: &[Value], target: &Value, pred: fn(Ordering) -> bool) -> bool {
    if target.is_null() {
        let inclusive = pred(Ordering::Equal);
        return inclusive && (leaves.is_empty() || leaves.iter().any(Value::is_null));
    }
    expanded(leaves)
        .iter()
        .any(|v| type_rank(v) == type_rank(target) && pred(cmp_values(v, target)))
}

fn in_match(leaves: &[Value], arg: &Value) -> QResult<bool> {
    let Some(items) = arg.as_array() else {
        return Err("$in needs an array".to_owned());
    };
    for item in items {
        if let Some(re) = regex_of(item)? {
            if expanded(leaves).iter().any(|v| regex_hit(&re, v)) {
                return Ok(true);
            }
        } else if eq_match(leaves, item) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn apply_op(leaves: &[Value], op: &str, arg: &Value, all: &Map<String, Value>) -> QResult<bool> {
    Ok(match op {
        "$eq" => eq_match(leaves, arg),
        "$ne" => !eq_match(leaves, arg),
        "$gt" => cmp_match(leaves, arg, |o| o == Ordering::Greater),
        "$gte" => cmp_match(leaves, arg, |o| o != Ordering::Less),
        "$lt" => cmp_match(leaves, arg, |o| o == Ordering::Less),
        "$lte" => cmp_match(leaves, arg, |o| o != Ordering::Greater),
        "$in" => in_match(leaves, arg)?,
        "$nin" => {
            if !arg.is_array() {
                return Err("$nin needs an array".to_owned());
            }
            !in_match(leaves, arg)?
        }
        "$exists" => {
            let want = agg::truthy(arg);
            !leaves.is_empty() == want
        }
        "$type" => {
            let wanted: Vec<&Value> = match arg {
                Value::Array(items) => items.iter().collect(),
                v => vec![v],
            };
            expanded(leaves)
                .iter()
                .any(|v| wanted.iter().any(|w| type_is(v, w)))
        }
        "$regex" => {
            let options = all.get("$options").and_then(Value::as_str).unwrap_or("");
            let re = match arg {
                Value::String(p) => build_regex(p, options)?,
                v => regex_of(v)?.ok_or_else(|| "$regex has to be a string".to_owned())?,
            };
            expanded(leaves).iter().any(|v| regex_hit(&re, v))
        }
        "$not" => {
            if let Some(re) = regex_of(arg)? {
                !expanded(leaves).iter().any(|v| regex_hit(&re, v))
            } else if is_operator_object(arg) {
                !field_matches(leaves, arg)?
            } else {
                return Err("$not needs a regex or a document".to_owned());
            }
        }
        "$size" => {
            let n = as_f64(arg).ok_or_else(|| "$size needs a number".to_owned())?;
            leaves
                .iter()
                .any(|v| v.as_array().is_some_and(|a| a.len() as f64 == n))
        }
        "$all" => {
            let Some(items) = arg.as_array() else {
                return Err("$all needs an array".to_owned());
            };
            !items.is_empty()
                && items.iter().all(|item| {
                    if let Some(em) = item.get("$elemMatch") {
                        apply_op(leaves, "$elemMatch", em, all).unwrap_or(false)
                    } else {
                        eq_match(leaves, item)
                    }
                })
        }
        "$elemMatch" => {
            if !arg.is_object() {
                return Err("$elemMatch needs an Object".to_owned());
            }
            let value_mode = arg.as_object().is_some_and(|m| {
                m.keys().all(|k| {
                    k.starts_with('$') && !matches!(k.as_str(), "$and" | "$or" | "$nor" | "$expr")
                })
            });
            let mut hit = false;
            for leaf in leaves {
                if let Value::Array(items) = leaf {
                    for item in items {
                        let ok = if value_mode {
                            field_matches(std::slice::from_ref(item), arg)?
                        } else {
                            item.is_object() && matches(item, arg)?
                        };
                        if ok {
                            hit = true;
                            break;
                        }
                    }
                }
                if hit {
                    break;
                }
            }
            hit
        }
        "$mod" => {
            let parts = arg
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| "malformed mod, needs to be an array of two numbers".to_owned())?;
            let d = as_f64(&parts[0]).unwrap_or(0.0);
            let r = as_f64(&parts[1]).unwrap_or(0.0);
            if d == 0.0 {
                return Err("divisor cannot be 0".to_owned());
            }
            expanded(leaves)
                .iter()
                .any(|v| as_f64(v).is_some_and(|x| (x.trunc() % d) == r))
        }
        "$comment" => true,
        other => return Err(format!("unknown operator: {other}")),
    })
}

fn type_is(v: &Value, wanted: &Value) -> bool {
    let name = type_name(v);
    match wanted {
        Value::String(s) => match s.as_str() {
            "number" => matches!(name, "int" | "long" | "double" | "decimal"),
            other => other == name,
        },
        Value::Number(n) => {
            let code = match name {
                "double" => 1,
                "string" => 2,
                "object" => 3,
                "array" => 4,
                "binData" => 5,
                "objectId" => 7,
                "bool" => 8,
                "date" => 9,
                "null" => 10,
                "regex" => 11,
                "int" => 16,
                "timestamp" => 17,
                "long" => 18,
                "decimal" => 19,
                _ => -100,
            };
            n.as_i64() == Some(code)
        }
        _ => false,
    }
}

/// A `{"$regularExpression": ...}` or `{"$regex": .., "$options": ..}` literal, if `v` is one.
pub fn regex_of(v: &Value) -> QResult<Option<Regex>> {
    if let Some(re) = v.get("$regularExpression") {
        let pattern = re.get("pattern").and_then(Value::as_str).unwrap_or("");
        let options = re.get("options").and_then(Value::as_str).unwrap_or("");
        return build_regex(pattern, options).map(Some);
    }
    Ok(None)
}

pub fn build_regex(pattern: &str, options: &str) -> QResult<Regex> {
    let mut flags = String::new();
    for c in options.chars() {
        match c {
            'i' | 'm' | 's' | 'x' => flags.push(c),
            'u' => {}
            other => return Err(format!("invalid flag in regex options: {other}")),
        }
    }
    let full = if flags.is_empty() {
        pattern.to_owned()
    } else {
        format!("(?{flags}){pattern}")
    };
    Regex::new(&full).map_err(|e| format!("Regular expression is invalid: {e}"))
}

fn regex_hit(re: &Regex, v: &Value) -> bool {
    v.as_str().is_some_and(|s| re.is_match(s))
}

/// `find()` projection. Inclusion or exclusion; `_id` is included unless excluded.
pub fn project(doc: &Value, projection: &Value) -> QResult<Value> {
    let Some(spec) = projection.as_object().filter(|m| !m.is_empty()) else {
        return Ok(doc.clone());
    };
    agg::project_stage(doc, spec)
}

/// Sort comparator for a `{field: 1|-1}` spec.
pub fn sort_docs(docs: &mut [Value], spec: &Value) -> QResult<()> {
    let Some(spec) = spec.as_object().filter(|m| !m.is_empty()) else {
        return Ok(());
    };
    let mut keys = Vec::new();
    for (field, dir) in spec {
        let d = match dir {
            Value::String(s) => match s.as_str() {
                "asc" | "ascending" => 1,
                "desc" | "descending" => -1,
                _ => return Err(format!("Invalid sort direction: {s}")),
            },
            Value::Object(m) if m.contains_key("$meta") => 1,
            v => match as_f64(v) {
                Some(1.0) => 1,
                Some(-1.0) => -1,
                _ => {
                    return Err(
                        "$sort key ordering must be 1 (for ascending) or -1 (for descending)"
                            .to_owned(),
                    );
                }
            },
        };
        keys.push((field.clone(), d));
    }
    docs.sort_by(|a, b| {
        for (field, dir) in &keys {
            let va = sort_key(a, field, *dir);
            let vb = sort_key(b, field, *dir);
            let o = cmp_values(&va, &vb);
            if o != Ordering::Equal {
                return if *dir < 0 { o.reverse() } else { o };
            }
        }
        Ordering::Equal
    });
    Ok(())
}

fn sort_key(doc: &Value, field: &str, dir: i32) -> Value {
    let leaves = resolve(doc, field);
    let mut candidates: Vec<Value> = Vec::new();
    for leaf in leaves {
        match leaf {
            Value::Array(items) if !items.is_empty() => candidates.extend(items),
            Value::Array(_) => candidates.push(Value::Null),
            v => candidates.push(v),
        }
    }
    let pick = if dir < 0 {
        candidates.into_iter().max_by(cmp_values)
    } else {
        candidates.into_iter().min_by(cmp_values)
    };
    pick.unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn m(doc: Value, filter: Value) -> bool {
        matches(&doc, &filter).unwrap()
    }

    #[test]
    fn equality_ranges_and_arrays() {
        let d = json!({"a": 5, "tags": ["x", "y"], "items": [{"sku": "A", "qty": 2}, {"sku": "B", "qty": 9}], "c": {"$date": "2026-02-01T00:00:00.000Z"}});
        assert!(m(d.clone(), json!({"a": 5})));
        assert!(m(d.clone(), json!({"a": {"$gte": 5, "$lt": 6}})));
        assert!(!m(d.clone(), json!({"a": {"$gt": "4"}})));
        assert!(m(d.clone(), json!({"tags": "y"})));
        assert!(m(d.clone(), json!({"items.sku": "B"})));
        assert!(m(
            d.clone(),
            json!({"items": {"$elemMatch": {"sku": "B", "qty": {"$gt": 5}}}})
        ));
        assert!(!m(
            d.clone(),
            json!({"items": {"$elemMatch": {"sku": "A", "qty": {"$gt": 5}}}})
        ));
        assert!(m(d.clone(), json!({"missing": null})));
        assert!(m(d.clone(), json!({"missing": {"$exists": false}})));
        assert!(m(
            d.clone(),
            json!({"c": {"$gte": {"$date": "2026-01-15T00:00:00Z"}}})
        ));
        assert!(m(
            d.clone(),
            json!({"$or": [{"a": 1}, {"tags": {"$in": ["z", "x"]}}]})
        ));
        assert!(m(
            d.clone(),
            json!({"tags": {"$size": 2}, "a": {"$nin": [1, 2]}})
        ));
        assert!(m(
            d.clone(),
            json!({"tags": {"$regex": "^X", "$options": "i"}})
        ));
        assert!(m(d, json!({"a": {"$not": {"$gt": 9}}})));
    }

    #[test]
    fn unknown_operators_error() {
        assert_eq!(
            matches(&json!({}), &json!({"a": {"$foo": 1}})).unwrap_err(),
            "unknown operator: $foo"
        );
    }

    #[test]
    fn paths_set_and_unset() {
        let mut d = json!({"a": {"b": 1}});
        set_path(&mut d, "a.c.d", json!(2)).unwrap();
        assert_eq!(d, json!({"a": {"b": 1, "c": {"d": 2}}}));
        assert!(unset_path(&mut d, "a.b"));
        assert_eq!(d, json!({"a": {"c": {"d": 2}}}));
    }
}
