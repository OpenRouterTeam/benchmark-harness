//! SOQL: lexer, parser and evaluator over the store.
//!
//! Supported: field lists with parent relationship paths (`Account.Owner.Name`), child
//! subqueries (`(SELECT Id FROM Contacts)`), `FIELDS(ALL|STANDARD|CUSTOM)`, `WHERE` with
//! `AND`/`OR`/`NOT`/parentheses, `= != <> < <= > >= LIKE IN NOT IN INCLUDES EXCLUDES`,
//! semi-joins (`Id IN (SELECT AccountId FROM Opportunity)`), date and date-time literals
//! and relative date literals (`TODAY`, `THIS_QUARTER`, `LAST_N_DAYS:30`, ...), aggregate
//! functions (`COUNT() COUNT COUNT_DISTINCT SUM AVG MIN MAX`) with `GROUP BY`/`HAVING`,
//! date functions (`CALENDAR_YEAR` ...), `ORDER BY ... ASC|DESC NULLS FIRST|LAST`, `LIMIT`
//! and `OFFSET`. Errors use Salesforce's messages and caret layout.

use std::cmp::Ordering;

use serde_json::{Map, Value, json};

use crate::{
    db::{Db, SfError},
    schema::{Field, Object, Ty, fd},
    util::{
        DAY_MS, civil_from_days, days_from_civil, format_date, normalize_id, parse_date,
        parse_datetime,
    },
};

// ---------------------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum T {
    Ident(String),
    Str(String),
    Num(String),
    Date(String),
    DateTime(String),
    Op(String),
    LParen,
    RParen,
    Comma,
    Colon,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Tok {
    pub t: T,
    pub pos: usize,
}

impl Tok {
    fn text(&self) -> String {
        match &self.t {
            T::Eof => "<EOF>".to_owned(),
            T::Str(s) => format!("'{s}'"),
            T::Ident(s) | T::Num(s) | T::Date(s) | T::DateTime(s) | T::Op(s) => s.clone(),
            T::LParen => "(".to_owned(),
            T::RParen => ")".to_owned(),
            T::Comma => ",".to_owned(),
            T::Colon => ":".to_owned(),
        }
    }
}

/// Error text in Salesforce's layout: the query line, a caret, the position, the message.
pub fn positioned(src: &str, pos: usize, message: &str) -> String {
    let pos = pos.min(src.len());
    let before = &src[..pos];
    let row = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line_end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
    let col = before[line_start..].chars().count() + 1;
    format!(
        "{}\n{}^\nERROR at Row:{row}:Column:{col}\n{message}",
        &src[line_start..line_end],
        " ".repeat(col - 1)
    )
}

pub fn lex(src: &str) -> Result<Vec<Tok>, SfError> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| chars.get(i).map(|c| c.1);
    let pos_of = |i: usize| chars.get(i).map_or(src.len(), |c| c.0);
    while i < chars.len() {
        let (pos, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let single = match c {
            '(' => Some(T::LParen),
            ')' => Some(T::RParen),
            ',' => Some(T::Comma),
            ':' => Some(T::Colon),
            _ => None,
        };
        if let Some(t) = single {
            out.push(Tok { t, pos });
            i += 1;
            continue;
        }
        if c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match at(i) {
                    None => {
                        return Err(malformed(
                            src,
                            pos,
                            &format!(
                                "line 1:{} mismatched character '<EOF>' expecting '''",
                                src.len()
                            ),
                        ));
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        let e = at(i + 1);
                        match e {
                            Some('\'') => s.push('\''),
                            Some('"') => s.push('"'),
                            Some('\\') => s.push('\\'),
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('r') => s.push('\r'),
                            Some('b') => s.push('\u{8}'),
                            Some('f') => s.push('\u{c}'),
                            Some('_') => s.push_str("\\_"),
                            Some('%') => s.push_str("\\%"),
                            _ => {
                                return Err(malformed(
                                    src,
                                    pos_of(i),
                                    "Invalid string literal: Illegal character sequence '\\' in string literal.",
                                ));
                            }
                        }
                        i += 2;
                    }
                    Some(ch) => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            out.push(Tok { t: T::Str(s), pos });
            continue;
        }
        if c.is_ascii_digit()
            || ((c == '-' || c == '+' || c == '.') && at(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            // Date / datetime literal?
            let rest = &src[pos..];
            if let Some(len) = date_literal_len(rest) {
                let text = &rest[..len];
                let t = if len > 10 {
                    T::DateTime(text.to_owned())
                } else {
                    T::Date(text.to_owned())
                };
                out.push(Tok { t, pos });
                i += text.chars().count();
                continue;
            }
            let mut j = i + 1;
            while at(j).is_some_and(|d| d.is_ascii_digit() || d == '.') {
                j += 1;
            }
            out.push(Tok {
                t: T::Num(src[pos..pos_of(j)].to_owned()),
                pos,
            });
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i + 1;
            while at(j).is_some_and(|d| d.is_alphanumeric() || d == '_' || d == '.') {
                j += 1;
            }
            out.push(Tok {
                t: T::Ident(src[pos..pos_of(j)].to_owned()),
                pos,
            });
            i = j;
            continue;
        }
        let two: String = chars[i..chars.len().min(i + 2)]
            .iter()
            .map(|c| c.1)
            .collect();
        if ["!=", "<>", "<=", ">="].contains(&two.as_str()) {
            out.push(Tok { t: T::Op(two), pos });
            i += 2;
            continue;
        }
        if matches!(c, '=' | '<' | '>') {
            out.push(Tok {
                t: T::Op(c.to_string()),
                pos,
            });
            i += 1;
            continue;
        }
        return Err(malformed(
            src,
            pos,
            &format!("line 1:{} no viable alternative at character '{c}'", pos),
        ));
    }
    out.push(Tok {
        t: T::Eof,
        pos: src.len(),
    });
    Ok(out)
}

/// Length of a `YYYY-MM-DD` or `YYYY-MM-DDThh:mm:ss(.sss)(Z|±hh:mm)` literal at the start.
fn date_literal_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let is_d = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    if !(0..4).all(is_d)
        || b.get(4) != Some(&b'-')
        || !is_d(5)
        || !is_d(6)
        || b.get(7) != Some(&b'-')
        || !is_d(8)
        || !is_d(9)
    {
        return None;
    }
    if b.get(10) != Some(&b'T') {
        return Some(10);
    }
    let mut i = 11;
    for (k, want) in [(2, b':'), (2, b':'), (2, 0)] {
        if !(i..i + k).all(is_d) {
            return None;
        }
        i += k;
        if want != 0 {
            if b.get(i) != Some(&want) {
                return None;
            }
            i += 1;
        }
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        while is_d(i) {
            i += 1;
        }
    }
    match b.get(i) {
        Some(b'Z') => Some(i + 1),
        Some(b'+' | b'-') => {
            let mut j = i + 1;
            while is_d(j) || b.get(j) == Some(&b':') {
                j += 1;
            }
            Some(j)
        }
        _ => Some(i),
    }
}

pub fn malformed(src: &str, pos: usize, message: &str) -> SfError {
    SfError::bad("MALFORMED_QUERY", positioned(src, pos, message))
}

// ---------------------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Path(Vec<String>, usize),
    Func(String, Vec<Expr>, usize),
}

impl Expr {
    fn pos(&self) -> usize {
        match self {
            Expr::Path(_, p) | Expr::Func(_, _, p) => *p,
        }
    }

