//! Update operators (`updateMany` semantics).

use serde_json::{Map, Value, json};

use crate::agg::{self, Source};
use crate::ejson::{as_f64, cmp_values, date, eq_values, num, type_name};
use crate::query::{self, QResult, get_path, set_path, unset_path};

/// Applies `update` to `doc`. `inserting` enables `$setOnInsert`.
pub fn apply(doc: &mut Value, update: &Value, inserting: bool, src: &dyn Source) -> QResult<()> {
    if let Value::Array(stages) = update {
        for stage in stages {
            let name = stage
                .as_object()
                .and_then(|m| m.keys().next())
                .map(String::as_str);
            if !matches!(
                name,
                Some(
                    "$set" | "$addFields" | "$unset" | "$project" | "$replaceRoot" | "$replaceWith"
                )
            ) {
                return Err(format!(
                    "{} is not allowed to be used within an update",
                    name.unwrap_or("stage")
                ));
            }
        }
        let (mut out, _) = agg::run(vec![doc.clone()], stages, src)?;
        let id = doc.get("_id").cloned();
        *doc = out.pop().unwrap_or_else(|| json!({}));
        if let Some(id) = id {
            doc["_id"] = id;
        }
        return Ok(());
    }
    let Some(ops) = update.as_object() else {
        return Err("Update argument must be either an object or an array".to_owned());
    };
    if ops.is_empty() || ops.keys().any(|k| !k.starts_with('$')) {
        return Err("Update document requires atomic operators".to_owned());
    }
    for (op, fields) in ops {
        let Some(fields) = fields.as_object() else {
            return Err(format!(
                "Modifiers operate on fields but we found type {} instead. For example: {{$mod: {{<field>: ...}}}} not {{{op}: {}}}",
                type_name(fields),
                fields
            ));
        };
        for (path, arg) in fields {
            if path == "_id" && op != "$setOnInsert" && op != "$set" {
                return Err(
                    "Performing an update on the path '_id' would modify the immutable field '_id'"
                        .to_owned(),
                );
            }
            if path.contains(".$") {
                return Err(format!(
                    "The positional operator did not find the match needed from the query. ({path})"
                ));
            }
            apply_op(doc, op, path, arg, inserting, src)?;
        }
    }
    Ok(())
}

fn number_at(doc: &Value, path: &str, op: &str) -> QResult<Option<f64>> {
    match get_path(doc, path) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => as_f64(v).map(Some).ok_or_else(|| {
            format!(
                "Cannot apply {op} to a value of non-numeric type. {{_id: {}}} has the field '{path}' of non-numeric type {}",
                doc.get("_id").map_or("null".to_owned(), crate::ejson::shell_repr),
                type_name(v)
            )
        }),
    }
}

