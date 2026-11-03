//! A small LogQL evaluator: stream selectors, line filters, `json`/`logfmt` parsers, label
//! filters, and the range aggregations (`count_over_time`, `rate`, `bytes_over_time`, ...)
//! with an optional outer `sum|count|avg|min|max|topk by (...)`.

use std::collections::BTreeMap;

use regex::Regex;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct Entry {
    pub ts: f64,
    pub line: String,
}

#[derive(Debug, Clone)]
pub struct Stream {
    pub labels: Labels,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
enum Stage {
    Line {
        op: String,
        values: Vec<String>,
    },
    Json,
    Logfmt,
    Regexp(Regex),
    Label {
        name: String,
        op: String,
        value: String,
    },
    Ignore,
}

#[derive(Debug, Clone)]
pub struct LogQuery {
    pub matchers: Vec<(String, String, String)>,
    stages: Vec<Stage>,
}

/// Outer aggregation: operator, `topk` parameter, and `by` (true) / `without` labels.
pub type Agg = (String, Option<usize>, Option<(bool, Vec<String>)>);

#[derive(Debug, Clone)]
pub struct MetricQuery {
    pub func: String,
    pub range: f64,
    pub log: LogQuery,
    pub agg: Option<Agg>,
}

#[derive(Debug, Clone)]
pub enum Query {
    Log(LogQuery),
    Metric(MetricQuery),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn peek(&mut self, t: &str) -> bool {
        self.ws();
        self.s[self.i..].starts_with(t.as_bytes())
    }

