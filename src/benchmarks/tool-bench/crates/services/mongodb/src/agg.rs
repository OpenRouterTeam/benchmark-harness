//! Aggregation pipelines and expressions over in-memory documents.

use std::{cmp::Ordering, collections::BTreeMap};

use serde_json::{Map, Value, json};

use crate::ejson::{
    self, as_f64, cmp_values, date, date_millis, eq_values, is_wrapper, num, type_name,
};
use crate::query::{self, QResult};

/// Other collections of the same database, for `$lookup` / `$unionWith`.
pub trait Source {
    fn docs(&self, collection: &str) -> Vec<Value>;
    /// Logical "now" in epoch milliseconds (`$$NOW`).
    fn now_ms(&self) -> i64;
}

/// Write stage found at the end of a pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteStage {
    Out { db: Option<String>, coll: String },
    Merge { db: Option<String>, coll: String },
}

pub type Vars = BTreeMap<String, Value>;

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(_) => as_f64(v).is_some_and(|f| f != 0.0),
        _ => true,
    }
}

/// Evaluates an expression with `$$ROOT` = `doc`; missing becomes `null`.
pub fn eval_root(expr: &Value, doc: &Value) -> QResult<Value> {
    let vars = Vars::new();
    Ok(eval(expr, doc, &vars, 0)?.unwrap_or(Value::Null))
}

fn field_path(doc: &Value, path: &str) -> Option<Value> {
    let mut cur = doc.clone();
    for part in path.split('.') {
        cur = match cur {
            Value::Object(ref m) if !is_wrapper(&cur) => m.get(part)?.clone(),
            Value::Array(items) => {
                let collected: Vec<Value> = items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Object(m) if !is_wrapper(item) => m.get(part).cloned(),
                        Value::Array(_) => field_path(item, part),
                        _ => None,
                    })
                    .collect();
                Value::Array(collected)
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// Evaluates an aggregation expression. `None` means "missing".
pub fn eval(expr: &Value, doc: &Value, vars: &Vars, depth: usize) -> QResult<Option<Value>> {
    if depth > 64 {
        return Err("expression nesting too deep".to_owned());
    }
    match expr {
        Value::String(s) if s.starts_with("$$") => {
            let name = &s[2..];
            let (var, rest) = name
                .split_once('.')
                .map_or((name, None), |(a, b)| (a, Some(b)));
            let base = match var {
                "ROOT" | "CURRENT" => Some(doc.clone()),
                "NOW" => Some(date(vars.get("NOW").and_then(as_f64).unwrap_or(0.0) as i64)),
                "REMOVE" => None,
                v => Some(
                    vars.get(v)
                        .cloned()
                        .ok_or_else(|| format!("Use of undefined variable: {v}"))?,
                ),
            };
            Ok(match (base, rest) {
                (Some(b), Some(r)) => field_path(&b, r),
                (b, _) => b,
            })
        }
        Value::String(s) if s.starts_with('$') => Ok(field_path(doc, &s[1..])),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(eval(item, doc, vars, depth + 1)?.unwrap_or(Value::Null));
            }
            Ok(Some(Value::Array(out)))
        }
        Value::Object(m) if is_wrapper(expr) => Ok(Some(Value::Object(m.clone()))),
        Value::Object(m) => {
            if m.len() == 1 {
                let (k, arg) = m.iter().next().expect("one entry");
                if k.starts_with('$') {
                    return operator(k, arg, doc, vars, depth);
                }
            }
            let mut out = Map::new();
            for (k, v) in m {
                if k.starts_with('$') {
                    return Err(format!("Unrecognized expression '{k}'"));
                }
                if let Some(val) = eval(v, doc, vars, depth + 1)? {
                    out.insert(k.clone(), val);
                }
            }
            Ok(Some(Value::Object(out)))
        }
        other => Ok(Some(other.clone())),
    }
}

fn args(arg: &Value, doc: &Value, vars: &Vars, depth: usize) -> QResult<Vec<Value>> {
    match arg {
        Value::Array(items) => items
            .iter()
            .map(|i| Ok(eval(i, doc, vars, depth + 1)?.unwrap_or(Value::Null)))
            .collect(),
        v => Ok(vec![eval(v, doc, vars, depth + 1)?.unwrap_or(Value::Null)]),
    }
}

fn arith(name: &str, vals: &[Value]) -> QResult<Value> {
    if vals.iter().any(Value::is_null) {
        return Ok(Value::Null);
    }
    // Date arithmetic for $add / $subtract.
    let date_idx = vals.iter().position(|v| date_millis(v).is_some());
    let nums: Vec<f64> = vals
        .iter()
        .map(|v| {
            as_f64(v)
                .or_else(|| date_millis(v).map(|d| d as f64))
                .ok_or_else(|| {
                    format!(
                        "{name} only supports numeric or date types, not {}",
                        type_name(v)
                    )
                })
        })
        .collect::<QResult<_>>()?;
    let r = match name {
        "$add" => nums.iter().sum(),
        "$multiply" => nums.iter().product(),
        "$subtract" => {
            if nums.len() != 2 {
                return Err("Expression $subtract takes exactly 2 arguments.".to_owned());
            }
            let r = nums[0] - nums[1];
            if date_idx == Some(0) && date_millis(&vals[1]).is_none() {
                return Ok(date(r as i64));
            }
            return Ok(num(r));
        }
        "$divide" => {
            if nums.len() != 2 {
                return Err("Expression $divide takes exactly 2 arguments.".to_owned());
            }
            if nums[1] == 0.0 {
                return Err("can't $divide by zero".to_owned());
            }
            nums[0] / nums[1]
        }
        "$mod" => {
            if nums.len() != 2 || nums[1] == 0.0 {
                return Err("can't $mod by zero".to_owned());
            }
            nums[0] % nums[1]
        }
        "$pow" => nums
            .first()
            .copied()
            .unwrap_or(0.0)
            .powf(nums.get(1).copied().unwrap_or(1.0)),
        _ => unreachable!(),
    };
    if name == "$add" && date_idx.is_some() {
        return Ok(date(r as i64));
    }
    Ok(num(r))
}

fn to_string(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => Value::String(s.clone()),
        Value::Bool(b) => Value::String(b.to_string()),
        Value::Number(_) => Value::String(num(as_f64(v).unwrap_or(0.0)).to_string()),
        other => {
            if let Some(h) = ejson::oid_hex(other) {
                Value::String(h.to_owned())
            } else if let Some(ms) = date_millis(other) {
                Value::String(ejson::iso_millis(ms))
            } else if let Some(d) = other.get("$numberDecimal").and_then(Value::as_str) {
                Value::String(d.to_owned())
            } else {
                Value::String(other.to_string())
            }
        }
    }
}

