//! Query DSL evaluation with deterministic BM25-style scoring.
//!
//! Supported: `match_all`, `match_none`, `match`, `match_phrase`, `match_phrase_prefix`,
//! `multi_match`, `query_string`, `simple_query_string`, `term`, `terms`, `range`,
//! `exists`, `prefix`, `wildcard`, `regexp`, `ids`, `bool`, `constant_score`, `dis_max`,
//! `boosting`. Anything else is a `parsing_exception` like the real server's.

use std::{cell::RefCell, cmp::Ordering, collections::HashMap};

use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    error::EsError,
    model::{
        Doc, Index, analyze, compare, field_values, is_numeric, scalar_string, type_across,
        value_eq, wildcard_match,
    },
};

pub struct Ctx<'a> {
    pub indices: Vec<&'a Index>,
    pub corpus: Vec<&'a Doc>,
    pub now_ms: i64,
    df: RefCell<HashMap<(String, String), usize>>,
}

impl<'a> Ctx<'a> {
    pub fn new(indices: Vec<&'a Index>, now_ms: i64) -> Self {
        let corpus = indices.iter().flat_map(|i| i.docs.iter()).collect();
        Self {
            indices,
            corpus,
            now_ms,
            df: RefCell::new(HashMap::new()),
        }
    }

    pub fn ty(&self, field: &str) -> Option<String> {
        type_across(&self.indices, field)
    }

    fn idf(&self, field: &str, token: &str) -> f64 {
        let key = (field.to_owned(), token.to_owned());
        let df = *self.df.borrow_mut().entry(key).or_insert_with(|| {
            self.corpus
                .iter()
                .filter(|d| {
                    field_values(&d.source, field)
                        .iter()
                        .any(|v| analyze(&scalar_string(v)).iter().any(|t| t == token))
                })
                .count()
        });
        let n = self.corpus.len() as f64;
        let df = df as f64;
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }
}

/// `Some(score)` when `doc` matches `query`.
pub type Hit = Option<f64>;

fn single_field<'v>(kind: &str, body: &'v Value) -> Result<(&'v str, &'v Value), EsError> {
    let obj = body.as_object().ok_or_else(|| {
        EsError::parsing(format!(
            "[{kind}] query malformed, no start_object after query name"
        ))
    })?;
    let mut fields = obj.iter().filter(|(k, _)| *k != "boost" && *k != "_name");
    let (field, spec) = fields
        .next()
        .ok_or_else(|| EsError::parsing(format!("[{kind}] query malformed, no field specified")))?;
    if let Some((other, _)) = fields.next() {
        return Err(EsError::parsing(format!(
            "[{kind}] query doesn't support multiple fields, found [{field}] and [{other}]"
        )));
    }
    Ok((field.as_str(), spec))
}

fn boost_of(spec: &Value) -> f64 {
    spec.get("boost").and_then(Value::as_f64).unwrap_or(1.0)
}

/// `value` from `{field: value}` or `{field: {value|query: ...}}`.
fn inner<'v>(spec: &'v Value, key: &str) -> &'v Value {
    match spec {
        Value::Object(m) => m.get(key).unwrap_or(&Value::Null),
        other => other,
    }
}

fn clauses(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other],
    }
}

