//! A CloudWatch Logs Insights (CWLI) interpreter covering the commonly used commands:
//! `fields`, `display`, `filter`, `parse` (glob and regex), `stats ... by` (count,
//! count_distinct, sum, avg, min, max, pct, earliest, latest; `bin(…)`), `sort`, `limit`,
//! `dedup` and `pattern`. JSON messages contribute their (flattened) fields.

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::{Map, Value};

use crate::util::iso;

#[derive(Debug, Clone)]
pub struct Event {
    pub timestamp_ms: i64,
    pub ingestion_ms: i64,
    pub message: String,
    pub stream: String,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub rows: Vec<Vec<(String, String)>>,
    pub records_matched: usize,
    pub records_scanned: usize,
    pub bytes_scanned: usize,
}

type Row = Map<String, Value>;

pub fn format_ts(ms: i64) -> String {
    let base = iso(ms.div_euclid(1000));
    format!(
        "{} {}.{:03}",
        &base[..10],
        &base[11..19],
        ms.rem_euclid(1000)
    )
}

fn flatten(prefix: &str, v: &Value, out: &mut Row) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                flatten(&format!("{prefix}.{i}"), v, out);
            }
        }
        other => {
            out.insert(prefix.to_owned(), other.clone());
        }
    }
}

fn row_of(e: &Event, account: &str) -> Row {
    let mut r = Row::new();
    r.insert(
        "@timestamp".into(),
        Value::String(format_ts(e.timestamp_ms)),
    );
    r.insert("@message".into(), Value::String(e.message.clone()));
    r.insert("@logStream".into(), Value::String(e.stream.clone()));
    r.insert(
        "@log".into(),
        Value::String(format!("{account}:{}", e.group)),
    );
    r.insert(
        "@ingestionTime".into(),
        Value::String(format_ts(e.ingestion_ms)),
    );
    r.insert("_ts".into(), Value::from(e.timestamp_ms));
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(e.message.trim()) {
        let mut fields = Row::new();
        flatten("", &v, &mut fields);
        for (k, v) in fields {
            r.entry(k).or_insert(v);
        }
    }
    r
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => fmt_num(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn fmt_num(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        let s = format!("{:.4}", f);
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

// ───────────────────────────── tokens ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Regex(String, String),
    Op(String),
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let prev_is_value = |out: &Vec<Tok>| {
        matches!(out.last(), Some(Tok::Ident(_) | Tok::Str(_) | Tok::Num(_)))
            || matches!(out.last(), Some(Tok::Op(o)) if o == ")")
    };
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '\'' || c == '"' {
            let mut j = i + 1;
            let mut text = String::new();
            while j < chars.len() && chars[j] != c {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    j += 1;
                }
                text.push(chars[j]);
                j += 1;
            }
            if j >= chars.len() {
                return Err("unterminated string".into());
            }
            out.push(Tok::Str(text));
            i = j + 1;
        } else if c == '/'
            && (!prev_is_value(&out)
                || matches!(out.last(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case("like")))
        {
            let mut j = i + 1;
            let mut text = String::new();
            while j < chars.len() && chars[j] != '/' {
                if chars[j] == '\\' && j + 1 < chars.len() && chars[j + 1] == '/' {
                    j += 1;
                }
                text.push(chars[j]);
                j += 1;
            }
            if j >= chars.len() {
                return Err("unterminated regular expression".into());
            }
            j += 1;
            let mut flags = String::new();
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                flags.push(chars[j]);
                j += 1;
            }
            out.push(Tok::Regex(text, flags));
            i = j;
        } else if c == '`' {
            let end = chars[i + 1..]
                .iter()
                .position(|x| *x == '`')
                .ok_or("unterminated backtick")?;
            out.push(Tok::Ident(chars[i + 1..i + 1 + end].iter().collect()));
            i += end + 2;
        } else if c.is_ascii_digit()
            || (c == '-'
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                && !prev_is_value(&out))
        {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            // Durations like `5m` stay identifiers for bin().
            if j < chars.len() && chars[j].is_ascii_alphabetic() {
                let mut k = j;
                while k < chars.len() && chars[k].is_ascii_alphanumeric() {
                    k += 1;
                }
                out.push(Tok::Ident(chars[i..k].iter().collect()));
                i = k;
                continue;
            }
            let text: String = chars[i..j].iter().collect();
            out.push(Tok::Num(
                text.parse().map_err(|_| format!("bad number {text}"))?,
            ));
            i = j;
        } else if c.is_alphanumeric() || c == '@' || c == '_' || c == '$' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || "@_.$-".contains(chars[j])) {
                j += 1;
            }
            out.push(Tok::Ident(chars[i..j].iter().collect()));
            i = j;
        } else {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            if ["!=", "<=", ">=", "=~"].contains(&two.as_str()) {
                out.push(Tok::Op(two));
                i += 2;
            } else if "=<>(),[]*+-/%".contains(c) {
                out.push(Tok::Op(c.to_string()));
                i += 1;
            } else {
                return Err(format!("unexpected character '{c}'"));
            }
        }
    }
    Ok(out)
}

