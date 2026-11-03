//! A small re-implementation of the zod 3 checks the pinned server runs on tool arguments,
//! producing the same issue objects (and so the same `ZodError` message text).

use serde_json::{Map, Value, json};

pub enum Kind {
    /// `z.string()` with optional `.min(n, message)` and `.max(n)`.
    Str {
        min: Option<(usize, Option<&'static str>)>,
        max: Option<usize>,
    },
    Num,
    /// `z.number().int()`.
    Int,
    /// `z.number().int().positive()`.
    PosInt,
    Bool,
    Enum(&'static [&'static str]),
    /// `z.array(z.enum(...))`.
    EnumArray(&'static [&'static str]),
    /// `z.array(z.object(fields)).min(1)`.
    ObjectArray(&'static [Field]),
    /// `z.object(fields)`.
    Object(&'static [Field]),
}

pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub optional: bool,
}

pub const fn req(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        optional: false,
    }
}

pub const fn opt(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        optional: true,
    }
}

pub const STR: Kind = Kind::Str {
    min: None,
    max: None,
};

pub const fn str_min(message: &'static str) -> Kind {
    Kind::Str {
        min: Some((1, Some(message))),
        max: None,
    }
}

/// zod's `getParsedType` name for a JSON value.
pub fn type_name(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn path_with(path: &[Value], last: Value) -> Vec<Value> {
    let mut p = path.to_vec();
    p.push(last);
    p
}

fn invalid_type(expected: &str, received: &str, path: Vec<Value>) -> Value {
    let message = if received == "undefined" {
        "Required".to_owned()
    } else {
        format!("Expected {expected}, received {received}")
    };
    json!({
        "code": "invalid_type",
        "expected": expected,
        "received": received,
        "path": path,
        "message": message,
    })
}

fn too_small(min: usize, ty: &str, custom: Option<&str>, path: Vec<Value>) -> Value {
    let mut m = Map::new();
    m.insert("code".into(), json!("too_small"));
    m.insert("minimum".into(), json!(min));
    m.insert("type".into(), json!(ty));
    m.insert("inclusive".into(), json!(true));
    m.insert("exact".into(), json!(false));
    match custom {
        Some(msg) => {
            m.insert("message".into(), json!(msg));
            m.insert("path".into(), json!(path));
        }
        None => {
            m.insert("path".into(), json!(path));
            let unit = if ty == "array" {
                "element(s)"
            } else {
                "character(s)"
            };
            let what = if ty == "array" { "Array" } else { "String" };
            m.insert(
                "message".into(),
                json!(format!("{what} must contain at least {min} {unit}")),
            );
        }
    }
    Value::Object(m)
}

fn enum_list(options: &[&str]) -> String {
    options
        .iter()
        .map(|o| format!("'{o}'"))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn check_enum(v: Option<&Value>, options: &[&str], path: Vec<Value>, issues: &mut Vec<Value>) {
    match v {
        Some(Value::String(s)) => {
            if !options.contains(&s.as_str()) {
                issues.push(json!({
                    "received": s,
                    "code": "invalid_enum_value",
                    "options": options,
                    "path": path,
                    "message": format!("Invalid enum value. Expected {}, received '{s}'", enum_list(options)),
                }));
            }
        }
        other => issues.push(invalid_type(&enum_list(options), type_name(other), path)),
    }
}

fn check_value(v: Option<&Value>, kind: &Kind, path: Vec<Value>, issues: &mut Vec<Value>) {
    match kind {
        Kind::Str { min, max } => match v {
            Some(Value::String(s)) => {
                let len = s.encode_utf16().count();
                if let Some((n, msg)) = min
                    && len < *n
                {
                    issues.push(too_small(*n, "string", *msg, path.clone()));
                }
                if let Some(n) = max
                    && len > *n
                {
                    issues.push(json!({
                        "code": "too_big",
                        "maximum": n,
                        "type": "string",
                        "inclusive": true,
                        "exact": false,
                        "path": path,
                        "message": format!("String must contain at most {n} character(s)"),
                    }));
                }
            }
            other => issues.push(invalid_type("string", type_name(other), path)),
        },
        Kind::Num | Kind::Int | Kind::PosInt => match v {
            Some(Value::Number(n)) => {
                let f = n.as_f64().unwrap_or(0.0);
                if matches!(kind, Kind::Int | Kind::PosInt) && f.fract() != 0.0 {
                    issues.push(invalid_type("integer", "float", path.clone()));
                }
                if matches!(kind, Kind::PosInt) && f <= 0.0 {
                    issues.push(json!({
                        "code": "too_small",
                        "minimum": 0,
                        "type": "number",
                        "inclusive": false,
                        "exact": false,
                        "path": path,
                        "message": "Number must be greater than 0",
                    }));
                }
            }
            other => issues.push(invalid_type("number", type_name(other), path)),
        },
        Kind::Bool => {
            if !matches!(v, Some(Value::Bool(_))) {
                issues.push(invalid_type("boolean", type_name(v), path));
            }
        }
        Kind::Enum(options) => check_enum(v, options, path, issues),
        Kind::EnumArray(options) => match v {
            Some(Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    check_enum(Some(item), options, path_with(&path, json!(i)), issues);
                }
            }
            other => issues.push(invalid_type("array", type_name(other), path)),
        },
        Kind::ObjectArray(fields) => match v {
            Some(Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    check_object(item, fields, &path_with(&path, json!(i)), issues);
                }
                if items.is_empty() {
                    issues.push(too_small(1, "array", None, path));
                }
            }
            other => issues.push(invalid_type("array", type_name(other), path)),
        },
        Kind::Object(fields) => check_object(v.unwrap_or(&Value::Null), fields, &path, issues),
    }
}