/// Evaluates every clause (no short-circuiting), so running it once against an empty
/// document validates the whole query.
pub fn eval(q: &Value, doc: &Doc, ctx: &Ctx) -> Result<Hit, EsError> {
    let obj = q
        .as_object()
        .ok_or_else(|| EsError::parsing("query malformed, must start with start_object"))?;
    if obj.len() != 1 {
        let keys: Vec<&String> = obj.keys().collect();
        return Err(EsError::parsing(match keys.get(1) {
            Some(k) => format!(
                "[{}] malformed query, expected [END_OBJECT] but found [FIELD_NAME] ({k})",
                keys[0]
            ),
            None => "query malformed, empty clause found".to_owned(),
        }));
    }
    let (kind, body) = obj.iter().next().expect("one key");
    match kind.as_str() {
        "match_all" => Ok(Some(boost_of(body))),
        "match_none" => Ok(None),
        "match" | "match_phrase" | "match_phrase_prefix" | "match_bool_prefix" => {
            let (field, spec) = single_field(kind, body)?;
            let query = inner(spec, "query");
            let operator = spec
                .get("operator")
                .and_then(Value::as_str)
                .unwrap_or("or")
                .to_lowercase();
            let msm = spec.get("minimum_should_match");
            let mode = match kind.as_str() {
                "match_phrase" => Mode::Phrase,
                "match_phrase_prefix" | "match_bool_prefix" => Mode::PhrasePrefix,
                _ => Mode::Match {
                    and: operator == "and",
                    msm: msm.cloned(),
                },
            };
            Ok(match_field(field, query, &mode, doc, ctx).map(|s| s * boost_of(spec)))
        }
        "multi_match" => {
            let query = body.get("query").unwrap_or(&Value::Null);
            let and = body
                .get("operator")
                .and_then(Value::as_str)
                .is_some_and(|o| o.eq_ignore_ascii_case("and"));
            let mode = match body.get("type").and_then(Value::as_str) {
                Some("phrase") => Mode::Phrase,
                Some("phrase_prefix") | Some("bool_prefix") => Mode::PhrasePrefix,
                _ => Mode::Match {
                    and,
                    msm: body.get("minimum_should_match").cloned(),
                },
            };
            let fields = expand_fields(body.get("fields"), ctx);
            let mut best: Hit = None;
            for (field, boost) in fields {
                if let Some(s) = match_field(&field, query, &mode, doc, ctx) {
                    best = Some(best.map_or(s * boost, |b: f64| b.max(s * boost)));
                }
            }
            Ok(best.map(|s| s * boost_of(body)))
        }
        "query_string" | "simple_query_string" => {
            let text = body.get("query").and_then(Value::as_str).ok_or_else(|| {
                EsError::parsing(format!("[{kind}] must be provided with a [query]"))
            })?;
            let default_and = body
                .get("default_operator")
                .and_then(Value::as_str)
                .is_some_and(|o| o.eq_ignore_ascii_case("and"));
            let mut fields: Vec<String> = body
                .get("fields")
                .and_then(Value::as_array)
                .map(|f| {
                    f.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(df) = body.get("default_field").and_then(Value::as_str) {
                fields.push(df.to_owned());
            }
            let translated =
                query_string(text, &fields, default_and, kind == "simple_query_string")?;
            eval(&translated, doc, ctx)
        }
        "term" => {
            let (field, spec) = single_field(kind, body)?;
            let value = inner(spec, "value");
            let ci = spec
                .get("case_insensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(term_match(field, value, ci, doc, ctx)
                .then(|| ctx_score(field, value, ctx) * boost_of(spec)))
        }
        "terms" => {
            let obj = body
                .as_object()
                .ok_or_else(|| EsError::parsing("[terms] query malformed"))?;
            let (field, values) = obj
                .iter()
                .find(|(k, _)| *k != "boost" && *k != "_name")
                .ok_or_else(|| EsError::parsing("[terms] query requires a field"))?;
            let values = values.as_array().ok_or_else(|| {
                EsError::parsing(format!("[terms] query does not support [{field}]"))
            })?;
            let hit = values.iter().any(|v| term_match(field, v, false, doc, ctx));
            Ok(hit.then(|| boost_of(body)))
        }
        "range" => {
            let (field, spec) = single_field(kind, body)?;
            let spec_obj = spec.as_object().ok_or_else(|| {
                EsError::parsing("[range] query malformed, no start_object after query name")
            })?;
            let ty = ctx.ty(field);
            let values = field_values(&doc.source, field);
            let mut ok = !values.is_empty();
            for (op, bound) in spec_obj {
                let (want, round_up): (&[Ordering], bool) = match op.as_str() {
                    "gt" => (&[Ordering::Greater], true),
                    "gte" | "from" => (&[Ordering::Greater, Ordering::Equal], false),
                    "lt" => (&[Ordering::Less], false),
                    "lte" | "to" => (&[Ordering::Less, Ordering::Equal], true),
                    "format" | "time_zone" | "boost" | "relation" | "include_lower"
                    | "include_upper" | "_name" => {
                        continue;
                    }
                    other => {
                        return Err(EsError::parsing(format!(
                            "[range] query does not support [{other}]"
                        )));
                    }
                };
                if bound.is_null() {
                    continue;
                }
                if ty.as_deref() == Some("date")
                    && let Value::String(s) = bound
                    && crate::model::date_math(s, ctx.now_ms, round_up).is_none()
                {
                    return Err(EsError::new(
                        axum::http::StatusCode::BAD_REQUEST,
                        "parse_exception",
                        format!(
                            "failed to parse date field [{s}] with format [strict_date_optional_time||epoch_millis]"
                        ),
                    ));
                }
                ok = ok
                    && values.iter().any(|v| {
                        compare(v, bound, ty.as_deref(), ctx.now_ms, round_up)
                            .is_some_and(|o| want.contains(&o))
                    });
            }
            Ok(ok.then(|| boost_of(spec)))
        }
        "exists" => {
            let field = body
                .get("field")
                .and_then(Value::as_str)
                .ok_or_else(|| EsError::parsing("[exists] must be provided with a [field]"))?;
            Ok((!field_values(&doc.source, field).is_empty()).then_some(1.0))
        }
        "prefix" | "wildcard" | "regexp" => {
            let (field, spec) = single_field(kind, body)?;
            let pattern = scalar_string(match spec {
                Value::Object(m) => m
                    .get("value")
                    .or_else(|| m.get("wildcard"))
                    .unwrap_or(&Value::Null),
                other => other,
            });
            let ci = spec
                .get("case_insensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let re = if kind == "regexp" {
                Some(Regex::new(&format!("^(?:{pattern})$")).map_err(|e| {
                    EsError::illegal(format!("failed to parse regexp [{pattern}]: {e}"))
                })?)
            } else {
                None
            };
            let text_field = ctx.ty(field).as_deref() == Some("text");
            let test = |s: &str| {
                let (s, p) = if ci {
                    (s.to_lowercase(), pattern.to_lowercase())
                } else {
                    (s.to_owned(), pattern.clone())
                };
                match kind.as_str() {
                    "prefix" => s.starts_with(&p),
                    "wildcard" => wildcard_match(&p, &s),
                    _ => re.as_ref().is_some_and(|r| r.is_match(&s)),
                }
            };
            let hit = field_values(&doc.source, field).iter().any(|v| {
                let s = scalar_string(v);
                if text_field {
                    analyze(&s).iter().any(|t| test(t))
                } else {
                    test(&s)
                }
            });
            Ok(hit.then(|| boost_of(spec)))
        }
        "ids" => {
            let values: Vec<String> = body
                .get("values")
                .and_then(Value::as_array)
                .map(|v| v.iter().map(scalar_string).collect())
                .unwrap_or_default();
            Ok(values.contains(&doc.id).then_some(1.0))
        }
        "bool" => eval_bool(body, doc, ctx),
        "constant_score" => {
            let filter = body
                .get("filter")
                .ok_or_else(|| EsError::parsing("[constant_score] requires a 'filter' element"))?;
            Ok(eval(filter, doc, ctx)?.map(|_| boost_of(body)))
        }
        "dis_max" => {
            let tie = body
                .get("tie_breaker")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let mut scores = Vec::new();
            for q in clauses(body.get("queries")) {
                if let Some(s) = eval(q, doc, ctx)? {
                    scores.push(s);
                }
            }
            if scores.is_empty() {
                return Ok(None);
            }
            let max = scores.iter().copied().fold(f64::MIN, f64::max);
            let rest: f64 = scores.iter().sum::<f64>() - max;
            Ok(Some(max + tie * rest))
        }
        "boosting" => {
            let positive = body.get("positive").ok_or_else(|| {
                EsError::parsing("[boosting] query requires 'positive' query to be set")
            })?;
            let negative = body.get("negative").ok_or_else(|| {
                EsError::parsing("[boosting] query requires 'negative' query to be set")
            })?;
            let factor = body
                .get("negative_boost")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let p = eval(positive, doc, ctx)?;
            let n = eval(negative, doc, ctx)?;
            Ok(p.map(|s| if n.is_some() { s * factor } else { s }))
        }
        other => Err(EsError::parsing(format!("unknown query [{other}]")).with(
            "caused_by",
            serde_json::json!({
                "type": "named_object_not_found_exception",
                "reason": format!("[1:{}] unknown field [{other}]", 12 + other.len()),
            }),
        )),
    }
}

fn eval_bool(body: &Value, doc: &Doc, ctx: &Ctx) -> Result<Hit, EsError> {
    let obj = body
        .as_object()
        .ok_or_else(|| EsError::parsing("[bool] query malformed"))?;
    for key in obj.keys() {
        if ![
            "must",
            "filter",
            "should",
            "must_not",
            "minimum_should_match",
            "boost",
            "_name",
            "adjust_pure_negative",
        ]
        .contains(&key.as_str())
        {
            return Err(EsError::parsing(format!(
                "[bool] query does not support [{key}]"
            )));
        }
    }
    let mut ok = true;
    let mut score = 0.0;
    let mut scoring = false;
    for q in clauses(obj.get("must")) {
        scoring = true;
        match eval(q, doc, ctx)? {
            Some(s) => score += s,
            None => ok = false,
        }
    }
    let filters = clauses(obj.get("filter"));
    for q in &filters {
        if eval(q, doc, ctx)?.is_none() {
            ok = false;
        }
    }
    for q in clauses(obj.get("must_not")) {
        if eval(q, doc, ctx)?.is_some() {
            ok = false;
        }
    }
    let should = clauses(obj.get("should"));
    let mut matched_should = 0;
    for q in &should {
        if let Some(s) = eval(q, doc, ctx)? {
            matched_should += 1;
            score += s;
            scoring = true;
        }
    }
    let required_should = match obj.get("minimum_should_match") {
        Some(v) => msm_count(v, should.len()),
        None if !should.is_empty() && clauses(obj.get("must")).is_empty() && filters.is_empty() => {
            1
        }
        None => 0,
    };
    if matched_should < required_should {
        ok = false;
    }
    if !ok {
        return Ok(None);
    }
    let base = if scoring {
        score
    } else if filters.is_empty() && should.is_empty() && clauses(obj.get("must")).is_empty() {
        1.0
    } else {
        0.0
    };
    Ok(Some(base * boost_of(body)))
}

/// `minimum_should_match` as an absolute clause count (`2`, `"75%"`, `"-1"`).
fn msm_count(v: &Value, total: usize) -> usize {
    let s = scalar_string(v);
    if let Some(p) = s.strip_suffix('%') {
        let pct: f64 = p.trim().parse().unwrap_or(0.0);
        let n = (total as f64 * pct.abs() / 100.0).floor() as usize;
        return if pct < 0.0 {
            total.saturating_sub(n)
        } else {
            n
        };
    }
    match s.trim().parse::<i64>() {
        Ok(n) if n < 0 => total.saturating_sub(n.unsigned_abs() as usize),
        Ok(n) => n as usize,
        Err(_) => 1,
    }
}

enum Mode {
    Match { and: bool, msm: Option<Value> },
    Phrase,
    PhrasePrefix,
}

fn ctx_score(field: &str, value: &Value, ctx: &Ctx) -> f64 {
    let token = scalar_string(value).to_lowercase();
    round6(ctx.idf(field, &token).max(0.1))
}

fn round6(f: f64) -> f64 {
    (f * 1e6).round() / 1e6
}

fn match_field(field: &str, query: &Value, mode: &Mode, doc: &Doc, ctx: &Ctx) -> Hit {
    let ty = ctx.ty(field);
    let values = field_values(&doc.source, field);
    if values.is_empty() {
        return None;
    }
    let is_text = match ty.as_deref() {
        Some("text") | Some("match_only_text") => true,
        None => values.iter().all(Value::is_string),
        _ => false,
    };
    if !is_text {
        // keyword, numeric, date, boolean: the query value must equal a value.
        let hit = values.iter().any(|v| value_eq(v, query, ty.as_deref()));
        return hit.then(|| ctx_score(field, query, ctx));
    }
    let q_tokens = analyze(&scalar_string(query));
    if q_tokens.is_empty() {
        return None;
    }
    let doc_tokens: Vec<Vec<String>> = values.iter().map(|v| analyze(&scalar_string(v))).collect();
    let all: Vec<&String> = doc_tokens.iter().flatten().collect();
    match mode {
        Mode::Phrase | Mode::PhrasePrefix => {
            let prefix = matches!(mode, Mode::PhrasePrefix);
            let found = doc_tokens.iter().any(|toks| {
                toks.windows(q_tokens.len()).any(|w| {
                    w.iter().zip(&q_tokens).enumerate().all(|(i, (a, b))| {
                        if prefix && i == q_tokens.len() - 1 {
                            a.starts_with(b.as_str())
                        } else {
                            a == b
                        }
                    })
                })
            });
            found.then(|| round6(q_tokens.iter().map(|t| ctx.idf(field, t)).sum::<f64>() * 1.2))
        }
        Mode::Match { and, msm } => {
            let mut matched = 0;
            let mut score = 0.0;
            for t in &q_tokens {
                let tf = all.iter().filter(|d| **d == t).count() as f64;
                if tf > 0.0 {
                    matched += 1;
                    score += ctx.idf(field, t) * (tf * 2.2) / (tf + 1.2);
                }
            }
            let needed = if *and {
                q_tokens.len()
            } else {
                msm.as_ref()
                    .map_or(1, |m| msm_count(m, q_tokens.len()).max(1))
            };
            (matched >= needed).then(|| round6(score))
        }
    }
}

fn term_match(field: &str, value: &Value, case_insensitive: bool, doc: &Doc, ctx: &Ctx) -> bool {
    let ty = ctx.ty(field);
    let values = field_values(&doc.source, field);
    match ty.as_deref() {
        Some("text") | Some("match_only_text") => {
            let needle = scalar_string(value);
            values.iter().any(|v| {
                analyze(&scalar_string(v)).iter().any(|t| {
                    if case_insensitive {
                        t.eq_ignore_ascii_case(&needle)
                    } else {
                        *t == needle
                    }
                })
            })
        }
        Some(t) if is_numeric(t) || t == "date" || t == "boolean" => {
            values.iter().any(|v| value_eq(v, value, Some(t)))
        }
        _ => values.iter().any(|v| {
            if case_insensitive {
                scalar_string(v).eq_ignore_ascii_case(&scalar_string(value))
            } else {
                value_eq(v, value, ty.as_deref())
            }
        }),
    }
}

/// `fields` of multi_match/query_string with `^boost` and wildcards; default: every
/// text and keyword field.
fn expand_fields(fields: Option<&Value>, ctx: &Ctx) -> Vec<(String, f64)> {
    let specs: Vec<String> = fields
        .and_then(Value::as_array)
        .map(|f| {
            f.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["*".to_owned()]);
    let leaves: Vec<(String, String)> = ctx.indices.iter().flat_map(|i| i.leaf_fields()).collect();
    let mut out: Vec<(String, f64)> = Vec::new();
    for spec in specs {
        let (name, boost) = match spec.split_once('^') {
            Some((n, b)) => (n.to_owned(), b.parse().unwrap_or(1.0)),
            None => (spec.clone(), 1.0),
        };
        if name.contains('*') {
            for (leaf, ty) in &leaves {
                if wildcard_match(&name, leaf)
                    && (ty == "text" || ty == "keyword" || name != "*")
                    && !out.iter().any(|(f, _)| f == leaf)
                {
                    out.push((leaf.clone(), boost));
                }
            }
        } else if !out.iter().any(|(f, _)| *f == name) {
            out.push((name, boost));
        }
    }
    out
}

// ───────────────────────────── query_string ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Phrase(String),
    And,
    Or,
    Not,
    Open,
    Close,
}

fn lex(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
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
        } else if c == '"' {
            let end = chars[i + 1..]
                .iter()
                .position(|c| *c == '"')
                .map_or(chars.len(), |p| i + 1 + p);
            out.push(Tok::Phrase(chars[i + 1..end].iter().collect()));
            i = end + 1;
        } else {
            let mut word = String::new();
            let mut in_quote = false;
            let mut in_range = false;
            while i < chars.len() {
                let c = chars[i];
                if c == '"' {
                    in_quote = !in_quote;
                } else if c == '[' || c == '{' {
                    in_range = true;
                } else if (c == ']' || c == '}') && in_range {
                    in_range = false;
                } else if !in_quote && !in_range && (c.is_whitespace() || c == ')' || c == '(') {
                    break;
                }
                word.push(c);
                i += 1;
            }
            match word.as_str() {
                "AND" | "&&" => out.push(Tok::And),
                "OR" | "||" => out.push(Tok::Or),
                "NOT" | "!" => out.push(Tok::Not),
                _ => out.push(Tok::Word(word)),
            }
        }
    }
    out
}