    fn text(&self) -> String {
        match self {
            Expr::Path(p, _) => p.join("."),
            Expr::Func(n, a, _) => format!(
                "{n}({})",
                a.iter().map(Expr::text).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SelKind {
    Expr(Expr),
    Sub(Box<Query>),
    Fields(String),
}

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub kind: SelKind,
    pub alias: Option<String>,
    pub pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Date(String),
    DateTime(String),
    Relative(String, i64),
}

#[derive(Debug, Clone)]
pub enum Cond {
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
    Cmp {
        lhs: Expr,
        op: String,
        rhs: Lit,
        pos: usize,
    },
    In {
        lhs: Expr,
        neg: bool,
        list: Vec<(Lit, usize)>,
        sub: Option<Box<Query>>,
    },
    Like {
        lhs: Expr,
        pattern: String,
        pos: usize,
    },
    Includes {
        lhs: Expr,
        neg: bool,
        list: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct OrderItem {
    pub expr: Expr,
    pub desc: bool,
    pub nulls_last: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub select: Vec<SelectItem>,
    pub from: String,
    pub from_pos: usize,
    pub alias: Option<String>,
    pub cond: Option<Cond>,
    pub group: Vec<Expr>,
    pub having: Option<Cond>,
    pub order: Vec<OrderItem>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub limit_pos: usize,
}

const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "LIKE", "GROUP", "BY", "HAVING", "ORDER",
    "LIMIT", "OFFSET", "NULL", "TRUE", "FALSE", "ASC", "DESC", "NULLS", "FIRST", "LAST", "WITH",
    "FOR", "INCLUDES", "EXCLUDES", "USING", "SCOPE", "TYPEOF", "UPDATE", "ROLLUP", "CUBE", "END",
    "WHEN", "THEN", "ELSE",
];

const AGGREGATES: &[&str] = &["COUNT", "COUNT_DISTINCT", "SUM", "AVG", "MIN", "MAX"];
const DATE_FUNCTIONS: &[&str] = &[
    "CALENDAR_YEAR",
    "CALENDAR_MONTH",
    "CALENDAR_QUARTER",
    "FISCAL_YEAR",
    "FISCAL_MONTH",
    "FISCAL_QUARTER",
    "DAY_IN_MONTH",
    "DAY_IN_WEEK",
    "DAY_IN_YEAR",
    "DAY_ONLY",
    "HOUR_IN_DAY",
    "WEEK_IN_MONTH",
    "WEEK_IN_YEAR",
];
const PASSTHROUGH: &[&str] = &["TOLABEL", "FORMAT", "CONVERTCURRENCY"];

const RELATIVE: &[&str] = &[
    "YESTERDAY",
    "TODAY",
    "TOMORROW",
    "LAST_WEEK",
    "THIS_WEEK",
    "NEXT_WEEK",
    "LAST_MONTH",
    "THIS_MONTH",
    "NEXT_MONTH",
    "LAST_90_DAYS",
    "NEXT_90_DAYS",
    "THIS_QUARTER",
    "LAST_QUARTER",
    "NEXT_QUARTER",
    "THIS_YEAR",
    "LAST_YEAR",
    "NEXT_YEAR",
    "THIS_FISCAL_QUARTER",
    "LAST_FISCAL_QUARTER",
    "NEXT_FISCAL_QUARTER",
    "THIS_FISCAL_YEAR",
    "LAST_FISCAL_YEAR",
    "NEXT_FISCAL_YEAR",
];
const RELATIVE_N: &[&str] = &[
    "LAST_N_DAYS",
    "NEXT_N_DAYS",
    "N_DAYS_AGO",
    "LAST_N_WEEKS",
    "NEXT_N_WEEKS",
    "N_WEEKS_AGO",
    "LAST_N_MONTHS",
    "NEXT_N_MONTHS",
    "N_MONTHS_AGO",
    "LAST_N_QUARTERS",
    "NEXT_N_QUARTERS",
    "N_QUARTERS_AGO",
    "LAST_N_YEARS",
    "NEXT_N_YEARS",
    "N_YEARS_AGO",
    "LAST_N_FISCAL_QUARTERS",
    "NEXT_N_FISCAL_QUARTERS",
    "N_FISCAL_QUARTERS_AGO",
    "LAST_N_FISCAL_YEARS",
    "NEXT_N_FISCAL_YEARS",
    "N_FISCAL_YEARS_AGO",
];

fn is_kw(s: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(s))
}

// ---------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<Tok>,
    i: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Result<Self, SfError> {
        Ok(Self {
            src,
            toks: lex(src)?,
            i: 0,
        })
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.i.min(self.toks.len() - 1)]
    }

    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)]
    }

    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        self.i += 1;
        t
    }

    fn unexpected(&self) -> SfError {
        let t = self.peek();
        malformed(
            self.src,
            t.pos,
            &format!("unexpected token: '{}'", t.text()),
        )
    }

    fn is(&self, kw: &str) -> bool {
        matches!(&self.peek().t, T::Ident(s) if s.eq_ignore_ascii_case(kw))
    }

    fn eat(&mut self, kw: &str) -> bool {
        if self.is(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kw: &str) -> Result<(), SfError> {
        if self.eat(kw) {
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn expect_t(&mut self, t: &T) -> Result<(), SfError> {
        if &self.peek().t == t {
            self.bump();
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    pub fn parse(mut self) -> Result<Query, SfError> {
        let q = self.query()?;
        if self.peek().t != T::Eof {
            return Err(self.unexpected());
        }
        Ok(q)
    }

    fn query(&mut self) -> Result<Query, SfError> {
        self.expect("SELECT")?;
        let mut select = Vec::new();
        loop {
            select.push(self.select_item()?);
            if self.peek().t == T::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.expect("FROM")?;
        let from_tok = self.bump();
        let T::Ident(from) = from_tok.t.clone() else {
            self.i -= 1;
            return Err(self.unexpected());
        };
        let mut q = Query {
            select,
            from,
            from_pos: from_tok.pos,
            alias: None,
            cond: None,
            group: Vec::new(),
            having: None,
            order: Vec::new(),
            limit: None,
            offset: None,
            limit_pos: 0,
        };
        if let T::Ident(a) = &self.peek().t
            && !is_kw(a)
        {
            q.alias = Some(a.clone());
            self.bump();
        }
        if self.eat("USING") {
            self.expect("SCOPE")?;
            self.bump();
        }
        if self.eat("WHERE") {
            q.cond = Some(self.cond()?);
        }
        while self.is("WITH") {
            self.bump();
            while !matches!(self.peek().t, T::Eof | T::RParen)
                && !["GROUP", "ORDER", "LIMIT", "OFFSET", "FOR", "UPDATE"]
                    .iter()
                    .any(|k| self.is(k))
            {
                self.bump();
            }
        }
        if self.eat("GROUP") {
            self.expect("BY")?;
            if self.is("ROLLUP") || self.is("CUBE") {
                return Err(self.unexpected());
            }
            loop {
                q.group.push(self.expr()?);
                if self.peek().t == T::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if self.eat("HAVING") {
            q.having = Some(self.cond()?);
        }
        if self.eat("ORDER") {
            self.expect("BY")?;
            loop {
                let expr = self.expr()?;
                let desc = if self.eat("DESC") {
                    true
                } else {
                    self.eat("ASC");
                    false
                };
                let mut nulls_last = None;
                if self.eat("NULLS") {
                    if self.eat("LAST") {
                        nulls_last = Some(true);
                    } else {
                        self.expect("FIRST")?;
                        nulls_last = Some(false);
                    }
                }
                q.order.push(OrderItem {
                    expr,
                    desc,
                    nulls_last,
                });
                if self.peek().t == T::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if self.is("LIMIT") {
            q.limit_pos = self.peek().pos;
            self.bump();
            q.limit = Some(self.integer()?);
        }
        if self.eat("OFFSET") {
            q.offset = Some(self.integer()?);
        }
        if self.eat("FOR") {
            if !(self.eat("VIEW") || self.eat("REFERENCE") || self.eat("UPDATE")) {
                return Err(self.unexpected());
            }
        } else if self.eat("UPDATE") && !(self.eat("TRACKING") || self.eat("VIEWSTAT")) {
            return Err(self.unexpected());
        }
        Ok(q)
    }

    fn integer(&mut self) -> Result<i64, SfError> {
        match &self.peek().t {
            T::Num(n) if n.parse::<i64>().is_ok() => {
                let v = n.parse().expect("checked");
                self.bump();
                Ok(v)
            }
            _ => Err(self.unexpected()),
        }
    }

    fn select_item(&mut self) -> Result<SelectItem, SfError> {
        let pos = self.peek().pos;
        if self.peek().t == T::LParen {
            self.bump();
            let sub = self.query()?;
            self.expect_t(&T::RParen)?;
            return Ok(SelectItem {
                kind: SelKind::Sub(Box::new(sub)),
                alias: None,
                pos,
            });
        }
        if let T::Ident(name) = &self.peek().t
            && name.eq_ignore_ascii_case("FIELDS")
            && self.peek_at(1).t == T::LParen
        {
            self.bump();
            self.bump();
            let T::Ident(kind) = self.bump().t else {
                self.i -= 1;
                return Err(self.unexpected());
            };
            self.expect_t(&T::RParen)?;
            return Ok(SelectItem {
                kind: SelKind::Fields(kind.to_ascii_uppercase()),
                alias: None,
                pos,
            });
        }
        if matches!(&self.peek().t, T::Ident(s) if is_kw(s)) {
            return Err(self.unexpected());
        }
        let expr = self.expr()?;
        let mut alias = None;
        if let T::Ident(a) = &self.peek().t
            && !is_kw(a)
        {
            if matches!(expr, Expr::Path(..)) {
                return Err(self.unexpected());
            }
            alias = Some(a.clone());
            self.bump();
        }
        Ok(SelectItem {
            kind: SelKind::Expr(expr),
            alias,
            pos,
        })
    }

    fn expr(&mut self) -> Result<Expr, SfError> {
        let tok = self.bump();
        let T::Ident(name) = tok.t else {
            self.i -= 1;
            return Err(self.unexpected());
        };
        if self.peek().t == T::LParen {
            self.bump();
            let mut args = Vec::new();
            if self.peek().t != T::RParen {
                args.push(self.expr()?);
            }
            self.expect_t(&T::RParen)?;
            return Ok(Expr::Func(name.to_ascii_uppercase(), args, tok.pos));
        }
        if is_kw(&name) {
            self.i -= 1;
            return Err(self.unexpected());
        }
        Ok(Expr::Path(
            name.split('.').map(str::to_owned).collect(),
            tok.pos,
        ))
    }

    fn cond(&mut self) -> Result<Cond, SfError> {
        let mut parts = vec![self.unary()?];
        let mut joiner: Option<&'static str> = None;
        loop {
            let this = if self.is("AND") {
                "AND"
            } else if self.is("OR") {
                "OR"
            } else {
                break;
            };
            if joiner.is_some_and(|j| j != this) {
                return Err(self.unexpected());
            }
            joiner = Some(this);
            self.bump();
            parts.push(self.unary()?);
        }
        Ok(match joiner {
            None => parts.pop().expect("one part"),
            Some("AND") => Cond::And(parts),
            Some(_) => Cond::Or(parts),
        })
    }

    fn unary(&mut self) -> Result<Cond, SfError> {
        if self.eat("NOT") {
            return Ok(Cond::Not(Box::new(self.unary()?)));
        }
        if self.peek().t == T::LParen {
            self.bump();
            let c = self.cond()?;
            self.expect_t(&T::RParen)?;
            return Ok(c);
        }
        self.simple()
    }

    fn simple(&mut self) -> Result<Cond, SfError> {
        let lhs = self.expr()?;
        if self.eat("NOT") {
            if !self.is("IN") {
                return Err(self.unexpected());
            }
            self.bump();
            return self.in_list(lhs, true);
        }
        if self.eat("IN") {
            return self.in_list(lhs, false);
        }
        if self.eat("LIKE") {
            let pos = self.peek().pos;
            let T::Str(pattern) = self.bump().t else {
                self.i -= 1;
                return Err(self.unexpected());
            };
            return Ok(Cond::Like { lhs, pattern, pos });
        }
        let neg = if self.eat("INCLUDES") {
            Some(false)
        } else if self.eat("EXCLUDES") {
            Some(true)
        } else {
            None
        };
        if let Some(neg) = neg {
            self.expect_t(&T::LParen)?;
            let mut list = Vec::new();
            loop {
                let T::Str(s) = self.bump().t else {
                    self.i -= 1;
                    return Err(self.unexpected());
                };
                list.push(s);
                if self.peek().t == T::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect_t(&T::RParen)?;
            return Ok(Cond::Includes { lhs, neg, list });
        }
        let T::Op(op) = self.peek().t.clone() else {
            return Err(self.unexpected());
        };
        self.bump();
        let pos = self.peek().pos;
        let rhs = self.literal()?;
        Ok(Cond::Cmp { lhs, op, rhs, pos })
    }

    fn in_list(&mut self, lhs: Expr, neg: bool) -> Result<Cond, SfError> {
        self.expect_t(&T::LParen)?;
        if self.is("SELECT") {
            let sub = self.query()?;
            self.expect_t(&T::RParen)?;
            return Ok(Cond::In {
                lhs,
                neg,
                list: Vec::new(),
                sub: Some(Box::new(sub)),
            });
        }
        let mut list = Vec::new();
        loop {
            let pos = self.peek().pos;
            list.push((self.literal()?, pos));
            if self.peek().t == T::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.expect_t(&T::RParen)?;
        Ok(Cond::In {
            lhs,
            neg,
            list,
            sub: None,
        })
    }

    fn literal(&mut self) -> Result<Lit, SfError> {
        let tok = self.peek().clone();
        let lit = match &tok.t {
            T::Str(s) => Lit::Str(s.clone()),
            T::Num(n) => Lit::Num(n.parse().map_err(|_| self.unexpected())?),
            T::Date(d) => Lit::Date(d.clone()),
            T::DateTime(d) => Lit::DateTime(d.clone()),
            T::Ident(s) => {
                let up = s.to_ascii_uppercase();
                match up.as_str() {
                    "NULL" => Lit::Null,
                    "TRUE" => Lit::Bool(true),
                    "FALSE" => Lit::Bool(false),
                    _ if RELATIVE.contains(&up.as_str()) => Lit::Relative(up, 0),
                    _ if RELATIVE_N.contains(&up.as_str()) => {
                        self.bump();
                        self.expect_t(&T::Colon)?;
                        let n = self.integer()?;
                        return Ok(Lit::Relative(up, n));
                    }
                    _ => return Err(self.unexpected()),
                }
            }
            _ => return Err(self.unexpected()),
        };
        self.bump();
        Ok(lit)
    }
}

pub fn parse(src: &str) -> Result<Query, SfError> {
    Parser::new(src)?.parse()
}

// ---------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------

/// A comparable value derived from a field value and its type.
#[derive(Debug, Clone, PartialEq)]
pub enum Cv {
    Null,
    B(bool),
    N(f64),
    S(String),
    Id(String),
    D(i64),
    T(i64),
}

impl Cv {
    pub fn of(v: &Value, ty: Ty) -> Cv {
        match v {
            Value::Null => Cv::Null,
            Value::Bool(b) => Cv::B(*b),
            Value::Number(n) => Cv::N(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => match ty {
                Ty::Date => {
                    parse_date(s.get(..10).unwrap_or(s)).map_or(Cv::S(s.to_lowercase()), Cv::D)
                }
                Ty::DateTime => parse_datetime(s).map_or(Cv::S(s.to_lowercase()), Cv::T),
                Ty::Id | Ty::Reference => Cv::Id(s.chars().take(15).collect()),
                t if t.is_numeric() => s.parse().map_or(Cv::S(s.to_lowercase()), Cv::N),
                Ty::Boolean => Cv::B(s.eq_ignore_ascii_case("true")),
                _ => Cv::S(s.to_lowercase()),
            },
            other => Cv::S(other.to_string()),
        }
    }

    fn cmp(&self, other: &Cv) -> Option<Ordering> {
        match (self, other) {
            (Cv::B(a), Cv::B(b)) => Some(a.cmp(b)),
            (Cv::N(a), Cv::N(b)) => a.partial_cmp(b),
            (Cv::S(a), Cv::S(b)) | (Cv::Id(a), Cv::Id(b)) => Some(a.cmp(b)),
            (Cv::D(a), Cv::D(b)) | (Cv::T(a), Cv::T(b)) => Some(a.cmp(b)),
            (Cv::D(a), Cv::T(b)) => Some((a * DAY_MS).cmp(b)),
            (Cv::T(a), Cv::D(b)) => Some(a.cmp(&(b * DAY_MS))),
            _ => None,
        }
    }

    /// Ordering for ORDER BY; nulls handled by the caller.
    fn sort_cmp(&self, other: &Cv) -> Ordering {
        self.cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// `[start, end)` in days for a relative date literal.
pub fn relative_range(name: &str, n: i64, today: i64) -> (i64, i64) {
    let (y, m, _) = civil_from_days(today);
    let month_start = |k: i64| {
        let idx = y * 12 + i64::from(m) - 1 + k;
        days_from_civil(idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32, 1)
    };
    let quarter_start = |k: i64| {
        let idx = y * 4 + (i64::from(m) - 1) / 3 + k;
        days_from_civil(idx.div_euclid(4), (idx.rem_euclid(4) * 3 + 1) as u32, 1)
    };
    let year_start = |k: i64| days_from_civil(y + k, 1, 1);
    let dow = (today + 4).rem_euclid(7);
    let week_start = |k: i64| today - dow + 7 * k;
    let name = name.replace("_FISCAL", "");
    match name.as_str() {
        "YESTERDAY" => (today - 1, today),
        "TODAY" => (today, today + 1),
        "TOMORROW" => (today + 1, today + 2),
        "LAST_WEEK" => (week_start(-1), week_start(0)),
        "THIS_WEEK" => (week_start(0), week_start(1)),
        "NEXT_WEEK" => (week_start(1), week_start(2)),
        "LAST_MONTH" => (month_start(-1), month_start(0)),
        "THIS_MONTH" => (month_start(0), month_start(1)),
        "NEXT_MONTH" => (month_start(1), month_start(2)),
        "LAST_90_DAYS" => (today - 90, today + 1),
        "NEXT_90_DAYS" => (today + 1, today + 91),
        "THIS_QUARTER" => (quarter_start(0), quarter_start(1)),
        "LAST_QUARTER" => (quarter_start(-1), quarter_start(0)),
        "NEXT_QUARTER" => (quarter_start(1), quarter_start(2)),
        "THIS_YEAR" => (year_start(0), year_start(1)),
        "LAST_YEAR" => (year_start(-1), year_start(0)),
        "NEXT_YEAR" => (year_start(1), year_start(2)),
        "LAST_N_DAYS" => (today - n, today + 1),
        "NEXT_N_DAYS" => (today + 1, today + 1 + n),
        "N_DAYS_AGO" => (today - n, today - n + 1),
        "LAST_N_WEEKS" => (week_start(-n), week_start(0)),
        "NEXT_N_WEEKS" => (week_start(1), week_start(1 + n)),
        "N_WEEKS_AGO" => (week_start(-n), week_start(-n + 1)),
        "LAST_N_MONTHS" => (month_start(-n), month_start(0)),
        "NEXT_N_MONTHS" => (month_start(1), month_start(1 + n)),
        "N_MONTHS_AGO" => (month_start(-n), month_start(-n + 1)),
        "LAST_N_QUARTERS" => (quarter_start(-n), quarter_start(0)),
        "NEXT_N_QUARTERS" => (quarter_start(1), quarter_start(1 + n)),
        "N_QUARTERS_AGO" => (quarter_start(-n), quarter_start(-n + 1)),
        "LAST_N_YEARS" => (year_start(-n), year_start(0)),
        "NEXT_N_YEARS" => (year_start(1), year_start(1 + n)),
        "N_YEARS_AGO" => (year_start(-n), year_start(-n + 1)),
        _ => (today, today + 1),
    }
}

// ---------------------------------------------------------------------------------------
// Compiled query
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Step {
    field: String,
}

/// A resolved field path from the query's object.
#[derive(Debug, Clone)]
pub struct Resolved {
    steps: Vec<Step>,
    rels: Vec<String>,
    pub field: Field,
    synthetic_type: bool,
}

impl Resolved {
    fn key(&self) -> String {
        let mut parts = self.rels.clone();
        parts.push(self.field.name.clone());
        parts.join(".").to_lowercase()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agg {
    Count,
    CountDistinct,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone)]
enum CExpr {
    Field(Resolved),
    DateFn(String, Resolved),
    Agg(Agg, Option<Resolved>, String),
}

impl CExpr {
    fn ty(&self) -> Ty {
        match self {
            CExpr::Field(r) => r.field.ty,
            CExpr::DateFn(f, _) if f == "DAY_ONLY" => Ty::Date,
            CExpr::DateFn(..) => Ty::Int,
            CExpr::Agg(Agg::Count | Agg::CountDistinct, ..) => Ty::Int,
            CExpr::Agg(Agg::Sum | Agg::Avg, ..) => Ty::Double,
            CExpr::Agg(_, Some(r), _) => r.field.ty,
            CExpr::Agg(_, None, _) => Ty::Int,
        }
    }

    fn key(&self) -> String {
        match self {
            CExpr::Field(r) => r.key(),
            CExpr::DateFn(f, r) => format!("{f}({})", r.key()),
            CExpr::Agg(_, _, text) => text.to_lowercase(),
        }
    }
}

#[derive(Debug, Clone)]
enum Rhs {
    V(Cv),
    Range(Cv, Cv),
}

#[derive(Debug, Clone)]
enum CCond {
    And(Vec<CCond>),
    Or(Vec<CCond>),
    Not(Box<CCond>),
    Cmp(CExpr, String, Rhs),
    In(CExpr, bool, Vec<Cv>),
    Like(CExpr, Vec<LikeTok>),
    Includes(CExpr, bool, Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum LikeTok {
    Ch(char),
    Any,
    One,
}

fn like_compile(p: &str) -> Vec<LikeTok> {
    let mut out = Vec::new();
    let mut it = p.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                if let Some(n) = it.next() {
                    out.push(LikeTok::Ch(n.to_ascii_lowercase()));
                }
            }
            '%' => out.push(LikeTok::Any),
            '_' => out.push(LikeTok::One),
            c => out.extend(c.to_lowercase().map(LikeTok::Ch)),
        }
    }
    out
}

fn like_match(p: &[LikeTok], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(LikeTok::Any) => (0..=s.len()).any(|i| like_match(&p[1..], &s[i..])),
        Some(LikeTok::One) => !s.is_empty() && like_match(&p[1..], &s[1..]),
        Some(LikeTok::Ch(c)) => s.first() == Some(c) && like_match(&p[1..], &s[1..]),
    }
}

#[derive(Debug, Clone)]
enum CSel {
    Field(Resolved),
    Expr(CExpr, Option<String>),
    Sub(Box<Compiled>, String, String),
}

#[derive(Debug, Clone)]
pub struct Compiled {
    object: Object,
    select: Vec<CSel>,
    cond: Option<CCond>,
    group: Vec<CExpr>,
    having: Option<CCond>,
    order: Vec<(CExpr, bool, bool)>,
    limit: Option<usize>,
    offset: usize,
    aggregate: bool,
    count_only: bool,
}

/// The result rows of a query before paging into batches.
pub struct Output {
    pub total: usize,
    pub records: Vec<Value>,
}

pub struct Engine<'a, 'b> {
    pub db: &'b Db<'a>,
    pub src: &'b str,
    /// `queryAll`: include deleted rows.
    pub all_rows: bool,
    /// Only these Ids of this object are visible (SOSL matches).
    pub restrict: Option<(&'b str, &'b [String])>,
}

impl<'a, 'b> Engine<'a, 'b> {
    fn err_at(&self, pos: usize, code: &str, message: &str) -> SfError {
        SfError::bad(code, positioned(self.src, pos, message))
    }

    fn invalid_field(&self, pos: usize, message: &str) -> SfError {
        self.err_at(pos, "INVALID_FIELD", message)
    }

    pub fn object(&self, name: &str, pos: usize) -> Result<Object, SfError> {
        self.db
            .schema
            .get_in(name, self.db.tooling)
            .cloned()
            .ok_or_else(|| {
                self.err_at(
                    pos,
                    "INVALID_TYPE",
                    &format!(
                        "sObject type '{name}' is not supported. If you are attempting to use a custom object, be sure to append the '__c' after the entity name. Please reference your WSDL or the describe call for the appropriate names."
                    ),
                )
            })
    }

    fn resolve(
        &self,
        obj: &Object,
        path: &[String],
        alias: Option<&str>,
        pos: usize,
    ) -> Result<Resolved, SfError> {
        let mut path = path;
        if path.len() > 1
            && (alias.is_some_and(|a| a.eq_ignore_ascii_case(&path[0]))
                || (path[0].eq_ignore_ascii_case(&obj.name)
                    && obj.relationship(&path[0]).is_none()))
        {
            path = &path[1..];
        }
        let mut current = obj.clone();
        let mut steps = Vec::new();
        let mut rels = Vec::new();
        let mut targets: Vec<String> = vec![obj.name.clone()];
        for (i, seg) in path.iter().enumerate() {
            let last = i == path.len() - 1;
            if last {
                if targets.len() > 1 {
                    if seg.eq_ignore_ascii_case("Type") {
                        return Ok(Resolved {
                            steps,
                            rels,
                            field: fd("Type", "Object Type", Ty::Picklist),
                            synthetic_type: true,
                        });
                    }
                    for t in &targets {
                        if let Some(o) = self.db.schema.get(t)
                            && let Some(f) = o.field(seg)
                        {
                            return Ok(Resolved {
                                steps,
                                rels,
                                field: f.clone(),
                                synthetic_type: false,
                            });
                        }
                    }
                    return Err(self.invalid_field(pos, &no_such_column(seg, &targets[0])));
                }
                let Some(f) = current.field(seg) else {
                    return Err(self.invalid_field(pos, &no_such_column(seg, &current.name)));
                };
                return Ok(Resolved {
                    steps,
                    rels,
                    field: f.clone(),
                    synthetic_type: false,
                });
            }
            let rel_field = if targets.len() > 1 {
                targets
                    .iter()
                    .filter_map(|t| self.db.schema.get(t))
                    .find_map(|o| o.relationship(seg).cloned())
            } else {
                current.relationship(seg).cloned()
            };
            let Some(rf) = rel_field else {
                return Err(self.invalid_field(
                    pos,
                    &format!(
                        "Didn't understand relationship '{seg}' in field path. If you are attempting to use a custom relationship, be sure to append the '__r' after the custom relationship name. Please reference your WSDL or the describe call for the appropriate names."
                    ),
                ));
            };
            steps.push(Step {
                field: rf.name.clone(),
            });
            rels.push(rf.rel.clone().unwrap_or_default());
            targets = rf.refs.clone();
            current = self
                .db
                .schema
                .get(&targets[0])
                .cloned()
                .unwrap_or_else(|| current.clone());
        }
        Err(self.invalid_field(pos, "unexpected empty field path"))
    }

    fn expr(
        &self,
        obj: &Object,
        e: &Expr,
        alias: Option<&str>,
        allow_agg: bool,
    ) -> Result<CExpr, SfError> {
        match e {
            Expr::Path(p, pos) => Ok(CExpr::Field(self.resolve(obj, p, alias, *pos)?)),
            Expr::Func(name, args, pos) => {
                if PASSTHROUGH.contains(&name.as_str()) {
                    let Some(a) = args.first() else {
                        return Err(self.err_at(*pos, "MALFORMED_QUERY", "unexpected token: ')'"));
                    };
                    return self.expr(obj, a, alias, allow_agg);
                }
                if DATE_FUNCTIONS.contains(&name.as_str()) {
                    let Some(Expr::Path(p, ppos)) = args.first() else {
                        return Err(self.err_at(
                            *pos,
                            "MALFORMED_QUERY",
                            &format!("unexpected token: '{name}'"),
                        ));
                    };
                    let r = self.resolve(obj, p, alias, *ppos)?;
                    if !matches!(r.field.ty, Ty::Date | Ty::DateTime) {
                        return Err(self.err_at(
                            *ppos,
                            "INVALID_FIELD",
                            &format!(
                                "{name} can only be applied to date or dateTime fields: {}",
                                r.field.name
                            ),
                        ));
                    }
                    return Ok(CExpr::DateFn(name.clone(), r));
                }
                if AGGREGATES.contains(&name.as_str()) {
                    if !allow_agg {
                        return Err(self.err_at(
                            *pos,
                            "MALFORMED_QUERY",
                            "Aggregate functions are not supported in this context",
                        ));
                    }
                    let agg = match name.as_str() {
                        "COUNT" => Agg::Count,
                        "COUNT_DISTINCT" => Agg::CountDistinct,
                        "SUM" => Agg::Sum,
                        "AVG" => Agg::Avg,
                        "MIN" => Agg::Min,
                        _ => Agg::Max,
                    };
                    let arg = match args.first() {
                        None if agg == Agg::Count => None,
                        None => {
                            return Err(self.err_at(
                                *pos,
                                "MALFORMED_QUERY",
                                "unexpected token: ')'",
                            ));
                        }
                        Some(Expr::Path(p, ppos)) => {
                            let r = self.resolve(obj, p, alias, *ppos)?;
                            let ok = match agg {
                                Agg::Sum | Agg::Avg => r.field.ty.is_numeric(),
                                Agg::Min | Agg::Max => {
                                    r.field.ty.sortable() && r.field.ty != Ty::Boolean
                                }
                                _ => true,
                            };
                            if !ok {
                                return Err(self.err_at(
                                    *ppos,
                                    "INVALID_FIELD",
                                    &format!(
                                        "field {} does not support aggregate operator {name}",
                                        r.field.name
                                    ),
                                ));
                            }
                            Some(r)
                        }
                        Some(other) => {
                            return Err(self.err_at(
                                other.pos(),
                                "MALFORMED_QUERY",
                                &format!("unexpected token: '{}'", other.text()),
                            ));
                        }
                    };
                    return Ok(CExpr::Agg(agg, arg, e.text()));
                }
                Err(self.err_at(
                    *pos,
                    "MALFORMED_QUERY",
                    &format!("Unknown function: {name}"),
                ))
            }
        }
    }

    fn rhs(&self, lit: &Lit, ex: &CExpr, pos: usize) -> Result<Rhs, SfError> {
        let ty = ex.ty();
        let name = match ex {
            CExpr::Field(r) | CExpr::DateFn(_, r) => r.field.name.clone(),
            CExpr::Agg(_, _, t) => t.clone(),
        };
        let type_err = || {
            let tname = ty.soql_name();
            let tail = if matches!(tname, "string" | "id") {
                "should be enclosed in quotes"
            } else {
                "should not be enclosed in quotes"
            };
            self.invalid_field(
                pos,
                &format!("value of filter criterion for field '{name}' must be of type {tname} and {tail}"),
            )
        };
        let today = self.db.now_ms.div_euclid(DAY_MS);
        Ok(Rhs::V(match (lit, ty) {
            (Lit::Null, _) => Cv::Null,
            (Lit::Relative(n, k), Ty::Date) => {
                let (a, b) = relative_range(n, *k, today);
                return Ok(Rhs::Range(Cv::D(a), Cv::D(b)));
            }
            (Lit::Relative(n, k), Ty::DateTime) => {
                let (a, b) = relative_range(n, *k, today);
                return Ok(Rhs::Range(Cv::T(a * DAY_MS), Cv::T(b * DAY_MS)));
            }
            (Lit::Date(d), Ty::Date) => Cv::D(parse_date(d).ok_or_else(type_err)?),
            (Lit::DateTime(d), Ty::DateTime) => Cv::T(parse_datetime(d).ok_or_else(type_err)?),
            (Lit::Num(n), t) if t.is_numeric() => Cv::N(*n),
            (Lit::Bool(b), Ty::Boolean) => Cv::B(*b),
            (Lit::Str(s), Ty::Id | Ty::Reference) => match normalize_id(s) {
                Some(id) => Cv::Id(id[..15].to_owned()),
                None => {
                    return Err(self.err_at(
                        pos,
                        "INVALID_QUERY_FILTER_OPERATOR",
                        &format!("invalid ID field: {s}"),
                    ));
                }
            },
            (Lit::Str(s), t)
                if !t.is_numeric() && !matches!(t, Ty::Boolean | Ty::Date | Ty::DateTime) =>
            {
                Cv::S(s.to_lowercase())
            }
            (Lit::Str(_), _) => return Err(type_err()),
            _ => return Err(type_err()),
        }))
    }

    fn cond(
        &self,
        obj: &Object,
        c: &Cond,
        alias: Option<&str>,
        allow_agg: bool,
    ) -> Result<CCond, SfError> {
        Ok(match c {
            Cond::And(v) => CCond::And(
                v.iter()
                    .map(|c| self.cond(obj, c, alias, allow_agg))
                    .collect::<Result<_, _>>()?,
            ),
            Cond::Or(v) => CCond::Or(
                v.iter()
                    .map(|c| self.cond(obj, c, alias, allow_agg))
                    .collect::<Result<_, _>>()?,
            ),
            Cond::Not(c) => CCond::Not(Box::new(self.cond(obj, c, alias, allow_agg)?)),
            Cond::Cmp { lhs, op, rhs, pos } => {
                let ex = self.expr(obj, lhs, alias, allow_agg)?;
                self.check_filterable(&ex, lhs.pos())?;
                let r = self.rhs(rhs, &ex, *pos)?;
                if matches!(r, Rhs::V(Cv::Null)) && !matches!(op.as_str(), "=" | "!=" | "<>") {
                    return Err(self.err_at(*pos, "MALFORMED_QUERY", "unexpected token: 'null'"));
                }
                CCond::Cmp(ex, op.clone(), r)
            }
            Cond::Like { lhs, pattern, pos } => {
                let ex = self.expr(obj, lhs, alias, allow_agg)?;
                self.check_filterable(&ex, lhs.pos())?;
                let ty = ex.ty();
                if ty.is_numeric() || matches!(ty, Ty::Boolean | Ty::Date | Ty::DateTime) {
                    return Err(self.invalid_field(
                        *pos,
                        &format!("invalid operator on {} field: LIKE", ty.api_name()),
                    ));
                }
                CCond::Like(ex, like_compile(pattern))
            }
            Cond::In {
                lhs,
                neg,
                list,
                sub,
            } => {
                let ex = self.expr(obj, lhs, alias, allow_agg)?;
                self.check_filterable(&ex, lhs.pos())?;
                let ty = ex.ty();
                let values = if let Some(sub) = sub {
                    let compiled = self.compile(sub, None)?;
                    if compiled.select.len() != 1 || compiled.aggregate {
                        return Err(self.err_at(sub.from_pos, "MALFORMED_QUERY", "The selected field in a semi join subquery must be an Id or reference field"));
                    }
                    let CSel::Field(r) = &compiled.select[0] else {
                        return Err(self.err_at(sub.from_pos, "MALFORMED_QUERY", "The selected field in a semi join subquery must be an Id or reference field"));
                    };
                    let r = r.clone();
                    self.rows(&compiled)?
                        .iter()
                        .map(|(o, rec)| Cv::of(&self.value(rec, o, &r), ty))
                        .collect()
                } else {
                    list.iter()
                        .map(|(lit, pos)| match self.rhs(lit, &ex, *pos)? {
                            Rhs::V(v) => Ok(v),
                            Rhs::Range(..) => Err(self.err_at(
                                *pos,
                                "MALFORMED_QUERY",
                                "unexpected token: date literal in IN list",
                            )),
                        })
                        .collect::<Result<_, _>>()?
                };
                CCond::In(ex, *neg, values)
            }
            Cond::Includes { lhs, neg, list } => {
                let ex = self.expr(obj, lhs, alias, allow_agg)?;
                if ex.ty() != Ty::MultiPicklist {
                    return Err(self.invalid_field(
                        lhs.pos(),
                        "INCLUDES/EXCLUDES operator only valid on multipicklist field",
                    ));
                }
                CCond::Includes(ex, *neg, list.iter().map(|s| s.to_lowercase()).collect())
            }
        })
    }

    fn check_filterable(&self, ex: &CExpr, pos: usize) -> Result<(), SfError> {
        if let CExpr::Field(r) = ex
            && !r.field.ty.filterable()
        {
            return Err(self.invalid_field(
                pos,
                &format!(
                    "field '{}' can not be filtered in a query call",
                    r.field.name
                ),
            ));
        }
        Ok(())
    }

    /// Compiles a query (or a child subquery when `parent` is given).
    pub fn compile(&self, q: &Query, parent: Option<&Object>) -> Result<Compiled, SfError> {
        let object = match parent {
            None => self.object(&q.from, q.from_pos)?,
            Some(p) => {
                let Some(rel) = p.child(&q.from) else {
                    return Err(self.err_at(
                        q.from_pos,
                        "INVALID_TYPE",
                        &format!(
                            "Didn't understand relationship '{}' in FROM part of query call. If you are attempting to use a custom relationship, be sure to append the '__r' after the custom relationship name. Please reference your WSDL or the describe call for the appropriate names.",
                            q.from
                        ),
                    ));
                };
                self.object(&rel.child, q.from_pos)?
            }
        };
        let alias = q.alias.as_deref();
        let aggregate = !q.group.is_empty()
            || q.select.iter().any(|s| matches!(&s.kind, SelKind::Expr(Expr::Func(n, _, _)) if AGGREGATES.contains(&n.as_str())));
        let mut select = Vec::new();
        let mut count_only = false;
        for item in &q.select {
            match &item.kind {
                SelKind::Fields(kind) => {
                    if aggregate {
                        return Err(self.err_at(
                            item.pos,
                            "MALFORMED_QUERY",
                            "FIELDS() can not be used with aggregate queries",
                        ));
                    }
                    if kind != "STANDARD" && q.limit.is_none_or(|l| l > 200) {
                        return Err(self.err_at(
                            item.pos,
                            "MALFORMED_QUERY",
                            "The SOQL FIELDS function must have a LIMIT of at most 200",
                        ));
                    }
                    for f in &object.fields {
                        let take = match kind.as_str() {
                            "ALL" => true,
                            "STANDARD" => !f.custom,
                            "CUSTOM" => f.custom,
                            _ => {
                                return Err(self.err_at(
                                    item.pos,
                                    "MALFORMED_QUERY",
                                    &format!("unexpected token: '{kind}'"),
                                ));
                            }
                        };
                        if take {
                            select.push(CSel::Field(self.resolve(
                                &object,
                                std::slice::from_ref(&f.name),
                                None,
                                item.pos,
                            )?));
                        }
                    }
                }
                SelKind::Sub(sub) => {
                    if parent.is_some() {
                        return Err(self.err_at(
                            item.pos,
                            "MALFORMED_QUERY",
                            "Nesting of semi join sub-selects is not supported",
                        ));
                    }
                    let compiled = self.compile(sub, Some(&object))?;
                    let rel = object.child(&sub.from).expect("compiled above");
                    select.push(CSel::Sub(
                        Box::new(compiled),
                        rel.name.clone().unwrap_or_default(),
                        rel.field.clone(),
                    ));
                }
                SelKind::Expr(e) => {
                    let ex = self.expr(&object, e, alias, true)?;
                    if let (CExpr::Agg(Agg::Count, None, _), true) =
                        (&ex, q.select.len() == 1 && q.group.is_empty())
                    {
                        count_only = true;
                    }
                    match ex {
                        CExpr::Field(r) if !aggregate => select.push(CSel::Field(r)),
                        CExpr::DateFn(..) if !aggregate => {
                            return Err(self.err_at(
                                item.pos,
                                "MALFORMED_QUERY",
                                &format!("Field must be grouped or aggregated: {}", e.text()),
                            ));
                        }
                        other => select.push(CSel::Expr(other, item.alias.clone())),
                    }
                }
            }
        }
        let cond = q
            .cond
            .as_ref()
            .map(|c| self.cond(&object, c, alias, false))
            .transpose()?;
        let group = q
            .group
            .iter()
            .map(|g| {
                let ex = self.expr(&object, g, alias, false)?;
                if let CExpr::Field(r) = &ex
                    && !r.field.ty.groupable()
                {
                    return Err(self.invalid_field(
                        g.pos(),
                        &format!(
                            "field '{}' can not be grouped in a query call",
                            r.field.name
                        ),
                    ));
                }
                Ok(ex)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if aggregate {
            let keys: Vec<String> = group.iter().map(CExpr::key).collect();
            for (item, sel) in q.select.iter().zip(&select) {
                if let CSel::Expr(ex @ (CExpr::Field(_) | CExpr::DateFn(..)), _) = sel
                    && !keys.contains(&ex.key())
                {
                    let text = match &item.kind {
                        SelKind::Expr(e) => e.text(),
                        _ => String::new(),
                    };
                    return Err(self.err_at(
                        item.pos,
                        "MALFORMED_QUERY",
                        &format!("Field must be grouped or aggregated: {text}"),
                    ));
                }
            }
        }
        let having = match &q.having {
            Some(_) if !aggregate => {
                return Err(self.err_at(
                    q.from_pos,
                    "MALFORMED_QUERY",
                    "HAVING clause requires GROUP BY or aggregate functions",
                ));
            }
            Some(h) => Some(self.cond(&object, h, alias, true)?),
            None => None,
        };
        let mut order = Vec::new();
        for o in &q.order {
            let ex = match &o.expr {
                Expr::Path(p, _) if aggregate && p.len() == 1 => {
                    let alias_hit = select.iter().find_map(|sel| match sel {
                        CSel::Expr(ex, Some(a)) if a.eq_ignore_ascii_case(&p[0]) => {
                            Some(ex.clone())
                        }
                        _ => None,
                    });
                    match alias_hit {
                        Some(ex) => ex,
                        None => self.expr(&object, &o.expr, alias, true)?,
                    }
                }
                e => self.expr(&object, e, alias, aggregate)?,
            };
            if let CExpr::Field(r) = &ex
                && !r.field.ty.sortable()
            {
                return Err(self.invalid_field(
                    o.expr.pos(),
                    &format!("field '{}' can not be sorted in a query call", r.field.name),
                ));
            }
            if aggregate
                && !matches!(ex, CExpr::Agg(..))
                && !group.iter().any(|g| g.key() == ex.key())
            {
                return Err(self.err_at(
                    o.expr.pos(),
                    "MALFORMED_QUERY",
                    &format!(
                        "Ordered field must be grouped or aggregated: {}",
                        o.expr.text()
                    ),
                ));
            }
            order.push((ex, o.desc, o.nulls_last.unwrap_or(o.desc)));
        }
        let limit = match q.limit {
            Some(l) if !(0..=50_000).contains(&l) && parent.is_none() => {
                return Err(self.err_at(
                    q.limit_pos,
                    "NUMBER_OUTSIDE_VALID_RANGE",
                    &format!("The limit must be between 0 and 50000: {l}"),
                ));
            }
            Some(l) => Some(l.max(0) as usize),
            None => None,
        };
        let offset = q.offset.unwrap_or(0);
        if offset > 2000 {
            return Err(self.err_at(
                q.limit_pos,
                "NUMBER_OUTSIDE_VALID_RANGE",
                &format!("Maximum SOQL offset allowed is 2000: {offset}"),
            ));
        }
        if aggregate && q.offset.is_some() {
            return Err(self.err_at(
                q.from_pos,
                "MALFORMED_QUERY",
                "OFFSET is not supported with aggregate queries",
            ));
        }
        Ok(Compiled {
            object,
            select,
            cond,
            group,
            having,
            order,
            limit,
            offset: offset.max(0) as usize,
            aggregate,
            count_only,
        })
    }

    // ---------------------------------------------------------------------------------
    // Evaluation
    // ---------------------------------------------------------------------------------

    /// The value of a resolved path on a record of `object`.
    pub fn value(&self, rec: &Value, object: &str, r: &Resolved) -> Value {
        let mut cur = rec;
        let mut cur_obj = object.to_owned();
        for step in &r.steps {
            let Some(id) = cur.get(&step.field).and_then(Value::as_str) else {
                return Value::Null;
            };
            let Some((o, next)) = self.db.find(id) else {
                return Value::Null;
            };
            cur = next;
            cur_obj = o.name.clone();
        }
        if r.synthetic_type {
            return Value::String(cur_obj);
        }
        field_value(cur, &cur_obj, &r.field.name)
    }

    fn row_value(&self, ex: &CExpr, obj: &str, rec: &Value) -> Value {
        match ex {
            CExpr::Field(r) => self.value(rec, obj, r),
            CExpr::DateFn(f, r) => date_fn(f, &self.value(rec, obj, r)),
            CExpr::Agg(..) => Value::Null,
        }
    }

    fn group_value(&self, ex: &CExpr, obj: &str, rows: &[&Value]) -> Value {
        match ex {
            CExpr::Agg(agg, arg, _) => {
                let values: Vec<Value> = match arg {
                    None => return json!(rows.len()),
                    Some(r) => rows
                        .iter()
                        .map(|rec| self.value(rec, obj, r))
                        .filter(|v| !v.is_null())
                        .collect(),
                };
                let ty = arg.as_ref().map_or(Ty::Int, |r| r.field.ty);
                match agg {
                    Agg::Count => json!(values.len()),
                    Agg::CountDistinct => {
                        let mut seen: Vec<String> = values
                            .iter()
                            .map(|v| match Cv::of(v, ty) {
                                Cv::S(s) | Cv::Id(s) => s,
                                other => format!("{other:?}"),
                            })
                            .collect();
                        seen.sort();
                        seen.dedup();
                        json!(seen.len())
                    }
                    Agg::Sum | Agg::Avg => {
                        if values.is_empty() {
                            return Value::Null;
                        }
                        let nums: Vec<f64> = values
                            .iter()
                            .filter_map(|v| {
                                v.as_f64()
                                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                            })
                            .collect();
                        let sum: f64 = nums.iter().sum();
                        if *agg == Agg::Avg {
                            json!(sum / nums.len().max(1) as f64)
                        } else if ty == Ty::Int && values.iter().all(Value::is_i64) {
                            json!(values.iter().filter_map(Value::as_i64).sum::<i64>())
                        } else {
                            json!(sum)
                        }
                    }
                    Agg::Min | Agg::Max => {
                        let mut best: Option<&Value> = None;
                        for v in &values {
                            best = match best {
                                None => Some(v),
                                Some(b) => {
                                    let ord = Cv::of(v, ty).sort_cmp(&Cv::of(b, ty));
                                    let better = if *agg == Agg::Min {
                                        ord == Ordering::Less
                                    } else {
                                        ord == Ordering::Greater
                                    };
                                    Some(if better { v } else { b })
                                }
                            };
                        }
                        best.cloned().unwrap_or(Value::Null)
                    }
                }
            }
            other => rows
                .first()
                .map_or(Value::Null, |rec| self.row_value(other, obj, rec)),
        }
    }

    fn test(&self, c: &CCond, get: &dyn Fn(&CExpr) -> Value) -> bool {
        match c {
            CCond::And(v) => v.iter().all(|c| self.test(c, get)),
            CCond::Or(v) => v.iter().any(|c| self.test(c, get)),
            CCond::Not(c) => !self.test(c, get),
            CCond::Cmp(ex, op, rhs) => {
                let v = Cv::of(&get(ex), ex.ty());
                match rhs {
                    Rhs::V(Cv::Null) => match op.as_str() {
                        "=" => v == Cv::Null,
                        _ => v != Cv::Null,
                    },
                    Rhs::V(r) => {
                        if v == Cv::Null {
                            return matches!(op.as_str(), "!=" | "<>");
                        }
                        let ord = v.cmp(r);
                        match op.as_str() {
                            "=" => ord == Some(Ordering::Equal),
                            "!=" | "<>" => ord != Some(Ordering::Equal),
                            "<" => ord == Some(Ordering::Less),
                            "<=" => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
                            ">" => ord == Some(Ordering::Greater),
                            ">=" => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
                            _ => false,
                        }
                    }
                    Rhs::Range(lo, hi) => {
                        if v == Cv::Null {
                            return matches!(op.as_str(), "!=" | "<>");
                        }
                        let ge_lo = matches!(v.cmp(lo), Some(Ordering::Greater | Ordering::Equal));
                        let lt_hi = v.cmp(hi) == Some(Ordering::Less);
                        match op.as_str() {
                            "=" => ge_lo && lt_hi,
                            "!=" | "<>" => !(ge_lo && lt_hi),
                            "<" => !ge_lo,
                            "<=" => lt_hi,
                            ">" => !lt_hi,
                            ">=" => ge_lo,
                            _ => false,
                        }
                    }
                }
            }
            CCond::In(ex, neg, set) => {
                let v = Cv::of(&get(ex), ex.ty());
                let hit = v != Cv::Null && set.iter().any(|s| v.cmp(s) == Some(Ordering::Equal));
                if *neg { !hit } else { hit }
            }
            CCond::Like(ex, pat) => match get(ex) {
                Value::String(s) => {
                    let chars: Vec<char> = s.to_lowercase().chars().collect();
                    like_match(pat, &chars)
                }
                Value::Null => false,
                other => like_match(pat, &other.to_string().chars().collect::<Vec<_>>()),
            },
            CCond::Includes(ex, neg, list) => {
                let v = get(ex);
                let values: Vec<String> = v
                    .as_str()
                    .unwrap_or("")
                    .split(';')
                    .map(|s| s.trim().to_lowercase())
                    .collect();
                let hit = list.iter().any(|l| {
                    l.split(';')
                        .all(|part| values.contains(&part.trim().to_owned()))
                });
                if *neg { !hit } else { hit }
            }
        }
    }

    fn visible(&self, obj: &str, rec: &Value) -> bool {
        match self.restrict {
            Some((o, ids)) if o == obj => rec["Id"]
                .as_str()
                .is_some_and(|id| ids.iter().any(|i| i == id)),
            _ => true,
        }
    }

    /// Filtered and ordered rows (before OFFSET/LIMIT) of a non-aggregate query.
    fn rows(&self, q: &Compiled) -> Result<Vec<(String, &'a Value)>, SfError> {
        let obj = q.object.name.clone();
        let mut rows: Vec<&'a Value> = self
            .db
            .rows(&obj)
            .into_iter()
            .filter(|r| self.all_rows || r.get("IsDeleted") != Some(&Value::Bool(true)))
            .filter(|r| self.visible(&obj, r))
            .filter(|r| {
                q.cond
                    .as_ref()
                    .is_none_or(|c| self.test(c, &|ex| self.row_value(ex, &obj, r)))
            })
            .collect();
        if !q.order.is_empty() {
            rows.sort_by(|a, b| {
                for (ex, desc, nulls_last) in &q.order {
                    let ty = ex.ty();
                    let va = Cv::of(&self.row_value(ex, &obj, a), ty);
                    let vb = Cv::of(&self.row_value(ex, &obj, b), ty);
                    let ord = order_cmp(&va, &vb, *desc, *nulls_last);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                Ordering::Equal
            });
        }
        Ok(rows.into_iter().map(|r| (obj.clone(), r)).collect())
    }

    fn project(&self, q: &Compiled, obj: &str, rec: &Value) -> Result<Value, SfError> {
        let mut out = Map::new();
        out.insert("attributes".into(), self.db.attributes(obj, rec));
        for sel in &q.select {
            match sel {
                CSel::Field(r) => self.insert_path(&mut out, obj, rec, r, 0),
                CSel::Sub(child, rel_name, link) => {
                    let parent_id: String =
                        rec["Id"].as_str().unwrap_or("").chars().take(15).collect();
                    let mut kids = self.rows(child)?;
                    kids.retain(|(_, k)| {
                        k.get(link)
                            .and_then(Value::as_str)
                            .is_some_and(|v| v.starts_with(&parent_id) && !parent_id.is_empty())
                    });
                    let kids: Vec<Value> = kids
                        .into_iter()
                        .skip(child.offset)
                        .take(child.limit.unwrap_or(usize::MAX))
                        .map(|(o, k)| self.project(child, &o, k))
                        .collect::<Result<_, _>>()?;
                    let value = if kids.is_empty() {
                        Value::Null
                    } else {
                        json!({ "totalSize": kids.len(), "done": true, "records": kids })
                    };
                    out.insert(rel_name.clone(), value);
                }
                CSel::Expr(..) => {}
            }
        }
        Ok(Value::Object(out))
    }

    fn insert_path(
        &self,
        out: &mut Map<String, Value>,
        obj: &str,
        rec: &Value,
        r: &Resolved,
        depth: usize,
    ) {
        if depth == r.steps.len() {
            let name = if r.synthetic_type {
                "Type".to_owned()
            } else {
                r.field.name.clone()
            };
            let v = if r.synthetic_type {
                Value::String(obj.to_owned())
            } else {
                field_value(rec, obj, &r.field.name)
            };
            out.insert(name, v);
            return;
        }
        let rel = r.rels[depth].clone();
        if let Some(Value::Null) = out.get(&rel) {
            return;
        }
        let target = rec
            .get(&r.steps[depth].field)
            .and_then(Value::as_str)
            .and_then(|id| self.db.find(id));
        let Some((o, next)) = target else {
            out.insert(rel, Value::Null);
            return;
        };
        let entry = out
            .entry(rel)
            .or_insert_with(|| json!({ "attributes": self.db.attributes(&o.name, next) }));
        if let Value::Object(m) = entry {
            self.insert_path(m, &o.name, next, r, depth + 1);
        }
    }

    /// Runs a compiled top-level query.
    pub fn run(&self, q: &Compiled) -> Result<Output, SfError> {
        if !q.aggregate {
            let rows = self.rows(q)?;
            let rows: Vec<_> = rows
                .into_iter()
                .skip(q.offset)
                .take(q.limit.unwrap_or(usize::MAX))
                .collect();
            let total = rows.len();
            let records = rows
                .into_iter()
                .map(|(o, r)| self.project(q, &o, r))
                .collect::<Result<_, _>>()?;
            return Ok(Output { total, records });
        }
        let obj = q.object.name.clone();
        let rows: Vec<&Value> = self
            .db
            .rows(&obj)
            .into_iter()
            .filter(|r| self.all_rows || r.get("IsDeleted") != Some(&Value::Bool(true)))
            .filter(|r| {
                q.cond
                    .as_ref()
                    .is_none_or(|c| self.test(c, &|ex| self.row_value(ex, &obj, r)))
            })
            .collect();
        if q.count_only {
            let n = rows
                .len()
                .saturating_sub(q.offset)
                .min(q.limit.unwrap_or(usize::MAX));
            return Ok(Output {
                total: n,
                records: Vec::new(),
            });
        }
        let mut groups: Vec<(Vec<Cv>, Vec<&Value>)> = Vec::new();
        for r in rows {
            let key: Vec<Cv> = q
                .group
                .iter()
                .map(|g| Cv::of(&self.row_value(g, &obj, r), g.ty()))
                .collect();
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push(r),
                None => groups.push((key, vec![r])),
            }
        }
        if q.group.is_empty() && groups.is_empty() {
            groups.push((Vec::new(), Vec::new()));
        }
        let mut groups: Vec<Vec<&Value>> = groups
            .into_iter()
            .map(|(_, m)| m)
            .filter(|m| {
                q.having
                    .as_ref()
                    .is_none_or(|h| self.test(h, &|ex| self.group_value(ex, &obj, m)))
            })
            .collect();
        if !q.order.is_empty() {
            groups.sort_by(|a, b| {
                for (ex, desc, nulls_last) in &q.order {
                    let ty = ex.ty();
                    let va = Cv::of(&self.group_value(ex, &obj, a), ty);
                    let vb = Cv::of(&self.group_value(ex, &obj, b), ty);
                    let ord = order_cmp(&va, &vb, *desc, *nulls_last);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                Ordering::Equal
            });
        }
        let groups: Vec<_> = groups
            .into_iter()
            .take(q.limit.unwrap_or(usize::MAX))
            .collect();
        let mut records = Vec::new();
        for members in &groups {
            let mut out = Map::new();
            out.insert("attributes".into(), json!({ "type": "AggregateResult" }));
            let mut expr_n = 0;
            for sel in &q.select {
                let CSel::Expr(ex, alias) = sel else { continue };
                let key = match (alias, ex) {
                    (Some(a), _) => a.clone(),
                    (None, CExpr::Field(r)) => r.field.name.clone(),
                    (None, _) => {
                        let k = format!("expr{expr_n}");
                        expr_n += 1;
                        k
                    }
                };
                out.insert(key, self.group_value(ex, &obj, members));
            }
            records.push(Value::Object(out));
        }
        Ok(Output {
            total: records.len(),
            records,
        })
    }
}

fn order_cmp(a: &Cv, b: &Cv, desc: bool, nulls_last: bool) -> Ordering {
    match (a == &Cv::Null, b == &Cv::Null) {
        (true, true) => Ordering::Equal,
        (true, false) => {
            if nulls_last {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (false, true) => {
            if nulls_last {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        _ => {
            let o = a.sort_cmp(b);
            if desc { o.reverse() } else { o }
        }
    }
}

fn no_such_column(field: &str, object: &str) -> String {
    format!(
        "No such column '{field}' on entity '{object}'. If you are attempting to use a custom field, be sure to append the '__c' after the custom field name. Please reference your WSDL or the describe call for the appropriate names."
    )
}

/// A stored field value, computing `Name` for person objects when the dataset omits it.
pub fn field_value(rec: &Value, object: &str, field: &str) -> Value {
    match rec.get(field) {
        Some(v) => v.clone(),
        None if field == "Name" && matches!(object, "Contact" | "Lead" | "User") => {
            Value::String(person_name(rec))
        }
        None => Value::Null,
    }
}

pub fn person_name(rec: &Value) -> String {
    [rec.get("FirstName"), rec.get("LastName")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn date_fn(f: &str, v: &Value) -> Value {
    let Some(s) = v.as_str() else {
        return Value::Null;
    };
    let (days, ms) = match parse_datetime(s) {
        Some(ms) => (ms.div_euclid(DAY_MS), Some(ms)),
        None => match parse_date(s.get(..10).unwrap_or(s)) {
            Some(d) => (d, None),
            None => return Value::Null,
        },
    };
    let (y, m, d) = civil_from_days(days);
    let jan1 = days_from_civil(y, 1, 1);
    match f {
        "CALENDAR_YEAR" | "FISCAL_YEAR" => json!(y),
        "CALENDAR_MONTH" | "FISCAL_MONTH" => json!(m),
        "CALENDAR_QUARTER" | "FISCAL_QUARTER" => json!((m - 1) / 3 + 1),
        "DAY_IN_MONTH" => json!(d),
        "DAY_IN_WEEK" => json!((days + 4).rem_euclid(7) + 1),
        "DAY_IN_YEAR" => json!(days - jan1 + 1),
        "DAY_ONLY" => json!(format_date(days)),
        "HOUR_IN_DAY" => json!(ms.map_or(0, |ms| ms.rem_euclid(DAY_MS) / 3_600_000)),
        "WEEK_IN_MONTH" => json!((d - 1) / 7 + 1),
        "WEEK_IN_YEAR" => json!((days - jan1) / 7 + 1),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_where_with_mixed_joiners_as_error() {
        assert!(parse("SELECT Id FROM Account WHERE A = 1 AND B = 2 OR C = 3").is_err());
        assert!(parse("SELECT Id FROM Account WHERE (A = 1 AND B = 2) OR C = 3").is_ok());
    }

    #[test]
    fn parses_aggregates_and_literals() {
        let q = parse("SELECT StageName, SUM(Amount) total FROM Opportunity WHERE CloseDate = THIS_QUARTER AND CreatedDate > 2026-01-01T00:00:00Z GROUP BY StageName HAVING SUM(Amount) > 100 ORDER BY SUM(Amount) DESC NULLS LAST LIMIT 5").unwrap();
        assert_eq!(q.group.len(), 1);
        assert_eq!(q.select[1].alias.as_deref(), Some("total"));
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn error_layout_matches_salesforce() {
        let e = parse("SELECT Id FROM Account WHERE").unwrap_err();
        assert_eq!(e.code, "MALFORMED_QUERY");
        assert_eq!(
            e.message,
            "SELECT Id FROM Account WHERE\n                            ^\nERROR at Row:1:Column:29\nunexpected token: '<EOF>'"
        );
    }

    #[test]
    fn relative_ranges() {
        let today = days_from_civil(2026, 5, 12);
        assert_eq!(
            relative_range("THIS_QUARTER", 0, today),
            (days_from_civil(2026, 4, 1), days_from_civil(2026, 7, 1))
        );
        assert_eq!(
            relative_range("LAST_MONTH", 0, today),
            (days_from_civil(2026, 4, 1), days_from_civil(2026, 5, 1))
        );
        assert_eq!(
            relative_range("THIS_WEEK", 0, today),
            (days_from_civil(2026, 5, 10), days_from_civil(2026, 5, 17))
        );
    }

    #[test]
    fn like_patterns() {
        let p = like_compile("acme%");
        assert!(like_match(&p, &"acme corp".chars().collect::<Vec<_>>()));
        assert!(!like_match(&p, &"the acme".chars().collect::<Vec<_>>()));
        let p = like_compile("%a_e%");
        assert!(like_match(&p, &"blade".chars().collect::<Vec<_>>()));
    }
}
