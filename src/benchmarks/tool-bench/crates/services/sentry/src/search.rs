//! Sentry search syntax (`is:unresolved level:error !release:1.0 times_seen:>10 "free text"`),
//! shared by issue search and the events (Discover) endpoints.

use serde_json::Value;

use crate::util::{parse_period, parse_time, value_time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// `key:value`, `!key:value`, `key:[a,b]`, `key:>10`.
    Filter {
        negated: bool,
        key: String,
        op: Op,
        values: Vec<String>,
    },
    /// `has:key` / `!has:key`.
    Has { negated: bool, key: String },
    /// Free text (a bare word or quoted phrase).
    Text { negated: bool, text: String },
}

/// Splits on whitespace outside quotes and brackets.
fn tokenize(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = false;
    let mut bracket = 0usize;
    let mut escaped = false;
    for c in query.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                current.push(c);
                escaped = true;
            }
            '"' => {
                quote = !quote;
                current.push(c);
            }
            '[' if !quote => {
                bracket += 1;
                current.push(c);
            }
            ']' if !quote => {
                bracket = bracket.saturating_sub(1);
                current.push(c);
            }
            c if c.is_whitespace() && !quote && bracket == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    inner.replace("\\\"", "\"")
}

/// Index of the `:` separating key and value, ignoring colons inside quotes or brackets
/// (e.g. `tags[foo:bar]:x`).
fn key_split(token: &str) -> Option<usize> {
    let mut quote = false;
    let mut bracket = 0usize;
    let mut paren = 0usize;
    for (i, c) in token.char_indices() {
        match c {
            '"' => quote = !quote,
            '[' if !quote => bracket += 1,
            ']' if !quote => bracket = bracket.saturating_sub(1),
            '(' if !quote => paren += 1,
            ')' if !quote => paren = paren.saturating_sub(1),
            ':' if !quote && bracket == 0 && paren == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

pub fn parse(query: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    for token in tokenize(query) {
        if token.eq_ignore_ascii_case("AND") || token.eq_ignore_ascii_case("OR") {
            continue;
        }
        let (negated, body) = match token.strip_prefix('!') {
            Some(rest) if !rest.is_empty() => (true, rest.to_owned()),
            _ => (false, token.clone()),
        };
        let split = if body.starts_with('"') {
            None
        } else {
            key_split(&body)
        };
        match split {
            // A key needs a value and a key-like name; otherwise (`TypeError:`) it is text.
            Some(i)
                if i > 0
                    && i + 1 < body.len()
                    && body[..i].chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '[' | ']')
                    }) =>
            {
                let key = body[..i].to_owned();
                let raw = &body[i + 1..];
                if key == "has" {
                    terms.push(Term::Has {
                        negated,
                        key: unquote(raw),
                    });
                    continue;
                }
                let (op, rest) = if let Some(r) = raw.strip_prefix(">=") {
                    (Op::Gte, r)
                } else if let Some(r) = raw.strip_prefix("<=") {
                    (Op::Lte, r)
                } else if let Some(r) = raw.strip_prefix('>') {
                    (Op::Gt, r)
                } else if let Some(r) = raw.strip_prefix('<') {
                    (Op::Lt, r)
                } else {
                    (Op::Eq, raw)
                };
                let values = if rest.starts_with('[') && rest.ends_with(']') {
                    rest[1..rest.len() - 1]
                        .split(',')
                        .map(unquote)
                        .filter(|v| !v.is_empty())
                        .collect()
                } else {
                    vec![unquote(rest)]
                };
                terms.push(Term::Filter {
                    negated,
                    key,
                    op,
                    values,
                });
            }
            _ => terms.push(Term::Text {
                negated,
                text: unquote(&body),
            }),
        }
    }
    terms
}

/// `*` wildcard match, case-insensitive.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
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

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Parses `100`, `1.5k`, `250ms`, `2s` to a number (durations in milliseconds).
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let units: [(&str, f64); 8] = [
        ("ms", 1.0),
        ("s", 1000.0),
        ("min", 60_000.0),
        ("m", 60_000.0),
        ("h", 3_600_000.0),
        ("k", 1000.0),
        ("mb", 1_000_000.0),
        ("kb", 1000.0),
    ];
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }
    for (suffix, mult) in units {
        if let Some(num) = s.strip_suffix(suffix)
            && let Ok(n) = num.parse::<f64>()
        {
            return Some(n * mult);
        }
    }
    None
}