/// Translates Lucene query syntax into bool/match/range clauses.
fn query_string(
    text: &str,
    fields: &[String],
    default_and: bool,
    simple: bool,
) -> Result<Value, EsError> {
    let toks = lex(text);
    let mut pos = 0;
    let q = parse_or(&toks, &mut pos, fields, default_and, simple)?;
    if pos < toks.len() && !simple {
        return Err(EsError::parsing(format!("Failed to parse query [{text}]"))
            .with("caused_by", serde_json::json!({"type": "parse_exception", "reason": format!("Cannot parse '{text}': unexpected token")})));
    }
    Ok(q)
}

fn parse_or(
    toks: &[Tok],
    pos: &mut usize,
    fields: &[String],
    default_and: bool,
    simple: bool,
) -> Result<Value, EsError> {
    let mut groups = vec![parse_and(toks, pos, fields, default_and, simple)?];
    while *pos < toks.len() && toks[*pos] == Tok::Or {
        *pos += 1;
        groups.push(parse_and(toks, pos, fields, default_and, simple)?);
    }
    Ok(if groups.len() == 1 {
        groups.pop().expect("one group")
    } else {
        serde_json::json!({ "bool": { "should": groups, "minimum_should_match": 1 } })
    })
}

fn parse_and(
    toks: &[Tok],
    pos: &mut usize,
    fields: &[String],
    default_and: bool,
    simple: bool,
) -> Result<Value, EsError> {
    let mut must: Vec<Value> = Vec::new();
    let mut should: Vec<Value> = Vec::new();
    let mut must_not: Vec<Value> = Vec::new();
    let mut explicit_and = false;
    while *pos < toks.len() {
        match &toks[*pos] {
            Tok::Or | Tok::Close => break,
            Tok::And => {
                explicit_and = true;
                *pos += 1;
            }
            Tok::Not => {
                *pos += 1;
                if let Some(q) = parse_unary(toks, pos, fields, default_and, simple)? {
                    must_not.push(q);
                }
            }
            _ => {
                let (neg, req) = match &toks[*pos] {
                    Tok::Word(w) if w.starts_with('-') && w.len() > 1 => (true, false),
                    Tok::Word(w) if w.starts_with('+') && w.len() > 1 => (false, true),
                    _ => (false, false),
                };
                if let Some(q) = parse_unary(toks, pos, fields, default_and, simple)? {
                    if neg {
                        must_not.push(q);
                    } else if req || default_and || explicit_and {
                        must.push(q);
                    } else {
                        should.push(q);
                    }
                }
            }
        }
    }
    if explicit_and {
        must.append(&mut should);
    }
    if must.len() == 1 && should.is_empty() && must_not.is_empty() {
        return Ok(must.pop().expect("one clause"));
    }
    if should.len() == 1 && must.is_empty() && must_not.is_empty() {
        return Ok(should.pop().expect("one clause"));
    }
    let mut b = Map::new();
    if !must.is_empty() {
        b.insert("must".into(), Value::Array(must));
    }
    if !should.is_empty() {
        let n = should.len();
        b.insert("should".into(), Value::Array(should));
        if b.get("must").is_none() {
            b.insert(
                "minimum_should_match".into(),
                serde_json::json!(if n > 0 { 1 } else { 0 }),
            );
        }
    }
    if !must_not.is_empty() {
        b.insert("must_not".into(), Value::Array(must_not));
    }
    if b.is_empty() {
        return Ok(serde_json::json!({ "match_all": {} }));
    }
    Ok(serde_json::json!({ "bool": b }))
}

