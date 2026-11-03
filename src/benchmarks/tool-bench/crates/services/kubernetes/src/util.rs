//! Small self-contained helpers: base64, RFC 3339 parsing, kubectl-style durations,
//! deterministic UIDs and name suffixes, JSON merge helpers.

use serde_json::{Map, Value};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
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
    let mut values = Vec::with_capacity(text.len());
    for c in text.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        values.push(B64.iter().position(|b| *b == c)? as u32);
    }
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` to unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        rest = &r[end..];
    }
    let offset = if rest == "Z" || rest == "z" {
        0
    } else if rest.len() == 6 {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let oh = rest.get(1..3)?.parse::<i64>().ok()?;
        let om = rest.get(4..6)?.parse::<i64>().ok()?;
        sign * (oh * 3600 + om * 60)
    } else {
        return None;
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// kubectl's `duration.HumanDuration`.
pub fn human_duration(secs: i64) -> String {
    if secs < -1 {
        return "<invalid>".into();
    }
    if secs < 0 {
        return "0s".into();
    }
    if secs < 120 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 10 {
        let s = secs % 60;
        return if s == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m{s}s")
        };
    }
    if minutes < 180 {
        return format!("{minutes}m");
    }
    let hours = secs / 3600;
    if hours < 8 {
        let m = minutes % 60;
        return if m == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{m}m")
        };
    }
    if hours < 48 {
        return format!("{hours}h");
    }
    if hours < 24 * 8 {
        let h = hours % 24;
        return if h == 0 {
            format!("{}d", hours / 24)
        } else {
            format!("{}d{h}h", hours / 24)
        };
    }
    if hours < 24 * 365 * 2 {
        return format!("{}d", hours / 24);
    }
    if hours < 24 * 365 * 8 {
        let dy = (hours / 24) % 365;
        return if dy == 0 {
            format!("{}y", hours / 24 / 365)
        } else {
            format!("{}y{dy}d", hours / 24 / 365)
        };
    }
    format!("{}y", hours / 24 / 365)
}

/// 64-bit FNV-1a.
pub fn fnv(data: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A stable UUID-shaped identifier derived from `seed`.
pub fn stable_uid(seed: &str) -> String {
    let a = fnv(seed);
    let b = fnv(&format!("{seed}#uid"));
    let hex = format!("{a:016x}{b:016x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// Kubernetes' `rand.String` alphabet (no vowels, no confusable characters).
const NAME_ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";

pub fn name_suffix(seed: &str, len: usize) -> String {
    let mut h = fnv(seed);
    let mut out = String::with_capacity(len);
    for i in 0..len {
        if i % 8 == 7 {
            h = fnv(&format!("{seed}{i}"));
        }
        out.push(NAME_ALPHABET[(h % NAME_ALPHABET.len() as u64) as usize] as char);
        h /= NAME_ALPHABET.len() as u64;
    }
    out
}

/// RFC 7386 JSON merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Map::new());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Strategic merge / server-side apply approximation: objects merge recursively, lists of
/// objects that all carry a `name` (containers, env, volumes, ports) merge by name, other
/// lists are replaced. `null` deletes a key.
pub fn strategic_merge(target: &mut Value, patch: &Value) {
    match (target.is_object(), patch) {
        (_, Value::Object(p)) => {
            if !target.is_object() {
                *target = Value::Object(Map::new());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if k.starts_with("$") {
                    continue;
                }
                if v.is_null() {
                    t.remove(k);
                    continue;
                }
                let slot = t.entry(k.clone()).or_insert(Value::Null);
                if let (Value::Array(existing), Value::Array(incoming)) = (&mut *slot, v)
                    && keyed_by_name(incoming)
                    && keyed_by_name(existing)
                {
                    for item in incoming {
                        let name = &item["name"];
                        match existing.iter_mut().find(|e| &e["name"] == name) {
                            Some(e) => strategic_merge(e, item),
                            None => existing.push(item.clone()),
                        }
                    }
                    continue;
                }
                strategic_merge(slot, v);
            }
        }
        (_, other) => *target = other.clone(),
    }
}

fn keyed_by_name(items: &[Value]) -> bool {
    items
        .iter()
        .all(|i| i.get("name").is_some_and(Value::is_string))
}

/// RFC 6902 JSON patch. Returns an error message on failure.
pub fn json_patch(target: &mut Value, ops: &Value) -> Result<(), String> {
    let ops = ops.as_array().ok_or("json patch must be an array")?;
    for op in ops {
        let kind = op["op"].as_str().ok_or("missing op")?;
        let path = op["path"].as_str().ok_or("missing path")?;
        match kind {
            "add" | "replace" => set_pointer(target, path, op["value"].clone(), kind == "add")?,
            "remove" => {
                remove_pointer(target, path)?;
            }
            "test" => {
                if target.pointer(path) != Some(&op["value"]) {
                    return Err(format!("test failed at {path}"));
                }
            }
            "copy" | "move" => {
                let from = op["from"].as_str().ok_or("missing from")?;
                let value = target
                    .pointer(from)
                    .cloned()
                    .ok_or_else(|| format!("missing value at {from}"))?;
                if kind == "move" {
                    remove_pointer(target, from)?;
                }
                set_pointer(target, path, value, true)?;
            }
            other => return Err(format!("unknown op {other}")),
        }
    }
    Ok(())
}

fn split_pointer(path: &str) -> Result<(String, String), String> {
    let idx = path.rfind('/').ok_or_else(|| format!("bad path {path}"))?;
    let last = path[idx + 1..].replace("~1", "/").replace("~0", "~");
    Ok((path[..idx].to_owned(), last))
}

fn set_pointer(target: &mut Value, path: &str, value: Value, add: bool) -> Result<(), String> {
    if path.is_empty() {
        *target = value;
        return Ok(());
    }
    let (parent, last) = split_pointer(path)?;
    let parent = target
        .pointer_mut(&parent)
        .ok_or_else(|| format!("doc is missing path: \"{path}\""))?;
    match parent {
        Value::Object(map) => {
            if !add && !map.contains_key(&last) {
                return Err(format!(
                    "replace operation does not apply: doc is missing key: {path}"
                ));
            }
            map.insert(last, value);
        }
        Value::Array(items) => {
            if last == "-" {
                items.push(value);
            } else {
                let i: usize = last.parse().map_err(|_| format!("bad index {last}"))?;
                if add && i <= items.len() {
                    items.insert(i, value);
                } else if i < items.len() {
                    items[i] = value;
                } else {
                    return Err(format!("index out of range: {path}"));
                }
            }
        }
        _ => return Err(format!("doc is missing path: \"{path}\"")),
    }
    Ok(())
}

fn remove_pointer(target: &mut Value, path: &str) -> Result<Value, String> {
    let (parent, last) = split_pointer(path)?;
    let parent = target
        .pointer_mut(&parent)
        .ok_or_else(|| format!("doc is missing path: \"{path}\""))?;
    match parent {
        Value::Object(map) => map.remove(&last).ok_or_else(|| {
            format!("remove operation does not apply: doc is missing path: \"{path}\"")
        }),
        Value::Array(items) => {
            let i: usize = last.parse().map_err(|_| format!("bad index {last}"))?;
            if i < items.len() {
                Ok(items.remove(i))
            } else {
                Err(format!("index out of range: {path}"))
            }
        }
        _ => Err(format!("doc is missing path: \"{path}\"")),
    }
}

/// Percent-decodes a query component (`+` is a space).
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(q: Option<&str>) -> Vec<(String, String)> {
    q.unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(p), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn base64_round_trips() {
        for s in ["", "a", "ab", "abc", "abcd", "hello world"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(b64_encode(b"ab"), "YWI=");
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-03-14T09:00:00Z"), Some(1_773_478_800));
        assert_eq!(
            parse_rfc3339("2026-03-14T10:00:00+01:00"),
            Some(1_773_478_800)
        );
        assert_eq!(
            parse_rfc3339("2026-03-14T09:00:00.123Z"),
            Some(1_773_478_800)
        );
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn human_durations_match_kubectl() {
        assert_eq!(human_duration(30), "30s");
        assert_eq!(human_duration(150), "2m30s");
        assert_eq!(human_duration(3600), "60m");
        assert_eq!(human_duration(5 * 3600 + 120), "5h2m");
        assert_eq!(human_duration(20 * 3600), "20h");
        assert_eq!(human_duration(3 * 86400 + 3600), "3d1h");
        assert_eq!(human_duration(30 * 86400), "30d");
    }

    #[test]
    fn strategic_merge_merges_named_lists() {
        let mut t =
            json!({"containers": [{"name": "a", "image": "x"}, {"name": "b", "image": "y"}]});
        strategic_merge(
            &mut t,
            &json!({"containers": [{"name": "b", "resources": {"limits": {"memory": "1Gi"}}}]}),
        );
        assert_eq!(t["containers"][1]["image"], "y");
        assert_eq!(t["containers"][1]["resources"]["limits"]["memory"], "1Gi");
        assert_eq!(t["containers"][0]["image"], "x");
    }

    #[test]
    fn json_patch_ops() {
        let mut t = json!({"spec": {"replicas": 1, "list": [1, 2]}});
        json_patch(
            &mut t,
            &json!([
                {"op": "replace", "path": "/spec/replicas", "value": 4},
                {"op": "add", "path": "/spec/list/-", "value": 3},
                {"op": "remove", "path": "/spec/list/0"}
            ]),
        )
        .unwrap();
        assert_eq!(t, json!({"spec": {"replicas": 4, "list": [2, 3]}}));
        assert!(
            json_patch(
                &mut t,
                &json!([{"op": "replace", "path": "/nope", "value": 1}])
            )
            .is_err()
        );
    }

    #[test]
    fn suffixes_are_dns_safe_and_stable() {
        let a = name_suffix("x", 5);
        assert_eq!(a, name_suffix("x", 5));
        assert_eq!(a.len(), 5);
        assert!(a.bytes().all(|b| NAME_ALPHABET.contains(&b)));
        assert_ne!(a, name_suffix("y", 5));
    }
}
