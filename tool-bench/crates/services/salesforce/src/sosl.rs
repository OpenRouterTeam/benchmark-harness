//! SOSL: `FIND {term} [IN ... FIELDS] [WITH ...] [RETURNING Obj(fields [WHERE] [ORDER BY]
//! [LIMIT]), ...] [LIMIT n]`. Matching is token based like Salesforce's index: a term
//! matches whole words, `*` and `?` are wildcards, quoted phrases match word sequences, and
//! `OR` / `AND NOT` combine terms.

use serde_json::{Value, json};

use crate::{
    db::{Db, SfError},
    schema::{Object, Ty},
    soql::{self, Engine, T, Tok, lex},
};

fn malformed(message: impl Into<String>) -> SfError {
    SfError::bad("MALFORMED_SEARCH", message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    Name,
    Email,
    Phone,
}

#[derive(Debug, Clone)]
enum Term {
    Word(String),
    Phrase(Vec<String>),
}

/// OR of ANDs, each with optional negated terms.
#[derive(Debug, Clone, Default)]
struct Search {
    any: Vec<(Vec<Term>, Vec<Term>)>,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '*' || c == '?' || c == '\'' || c == '@'))
        .flat_map(|w| w.split('@'))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

fn parse_term(raw: &str) -> Result<Search, SfError> {
    let mut tokens: Vec<(bool, String)> = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('"') {
            let end = r
                .find('"')
                .ok_or_else(|| malformed("Unmatched quote in search term"))?;
            tokens.push((true, r[..end].to_owned()));
            rest = r[end + 1..].trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            tokens.push((false, rest[..end].to_owned()));
            rest = rest[end..].trim_start();
        }
    }
    let mut search = Search::default();
    let mut current: (Vec<Term>, Vec<Term>) = (Vec::new(), Vec::new());
    let mut negate = false;
    let mut i = 0;
    while i < tokens.len() {
        let (quoted, tok) = &tokens[i];
        let up = tok.to_ascii_uppercase();
        if !quoted && up == "OR" {
            search.any.push(std::mem::take(&mut current));
        } else if !quoted && up == "AND" {
            if tokens
                .get(i + 1)
                .is_some_and(|(q, t)| !q && t.eq_ignore_ascii_case("NOT"))
            {
                negate = true;
                i += 1;
            }
        } else if !quoted && up == "NOT" {
            negate = true;
        } else {
            let ws = words(tok);
            let term = if *quoted || ws.len() > 1 {
                Term::Phrase(ws)
            } else if let Some(w) = ws.into_iter().next() {
                Term::Word(w)
            } else {
                i += 1;
                continue;
            };
            if negate {
                current.1.push(term);
            } else {
                current.0.push(term);
            }
            negate = false;
        }
        i += 1;
    }
    search.any.push(current);
    search.any.retain(|(pos, _)| !pos.is_empty());
    if search.any.is_empty() {
        return Err(malformed("search term must be longer than one character"));
    }
    let shortest = search
        .any
        .iter()
        .flat_map(|(p, _)| p)
        .map(|t| match t {
            Term::Word(w) => w.trim_matches(['*', '?']).chars().count(),
            Term::Phrase(p) => p.iter().map(|w| w.chars().count()).sum(),
        })
        .min()
        .unwrap_or(0);
    if shortest < 2 {
        return Err(malformed(format!(
            "search term must be longer than one character: {}",
            raw.trim()
        )));
    }
    Ok(search)
}

fn glob(p: &[char], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| glob(&p[1..], &s[i..])),
        Some('?') => !s.is_empty() && glob(&p[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&p[1..], &s[1..]),
    }
}

fn word_match(pattern: &str, word: &str) -> bool {
    if pattern.contains(['*', '?']) {
        glob(
            &pattern.chars().collect::<Vec<_>>(),
            &word.chars().collect::<Vec<_>>(),
        )
    } else {
        pattern == word
    }
}

fn term_match(term: &Term, texts: &[String]) -> bool {
    texts.iter().any(|t| {
        let ws = words(t);
        let digits: String = t.chars().filter(char::is_ascii_digit).collect();
        match term {
            Term::Word(w) => {
                ws.iter().any(|x| word_match(w, x))
                    || t.to_lowercase() == *w
                    || (w.len() >= 4
                        && w.chars().all(|c| c.is_ascii_digit())
                        && digits.contains(w.as_str()))
            }
            Term::Phrase(p) => {
                !p.is_empty()
                    && ws
                        .windows(p.len())
                        .any(|win| win.iter().zip(p).all(|(x, y)| word_match(y, x)))
            }
        }
    })
}

