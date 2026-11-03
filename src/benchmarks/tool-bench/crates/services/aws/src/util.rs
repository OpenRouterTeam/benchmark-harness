//! Protocol plumbing shared by the AWS service emulations: URL encoding, query-protocol
//! parameters, timestamps, a small XML writer/reader, and response builders.

use std::collections::BTreeMap;

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

// ───────────────────────────── URL encoding ─────────────────────────────

pub fn percent_decode(s: &str, plus_as_space: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' if plus_as_space => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or(())
                {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// RFC 3986 unreserved characters stay; everything else is `%XX` (as IAM encodes policies).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `a=1&b=2` into ordered pairs.
pub fn parse_pairs(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(k, true), percent_decode(v, true)),
            None => (percent_decode(p, true), String::new()),
        })
        .collect()
}

/// Query-protocol parameters (`Action=...&UserName=...&PolicyArns.member.1=...`).
#[derive(Debug, Clone, Default)]
pub struct Params(pub BTreeMap<String, String>);

impl Params {
    pub fn parse(body: &str) -> Self {
        Self(parse_pairs(body).into_iter().collect())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn str(&self, key: &str) -> String {
        self.get(key).unwrap_or_default().to_owned()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| v.eq_ignore_ascii_case("true"))
    }

    /// `<prefix>.1`, `<prefix>.2`, ... in numeric order.
    pub fn list(&self, prefix: &str) -> Vec<String> {
        let mut items = Vec::new();
        for n in 1.. {
            match self.get(&format!("{prefix}.{n}")) {
                Some(v) => items.push(v.to_owned()),
                None => break,
            }
        }
        items
    }

    /// Indexes `n` for which some key starts with `<prefix>.<n>.`.
    pub fn indexes(&self, prefix: &str) -> Vec<usize> {
        let mut out = Vec::new();
        for n in 1.. {
            let p = format!("{prefix}.{n}.");
            if self
                .0
                .range(p.clone()..)
                .next()
                .is_some_and(|(k, _)| k.starts_with(&p))
            {
                out.push(n);
            } else {
                break;
            }
        }
        out
    }
}

// ───────────────────────────── Time ─────────────────────────────

pub fn iso(secs: i64) -> String {
    mock_core::clock::format_unix(secs)
}

/// ISO 8601 with milliseconds, as IAM/S3 XML timestamps look (`2026-03-14T09:00:00.000Z`).
pub fn iso_ms(secs: i64) -> String {
    let s = iso(secs);
    format!("{}.000Z", s.trim_end_matches('Z'))
}

/// Parses RFC 3339 / ISO 8601 (with or without fraction/offset), epoch seconds, or
/// epoch seconds with fraction. Returns unix seconds.
pub fn parse_time(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<f64>() {
        // Values this large are milliseconds.
        return Some(if n > 1e11 {
            (n / 1000.0) as i64
        } else {
            n as i64
        });
    }
    if let Ok(dt) = OffsetDateTime::parse(t, &Rfc3339) {
        return Some(dt.unix_timestamp());
    }
    // `2026-03-14T09:00:00` (no zone) or with a space separator.
    let normalized = t.replace(' ', "T");
    let with_zone = if normalized.ends_with('Z') || normalized[10..].contains(['+', '-']) {
        normalized
    } else {
        format!("{normalized}Z")
    };
    OffsetDateTime::parse(&with_zone, &Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

/// Dataset timestamps are ISO strings or epoch numbers.
pub fn value_time(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| {
            if f > 1e11 {
                (f / 1000.0) as i64
            } else {
                f as i64
            }
        }),
        Value::String(s) => parse_time(s),
        _ => None,
    }
}

// ───────────────────────────── XML writing ─────────────────────────────

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

pub fn el(name: &str, inner: &str) -> String {
    format!("<{name}>{inner}</{name}>")
}

pub fn tel(name: &str, text: &str) -> String {
    el(name, &esc(text))
}

/// Serializes JSON into AWS XML: object keys become elements, arrays wrap each item in
/// `item_name` (`member` for query services, `item` for EC2), nulls are omitted.
pub fn to_xml(v: &Value, item_name: &str) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => esc(s),
        Value::Array(items) => items
            .iter()
            .map(|i| el(item_name, &to_xml(i, item_name)))
            .collect(),
        Value::Object(map) => map
            .iter()
            .filter(|(k, v)| !v.is_null() && !k.starts_with('_'))
            .map(|(k, v)| el(k, &to_xml(v, item_name)))
            .collect(),
    }
}

// ───────────────────────────── XML reading ─────────────────────────────

