//! Small private helpers: calendar math, Salesforce Ids, percent decoding and a minimal XML
//! reader for the Metadata API envelopes jsforce sends.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------------------------

pub const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `YYYY-MM-DD` to days since the epoch.
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y = digits(&s[0..4])?;
    let m = digits(&s[5..7])? as u32;
    let d = digits(&s[8..10])? as u32;
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// ISO 8601 date-time (`2026-03-14T09:00:00Z`, `...00.123Z`, `...+0000`, `...+01:00`) to
/// epoch milliseconds.
pub fn parse_datetime(s: &str) -> Option<i64> {
    if s.len() < 19 || s.as_bytes()[10] != b'T' {
        return None;
    }
    let day = parse_date(&s[..10])?;
    let t = &s[11..];
    let b = t.as_bytes();
    if b.len() < 8 || b[2] != b':' || b[5] != b':' {
        return None;
    }
    let h = digits(&t[0..2])?;
    let mi = digits(&t[3..5])?;
    let se = digits(&t[6..8])?;
    if h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &t[8..];
    let mut ms = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        let mut digits3 = frac[..n].to_owned();
        digits3.truncate(3);
        while digits3.len() < 3 {
            digits3.push('0');
        }
        ms = digits3.parse().ok()?;
        rest = &frac[n..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let r = rest[1..].replace(':', "");
            if r.len() != 4 {
                return None;
            }
            sign * (digits(&r[0..2])? * 60 + digits(&r[2..4])?)
        }
    };
    Some(day * DAY_MS + ((h * 60 + mi) * 60 + se) * 1000 + ms - offset_min * 60_000)
}

pub fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Salesforce REST date-time format: `2026-03-14T09:00:00.000+0000`.
pub fn format_datetime(ms: i64) -> String {
    let days = ms.div_euclid(DAY_MS);
    let rem = ms.rem_euclid(DAY_MS);
    let (h, mi, s, milli) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    format!(
        "{}T{h:02}:{mi:02}:{s:02}.{milli:03}+0000",
        format_date(days)
    )
}

// ---------------------------------------------------------------------------------------
// Salesforce Ids
// ---------------------------------------------------------------------------------------

const SUFFIX_CHARS: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The case-safe 18-character form of a 15-character Id.
pub fn id18(id15: &str) -> String {
    let mut out = id15.to_owned();
    for chunk in id15.as_bytes().chunks(5) {
        let mut bits = 0usize;
        for (i, c) in chunk.iter().enumerate() {
            if c.is_ascii_uppercase() {
                bits |= 1 << i;
            }
        }
        out.push(SUFFIX_CHARS[bits] as char);
    }
    out
}

/// A 15- or 18-character alphanumeric Id normalized to its 18-character form.
pub fn normalize_id(id: &str) -> Option<String> {
    if !(id.len() == 15 || id.len() == 18) || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let full = id18(&id[..15]);
    if id.len() == 18 && !full[15..].eq_ignore_ascii_case(&id[15..]) {
        return None;
    }
    Some(full)
}

/// The `n`th Id minted by the mock for a key prefix: `<prefix>8c00000<serial>` + suffix.
pub fn minted_id(prefix: &str, n: u64) -> String {
    let mut serial = Vec::new();
    let mut v = n;
    for _ in 0..5 {
        serial.push(BASE62[(v % 62) as usize]);
        v /= 62;
    }
    serial.reverse();
    let mut id15 = String::with_capacity(15);
    id15.push_str(&prefix[..3.min(prefix.len())]);
    id15.push_str("8c00000");
    id15.push_str(std::str::from_utf8(&serial).expect("ascii"));
    id18(&id15)
}

// ---------------------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------------------

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Query string pairs, `+` as space, percent-decoded.
pub fn query_pairs(query: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        out.insert(
            percent_decode(&k.replace('+', " ")),
            percent_decode(&v.replace('+', " ")),
        );
    }
    out
}

// ---------------------------------------------------------------------------------------
// XML (just enough for SOAP envelopes)
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct XmlNode {
    /// Local name, namespace prefix removed.
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<XmlNode>,
    pub text: String,
}

impl XmlNode {
    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub fn is_nil(&self) -> bool {
        self.attrs
            .iter()
            .any(|(k, v)| k.ends_with("nil") && v == "true")
    }

    /// `xsi:type` without its prefix.
    pub fn xsi_type(&self) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.as_str() == "xsi:type" || k.ends_with(":type") || *k == "type")
            .map(|(_, v)| v.rsplit(':').next().unwrap_or(v))
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_owned()
}

pub fn unescape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16)
                .ok()
                .and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parses a document into its root element. Comments, processing instructions and
