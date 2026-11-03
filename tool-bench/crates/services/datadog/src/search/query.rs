//! The Datadog event search syntax shared by logs, spans and RUM:
//! `service:checkout status:error -host:web-1 @http.status_code:>=500 "connection reset"`,
//! `service:(a OR b)`, `(x OR y) AND NOT z`, `*` wildcards.

#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    All,
    And(Vec<Query>),
    Or(Vec<Query>),
    Not(Box<Query>),
    /// `key:value` (key may start with `@` for an attribute).
    Field(String, Value),
    /// Free text, matched against the event's message.
    Text(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Glob pattern (`*` wildcard), case-insensitive.
    Pattern(String),
    /// Exact phrase from a quoted value.
    Phrase(String),
    Compare(Cmp, f64),
    Range(f64, f64),
    Any(Vec<Value>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open,
    Close,
    /// `quoted`: some part was in double quotes; `phrase`: the whole word was.
    Word {
        text: String,
        quoted: bool,
        phrase: bool,
    },
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '(' {
            chars.next();
            tokens.push(Token::Open);
        } else if c == ')' {
            chars.next();
            tokens.push(Token::Close);
        } else {
            let mut text = String::new();
            let mut quoted = false;
            let mut phrase = false;
            while let Some(&c) = chars.peek() {
                if c == '"' {
                    if !quoted && text.is_empty() {
                        phrase = true;
                    }
                    quoted = true;
                    chars.next();
                    let mut closed = false;
                    while let Some(c) = chars.next() {
                        if c == '\\' {
                            if let Some(n) = chars.next() {
                                text.push(n);
                            }
                        } else if c == '"' {
                            closed = true;
                            break;
                        } else {
                            text.push(c);
                        }
                    }
                    if !closed {
                        return Err("unterminated quote".to_owned());
                    }
                } else if c.is_whitespace() || c == ')' || c == '(' {
                    break;
                } else if c == '\\' {
                    chars.next();
                    if let Some(n) = chars.next() {
                        text.push(n);
                    }
                } else {
                    text.push(c);
                    chars.next();
                }
            }
            tokens.push(Token::Word {
                text,
                quoted,
                phrase,
            });
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Token::Word { text, quoted: false, .. }) if text == kw)
    }

    fn or(&mut self) -> Result<Query, String> {
        let mut parts = vec![self.and()?];
        while self.is_keyword("OR") {
            self.pos += 1;
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().expect("one part")
        } else {
            Query::Or(parts)
        })
    }

    fn and(&mut self) -> Result<Query, String> {
        let mut parts = vec![self.unary()?];
        loop {
            if self.is_keyword("AND") {
                self.pos += 1;
                parts.push(self.unary()?);
            } else if self.peek().is_some()
                && !self.is_keyword("OR")
                && self.peek() != Some(&Token::Close)
            {
                parts.push(self.unary()?);
            } else {
                break;
            }
        }
        Ok(if parts.len() == 1 {
            parts.pop().expect("one part")
        } else {
            Query::And(parts)
        })
    }

    fn unary(&mut self) -> Result<Query, String> {
        if self.is_keyword("NOT") {
            self.pos += 1;
            return Ok(Query::Not(Box::new(self.unary()?)));
        }
        match self.peek().cloned() {
            None => Err("unexpected end of query".to_owned()),
            Some(Token::Close) => Err("unexpected ')'".to_owned()),
            Some(Token::Open) => {
                self.pos += 1;
                let inner = self.or()?;
                if self.peek() != Some(&Token::Close) {
                    return Err("missing ')'".to_owned());
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(Token::Word {
                text,
                quoted,
                phrase,
            }) => {
                self.pos += 1;
                if phrase {
                    return Ok(Query::Text(Value::Phrase(text)));
                }
                if let Some(rest) = text.strip_prefix('-').filter(|r| !r.is_empty()) {
                    let inner = self.term(rest.to_owned(), quoted)?;
                    return Ok(Query::Not(Box::new(inner)));
                }
                self.term(text, quoted)
            }
        }
    }

    fn term(&mut self, text: String, quoted: bool) -> Result<Query, String> {
        if !quoted && text == "*" {
            return Ok(Query::All);
        }
        match split_field(&text) {
            Some((key, raw)) if raw.is_empty() && self.peek() == Some(&Token::Open) => {
                self.pos += 1;
                let mut values = Vec::new();
                loop {
                    match self.peek().cloned() {
                        Some(Token::Close) => {
                            self.pos += 1;
                            break;
                        }
                        Some(Token::Word { text, quoted, .. }) => {
                            self.pos += 1;
                            if !quoted && (text == "OR" || text == "AND") {
                                continue;
                            }
                            values.push(value(&text, quoted));
                        }
                        _ => return Err("bad value group".to_owned()),
                    }
                }
                Ok(Query::Field(key, Value::Any(values)))
            }
            Some((key, raw)) => {
                if raw.starts_with('[') {
                    let mut range = raw.to_owned();
                    while !range.ends_with(']') {
                        match self.peek().cloned() {
                            Some(Token::Word { text, .. }) => {
                                self.pos += 1;
                                range.push(' ');
                                range.push_str(&text);
                            }
                            _ => return Err("bad range".to_owned()),
                        }
                    }
                    return parse_range(&range)
                        .map(|(a, b)| Query::Field(key, Value::Range(a, b)))
                        .ok_or_else(|| "bad range".to_owned());
                }
                Ok(Query::Field(key, value(raw, quoted)))
            }
            None => Ok(Query::Text(value(&text, quoted))),
        }
    }
}

/// `key:value` split at the first unescaped colon; keys are identifiers or `@paths`.
fn split_field(text: &str) -> Option<(String, &str)> {
    let idx = text.find(':')?;
    let key = &text[..idx];
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._-".contains(c))
    {
        return None;
    }
    Some((key.to_owned(), &text[idx + 1..]))
}

fn value(raw: &str, quoted: bool) -> Value {
    if quoted {
        return Value::Phrase(raw.to_owned());
    }
    for (prefix, cmp) in [
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
    ] {
        if let Some(n) = raw.strip_prefix(prefix).and_then(|n| n.parse::<f64>().ok()) {
            return Value::Compare(cmp, n);
        }
    }
    Value::Pattern(raw.to_owned())
}

fn parse_range(raw: &str) -> Option<(f64, f64)> {
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    let (a, b) = inner.split_once(" TO ")?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

pub fn parse(input: &str) -> Result<Query, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(Query::All);
    }
    let mut parser = Parser { tokens, pos: 0 };
    let query = parser.or()?;
    if parser.pos != parser.tokens.len() {
        return Err("unexpected ')'".to_owned());
    }
    Ok(query)
}