/// A parsed XML element (attributes and namespaces are ignored).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn all(&self, name: &str) -> Vec<&Node> {
        self.children.iter().filter(|c| c.name == name).collect()
    }

    pub fn text_of(&self, name: &str) -> Option<String> {
        self.child(name).map(|c| c.text.trim().to_owned())
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parses an XML document into its root element. Good enough for AWS request bodies.
pub fn parse_xml(text: &str) -> Option<Node> {
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        let chunk = &rest[..lt];
        if let Some(top) = stack.last_mut() {
            top.text.push_str(&unescape(chunk));
        }
        rest = &rest[lt..];
        if rest.starts_with("<?") {
            rest = &rest[rest.find("?>")? + 2..];
            continue;
        }
        if rest.starts_with("<!--") {
            rest = &rest[rest.find("-->")? + 3..];
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&rest[9..end]);
            }
            rest = &rest[end + 3..];
            continue;
        }
        let gt = rest.find('>')?;
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let node = stack.pop()?;
            if local(name.trim()) != node.name {
                return None;
            }
            stack.last_mut()?.children.push(node);
        } else {
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let name = local(tag.split_whitespace().next().unwrap_or_default()).to_owned();
            let node = Node {
                name,
                ..Node::default()
            };
            if self_closing {
                stack.last_mut()?.children.push(node);
            } else {
                stack.push(node);
            }
        }
    }
    if stack.len() != 1 {
        return None;
    }
    stack.pop()?.children.into_iter().next()
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

// ───────────────────────────── Responses ─────────────────────────────

pub fn respond(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response {
    let mut resp = Response::new(body.into());
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).expect("content type is ASCII"),
    );
    resp
}

pub fn with_header(mut resp: Response, name: &'static str, value: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(value) {
        resp.headers_mut().insert(name, v);
    }
    resp
}

pub fn xml_response(status: StatusCode, body: String) -> Response {
    respond(
        status,
        "text/xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{body}"),
    )
}

pub fn json_response(status: StatusCode, content_type: &str, body: &Value) -> Response {
    respond(status, content_type, body.to_string())
}

// ───────────────────────────── JSON helpers ─────────────────────────────

pub fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

pub fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Shell-style glob with `*` and `?` (IAM resources, CloudWatch prefixes).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi].eq_ignore_ascii_case(&t[ti]) && p[pi] != '*') {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decodes_and_encodes() {
        assert_eq!(percent_decode("a%20b+c", true), "a b c");
        assert_eq!(percent_decode("100%", false), "100%");
        assert_eq!(percent_encode("{\"a\": 1}"), "%7B%22a%22%3A%201%7D");
    }

    #[test]
    fn params_lists_and_indexes() {
        let p = Params::parse("Action=X&A.member.1=a&A.member.2=b&Filter.1.Name=n&Filter.2.Name=m");
        assert_eq!(p.list("A.member"), ["a", "b"]);
        assert_eq!(p.indexes("Filter"), [1, 2]);
    }

    #[test]
    fn xml_round_trip() {
        let x = to_xml(
            &json!({"A": [{"B": "x&y"}], "C": null, "D": true}),
            "member",
        );
        assert_eq!(x, "<A><member><B>x&amp;y</B></member></A><D>true</D>");
        let n = parse_xml(
            "<?xml version=\"1.0\"?><Root xmlns=\"x\"><A>1</A><B><C>2</C><C>3</C></B><E/></Root>",
        )
        .unwrap();
        assert_eq!(n.name, "Root");
        assert_eq!(n.text_of("A").unwrap(), "1");
        assert_eq!(n.child("B").unwrap().all("C").len(), 2);
        assert!(n.child("E").is_some());
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("2026-03-14T09:00:00Z"), Some(1_773_478_800));
        assert_eq!(parse_time("2026-03-14T09:00:00+00:00"), Some(1_773_478_800));
        assert_eq!(parse_time("2026-03-14T09:00:00"), Some(1_773_478_800));
        assert_eq!(parse_time("2026-03-14T09:00:00.123Z"), Some(1_773_478_800));
        assert_eq!(parse_time("1773478800"), Some(1_773_478_800));
        assert_eq!(parse_time("1773478800000"), Some(1_773_478_800));
        assert_eq!(iso_ms(1_773_478_800), "2026-03-14T09:00:00.000Z");
    }

    #[test]
    fn globs() {
        assert!(glob_match("s3:Get*", "s3:GetObject"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("s3:Put*", "s3:GetObject"));
        assert!(glob_match(
            "arn:aws:s3:::bucket/*",
            "arn:aws:s3:::bucket/a/b"
        ));
    }
}
