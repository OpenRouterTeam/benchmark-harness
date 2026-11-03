//! A small PromQL evaluator over static series: selectors with matchers, range selectors,
//! the common rate / *_over_time / math functions, aggregations with `by`/`without`,
//! `topk`/`bottomk`/`quantile`, `histogram_quantile`, and arithmetic / comparison operators.
//!
//! Each dataset series has a current `value` and, for counters, a per-second `rate`. Values do
//! not vary with time, so instant and range queries agree.

use std::collections::BTreeMap;

use regex::Regex;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct Series {
    pub labels: Labels,
    pub value: f64,
    pub rate: Option<f64>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Scalar(f64),
    Vector(Vec<(Labels, f64)>),
    Range(Vec<Series>, f64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Dur(f64),
    Op(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
}

fn parse_duration(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut num = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut any = false;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            i += 1;
            continue;
        }
        let unit: String = if c == 'm' && chars.get(i + 1) == Some(&'s') {
            i += 2;
            "ms".into()
        } else {
            i += 1;
            c.to_string()
        };
        let n: f64 = num.parse().ok()?;
        num.clear();
        total += n * match unit.as_str() {
            "ms" => 0.001,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            "d" => 86400.0,
            "w" => 604_800.0,
            "y" => 31_536_000.0,
            _ => return None,
        };
        any = true;
    }
    if !num.is_empty() {
        return None;
    }
    any.then_some(total)
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_bracket = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1
            }
            '{' => {
                out.push(Tok::LBrace);
                i += 1
            }
            '}' => {
                out.push(Tok::RBrace);
                i += 1
            }
            '[' => {
                out.push(Tok::LBracket);
                in_bracket = true;
                i += 1
            }
            ']' => {
                out.push(Tok::RBracket);
                in_bracket = false;
                i += 1
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1
            }
            '"' | '\'' | '`' => {
                let q = c;
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' && q != '`' && i + 1 < chars.len() {
                        i += 1;
                        match chars[i] {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            other => {
                                if other != q && other != '\\' {
                                    s.push('\\');
                                }
                                s.push(other)
                            }
                        }
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated quoted string".into());
                }
                i += 1;
                out.push(Tok::Str(s));
            }
            _ if in_bracket && (c.is_ascii_digit()) => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                // `[5m:1m]` subquery steps are ignored.
                out.push(Tok::Dur(
                    parse_duration(&s).ok_or_else(|| format!("bad duration {s:?}"))?,
                ));
                if i < chars.len() && chars[i] == ':' {
                    while i < chars.len() && chars[i] != ']' {
                        i += 1;
                    }
                }
            }
            _ if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric()
                        || chars[i] == '.'
                        || ((chars[i] == '+' || chars[i] == '-')
                            && matches!(chars[i - 1], 'e' | 'E')))
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                if let Ok(n) = s.parse::<f64>() {
                    out.push(Tok::Num(n));
                } else if let Some(d) = parse_duration(&s) {
                    out.push(Tok::Dur(d));
                } else {
                    return Err(format!("bad number or duration syntax: \"{s}\""));
                }
            }
            _ if c.is_alphabetic() || c == '_' || c == ':' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric()
                        || chars[i] == '_'
                        || chars[i] == ':'
                        || chars[i] == '.')
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                match s.to_ascii_lowercase().as_str() {
                    "and" | "or" | "unless" => out.push(Tok::Op(s.to_ascii_lowercase())),
                    "inf" => out.push(Tok::Num(f64::INFINITY)),
                    "nan" => out.push(Tok::Num(f64::NAN)),
                    _ => out.push(Tok::Ident(s)),
                }
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if ["==", "!=", ">=", "<=", "=~", "!~"].contains(&two.as_str()) {
                    out.push(Tok::Op(two));
                    i += 2;
                } else if "+-*/%^<>=".contains(c) {
                    out.push(Tok::Op(c.to_string()));
                    i += 1;
                } else {
                    return Err(format!("unexpected character: '{c}'"));
                }
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    Str(String),
    Selector {
        matchers: Vec<Matcher>,
        range: Option<f64>,
    },
    Call(String, Vec<Expr>),
    Agg {
        op: String,
        by: Option<(bool, Vec<String>)>,
        param: Option<Box<Expr>>,
        expr: Box<Expr>,
    },
    Binary {
        op: String,
        bool_mod: bool,
        on: Option<(bool, Vec<String>)>,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Neg(Box<Expr>),
    Paren(Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct Matcher {
    pub name: String,
    pub op: String,
    pub value: String,
}

impl Matcher {
    pub fn matches(&self, labels: &Labels) -> bool {
        let v = labels.get(&self.name).map(String::as_str).unwrap_or("");
        match self.op.as_str() {
            "=" => v == self.value,
            "!=" => v != self.value,
            "=~" => anchored(&self.value).is_some_and(|r| r.is_match(v)),
            "!~" => anchored(&self.value).is_none_or(|r| !r.is_match(v)),
            _ => false,
        }
    }
}

fn anchored(re: &str) -> Option<Regex> {
    Regex::new(&format!("^(?:{re})$")).ok()
}

const AGGS: &[&str] = &[
    "sum",
    "avg",
    "min",
    "max",
    "count",
    "group",
    "stddev",
    "stdvar",
    "topk",
    "bottomk",
    "quantile",
    "count_values",
];

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect(&mut self, t: Tok) -> Result<(), String> {
        match self.next() {
            Some(x) if x == t => Ok(()),
            other => Err(format!(
                "unexpected {}, expected {}",
                describe(other.as_ref()),
                describe(Some(&t))
            )),
        }
    }

    fn parse(&mut self) -> Result<Expr, String> {
        let e = self.binary(0)?;
        if let Some(t) = self.peek() {
            return Err(format!("unexpected {}", describe(Some(t))));
        }
        Ok(e)
    }

    fn prec(op: &str) -> Option<(u8, bool)> {
        Some(match op {
            "or" => (1, false),
            "and" | "unless" => (2, false),
            "==" | "!=" | ">" | "<" | ">=" | "<=" => (3, false),
            "+" | "-" => (4, false),
            "*" | "/" | "%" => (5, false),
            "^" => (6, true),
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        while let Some(Tok::Op(op)) = self.peek().cloned() {
            let Some((p, right)) = Self::prec(&op) else {
                break;
            };
            if p < min {
                break;
            }
            self.next();
            let mut bool_mod = false;
            if let Some(Tok::Ident(i)) = self.peek()
                && i == "bool"
            {
                bool_mod = true;
                self.next();
            }
            let mut on = None;
            if let Some(Tok::Ident(i)) = self.peek().cloned()
                && (i == "on" || i == "ignoring")
            {
                self.next();
                on = Some((i == "on", self.label_list()?));
                if let Some(Tok::Ident(g)) = self.peek().cloned()
                    && (g == "group_left" || g == "group_right")
                {
                    self.next();
                    if self.peek() == Some(&Tok::LParen) {
                        self.label_list()?;
                    }
                }
            }
            let rhs = self.binary(if right { p } else { p + 1 })?;
            lhs = Expr::Binary {
                op,
                bool_mod,
                on,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if let Some(Tok::Op(o)) = self.peek().cloned() {
            if o == "-" {
                self.next();
                return Ok(Expr::Neg(Box::new(self.unary()?)));
            }
            if o == "+" {
                self.next();
                return self.unary();
            }
        }
        let e = self.primary()?;
        self.postfix(e)
    }

    fn postfix(&mut self, mut e: Expr) -> Result<Expr, String> {
        loop {
            match self.peek() {
                Some(Tok::LBracket) => {
                    self.next();
                    let d = match self.next() {
                        Some(Tok::Dur(d)) => d,
                        Some(Tok::Num(n)) => n,
                        other => {
                            return Err(format!(
                                "unexpected {} in range",
                                describe(other.as_ref())
                            ));
                        }
                    };
                    self.expect(Tok::RBracket)?;
                    e = match e {
                        Expr::Selector { matchers, .. } => Expr::Selector {
                            matchers,
                            range: Some(d),
                        },
                        other => Expr::Call("__subquery".into(), vec![other, Expr::Num(d)]),
                    };
                }
                Some(Tok::Ident(i)) if i == "offset" => {
                    self.next();
                    self.next();
                }
                Some(Tok::Op(o)) if o == "@" => {
                    self.next();
                    self.next();
                }
                _ => return Ok(e),
            }
        }
    }

    fn label_list(&mut self) -> Result<Vec<String>, String> {
        self.expect(Tok::LParen)?;
        let mut out = Vec::new();
        loop {
            match self.next() {
                Some(Tok::RParen) => break,
                Some(Tok::Ident(i)) => out.push(i),
                Some(Tok::Str(s)) => out.push(s),
                Some(Tok::Comma) => {}
                other => {
                    return Err(format!(
                        "unexpected {} in grouping opts",
                        describe(other.as_ref())
                    ));
                }
            }
        }
        Ok(out)
    }

    fn matchers(&mut self, name: Option<String>) -> Result<Vec<Matcher>, String> {
        let mut ms = Vec::new();
        if let Some(n) = name {
            ms.push(Matcher {
                name: "__name__".into(),
                op: "=".into(),
                value: n,
            });
        }
        if self.peek() != Some(&Tok::LBrace) {
            return Ok(ms);
        }
        self.next();
        loop {
            match self.next() {
                Some(Tok::RBrace) => break,
                Some(Tok::Comma) => {}
                Some(Tok::Ident(label)) => {
                    let op = match self.next() {
                        Some(Tok::Op(o)) if ["=", "!=", "=~", "!~"].contains(&o.as_str()) => o,
                        other => {
                            return Err(format!(
                                "unexpected {} in label matching, expected label matching operator",
                                describe(other.as_ref())
                            ));
                        }
                    };
                    let value = match self.next() {
                        Some(Tok::Str(s)) => s,
                        other => {
                            return Err(format!(
                                "unexpected {} in label matching, expected string",
                                describe(other.as_ref())
                            ));
                        }
                    };
                    ms.push(Matcher {
                        name: label,
                        op,
                        value,
                    });
                }
                Some(Tok::Str(metric)) => ms.push(Matcher {
                    name: "__name__".into(),
                    op: "=".into(),
                    value: metric,
                }),
                other => {
                    return Err(format!(
                        "unexpected {} in label matching",
                        describe(other.as_ref())
                    ));
                }
            }
        }
        let empty = Labels::new();
        if ms.iter().all(|m| m.matches(&empty)) {
            return Err("vector selector must contain at least one non-empty matcher".into());
        }
        Ok(ms)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Dur(d)) => Ok(Expr::Num(d)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::LParen) => {
                let e = self.binary(0)?;
                self.expect(Tok::RParen)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            Some(Tok::LBrace) => {
                self.pos -= 1;
                Ok(Expr::Selector {
                    matchers: self.matchers(None)?,
                    range: None,
                })
            }
            Some(Tok::Ident(name)) => {
                let lower = name.to_ascii_lowercase();
                if AGGS.contains(&lower.as_str())
                    && matches!(self.peek(), Some(Tok::LParen) | Some(Tok::Ident(_)))
                {
                    let mut by = None;
                    if let Some(Tok::Ident(m)) = self.peek().cloned()
                        && (m == "by" || m == "without")
                    {
                        self.next();
                        by = Some((m == "by", self.label_list()?));
                    }
                    self.expect(Tok::LParen)?;
                    let mut args = vec![self.binary(0)?];
                    while self.peek() == Some(&Tok::Comma) {
                        self.next();
                        args.push(self.binary(0)?);
                    }
                    self.expect(Tok::RParen)?;
                    if let Some(Tok::Ident(m)) = self.peek().cloned()
                        && (m == "by" || m == "without")
                    {
                        self.next();
                        by = Some((m == "by", self.label_list()?));
                    }
                    let (param, expr) = if args.len() == 2 {
                        let e = args.pop().expect("arg");
                        (Some(Box::new(args.pop().expect("param"))), e)
                    } else {
                        (None, args.pop().expect("arg"))
                    };
                    return Ok(Expr::Agg {
                        op: lower,
                        by,
                        param,
                        expr: Box::new(expr),
                    });
                }
                if self.peek() == Some(&Tok::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        args.push(self.binary(0)?);
                        while self.peek() == Some(&Tok::Comma) {
                            self.next();
                            args.push(self.binary(0)?);
                        }
                    }
                    self.expect(Tok::RParen)?;
                    return Ok(Expr::Call(lower, args));
                }
                Ok(Expr::Selector {
                    matchers: self.matchers(Some(name))?,
                    range: None,
                })
            }
            other => Err(format!("unexpected {}", describe(other.as_ref()))),
        }
    }
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "end of input".into(),
        Some(Tok::Num(n)) => format!("number \"{n}\""),
        Some(Tok::Str(s)) => format!("string \"{s}\""),
        Some(Tok::Ident(i)) => format!("identifier \"{i}\""),
        Some(Tok::Dur(_)) => "duration".into(),
        Some(Tok::Op(o)) => format!("operator \"{o}\""),
        Some(Tok::LParen) => "\"(\"".into(),
        Some(Tok::RParen) => "right parenthesis \")\"".into(),
        Some(Tok::LBrace) => "\"{\"".into(),
        Some(Tok::RBrace) => "\"}\"".into(),
        Some(Tok::LBracket) => "\"[\"".into(),
        Some(Tok::RBracket) => "\"]\"".into(),
        Some(Tok::Comma) => "\",\"".into(),
    }
}

pub fn parse(src: &str) -> Result<Expr, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("no expression found in input".into());
    }
    Parser { toks, pos: 0 }.parse()
}

/// Metric names referenced by selectors in `e`.
pub fn selectors(e: &Expr, out: &mut Vec<Vec<Matcher>>) {
    match e {
        Expr::Selector { matchers, .. } => out.push(matchers.clone()),
        Expr::Call(_, args) => args.iter().for_each(|a| selectors(a, out)),
        Expr::Agg { param, expr, .. } => {
            if let Some(p) = param {
                selectors(p, out);
            }
            selectors(expr, out);
        }
        Expr::Binary { lhs, rhs, .. } => {
            selectors(lhs, out);
            selectors(rhs, out);
        }
        Expr::Neg(e) | Expr::Paren(e) => selectors(e, out),
        Expr::Num(_) | Expr::Str(_) => {}
    }
}

pub struct Ctx<'a> {
    pub series: &'a [Series],
    pub time: f64,
}

fn drop_name(mut l: Labels) -> Labels {
    l.remove("__name__");
    l
}

fn group_key(labels: &Labels, by: &Option<(bool, Vec<String>)>) -> Labels {
    match by {
        None => Labels::new(),
        Some((true, names)) => labels
            .iter()
            .filter(|(k, _)| names.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        Some((false, names)) => labels
            .iter()
            .filter(|(k, _)| !names.contains(k) && k.as_str() != "__name__")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

fn scalar_of(v: &Value) -> Result<f64, String> {
    match v {
        Value::Scalar(s) => Ok(*s),
        _ => Err("expected type scalar".into()),
    }
}

fn vector_of(v: Value, what: &str) -> Result<Vec<(Labels, f64)>, String> {
    match v {
        Value::Vector(v) => Ok(v),
        Value::Scalar(_) => Err(format!(
            "expected type instant vector in {what}, got scalar"
        )),
        Value::Range(..) => Err(format!(
            "expected type instant vector in {what}, got range vector"
        )),
        Value::Str(_) => Err(format!(
            "expected type instant vector in {what}, got string"
        )),
    }
}

pub fn eval(e: &Expr, ctx: &Ctx) -> Result<Value, String> {
    match e {
        Expr::Num(n) => Ok(Value::Scalar(*n)),
        Expr::Str(s) => Ok(Value::Str(s.clone())),
        Expr::Paren(e) => eval(e, ctx),
        Expr::Neg(e) => Ok(match eval(e, ctx)? {
            Value::Scalar(s) => Value::Scalar(-s),
            Value::Vector(v) => {
                Value::Vector(v.into_iter().map(|(l, x)| (drop_name(l), -x)).collect())
            }
            other => other,
        }),
        Expr::Selector { matchers, range } => {
            let picked: Vec<Series> = ctx
                .series
                .iter()
                .filter(|s| matchers.iter().all(|m| m.matches(&s.labels)))
                .cloned()
                .collect();
            Ok(match range {
                Some(d) => Value::Range(picked, *d),
                None => Value::Vector(picked.into_iter().map(|s| (s.labels, s.value)).collect()),
            })
        }
        Expr::Call(name, args) => call(name, args, ctx),
        Expr::Agg {
            op,
            by,
            param,
            expr,
        } => {
            let v = vector_of(eval(expr, ctx)?, "aggregation expression")?;
            let mut groups: Vec<(Labels, Vec<(Labels, f64)>)> = Vec::new();
            for (l, x) in v {
                let k = group_key(&l, by);
                match groups.iter_mut().find(|(g, _)| *g == k) {
                    Some((_, items)) => items.push((l, x)),
                    None => groups.push((k, vec![(l, x)])),
                }
            }
            let mut out = Vec::new();
            for (k, items) in groups {
                let xs: Vec<f64> = items.iter().map(|(_, x)| *x).collect();
                let n = xs.len() as f64;
                match op.as_str() {
                    "sum" => out.push((k, xs.iter().sum())),
                    "avg" => out.push((k, xs.iter().sum::<f64>() / n)),
                    "min" => out.push((k, xs.iter().cloned().fold(f64::INFINITY, f64::min))),
                    "max" => out.push((k, xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max))),
                    "count" => out.push((k, n)),
                    "group" => out.push((k, 1.0)),
                    "stddev" | "stdvar" => {
                        let mean = xs.iter().sum::<f64>() / n;
                        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
                        out.push((k, if op == "stddev" { var.sqrt() } else { var }));
                    }
                    "quantile" => {
                        let q = scalar_of(&eval(
                            param.as_ref().ok_or("quantile needs a parameter")?,
                            ctx,
                        )?)?;
                        let mut s = xs.clone();
                        s.sort_by(|a, b| a.total_cmp(b));
                        let rank = q * (s.len() as f64 - 1.0);
                        let lo = rank.floor() as usize;
                        let hi = rank.ceil() as usize;
                        let w = rank - lo as f64;
                        out.push((k, s[lo] * (1.0 - w) + s[hi.min(s.len() - 1)] * w));
                    }
                    "topk" | "bottomk" => {
                        let kk = scalar_of(&eval(
                            param.as_ref().ok_or("topk needs a parameter")?,
                            ctx,
                        )?)? as usize;
                        let mut items = items;
                        items.sort_by(|a, b| {
                            if op == "topk" {
                                b.1.total_cmp(&a.1)
                            } else {
                                a.1.total_cmp(&b.1)
                            }
                        });
                        out.extend(items.into_iter().take(kk));
                    }
                    "count_values" => out.push((k, n)),
                    _ => return Err(format!("unknown aggregation {op}")),
                }
            }
            Ok(Value::Vector(out))
        }
        Expr::Binary {
            op,
            bool_mod,
            on,
            lhs,
            rhs,
        } => {
            let l = eval(lhs, ctx)?;
            let r = eval(rhs, ctx)?;
            binop(op, *bool_mod, on, l, r)
        }
    }
}

fn arith(op: &str, a: f64, b: f64) -> Option<f64> {
    Some(match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "%" => a % b,
        "^" => a.powf(b),
        "==" => f64::from(u8::from(a == b)),
        "!=" => f64::from(u8::from(a != b)),
        ">" => f64::from(u8::from(a > b)),
        "<" => f64::from(u8::from(a < b)),
        ">=" => f64::from(u8::from(a >= b)),
        "<=" => f64::from(u8::from(a <= b)),
        _ => return None,
    })
}

fn is_cmp(op: &str) -> bool {
    matches!(op, "==" | "!=" | ">" | "<" | ">=" | "<=")
}

fn binop(
    op: &str,
    bool_mod: bool,
    on: &Option<(bool, Vec<String>)>,
    l: Value,
    r: Value,
) -> Result<Value, String> {
    match (l, r) {
        (Value::Scalar(a), Value::Scalar(b)) => {
            Ok(Value::Scalar(arith(op, a, b).ok_or("bad operator")?))
        }
        (Value::Vector(v), Value::Scalar(b)) => Ok(Value::Vector(
            v.into_iter()
                .filter_map(|(l, a)| {
                    let x = arith(op, a, b)?;
                    if is_cmp(op) && !bool_mod {
                        (x == 1.0).then_some((l, a))
                    } else {
                        Some((drop_name(l), x))
                    }
                })
                .collect(),
        )),
        (Value::Scalar(a), Value::Vector(v)) => Ok(Value::Vector(
            v.into_iter()
                .filter_map(|(l, b)| {
                    let x = arith(op, a, b)?;
                    if is_cmp(op) && !bool_mod {
                        (x == 1.0).then_some((l, b))
                    } else {
                        Some((drop_name(l), x))
                    }
                })
                .collect(),
        )),
        (Value::Vector(lv), Value::Vector(rv)) => {
            let key = |l: &Labels| -> Labels {
                match on {
                    Some((true, names)) => l
                        .iter()
                        .filter(|(k, _)| names.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    Some((false, names)) => l
                        .iter()
                        .filter(|(k, _)| !names.contains(k) && k.as_str() != "__name__")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    None => drop_name(l.clone()),
                }
            };
            match op {
                "and" => Ok(Value::Vector(
                    lv.into_iter()
                        .filter(|(l, _)| rv.iter().any(|(r, _)| key(r) == key(l)))
                        .collect(),
                )),
                "unless" => Ok(Value::Vector(
                    lv.into_iter()
                        .filter(|(l, _)| !rv.iter().any(|(r, _)| key(r) == key(l)))
                        .collect(),
                )),
                "or" => {
                    let mut out = lv.clone();
                    for (r, x) in rv {
                        if !lv.iter().any(|(l, _)| key(l) == key(&r)) {
                            out.push((r, x));
                        }
                    }
                    Ok(Value::Vector(out))
                }
                _ => {
                    let mut out = Vec::new();
                    for (l, a) in &lv {
                        if let Some((_, b)) = rv.iter().find(|(r, _)| key(r) == key(l)) {
                            let x = arith(op, *a, *b).ok_or("bad operator")?;
                            if is_cmp(op) && !bool_mod {
                                if x == 1.0 {
                                    out.push((l.clone(), *a));
                                }
                            } else {
                                let labels = if on.is_some() {
                                    key(l)
                                } else {
                                    drop_name(l.clone())
                                };
                                out.push((labels, x));
                            }
                        }
                    }
                    Ok(Value::Vector(out))
                }
            }
        }
        _ => Err(format!(
            "binary expression must contain only scalar and instant vector types (operator {op})"
        )),
    }
}

fn range_arg(args: &[Expr], ctx: &Ctx, name: &str) -> Result<(Vec<Series>, f64), String> {
    match args.first().map(|a| eval(a, ctx)).transpose()? {
        Some(Value::Range(s, d)) => Ok((s, d)),
        _ => Err(format!(
            "expected type range vector in call to function \"{name}\", got instant vector"
        )),
    }
}

fn call(name: &str, args: &[Expr], ctx: &Ctx) -> Result<Value, String> {
    let math = |f: fn(f64) -> f64| -> Result<Value, String> {
        let v = vector_of(eval(args.first().ok_or("missing argument")?, ctx)?, "call")?;
        Ok(Value::Vector(
            v.into_iter().map(|(l, x)| (drop_name(l), f(x))).collect(),
        ))
    };
    match name {
        "rate" | "irate" => {
            let (s, _) = range_arg(args, ctx, name)?;
            Ok(Value::Vector(
                s.into_iter()
                    .map(|s| (drop_name(s.labels), s.rate.unwrap_or(0.0)))
                    .collect(),
            ))
        }
        "increase" => {
            let (s, d) = range_arg(args, ctx, name)?;
            Ok(Value::Vector(
                s.into_iter()
                    .map(|s| (drop_name(s.labels), s.rate.unwrap_or(0.0) * d))
                    .collect(),
            ))
        }
        "delta" | "idelta" | "deriv" | "changes" | "resets" => {
            let (s, _) = range_arg(args, ctx, name)?;
            Ok(Value::Vector(
                s.into_iter().map(|s| (drop_name(s.labels), 0.0)).collect(),
            ))
        }
        "avg_over_time" | "min_over_time" | "max_over_time" | "last_over_time"
        | "quantile_over_time" | "present_over_time" => {
            let idx = usize::from(name == "quantile_over_time");
            let (s, _) = range_arg(&args[idx..], ctx, name)?;
            Ok(Value::Vector(
                s.into_iter()
                    .map(|s| {
                        let l = if name == "last_over_time" {
                            s.labels
                        } else {
                            drop_name(s.labels)
                        };
                        (
                            l,
                            if name == "present_over_time" {
                                1.0
                            } else {
                                s.value
                            },
                        )
                    })
                    .collect(),
            ))
        }
        "sum_over_time" | "count_over_time" => {
            let (s, d) = range_arg(args, ctx, name)?;
            let samples = (d / 15.0).max(1.0).floor();
            Ok(Value::Vector(
                s.into_iter()
                    .map(|s| {
                        (
                            drop_name(s.labels),
                            if name == "count_over_time" {
                                samples
                            } else {
                                s.value * samples
                            },
                        )
                    })
                    .collect(),
            ))
        }
        "__subquery" => eval(&args[0], ctx).map(|v| match v {
            Value::Vector(v) => Value::Range(
                v.into_iter()
                    .map(|(labels, value)| Series {
                        labels,
                        value,
                        rate: Some(0.0),
                    })
                    .collect(),
                0.0,
            ),
            other => other,
        }),
        "abs" => math(f64::abs),
        "ceil" => math(f64::ceil),
        "floor" => math(f64::floor),
        "sqrt" => math(f64::sqrt),
        "exp" => math(f64::exp),
        "ln" => math(f64::ln),
        "log2" => math(f64::log2),
        "log10" => math(f64::log10),
        "round" => {
            let v = vector_of(eval(&args[0], ctx)?, "call to round")?;
            let to = match args.get(1) {
                Some(a) => scalar_of(&eval(a, ctx)?)?,
                None => 1.0,
            };
            Ok(Value::Vector(
                v.into_iter()
                    .map(|(l, x)| (drop_name(l), (x / to).round() * to))
                    .collect(),
            ))
        }
        "clamp" | "clamp_min" | "clamp_max" => {
            let v = vector_of(eval(&args[0], ctx)?, "call to clamp")?;
            let a = scalar_of(&eval(args.get(1).ok_or("missing argument")?, ctx)?)?;
            let b = match args.get(2) {
                Some(x) => scalar_of(&eval(x, ctx)?)?,
                None => a,
            };
            Ok(Value::Vector(
                v.into_iter()
                    .map(|(l, x)| {
                        let y = match name {
                            "clamp" => x.max(a).min(b),
                            "clamp_min" => x.max(a),
                            _ => x.min(a),
                        };
                        (drop_name(l), y)
                    })
                    .collect(),
            ))
        }
        "scalar" => {
            let v = vector_of(eval(&args[0], ctx)?, "call to scalar")?;
            Ok(Value::Scalar(if v.len() == 1 { v[0].1 } else { f64::NAN }))
        }
        "vector" => Ok(Value::Vector(vec![(
            Labels::new(),
            scalar_of(&eval(&args[0], ctx)?)?,
        )])),
        "time" => Ok(Value::Scalar(ctx.time)),
        "timestamp" => {
            let v = vector_of(eval(&args[0], ctx)?, "call to timestamp")?;
            Ok(Value::Vector(
                v.into_iter()
                    .map(|(l, _)| (drop_name(l), ctx.time))
                    .collect(),
            ))
        }
        "sort" | "sort_desc" => {
            let mut v = vector_of(eval(&args[0], ctx)?, "call to sort")?;
            v.sort_by(|a, b| {
                if name == "sort" {
                    a.1.total_cmp(&b.1)
                } else {
                    b.1.total_cmp(&a.1)
                }
            });
            Ok(Value::Vector(v))
        }
        "absent" => {
            let v = vector_of(eval(&args[0], ctx)?, "call to absent")?;
            Ok(Value::Vector(if v.is_empty() {
                vec![(Labels::new(), 1.0)]
            } else {
                vec![]
            }))
        }
        "label_replace" | "label_join" => eval(&args[0], ctx),
        "histogram_quantile" => {
            let q = scalar_of(&eval(&args[0], ctx)?)?;
            let v = vector_of(
                eval(args.get(1).ok_or("missing argument")?, ctx)?,
                "call to histogram_quantile",
            )?;
            let mut groups: Vec<(Labels, Vec<(f64, f64)>)> = Vec::new();
            for (l, x) in v {
                let Some(le) = l.get("le").and_then(|s| {
                    if s == "+Inf" {
                        Some(f64::INFINITY)
                    } else {
                        s.parse().ok()
                    }
                }) else {
                    continue;
                };
                let mut k = drop_name(l);
                k.remove("le");
                match groups.iter_mut().find(|(g, _)| *g == k) {
                    Some((_, b)) => b.push((le, x)),
                    None => groups.push((k, vec![(le, x)])),
                }
            }
            Ok(Value::Vector(
                groups
                    .into_iter()
                    .map(|(k, b)| (k, bucket_quantile(q, b)))
                    .collect(),
            ))
        }
        other => Err(format!("unknown function with name \"{other}\"")),
    }
}

/// Prometheus' `bucketQuantile` with linear interpolation inside the bucket.
pub fn bucket_quantile(q: f64, mut buckets: Vec<(f64, f64)>) -> f64 {
    if q < 0.0 {
        return f64::NEG_INFINITY;
    }
    if q > 1.0 {
        return f64::INFINITY;
    }
    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    if buckets.len() < 2 || buckets.last().is_none_or(|b| !b.0.is_infinite()) {
        return f64::NAN;
    }
    let total = buckets.last().expect("bucket").1;
    if total == 0.0 {
        return f64::NAN;
    }
    let rank = q * total;
    let b = buckets
        .iter()
        .position(|(_, c)| *c >= rank)
        .unwrap_or(buckets.len() - 1);
    if b == buckets.len() - 1 {
        return buckets[buckets.len() - 2].0;
    }
    if b == 0 && buckets[0].0 <= 0.0 {
        return buckets[0].0;
    }
    let (start, prev) = if b == 0 {
        (0.0, 0.0)
    } else {
        (buckets[b - 1].0, buckets[b - 1].1)
    };
    let (end, count) = buckets[b];
    start + (end - start) * ((rank - prev) / (count - prev))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(labels: &[(&str, &str)], value: f64, rate: f64) -> Series {
        Series {
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            value,
            rate: Some(rate),
        }
    }

    fn run(expr: &str, series: &[Series]) -> Vec<(Labels, f64)> {
        match eval(&parse(expr).unwrap(), &Ctx { series, time: 0.0 }).unwrap() {
            Value::Vector(v) => v,
            Value::Scalar(x) => vec![(Labels::new(), x)],
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn selectors_rates_and_sums() {
        let data = vec![
            s(
                &[
                    ("__name__", "http_requests_total"),
                    ("pod", "a"),
                    ("status", "500"),
                ],
                100.0,
                2.0,
            ),
            s(
                &[
                    ("__name__", "http_requests_total"),
                    ("pod", "b"),
                    ("status", "500"),
                ],
                50.0,
                0.5,
            ),
            s(
                &[
                    ("__name__", "http_requests_total"),
                    ("pod", "a"),
                    ("status", "200"),
                ],
                900.0,
                40.0,
            ),
        ];
        let r = run(
            "sum by (pod) (rate(http_requests_total{status=~\"5..\"}[5m]))",
            &data,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].1, 2.0);
        let ratio = run(
            "sum(rate(http_requests_total{status=~\"5..\"}[5m])) / sum(rate(http_requests_total[5m]))",
            &data,
        );
        assert!((ratio[0].1 - 2.5 / 42.5).abs() < 1e-9);
        let top = run("topk(1, rate(http_requests_total[5m]))", &data);
        assert_eq!(top[0].0["status"], "200");
        assert_eq!(run("count(http_requests_total) > 2", &data)[0].1, 3.0);
        assert!(parse("sum(").is_err());
    }

    #[test]
    fn histogram_quantile_interpolates() {
        let data: Vec<Series> = [
            ("0.1", 50.0),
            ("0.5", 90.0),
            ("1", 98.0),
            ("2.5", 100.0),
            ("+Inf", 100.0),
        ]
        .iter()
        .map(|(le, c)| {
            s(
                &[
                    ("__name__", "lat_bucket"),
                    ("le", le),
                    ("service", "checkout"),
                ],
                *c * 10.0,
                *c,
            )
        })
        .collect();
        let r = run(
            "histogram_quantile(0.99, sum by (le) (rate(lat_bucket[5m])))",
            &data,
        );
        assert!((r[0].1 - 1.75).abs() < 1e-9, "{r:?}");
    }
}