// ───────────────────────────── expressions ─────────────────────────────

#[derive(Debug, Clone)]
enum Expr {
    Field(String),
    Lit(Value),
    Cmp(Box<Expr>, String, Box<Expr>),
    Like(Box<Expr>, Pattern, bool),
    In(Box<Expr>, Vec<Value>, bool),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Call(String, Vec<Expr>),
    Arith(Box<Expr>, char, Box<Expr>),
}

#[derive(Debug, Clone)]
enum Pattern {
    Re(Regex),
    Sub(String),
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn kw(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(word))
    }

    fn op(&self, o: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(x)) if x == o)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        self.i += 1;
        t
    }

    fn expect_op(&mut self, o: &str) -> Result<(), String> {
        if self.op(o) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{o}'"))
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.kw("or") {
            self.i += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.not()?;
        while self.kw("and") {
            self.i += 1;
            left = Expr::And(Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.kw("not") {
            self.i += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn pattern(&mut self) -> Result<Pattern, String> {
        match self.next() {
            Some(Tok::Regex(r, flags)) => {
                let prefix = if flags.contains('i') { "(?i)" } else { "" };
                Regex::new(&format!("{prefix}{r}"))
                    .map(Pattern::Re)
                    .map_err(|e| format!("invalid regex: {e}"))
            }
            Some(Tok::Str(s)) => Ok(Pattern::Sub(s)),
            _ => Err("expected a regular expression or string after like".into()),
        }
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let left = self.sum()?;
        if self.kw("like") || self.op("=~") {
            self.i += 1;
            return Ok(Expr::Like(Box::new(left), self.pattern()?, true));
        }
        if self.kw("not")
            && matches!(self.toks.get(self.i + 1), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case("like"))
        {
            self.i += 2;
            return Ok(Expr::Like(Box::new(left), self.pattern()?, false));
        }
        let negate_in = self.kw("not")
            && matches!(self.toks.get(self.i + 1), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case("in"));
        if self.kw("in") || negate_in {
            self.i += if negate_in { 2 } else { 1 };
            self.expect_op("[")?;
            let mut items = Vec::new();
            while !self.op("]") {
                match self.next() {
                    Some(Tok::Str(s)) => items.push(Value::String(s)),
                    Some(Tok::Num(n)) => items.push(Value::from(n)),
                    Some(Tok::Ident(s)) => items.push(Value::String(s)),
                    Some(Tok::Op(o)) if o == "," => {}
                    _ => return Err("bad list".into()),
                }
            }
            self.i += 1;
            return Ok(Expr::In(Box::new(left), items, !negate_in));
        }
        for o in ["=", "!=", "<", "<=", ">", ">="] {
            if self.op(o) {
                self.i += 1;
                let right = self.sum()?;
                return Ok(Expr::Cmp(Box::new(left), o.to_owned(), Box::new(right)));
            }
        }
        Ok(left)
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut left = self.product()?;
        while self.op("+") || self.op("-") {
            let op = if self.op("+") { '+' } else { '-' };
            self.i += 1;
            left = Expr::Arith(Box::new(left), op, Box::new(self.product()?));
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Expr, String> {
        let mut left = self.atom()?;
        while self.op("*") || self.op("/") || self.op("%") {
            let op = match self.next() {
                Some(Tok::Op(o)) => o.chars().next().unwrap_or('*'),
                _ => '*',
            };
            left = Expr::Arith(Box::new(left), op, Box::new(self.atom()?));
        }
        Ok(left)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Op(o)) if o == "(" => {
                let e = self.or()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Some(Tok::Op(o)) if o == "*" => Ok(Expr::Lit(Value::String("*".into()))),
            Some(Tok::Str(s)) => Ok(Expr::Lit(Value::String(s))),
            Some(Tok::Num(n)) => Ok(Expr::Lit(Value::from(n))),
            Some(Tok::Ident(name)) => {
                if self.op("(") {
                    self.i += 1;
                    let mut args = Vec::new();
                    while !self.op(")") {
                        if self.op(",") {
                            self.i += 1;
                            continue;
                        }
                        args.push(self.or()?);
                        if self.peek().is_none() {
                            return Err("unterminated function call".into());
                        }
                    }
                    self.i += 1;
                    Ok(Expr::Call(name.to_ascii_lowercase(), args))
                } else if name.eq_ignore_ascii_case("true") || name.eq_ignore_ascii_case("false") {
                    Ok(Expr::Lit(Value::Bool(name.eq_ignore_ascii_case("true"))))
                } else {
                    Ok(Expr::Field(name))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

fn parse_expr(text: &str) -> Result<Expr, String> {
    let mut p = Parser {
        toks: tokenize(text)?,
        i: 0,
    };
    let e = p.or()?;
    if p.i != p.toks.len() {
        return Err(format!("unexpected input near token {}", p.i + 1));
    }
    Ok(e)
}

fn eval(e: &Expr, row: &Row) -> Option<Value> {
    match e {
        Expr::Field(f) => row.get(f).cloned(),
        Expr::Lit(v) => Some(v.clone()),
        Expr::Cmp(a, op, b) => {
            let (Some(x), Some(y)) = (eval(a, row), eval(b, row)) else {
                return Some(Value::Bool(false));
            };
            let ord = match (as_num(&x), as_num(&y)) {
                (Some(m), Some(n)) => m.partial_cmp(&n)?,
                _ => as_text(&x).cmp(&as_text(&y)),
            };
            use std::cmp::Ordering::*;
            Some(Value::Bool(match op.as_str() {
                "=" => ord == Equal,
                "!=" => ord != Equal,
                "<" => ord == Less,
                "<=" => ord != Greater,
                ">" => ord == Greater,
                _ => ord != Less,
            }))
        }
        Expr::Like(a, p, positive) => {
            let text = eval(a, row).map(|v| as_text(&v));
            let hit = text.is_some_and(|t| match p {
                Pattern::Re(r) => r.is_match(&t),
                Pattern::Sub(s) => t.contains(s.as_str()),
            });
            Some(Value::Bool(hit == *positive))
        }
        Expr::In(a, items, positive) => {
            let v = eval(a, row).map(|v| as_text(&v));
            let hit = v.is_some_and(|v| items.iter().any(|i| as_text(i) == v));
            Some(Value::Bool(hit == *positive))
        }
        Expr::And(a, b) => Some(Value::Bool(truthy(eval(a, row)) && truthy(eval(b, row)))),
        Expr::Or(a, b) => Some(Value::Bool(truthy(eval(a, row)) || truthy(eval(b, row)))),
        Expr::Not(a) => Some(Value::Bool(!truthy(eval(a, row)))),
        Expr::Arith(a, op, b) => {
            let x = as_num(&eval(a, row)?)?;
            let y = as_num(&eval(b, row)?)?;
            Some(Value::from(match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '%' => x % y,
                _ => x / y,
            }))
        }
        Expr::Call(name, args) => {
            let arg = |i: usize| args.get(i).and_then(|a| eval(a, row));
            match name.as_str() {
                "ispresent" => Some(Value::Bool(arg(0).is_some())),
                "isempty" => Some(Value::Bool(arg(0).is_none_or(|v| as_text(&v).is_empty()))),
                "isblank" => Some(Value::Bool(
                    arg(0).is_none_or(|v| as_text(&v).trim().is_empty()),
                )),
                "strcontains" => Some(Value::Bool(
                    arg(0)
                        .zip(arg(1))
                        .is_some_and(|(a, b)| as_text(&a).contains(&as_text(&b))),
                )),
                "tolower" => arg(0).map(|v| Value::String(as_text(&v).to_lowercase())),
                "toupper" => arg(0).map(|v| Value::String(as_text(&v).to_uppercase())),
                "strlen" => arg(0).map(|v| Value::from(as_text(&v).chars().count())),
                "concat" => Some(Value::String(
                    args.iter()
                        .filter_map(|a| eval(a, row))
                        .map(|v| as_text(&v))
                        .collect(),
                )),
                "abs" => arg(0)
                    .and_then(|v| as_num(&v))
                    .map(|n| Value::from(n.abs())),
                "floor" => arg(0)
                    .and_then(|v| as_num(&v))
                    .map(|n| Value::from(n.floor())),
                "ceil" => arg(0)
                    .and_then(|v| as_num(&v))
                    .map(|n| Value::from(n.ceil())),
                "bin" => {
                    let secs = args
                        .first()
                        .and_then(|a| match a {
                            Expr::Field(d) => duration_secs(d),
                            Expr::Lit(v) => as_num(v).map(|n| n as i64),
                            _ => None,
                        })
                        .filter(|s| *s > 0)?;
                    let ts = row.get("_ts").and_then(Value::as_i64)?;
                    let b = ts.div_euclid(secs * 1000) * secs * 1000;
                    Some(Value::String(format_ts(b)))
                }
                _ => None,
            }
        }
    }
}

fn truthy(v: Option<Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => b,
        Some(Value::Null) | None => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(_) => true,
    }
}

fn duration_secs(d: &str) -> Option<i64> {
    let d = d.trim();
    let split = d.find(|c: char| c.is_ascii_alphabetic())?;
    let n: i64 = d[..split].parse().ok()?;
    let mult = match &d[split..] {
        "ms" => return Some((n / 1000).max(1)),
        "s" | "sec" | "second" | "seconds" => 1,
        "m" | "min" | "minute" | "minutes" => 60,
        "h" | "hr" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 604_800,
        _ => return None,
    };
    Some(n * mult)
}

// ───────────────────────────── commands ─────────────────────────────

fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut in_regex = false;
    let mut prev_sig: Option<char> = None;
    for c in text.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        if in_regex {
            cur.push(c);
            if c == '/' {
                in_regex = false;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '/' if prev_sig.is_none_or(|p| !(p.is_alphanumeric() || p == ')' || p == '_'))
                || regex_context(&cur) =>
            {
                in_regex = true
            }
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            parts.push(cur.trim().to_owned());
            cur.clear();
            prev_sig = None;
            continue;
        }
        cur.push(c);
        if !c.is_whitespace() {
            prev_sig = Some(c);
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_owned());
    }
    parts
}

/// A `/` after `like`, `=~`, or `parse <field>` opens a regular expression.
fn regex_context(before: &str) -> bool {
    let words: Vec<&str> = before.split_whitespace().collect();
    match words.as_slice() {
        [.., last] if last.eq_ignore_ascii_case("like") || *last == "=~" => true,
        [cmd, _field] => cmd.eq_ignore_ascii_case("parse"),
        _ => false,
    }
}

/// `expr as alias` -> (expr text, alias).
fn alias(item: &str) -> (String, String) {
    let re = Regex::new(r"(?i)^(.*?)\s+as\s+([@\w.$-]+|`[^`]+`)\s*$").expect("valid regex");
    match re.captures(item) {
        Some(c) => (c[1].trim().to_owned(), c[2].trim_matches('`').to_owned()),
        None => (
            item.trim().to_owned(),
            item.trim().trim_matches('`').to_owned(),
        ),
    }
}

fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("(?s)");
    for c in glob.chars() {
        if c == '*' {
            out.push_str("(.*?)");
        } else {
            out.push_str(&regex::escape(&c.to_string()));
        }
    }
    out
}

pub struct Query<'a> {
    pub text: &'a str,
    pub limit: Option<usize>,
    pub account: &'a str,
}

pub fn run(q: &Query, events: &[Event]) -> Result<Outcome, String> {
    let scanned_bytes: usize = events.iter().map(|e| e.message.len()).sum();
    let mut rows: Vec<Row> = events.iter().map(|e| row_of(e, q.account)).collect();
    let mut display: Option<Vec<String>> = None;
    let mut aggregated = false;
    let mut limit = q.limit;
    let mut sorted = false;
    let mut matched_after_filter: Option<usize> = None;
    let glob_parse = Regex::new(r#"(?is)^(['"])(.*)(['"])\s+as\s+(.+)$"#).expect("valid regex");
    let commands = split_top(q.text, '|');
    if commands.is_empty() {
        return Err("Query string is empty".into());
    }
    for cmd in commands {
        let (word, rest) = match cmd.split_once(char::is_whitespace) {
            Some((w, r)) => (w.to_ascii_lowercase(), r.trim().to_owned()),
            None => (cmd.to_ascii_lowercase(), String::new()),
        };
        match word.as_str() {
            "fields" | "display" => {
                let mut names = Vec::new();
                for item in split_top(&rest, ',') {
                    let (expr, name) = alias(&item);
                    if expr != name {
                        let e = parse_expr(&expr)?;
                        for r in &mut rows {
                            if let Some(v) = eval(&e, r) {
                                r.insert(name.clone(), v);
                            }
                        }
                    }
                    names.push(name);
                }
                if word == "fields" {
                    let mut d = display.take().unwrap_or_default();
                    for n in names {
                        if !d.contains(&n) {
                            d.push(n);
                        }
                    }
                    display = Some(d);
                } else {
                    display = Some(names);
                }
            }
            "filter" => {
                let e = parse_expr(&rest)?;
                rows.retain(|r| truthy(eval(&e, r)));
                if !aggregated {
                    matched_after_filter = Some(rows.len());
                }
            }
            "parse" => {
                let (field, pattern) = rest
                    .split_once(char::is_whitespace)
                    .map(|(f, p)| (f.to_owned(), p.trim().to_owned()))
                    .ok_or("parse needs a field and a pattern")?;
                if pattern.starts_with('/') {
                    let end = pattern
                        .rfind('/')
                        .filter(|e| *e > 0)
                        .ok_or("bad parse regex")?;
                    let re =
                        Regex::new(&pattern[1..end]).map_err(|e| format!("invalid regex: {e}"))?;
                    let names: Vec<String> =
                        re.capture_names().flatten().map(str::to_owned).collect();
                    for r in &mut rows {
                        let text = r.get(&field).map(as_text).unwrap_or_default();
                        if let Some(c) = re.captures(&text) {
                            for n in &names {
                                if let Some(m) = c.name(n) {
                                    r.insert(n.clone(), Value::String(m.as_str().to_owned()));
                                }
                            }
                        }
                    }
                } else {
                    let c = glob_parse
                        .captures(&pattern)
                        .ok_or("parse needs \"pattern\" as fields")?;
                    let glob = c[2].to_owned();
                    let names: Vec<String> = c[4].split(',').map(|n| n.trim().to_owned()).collect();
                    let gre = Regex::new(&glob_to_regex(&glob)).map_err(|e| e.to_string())?;
                    for r in &mut rows {
                        let text = r.get(&field).map(as_text).unwrap_or_default();
                        if let Some(caps) = gre.captures(&text) {
                            for (i, n) in names.iter().enumerate() {
                                if let Some(m) = caps.get(i + 1) {
                                    r.insert(n.clone(), Value::String(m.as_str().to_owned()));
                                }
                            }
                        }
                    }
                }
            }
            "stats" => {
                rows = stats(&rest, rows)?;
                aggregated = true;
                display = None;
            }
            "sort" => {
                let keys: Vec<(String, bool)> = split_top(&rest, ',')
                    .iter()
                    .map(|k| {
                        let mut parts = k.split_whitespace();
                        let f = parts
                            .next()
                            .unwrap_or_default()
                            .trim_matches('`')
                            .to_owned();
                        let desc = !parts.next().is_some_and(|d| d.eq_ignore_ascii_case("asc"));
                        (f, desc)
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    for (k, desc) in &keys {
                        let ka = if k == "@timestamp" {
                            a.get("_ts").cloned()
                        } else {
                            a.get(k).cloned()
                        };
                        let kb = if k == "@timestamp" {
                            b.get("_ts").cloned()
                        } else {
                            b.get(k).cloned()
                        };
                        let o = match (ka.as_ref().and_then(as_num), kb.as_ref().and_then(as_num)) {
                            (Some(x), Some(y)) => {
                                x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                            }
                            _ => ka.map(|v| as_text(&v)).cmp(&kb.map(|v| as_text(&v))),
                        };
                        let o = if *desc { o.reverse() } else { o };
                        if o != std::cmp::Ordering::Equal {
                            return o;
                        }
                    }
                    std::cmp::Ordering::Equal
                });
                sorted = true;
            }
            "limit" | "head" => {
                let n: usize = rest.trim().parse().map_err(|_| "limit needs a number")?;
                limit = Some(limit.map_or(n, |l| l.min(n)));
                rows.truncate(n);
            }
            "dedup" => {
                let fields: Vec<String> = split_top(&rest, ',');
                let mut seen = Vec::new();
                rows.retain(|r| {
                    let key: Vec<String> = fields
                        .iter()
                        .map(|f| r.get(f).map(as_text).unwrap_or_default())
                        .collect();
                    if seen.contains(&key) {
                        false
                    } else {
                        seen.push(key);
                        true
                    }
                });
            }
            "pattern" => {
                rows = pattern(&rest, rows);
                aggregated = true;
                display = None;
            }
            "unmask" | "source" => {}
            other => return Err(format!("unexpected symbol found {other} at line 1")),
        }
    }
    if !sorted && !aggregated {
        rows.sort_by(|a, b| {
            b.get("_ts")
                .and_then(Value::as_i64)
                .cmp(&a.get("_ts").and_then(Value::as_i64))
        });
    }
    if let Some(l) = limit {
        rows.truncate(l);
    }
    let matched = if aggregated {
        matched_after_filter.unwrap_or(events.len())
    } else {
        matched_after_filter
            .unwrap_or(events.len())
            .min(events.len())
    };
    let out_rows = rows
        .iter()
        .map(|r| {
            let names: Vec<String> = match &display {
                Some(d) => d.clone(),
                None if aggregated => r.keys().filter(|k| *k != "_ts").cloned().collect(),
                None => vec![
                    "@timestamp".into(),
                    "@message".into(),
                    "@logStream".into(),
                    "@log".into(),
                ],
            };
            names
                .into_iter()
                .filter_map(|n| r.get(&n).map(|v| (n.clone(), as_text(v))))
                .collect()
        })
        .collect();
    Ok(Outcome {
        rows: out_rows,
        records_matched: matched,
        records_scanned: events.len(),
        bytes_scanned: scanned_bytes,
    })
}

fn stats(spec: &str, rows: Vec<Row>) -> Result<Vec<Row>, String> {
    let re = Regex::new(r"(?is)^(.*?)(?:\s+by\s+(.+))?$").expect("valid regex");
    let c = re.captures(spec).ok_or("bad stats")?;
    let aggs: Vec<(Expr, String)> = split_top(&c[1], ',')
        .iter()
        .map(|item| {
            let (expr, name) = alias(item);
            parse_expr(&expr).map(|e| (e, name))
        })
        .collect::<Result<_, _>>()?;
    let groups: Vec<(Expr, String)> = c
        .get(2)
        .map(|g| split_top(g.as_str(), ','))
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let (expr, name) = alias(item);
            parse_expr(&expr).map(|e| (e, name))
        })
        .collect::<Result<_, _>>()?;
    let mut buckets: Vec<(Vec<String>, Vec<Row>)> = Vec::new();
    for r in rows {
        let key: Vec<String> = groups
            .iter()
            .map(|(e, _)| eval(e, &r).map(|v| as_text(&v)).unwrap_or_default())
            .collect();
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(r),
            None => buckets.push((key, vec![r])),
        }
    }
    if buckets.is_empty() && groups.is_empty() {
        buckets.push((Vec::new(), Vec::new()));
    }
    let mut out = Vec::new();
    for (key, members) in buckets {
        let mut row = Row::new();
        for ((_, name), value) in groups.iter().zip(&key) {
            row.insert(name.clone(), Value::String(value.clone()));
        }
        for (e, name) in &aggs {
            if let Some(v) = aggregate(e, &members) {
                row.insert(name.clone(), v);
            }
        }
        if let Some(ts) = members
            .iter()
            .filter_map(|m| m.get("_ts").and_then(Value::as_i64))
            .min()
        {
            row.insert("_ts".into(), Value::from(ts));
        }
        out.push(row);
    }
    // Time-binned results come out in time order.
    if groups
        .iter()
        .any(|(e, _)| matches!(e, Expr::Call(n, _) if n == "bin"))
    {
        out.sort_by(|a, b| {
            let first = groups
                .iter()
                .find(|(e, _)| matches!(e, Expr::Call(n, _) if n == "bin"))
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            a.get(&first).map(as_text).cmp(&b.get(&first).map(as_text))
        });
    }
    Ok(out)
}

fn aggregate(e: &Expr, rows: &[Row]) -> Option<Value> {
    let Expr::Call(name, args) = e else {
        return None;
    };
    let values = |i: usize| -> Vec<Value> {
        match args.get(i) {
            None => rows.iter().map(|_| Value::Bool(true)).collect(),
            Some(Expr::Lit(Value::String(s))) if s == "*" => {
                rows.iter().map(|_| Value::Bool(true)).collect()
            }
            Some(a) => rows.iter().filter_map(|r| eval(a, r)).collect(),
        }
    };
    let nums = |i: usize| -> Vec<f64> { values(i).iter().filter_map(as_num).collect() };
    Some(match name.as_str() {
        "count" => Value::from(values(0).len()),
        "count_distinct" => {
            let mut set: Vec<String> = values(0).iter().map(as_text).collect();
            set.sort();
            set.dedup();
            Value::from(set.len())
        }
        "sum" => Value::from(nums(0).iter().sum::<f64>()),
        "avg" => {
            let n = nums(0);
            if n.is_empty() {
                return None;
            }
            Value::from(n.iter().sum::<f64>() / n.len() as f64)
        }
        "min" => Value::from(nums(0).into_iter().reduce(f64::min)?),
        "max" => Value::from(nums(0).into_iter().reduce(f64::max)?),
        "pct" | "percentile" => {
            let mut n = nums(0);
            if n.is_empty() {
                return None;
            }
            n.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
            let q = args
                .get(1)
                .and_then(|a| eval(a, &Row::new()))
                .and_then(|v| as_num(&v))
                .unwrap_or(50.0);
            let rank = ((q / 100.0) * n.len() as f64).ceil().max(1.0) as usize;
            Value::from(n[rank.min(n.len()) - 1])
        }
        "earliest" | "latest" => {
            let mut with_ts: Vec<(i64, Value)> = rows
                .iter()
                .filter_map(|r| Some((r.get("_ts")?.as_i64()?, eval(args.first()?, r)?)))
                .collect();
            with_ts.sort_by_key(|(t, _)| *t);
            let pick = if name == "earliest" {
                with_ts.first()
            } else {
                with_ts.last()
            };
            pick?.1.clone()
        }
        _ => return None,
    })
}

/// Groups messages by their shape: numbers, hex ids and UUIDs become `<*>`.
fn pattern(_spec: &str, rows: Vec<Row>) -> Vec<Row> {
    let token = Regex::new(r"(?i)\b(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|0x[0-9a-f]+|[0-9a-f]{12,}|\d+(?:\.\d+)?)\b").expect("valid regex");
    let total = rows.len().max(1);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for r in &rows {
        let msg = r.get("@message").map(as_text).unwrap_or_default();
        let pat = token.replace_all(&msg, "<*>").into_owned();
        if !groups.contains_key(&pat) {
            order.push(pat.clone());
        }
        groups.entry(pat).or_default().push(msg);
    }
    let mut out: Vec<Row> = order
        .into_iter()
        .map(|p| {
            let samples = &groups[&p];
            let lower = p.to_lowercase();
            let severity = if lower.contains("error")
                || lower.contains("fatal")
                || lower.contains("exception")
            {
                "ERROR"
            } else if lower.contains("warn") {
                "WARN"
            } else {
                "INFO"
            };
            let mut r = Row::new();
            r.insert("@pattern".into(), Value::String(p.clone()));
            r.insert(
                "@ratio".into(),
                Value::String(fmt_num(samples.len() as f64 / total as f64)),
            );
            r.insert("@sampleCount".into(), Value::from(samples.len()));
            r.insert("@severityLabel".into(), Value::String(severity.into()));
            let s: Vec<Value> = samples
                .iter()
                .take(3)
                .map(|m| Value::String(m.clone()))
                .collect();
            r.insert(
                "@logSamples".into(),
                Value::String(Value::Array(s).to_string()),
            );
            r
        })
        .collect();
    out.sort_by(|a, b| b["@sampleCount"].as_u64().cmp(&a["@sampleCount"].as_u64()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Vec<Event> {
        let base = 1_773_478_800_000;
        [
            "{\"level\":\"ERROR\",\"path\":\"/checkout\",\"latency\":812}",
            "{\"level\":\"INFO\",\"path\":\"/checkout\",\"latency\":120}",
            "{\"level\":\"ERROR\",\"path\":\"/cart\",\"latency\":95}",
            "plain text Timeout after 30000 ms",
        ]
        .iter()
        .enumerate()
        .map(|(i, m)| Event {
            timestamp_ms: base + i as i64 * 60_000,
            ingestion_ms: base + i as i64 * 60_000 + 500,
            message: (*m).to_owned(),
            stream: "s1".into(),
            group: "/app".into(),
        })
        .collect()
    }

    fn q(text: &str) -> Outcome {
        run(
            &Query {
                text,
                limit: None,
                account: "123456789012",
            },
            &events(),
        )
        .unwrap()
    }

    #[test]
    fn filters_and_fields() {
        let out = q("fields @timestamp, path | filter level = 'ERROR' | sort @timestamp asc");
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0][1], ("path".to_owned(), "/checkout".to_owned()));
        assert_eq!(out.records_matched, 2);
        let like = q("filter @message like /(?i)timeout/");
        assert_eq!(like.rows.len(), 1);
    }

    #[test]
    fn stats_by_field_and_bin() {
        let out =
            q("filter ispresent(level) | stats count(*) as n, avg(latency) by level | sort n desc");
        assert_eq!(
            out.rows[0],
            vec![
                ("level".into(), "ERROR".into()),
                ("n".into(), "2".into()),
                ("avg(latency)".into(), "453.5".into())
            ]
        );
        let binned = q("stats count() by bin(2m)");
        assert_eq!(binned.rows.len(), 2);
        assert_eq!(
            binned.rows[0][0],
            ("bin(2m)".into(), "2026-03-14 09:00:00.000".into())
        );
    }

    #[test]
    fn parse_glob_and_limit() {
        let out =
            q("parse @message 'Timeout after * ms' as wait | filter ispresent(wait) | fields wait");
        assert_eq!(out.rows, vec![vec![("wait".into(), "30000".into())]]);
        assert_eq!(q("limit 1").rows.len(), 1);
    }

    #[test]
    fn regex_with_alternation_inside_a_pipeline() {
        // The cloudwatch server's analyze_log_group error-pattern query.
        let out = q(
            "fields @timestamp, @message | filter @message like /(?i)(error|exception|fail|timeout|fatal)/ | pattern @message | limit 5",
        );
        let total: usize = out
            .rows
            .iter()
            .map(|r| {
                r.iter()
                    .find(|(k, _)| k == "@sampleCount")
                    .unwrap()
                    .1
                    .parse::<usize>()
                    .unwrap()
            })
            .sum();
        assert_eq!(total, 3);
        let parsed = q("parse @message /latency\":(?<lat>\\d+)/ | filter lat > 100 | fields lat");
        assert_eq!(parsed.rows.len(), 2);
    }

    #[test]
    fn errors_are_reported() {
        assert!(
            run(
                &Query {
                    text: "frobnicate x",
                    limit: None,
                    account: "1"
                },
                &events()
            )
            .is_err()
        );
    }
}
