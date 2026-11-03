//! ES|QL (`POST /_query`): a pipeline of `FROM`, `WHERE`, `EVAL`, `STATS ... BY`,
//! `SORT`, `LIMIT`, `KEEP`, `DROP` and `RENAME`, with comparison, boolean, arithmetic,
//! `LIKE`/`RLIKE`, `IN`, `IS [NOT] NULL`, time spans (`NOW() - 1 hour`) and the common
//! scalar and aggregate functions. Errors use the real `parsing_exception` /
//! `verification_exception` messages (`line 1:N: ...`).

use std::{cmp::Ordering, collections::BTreeSet};

use regex::Regex;
use serde_json::{Value, json};

use crate::{
    error::EsError,
    model::{
        Index, field_values, format_millis, parse_date_millis, parse_date_str, resolve,
        resolve_open, wildcard_match,
    },
};

// ───────────────────────────────── values ─────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum V {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Date(i64),
    Span(i64),
    Multi(Vec<V>),
}

impl V {
    fn to_json(&self) -> Value {
        match self {
            V::Null => Value::Null,
            V::Bool(b) => json!(b),
            V::Int(i) => json!(i),
            V::Float(f) => json!(f),
            V::Str(s) => json!(s),
            V::Date(ms) => json!(format_millis(*ms)),
            V::Span(ms) => json!(format!("{ms}ms")),
            V::Multi(items) => Value::Array(items.iter().map(V::to_json).collect()),
        }
    }

    fn num(&self) -> Option<f64> {
        match self {
            V::Int(i) => Some(*i as f64),
            V::Float(f) => Some(*f),
            V::Date(d) => Some(*d as f64),
            _ => None,
        }
    }

    fn key(&self) -> String {
        format!("{self:?}")
    }
}

fn from_json(v: &Value, ty: &str) -> V {
    match v {
        Value::Null => V::Null,
        Value::Array(items) => {
            let vals: Vec<V> = items.iter().map(|i| from_json(i, ty)).collect();
            match vals.len() {
                0 => V::Null,
                1 => vals.into_iter().next().expect("one value"),
                _ => V::Multi(vals),
            }
        }
        _ => match ty {
            "date" | "date_nanos" => parse_date_millis(v).map_or(V::Null, V::Date),
            "long" | "integer" | "short" | "byte" | "unsigned_long" => match v {
                Value::Number(n) => n
                    .as_i64()
                    .map_or_else(|| V::Float(n.as_f64().unwrap_or(0.0)), V::Int),
                Value::String(s) => s.parse().map_or(V::Null, V::Int),
                _ => V::Null,
            },
            "double" | "float" | "half_float" | "scaled_float" => match v {
                Value::Number(n) => V::Float(n.as_f64().unwrap_or(0.0)),
                Value::String(s) => s.parse().map_or(V::Null, V::Float),
                _ => V::Null,
            },
            "boolean" => match v {
                Value::Bool(b) => V::Bool(*b),
                Value::String(s) => s.parse().map_or(V::Null, V::Bool),
                _ => V::Null,
            },
            _ => match v {
                Value::String(s) => V::Str(s.clone()),
                Value::Bool(b) => V::Bool(*b),
                Value::Number(n) => n
                    .as_i64()
                    .map_or_else(|| V::Float(n.as_f64().unwrap_or(0.0)), V::Int),
                other => V::Str(other.to_string()),
            },
        },
    }
}

/// ES|QL column type for a mapped field type.
fn esql_type(ty: &str) -> &'static str {
    match ty {
        "keyword" | "constant_keyword" | "wildcard" => "keyword",
        "text" | "match_only_text" => "text",
        "long" | "unsigned_long" => "long",
        "integer" | "short" | "byte" => "integer",
        "double" | "float" | "half_float" | "scaled_float" => "double",
        "date" | "date_nanos" => "date",
        "boolean" => "boolean",
        "ip" => "ip",
        "version" => "version",
        _ => "unsupported",
    }
}

// ───────────────────────────────── lexer ─────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum T {
    Ident(String),
    Quoted(String),
    Str(String),
    Int(i64),
    Float(f64),
    Op(&'static str),
    Comma,
    LParen,
    RParen,
    Pipe,
    Star,
}

#[derive(Debug, Clone)]
struct Tok {
    t: T,
    col: usize,
    text: String,
}

fn lex(src: &str) -> Result<Vec<Tok>, EsError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let col = i + 1;
        let start = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let t = match c {
            '|' => {
                i += 1;
                T::Pipe
            }
            ',' => {
                i += 1;
                T::Comma
            }
            '(' => {
                i += 1;
                T::LParen
            }
            ')' => {
                i += 1;
                T::RParen
            }
            '*' => {
                i += 1;
                T::Star
            }
            '"' => {
                let triple = chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"');
                let mut s = String::new();
                if triple {
                    i += 3;
                    while i < chars.len()
                        && !(chars[i] == '"'
                            && chars.get(i + 1) == Some(&'"')
                            && chars.get(i + 2) == Some(&'"'))
                    {
                        s.push(chars[i]);
                        i += 1;
                    }
                    i += 3;
                } else {
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        if chars[i] == '\\' && i + 1 < chars.len() {
                            i += 1;
                            s.push(match chars[i] {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        } else {
                            s.push(chars[i]);
                        }
                        i += 1;
                    }
                    if i >= chars.len() {
                        return Err(parse_err(col, "token recognition error at: '\"'"));
                    }
                    i += 1;
                }
                T::Str(s)
            }
            '`' => {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '`' {
                    s.push(chars[i]);
                    i += 1;
                }
                i += 1;
                T::Quoted(s)
            }
            '=' | '!' | '<' | '>' | '+' | '-' | '/' | '%' => {
                let next = chars.get(i + 1).copied();
                let (op, len) = match (c, next) {
                    ('=', Some('=')) => ("==", 2),
                    ('!', Some('=')) => ("!=", 2),
                    ('<', Some('=')) => ("<=", 2),
                    ('>', Some('=')) => (">=", 2),
                    ('=', _) => ("=", 1),
                    ('<', _) => ("<", 1),
                    ('>', _) => (">", 1),
                    ('+', _) => ("+", 1),
                    ('-', _) => ("-", 1),
                    ('/', _) => ("/", 1),
                    ('%', _) => ("%", 1),
                    _ => {
                        return Err(parse_err(
                            col,
                            &format!("token recognition error at: '{c}'"),
                        ));
                    }
                };
                i += len;
                T::Op(op)
            }
            d if d.is_ascii_digit() => {
                let mut s = String::new();
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    s.push(chars[i]);
                    i += 1;
                }
                if s.contains('.') {
                    T::Float(
                        s.parse()
                            .map_err(|_| parse_err(col, &format!("invalid number [{s}]")))?,
                    )
                } else {
                    T::Int(
                        s.parse()
                            .map_err(|_| parse_err(col, &format!("invalid number [{s}]")))?,
                    )
                }
            }
            a if a.is_alphabetic() || a == '_' || a == '@' => {
                let mut s = String::new();
                while i < chars.len()
                    && (chars[i].is_alphanumeric()
                        || matches!(chars[i], '_' | '@' | '.' | '-' | '*'))
                {
                    // A '-' only continues an index-pattern-like identifier (logs-app-*).
                    if chars[i] == '-'
                        && !chars
                            .get(i + 1)
                            .is_some_and(|n| n.is_alphanumeric() || *n == '*')
                    {
                        break;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                T::Ident(s)
            }
            other => {
                return Err(parse_err(
                    col,
                    &format!("token recognition error at: '{other}'"),
                ));
            }
        };
        out.push(Tok {
            t,
            col,
            text: chars[start..i.min(chars.len())].iter().collect(),
        });
    }
    Ok(out)
}