fn parse_unary(
    toks: &[Tok],
    pos: &mut usize,
    fields: &[String],
    default_and: bool,
    simple: bool,
) -> Result<Option<Value>, EsError> {
    let Some(tok) = toks.get(*pos) else {
        return Ok(None);
    };
    *pos += 1;
    match tok {
        Tok::Open => {
            let q = parse_or(toks, pos, fields, default_and, simple)?;
            if toks.get(*pos) == Some(&Tok::Close) {
                *pos += 1;
            }
            Ok(Some(q))
        }
        Tok::Phrase(p) => Ok(Some(on_fields(
            fields,
            |f| serde_json::json!({ "match_phrase": { f: p } }),
        ))),
        Tok::Word(w) => {
            let w = w.trim_start_matches(['+', '-']);
            if !simple && let Some((field, value)) = split_field(w) {
                return Ok(Some(field_clause(field, value)));
            }
            Ok(Some(free_term(w, fields)))
        }
        _ => Ok(None),
    }
}

fn split_field(w: &str) -> Option<(&str, &str)> {
    let (f, v) = w.split_once(':')?;
    (!f.is_empty() && !v.is_empty() && !f.contains('/')).then_some((f, v))
}

fn on_fields(fields: &[String], make: impl Fn(&str) -> Value) -> Value {
    if fields.is_empty() {
        return make("*");
    }
    if fields.len() == 1 {
        return make(&fields[0]);
    }
    let qs: Vec<Value> = fields.iter().map(|f| make(f)).collect();
    serde_json::json!({ "dis_max": { "queries": qs } })
}