    fn ident(&mut self) -> Option<String> {
        self.ws();
        let start = self.i;
        while self.i < self.s.len()
            && ((self.s[self.i] as char).is_alphanumeric() || b"_.".contains(&self.s[self.i]))
        {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        self.ws();
        let q = *self
            .s
            .get(self.i)
            .ok_or("unexpected end of query, expected string")?;
        if q != b'"' && q != b'`' {
            return Err(format!(
                "syntax error: unexpected {}, expecting STRING",
                self.s[self.i] as char
            ));
        }
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() && self.s[self.i] != q {
            if q == b'"' && self.s[self.i] == b'\\' && self.i + 1 < self.s.len() {
                self.i += 1;
                match self.s[self.i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    other => {
                        out.push(b'\\');
                        out.push(other);
                    }
                }
            } else {
                out.push(self.s[self.i]);
            }
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return Err("syntax error: unterminated string".into());
        }
        self.i += 1;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn word(&mut self) -> String {
        self.ws();
        let start = self.i;
        while self.i < self.s.len()
            && !(self.s[self.i] as char).is_whitespace()
            && !b"|)}".contains(&self.s[self.i])
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }
}

fn duration(s: &str) -> Option<f64> {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.')?);
    let n: f64 = num.parse().ok()?;
    Some(
        n * match unit {
            "ms" => 0.001,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            "d" => 86400.0,
            "w" => 604_800.0,
            _ => return None,
        },
    )
}

fn log_query(p: &mut P) -> Result<LogQuery, String> {
    if !p.eat("{") {
        return Err("parse error : queries require at least one regexp or equality matcher that does not have an empty-compatible value. For instance, app=~\".*\" does not meet this requirement, but app=~\".+\" will".into());
    }
    let mut matchers = Vec::new();
    loop {
        if p.eat("}") {
            break;
        }
        p.eat(",");
        let Some(name) = p.ident() else {
            return Err(
                "parse error : syntax error: unexpected character in stream selector".into(),
            );
        };
        let op = ["=~", "!~", "!=", "="]
            .into_iter()
            .find(|o| p.eat(o))
            .ok_or(
                "parse error : syntax error: unexpected IDENTIFIER, expecting = or =~ or != or !~",
            )?;
        let value = p.string().map_err(|e| format!("parse error : {e}"))?;
        matchers.push((name, op.to_owned(), value));
    }
    if matchers.is_empty()
        || matchers.iter().all(|(_, op, v)| {
            (op == "=" && v.is_empty())
                || (op == "=~" && (v == ".*" || v.is_empty()))
                || op == "!="
                || op == "!~"
        })
    {
        return Err("parse error : queries require at least one regexp or equality matcher that does not have an empty-compatible value. For instance, app=~\".*\" does not meet this requirement, but app=~\".+\" will".into());
    }
    let mut stages = Vec::new();
    loop {
        p.ws();
        let op = ["|=", "!=", "|~", "!~"].into_iter().find(|o| p.peek(o));
        if let Some(op) = op {
            p.eat(op);
            let mut values = vec![p.string().map_err(|e| format!("parse error : {e}"))?];
            while p.eat("or") {
                values.push(p.string().map_err(|e| format!("parse error : {e}"))?);
            }
            for v in &values {
                if op.ends_with('~') && Regex::new(v).is_err() {
                    return Err(format!("parse error : invalid regex {v:?}"));
                }
            }
            stages.push(Stage::Line {
                op: op.to_owned(),
                values,
            });
            continue;
        }
        if !p.eat("|") {
            break;
        }
        let save = p.i;
        let kw = p.ident().unwrap_or_default();
        match kw.as_str() {
            "json" => {
                while !p.peek("|") && !p.peek(")") && !p.peek("[") && p.i < p.s.len() {
                    p.word();
                }
                stages.push(Stage::Json);
            }
            "logfmt" => {
                while p.eat("--") {
                    p.word();
                }
                stages.push(Stage::Logfmt);
            }
            "regexp" => {
                let re = p.string()?;
                stages.push(Stage::Regexp(
                    Regex::new(&re).map_err(|e| format!("parse error : invalid regexp: {e}"))?,
                ));
            }
            "pattern" | "line_format" | "label_format" | "unpack" | "decolorize" | "drop"
            | "keep" | "unwrap" => {
                if p.peek("\"") || p.peek("`") {
                    p.string()?;
                } else {
                    while !p.peek("|") && !p.peek(")") && !p.peek("[") && p.i < p.s.len() {
                        p.word();
                    }
                }
                stages.push(Stage::Ignore);
            }
            "" => return Err("parse error : syntax error: unexpected |".into()),
            name => {
                let op = ["==", "=~", "!~", "!=", ">=", "<=", ">", "<", "="]
                    .into_iter()
                    .find(|o| p.eat(o));
                let Some(op) = op else {
                    p.i = save;
                    return Err(format!(
                        "parse error : syntax error: unexpected IDENTIFIER {name}"
                    ));
                };
                let value = if p.peek("\"") || p.peek("`") {
                    p.string()?
                } else {
                    p.word()
                };
                stages.push(Stage::Label {
                    name: name.to_owned(),
                    op: op.to_owned(),
                    value,
                });
            }
        }
    }
    Ok(LogQuery { matchers, stages })
}

const RANGE_FUNCS: &[&str] = &[
    "count_over_time",
    "rate",
    "bytes_over_time",
    "bytes_rate",
    "absent_over_time",
];
const AGGS: &[&str] = &["sum", "count", "avg", "min", "max", "topk", "bottomk"];

pub fn parse(src: &str) -> Result<Query, String> {
    let mut p = P {
        s: src.trim().as_bytes(),
        i: 0,
    };
    p.ws();
    if p.peek("{") {
        let q = log_query(&mut p)?;
        p.ws();
        if p.i < p.s.len() {
            return Err(format!(
                "parse error at line 1, col {}: syntax error: unexpected input",
                p.i + 1
            ));
        }
        return Ok(Query::Log(q));
    }
    let name = p
        .ident()
        .ok_or("parse error : syntax error: unexpected input")?;
    let mut agg = None;
    let func;
    if AGGS.contains(&name.as_str()) {
        let mut by = None;
        if let Some(kw) = {
            let save = p.i;
            let id = p.ident();
            if matches!(id.as_deref(), Some("by" | "without")) {
                id
            } else {
                p.i = save;
                None
            }
        } {
            by = Some((kw == "by", labels(&mut p)?));
        }
        if !p.eat("(") {
            return Err("parse error : syntax error: expecting (".into());
        }
        let mut k = None;
        if name == "topk" || name == "bottomk" {
            k = Some(
                p.word()
                    .trim_end_matches(',')
                    .parse::<usize>()
                    .map_err(|_| "parse error : invalid topk parameter")?,
            );
            p.eat(",");
        }
        func = p
            .ident()
            .ok_or("parse error : syntax error: expecting range aggregation")?;
        let inner = range_agg(&mut p, &func)?;
        if !p.eat(")") {
            return Err("parse error : syntax error: expecting )".into());
        }
        if let Some(kw) = {
            let save = p.i;
            let id = p.ident();
            if matches!(id.as_deref(), Some("by" | "without")) {
                id
            } else {
                p.i = save;
                None
            }
        } {
            by = Some((kw == "by", labels(&mut p)?));
        }
        agg = Some((name, k, by));
        return Ok(Query::Metric(MetricQuery {
            func,
            range: inner.1,
            log: inner.0,
            agg,
        }));
    }
    func = name;
    let (log, range) = range_agg(&mut p, &func)?;
    Ok(Query::Metric(MetricQuery {
        func,
        range,
        log,
        agg,
    }))
}

fn labels(p: &mut P) -> Result<Vec<String>, String> {
    if !p.eat("(") {
        return Err("parse error : expecting (".into());
    }
    let mut out = Vec::new();
    loop {
        if p.eat(")") {
            break;
        }
        p.eat(",");
        out.push(p.ident().ok_or("parse error : bad grouping")?);
    }
    Ok(out)
}

fn range_agg(p: &mut P, func: &str) -> Result<(LogQuery, f64), String> {
    if !RANGE_FUNCS.contains(&func) {
        return Err(format!(
            "parse error : syntax error: unexpected IDENTIFIER {func}"
        ));
    }
    if !p.eat("(") {
        return Err("parse error : syntax error: expecting (".into());
    }
    let q = log_query(p)?;
    if !p.eat("[") {
        return Err("parse error : syntax error: unexpected ), expecting [".into());
    }
    let d = p.word();
    let d = d.trim_end_matches(']');
    let range =
        duration(d).ok_or_else(|| format!("parse error : not a valid duration string: {d:?}"))?;
    p.eat("]");
    if !p.eat(")") {
        return Err("parse error : syntax error: expecting )".into());
    }
    Ok((q, range))
}

fn matcher_ok(op: &str, actual: Option<&str>, value: &str) -> bool {
    let a = actual.unwrap_or("");
    match op {
        "=" => a == value,
        "!=" => a != value,
        "=~" => Regex::new(&format!("^(?:{value})$")).is_ok_and(|r| r.is_match(a)),
        "!~" => Regex::new(&format!("^(?:{value})$")).is_ok_and(|r| !r.is_match(a)),
        _ => false,
    }
}

fn parse_logfmt(line: &str, out: &mut Labels) {
    let mut rest = line.trim();
    while !rest.is_empty() {
        let key_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = &rest[..key_end];
        rest = &rest[key_end..];
        if let Some(r) = rest.strip_prefix('=') {
            let (val, remaining) = if let Some(r2) = r.strip_prefix('"') {
                let end = r2.find('"').unwrap_or(r2.len());
                (r2[..end].to_owned(), r2.get(end + 1..).unwrap_or(""))
            } else {
                let end = r.find(char::is_whitespace).unwrap_or(r.len());
                (r[..end].to_owned(), &r[end..])
            };
            if !key.is_empty() {
                out.insert(key.to_owned(), val);
            }
            rest = remaining.trim_start();
        } else {
            rest = rest.trim_start();
        }
    }
}

fn flatten_json(prefix: &str, v: &serde_json::Value, out: &mut Labels) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}_{k}")
                };
                flatten_json(&key, x, out);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_owned(), s.clone());
        }
        serde_json::Value::Null => {}
        other => {
            out.insert(prefix.to_owned(), other.to_string());
        }
    }
}

