//! A JQL subset: clauses (`=`, `!=`, `~`, `!~`, `<`, `<=`, `>`, `>=`, `IN`, `NOT IN`,
//! `IS [NOT] EMPTY|NULL`), `AND`/`OR`/`NOT`, parentheses, functions (`currentUser()`,
//! `now()`, `startOfDay()`, `endOfDay()`, `startOfWeek()`, `startOfMonth()`,
//! `openSprints()`, `closedSprints()`, `futureSprints()`, `membersOf()`), relative dates
//! (`-7d`, `-2w`, `-4h`) and `ORDER BY`.

use crate::model::{days_from_civil, parse_time};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Op(String),
    LParen,
    RParen,
    Comma,
}

fn lex(q: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = q.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if c == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err(format!(
                    "Error in the JQL Query: The quoted string '{s}' has not been completed. (line 1, character {})",
                    i
                ));
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if "=!~<>".contains(c) {
            let mut op = c.to_string();
            if i + 1 < chars.len()
                && (chars[i + 1] == '=' || chars[i + 1] == '~')
                && c != '='
                && c != '~'
            {
                op.push(chars[i + 1]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Op(op));
        } else {
            let mut s = String::new();
            while i < chars.len() && !chars[i].is_whitespace() && !"()=!~<>,\"'".contains(chars[i])
            {
                s.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub enum Val {
    Lit(String),
    Func(String, Vec<String>),
    Empty,
}

#[derive(Debug, Clone)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Clause {
        field: String,
        op: String,
        vals: Vec<Val>,
    },
    All,
}

pub struct Query {
    pub expr: Expr,
    pub order: Vec<(String, bool)>,
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

fn kw(t: Option<&Tok>, k: &str) -> bool {
    matches!(t, Some(Tok::Word(w)) if w.eq_ignore_ascii_case(k))
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while kw(self.peek(), "or") || matches!(self.peek(), Some(Tok::Op(o)) if o == "||") {
            self.next();
            l = Expr::Or(Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while kw(self.peek(), "and") {
            self.next();
            l = Expr::And(Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }
    fn not(&mut self) -> Result<Expr, String> {
        if kw(self.peek(), "not") || matches!(self.peek(), Some(Tok::Op(o)) if o == "!") {
            self.next();
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        if self.peek() == Some(&Tok::LParen) {
            self.next();
            let e = self.or()?;
            if self.next() != Some(Tok::RParen) {
                return Err(
                    "Error in the JQL Query: Expecting ')' before the end of the query.".into(),
                );
            }
            return Ok(e);
        }
        self.clause()
    }
    fn value(&mut self) -> Result<Val, String> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(Val::Lit(s)),
            Some(Tok::Word(w)) => {
                if w.eq_ignore_ascii_case("empty") || w.eq_ignore_ascii_case("null") {
                    return Ok(Val::Empty);
                }
                if self.peek() == Some(&Tok::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    loop {
                        match self.next() {
                            Some(Tok::RParen) => break,
                            Some(Tok::Comma) => {}
                            Some(Tok::Word(a) | Tok::Str(a)) => args.push(a),
                            _ => {
                                return Err(format!(
                                    "Error in the JQL Query: Expecting ')' after the arguments of function '{w}'."
                                ));
                            }
                        }
                    }
                    return Ok(Val::Func(w.to_lowercase(), args));
                }
                Ok(Val::Lit(w))
            }
            other => Err(format!(
                "Error in the JQL Query: Expecting either a value, list or function but got '{}'.",
                tok_text(other.as_ref())
            )),
        }
    }
    fn clause(&mut self) -> Result<Expr, String> {
        let field = match self.next() {
            Some(Tok::Word(w) | Tok::Str(w)) => w,
            other => {
                return Err(format!(
                    "Error in the JQL Query: Expecting a field name but got '{}'.",
                    tok_text(other.as_ref())
                ));
            }
        };
        let mut field = field;
        // cf[10001]
        if field.to_lowercase().starts_with("cf[") {
            field = format!("customfield_{}", field[3..].trim_end_matches(']'));
        }
        let op = match self.next() {
            Some(Tok::Op(o)) => o,
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("in") => "in".into(),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("not") => {
                if kw(self.peek(), "in") {
                    self.next();
                    "not in".into()
                } else {
                    return Err("Error in the JQL Query: Expecting 'IN' after 'NOT'.".into());
                }
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("is") => {
                if kw(self.peek(), "not") {
                    self.next();
                    "is not".into()
                } else {
                    "is".into()
                }
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("was") => "was".into(),
            other => {
                return Err(format!(
                    "Error in the JQL Query: Expecting operator but got '{}'. The valid operators are '=', '!=', '<', '>', '<=', '>=', '~', '!~', 'IN', 'NOT IN', 'IS' and 'IS NOT'.",
                    tok_text(other.as_ref())
                ));
            }
        };
        let list_fn = matches!(self.peek(), Some(Tok::Word(_)))
            && self.t.get(self.i + 1) == Some(&Tok::LParen);
        let vals = if (op == "in" || op == "not in") && list_fn {
            vec![self.value()?]
        } else if op == "in" || op == "not in" {
            if self.next() != Some(Tok::LParen) {
                return Err("Error in the JQL Query: Expecting '(' after 'IN'.".into());
            }
            let mut v = Vec::new();
            loop {
                if self.peek() == Some(&Tok::RParen) {
                    self.next();
                    break;
                }
                v.push(self.value()?);
                match self.next() {
                    Some(Tok::Comma) => {}
                    Some(Tok::RParen) => break,
                    _ => {
                        return Err(
                            "Error in the JQL Query: Expecting ',' or ')' in the value list."
                                .into(),
                        );
                    }
                }
            }
            v
        } else {
            vec![self.value()?]
        };
        Ok(Expr::Clause { field, op, vals })
    }
}

fn tok_text(t: Option<&Tok>) -> String {
    match t {
        Some(Tok::Word(w) | Tok::Str(w) | Tok::Op(w)) => w.clone(),
        Some(Tok::LParen) => "(".into(),
        Some(Tok::RParen) => ")".into(),
        Some(Tok::Comma) => ",".into(),
        None => "<EOF>".into(),
    }
}

pub fn parse(q: &str) -> Result<Query, String> {
    let toks = lex(q)?;
    // Split off ORDER BY.
    let mut cut = toks.len();
    for i in 0..toks.len() {
        if kw(toks.get(i), "order") && kw(toks.get(i + 1), "by") {
            cut = i;
            break;
        }
    }
    let (head, tail) = toks.split_at(cut);
    let expr = if head.is_empty() {
        Expr::All
    } else {
        let mut p = P {
            t: head.to_vec(),
            i: 0,
        };
        let e = p.or()?;
        if p.i < p.t.len() {
            return Err(format!(
                "Error in the JQL Query: Expecting either 'OR' or 'AND' but got '{}'.",
                tok_text(p.t.get(p.i))
            ));
        }
        e
    };
    let mut order = Vec::new();
    let mut i = 2;
    while i < tail.len() {
        let f = match &tail[i] {
            Tok::Word(w) | Tok::Str(w) => w.clone(),
            Tok::Comma => {
                i += 1;
                continue;
            }
            _ => return Err("Error in the JQL Query: Expecting a field name in ORDER BY.".into()),
        };
        let mut desc = false;
        if kw(tail.get(i + 1), "desc") {
            desc = true;
            i += 1;
        } else if kw(tail.get(i + 1), "asc") {
            i += 1;
        }
        order.push((f.to_lowercase(), desc));
        i += 1;
    }
    Ok(Query { expr, order })
}

/// What a clause compares against for one issue.
pub struct Env<'a> {
    pub now: i64,
    pub me: &'a str,
    pub sprints: &'a dyn Fn(&str) -> Vec<String>,
}

/// Resolves a value (functions, relative dates) into candidate strings.
fn resolve(v: &Val, env: &Env) -> Vec<String> {
    match v {
        Val::Lit(x) => vec![x.clone()],
        Val::Empty => vec![],
        Val::Func(name, args) => match name.as_str() {
            "currentuser" => vec![env.me.to_owned()],
            "now" => vec![env.now.to_string()],
            "startofday" | "endofday" | "startofweek" | "endofweek" | "startofmonth"
            | "endofmonth" | "startofyear" => {
                let shift = args.first().and_then(|a| relative(a, 0)).unwrap_or(0);
                vec![boundary(name, env.now + shift).to_string()]
            }
            "opensprints" => (env.sprints)("active"),
            "closedsprints" => (env.sprints)("closed"),
            "futuresprints" => (env.sprints)("future"),
            _ => args.clone(),
        },
    }
}

fn boundary(name: &str, t: i64) -> i64 {
    let day = t.div_euclid(86_400);
    match name {
        "startofday" => day * 86_400,
        "endofday" => day * 86_400 + 86_399,
        "startofweek" => (day - (day + 4).rem_euclid(7)) * 86_400,
        "endofweek" => (day - (day + 4).rem_euclid(7) + 6) * 86_400 + 86_399,
        "startofmonth" | "endofmonth" | "startofyear" => {
            let (y, m, _) = crate::model::civil(day);
            match name {
                "startofmonth" => days_from_civil(y, m, 1) * 86_400,
                "startofyear" => days_from_civil(y, 1, 1) * 86_400,
                _ => {
                    days_from_civil(
                        if m == 12 { y + 1 } else { y },
                        if m == 12 { 1 } else { m + 1 },
                        1,
                    ) * 86_400
                        - 1
                }
            }
        }
        _ => t,
    }
}

/// `-7d` → seconds offset.
fn relative(v: &str, _now: i64) -> Option<i64> {
    let v = v.trim();
    let (sign, rest) = match v.chars().next()? {
        '-' => (-1, &v[1..]),
        '+' => (1, &v[1..]),
        _ => (1, v),
    };
    let unit = rest.chars().last()?;
    let n: i64 = rest[..rest.len() - 1].trim().parse().ok()?;
    let secs = match unit {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 7 * 86_400,
        'M' => 30 * 86_400,
        'y' => 365 * 86_400,
        _ => return None,
    };
    Some(sign * n * secs)
}

fn to_time(v: &str, now: i64) -> Option<i64> {
    if let Ok(n) = v.parse::<i64>()
        && n > 100_000_000
    {
        return Some(n);
    }
    if let Some(r) = relative(v, now) {
        return Some(now + r);
    }
    parse_time(v)
}

pub const TIME_FIELDS: [&str; 6] = [
    "created",
    "updated",
    "duedate",
    "resolved",
    "resolutiondate",
    "lastviewed",
];

/// Evaluates `e`; `get(field)` returns the issue's candidate values (None: unknown field).
pub fn eval(
    e: &Expr,
    env: &Env,
    get: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Result<bool, String> {
    Ok(match e {
        Expr::All => true,
        Expr::And(a, b) => eval(a, env, get)? && eval(b, env, get)?,
        Expr::Or(a, b) => eval(a, env, get)? || eval(b, env, get)?,
        Expr::Not(a) => !eval(a, env, get)?,
        Expr::Clause { field, op, vals } => {
            let f = field.to_lowercase();
            let have = get(&f).ok_or_else(|| {
                format!("Field '{field}' does not exist or you do not have permission to view it.")
            })?;
            let wanted: Vec<String> = vals.iter().flat_map(|v| resolve(v, env)).collect();
            let empty_wanted = vals.iter().all(|v| matches!(v, Val::Empty));
            let is_time = TIME_FIELDS.contains(&f.as_str());
            let eq = |a: &str, b: &str| {
                if is_time {
                    match (to_time(a, env.now), to_time(b, env.now)) {
                        (Some(x), Some(y)) => {
                            x == y
                                || (b.len() <= 10 && x.div_euclid(86_400) == y.div_euclid(86_400))
                        }
                        _ => false,
                    }
                } else {
                    a.eq_ignore_ascii_case(b)
                }
            };
            match op.as_str() {
                "=" | "is" | "was" => {
                    if empty_wanted {
                        have.is_empty()
                    } else {
                        have.iter().any(|h| wanted.iter().any(|w| eq(h, w)))
                    }
                }
                "!=" | "is not" => {
                    if empty_wanted {
                        !have.is_empty()
                    } else {
                        !have.is_empty() && !have.iter().any(|h| wanted.iter().any(|w| eq(h, w)))
                    }
                }
                "in" => {
                    have.iter().any(|h| wanted.iter().any(|w| eq(h, w)))
                        || (vals.iter().any(|v| matches!(v, Val::Empty)) && have.is_empty())
                }
                "not in" => {
                    !have.is_empty() && !have.iter().any(|h| wanted.iter().any(|w| eq(h, w)))
                }
                "~" | "!~" => {
                    let hay = have.join(" ").to_lowercase();
                    let hit = wanted.iter().all(|w| {
                        let w = w.to_lowercase();
                        let w = w.trim_matches('"');
                        w.split_whitespace().all(|word| {
                            let word = word.trim_end_matches('*');
                            hay.contains(word)
                        })
                    });
                    if op == "~" { hit } else { !hit }
                }
                "<" | "<=" | ">" | ">=" => {
                    let Some(target) = wanted.first() else {
                        return Ok(false);
                    };
                    have.iter().any(|h| {
                        let ord = if is_time {
                            match (to_time(h, env.now), to_time(target, env.now)) {
                                (Some(x), Some(y)) => x.cmp(&y),
                                _ => return false,
                            }
                        } else if let (Ok(x), Ok(y)) = (h.parse::<f64>(), target.parse::<f64>()) {
                            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                        } else {
                            h.cmp(target)
                        };
                        match op.as_str() {
                            "<" => ord.is_lt(),
                            "<=" => ord.is_le(),
                            ">" => ord.is_gt(),
                            _ => ord.is_ge(),
                        }
                    })
                }
                _ => false,
            }
        }
    })
}

/// Project keys a query names with `project = X` / `project in (...)` (for validation).
pub fn named_values(e: &Expr, field: &str, out: &mut Vec<String>) {
    match e {
        Expr::And(a, b) | Expr::Or(a, b) => {
            named_values(a, field, out);
            named_values(b, field, out);
        }
        Expr::Not(a) => named_values(a, field, out),
        Expr::Clause { field: f, op, vals }
            if f.eq_ignore_ascii_case(field)
                && ["=", "in", "!=", "not in"].contains(&op.as_str()) =>
        {
            out.extend(vals.iter().filter_map(|v| {
                if let Val::Lit(x) = v {
                    Some(x.clone())
                } else {
                    None
                }
            }));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(q: &str, fields: &[(&str, &[&str])]) -> bool {
        let parsed = parse(q).unwrap();
        let sprints = |_: &str| vec!["7".to_owned()];
        let env = Env {
            now: 1_793_286_000,
            me: "alex",
            sprints: &sprints,
        };
        let get = |f: &str| {
            fields
                .iter()
                .find(|(k, _)| *k == f)
                .map(|(_, v)| v.iter().map(|x| (*x).to_owned()).collect())
        };
        eval(&parsed.expr, &env, &get).unwrap()
    }

    #[test]
    fn clauses() {
        let f: &[(&str, &[&str])] = &[
            ("project", &["PAY", "10000"]),
            ("status", &["In Progress"]),
            ("assignee", &["alex"]),
            ("labels", &["safari", "checkout"]),
            ("summary", &["Checkout fails on Safari 17"]),
            ("created", &["2026-10-27T10:00:00.000+0000"]),
            ("sprint", &["7"]),
            ("resolution", &[]),
        ];
        assert!(run("project = PAY AND status = \"In Progress\"", f));
        assert!(run("project = pay and assignee = currentUser()", f));
        assert!(run(
            "labels in (safari, ios) AND summary ~ \"safari checkout\"",
            f
        ));
        assert!(!run("status not in (\"In Progress\", Done)", f));
        assert!(run("resolution is EMPTY and created >= -7d", f));
        assert!(run("created > \"2026-10-01\" ORDER BY created DESC", f));
        assert!(run("sprint in openSprints()", f));
        assert!(!run(
            "(project = WEB OR labels = ios) AND NOT status = Done",
            f
        ));
        let q = parse("project = PAY ORDER BY priority DESC, key ASC").unwrap();
        assert_eq!(
            q.order,
            vec![("priority".into(), true), ("key".into(), false)]
        );
        assert!(parse("project = ").is_err());
        assert!(parse("project PAY").is_err());
    }
}