fn searched_text(obj: &Object, rec: &Value, scope: Scope) -> Vec<String> {
    obj.fields
        .iter()
        .filter(|f| match scope {
            Scope::All => f.ty.is_searchable_text() || f.ty == Ty::Picklist && f.name == "Name",
            Scope::Name => {
                f.name_field || matches!(f.name.as_str(), "FirstName" | "LastName" | "Company")
            }
            Scope::Email => f.ty == Ty::Email,
            Scope::Phone => f.ty == Ty::Phone,
        })
        .filter_map(|f| {
            soql::field_value(rec, &obj.name, &f.name)
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn matches(search: &Search, texts: &[String]) -> bool {
    search.any.iter().any(|(pos, neg)| {
        pos.iter().all(|t| term_match(t, texts)) && !neg.iter().any(|t| term_match(t, texts))
    })
}

/// Splits the text after `FIND {term}` into the scope, the RETURNING specs and the limit.
struct Spec {
    object: String,
    object_pos: usize,
    /// Text inside the parentheses, if any.
    inner: Option<String>,
}

pub fn run(db: &Db, sosl: &str) -> Result<Value, SfError> {
    let trimmed = sosl.trim_start();
    let lead = sosl.len() - trimmed.len();
    if !trimmed
        .get(..4)
        .is_some_and(|f| f.eq_ignore_ascii_case("FIND"))
    {
        return Err(malformed(soql::positioned(
            sosl,
            lead,
            "unexpected token: FIND expected",
        )));
    }
    let after = &trimmed[4..];
    let after_trim = after.trim_start();
    let open_at = lead + 4 + (after.len() - after_trim.len());
    let (term, rest_start) = match after_trim.chars().next() {
        Some('{') => {
            let mut esc = false;
            let mut end = None;
            for (i, c) in after_trim.char_indices().skip(1) {
                match (esc, c) {
                    (true, _) => esc = false,
                    (false, '\\') => esc = true,
                    (false, '}') => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let end = end.ok_or_else(|| {
                malformed(soql::positioned(
                    sosl,
                    open_at,
                    "Unmatched brace in search term",
                ))
            })?;
            (after_trim[1..end].replace('\\', ""), open_at + end + 1)
        }
        Some('\'') => {
            let end = after_trim[1..]
                .find('\'')
                .ok_or_else(|| malformed("Unmatched quote"))?
                + 1;
            (after_trim[1..end].to_owned(), open_at + end + 1)
        }
        _ => {
            return Err(malformed(soql::positioned(
                sosl,
                open_at,
                "unexpected token: search term expected",
            )));
        }
    };
    let search = parse_term(&term)?;
    let rest = &sosl[rest_start..];
    let toks: Vec<Tok> = lex(rest).map_err(|e| malformed(e.message))?;
    let tok_text = |t: &Tok| match &t.t {
        T::Ident(s) => s.clone(),
        T::Eof => "<EOF>".to_owned(),
        other => format!("{other:?}"),
    };
    let unexpected = |t: &Tok| {
        malformed(soql::positioned(
            sosl,
            rest_start + t.pos,
            &format!("unexpected token: '{}'", tok_text(t)),
        ))
    };
    let is = |t: &Tok, kw: &str| matches!(&t.t, T::Ident(s) if s.eq_ignore_ascii_case(kw));
    let mut i = 0;
    let mut scope = Scope::All;
    let mut specs: Vec<Spec> = Vec::new();
    let mut limit: Option<usize> = None;
    let mut seen_returning = false;
    while toks[i].t != T::Eof {
        let t = &toks[i];
        if is(t, "IN") {
            let kind = &toks[i + 1];
            scope = if is(kind, "ALL") || is(kind, "SIDEBAR") {
                Scope::All
            } else if is(kind, "NAME") {
                Scope::Name
            } else if is(kind, "EMAIL") {
                Scope::Email
            } else if is(kind, "PHONE") {
                Scope::Phone
            } else {
                return Err(unexpected(kind));
            };
            if !is(&toks[i + 2], "FIELDS") {
                return Err(unexpected(&toks[i + 2]));
            }
            i += 3;
        } else if is(t, "WITH") {
            i += 1;
            let mut depth = 0;
            while toks[i].t != T::Eof {
                match toks[i].t {
                    T::LParen => depth += 1,
                    T::RParen => depth -= 1,
                    _ => {}
                }
                if depth == 0
                    && ["RETURNING", "LIMIT", "WITH", "UPDATE"]
                        .iter()
                        .any(|k| is(&toks[i], k))
                    && i > 0
                {
                    break;
                }
                i += 1;
            }
        } else if is(t, "RETURNING") && !seen_returning {
            seen_returning = true;
            i += 1;
            loop {
                let T::Ident(name) = &toks[i].t else {
                    return Err(unexpected(&toks[i]));
                };
                if ["LIMIT", "WITH", "RETURNING", "UPDATE"]
                    .iter()
                    .any(|k| name.eq_ignore_ascii_case(k))
                {
                    return Err(unexpected(&toks[i]));
                }
                let mut spec = Spec {
                    object: name.clone(),
                    object_pos: rest_start + toks[i].pos,
                    inner: None,
                };
                i += 1;
                if toks[i].t == T::LParen {
                    let start = toks[i].pos + 1;
                    let mut depth = 0;
                    let mut j = i;
                    loop {
                        match toks[j].t {
                            T::LParen => depth += 1,
                            T::RParen => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            T::Eof => return Err(unexpected(&toks[j])),
                            _ => {}
                        }
                        j += 1;
                    }
                    spec.inner = Some(rest[start..toks[j].pos].to_owned());
                    i = j + 1;
                }
                specs.push(spec);
                if toks[i].t == T::Comma {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if is(t, "LIMIT") {
            let T::Num(n) = &toks[i + 1].t else {
                return Err(unexpected(&toks[i + 1]));
            };
            limit = n.parse().ok();
            i += 2;
        } else if is(t, "UPDATE") && (is(&toks[i + 1], "TRACKING") || is(&toks[i + 1], "VIEWSTAT"))
        {
            i += 2;
        } else {
            return Err(unexpected(t));
        }
    }
    if !seen_returning {
        specs = db
            .schema
            .visible(false)
            .filter(|o| o.searchable && o.modelled)
            .map(|o| Spec {
                object: o.name.clone(),
                object_pos: 0,
                inner: None,
            })
            .collect();
    }
    let engine = Engine {
        db,
        src: sosl,
        all_rows: false,
        restrict: None,
    };
    let mut records = Vec::new();
    for spec in &specs {
        let object = engine
            .object(&spec.object, spec.object_pos)
            .map_err(|e| SfError::bad("INVALID_TYPE", e.message))?;
        let inner = spec.inner.clone().unwrap_or_default();
        let soql_text = build_soql(&object.name, &inner);
        let parsed = soql::parse(&soql_text).map_err(|e| SfError::bad(&e.code, e.message))?;
        let ids: Vec<String> = db
            .rows(&object.name)
            .into_iter()
            .filter(|r| r.get("IsDeleted") != Some(&Value::Bool(true)))
            .filter(|r| matches(&search, &searched_text(&object, r, scope)))
            .filter_map(|r| r["Id"].as_str().map(str::to_owned))
            .collect();
        let engine = Engine {
            db,
            src: &soql_text,
            all_rows: false,
            restrict: Some((object.name.as_str(), ids.as_slice())),
        };
        let compiled = engine.compile(&parsed, None)?;
        records.extend(engine.run(&compiled)?.records);
    }
    records.truncate(limit.unwrap_or(2000));
    Ok(json!({ "searchRecords": records }))
}

/// `Name, Industry WHERE x ORDER BY y LIMIT 5` -> `SELECT Name, Industry FROM Obj WHERE ...`.
fn build_soql(object: &str, inner: &str) -> String {
    let toks = lex(inner).unwrap_or_default();
    let mut depth = 0;
    let mut split = inner.len();
    for t in &toks {
        match &t.t {
            T::LParen => depth += 1,
            T::RParen => depth -= 1,
            T::Ident(s)
                if depth == 0
                    && ["WHERE", "ORDER", "LIMIT", "OFFSET", "USING", "WITH"]
                        .iter()
                        .any(|k| s.eq_ignore_ascii_case(k)) =>
            {
                split = t.pos;
                break;
            }
            _ => {}
        }
    }
    let fields = inner[..split].trim();
    let fields = if fields.is_empty() { "Id" } else { fields };
    format!("SELECT {fields} FROM {object} {}", inner[split..].trim())
        .trim_end()
        .to_owned()
}