fn apply_op(
    doc: &mut Value,
    op: &str,
    path: &str,
    arg: &Value,
    inserting: bool,
    src: &dyn Source,
) -> QResult<()> {
    match op {
        "$set" => {
            if path == "_id" && get_path(doc, "_id").is_some_and(|id| !eq_values(id, arg)) {
                return Err(
                    "Performing an update on the path '_id' would modify the immutable field '_id'"
                        .to_owned(),
                );
            }
            set_path(doc, path, arg.clone())?;
        }
        "$setOnInsert" => {
            if inserting {
                set_path(doc, path, arg.clone())?;
            }
        }
        "$unset" => {
            unset_path(doc, path);
        }
        "$inc" | "$mul" => {
            let by = as_f64(arg).ok_or_else(|| {
                format!(
                    "Cannot {} with non-numeric argument: {{{path}: {arg}}}",
                    &op[1..]
                )
            })?;
            let cur = number_at(doc, path, op)?;
            let v = match (op, cur) {
                ("$inc", c) => c.unwrap_or(0.0) + by,
                (_, Some(c)) => c * by,
                (_, None) => 0.0,
            };
            set_path(doc, path, num(v))?;
        }
        "$min" | "$max" => {
            let replace = match get_path(doc, path) {
                None => true,
                Some(cur) => {
                    let o = cmp_values(arg, cur);
                    if op == "$min" { o.is_lt() } else { o.is_gt() }
                }
            };
            if replace {
                set_path(doc, path, arg.clone())?;
            }
        }
        "$rename" => {
            let to = arg
                .as_str()
                .ok_or("The 'to' field for $rename must be a string")?;
            if let Some(v) = get_path(doc, path).cloned() {
                unset_path(doc, path);
                set_path(doc, to, v)?;
            }
        }
        "$currentDate" => {
            set_path(doc, path, date(src.now_ms()))?;
        }
        "$push" | "$addToSet" => {
            let (items, slice, sort, position) = match arg
                .as_object()
                .and_then(|m| m.get("$each").map(|e| (m, e)))
            {
                Some((m, Value::Array(each))) => (
                    each.clone(),
                    m.get("$slice").and_then(as_f64),
                    m.get("$sort").cloned(),
                    m.get("$position").and_then(as_f64),
                ),
                Some(_) => {
                    return Err(format!(
                        "The argument to $each in {op} must be an array but it was of type: object"
                    ));
                }
                None => (vec![arg.clone()], None, None, None),
            };
            let mut arr = match get_path(doc, path) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                Some(other) => {
                    return Err(format!(
                        "The field '{path}' must be an array but is of type {} in document {{_id: {}}}",
                        type_name(other),
                        doc.get("_id")
                            .map_or("null".to_owned(), crate::ejson::shell_repr)
                    ));
                }
            };
            let mut pos = position.map_or(arr.len(), |p| (p.max(0.0) as usize).min(arr.len()));
            for item in items {
                if op == "$addToSet" && arr.iter().any(|a| eq_values(a, &item)) {
                    continue;
                }
                arr.insert(pos, item);
                pos += 1;
            }
            if let Some(sort) = sort {
                let mut docs = arr;
                match &sort {
                    Value::Object(_) => query::sort_docs(&mut docs, &sort)?,
                    v => {
                        let desc = as_f64(v) == Some(-1.0);
                        docs.sort_by(|a, b| {
                            if desc {
                                cmp_values(b, a)
                            } else {
                                cmp_values(a, b)
                            }
                        });
                    }
                }
                arr = docs;
            }
            if let Some(n) = slice {
                let n = n as i64;
                if n >= 0 {
                    arr.truncate(n as usize);
                } else {
                    let keep = n.unsigned_abs() as usize;
                    if arr.len() > keep {
                        arr = arr.split_off(arr.len() - keep);
                    }
                }
            }
            set_path(doc, path, Value::Array(arr))?;
        }
        "$pull" | "$pullAll" => {
            if let Some(Value::Array(arr)) = get_path(doc, path).cloned() {
                let kept: Vec<Value> = arr
                    .into_iter()
                    .filter(|item| {
                        if op == "$pullAll" {
                            return !arg
                                .as_array()
                                .is_some_and(|a| a.iter().any(|x| eq_values(x, item)));
                        }
                        let hit = match arg {
                            Value::Object(m) if !crate::ejson::is_wrapper(arg) => {
                                if m.keys().all(|k| k.starts_with('$')) {
                                    query::field_matches(std::slice::from_ref(item), arg)
                                        .unwrap_or(false)
                                } else {
                                    item.is_object() && query::matches(item, arg).unwrap_or(false)
                                }
                            }
                            v => eq_values(item, v),
                        };
                        !hit
                    })
                    .collect();
                set_path(doc, path, Value::Array(kept))?;
            }
        }
        "$pop" => {
            if let Some(Value::Array(mut arr)) = get_path(doc, path).cloned() {
                if as_f64(arg) == Some(-1.0) {
                    if !arr.is_empty() {
                        arr.remove(0);
                    }
                } else {
                    arr.pop();
                }
                set_path(doc, path, Value::Array(arr))?;
            }
        }
        other => {
            return Err(format!(
                "Unknown modifier: {other}. Expected a valid update modifier or pipeline-style update specified as an array"
            ));
        }
    }
    Ok(())
}

/// The document an upsert starts from: equality fields of the filter.
pub fn upsert_seed(filter: &Value) -> Value {
    let mut doc = Value::Object(Map::new());
    if let Some(m) = filter.as_object() {
        for (k, v) in m {
            if k.starts_with('$') {
                if k == "$and" {
                    for part in v.as_array().into_iter().flatten() {
                        if let Value::Object(pm) = upsert_seed(part) {
                            for (pk, pv) in pm {
                                let _ = set_path(&mut doc, &pk, pv);
                            }
                        }
                    }
                }
                continue;
            }
            let eq = match v {
                Value::Object(om)
                    if om.keys().next().is_some_and(|k| k.starts_with('$'))
                        && !crate::ejson::is_wrapper(v) =>
                {
                    om.get("$eq").cloned()
                }
                other => Some(other.clone()),
            };
            if let Some(eq) = eq {
                let _ = set_path(&mut doc, k, eq);
            }
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Now;
    impl Source for Now {
        fn docs(&self, _: &str) -> Vec<Value> {
            vec![]
        }
        fn now_ms(&self) -> i64 {
            1_772_323_200_000
        }
    }

    #[test]
    fn operators_apply() {
        let mut d = json!({"_id": 1, "n": 1, "tags": ["a"], "s": "x"});
        apply(
            &mut d,
            &json!({"$inc": {"n": 2}, "$push": {"tags": "b"}, "$set": {"a.b": true}, "$unset": {"s": ""}, "$currentDate": {"at": true}}),
            false,
            &Now,
        )
        .unwrap();
        assert_eq!(
            d,
            json!({"_id": 1, "n": 3, "tags": ["a", "b"], "a": {"b": true}, "at": {"$date": "2026-03-01T00:00:00.000Z"}})
        );
        assert_eq!(
            apply(&mut d, &json!({"n": 5}), false, &Now).unwrap_err(),
            "Update document requires atomic operators"
        );
        apply(
            &mut d,
            &json!([{"$set": {"m": {"$add": ["$n", 1]}}}]),
            false,
            &Now,
        )
        .unwrap();
        assert_eq!(d["m"], 4);
    }
}
