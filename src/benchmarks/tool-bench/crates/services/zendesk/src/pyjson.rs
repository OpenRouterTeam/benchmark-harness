//! Python `json.dumps` output (default `ensure_ascii=True`), which the replicated server uses
//! to print every tool result. `indent: None` gives `", "` / `": "` separators on one line;
//! `Some(n)` gives Python's indented layout (`","` at line ends, `"[]"`/`"{}"` when empty).

use serde_json::Value;

pub fn dumps(value: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent, 0);
    out
}

fn write_value(out: &mut String, value: &Value, indent: Option<usize>, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                separator(out, i, indent, level + 1);
                write_value(out, item, indent, level + 1);
            }
            close(out, indent, level);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                separator(out, i, indent, level + 1);
                write_string(out, k);
                out.push_str(": ");
                write_value(out, v, indent, level + 1);
            }
            close(out, indent, level);
            out.push('}');
        }
    }
}

fn separator(out: &mut String, index: usize, indent: Option<usize>, level: usize) {
    match indent {
        None => {
            if index > 0 {
                out.push_str(", ");
            }
        }
        Some(n) => {
            if index > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(n * level));
        }
    }
}

fn close(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn compact_uses_python_separators() {
        let v =
            json!({"error": "RecordNotFound", "description": "Not found", "n": [1, null, true]});
        assert_eq!(
            dumps(&v, None),
            r#"{"error": "RecordNotFound", "description": "Not found", "n": [1, null, true]}"#
        );
    }

    #[test]
    fn indented_matches_python_layout() {
        let v = json!({"a": [], "b": {}, "c": [1, {"d": "x"}]});
        assert_eq!(
            dumps(&v, Some(2)),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": \"x\"\n    }\n  ]\n}"
        );
    }

    #[test]
    fn escapes_non_ascii_like_ensure_ascii() {
        assert_eq!(
            dumps(&json!("caf\u{e9} \u{1F600}\n\"q\"\u{7f}"), None),
            r#""caf\u00e9 \ud83d\ude00\n\"q\"\u007f""#
        );
    }
}
