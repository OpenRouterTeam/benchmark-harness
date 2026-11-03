//! Okta's SCIM-style `filter` / `search` expressions, e.g.
//! `profile.department eq "Engineering" and (status eq "ACTIVE" or status eq "STAGED")`.
//! Supports `eq ne gt ge lt le sw co ew pr`, `and`, `or`, `not`, parentheses, and dotted
//! attribute paths that fan out over arrays (`target.id eq "..."`).

use serde_json::Value;

use crate::util::parse_ts;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp { path: String, op: Op, value: Value },
    Present(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Sw,
    Co,
    Ew,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Open,
    Close,
}

fn tokenize(input: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Tok::Open);
            i += 1;
        } else if c == ')' {
            out.push(Tok::Close);
            i += 1;
        } else if c == '"' || c == '\'' {
            let quote = c;
            let mut s = String::new();
            i += 1;
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err("Unterminated string literal".into());
                };
                i += 1;
                if ch == '\\' {
                    if let Some(&next) = chars.get(i) {
                        s.push(next);
                        i += 1;
                    }
                } else if ch == quote {
                    break;
                } else {
                    s.push(ch);
                }
            }
            out.push(Tok::Str(s));
        } else {
            let mut s = String::new();
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '(' && chars[i] != ')'
            {
                s.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek_word(&self) -> Option<String> {
        match self.toks.get(self.pos) {
            Some(Tok::Word(w)) => Some(w.to_ascii_lowercase()),
            _ => None,
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.peek_word().as_deref() == Some("or") {
            self.pos += 1;
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.factor()?;
        while self.peek_word().as_deref() == Some("and") {
            self.pos += 1;
            let right = self.factor()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, String> {
        if self.peek_word().as_deref() == Some("not") {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.factor()?)));
        }
        match self.toks.get(self.pos).cloned() {
            Some(Tok::Open) => {
                self.pos += 1;
                let e = self.or()?;
                if self.toks.get(self.pos) != Some(&Tok::Close) {
                    return Err("Missing closing parenthesis".into());
                }
                self.pos += 1;
                Ok(e)
            }
            Some(Tok::Word(path)) => {
                self.pos += 1;
                let op = self.peek_word().ok_or("Missing operator")?;
                self.pos += 1;
                let op = match op.as_str() {
                    "pr" => return Ok(Expr::Present(path)),
                    "eq" => Op::Eq,
                    "ne" => Op::Ne,
                    "gt" => Op::Gt,
                    "ge" => Op::Ge,
                    "lt" => Op::Lt,
                    "le" => Op::Le,
                    "sw" => Op::Sw,
                    "co" => Op::Co,
                    "ew" => Op::Ew,
                    other => return Err(format!("Unsupported operator: {other}")),
                };
                let value = match self.toks.get(self.pos).cloned() {
                    Some(Tok::Str(s)) => Value::String(s),
                    Some(Tok::Word(w)) => match w.as_str() {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        "null" => Value::Null,
                        n => n
                            .parse::<f64>()
                            .ok()
                            .and_then(serde_json::Number::from_f64)
                            .map(Value::Number)
                            .ok_or_else(|| format!("Invalid value: {n}"))?,
                    },
                    _ => return Err("Missing value".into()),
                };
                self.pos += 1;
                Ok(Expr::Cmp { path, op, value })
            }
            _ => Err("Invalid search expression".into()),
        }
    }
}

pub fn parse(input: &str) -> Result<Expr, String> {
    let toks = tokenize(input)?;
    if toks.is_empty() {
        return Err("Empty expression".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err("Unexpected trailing tokens".into());
    }
    Ok(e)
}

impl Expr {
    /// Whether the expression mentions `attr` (e.g. `status`) anywhere.
    pub fn mentions(&self, attr: &str) -> bool {
        match self {
            Expr::And(a, b) | Expr::Or(a, b) => a.mentions(attr) || b.mentions(attr),
            Expr::Not(a) => a.mentions(attr),
            Expr::Cmp { path, .. } | Expr::Present(path) => path == attr,
        }
    }

    pub fn matches(&self, doc: &Value) -> bool {
        match self {
            Expr::And(a, b) => a.matches(doc) && b.matches(doc),
            Expr::Or(a, b) => a.matches(doc) || b.matches(doc),
            Expr::Not(a) => !a.matches(doc),
            Expr::Present(path) => resolve(doc, path)
                .iter()
                .any(|v| !v.is_null() && v.as_str() != Some("")),
            Expr::Cmp { path, op, value } => {
                let found = resolve(doc, path);
                if *op == Op::Ne {
                    return !found.iter().any(|v| compare(v, Op::Eq, value));
                }
                found.iter().any(|v| compare(v, *op, value))
            }
        }
    }
}

/// Values at a dotted path; arrays along the way fan out.
pub fn resolve<'a>(doc: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut current = vec![doc];
    for part in path.split('.') {
        let mut next = Vec::new();
        for v in current {
            match v {
                Value::Object(map) => {
                    if let Some(x) = map.get(part).or_else(|| {
                        map.iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(part))
                            .map(|(_, v)| v)
                    }) {
                        match x {
                            Value::Array(items) => next.extend(items.iter()),
                            other => next.push(other),
                        }
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        if let Some(x) = item.get(part) {
                            next.push(x);
                        }
                    }
                }
                _ => {}
            }
        }
        current = next;
    }
    current
}

fn compare(actual: &Value, op: Op, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::String(a), Value::String(e)) => {
            if matches!(op, Op::Gt | Op::Ge | Op::Lt | Op::Le)
                && let (Some(x), Some(y)) = (parse_ts(a), parse_ts(e))
            {
                return ord(x.cmp(&y), op);
            }
            let (a, e) = (a.to_lowercase(), e.to_lowercase());
            match op {
                Op::Eq => a == e,
                Op::Ne => a != e,
                Op::Sw => a.starts_with(&e),
                Op::Co => a.contains(&e),
                Op::Ew => a.ends_with(&e),
                _ => ord(a.cmp(&e), op),
            }
        }
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap_or(0.0), e.as_f64().unwrap_or(0.0));
            match op {
                Op::Eq => a == e,
                Op::Ne => a != e,
                _ => a.partial_cmp(&e).is_some_and(|o| ord(o, op)),
            }
        }
        (Value::Bool(a), Value::Bool(e)) => match op {
            Op::Eq => a == e,
            Op::Ne => a != e,
            _ => false,
        },
        (Value::Bool(a), Value::String(e)) => {
            compare(&Value::String(a.to_string()), op, &Value::String(e.clone()))
        }
        (Value::Number(a), Value::String(e)) => {
            compare(&Value::String(a.to_string()), op, &Value::String(e.clone()))
        }
        _ => false,
    }
}