/// Case-insensitive glob match with `*` (any run) and `?` (one character).
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
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

impl Value {
    /// Matches one field value (string, number or bool, already stringified).
    pub fn matches(&self, candidate: &serde_json::Value) -> bool {
        let as_text = match candidate {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => return false,
            other => other.to_string(),
        };
        match self {
            Value::Pattern(p) => glob(p, &as_text),
            Value::Phrase(p) => as_text.eq_ignore_ascii_case(p),
            Value::Compare(cmp, n) => as_number(candidate).is_some_and(|v| match cmp {
                Cmp::Gt => v > *n,
                Cmp::Ge => v >= *n,
                Cmp::Lt => v < *n,
                Cmp::Le => v <= *n,
            }),
            Value::Range(a, b) => as_number(candidate).is_some_and(|v| v >= *a && v <= *b),
            Value::Any(values) => values.iter().any(|v| v.matches(candidate)),
        }
    }

    /// Matches free text: substring for plain words (wildcards allowed), phrase containment.
    pub fn matches_text(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        match self {
            Value::Pattern(p) => {
                let p = p.to_lowercase();
                if p.contains('*') || p.contains('?') {
                    glob(&format!("*{p}*"), &lower)
                } else {
                    lower.contains(&p)
                }
            }
            Value::Phrase(p) => lower.contains(&p.to_lowercase()),
            Value::Any(values) => values.iter().any(|v| v.matches_text(text)),
            Value::Compare(..) | Value::Range(..) => false,
        }
    }
}

fn as_number(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// How one event family exposes fields to the query language.
pub trait Searchable {
    /// Values of `key` on this event (empty if absent). Arrays are flattened.
    fn field_values(&self, key: &str) -> Vec<serde_json::Value>;
    /// Text for free-text terms.
    fn text(&self) -> String;
}

impl Query {
    pub fn matches(&self, event: &dyn Searchable) -> bool {
        match self {
            Query::All => true,
            Query::And(parts) => parts.iter().all(|p| p.matches(event)),
            Query::Or(parts) => parts.iter().any(|p| p.matches(event)),
            Query::Not(inner) => !inner.matches(event),
            Query::Field(key, value) => event
                .field_values(key)
                .iter()
                .any(|candidate| value.matches(candidate)),
            Query::Text(value) => value.matches_text(&event.text()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Ev(serde_json::Value);

    impl Searchable for Ev {
        fn field_values(&self, key: &str) -> Vec<serde_json::Value> {
            match self.0.get(key.trim_start_matches('@')) {
                Some(serde_json::Value::Array(a)) => a.clone(),
                Some(v) => vec![v.clone()],
                None => vec![],
            }
        }
        fn text(&self) -> String {
            self.0["message"].as_str().unwrap_or_default().to_owned()
        }
    }

    fn m(q: &str, ev: &serde_json::Value) -> bool {
        parse(q).unwrap().matches(&Ev(ev.clone()))
    }

    #[test]
    fn parses_and_matches_common_queries() {
        let ev = json!({"service": "checkout", "status": "error", "code": 503, "message": "upstream connection reset by peer"});
        assert!(m("*", &ev));
        assert!(m("", &ev));
        assert!(m("service:checkout status:error", &ev));
        assert!(m("service:check*", &ev));
        assert!(m("service:(cart OR checkout)", &ev));
        assert!(!m("service:checkout -status:error", &ev));
        assert!(m("service:checkout AND NOT status:info", &ev));
        assert!(m("@code:>=500", &ev));
        assert!(m("@code:[500 TO 599]", &ev));
        assert!(!m("@code:<500", &ev));
        assert!(m("\"connection reset\"", &ev));
        assert!(m("reset", &ev));
        assert!(!m("\"reset connection\"", &ev));
        assert!(m("(service:cart OR service:checkout) status:error", &ev));
        assert!(m("service:cart OR status:error", &ev));
        assert!(!m("service:cart", &ev));
        assert!(m("service:\"checkout\"", &ev));
        assert!(m("\"reset by: peer\" OR \"reset by peer\"", &ev));
        assert!(m("-\"timeout\"", &ev));
    }

    #[test]
    fn rejects_broken_queries() {
        assert!(parse("(service:a").is_err());
        assert!(parse("service:a)").is_err());
        assert!(parse("\"open").is_err());
    }

    #[test]
    fn glob_matching() {
        assert!(glob("web-prod-*", "web-prod-03"));
        assert!(glob("*prod*", "web-prod-03"));
        assert!(!glob("web-prod-0?", "web-prod-030"));
        assert!(glob("WEB*", "web"));
    }
}