impl LogQuery {
    pub fn selects(&self, labels: &Labels) -> bool {
        self.matchers
            .iter()
            .all(|(n, op, v)| matcher_ok(op, labels.get(n).map(String::as_str), v))
    }

    /// Applies the pipeline; returns the entry's labels (stream + extracted) when it passes.
    pub fn pipeline(&self, stream: &Labels, line: &str) -> Option<Labels> {
        let mut labels = stream.clone();
        for st in &self.stages {
            match st {
                Stage::Line { op, values } => {
                    let hit = values.iter().any(|v| match op.as_str() {
                        "|=" | "!=" => line.contains(v.as_str()),
                        _ => Regex::new(v).is_ok_and(|r| r.is_match(line)),
                    });
                    let want = op.starts_with('|');
                    if hit != want {
                        return None;
                    }
                }
                Stage::Json => match serde_json::from_str::<serde_json::Value>(line) {
                    Ok(v) => flatten_json("", &v, &mut labels),
                    Err(_) => {
                        labels.insert("__error__".into(), "JSONParserErr".into());
                    }
                },
                Stage::Logfmt => parse_logfmt(line, &mut labels),
                Stage::Regexp(re) => {
                    if let Some(c) = re.captures(line) {
                        for name in re.capture_names().flatten() {
                            if let Some(m) = c.name(name) {
                                labels.insert(name.to_owned(), m.as_str().to_owned());
                            }
                        }
                    }
                }
                Stage::Label { name, op, value } => {
                    let actual = labels.get(name).map(String::as_str);
                    let ok = match op.as_str() {
                        "=" | "==" | "!=" | "=~" | "!~"
                            if value.parse::<f64>().is_err() || op.contains('~') =>
                        {
                            matcher_ok(if op == "==" { "=" } else { op }, actual, value)
                        }
                        _ => {
                            let a =
                                actual.and_then(|a| a.parse::<f64>().ok().or_else(|| duration(a)));
                            let b = value.parse::<f64>().ok().or_else(|| duration(value));
                            match (a, b) {
                                (Some(a), Some(b)) => match op.as_str() {
                                    ">" => a > b,
                                    ">=" => a >= b,
                                    "<" => a < b,
                                    "<=" => a <= b,
                                    "!=" => a != b,
                                    _ => a == b,
                                },
                                _ => false,
                            }
                        }
                    };
                    if !ok {
                        return None;
                    }
                }
                Stage::Ignore => {}
            }
        }
        Some(labels)
    }