fn parse_err(col: usize, msg: &str) -> EsError {
    EsError::new(
        axum::http::StatusCode::BAD_REQUEST,
        "parsing_exception",
        format!("line 1:{col}: {msg}"),
    )
}

fn verify_err(col: usize, msg: &str) -> EsError {
    EsError::verification(&[format!("line 1:{col}: {msg}")])
}

// ───────────────────────────────── parser ─────────────────────────────────

#[derive(Debug, Clone)]
enum E {
    Lit(V),
    Field(String, usize),
    Not(Box<E>),
    Neg(Box<E>),
    Bin(&'static str, Box<E>, Box<E>),
    Like(Box<E>, String, bool, bool),
    In(Box<E>, Vec<E>, bool),
    IsNull(Box<E>, bool),
    Call(String, Vec<E>, usize),
    Star,
}

#[derive(Debug)]
enum Cmd {
    From(Vec<String>, bool, usize),
    Where(E),
    Eval(Vec<(String, E)>),
    Stats(Vec<(String, E)>, Vec<(String, E)>),
    Sort(Vec<(E, bool, Option<bool>)>),
    Limit(usize),
    Keep(Vec<String>),
    Drop(Vec<String>),
    Rename(Vec<(String, String)>),
}

struct P<'a> {
    toks: &'a [Tok],
    pos: usize,
    src: &'a str,
}