fn date_parts(v: &Value) -> QResult<Option<time::OffsetDateTime>> {
    if v.is_null() {
        return Ok(None);
    }
    let ms = date_millis(v)
        .ok_or_else(|| format!("can't convert from BSON type {} to Date", type_name(v)))?;
    Ok(time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok())
}

fn format_date(fmt: &str, dt: time::OffsetDateTime) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{:04}", dt.year())),
            Some('m') => out.push_str(&format!("{:02}", u8::from(dt.month()))),
            Some('d') => out.push_str(&format!("{:02}", dt.day())),
            Some('H') => out.push_str(&format!("{:02}", dt.hour())),
            Some('M') => out.push_str(&format!("{:02}", dt.minute())),
            Some('S') => out.push_str(&format!("{:02}", dt.second())),
            Some('L') => out.push_str(&format!("{:03}", dt.millisecond())),
            Some('j') => out.push_str(&format!("{:03}", dt.ordinal())),
            Some('u') => out.push_str(&dt.weekday().number_from_monday().to_string()),
            Some('w') => out.push_str(&dt.weekday().number_days_from_sunday().to_string()),
            Some('z') => out.push_str("+0000"),
            Some('Z') => out.push('0'),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

fn operator(
    op: &str,
    arg: &Value,
    doc: &Value,
    vars: &Vars,
    depth: usize,
) -> QResult<Option<Value>> {
    let one = |a: &Value| -> QResult<Value> {
        let a = match a {
            Value::Array(items) if items.len() == 1 => &items[0],
            v => v,
        };
        Ok(eval(a, doc, vars, depth + 1)?.unwrap_or(Value::Null))
    };
    let v = match op {
        "$literal" => arg.clone(),
        "$add" | "$multiply" | "$subtract" | "$divide" | "$mod" | "$pow" => {
            arith(op, &args(arg, doc, vars, depth)?)?
        }
        "$abs" | "$ceil" | "$floor" | "$sqrt" | "$trunc" | "$round" => {
            let a = args(arg, doc, vars, depth)?;
            let Some(x) = a.first().and_then(as_f64) else {
                return Ok(Some(Value::Null));
            };
            let places = a.get(1).and_then(as_f64).unwrap_or(0.0) as i32;
            let scale = 10f64.powi(places);
            num(match op {
                "$abs" => x.abs(),
                "$ceil" => x.ceil(),
                "$floor" => x.floor(),
                "$sqrt" => x.sqrt(),
                "$trunc" => (x * scale).trunc() / scale,
                _ => {
                    // Banker's rounding, as MongoDB does.
                    let y = x * scale;
                    let r = y.round();
                    let r = if (y - y.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
                        r - y.signum()
                    } else {
                        r
                    };
                    r / scale
                }
            })
        }
        "$eq" | "$ne" | "$gt" | "$gte" | "$lt" | "$lte" | "$cmp" => {
            let a = args(arg, doc, vars, depth)?;
            if a.len() != 2 {
                return Err(format!(
                    "Expression {op} takes exactly 2 arguments. {} were passed in.",
                    a.len()
                ));
            }
            let o = cmp_values(&a[0], &a[1]);
            match op {
                "$eq" => json!(o == Ordering::Equal),
                "$ne" => json!(o != Ordering::Equal),
                "$gt" => json!(o == Ordering::Greater),
                "$gte" => json!(o != Ordering::Less),
                "$lt" => json!(o == Ordering::Less),
                "$lte" => json!(o != Ordering::Greater),
                _ => json!(match o {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                }),
            }
        }
        "$and" => json!(args(arg, doc, vars, depth)?.iter().all(truthy)),
        "$or" => json!(args(arg, doc, vars, depth)?.iter().any(truthy)),
        "$not" => json!(!truthy(&one(arg)?)),
        "$in" => {
            let a = args(arg, doc, vars, depth)?;
            let Some(Value::Array(items)) = a.get(1) else {
                return Err("$in requires an array as a second argument".to_owned());
            };
            json!(items.iter().any(|i| eq_values(i, &a[0])))
        }
        "$cond" => {
            let (c, t, e) = match arg {
                Value::Array(items) if items.len() == 3 => (&items[0], &items[1], &items[2]),
                Value::Object(m) => (
                    m.get("if").unwrap_or(&Value::Null),
                    m.get("then").unwrap_or(&Value::Null),
                    m.get("else").unwrap_or(&Value::Null),
                ),
                _ => return Err("Expression $cond takes exactly 3 arguments.".to_owned()),
            };
            let branch = if truthy(&eval(c, doc, vars, depth + 1)?.unwrap_or(Value::Null)) {
                t
            } else {
                e
            };
            return eval(branch, doc, vars, depth + 1);
        }
        "$ifNull" => {
            let items = arg.as_array().cloned().unwrap_or_else(|| vec![arg.clone()]);
            for (i, item) in items.iter().enumerate() {
                let v = eval(item, doc, vars, depth + 1)?;
                if i == items.len() - 1 {
                    return Ok(v);
                }
                if let Some(v) = v.filter(|v| !v.is_null()) {
                    return Ok(Some(v));
                }
            }
            Value::Null
        }
        "$switch" => {
            let branches = arg
                .get("branches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for b in branches {
                if truthy(
                    &eval(b.get("case").unwrap_or(&Value::Null), doc, vars, depth + 1)?
                        .unwrap_or(Value::Null),
                ) {
                    return eval(b.get("then").unwrap_or(&Value::Null), doc, vars, depth + 1);
                }
            }
            match arg.get("default") {
                Some(d) => return eval(d, doc, vars, depth + 1),
                None => return Err("$switch could not find a matching branch for an input, and no default was specified.".to_owned()),
            }
        }
        "$concat" => {
            let a = args(arg, doc, vars, depth)?;
            if a.iter().any(Value::is_null) {
                Value::Null
            } else {
                let mut s = String::new();
                for v in &a {
                    s.push_str(v.as_str().ok_or_else(|| {
                        format!("$concat only supports strings, not {}", type_name(v))
                    })?);
                }
                Value::String(s)
            }
        }
        "$toUpper" | "$toLower" | "$trim" | "$ltrim" | "$rtrim" | "$strLenCP" | "$strLenBytes" => {
            let input = if let (Some(m), true) = (arg.as_object(), op.ends_with("trim")) {
                eval(m.get("input").unwrap_or(&Value::Null), doc, vars, depth + 1)?
                    .unwrap_or(Value::Null)
            } else {
                one(arg)?
            };
            let s = match &input {
                Value::Null => String::new(),
                v => to_string(v).as_str().unwrap_or("").to_owned(),
            };
            match op {
                "$toUpper" => Value::String(s.to_uppercase()),
                "$toLower" => Value::String(s.to_lowercase()),
                "$trim" => Value::String(s.trim().to_owned()),
                "$ltrim" => Value::String(s.trim_start().to_owned()),
                "$rtrim" => Value::String(s.trim_end().to_owned()),
                "$strLenBytes" => json!(s.len()),
                _ => json!(s.chars().count()),
            }
        }
        "$substr" | "$substrCP" | "$substrBytes" => {
            let a = args(arg, doc, vars, depth)?;
            let s = to_string(a.first().unwrap_or(&Value::Null));
            let s = s.as_str().unwrap_or("");
            let start = a.get(1).and_then(as_f64).unwrap_or(0.0).max(0.0) as usize;
            let len = a.get(2).and_then(as_f64).unwrap_or(-1.0);
            let chars: Vec<char> = s.chars().collect();
            let end = if len < 0.0 {
                chars.len()
            } else {
                (start + len as usize).min(chars.len())
            };
            Value::String(
                chars
                    .get(start.min(chars.len())..end.max(start.min(chars.len())))
                    .map(|c| c.iter().collect())
                    .unwrap_or_default(),
            )
        }
        "$split" => {
            let a = args(arg, doc, vars, depth)?;
            match (a.first(), a.get(1)) {
                (Some(Value::String(s)), Some(Value::String(d))) => Value::Array(
                    s.split(d.as_str())
                        .map(|p| Value::String(p.to_owned()))
                        .collect(),
                ),
                (Some(Value::Null), _) => Value::Null,
                _ => return Err(
                    "$split requires an expression that evaluates to a string as a first argument"
                        .to_owned(),
                ),
            }
        }
        "$regexMatch" => {
            let input = eval(
                arg.get("input").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let pattern = arg.get("regex").and_then(Value::as_str).unwrap_or("");
            let options = arg.get("options").and_then(Value::as_str).unwrap_or("");
            let re = query::build_regex(pattern, options)?;
            json!(input.as_str().is_some_and(|s| re.is_match(s)))
        }
        "$toString" => to_string(&one(arg)?),
        "$toInt" | "$toLong" | "$toDouble" | "$toDecimal" => {
            let v = one(arg)?;
            match &v {
                Value::Null => Value::Null,
                Value::Bool(b) => json!(i32::from(*b)),
                Value::String(s) => match s.trim().parse::<f64>() {
                    Ok(f) => {
                        if op == "$toInt" || op == "$toLong" {
                            num(f.trunc())
                        } else {
                            num(f)
                        }
                    }
                    Err(_) => {
                        return Err(format!(
                            "Failed to parse number '{s}' in $convert with no onError value"
                        ));
                    }
                },
                v => match as_f64(v).or_else(|| date_millis(v).map(|d| d as f64)) {
                    Some(f) => {
                        if op == "$toInt" || op == "$toLong" {
                            num(f.trunc())
                        } else {
                            num(f)
                        }
                    }
                    None => {
                        return Err(format!(
                            "Unsupported conversion from {} to {} in $convert with no onError value",
                            type_name(v),
                            op[3..].to_lowercase()
                        ));
                    }
                },
            }
        }
        "$toBool" => json!(truthy(&one(arg)?)),
        "$toDate" => {
            let v = one(arg)?;
            match &v {
                Value::Null => Value::Null,
                Value::String(s) => date(
                    ejson::parse_iso_millis(s)
                        .ok_or_else(|| format!("Error parsing date string '{s}'"))?,
                ),
                Value::Number(_) => date(as_f64(&v).unwrap_or(0.0) as i64),
                v if date_millis(v).is_some() => v.clone(),
                v => {
                    return Err(format!(
                        "can't convert from BSON type {} to Date",
                        type_name(v)
                    ));
                }
            }
        }
        "$type" => {
            let v = eval(
                match arg {
                    Value::Array(a) if a.len() == 1 => &a[0],
                    v => v,
                },
                doc,
                vars,
                depth + 1,
            )?;
            Value::String(v.as_ref().map_or("missing", type_name).to_owned())
        }
        "$isArray" => json!(one(arg)?.is_array()),
        "$isNumber" => json!(ejson::is_number(&one(arg)?)),
        "$size" => match one(arg)? {
            Value::Array(items) => json!(items.len()),
            v => {
                return Err(format!(
                    "The argument to $size must be an array. Type of argument: {}",
                    match v {
                        Value::Null => "missing",
                        other => type_name(&other),
                    }
                ));
            }
        },
        "$arrayElemAt" => {
            let a = args(arg, doc, vars, depth)?;
            match (a.first(), a.get(1).and_then(as_f64)) {
                (Some(Value::Array(items)), Some(i)) => {
                    let idx = if i < 0.0 {
                        items.len() as i64 + i as i64
                    } else {
                        i as i64
                    };
                    match usize::try_from(idx).ok().and_then(|i| items.get(i)) {
                        Some(v) => v.clone(),
                        None => return Ok(None),
                    }
                }
                _ => Value::Null,
            }
        }
        "$first" | "$last" => match one(arg)? {
            Value::Array(items) => {
                let v = if op == "$first" {
                    items.first()
                } else {
                    items.last()
                };
                return Ok(v.cloned());
            }
            Value::Null => Value::Null,
            v => {
                return Err(format!(
                    "{op}'s argument must be an array, but is {}",
                    type_name(&v)
                ));
            }
        },
        "$concatArrays" => {
            let a = args(arg, doc, vars, depth)?;
            let mut out = Vec::new();
            for v in a {
                match v {
                    Value::Array(items) => out.extend(items),
                    Value::Null => return Ok(Some(Value::Null)),
                    other => {
                        return Err(format!(
                            "$concatArrays only supports arrays, not {}",
                            type_name(&other)
                        ));
                    }
                }
            }
            Value::Array(out)
        }
        "$slice" => {
            let a = args(arg, doc, vars, depth)?;
            match a.first() {
                Some(Value::Array(items)) => {
                    let n = a.get(1).and_then(as_f64).unwrap_or(0.0) as i64;
                    let (start, count) = if a.len() == 3 {
                        let s = if n < 0 {
                            (items.len() as i64 + n).max(0)
                        } else {
                            n
                        };
                        (
                            s as usize,
                            a.get(2).and_then(as_f64).unwrap_or(0.0) as usize,
                        )
                    } else if n < 0 {
                        (
                            (items.len() as i64 + n).max(0) as usize,
                            n.unsigned_abs() as usize,
                        )
                    } else {
                        (0, n as usize)
                    };
                    Value::Array(items.iter().skip(start).take(count).cloned().collect())
                }
                _ => Value::Null,
            }
        }
        "$filter" | "$map" => {
            let input = eval(
                arg.get("input").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let name = arg
                .get("as")
                .and_then(Value::as_str)
                .unwrap_or("this")
                .to_owned();
            let Value::Array(items) = input else {
                if input.is_null() {
                    return Ok(Some(Value::Null));
                }
                return Err(format!(
                    "input to {op} must be an array not {}",
                    type_name(&input)
                ));
            };
            let body = if op == "$filter" {
                arg.get("cond")
            } else {
                arg.get("in")
            }
            .unwrap_or(&Value::Null);
            let mut out = Vec::new();
            let limit = arg.get("limit").and_then(as_f64).map(|l| l as usize);
            for item in items {
                let mut vs = vars.clone();
                vs.insert(name.clone(), item.clone());
                let r = eval(body, doc, &vs, depth + 1)?.unwrap_or(Value::Null);
                if op == "$filter" {
                    if truthy(&r) {
                        out.push(item);
                        if limit.is_some_and(|l| out.len() >= l) {
                            break;
                        }
                    }
                } else {
                    out.push(r);
                }
            }
            Value::Array(out)
        }
        "$reduce" => {
            let input = eval(
                arg.get("input").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let mut acc = eval(
                arg.get("initialValue").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let body = arg.get("in").unwrap_or(&Value::Null);
            if let Value::Array(items) = input {
                for item in items {
                    let mut vs = vars.clone();
                    vs.insert("this".to_owned(), item);
                    vs.insert("value".to_owned(), acc);
                    acc = eval(body, doc, &vs, depth + 1)?.unwrap_or(Value::Null);
                }
                acc
            } else {
                Value::Null
            }
        }
        "$sum" | "$avg" | "$min" | "$max" => {
            let a = args(arg, doc, vars, depth)?;
            let vals: Vec<Value> = if a.len() == 1 {
                match &a[0] {
                    Value::Array(items) => items.clone(),
                    v => vec![v.clone()],
                }
            } else {
                a
            };
            let mut acc = Accumulator::new(op);
            for v in vals {
                acc.add(Some(v));
            }
            acc.finish()
        }
        "$mergeObjects" => {
            let a = args(arg, doc, vars, depth)?;
            let mut out = Map::new();
            for v in a.iter().flat_map(|v| match v {
                Value::Array(items) => items.clone(),
                v => vec![v.clone()],
            }) {
                if let Value::Object(m) = v {
                    for (k, val) in m {
                        out.insert(k, val);
                    }
                }
            }
            Value::Object(out)
        }
        "$objectToArray" => match one(arg)? {
            Value::Object(m) => Value::Array(
                m.into_iter()
                    .map(|(k, v)| json!({"k": k, "v": v}))
                    .collect(),
            ),
            _ => Value::Null,
        },
        "$arrayToObject" => match one(arg)? {
            Value::Array(items) => {
                let mut out = Map::new();
                for item in items {
                    match item {
                        Value::Object(m) => {
                            if let (Some(Value::String(k)), Some(v)) = (m.get("k"), m.get("v")) {
                                out.insert(k.clone(), v.clone());
                            }
                        }
                        Value::Array(pair) if pair.len() == 2 => {
                            if let Value::String(k) = &pair[0] {
                                out.insert(k.clone(), pair[1].clone());
                            }
                        }
                        _ => {}
                    }
                }
                Value::Object(out)
            }
            _ => Value::Null,
        },
        "$setUnion" | "$setIntersection" | "$setDifference" => {
            let a = args(arg, doc, vars, depth)?;
            let sets: Vec<Vec<Value>> = a
                .into_iter()
                .map(|v| v.as_array().cloned().unwrap_or_default())
                .collect();
            let mut out: Vec<Value> = Vec::new();
            match op {
                "$setUnion" => {
                    for s in &sets {
                        for v in s {
                            if !out.iter().any(|o| eq_values(o, v)) {
                                out.push(v.clone());
                            }
                        }
                    }
                }
                "$setIntersection" => {
                    if let Some(first) = sets.first() {
                        for v in first {
                            if sets.iter().all(|s| s.iter().any(|x| eq_values(x, v)))
                                && !out.iter().any(|o| eq_values(o, v))
                            {
                                out.push(v.clone());
                            }
                        }
                    }
                }
                _ => {
                    if let (Some(a), Some(b)) = (sets.first(), sets.get(1)) {
                        for v in a {
                            if !b.iter().any(|x| eq_values(x, v))
                                && !out.iter().any(|o| eq_values(o, v))
                            {
                                out.push(v.clone());
                            }
                        }
                    }
                }
            }
            Value::Array(out)
        }
        "$year" | "$month" | "$dayOfMonth" | "$hour" | "$minute" | "$second" | "$millisecond"
        | "$dayOfWeek" | "$dayOfYear" | "$week" | "$isoWeek" | "$isoDayOfWeek" => {
            let input = match arg {
                Value::Object(m) if m.contains_key("date") => m["date"].clone(),
                v => v.clone(),
            };
            let Some(dt) = date_parts(&one(&input)?)? else {
                return Ok(Some(Value::Null));
            };
            json!(match op {
                "$year" => dt.year(),
                "$month" => i32::from(u8::from(dt.month())),
                "$dayOfMonth" => i32::from(dt.day()),
                "$hour" => i32::from(dt.hour()),
                "$minute" => i32::from(dt.minute()),
                "$second" => i32::from(dt.second()),
                "$millisecond" => i32::from(dt.millisecond()),
                "$dayOfWeek" => i32::from(dt.weekday().number_from_sunday()),
                "$isoDayOfWeek" => i32::from(dt.weekday().number_from_monday()),
                "$dayOfYear" => i32::from(dt.ordinal()),
                "$isoWeek" => i32::from(dt.iso_week()),
                _ => i32::from(dt.sunday_based_week()),
            })
        }
        "$dateToString" => {
            let d = eval(
                arg.get("date").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let fmt = arg
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("%Y-%m-%dT%H:%M:%S.%LZ");
            match date_parts(&d)? {
                Some(dt) => Value::String(format_date(fmt, dt)),
                None => arg.get("onNull").cloned().unwrap_or(Value::Null),
            }
        }
        "$dateFromString" => {
            let s = eval(
                arg.get("dateString").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            match s.as_str().and_then(ejson::parse_iso_millis) {
                Some(ms) => date(ms),
                None => Value::Null,
            }
        }
        "$dateTrunc" => {
            let d = eval(
                arg.get("date").unwrap_or(&Value::Null),
                doc,
                vars,
                depth + 1,
            )?
            .unwrap_or(Value::Null);
            let unit = arg.get("unit").and_then(Value::as_str).unwrap_or("day");
            let Some(dt) = date_parts(&d)? else {
                return Ok(Some(Value::Null));
            };
            let base = dt.replace_nanosecond(0).unwrap_or(dt);
            let t = match unit {
                "year" => base
                    .replace_ordinal(1)
                    .ok()
                    .and_then(|x| x.replace_time(time::Time::MIDNIGHT).into()),
                "month" => base
                    .replace_day(1)
                    .ok()
                    .map(|x| x.replace_time(time::Time::MIDNIGHT)),
                "day" => Some(base.replace_time(time::Time::MIDNIGHT)),
                "hour" => base
                    .replace_minute(0)
                    .ok()
                    .and_then(|x| x.replace_second(0).ok()),
                "minute" => base.replace_second(0).ok(),
                _ => Some(base),
            }
            .unwrap_or(base);
            date((t.unix_timestamp_nanos() / 1_000_000) as i64)
        }
        "$dateDiff" => {
            let s = date_millis(
                &eval(
                    arg.get("startDate").unwrap_or(&Value::Null),
                    doc,
                    vars,
                    depth + 1,
                )?
                .unwrap_or(Value::Null),
            );
            let e = date_millis(
                &eval(
                    arg.get("endDate").unwrap_or(&Value::Null),
                    doc,
                    vars,
                    depth + 1,
                )?
                .unwrap_or(Value::Null),
            );
            let unit = arg.get("unit").and_then(Value::as_str).unwrap_or("day");
            match (s, e) {
                (Some(s), Some(e)) => {
                    let div = match unit {
                        "week" => 604_800_000,
                        "day" => 86_400_000,
                        "hour" => 3_600_000,
                        "minute" => 60_000,
                        "second" => 1000,
                        _ => 1,
                    };
                    json!((e.div_euclid(div)) - (s.div_euclid(div)))
                }
                _ => Value::Null,
            }
        }
        "$dateAdd" | "$dateSubtract" => {
            let s = date_millis(
                &eval(
                    arg.get("startDate").unwrap_or(&Value::Null),
                    doc,
                    vars,
                    depth + 1,
                )?
                .unwrap_or(Value::Null),
            );
            let amount = as_f64(
                &eval(
                    arg.get("amount").unwrap_or(&Value::Null),
                    doc,
                    vars,
                    depth + 1,
                )?
                .unwrap_or(Value::Null),
            )
            .unwrap_or(0.0) as i64;
            let unit = arg.get("unit").and_then(Value::as_str).unwrap_or("day");
            let mul = match unit {
                "week" => 604_800_000,
                "day" => 86_400_000,
                "hour" => 3_600_000,
                "minute" => 60_000,
                "second" => 1000,
                _ => 1,
            };
            match s {
                Some(s) => date(if op == "$dateAdd" {
                    s + amount * mul
                } else {
                    s - amount * mul
                }),
                None => Value::Null,
            }
        }
        "$let" => {
            let mut vs = vars.clone();
            if let Some(Value::Object(defs)) = arg.get("vars") {
                for (k, e) in defs {
                    vs.insert(
                        k.clone(),
                        eval(e, doc, vars, depth + 1)?.unwrap_or(Value::Null),
                    );
                }
            }
            return eval(arg.get("in").unwrap_or(&Value::Null), doc, &vs, depth + 1);
        }
        "$getField" => {
            let (field, input) = match arg {
                Value::String(s) => (s.clone(), doc.clone()),
                Value::Object(m) => (
                    m.get("field")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    match m.get("input") {
                        Some(i) => eval(i, doc, vars, depth + 1)?.unwrap_or(Value::Null),
                        None => doc.clone(),
                    },
                ),
                _ => return Ok(None),
            };
            return Ok(input.get(&field).cloned());
        }
        other => return Err(format!("Unrecognized expression '{other}'")),
    };
    Ok(Some(v))
}

/// `$group` accumulator state.
pub struct Accumulator {
    op: String,
    sum: f64,
    count: usize,
    all_int: bool,
    best: Option<Value>,
    items: Vec<Value>,
    first: Option<Option<Value>>,
    last: Option<Value>,
}

impl Accumulator {
    pub fn new(op: &str) -> Self {
        Self {
            op: op.to_owned(),
            sum: 0.0,
            count: 0,
            all_int: true,
            best: None,
            items: Vec::new(),
            first: None,
            last: None,
        }
    }

    pub fn add(&mut self, v: Option<Value>) {
        match self.op.as_str() {
            "$sum" | "$avg" => {
                if let Some(f) = v.as_ref().and_then(as_f64) {
                    self.sum += f;
                    self.count += 1;
                    if v.as_ref()
                        .is_some_and(|v| !matches!(v, Value::Number(n) if !n.is_f64()))
                    {
                        self.all_int = false;
                    }
                }
            }
            "$min" | "$max" => {
                if let Some(v) = v.filter(|v| !v.is_null()) {
                    let replace = match &self.best {
                        None => true,
                        Some(b) => {
                            let o = cmp_values(&v, b);
                            if self.op == "$min" {
                                o == Ordering::Less
                            } else {
                                o == Ordering::Greater
                            }
                        }
                    };
                    if replace {
                        self.best = Some(v);
                    }
                }
            }
            "$push" => self.items.push(v.unwrap_or(Value::Null)),
            "$addToSet" => {
                if let Some(v) = v
                    && !self.items.iter().any(|i| eq_values(i, &v))
                {
                    self.items.push(v);
                }
            }
            "$first" => {
                if self.first.is_none() {
                    self.first = Some(v);
                }
            }
            "$last" => self.last = v,
            "$count" => self.count += 1,
            "$mergeObjects" => {
                if let Some(Value::Object(m)) = v {
                    let mut base = self
                        .best
                        .take()
                        .unwrap_or_else(|| Value::Object(Map::new()));
                    if let Value::Object(b) = &mut base {
                        for (k, val) in m {
                            b.insert(k, val);
                        }
                    }
                    self.best = Some(base);
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Value {
        match self.op.as_str() {
            "$sum" => num(self.sum),
            "$avg" => {
                if self.count == 0 {
                    Value::Null
                } else {
                    num(self.sum / self.count as f64)
                }
            }
            "$min" | "$max" => self.best.unwrap_or(Value::Null),
            "$push" | "$addToSet" => Value::Array(self.items),
            "$first" => self.first.flatten().unwrap_or(Value::Null),
            "$last" => self.last.unwrap_or(Value::Null),
            "$count" => json!(self.count),
            "$mergeObjects" => self.best.unwrap_or_else(|| json!({})),
            _ => Value::Null,
        }
    }
}

const ACCUMULATORS: [&str; 10] = [
    "$sum",
    "$avg",
    "$min",
    "$max",
    "$push",
    "$addToSet",
    "$first",
    "$last",
    "$count",
    "$mergeObjects",
];

fn group(docs: Vec<Value>, spec: &Map<String, Value>, vars: &Vars) -> QResult<Vec<Value>> {
    let id_expr = spec
        .get("_id")
        .ok_or("a group specification must include an _id")?;
    let mut order: Vec<Value> = Vec::new();
    let mut groups: Vec<Vec<Accumulator>> = Vec::new();
    let fields: Vec<(&String, &str, &Value)> = spec
        .iter()
        .filter(|(k, _)| *k != "_id")
        .map(|(k, v)| {
            let m = v
                .as_object()
                .filter(|m| m.len() == 1)
                .ok_or_else(|| format!("The field '{k}' must be an accumulator object"))?;
            let (op, arg) = m.iter().next().expect("one entry");
            if !ACCUMULATORS.contains(&op.as_str()) {
                return Err(format!("unknown group operator '{op}'"));
            }
            Ok((k, op.as_str(), arg))
        })
        .collect::<QResult<_>>()?;
    for doc in &docs {
        let key = eval(id_expr, doc, vars, 0)?.unwrap_or(Value::Null);
        let idx = match order
            .iter()
            .position(|k| eq_values(k, &key) && ejson::type_rank(k) == ejson::type_rank(&key))
        {
            Some(i) => i,
            None => {
                order.push(key);
                groups.push(
                    fields
                        .iter()
                        .map(|(_, op, _)| Accumulator::new(op))
                        .collect(),
                );
                order.len() - 1
            }
        };
        for (acc, (_, _, arg)) in groups[idx].iter_mut().zip(&fields) {
            acc.add(eval(arg, doc, vars, 0)?);
        }
    }
    Ok(order
        .into_iter()
        .zip(groups)
        .map(|(key, accs)| {
            let mut out = Map::new();
            out.insert("_id".to_owned(), key);
            for (acc, (name, _, _)) in accs.into_iter().zip(&fields) {
                out.insert((*name).clone(), acc.finish());
            }
            Value::Object(out)
        })
        .collect())
}

/// `$project` / `find` projection.
pub fn project_stage(doc: &Value, spec: &Map<String, Value>) -> QResult<Value> {
    let vars = Vars::new();
    let is_exclusion_value = |v: &Value| matches!(v, Value::Bool(false)) || as_f64(v) == Some(0.0);
    let is_inclusion_value =
        |v: &Value| matches!(v, Value::Bool(true)) || as_f64(v).is_some_and(|f| f != 0.0);
    let non_id: Vec<(&String, &Value)> = spec.iter().filter(|(k, _)| *k != "_id").collect();
    let exclusion = !non_id.is_empty() && non_id.iter().all(|(_, v)| is_exclusion_value(v))
        || non_id.is_empty() && spec.get("_id").is_some_and(is_exclusion_value);
    if exclusion {
        let mut out = doc.clone();
        for (k, v) in spec {
            if is_exclusion_value(v) {
                query::unset_path(&mut out, k);
            }
        }
        return Ok(out);
    }
    if non_id.iter().any(|(_, v)| is_exclusion_value(v)) {
        return Err("Cannot do exclusion on field in inclusion projection".to_owned());
    }
    let mut out = Value::Object(Map::new());
    let include_id = !spec.get("_id").is_some_and(is_exclusion_value);
    if include_id {
        match spec.get("_id") {
            Some(v) if !is_inclusion_value(v) => {
                if let Some(val) = eval(v, doc, &vars, 0)? {
                    out["_id"] = val;
                }
            }
            _ => {
                if let Some(id) = doc.get("_id") {
                    out["_id"] = id.clone();
                }
            }
        }
    }
    for (k, v) in non_id {
        if is_inclusion_value(v) {
            include_path(doc, &mut out, k);
        } else if let Some(val) = eval(v, doc, &vars, 0)? {
            query::set_path(&mut out, k, val)?;
        }
    }
    Ok(out)
}

fn include_path(src: &Value, dst: &mut Value, path: &str) {
    let (head, rest) = path
        .split_once('.')
        .map_or((path, None), |(a, b)| (a, Some(b)));
    let Some(val) = src.get(head) else { return };
    match rest {
        None => {
            dst[head] = val.clone();
        }
        Some(rest) => match val {
            Value::Object(_) if !is_wrapper(val) => {
                if !dst.get(head).is_some_and(Value::is_object) {
                    dst[head] = Value::Object(Map::new());
                }
                include_path(val, &mut dst[head], rest);
            }
            Value::Array(items) => {
                let projected: Vec<Value> = items
                    .iter()
                    .filter(|i| i.is_object())
                    .map(|i| {
                        let mut o = Value::Object(Map::new());
                        include_path(i, &mut o, rest);
                        o
                    })
                    .collect();
                dst[head] = Value::Array(projected);
            }
            _ => {}
        },
    }
}

fn add_fields(doc: &Value, spec: &Map<String, Value>, vars: &Vars) -> QResult<Value> {
    let mut out = doc.clone();
    for (k, e) in spec {
        match eval(e, doc, vars, 0)? {
            Some(v) => query::set_path(&mut out, k, v)?,
            None => {
                query::unset_path(&mut out, k);
            }
        }
    }
    Ok(out)
}

fn unwind(docs: Vec<Value>, spec: &Value) -> QResult<Vec<Value>> {
    let (path, index_field, preserve) = match spec {
        Value::String(p) => (p.clone(), None, false),
        Value::Object(m) => (
            m.get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            m.get("includeArrayIndex")
                .and_then(Value::as_str)
                .map(str::to_owned),
            m.get("preserveNullAndEmptyArrays").is_some_and(truthy),
        ),
        _ => {
            return Err(
                "expected either a string or an object as specification for $unwind stage"
                    .to_owned(),
            );
        }
    };
    let Some(field) = path.strip_prefix('$') else {
        return Err("path option to $unwind stage should be prefixed with a '$'".to_owned());
    };
    let mut out = Vec::new();
    for doc in docs {
        match query::get_path(&doc, field).cloned() {
            Some(Value::Array(items)) if !items.is_empty() => {
                for (i, item) in items.into_iter().enumerate() {
                    let mut d = doc.clone();
                    query::set_path(&mut d, field, item)?;
                    if let Some(f) = &index_field {
                        query::set_path(&mut d, f, json!(i))?;
                    }
                    out.push(d);
                }
            }
            Some(Value::Array(_)) | Some(Value::Null) | None => {
                if preserve {
                    let mut d = doc;
                    if let Some(f) = &index_field {
                        query::set_path(&mut d, f, Value::Null)?;
                    }
                    out.push(d);
                }
            }
            Some(_) => {
                let mut d = doc;
                if let Some(f) = &index_field {
                    query::set_path(&mut d, f, Value::Null)?;
                }
                out.push(d);
            }
        }
    }
    Ok(out)
}

/// Runs `pipeline` over `docs`. A trailing `$out`/`$merge` is returned, not executed.
pub fn run(
    mut docs: Vec<Value>,
    pipeline: &[Value],
    src: &dyn Source,
) -> QResult<(Vec<Value>, Option<WriteStage>)> {
    let mut vars = Vars::new();
    vars.insert("NOW".to_owned(), json!(src.now_ms()));
    for (i, stage) in pipeline.iter().enumerate() {
        let Some(m) = stage.as_object().filter(|m| m.len() == 1) else {
            return Err(
                "A pipeline stage specification object must contain exactly one field.".to_owned(),
            );
        };
        let (name, spec) = m.iter().next().expect("one entry");
        docs = match name.as_str() {
            "$match" => {
                let mut out = Vec::new();
                for d in docs {
                    if query::matches(&d, spec)? {
                        out.push(d);
                    }
                }
                out
            }
            "$project" => {
                let spec = spec
                    .as_object()
                    .filter(|m| !m.is_empty())
                    .ok_or("$project specification must have at least one field")?;
                docs.iter()
                    .map(|d| project_stage(d, spec))
                    .collect::<QResult<_>>()?
            }
            "$addFields" | "$set" => {
                let spec = spec
                    .as_object()
                    .ok_or("$addFields specification stage must be an object")?;
                docs.iter()
                    .map(|d| add_fields(d, spec, &vars))
                    .collect::<QResult<_>>()?
            }
            "$unset" => {
                let fields: Vec<String> = match spec {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => return Err("$unset specification must be a string or an array".to_owned()),
                };
                docs.into_iter()
                    .map(|mut d| {
                        for f in &fields {
                            query::unset_path(&mut d, f);
                        }
                        d
                    })
                    .collect()
            }
            "$group" => group(
                docs,
                spec.as_object()
                    .ok_or("a group's fields must be specified in an object")?,
                &vars,
            )?,
            "$sort" => {
                query::sort_docs(&mut docs, spec)?;
                docs
            }
            "$limit" => {
                let n = as_f64(spec)
                    .filter(|n| *n > 0.0)
                    .ok_or("the limit must be positive")?;
                docs.into_iter().take(n as usize).collect()
            }
            "$skip" => {
                let n = as_f64(spec)
                    .filter(|n| *n >= 0.0)
                    .ok_or("invalid argument to $skip stage: Expected a non-negative number")?;
                docs.into_iter().skip(n as usize).collect()
            }
            "$count" => {
                let field = spec
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.starts_with('$'))
                    .ok_or("the count field must be a non-empty string")?;
                if docs.is_empty() {
                    vec![]
                } else {
                    vec![json!({ field: docs.len() })]
                }
            }
            "$sortByCount" => {
                let mut grouped = group(
                    docs,
                    json!({"_id": spec, "count": {"$sum": 1}})
                        .as_object()
                        .expect("object"),
                    &vars,
                )?;
                query::sort_docs(&mut grouped, &json!({"count": -1}))?;
                grouped
            }
            "$unwind" => unwind(docs, spec)?,
            "$replaceRoot" | "$replaceWith" => {
                let expr = if name == "$replaceRoot" {
                    spec.get("newRoot")
                        .ok_or("no newRoot specified for the $replaceRoot stage")?
                } else {
                    spec
                };
                docs.iter()
                    .map(|d| match eval(expr, d, &vars, 0)? {
                        Some(v @ Value::Object(_)) => Ok(v),
                        other => Err(format!("'newRoot' expression must evaluate to an object, but resulting value was: {}. Type of resulting value: '{}'.", other.clone().unwrap_or(Value::Null), other.as_ref().map_or("missing", type_name))),
                    })
                    .collect::<QResult<_>>()?
            }
            "$lookup" => lookup(docs, spec, src, &vars)?,
            "$unionWith" => {
                let (coll, pipe) = match spec {
                    Value::String(c) => (c.clone(), Vec::new()),
                    m => (
                        m.get("coll")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        m.get("pipeline")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                };
                let (other, _) = run(src.docs(&coll), &pipe, src)?;
                docs.extend(other);
                docs
            }
            "$facet" => {
                let spec = spec.as_object().ok_or("$facet requires an object")?;
                let mut out = Map::new();
                for (k, p) in spec {
                    let p = p.as_array().ok_or("$facet sub-pipeline must be an array")?;
                    let (r, _) = run(docs.clone(), p, src)?;
                    out.insert(k.clone(), Value::Array(r));
                }
                vec![Value::Object(out)]
            }
            "$bucket" => bucket(docs, spec, &vars)?,
            "$sample" => {
                let n = spec
                    .get("size")
                    .and_then(as_f64)
                    .ok_or("size argument to $sample must be a number")?;
                docs.into_iter().take(n as usize).collect()
            }
            "$documents" => {
                let v = eval(spec, &json!({}), &vars, 0)?.unwrap_or(Value::Null);
                v.as_array()
                    .cloned()
                    .ok_or("error during aggregation :: caused by :: an array is expected")?
            }
            "$currentOp" | "$listLocalSessions" | "$queryStats" | "$listSessions"
            | "$indexStats" | "$planCacheStats" | "$collStats" => {
                if i != 0 {
                    return Err(format!(
                        "{name} is only valid as the first stage in a pipeline"
                    ));
                }
                Vec::new()
            }
            "$out" | "$merge" => {
                if i != pipeline.len() - 1 {
                    return Err(format!(
                        "{name} can only be the final stage in the pipeline"
                    ));
                }
                let (db, coll) = match spec {
                    Value::String(c) => (None, c.clone()),
                    Value::Object(m) => {
                        let target = if name == "$merge" {
                            m.get("into").unwrap_or(spec)
                        } else {
                            spec
                        };
                        match target {
                            Value::String(c) => (None, c.clone()),
                            t => (
                                t.get("db").and_then(Value::as_str).map(str::to_owned),
                                t.get("coll")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned(),
                            ),
                        }
                    }
                    _ => return Err(format!("{name} requires a collection name")),
                };
                let stage = if name == "$out" {
                    WriteStage::Out { db, coll }
                } else {
                    WriteStage::Merge { db, coll }
                };
                return Ok((docs, Some(stage)));
            }
            "$search" | "$searchMeta" | "$vectorSearch" => {
                return Err("Atlas Search is not supported in this cluster.".to_owned());
            }
            other => return Err(format!("Unrecognized pipeline stage name: '{other}'")),
        };
    }
    Ok((docs, None))
}

fn lookup(docs: Vec<Value>, spec: &Value, src: &dyn Source, vars: &Vars) -> QResult<Vec<Value>> {
    let from = spec
        .get("from")
        .and_then(Value::as_str)
        .ok_or("must specify 'from' field for a $lookup")?;
    let as_field = spec
        .get("as")
        .and_then(Value::as_str)
        .ok_or("must specify 'as' field for a $lookup")?;
    let foreign = src.docs(from);
    let local_field = spec.get("localField").and_then(Value::as_str);
    let foreign_field = spec.get("foreignField").and_then(Value::as_str);
    let pipeline = spec.get("pipeline").and_then(Value::as_array);
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let mut matched: Vec<Value> = match (local_field, foreign_field) {
            (Some(lf), Some(ff)) => {
                let locals = query::resolve(&doc, lf);
                let mut local_vals: Vec<Value> = Vec::new();
                for l in locals {
                    match l {
                        Value::Array(items) => local_vals.extend(items),
                        v => local_vals.push(v),
                    }
                }
                if local_vals.is_empty() {
                    local_vals.push(Value::Null);
                }
                foreign
                    .iter()
                    .filter(|f| {
                        let fv = query::resolve(f, ff);
                        local_vals
                            .iter()
                            .any(|l| query::field_matches(&fv, &json!({"$eq": l})).unwrap_or(false))
                    })
                    .cloned()
                    .collect()
            }
            _ => foreign.clone(),
        };
        if let Some(p) = pipeline {
            let mut vs = vars.clone();
            if let Some(Value::Object(lets)) = spec.get("let") {
                for (k, e) in lets {
                    vs.insert(k.clone(), eval(e, &doc, vars, 0)?.unwrap_or(Value::Null));
                }
            }
            let resolved: Vec<Value> = p.iter().map(|stage| substitute_vars(stage, &vs)).collect();
            matched = run(matched, &resolved, src)?.0;
        }
        let mut d = doc;
        query::set_path(&mut d, as_field, Value::Array(matched))?;
        out.push(d);
    }
    Ok(out)
}

/// Replaces `$$var` references from a `$lookup` `let` with literal values.
fn substitute_vars(v: &Value, vars: &Vars) -> Value {
    match v {
        Value::String(s) if s.starts_with("$$") => {
            let name = &s[2..];
            let (var, rest) = name
                .split_once('.')
                .map_or((name, None), |(a, b)| (a, Some(b)));
            match (vars.get(var), var) {
                (Some(val), _) if var != "ROOT" && var != "CURRENT" && var != "NOW" => {
                    let val = match rest {
                        Some(r) => field_path(val, r).unwrap_or(Value::Null),
                        None => val.clone(),
                    };
                    json!({ "$literal": val })
                }
                _ => v.clone(),
            }
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| substitute_vars(i, vars)).collect())
        }
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), substitute_vars(v, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn bucket(docs: Vec<Value>, spec: &Value, vars: &Vars) -> QResult<Vec<Value>> {
    let group_by = spec
        .get("groupBy")
        .ok_or("$bucket requires 'groupBy' and 'boundaries' to be specified.")?;
    let bounds = spec
        .get("boundaries")
        .and_then(Value::as_array)
        .ok_or("$bucket requires 'groupBy' and 'boundaries' to be specified.")?;
    let default = spec.get("default");
    let output = spec
        .get("output")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            json!({"count": {"$sum": 1}})
                .as_object()
                .expect("object")
                .clone()
        });
    let mut keyed = Vec::new();
    for doc in docs {
        let v = eval(group_by, &doc, vars, 0)?.unwrap_or(Value::Null);
        let mut key = None;
        for w in bounds.windows(2) {
            if cmp_values(&v, &w[0]) != Ordering::Less && cmp_values(&v, &w[1]) == Ordering::Less {
                key = Some(w[0].clone());
                break;
            }
        }
        let key = match (key, default) {
            (Some(k), _) => k,
            (None, Some(d)) => d.clone(),
            (None, None) => return Err("$bucket could not find a matching branch for an input, and no default was specified.".to_owned()),
        };
        let mut d = doc;
        d["__bucket"] = key;
        keyed.push(d);
    }
    let mut spec_map = output;
    spec_map.insert("_id".to_owned(), json!("$__bucket"));
    let mut grouped = group(keyed, &spec_map, vars)?;
    grouped.sort_by(|a, b| cmp_values(&a["_id"], &b["_id"]));
    Ok(grouped)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Empty;
    impl Source for Empty {
        fn docs(&self, c: &str) -> Vec<Value> {
            if c == "customers" {
                vec![
                    json!({"_id": 1, "name": "Ada"}),
                    json!({"_id": 2, "name": "Bo"}),
                ]
            } else {
                vec![]
            }
        }
        fn now_ms(&self) -> i64 {
            0
        }
    }

    #[test]
    fn groups_sorts_and_looks_up() {
        let docs = vec![
            json!({"c": 1, "cat": "a", "total": 10, "items": [{"q": 1}, {"q": 2}]}),
            json!({"c": 2, "cat": "b", "total": 5.5, "items": []}),
            json!({"c": 1, "cat": "a", "total": 4, "items": [{"q": 3}]}),
        ];
        let (out, write) = run(
            docs.clone(),
            &[
                json!({"$group": {"_id": "$cat", "revenue": {"$sum": "$total"}, "n": {"$sum": 1}, "avg": {"$avg": "$total"}}}),
                json!({"$sort": {"revenue": -1}}),
            ],
            &Empty,
        )
        .unwrap();
        assert!(write.is_none());
        assert_eq!(
            out,
            vec![
                json!({"_id": "a", "revenue": 14, "n": 2, "avg": 7}),
                json!({"_id": "b", "revenue": 5.5, "n": 1, "avg": 5.5})
            ]
        );

        let (out, _) = run(
            docs.clone(),
            &[json!({"$unwind": "$items"}), json!({"$count": "n"})],
            &Empty,
        )
        .unwrap();
        assert_eq!(out, vec![json!({"n": 3})]);

        let (out, _) = run(
            docs,
            &[
                json!({"$lookup": {"from": "customers", "localField": "c", "foreignField": "_id", "as": "cust"}}),
                json!({"$project": {"_id": 0, "name": {"$first": "$cust.name"}, "big": {"$gt": ["$total", 5]}}}),
                json!({"$limit": 1}),
            ],
            &Empty,
        )
        .unwrap();
        assert_eq!(out, vec![json!({"name": "Ada", "big": true})]);
    }

    #[test]
    fn date_expressions() {
        let d = json!({"at": {"$date": "2026-02-14T10:30:00.000Z"}});
        assert_eq!(eval_root(&json!({"$month": "$at"}), &d).unwrap(), json!(2));
        assert_eq!(
            eval_root(
                &json!({"$dateToString": {"format": "%Y-%m", "date": "$at"}}),
                &d
            )
            .unwrap(),
            json!("2026-02")
        );
    }
}