/// DOCTYPEs are skipped; CDATA is kept as text.
pub fn parse_xml(src: &str) -> Result<XmlNode, String> {
    let mut stack: Vec<XmlNode> = vec![XmlNode::default()];
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            if src[i..].starts_with("<?") {
                i = src[i..]
                    .find("?>")
                    .map(|e| i + e + 2)
                    .ok_or("unterminated PI")?;
            } else if src[i..].starts_with("<!--") {
                i = src[i..]
                    .find("-->")
                    .map(|e| i + e + 3)
                    .ok_or("unterminated comment")?;
            } else if src[i..].starts_with("<![CDATA[") {
                let end = src[i..].find("]]>").ok_or("unterminated CDATA")?;
                let text = &src[i + 9..i + end];
                stack.last_mut().expect("root").text.push_str(text);
                i += end + 3;
            } else if src[i..].starts_with("<!") {
                i = src[i..]
                    .find('>')
                    .map(|e| i + e + 1)
                    .ok_or("bad declaration")?;
            } else if src[i..].starts_with("</") {
                let end = src[i..].find('>').ok_or("unterminated end tag")? + i;
                let name = local(src[i + 2..end].trim());
                let node = stack.pop().ok_or("unbalanced")?;
                if node.name != name || stack.is_empty() {
                    return Err(format!("mismatched end tag </{name}>"));
                }
                stack.last_mut().expect("parent").children.push(node);
                i = end + 1;
            } else {
                let end = find_tag_end(src, i).ok_or("unterminated tag")?;
                let inner = &src[i + 1..end];
                let self_closing = inner.ends_with('/');
                let inner = inner.trim_end_matches('/');
                let (name, attrs) = parse_tag(inner)?;
                let node = XmlNode {
                    name: local(&name),
                    attrs,
                    ..XmlNode::default()
                };
                if self_closing {
                    stack.last_mut().expect("parent").children.push(node);
                } else {
                    stack.push(node);
                }
                i = end + 1;
            }
        } else {
            let end = src[i..].find('<').map_or(b.len(), |e| i + e);
            let text = unescape_xml(&src[i..end]);
            stack.last_mut().expect("node").text.push_str(&text);
            i = end;
        }
    }
    if stack.len() != 1 {
        return Err("unclosed elements".to_owned());
    }
    let mut root = stack.pop().expect("root");
    if root.children.len() != 1 {
        return Err("expected one root element".to_owned());
    }
    Ok(root.children.remove(0))
}

fn find_tag_end(src: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (off, c) in src.as_bytes()[start..].iter().enumerate() {
        match (quote, *c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(*c),
            (None, b'>') => return Some(start + off),
            _ => {}
        }
    }
    None
}

fn parse_tag(inner: &str) -> Result<(String, BTreeMap<String, String>), String> {
    let inner = inner.trim();
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_owned();
    if name.is_empty() {
        return Err("empty tag name".to_owned());
    }
    let mut attrs = BTreeMap::new();
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or("attribute without value")?;
        let key = rest[..eq].trim().to_owned();
        let after = rest[eq + 1..].trim_start();
        let q = after.chars().next().ok_or("attribute without value")?;
        if q != '"' && q != '\'' {
            return Err("unquoted attribute".to_owned());
        }
        let close = after[1..].find(q).ok_or("unterminated attribute")? + 1;
        attrs.insert(key, unescape_xml(&after[1..close]));
        rest = after[close + 1..].trim_start();
    }
    Ok((name, attrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round_trips() {
        for (y, m, d) in [(1970, 1, 1), (2026, 3, 14), (2024, 2, 29), (2000, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(parse_date("2026-02-30"), None);
    }

    #[test]
    fn datetimes_parse_and_format() {
        let ms = parse_datetime("2026-03-14T09:00:07.123Z").unwrap();
        assert_eq!(format_datetime(ms), "2026-03-14T09:00:07.123+0000");
        assert_eq!(
            parse_datetime("2026-03-14T10:00:07+01:00"),
            parse_datetime("2026-03-14T09:00:07.000+0000")
        );
    }

    #[test]
    fn ids_get_case_safe_suffix() {
        assert_eq!(id18("001D000000IqhSL"), "001D000000IqhSLIAZ");
        assert_eq!(
            normalize_id("001D000000IqhSL").as_deref(),
            Some("001D000000IqhSLIAZ")
        );
        assert_eq!(
            normalize_id("001D000000IqhSLiaz").unwrap(),
            "001D000000IqhSLIAZ"
        );
        assert!(normalize_id("001D000000IqhSLXXX").is_none());
        assert!(normalize_id("abc").is_none());
        assert_eq!(&minted_id("00T", 1)[..15], "00T8c0000000001");
    }

    #[test]
    fn xml_parses_envelopes() {
        let doc = r#"<?xml version="1.0"?><s:Envelope xmlns:s="x"><s:Body><m:create><metadata xsi:type="CustomObject"><fullName>A__c</fullName><label>A &amp; B</label><x/></metadata></m:create></s:Body></s:Envelope>"#;
        let root = parse_xml(doc).unwrap();
        assert_eq!(root.name, "Envelope");
        let md = root
            .child("Body")
            .unwrap()
            .child("create")
            .unwrap()
            .child("metadata")
            .unwrap();
        assert_eq!(md.xsi_type(), Some("CustomObject"));
        assert_eq!(md.child("label").unwrap().text, "A & B");
        assert!(md.child("x").is_some());
    }
}