fn kw(t: &Tok, word: &str) -> bool {
    matches!(&t.t, T::Ident(s) if s.eq_ignore_ascii_case(word))
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Tok> {
        let t = self.toks.get(self.pos);
        self.pos += 1;
        t
    }

    fn eof_col(&self) -> usize {
        self.src.chars().count() + 1
    }

    fn unexpected(&self) -> EsError {
        match self.peek() {
            Some(t) => parse_err(
                t.col,
                &format!("mismatched input '{}' expecting <EOF>", t.text),
            ),
            None => parse_err(
                self.eof_col(),
                "mismatched input '<EOF>' expecting {QUOTED_IDENTIFIER, UNQUOTED_IDENTIFIER}",
            ),
        }
    }

    fn at_kw(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| kw(t, word))
    }

    fn eat_kw(&mut self, word: &str) -> bool {
        if self.at_kw(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &T, what: &str) -> Result<(), EsError> {
        match self.peek() {
            Some(tok) if &tok.t == t => {
                self.pos += 1;
                Ok(())
            }
            Some(tok) => Err(parse_err(
                tok.col,
                &format!("mismatched input '{}' expecting {what}", tok.text),
            )),
            None => Err(parse_err(
                self.eof_col(),
                &format!("mismatched input '<EOF>' expecting {what}"),
            )),
        }
    }

    fn ident(&mut self) -> Result<(String, usize), EsError> {
        match self.next() {
            Some(Tok {
                t: T::Ident(s),
                col,
                ..
            })
            | Some(Tok {
                t: T::Quoted(s),
                col,
                ..
            }) => Ok((s.clone(), *col)),
            Some(t) => Err(parse_err(
                t.col,
                &format!(
                    "mismatched input '{}' expecting {{QUOTED_IDENTIFIER, UNQUOTED_IDENTIFIER}}",
                    t.text
                ),
            )),
            None => Err(parse_err(
                self.eof_col(),
                "mismatched input '<EOF>' expecting {QUOTED_IDENTIFIER, UNQUOTED_IDENTIFIER}",
            )),
        }
    }

    /// Source text from token `start` up to (not including) the current token.
    fn text_since(&self, start: usize) -> String {
        let from = self.toks[start].col - 1;
        let to = self
            .toks
            .get(self.pos)
            .map_or(self.src.chars().count(), |t| t.col - 1);
        self.src
            .chars()
            .skip(from)
            .take(to - from)
            .collect::<String>()
            .trim()
            .to_owned()
    }

    fn commands(&mut self) -> Result<Vec<Cmd>, EsError> {
        let mut cmds = Vec::new();
        loop {
            let tok = self.next().ok_or_else(|| self.unexpected())?;
            let T::Ident(word) = &tok.t else {
                return Err(parse_err(
                    tok.col,
                    &format!(
                        "mismatched input '{}' expecting {{'from', 'row', 'show'}}",
                        tok.text
                    ),
                ));
            };
            let cmd = match word.to_ascii_uppercase().as_str() {
                "FROM" if cmds.is_empty() => {
                    let mut idx = Vec::new();
                    loop {
                        let (name, _) = match self.next() {
                            Some(Tok {
                                t: T::Ident(s),
                                col,
                                ..
                            })
                            | Some(Tok {
                                t: T::Quoted(s),
                                col,
                                ..
                            })
                            | Some(Tok {
                                t: T::Str(s), col, ..
                            }) => (s.clone(), *col),
                            _ => {
                                return Err(parse_err(
                                    tok.col + 5,
                                    "mismatched input expecting {QUOTED_STRING, UNQUOTED_SOURCE}",
                                ));
                            }
                        };
                        idx.push(name);
                        if self.peek().is_some_and(|t| t.t == T::Comma) {
                            self.pos += 1;
                            continue;
                        }
                        break;
                    }
                    let mut metadata = false;
                    if self.eat_kw("METADATA") {
                        metadata = true;
                        loop {
                            self.ident()?;
                            if self.peek().is_some_and(|t| t.t == T::Comma) {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    Cmd::From(idx, metadata, tok.col)
                }
                "WHERE" => Cmd::Where(self.expr()?),
                "EVAL" => Cmd::Eval(self.assignments()?),
                "STATS" => {
                    let aggs = if self.at_kw("BY") {
                        Vec::new()
                    } else {
                        self.assignments()?
                    };
                    let by = if self.eat_kw("BY") {
                        self.assignments()?
                    } else {
                        Vec::new()
                    };
                    if aggs.is_empty() && by.is_empty() {
                        return Err(parse_err(
                            tok.col,
                            "at least one aggregation or grouping expression required in [STATS]",
                        ));
                    }
                    Cmd::Stats(aggs, by)
                }
                "SORT" => {
                    let mut keys = Vec::new();
                    loop {
                        let e = self.expr()?;
                        let desc = if self.eat_kw("DESC") {
                            true
                        } else {
                            self.eat_kw("ASC");
                            false
                        };
                        let nulls = if self.eat_kw("NULLS") {
                            if self.eat_kw("FIRST") {
                                Some(true)
                            } else if self.eat_kw("LAST") {
                                Some(false)
                            } else {
                                return Err(self.unexpected());
                            }
                        } else {
                            None
                        };
                        keys.push((e, desc, nulls));
                        if self.peek().is_some_and(|t| t.t == T::Comma) {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    Cmd::Sort(keys)
                }
                "LIMIT" => match self.next() {
                    Some(Tok { t: T::Int(n), .. }) => Cmd::Limit(*n as usize),
                    Some(t) => {
                        return Err(parse_err(
                            t.col,
                            &format!("mismatched input '{}' expecting INTEGER_LITERAL", t.text),
                        ));
                    }
                    None => {
                        return Err(parse_err(
                            self.eof_col(),
                            "mismatched input '<EOF>' expecting INTEGER_LITERAL",
                        ));
                    }
                },
                "KEEP" | "PROJECT" => Cmd::Keep(self.patterns()?),
                "DROP" => Cmd::Drop(self.patterns()?),
                "RENAME" => {
                    let mut pairs = Vec::new();
                    loop {
                        let (from, _) = self.ident()?;
                        if !self.eat_kw("AS") {
                            return Err(self.unexpected());
                        }
                        let (to, _) = self.ident()?;
                        pairs.push((from, to));
                        if self.peek().is_some_and(|t| t.t == T::Comma) {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    Cmd::Rename(pairs)
                }
                _ => {
                    let expecting = if cmds.is_empty() {
                        "{'from', 'row', 'show'}"
                    } else {
                        "{'eval', 'where', 'keep', 'drop', 'limit', 'rename', 'sort', 'stats', 'dissect', 'grok', 'enrich', 'mv_expand'}"
                    };
                    return Err(parse_err(
                        tok.col,
                        &format!("mismatched input '{}' expecting {expecting}", tok.text),
                    ));
                }
            };
            cmds.push(cmd);
            match self.next() {
                None => break,
                Some(Tok { t: T::Pipe, .. }) => continue,
                Some(t) => {
                    return Err(parse_err(
                        t.col,
                        &format!("mismatched input '{}' expecting <EOF>", t.text),
                    ));
                }
            }
        }
        if !matches!(cmds.first(), Some(Cmd::From(..))) {
            return Err(parse_err(
                1,
                "mismatched input expecting {'from', 'row', 'show'}",
            ));
        }
        Ok(cmds)
    }

    fn patterns(&mut self) -> Result<Vec<String>, EsError> {
        let mut out = Vec::new();
        loop {
            match self.next() {
                Some(Tok { t: T::Ident(s), .. })
                | Some(Tok {
                    t: T::Quoted(s), ..
                }) => out.push(s.clone()),
                Some(Tok { t: T::Star, .. }) => out.push("*".to_owned()),
                Some(t) => {
                    return Err(parse_err(
                        t.col,
                        &format!(
                            "mismatched input '{}' expecting {{QUOTED_IDENTIFIER, UNQUOTED_ID_PATTERN}}",
                            t.text
                        ),
                    ));
                }
                None => {
                    return Err(parse_err(
                        self.eof_col(),
                        "mismatched input '<EOF>' expecting {QUOTED_IDENTIFIER, UNQUOTED_ID_PATTERN}",
                    ));
                }
            }
            if self.peek().is_some_and(|t| t.t == T::Comma) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// `name = expr` or `expr` (named by its source text), comma separated.
    fn assignments(&mut self) -> Result<Vec<(String, E)>, EsError> {
        let mut out = Vec::new();
        loop {
            let start = self.pos;
            let named = matches!(
                self.toks.get(self.pos).map(|t| &t.t),
                Some(T::Ident(_)) | Some(T::Quoted(_))
            ) && matches!(self.toks.get(self.pos + 1).map(|t| &t.t), Some(T::Op("=")));
            if named {
                let (name, _) = self.ident()?;
                self.pos += 1;
                out.push((name, self.expr()?));
            } else {
                let e = self.expr()?;
                let name = match &e {
                    E::Field(f, _) => f.clone(),
                    _ => self.text_since(start),
                };
                out.push((name, e));
            }
            if self.peek().is_some_and(|t| t.t == T::Comma) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn expr(&mut self) -> Result<E, EsError> {
        let mut left = self.and_expr()?;
        while self.eat_kw("OR") {
            let right = self.and_expr()?;
            left = E::Bin("OR", Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<E, EsError> {
        let mut left = self.not_expr()?;
        while self.eat_kw("AND") {
            let right = self.not_expr()?;
            left = E::Bin("AND", Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> Result<E, EsError> {
        if self.eat_kw("NOT") {
            return Ok(E::Not(Box::new(self.not_expr()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<E, EsError> {
        let left = self.additive()?;
        let negated = self.at_kw("NOT")
            && self
                .toks
                .get(self.pos + 1)
                .is_some_and(|t| kw(t, "LIKE") || kw(t, "RLIKE") || kw(t, "IN"));
        if negated {
            self.pos += 1;
        }
        if self.at_kw("LIKE") || self.at_kw("RLIKE") {
            let regex = self.at_kw("RLIKE");
            self.pos += 1;
            let pattern = match self.next() {
                Some(Tok { t: T::Str(s), .. }) => s.clone(),
                Some(t) => {
                    return Err(parse_err(
                        t.col,
                        &format!("mismatched input '{}' expecting QUOTED_STRING", t.text),
                    ));
                }
                None => {
                    return Err(parse_err(
                        self.eof_col(),
                        "mismatched input '<EOF>' expecting QUOTED_STRING",
                    ));
                }
            };
            return Ok(E::Like(Box::new(left), pattern, regex, negated));
        }
        if self.eat_kw("IN") {
            self.expect(&T::LParen, "'('")?;
            let mut items = Vec::new();
            loop {
                items.push(self.additive()?);
                if self.peek().is_some_and(|t| t.t == T::Comma) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            self.expect(&T::RParen, "')'")?;
            return Ok(E::In(Box::new(left), items, negated));
        }
        if self.eat_kw("IS") {
            let not = self.eat_kw("NOT");
            if !self.eat_kw("NULL") {
                return Err(self.unexpected());
            }
            return Ok(E::IsNull(Box::new(left), not));
        }
        if let Some(Tok { t: T::Op(op), .. }) = self.peek()
            && matches!(*op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "=")
        {
            if *op == "=" {
                let t = self.peek().expect("peeked");
                return Err(parse_err(
                    t.col,
                    "mismatched input '=' expecting {'==', '!=', '<', '<=', '>', '>='}; use '==' to compare",
                ));
            }
            let op = *op;
            self.pos += 1;
            let right = self.additive()?;
            return Ok(E::Bin(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn additive(&mut self) -> Result<E, EsError> {
        let mut left = self.multiplicative()?;
        while let Some(Tok {
            t: T::Op(op @ ("+" | "-")),
            ..
        }) = self.peek()
        {
            let op = *op;
            self.pos += 1;
            let right = self.multiplicative()?;
            left = E::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<E, EsError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok {
                    t: T::Op(op @ ("/" | "%")),
                    ..
                }) => *op,
                Some(Tok { t: T::Star, .. }) => "*",
                _ => break,
            };
            self.pos += 1;
            let right = self.unary()?;
            left = E::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<E, EsError> {
        if let Some(Tok { t: T::Op("-"), .. }) = self.peek() {
            self.pos += 1;
            return Ok(E::Neg(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<E, EsError> {
        let tok = self.next().ok_or_else(|| {
            parse_err(
                self.eof_col(),
                "mismatched input '<EOF>' expecting an expression",
            )
        })?;
        match &tok.t {
            T::Int(_) | T::Float(_) if self.span_unit().is_some() => {
                let n = match &tok.t {
                    T::Int(n) => *n as f64,
                    T::Float(f) => *f,
                    _ => unreachable!(),
                };
                let unit = self.span_unit().expect("checked");
                self.pos += 1;
                Ok(E::Lit(V::Span((n * unit as f64) as i64)))
            }
            T::Int(n) => Ok(E::Lit(V::Int(*n))),
            T::Float(f) => Ok(E::Lit(V::Float(*f))),
            T::Str(s) => Ok(E::Lit(V::Str(s.clone()))),
            T::Star => Ok(E::Star),
            T::LParen => {
                let e = self.expr()?;
                self.expect(&T::RParen, "')'")?;
                Ok(e)
            }
            T::Quoted(s) => Ok(E::Field(s.clone(), tok.col)),
            T::Ident(s) => {
                if self.peek().is_some_and(|t| t.t == T::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !self.peek().is_some_and(|t| t.t == T::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.peek().is_some_and(|t| t.t == T::Comma) {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(&T::RParen, "')'")?;
                    return Ok(E::Call(s.to_ascii_uppercase(), args, tok.col));
                }
                match s.to_ascii_lowercase().as_str() {
                    "true" => Ok(E::Lit(V::Bool(true))),
                    "false" => Ok(E::Lit(V::Bool(false))),
                    "null" => Ok(E::Lit(V::Null)),
                    _ => Ok(E::Field(s.clone(), tok.col)),
                }
            }
            _ => Err(parse_err(
                tok.col,
                &format!("extraneous input '{}' expecting an expression", tok.text),
            )),
        }
    }

    fn span_unit(&self) -> Option<i64> {
        let Some(Tok { t: T::Ident(u), .. }) = self.peek() else {
            return None;
        };
        Some(match u.to_ascii_lowercase().as_str() {
            "ms" | "millisecond" | "milliseconds" => 1,
            "s" | "sec" | "second" | "seconds" => 1_000,
            "m" | "min" | "minute" | "minutes" => 60_000,
            "h" | "hour" | "hours" => 3_600_000,
            "d" | "day" | "days" => 86_400_000,
            "w" | "week" | "weeks" => 7 * 86_400_000,
            "mo" | "month" | "months" => 30 * 86_400_000,
            "y" | "yr" | "year" | "years" => 365 * 86_400_000,
            _ => return None,
        })
    }
}

// ─────────────────────────────── evaluation ───────────────────────────────

#[derive(Clone)]
struct Table {
    cols: Vec<(String, String)>,
    rows: Vec<Vec<V>>,
}

impl Table {
    fn col(&self, name: &str) -> Option<usize> {
        self.cols.iter().position(|(n, _)| n == name)
    }
}

struct Env {
    now_ms: i64,
}

const AGGS: [&str; 10] = [
    "COUNT",
    "COUNT_DISTINCT",
    "AVG",
    "SUM",
    "MIN",
    "MAX",
    "MEDIAN",
    "VALUES",
    "PERCENTILE",
    "TOP",
];

fn check_fields(e: &E, t: &Table) -> Result<(), EsError> {
    match e {
        E::Field(f, col) => {
            if t.col(f).is_none() {
                return Err(verify_err(*col, &format!("Unknown column [{f}]")));
            }
            Ok(())
        }
        E::Not(x) | E::Neg(x) | E::IsNull(x, _) | E::Like(x, _, _, _) => check_fields(x, t),
        E::Bin(_, a, b) => {
            check_fields(a, t)?;
            check_fields(b, t)
        }
        E::In(x, items, _) => {
            check_fields(x, t)?;
            items.iter().try_for_each(|i| check_fields(i, t))
        }
        E::Call(name, args, col) => {
            if !AGGS.contains(&name.as_str()) && !SCALARS.contains(&name.as_str()) {
                return Err(verify_err(
                    *col,
                    &format!("Unknown function [{}]", name.to_lowercase()),
                ));
            }
            args.iter().try_for_each(|a| check_fields(a, t))
        }
        E::Lit(_) | E::Star => Ok(()),
    }
}

const SCALARS: [&str; 27] = [
    "TO_LOWER",
    "TO_UPPER",
    "LENGTH",
    "CONCAT",
    "COALESCE",
    "ROUND",
    "ABS",
    "NOW",
    "DATE_TRUNC",
    "BUCKET",
    "TO_STRING",
    "TO_INTEGER",
    "TO_INT",
    "TO_LONG",
    "TO_DOUBLE",
    "TO_DATETIME",
    "SUBSTRING",
    "STARTS_WITH",
    "ENDS_WITH",
    "CASE",
    "TRIM",
    "FLOOR",
    "CEIL",
    "MV_COUNT",
    "DATE_EXTRACT",
    "GREATEST",
    "LEAST",
];

fn truthy(v: &V) -> bool {
    matches!(v, V::Bool(true))
}

fn cmp(a: &V, b: &V) -> Option<Ordering> {
    match (a, b) {
        (V::Null, _) | (_, V::Null) => None,
        (V::Date(x), V::Str(s)) => parse_date_str(s).map(|y| x.cmp(&y)),
        (V::Str(s), V::Date(y)) => parse_date_str(s).map(|x| x.cmp(y)),
        (V::Str(x), V::Str(y)) => Some(x.cmp(y)),
        (V::Bool(x), V::Bool(y)) => Some(x.cmp(y)),
        (V::Multi(items), other) => items
            .iter()
            .find_map(|i| cmp(i, other).filter(|o| *o == Ordering::Equal))
            .or_else(|| items.first().and_then(|i| cmp(i, other))),
        _ => a.num()?.partial_cmp(&b.num()?),
    }
}

fn arith(op: &str, a: V, b: V) -> V {
    match (op, &a, &b) {
        (_, V::Null, _) | (_, _, V::Null) => V::Null,
        ("+", V::Date(d), V::Span(s)) | ("+", V::Span(s), V::Date(d)) => V::Date(d + s),
        ("-", V::Date(d), V::Span(s)) => V::Date(d - s),
        ("+", V::Span(x), V::Span(y)) => V::Span(x + y),
        ("-", V::Date(x), V::Date(y)) => V::Int(x - y),
        (_, V::Int(x), V::Int(y)) => match op {
            "+" => V::Int(x + y),
            "-" => V::Int(x - y),
            "*" => V::Int(x * y),
            "/" if *y != 0 => V::Int(x / y),
            "%" if *y != 0 => V::Int(x % y),
            _ => V::Null,
        },
        _ => match (a.num(), b.num()) {
            (Some(x), Some(y)) => match op {
                "+" => V::Float(x + y),
                "-" => V::Float(x - y),
                "*" => V::Float(x * y),
                "/" if y != 0.0 => V::Float(x / y),
                "%" if y != 0.0 => V::Float(x % y),
                _ => V::Null,
            },
            _ => V::Null,
        },
    }
}

fn to_str(v: &V) -> Option<String> {
    match v {
        V::Null => None,
        V::Str(s) => Some(s.clone()),
        V::Int(i) => Some(i.to_string()),
        V::Float(f) => Some(f.to_string()),
        V::Bool(b) => Some(b.to_string()),
        V::Date(d) => Some(format_millis(*d)),
        V::Span(s) => Some(format!("{s}ms")),
        V::Multi(items) => items.first().and_then(to_str),
    }
}

fn eval(e: &E, row: &[V], t: &Table, env: &Env) -> V {
    match e {
        E::Lit(v) => v.clone(),
        E::Star => V::Null,
        E::Field(f, _) => t.col(f).map_or(V::Null, |i| row[i].clone()),
        E::Not(x) => match eval(x, row, t, env) {
            V::Bool(b) => V::Bool(!b),
            _ => V::Null,
        },
        E::Neg(x) => match eval(x, row, t, env) {
            V::Int(i) => V::Int(-i),
            V::Float(f) => V::Float(-f),
            V::Span(s) => V::Span(-s),
            _ => V::Null,
        },
        E::Bin(op, a, b) => {
            let (x, y) = (eval(a, row, t, env), eval(b, row, t, env));
            match *op {
                "AND" => match (&x, &y) {
                    (V::Bool(false), _) | (_, V::Bool(false)) => V::Bool(false),
                    (V::Bool(true), V::Bool(true)) => V::Bool(true),
                    _ => V::Null,
                },
                "OR" => match (&x, &y) {
                    (V::Bool(true), _) | (_, V::Bool(true)) => V::Bool(true),
                    (V::Bool(false), V::Bool(false)) => V::Bool(false),
                    _ => V::Null,
                },
                "==" | "!=" | "<" | "<=" | ">" | ">=" => match cmp(&x, &y) {
                    None => V::Null,
                    Some(o) => V::Bool(match *op {
                        "==" => o == Ordering::Equal,
                        "!=" => o != Ordering::Equal,
                        "<" => o == Ordering::Less,
                        "<=" => o != Ordering::Greater,
                        ">" => o == Ordering::Greater,
                        _ => o != Ordering::Less,
                    }),
                },
                other => arith(other, x, y),
            }
        }
        E::Like(x, pattern, regex, negated) => match to_str(&eval(x, row, t, env)) {
            None => V::Null,
            Some(s) => {
                let m = if *regex {
                    Regex::new(&format!("^(?:{pattern})$")).is_ok_and(|r| r.is_match(&s))
                } else {
                    wildcard_match(pattern, &s)
                };
                V::Bool(m != *negated)
            }
        },
        E::In(x, items, negated) => {
            let v = eval(x, row, t, env);
            if v == V::Null {
                return V::Null;
            }
            let hit = items
                .iter()
                .any(|i| cmp(&v, &eval(i, row, t, env)) == Some(Ordering::Equal));
            V::Bool(hit != *negated)
        }
        E::IsNull(x, not) => V::Bool((eval(x, row, t, env) == V::Null) != *not),
        E::Call(name, args, _) => call(name, args, row, t, env),
    }
}

fn call(name: &str, args: &[E], row: &[V], t: &Table, env: &Env) -> V {
    let a = |i: usize| args.get(i).map_or(V::Null, |e| eval(e, row, t, env));
    match name {
        "NOW" => V::Date(env.now_ms),
        "TO_LOWER" => to_str(&a(0)).map_or(V::Null, |s| V::Str(s.to_lowercase())),
        "TO_UPPER" => to_str(&a(0)).map_or(V::Null, |s| V::Str(s.to_uppercase())),
        "TRIM" => to_str(&a(0)).map_or(V::Null, |s| V::Str(s.trim().to_owned())),
        "LENGTH" => to_str(&a(0)).map_or(V::Null, |s| V::Int(s.chars().count() as i64)),
        "TO_STRING" => to_str(&a(0)).map_or(V::Null, V::Str),
        "CONCAT" => {
            let mut out = String::new();
            for i in 0..args.len() {
                match to_str(&a(i)) {
                    Some(s) => out.push_str(&s),
                    None => return V::Null,
                }
            }
            V::Str(out)
        }
        "COALESCE" => (0..args.len())
            .map(a)
            .find(|v| *v != V::Null)
            .unwrap_or(V::Null),
        "GREATEST" | "LEAST" => {
            let vals: Vec<V> = (0..args.len()).map(a).filter(|v| *v != V::Null).collect();
            let pick = vals.into_iter().reduce(|x, y| {
                let o = cmp(&x, &y).unwrap_or(Ordering::Equal);
                if (name == "GREATEST") == (o == Ordering::Less) {
                    y
                } else {
                    x
                }
            });
            pick.unwrap_or(V::Null)
        }
        "ABS" => match a(0) {
            V::Int(i) => V::Int(i.abs()),
            V::Float(f) => V::Float(f.abs()),
            _ => V::Null,
        },
        "FLOOR" | "CEIL" => match a(0) {
            V::Int(i) => V::Int(i),
            V::Float(f) => V::Float(if name == "FLOOR" { f.floor() } else { f.ceil() }),
            _ => V::Null,
        },
        "ROUND" => {
            let places = match a(1) {
                V::Int(p) => p as i32,
                _ => 0,
            };
            match a(0) {
                V::Int(i) => V::Int(i),
                V::Float(f) => {
                    let m = 10f64.powi(places);
                    V::Float((f * m).round() / m)
                }
                _ => V::Null,
            }
        }
        "TO_INTEGER" | "TO_INT" | "TO_LONG" => match a(0) {
            V::Int(i) => V::Int(i),
            V::Float(f) => V::Int(f as i64),
            V::Str(s) => s
                .trim()
                .parse::<f64>()
                .map_or(V::Null, |f| V::Int(f as i64)),
            V::Bool(b) => V::Int(i64::from(b)),
            V::Date(d) => V::Int(d),
            _ => V::Null,
        },
        "TO_DOUBLE" => match a(0) {
            V::Int(i) => V::Float(i as f64),
            V::Float(f) => V::Float(f),
            V::Str(s) => s.trim().parse().map_or(V::Null, V::Float),
            V::Date(d) => V::Float(d as f64),
            _ => V::Null,
        },
        "TO_DATETIME" => match a(0) {
            V::Str(s) => parse_date_str(&s).map_or(V::Null, V::Date),
            V::Int(i) => V::Date(i),
            V::Date(d) => V::Date(d),
            _ => V::Null,
        },
        "SUBSTRING" => {
            let Some(s) = to_str(&a(0)) else {
                return V::Null;
            };
            let start = match a(1) {
                V::Int(i) => i.max(1) as usize - 1,
                _ => 0,
            };
            let len = match a(2) {
                V::Int(l) => l.max(0) as usize,
                _ => usize::MAX,
            };
            V::Str(s.chars().skip(start).take(len).collect())
        }
        "STARTS_WITH" | "ENDS_WITH" => match (to_str(&a(0)), to_str(&a(1))) {
            (Some(s), Some(p)) => V::Bool(if name == "STARTS_WITH" {
                s.starts_with(&p)
            } else {
                s.ends_with(&p)
            }),
            _ => V::Null,
        },
        "MV_COUNT" => match a(0) {
            V::Null => V::Null,
            V::Multi(items) => V::Int(items.len() as i64),
            _ => V::Int(1),
        },
        "CASE" => {
            let mut i = 0;
            while i + 1 < args.len() {
                if truthy(&a(i)) {
                    return a(i + 1);
                }
                i += 2;
            }
            if args.len() % 2 == 1 {
                a(args.len() - 1)
            } else {
                V::Null
            }
        }
        "DATE_TRUNC" | "BUCKET" => {
            // DATE_TRUNC(span, date) / BUCKET(date, span)
            let (span, date) = if name == "DATE_TRUNC" {
                (a(0), a(1))
            } else {
                (a(1), a(0))
            };
            match (span, date) {
                (V::Span(s), V::Date(d)) if s > 0 => V::Date(d.div_euclid(s) * s),
                (V::Int(s), V::Int(x)) if s > 0 => V::Int(x.div_euclid(s) * s),
                (V::Int(s), V::Float(x)) if s > 0 => {
                    let s = s as f64;
                    V::Float((x / s).floor() * s)
                }
                (V::Float(s), V::Float(x)) if s > 0.0 => V::Float((x / s).floor() * s),
                _ => V::Null,
            }
        }
        "DATE_EXTRACT" => {
            let part = to_str(&a(0)).unwrap_or_default().to_lowercase();
            let V::Date(d) = a(1) else { return V::Null };
            let iso = format_millis(d);
            let field = |r: std::ops::Range<usize>| iso[r].parse::<i64>().map_or(V::Null, V::Int);
            match part.as_str() {
                "year" => field(0..4),
                "month_of_year" => field(5..7),
                "day_of_month" => field(8..10),
                "hour_of_day" => field(11..13),
                "minute_of_hour" => field(14..16),
                _ => V::Null,
            }
        }
        _ => V::Null,
    }
}

/// The ES|QL type of an expression's values.
fn type_of(e: &E, t: &Table) -> String {
    match e {
        E::Lit(V::Int(_)) => "integer".into(),
        E::Lit(V::Float(_)) => "double".into(),
        E::Lit(V::Str(_)) => "keyword".into(),
        E::Lit(V::Bool(_)) => "boolean".into(),
        E::Lit(V::Span(_)) => "time_duration".into(),
        E::Lit(_) | E::Star => "null".into(),
        E::Field(f, _) => t.col(f).map_or("null".into(), |i| t.cols[i].1.clone()),
        E::Not(_) | E::Like(..) | E::In(..) | E::IsNull(..) => "boolean".into(),
        E::Neg(x) => type_of(x, t),
        E::Bin(op, a, b) => match *op {
            "AND" | "OR" | "==" | "!=" | "<" | "<=" | ">" | ">=" => "boolean".into(),
            _ => {
                let (x, y) = (type_of(a, t), type_of(b, t));
                if x == "date" || y == "date" {
                    if *op == "-" && x == "date" && y == "date" {
                        "long".into()
                    } else {
                        "date".into()
                    }
                } else if x == "double" || y == "double" {
                    "double".into()
                } else if x == "long" || y == "long" {
                    "long".into()
                } else {
                    "integer".into()
                }
            }
        },
        E::Call(name, args, _) => {
            let arg = |i: usize| args.get(i).map_or("null".to_owned(), |a| type_of(a, t));
            match name.as_str() {
                "COUNT" | "COUNT_DISTINCT" | "TO_LONG" => "long".into(),
                "AVG" | "MEDIAN" | "PERCENTILE" | "TO_DOUBLE" => "double".into(),
                "SUM" => {
                    if arg(0) == "double" {
                        "double".into()
                    } else {
                        "long".into()
                    }
                }
                "MIN" | "MAX" | "VALUES" | "TOP" | "COALESCE" | "GREATEST" | "LEAST" | "ABS"
                | "FLOOR" | "CEIL" => {
                    let ty = arg(0);
                    if ty == "text" { "keyword".into() } else { ty }
                }
                "ROUND" => arg(0),
                "NOW" | "TO_DATETIME" => "date".into(),
                "DATE_TRUNC" => "date".into(),
                "BUCKET" => arg(0),
                "LENGTH" | "TO_INTEGER" | "TO_INT" | "MV_COUNT" | "DATE_EXTRACT" => {
                    "integer".into()
                }
                "STARTS_WITH" | "ENDS_WITH" => "boolean".into(),
                "CASE" => arg(1),
                _ => "keyword".into(),
            }
        }
    }
}

fn aggregate(e: &E, rows: &[&Vec<V>], t: &Table, env: &Env) -> Result<V, EsError> {
    let E::Call(name, args, col) = e else {
        return Ok(rows.first().map_or(V::Null, |r| eval(e, r, t, env)));
    };
    let arg = args.first();
    let values = |rows: &[&Vec<V>]| -> Vec<V> {
        rows.iter()
            .map(|r| arg.map_or(V::Null, |a| eval(a, r, t, env)))
            .flat_map(|v| match v {
                V::Multi(items) => items,
                V::Null => Vec::new(),
                other => vec![other],
            })
            .collect()
    };
    Ok(match name.as_str() {
        "COUNT" => match arg {
            None | Some(E::Star) => V::Int(rows.len() as i64),
            Some(_) => V::Int(values(rows).len() as i64),
        },
        "COUNT_DISTINCT" => {
            let set: BTreeSet<String> = values(rows).iter().map(V::key).collect();
            V::Int(set.len() as i64)
        }
        "SUM" => {
            let vals = values(rows);
            if vals.iter().all(|v| matches!(v, V::Int(_))) {
                V::Int(
                    vals.iter()
                        .filter_map(|v| if let V::Int(i) = v { Some(*i) } else { None })
                        .sum(),
                )
            } else {
                V::Float(vals.iter().filter_map(V::num).sum())
            }
        }
        "AVG" => {
            let nums: Vec<f64> = values(rows).iter().filter_map(V::num).collect();
            if nums.is_empty() {
                V::Null
            } else {
                V::Float(nums.iter().sum::<f64>() / nums.len() as f64)
            }
        }
        "MIN" | "MAX" => {
            let vals = values(rows);
            vals.into_iter()
                .reduce(|x, y| {
                    let o = cmp(&x, &y).unwrap_or(Ordering::Equal);
                    if (name == "MIN") == (o == Ordering::Greater) {
                        y
                    } else {
                        x
                    }
                })
                .unwrap_or(V::Null)
        }
        "MEDIAN" | "PERCENTILE" => {
            let mut nums: Vec<f64> = values(rows).iter().filter_map(V::num).collect();
            nums.sort_by(f64::total_cmp);
            let p = if name == "MEDIAN" {
                50.0
            } else {
                match args.get(1) {
                    Some(E::Lit(v)) => v.num().unwrap_or(50.0),
                    _ => {
                        return Err(verify_err(
                            *col,
                            "second argument of [percentile] must be a constant",
                        ));
                    }
                }
            };
            if nums.is_empty() {
                V::Null
            } else {
                let rank = p / 100.0 * (nums.len() - 1) as f64;
                let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
                V::Float(nums[lo] + (nums[hi] - nums[lo]) * (rank - lo as f64))
            }
        }
        "VALUES" => {
            let mut seen = BTreeSet::new();
            let mut out: Vec<V> = values(rows)
                .into_iter()
                .filter(|v| seen.insert(v.key()))
                .collect();
            out.sort_by(|a, b| cmp(a, b).unwrap_or(Ordering::Equal));
            match out.len() {
                0 => V::Null,
                1 => out.pop().expect("one value"),
                _ => V::Multi(out),
            }
        }
        "TOP" => {
            let n = match args.get(1) {
                Some(E::Lit(V::Int(n))) => *n as usize,
                _ => 1,
            };
            let desc =
                !matches!(args.get(2), Some(E::Lit(V::Str(s))) if s.eq_ignore_ascii_case("asc"));
            let mut vals = values(rows);
            vals.sort_by(|a, b| {
                let o = cmp(a, b).unwrap_or(Ordering::Equal);
                if desc { o.reverse() } else { o }
            });
            vals.truncate(n);
            match vals.len() {
                0 => V::Null,
                1 => vals.pop().expect("one value"),
                _ => V::Multi(vals),
            }
        }
        _ => {
            // A scalar wrapping aggregates, e.g. ROUND(AVG(x), 1).
            let inner: Vec<E> = args
                .iter()
                .map(|a| {
                    if contains_agg(a) {
                        aggregate(a, rows, t, env).map(E::Lit)
                    } else {
                        Ok(a.clone())
                    }
                })
                .collect::<Result<_, _>>()?;
            let call_e = E::Call(name.clone(), inner, *col);
            rows.first()
                .map_or_else(|| eval(&call_e, &[], t, env), |r| eval(&call_e, r, t, env))
        }
    })
}

fn contains_agg(e: &E) -> bool {
    match e {
        E::Call(n, args, _) => AGGS.contains(&n.as_str()) || args.iter().any(contains_agg),
        E::Bin(_, a, b) => contains_agg(a) || contains_agg(b),
        E::Not(x) | E::Neg(x) => contains_agg(x),
        _ => false,
    }
}

fn agg_type(e: &E, t: &Table) -> String {
    match e {
        E::Call(n, args, _) if !AGGS.contains(&n.as_str()) && args.iter().any(contains_agg) => {
            let inner = args.iter().find(|a| contains_agg(a)).expect("has agg");
            match n.as_str() {
                "ROUND" | "ABS" | "FLOOR" | "CEIL" => agg_type(inner, t),
                _ => type_of(e, t),
            }
        }
        _ => type_of(e, t),
    }
}

fn sort_rows(t: &mut Table, keys: &[(E, bool, Option<bool>)], env: &Env) {
    let snapshot = t.clone();
    let mut keyed: Vec<(Vec<V>, Vec<V>)> = t
        .rows
        .drain(..)
        .map(|r| {
            (
                keys.iter()
                    .map(|(e, _, _)| eval(e, &r, &snapshot, env))
                    .collect(),
                r,
            )
        })
        .collect();
    keyed.sort_by(|a, b| {
        for (i, (_, desc, nulls_first)) in keys.iter().enumerate() {
            let nulls_first = nulls_first.unwrap_or(*desc);
            let (x, y) = (&a.0[i], &b.0[i]);
            let o = match (x == &V::Null, y == &V::Null) {
                (true, true) => Ordering::Equal,
                (true, false) => {
                    if nulls_first {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (false, true) => {
                    if nulls_first {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                _ => {
                    let o = cmp(x, y).unwrap_or(Ordering::Equal);
                    if *desc { o.reverse() } else { o }
                }
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    t.rows = keyed.into_iter().map(|(_, r)| r).collect();
}

const DEFAULT_LIMIT: usize = 1000;

pub fn run(query: &str, all: &[Index], now_ms: i64) -> Result<Value, EsError> {
    let toks = lex(query)?;
    if toks.is_empty() {
        return Err(parse_err(
            1,
            "mismatched input '<EOF>' expecting {'from', 'row', 'show'}",
        ));
    }
    let mut p = P {
        toks: &toks,
        pos: 0,
        src: query,
    };
    let cmds = p.commands()?;
    let env = Env { now_ms };

    let mut table = Table {
        cols: Vec::new(),
        rows: Vec::new(),
    };
    let mut limited = false;
    for cmd in &cmds {
        match cmd {
            Cmd::From(patterns, metadata, col) => {
                let expr = patterns.join(",");
                let indices = resolve_open(all, &expr).map_err(|e| {
                    if e.kind == "index_closed_exception" {
                        return e;
                    }
                    let missing = patterns
                        .iter()
                        .find(|p| !p.contains('*') && resolve(all, p).is_err())
                        .cloned()
                        .unwrap_or(expr.clone());
                    verify_err(*col, &format!("Unknown index [{missing}]"))
                })?;
                if indices.is_empty() {
                    return Err(verify_err(*col, &format!("Unknown index [{expr}]")));
                }
                let mut fields: Vec<(String, String)> = Vec::new();
                for idx in &indices {
                    for (f, ty) in idx.leaf_fields() {
                        let ty = esql_type(&ty).to_owned();
                        match fields.iter_mut().find(|(n, _)| *n == f) {
                            Some((_, existing)) if *existing != ty => {
                                *existing = "unsupported".into()
                            }
                            Some(_) => {}
                            None => fields.push((f, ty)),
                        }
                    }
                }
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                if *metadata {
                    fields.push(("_id".into(), "keyword".into()));
                    fields.push(("_index".into(), "keyword".into()));
                }
                let rows = indices
                    .iter()
                    .flat_map(|i| i.docs.iter())
                    .map(|d| {
                        fields
                            .iter()
                            .map(|(f, ty)| match f.as_str() {
                                "_id" if *metadata => V::Str(d.id.clone()),
                                "_index" if *metadata => V::Str(d.index.clone()),
                                _ if ty == "unsupported" => V::Null,
                                _ => from_json(&Value::Array(field_values(&d.source, f)), ty),
                            })
                            .collect()
                    })
                    .collect();
                table = Table { cols: fields, rows };
            }
            Cmd::Where(e) => {
                check_fields(e, &table)?;
                if contains_agg(e) {
                    return Err(EsError::verification(&[
                        "aggregate function not allowed in WHERE".into(),
                    ]));
                }
                let t = table.clone();
                table.rows.retain(|r| truthy(&eval(e, r, &t, &env)));
            }
            Cmd::Eval(assigns) => {
                for (name, e) in assigns {
                    check_fields(e, &table)?;
                    let ty = type_of(e, &table);
                    let t = table.clone();
                    let values: Vec<V> = table.rows.iter().map(|r| eval(e, r, &t, &env)).collect();
                    match table.col(name) {
                        Some(i) => {
                            table.cols[i].1 = ty;
                            for (r, v) in table.rows.iter_mut().zip(values) {
                                r[i] = v;
                            }
                        }
                        None => {
                            table.cols.push((name.clone(), ty));
                            for (r, v) in table.rows.iter_mut().zip(values) {
                                r.push(v);
                            }
                        }
                    }
                }
            }
            Cmd::Stats(aggs, by) => {
                for (_, e) in aggs.iter().chain(by.iter()) {
                    check_fields(e, &table)?;
                }
                for (name, e) in aggs {
                    if !contains_agg(e) {
                        return Err(verify_err(
                            1,
                            &format!("expected an aggregate function but found [{name}]"),
                        ));
                    }
                }
                let mut groups: Vec<(Vec<V>, Vec<&Vec<V>>)> = Vec::new();
                for r in &table.rows {
                    let key: Vec<V> = by.iter().map(|(_, e)| eval(e, r, &table, &env)).collect();
                    match groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, members)) => members.push(r),
                        None => groups.push((key, vec![r])),
                    }
                }
                if by.is_empty() && groups.is_empty() {
                    groups.push((Vec::new(), Vec::new()));
                }
                let mut cols: Vec<(String, String)> = aggs
                    .iter()
                    .map(|(n, e)| (n.clone(), agg_type(e, &table)))
                    .collect();
                cols.extend(by.iter().map(|(n, e)| {
                    let ty = type_of(e, &table);
                    (n.clone(), if ty == "text" { "keyword".into() } else { ty })
                }));
                let mut rows = Vec::new();
                for (key, members) in &groups {
                    let mut row = Vec::new();
                    for (_, e) in aggs {
                        row.push(aggregate(e, members, &table, &env)?);
                    }
                    row.extend(key.iter().cloned());
                    rows.push(row);
                }
                table = Table { cols, rows };
            }
            Cmd::Sort(keys) => {
                for (e, _, _) in keys {
                    check_fields(e, &table)?;
                }
                sort_rows(&mut table, keys, &env);
            }
            Cmd::Limit(n) => {
                limited = true;
                table.rows.truncate(*n);
            }
            Cmd::Keep(patterns) => {
                let mut keep: Vec<usize> = Vec::new();
                for p in patterns {
                    let matched: Vec<usize> = (0..table.cols.len())
                        .filter(|i| wildcard_match(p, &table.cols[*i].0))
                        .collect();
                    if matched.is_empty() && !p.contains('*') {
                        return Err(verify_err(1, &format!("Unknown column [{p}]")));
                    }
                    for i in matched {
                        keep.retain(|k| *k != i);
                        keep.push(i);
                    }
                }
                table = project(&table, &keep);
            }
            Cmd::Drop(patterns) => {
                for p in patterns {
                    if !p.contains('*') && table.col(p).is_none() {
                        return Err(verify_err(1, &format!("Unknown column [{p}]")));
                    }
                }
                let keep: Vec<usize> = (0..table.cols.len())
                    .filter(|i| {
                        !patterns
                            .iter()
                            .any(|p| wildcard_match(p, &table.cols[*i].0))
                    })
                    .collect();
                table = project(&table, &keep);
            }
            Cmd::Rename(pairs) => {
                for (from, to) in pairs {
                    let i = table
                        .col(from)
                        .ok_or_else(|| verify_err(1, &format!("Unknown column [{from}]")))?;
                    table.cols[i].0 = to.clone();
                }
            }
        }
    }
    if !limited {
        table.rows.truncate(DEFAULT_LIMIT);
    }
    let docs = all.iter().map(|i| i.docs.len()).sum::<usize>();
    Ok(json!({
        "took": 3 + docs / 50,
        "is_partial": false,
        "columns": table.cols.iter().map(|(n, t)| json!({ "name": n, "type": t })).collect::<Vec<_>>(),
        "values": table.rows.iter().map(|r| r.iter().map(V::to_json).collect::<Vec<_>>()).collect::<Vec<_>>(),
    }))
}

fn project(t: &Table, keep: &[usize]) -> Table {
    Table {
        cols: keep.iter().map(|i| t.cols[*i].clone()).collect(),
        rows: t
            .rows
            .iter()
            .map(|r| keep.iter().map(|i| r[*i].clone()).collect())
            .collect(),
    }
}