fn ord(o: std::cmp::Ordering, op: Op) -> bool {
    use std::cmp::Ordering::*;
    match op {
        Op::Gt => o == Greater,
        Op::Ge => o != Less,
        Op::Lt => o == Less,
        Op::Le => o != Greater,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn evaluates_nested_boolean_expressions() {
        let user = json!({"status": "ACTIVE", "lastUpdated": "2026-03-01T10:00:00.000Z", "profile": {"department": "Engineering", "login": "ana@octo-corp.example"}});
        let e = parse(
            r#"profile.department eq "engineering" and (status eq "ACTIVE" or status eq "STAGED")"#,
        )
        .unwrap();
        assert!(e.matches(&user));
        assert!(
            parse(r#"lastUpdated gt "2026-02-28T00:00:00Z""#)
                .unwrap()
                .matches(&user)
        );
        assert!(
            !parse(r#"lastUpdated lt "2026-02-28T00:00:00Z""#)
                .unwrap()
                .matches(&user)
        );
        assert!(parse(r#"profile.login sw "ana""#).unwrap().matches(&user));
        assert!(parse("profile.department pr").unwrap().matches(&user));
        assert!(!parse(r#"not (status eq "ACTIVE")"#).unwrap().matches(&user));
        assert!(e.mentions("status"));
    }

    #[test]
    fn fans_out_over_arrays() {
        let event = json!({"target": [{"id": "a"}, {"id": "b"}]});
        assert!(parse(r#"target.id eq "b""#).unwrap().matches(&event));
        assert!(!parse(r#"target.id eq "c""#).unwrap().matches(&event));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("status").is_err());
        assert!(parse(r#"status zz "x""#).is_err());
        assert!(parse(r#"(status eq "x""#).is_err());
    }
}