    pub fn has_parser(&self) -> bool {
        self.stages
            .iter()
            .any(|s| matches!(s, Stage::Json | Stage::Logfmt | Stage::Regexp(_)))
    }
}

/// Instant metric value at `t` for each output series.
pub fn eval_metric(q: &MetricQuery, streams: &[Stream], t: f64) -> Vec<(Labels, f64)> {
    let mut series: Vec<(Labels, f64)> = Vec::new();
    for s in streams.iter().filter(|s| q.log.selects(&s.labels)) {
        for e in &s.entries {
            if e.ts <= t - q.range || e.ts > t {
                continue;
            }
            let Some(mut labels) = q.log.pipeline(&s.labels, &e.line) else {
                continue;
            };
            if !q.log.has_parser() {
                labels = s.labels.clone();
            }
            let w = match q.func.as_str() {
                "bytes_over_time" | "bytes_rate" => e.line.len() as f64,
                _ => 1.0,
            };
            match series.iter_mut().find(|(l, _)| *l == labels) {
                Some((_, v)) => *v += w,
                None => series.push((labels, w)),
            }
        }
    }
    if matches!(q.func.as_str(), "rate" | "bytes_rate") {
        for (_, v) in &mut series {
            *v /= q.range;
        }
    }
    let Some((op, k, by)) = &q.agg else {
        return series;
    };
    let mut groups: Vec<(Labels, Vec<f64>)> = Vec::new();
    let mut members: Vec<(Labels, Labels, f64)> = Vec::new();
    for (l, v) in series {
        let key: Labels = match by {
            None => Labels::new(),
            Some((true, names)) => l
                .iter()
                .filter(|(k, _)| names.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Some((false, names)) => l
                .iter()
                .filter(|(k, _)| !names.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        members.push((key.clone(), l, v));
        match groups.iter_mut().find(|(g, _)| *g == key) {
            Some((_, xs)) => xs.push(v),
            None => groups.push((key, vec![v])),
        }
    }
    if op == "topk" || op == "bottomk" {
        let kk = k.unwrap_or(1);
        let mut out = Vec::new();
        for (g, _) in &groups {
            let mut m: Vec<(Labels, f64)> = members
                .iter()
                .filter(|(k2, _, _)| k2 == g)
                .map(|(_, l, v)| (l.clone(), *v))
                .collect();
            m.sort_by(|a, b| {
                if op == "topk" {
                    b.1.total_cmp(&a.1)
                } else {
                    a.1.total_cmp(&b.1)
                }
            });
            out.extend(m.into_iter().take(kk));
        }
        return out;
    }
    groups
        .into_iter()
        .map(|(g, xs)| {
            let n = xs.len() as f64;
            let v = match op.as_str() {
                "sum" => xs.iter().sum(),
                "count" => n,
                "avg" => xs.iter().sum::<f64>() / n,
                "min" => xs.iter().cloned().fold(f64::INFINITY, f64::min),
                _ => xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            };
            (g, v)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams() -> Vec<Stream> {
        let l = |app: &str| {
            [
                ("app".to_string(), app.to_string()),
                ("env".to_string(), "prod".to_string()),
            ]
            .into_iter()
            .collect()
        };
        vec![
            Stream {
                labels: l("checkout"),
                entries: vec![
                    Entry {
                        ts: 100.0,
                        line: "level=error msg=\"payment declined\" code=502".into(),
                    },
                    Entry {
                        ts: 110.0,
                        line: "level=info msg=ok".into(),
                    },
                ],
            },
            Stream {
                labels: l("cart"),
                entries: vec![Entry {
                    ts: 105.0,
                    line: "{\"level\":\"error\",\"msg\":\"timeout\"}".into(),
                }],
            },
        ]
    }

    #[test]
    fn log_queries_filter_and_parse() {
        let Query::Log(q) = parse("{env=\"prod\"} |= \"error\"").unwrap() else {
            panic!()
        };
        let s = streams();
        let hits: usize = s
            .iter()
            .filter(|x| q.selects(&x.labels))
            .map(|x| {
                x.entries
                    .iter()
                    .filter(|e| q.pipeline(&x.labels, &e.line).is_some())
                    .count()
            })
            .sum();
        assert_eq!(hits, 2);
        let Query::Log(q) = parse("{app=\"checkout\"} | logfmt | code >= 500").unwrap() else {
            panic!()
        };
        assert!(q.pipeline(&s[0].labels, &s[0].entries[0].line).is_some());
        assert!(q.pipeline(&s[0].labels, &s[0].entries[1].line).is_none());
        assert!(parse("{app=~\".*\"}").is_err());
        assert!(parse("{app=\"x\"").is_err());
    }

    #[test]
    fn metric_queries_count_and_group() {
        let Query::Metric(q) =
            parse("sum by (app) (count_over_time({env=\"prod\"} |= \"error\" [5m]))").unwrap()
        else {
            panic!()
        };
        let mut r = eval_metric(&q, &streams(), 120.0);
        r.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].1, 1.0);
        let Query::Metric(q) = parse("rate({app=\"checkout\"}[1m])").unwrap() else {
            panic!()
        };
        let r = eval_metric(&q, &streams(), 120.0);
        assert!((r[0].1 - 2.0 / 60.0).abs() < 1e-9);
    }
}
