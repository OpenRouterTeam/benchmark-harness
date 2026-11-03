//! Small helpers: argument access, base64 cursors, Slack-style IDs.

use mock_core::world::ServiceState;
use serde_json::Value;

pub fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Array of strings, also accepting a single string or a comma-separated string.
pub fn arg_strings(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .filter(|s| !s.trim().is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => {
            // A JSON-encoded array passed as a string.
            if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(s) {
                return items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
            }
            vec![s.clone()]
        }
        _ => Vec::new(),
    }
}

pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let clean: Vec<u8> = text
        .trim()
        .bytes()
        .filter(|c| *c != b'=')
        .map(|c| match c {
            b'-' => b'+',
            b'_' => b'/',
            other => other,
        })
        .collect();
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = B64.iter().position(|x| x == c)? as u32;
            n |= v << (18 - 6 * i);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

pub fn cursor(kind: &str, value: impl std::fmt::Display) -> String {
    b64(format!("{kind}:{value}").as_bytes())
}

/// Decodes a cursor made by [`cursor`] with the same `kind`.
pub fn parse_cursor(text: &str, kind: &str) -> Option<String> {
    let raw = String::from_utf8(b64_decode(text)?).ok()?;
    raw.strip_prefix(&format!("{kind}:")).map(str::to_owned)
}

/// Slack-style encoded ID: `prefix` + uppercase alphanumerics, deterministic per kind.
pub fn slack_id(state: &mut ServiceState, kind: &str, prefix: &str, len: usize) -> String {
    let raw = state.ids.next_prefixed(kind, "", len * 2);
    let mut out = String::from(prefix);
    for c in raw.chars() {
        if out.len() == prefix.len() + len {
            break;
        }
        let c = c.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        }
    }
    while out.len() < prefix.len() + len {
        out.push('0');
    }
    out
}

pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    let flat: String = text.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_real_cursors() {
        assert_eq!(cursor("CURRENT_PAGE", 2), "Q1VSUkVOVF9QQUdFOjI=");
        assert_eq!(
            parse_cursor("Q1VSUkVOVF9QQUdFOjI=", "CURRENT_PAGE").as_deref(),
            Some("2")
        );
        assert_eq!(
            cursor("next_ts", "1793391046592639").trim_end_matches('='),
            "bmV4dF90czoxNzkzMzkxMDQ2NTkyNjM5"
        );
        assert_eq!(
            parse_cursor("bmV4dF90czoxNzkzMzkxMDQ2NTkyNjM5", "next_ts").as_deref(),
            Some("1793391046592639")
        );
    }
}
