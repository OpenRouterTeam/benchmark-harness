//! Decoder for the `application/vnd.kubernetes.protobuf` request bodies client-go's typed
//! clients send. Covers the kinds that kubernetes-mcp-server and the Helm SDK write through
//! typed clients: access reviews, Secrets, ConfigMaps and DeleteOptions.

use serde_json::{Map, Value, json};

use crate::util::b64_encode;

const MAGIC: &[u8] = b"k8s\0";

#[derive(Debug, Clone)]
enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut out = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*pos)?;
        *pos += 1;
        out |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(out);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn fields(buf: &[u8]) -> Option<Vec<(u64, Field<'_>)>> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < buf.len() {
        let tag = varint(buf, &mut pos)?;
        let (num, wire) = (tag >> 3, tag & 7);
        let f = match wire {
            0 => Field::Varint(varint(buf, &mut pos)?),
            1 => {
                pos += 8;
                Field::Fixed
            }
            2 => {
                let len = varint(buf, &mut pos)? as usize;
                let b = buf.get(pos..pos + len)?;
                pos += len;
                Field::Bytes(b)
            }
            5 => {
                pos += 4;
                Field::Fixed
            }
            _ => return None,
        };
        out.push((num, f));
    }
    Some(out)
}

fn string(f: &Field<'_>) -> String {
    match f {
        Field::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

fn get<'b, 'a>(fs: &'b [(u64, Field<'a>)], num: u64) -> Option<&'b Field<'a>> {
    fs.iter().find(|(n, _)| *n == num).map(|(_, f)| f)
}

fn sub<'a>(fs: &[(u64, Field<'a>)], num: u64) -> Vec<(u64, Field<'a>)> {
    match get(fs, num) {
        Some(Field::Bytes(b)) => fields(b).unwrap_or_default(),
        _ => vec![],
    }
}