fn free_term(w: &str, fields: &[String]) -> Value {
    if w.contains('*') || w.contains('?') {
        return on_fields(
            fields,
            |f| serde_json::json!({ "wildcard": { f: { "value": w.to_lowercase(), "case_insensitive": true } } }),
        );
    }
    if fields.is_empty() {
        return serde_json::json!({ "multi_match": { "query": w } });
    }
    on_fields(fields, |f| serde_json::json!({ "match": { f: w } }))
}

fn field_clause(field: &str, value: &str) -> Value {
    let value = value.trim_matches('"');
    if let Some(range) = value.strip_prefix(['[', '{']) {
        let inclusive_lo = value.starts_with('[');
        let inclusive_hi = range.ends_with(']');
        let inner = range.trim_end_matches([']', '}']);
        let (lo, hi) = inner.split_once(" TO ").unwrap_or((inner, "*"));
        let mut spec = Map::new();
        if lo.trim() != "*" {
            spec.insert(
                if inclusive_lo { "gte" } else { "gt" }.into(),
                Value::String(lo.trim().to_owned()),
            );
        }
        if hi.trim() != "*" {
            spec.insert(
                if inclusive_hi { "lte" } else { "lt" }.into(),
                Value::String(hi.trim().to_owned()),
            );
        }
        return serde_json::json!({ "range": { field: spec } });
    }
    for (prefix, op) in [(">=", "gte"), ("<=", "lte"), (">", "gt"), ("<", "lt")] {
        if let Some(v) = value.strip_prefix(prefix) {
            return serde_json::json!({ "range": { field: { op: v } } });
        }
    }
    if value == "*" {
        return serde_json::json!({ "exists": { "field": field } });
    }
    if value.contains('*') || value.contains('?') {
        return serde_json::json!({ "wildcard": { field: { "value": value, "case_insensitive": true } } });
    }
    serde_json::json!({ "match": { field: value } })
}