/// Compares one actual value against one filter value.
pub fn compare(actual: &Value, op: Op, expected: &str, now: i64) -> bool {
    if let Value::Array(items) = actual {
        return items.iter().any(|i| compare(i, op, expected, now));
    }
    if actual.is_null() {
        return false;
    }
    // Relative times: `-24h` (within the last day), `+7d` (older than a week).
    if let Some(rel) = expected.strip_prefix('-').and_then(parse_period)
        && let Some(t) = value_time(actual)
    {
        return match op {
            Op::Eq | Op::Gt | Op::Gte => t >= now - rel,
            Op::Lt | Op::Lte => t < now - rel,
        };
    }
    if let Some(rel) = expected.strip_prefix('+').and_then(parse_period)
        && let Some(t) = value_time(actual)
    {
        return t < now - rel;
    }
    if op != Op::Eq {
        if let (Some(a), Some(e)) = (value_time(actual), parse_time(expected))
            && actual.is_string()
        {
            return cmp(op, a as f64, e as f64);
        }
        let a = match actual {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => parse_number(s),
            _ => None,
        };
        return match (a, parse_number(expected)) {
            (Some(a), Some(e)) => cmp(op, a, e),
            _ => false,
        };
    }
    let Some(text) = scalar_text(actual) else {
        return false;
    };
    if expected.contains('*') {
        return glob(expected, &text);
    }
    if let (Value::Number(n), Some(e)) = (actual, parse_number(expected)) {
        return n.as_f64() == Some(e);
    }
    text.eq_ignore_ascii_case(expected)
}

fn cmp(op: Op, a: f64, e: f64) -> bool {
    match op {
        Op::Eq => a == e,
        Op::Gt => a > e,
        Op::Gte => a >= e,
        Op::Lt => a < e,
        Op::Lte => a <= e,
    }
}

/// Evaluates a parsed query. `lookup` returns the record's value for a key (or `None` when
/// the record has no such field), `text` decides free-text matches, and `custom` may claim
/// a term before generic evaluation (returning its result).
pub fn matches(
    terms: &[Term],
    now: i64,
    lookup: &dyn Fn(&str) -> Option<Value>,
    text: &dyn Fn(&str) -> bool,
    custom: &dyn Fn(&Term) -> Option<bool>,
) -> bool {
    terms.iter().all(|term| {
        if let Some(result) = custom(term) {
            return result;
        }
        match term {
            Term::Text { negated, text: t } => text(t) != *negated,
            Term::Has { negated, key } => {
                let present = lookup(key).is_some_and(|v| match v {
                    Value::Null => false,
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    _ => true,
                });
                present != *negated
            }
            Term::Filter {
                negated,
                key,
                op,
                values,
            } => {
                let actual = lookup(key).unwrap_or(Value::Null);
                let hit = values.iter().any(|v| compare(&actual, *op, v, now));
                hit != *negated
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_filters_negation_lists_and_text() {
        let terms = parse(
            r#"is:unresolved !level:info release:[a@1,b@2] times_seen:>=10 "Cannot read" foo has:user"#,
        );
        assert_eq!(terms.len(), 7);
        assert_eq!(
            terms[1],
            Term::Filter {
                negated: true,
                key: "level".into(),
                op: Op::Eq,
                values: vec!["info".into()]
            }
        );
        assert_eq!(
            terms[2],
            Term::Filter {
                negated: false,
                key: "release".into(),
                op: Op::Eq,
                values: vec!["a@1".into(), "b@2".into()]
            }
        );
        assert!(matches!(&terms[3], Term::Filter { op: Op::Gte, .. }));
        assert_eq!(
            terms[4],
            Term::Text {
                negated: false,
                text: "Cannot read".into()
            }
        );
        assert!(matches!(&terms[6], Term::Has { key, .. } if key == "user"));
    }

    #[test]
    fn compares_times_numbers_and_globs() {
        let now = 1_000_000;
        assert!(compare(&json!(999_000), Op::Eq, "-1h", now));
        assert!(!compare(&json!(990_000), Op::Eq, "-1h", now));
        assert!(compare(&json!("120"), Op::Gt, "100", now));
        assert!(compare(&json!(250.0), Op::Gte, "0.25s", now));
        assert!(compare(&json!("web@4.12.0"), Op::Eq, "web@4.*", now));
        assert!(compare(&json!(["TypeError"]), Op::Eq, "typeerror", now));
        assert!(compare(
            &json!("2026-03-14T09:00:00Z"),
            Op::Gt,
            "2026-03-01",
            now
        ));
    }
}