fn str_field(fs: &[(u64, Field<'_>)], num: u64) -> Option<String> {
    get(fs, num).map(string).filter(|s| !s.is_empty())
}

/// `map<string, string>` (or bytes, base64-encoded when `b64`) entries with field `num`.
fn map_field(fs: &[(u64, Field<'_>)], num: u64, b64: bool) -> Option<Value> {
    let mut m = Map::new();
    for (n, f) in fs {
        if *n != num {
            continue;
        }
        if let Field::Bytes(entry) = f {
            let e = fields(entry)?;
            let key = get(&e, 1).map(string).unwrap_or_default();
            let value = match get(&e, 2) {
                Some(Field::Bytes(v)) if b64 => b64_encode(v),
                Some(v) => string(v),
                None => String::new(),
            };
            m.insert(key, json!(value));
        }
    }
    (!m.is_empty()).then_some(Value::Object(m))
}

fn time(fs: &[(u64, Field<'_>)], num: u64) -> Option<Value> {
    let t = sub(fs, num);
    match get(&t, 1) {
        Some(Field::Varint(secs)) => Some(json!(mock_core::clock::format_unix(*secs as i64))),
        _ if get(fs, num).is_some() => Some(Value::Null),
        _ => None,
    }
}

fn object_meta(fs: &[(u64, Field<'_>)]) -> Value {
    let mut m = Map::new();
    let s = |k: &str, n: u64, m: &mut Map<String, Value>| {
        if let Some(v) = str_field(fs, n) {
            m.insert(k.into(), json!(v));
        }
    };
    s("name", 1, &mut m);
    s("generateName", 2, &mut m);
    s("namespace", 3, &mut m);
    s("uid", 5, &mut m);
    s("resourceVersion", 6, &mut m);
    if let Some(Field::Varint(g)) = get(fs, 7) {
        m.insert("generation".into(), json!(g));
    }
    if let Some(t) = time(fs, 8).filter(|t| !t.is_null()) {
        m.insert("creationTimestamp".into(), t);
    }
    if let Some(l) = map_field(fs, 11, false) {
        m.insert("labels".into(), l);
    }
    if let Some(a) = map_field(fs, 12, false) {
        m.insert("annotations".into(), a);
    }
    let finalizers: Vec<Value> = fs
        .iter()
        .filter(|(n, _)| *n == 14)
        .map(|(_, f)| json!(string(f)))
        .collect();
    if !finalizers.is_empty() {
        m.insert("finalizers".into(), json!(finalizers));
    }
    Value::Object(m)
}

/// Decodes a protobuf-encoded object to its JSON form, or `None` for unsupported kinds.
pub fn decode(body: &[u8]) -> Option<Value> {
    let rest = body.strip_prefix(MAGIC)?;
    let unknown = fields(rest)?;
    let type_meta = sub(&unknown, 1);
    let api_version = str_field(&type_meta, 1).unwrap_or_default();
    let kind = str_field(&type_meta, 2).unwrap_or_default();
    let raw = match get(&unknown, 2) {
        Some(Field::Bytes(b)) => fields(b)?,
        _ => vec![],
    };
    let meta = object_meta(&sub(&raw, 1));
    let mut obj = json!({"apiVersion": api_version, "kind": kind, "metadata": meta});
    match kind.as_str() {
        "SelfSubjectAccessReview" | "SubjectAccessReview" | "LocalSubjectAccessReview" => {
            let spec = sub(&raw, 2);
            let ra = sub(&spec, 1);
            let mut attrs = Map::new();
            for (k, n) in [
                ("namespace", 1),
                ("verb", 2),
                ("group", 3),
                ("version", 4),
                ("resource", 5),
                ("subresource", 6),
                ("name", 7),
            ] {
                if let Some(v) = str_field(&ra, n) {
                    attrs.insert(k.into(), json!(v));
                }
            }
            let mut s = json!({});
            if !ra.is_empty() {
                s["resourceAttributes"] = Value::Object(attrs);
            }
            let nra = sub(&spec, 2);
            if !nra.is_empty() {
                s["nonResourceAttributes"] =
                    json!({"path": str_field(&nra, 1), "verb": str_field(&nra, 2)});
            }
            if kind != "SelfSubjectAccessReview"
                && let Some(u) = str_field(&spec, 3)
            {
                s["user"] = json!(u);
            }
            obj["spec"] = s;
        }
        "SelfSubjectRulesReview" => {
            let spec = sub(&raw, 2);
            obj["spec"] = json!({"namespace": str_field(&spec, 1).unwrap_or_default()});
        }
        "SelfSubjectReview" => {}
        "TokenReview" => {
            let spec = sub(&raw, 2);
            obj["spec"] = json!({"token": str_field(&spec, 1).unwrap_or_default()});
        }
        "Secret" => {
            if let Some(d) = map_field(&raw, 2, true) {
                obj["data"] = d;
            }
            if let Some(t) = str_field(&raw, 3) {
                obj["type"] = json!(t);
            }
            if let Some(d) = map_field(&raw, 4, false) {
                obj["stringData"] = d;
            }
            if let Some(Field::Varint(i)) = get(&raw, 5) {
                obj["immutable"] = json!(*i != 0);
            }
        }
        "ConfigMap" => {
            if let Some(d) = map_field(&raw, 2, false) {
                obj["data"] = d;
            }
            if let Some(d) = map_field(&raw, 3, true) {
                obj["binaryData"] = d;
            }
            if let Some(Field::Varint(i)) = get(&raw, 4) {
                obj["immutable"] = json!(*i != 0);
            }
        }
        "DeleteOptions" => {
            let mut o = json!({"kind": "DeleteOptions", "apiVersion": api_version});
            if let Some(Field::Varint(g)) = get(&raw, 1) {
                o["gracePeriodSeconds"] = json!(g);
            }
            if let Some(p) = str_field(&raw, 4) {
                o["propagationPolicy"] = json!(p);
            }
            return Some(o);
        }
        _ => return None,
    }
    Some(obj)
}

/// True for protobuf request bodies.
pub fn is_protobuf(content_type: &str, body: &[u8]) -> bool {
    content_type.starts_with("application/vnd.kubernetes.protobuf") || body.starts_with(MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(num: u64, wire: u64) -> u8 {
        ((num << 3) | wire) as u8
    }

    fn bytes_field(num: u64, b: &[u8]) -> Vec<u8> {
        let mut out = vec![tag(num, 2), b.len() as u8];
        out.extend_from_slice(b);
        out
    }

    #[test]
    fn decodes_ssar() {
        let ra = [
            bytes_field(1, b"payments"),
            bytes_field(2, b"list"),
            bytes_field(4, b"v1"),
            bytes_field(5, b"pods"),
        ]
        .concat();
        let spec = bytes_field(1, &ra);
        let raw = [bytes_field(1, &[]), bytes_field(2, &spec)].concat();
        let tm = [
            bytes_field(1, b"authorization.k8s.io/v1"),
            bytes_field(2, b"SelfSubjectAccessReview"),
        ]
        .concat();
        let body = [MAGIC.to_vec(), bytes_field(1, &tm), bytes_field(2, &raw)].concat();
        let v = decode(&body).unwrap();
        assert_eq!(v["kind"], "SelfSubjectAccessReview");
        assert_eq!(v["spec"]["resourceAttributes"]["namespace"], "payments");
        assert_eq!(v["spec"]["resourceAttributes"]["resource"], "pods");
    }

    #[test]
    fn decodes_secret_with_labels_and_data() {
        let label = [bytes_field(1, b"owner"), bytes_field(2, b"helm")].concat();
        let meta = [
            bytes_field(1, b"s1"),
            bytes_field(3, b"perf"),
            bytes_field(11, &label),
        ]
        .concat();
        let data = [bytes_field(1, b"release"), bytes_field(2, b"abc")].concat();
        let raw = [
            bytes_field(1, &meta),
            bytes_field(2, &data),
            bytes_field(3, b"helm.sh/release.v1"),
        ]
        .concat();
        let tm = [bytes_field(1, b"v1"), bytes_field(2, b"Secret")].concat();
        let body = [MAGIC.to_vec(), bytes_field(1, &tm), bytes_field(2, &raw)].concat();
        let v = decode(&body).unwrap();
        assert_eq!(v["metadata"]["name"], "s1");
        assert_eq!(v["metadata"]["labels"]["owner"], "helm");
        assert_eq!(v["data"]["release"], "YWJj");
        assert_eq!(v["type"], "helm.sh/release.v1");
    }
}