fn check_object(v: &Value, fields: &[Field], path: &[Value], issues: &mut Vec<Value>) {
    let Value::Object(obj) = v else {
        issues.push(invalid_type("object", type_name(Some(v)), path.to_vec()));
        return;
    };
    for f in fields {
        let value = obj.get(f.name);
        if value.is_none() && f.optional {
            continue;
        }
        check_value(value, &f.kind, path_with(path, json!(f.name)), issues);
    }
}

/// The issues `z.object(fields).safeParse(args)` reports, in zod's order.
pub fn issues(args: &Value, fields: &[Field]) -> Vec<Value> {
    let mut out = Vec::new();
    check_object(args, fields, &[], &mut out);
    out
}

/// `ZodError.message`: the issues pretty-printed with two-space indentation.
pub fn parse(args: &Value, fields: &[Field]) -> Result<(), String> {
    let found = issues(args, fields);
    if found.is_empty() {
        Ok(())
    } else {
        Err(serde_json::to_string_pretty(&Value::Array(found)).expect("issues serialize"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_string_matches_zod_text() {
        let err = parse(&json!({}), &[req("idOrUrl", STR)]).unwrap_err();
        assert_eq!(
            err,
            "[\n  {\n    \"code\": \"invalid_type\",\n    \"expected\": \"string\",\n    \"received\": \"undefined\",\n    \"path\": [\n      \"idOrUrl\"\n    ],\n    \"message\": \"Required\"\n  }\n]"
        );
    }

    #[test]
    fn custom_min_message_comes_before_path() {
        let found = issues(
            &json!({"name": ""}),
            &[req("name", str_min("Project name is required"))],
        );
        let keys: Vec<&String> = found[0].as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "code",
                "minimum",
                "type",
                "inclusive",
                "exact",
                "message",
                "path"
            ]
        );
    }

    #[test]
    fn nested_array_paths_and_enum_messages() {
        const ITEM: &[Field] = &[req("target", Kind::EnumArray(&["production", "preview"]))];
        let found = issues(
            &json!({"vars": [{"target": ["prod"]}]}),
            &[req("vars", Kind::ObjectArray(ITEM))],
        );
        assert_eq!(found[0]["path"], json!(["vars", 0, "target", 0]));
        assert_eq!(
            found[0]["message"],
            "Invalid enum value. Expected 'production' | 'preview', received 'prod'"
        );
    }
}
