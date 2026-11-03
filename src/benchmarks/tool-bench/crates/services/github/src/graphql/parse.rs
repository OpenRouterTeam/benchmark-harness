//! A small GraphQL query-language parser (operations, fragments, variables, aliases,
//! arguments, inline fragments, spreads, `@include`/`@skip`). Type information is ignored.

use std::collections::HashMap;

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Var(String),
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Enum(String),
    List(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

impl Val {
    /// Resolves variables into JSON. Enums become strings.
    pub fn eval(&self, vars: &Map<String, Value>) -> Value {
        match self {
            Val::Var(v) => vars.get(v).cloned().unwrap_or(Value::Null),
            Val::Int(i) => Value::from(*i),
            Val::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
            Val::Str(s) | Val::Enum(s) => Value::String(s.clone()),
            Val::Bool(b) => Value::Bool(*b),
            Val::Null => Value::Null,
            Val::List(l) => Value::Array(l.iter().map(|v| v.eval(vars)).collect()),
            Val::Obj(o) => {
                Value::Object(o.iter().map(|(k, v)| (k.clone(), v.eval(vars))).collect())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub name: String,
    pub args: Vec<(String, Val)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, Val)>,
    pub directives: Vec<Directive>,
    pub selections: Vec<Selection>,
    pub line: usize,
    pub column: usize,
}

impl Field {
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Field(Field),
    Inline {
        type_cond: Option<String>,
        directives: Vec<Directive>,
        selections: Vec<Selection>,
    },
    Spread {
        name: String,
        directives: Vec<Directive>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Query,
    Mutation,
    Subscription,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub kind: OpKind,
    pub name: Option<String>,
    /// Variable name → default value.
    pub var_defaults: Vec<(String, Option<Val>)>,
    pub selections: Vec<Selection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub type_cond: String,
    pub selections: Vec<Selection>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub operations: Vec<Operation>,
    pub fragments: HashMap<String, Fragment>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Punct(char),
    Spread,
    Name(String),
    Int(i64),
    Float(f64),
    Str(String),
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    line_start: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            line: 1,
            line_start: 0,
        }
    }

    fn tokens(mut self) -> Result<Vec<(Tok, usize, usize)>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_ignored();
            if self.pos >= self.src.len() {
                return Ok(out);
            }
            let (line, col) = (self.line, self.pos - self.line_start + 1);
            let c = self.src[self.pos];
            let tok = match c {
                b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b'!' | b'$' | b'=' | b'@'
                | b'|' | b'&' => {
                    self.pos += 1;
                    Tok::Punct(c as char)
                }
                b'.' => {
                    if self.src[self.pos..].starts_with(b"...") {
                        self.pos += 3;
                        Tok::Spread
                    } else {
                        return Err(format!("Parse error on \".\" ({line}:{col})"));
                    }
                }
                b'"' => self.string()?,
                b'-' | b'0'..=b'9' => self.number()?,
                c if c == b'_' || c.is_ascii_alphabetic() => {
                    let start = self.pos;
                    while self.pos < self.src.len()
                        && (self.src[self.pos] == b'_'
                            || self.src[self.pos].is_ascii_alphanumeric())
                    {
                        self.pos += 1;
                    }
                    Tok::Name(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
                }
                other => {
                    return Err(format!(
                        "Parse error on \"{}\" ({line}:{col})",
                        other as char
                    ));
                }
            };
            out.push((tok, line, col));
        }
    }

    fn skip_ignored(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b'\n' => {
                    self.pos += 1;
                    self.line += 1;
                    self.line_start = self.pos;
                }
                b' ' | b'\t' | b'\r' | b',' => self.pos += 1,
                0xEF if self.src[self.pos..].starts_with(&[0xEF, 0xBB, 0xBF]) => self.pos += 3,
                b'#' => {
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn string(&mut self) -> Result<Tok, String> {
        if self.src[self.pos..].starts_with(b"\"\"\"") {
            self.pos += 3;
            let start = self.pos;
            while self.pos < self.src.len() && !self.src[self.pos..].starts_with(b"\"\"\"") {
                if self.src[self.pos] == b'\n' {
                    self.line += 1;
                    self.line_start = self.pos + 1;
                }
                self.pos += 1;
            }
            let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
            self.pos = (self.pos + 3).min(self.src.len());
            return Ok(Tok::Str(s));
        }
        self.pos += 1;
        let mut out = Vec::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(Tok::Str(String::from_utf8_lossy(&out).into_owned()));
                }
                b'\\' => {
                    self.pos += 1;
                    let e = *self.src.get(self.pos).ok_or("Unterminated string")?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(
                                self.src.get(self.pos..self.pos + 4).ok_or("Bad escape")?,
                            )
                            .map_err(|_| "Bad escape")?;
                            let cp = u32::from_str_radix(hex, 16).map_err(|_| "Bad escape")?;
                            self.pos += 4;
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                b'\n' => return Err("Unterminated string".into()),
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
        Err("Unterminated string".into())
    }

    fn number(&mut self) -> Result<Tok, String> {
        let start = self.pos;
        if self.src[self.pos] == b'-' {
            self.pos += 1;
        }
        let mut float = false;
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' | b'+' => {
                    float = true;
                    self.pos += 1;
                }
                b'-' if float => self.pos += 1,
                _ => break,
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| "Bad number")?;
        if float {
            text.parse()
                .map(Tok::Float)
                .map_err(|_| format!("Bad number {text}"))
        } else {
            text.parse()
                .map(Tok::Int)
                .map_err(|_| format!("Bad number {text}"))
        }
    }
}

struct Parser {
    toks: Vec<(Tok, usize, usize)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.0)
    }

    fn here(&self) -> (usize, usize) {
        self.toks.get(self.pos).map_or((0, 0), |t| (t.1, t.2))
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|t| t.0.clone());
        self.pos += 1;
        t
    }

    fn err<T>(&self, expected: &str) -> Result<T, String> {
        let (l, c) = self.here();
        let found = match self.peek() {
            Some(Tok::Name(n)) => n.clone(),
            Some(Tok::Punct(p)) => p.to_string(),
            Some(Tok::Spread) => "...".into(),
            Some(Tok::Str(s)) => format!("\"{s}\""),
            Some(Tok::Int(i)) => i.to_string(),
            Some(Tok::Float(f)) => f.to_string(),
            None => "end of file".into(),
        };
        Err(format!(
            "Parse error on \"{found}\" ({l}:{c}), expected {expected}"
        ))
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(&format!("'{c}'"))
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(Tok::Name(n)) => {
                let n = n.clone();
                self.pos += 1;
                Ok(n)
            }
            _ => self.err("a name"),
        }
    }

    fn document(&mut self) -> Result<Document, String> {
        let mut doc = Document::default();
        while self.peek().is_some() {
            match self.peek() {
                Some(Tok::Punct('{')) => {
                    let selections = self.selection_set()?;
                    doc.operations.push(Operation {
                        kind: OpKind::Query,
                        name: None,
                        var_defaults: Vec::new(),
                        selections,
                    });
                }
                Some(Tok::Name(n)) if n == "fragment" => {
                    self.pos += 1;
                    let name = self.name()?;
                    let on = self.name()?;
                    if on != "on" {
                        return self.err("'on'");
                    }
                    let type_cond = self.name()?;
                    self.directives()?;
                    let selections = self.selection_set()?;
                    doc.fragments.insert(
                        name,
                        Fragment {
                            type_cond,
                            selections,
                        },
                    );
                }
                Some(Tok::Name(n)) if n == "query" || n == "mutation" || n == "subscription" => {
                    let kind = match n.as_str() {
                        "query" => OpKind::Query,
                        "mutation" => OpKind::Mutation,
                        _ => OpKind::Subscription,
                    };
                    self.pos += 1;
                    let name = match self.peek() {
                        Some(Tok::Name(_)) => Some(self.name()?),
                        _ => None,
                    };
                    let var_defaults = if self.peek() == Some(&Tok::Punct('(')) {
                        self.var_defs()?
                    } else {
                        Vec::new()
                    };
                    self.directives()?;
                    let selections = self.selection_set()?;
                    doc.operations.push(Operation {
                        kind,
                        name,
                        var_defaults,
                        selections,
                    });
                }
                _ => return self.err("a definition"),
            }
        }
        if doc.operations.is_empty() {
            return Err("No operations in query document".into());
        }
        Ok(doc)
    }

    fn var_defs(&mut self) -> Result<Vec<(String, Option<Val>)>, String> {
        self.expect('(')?;
        let mut out = Vec::new();
        while !self.eat(')') {
            self.expect('$')?;
            let name = self.name()?;
            self.expect(':')?;
            self.type_ref()?;
            let default = if self.eat('=') {
                Some(self.value(true)?)
            } else {
                None
            };
            self.directives()?;
            out.push((name, default));
        }
        Ok(out)
    }

    fn type_ref(&mut self) -> Result<(), String> {
        if self.eat('[') {
            self.type_ref()?;
            self.expect(']')?;
        } else {
            self.name()?;
        }
        self.eat('!');
        Ok(())
    }

    fn directives(&mut self) -> Result<Vec<Directive>, String> {
        let mut out = Vec::new();
        while self.eat('@') {
            let name = self.name()?;
            let args = if self.peek() == Some(&Tok::Punct('(')) {
                self.arguments()?
            } else {
                Vec::new()
            };
            out.push(Directive { name, args });
        }
        Ok(out)
    }

    fn arguments(&mut self) -> Result<Vec<(String, Val)>, String> {
        self.expect('(')?;
        let mut out = Vec::new();
        while !self.eat(')') {
            let name = self.name()?;
            self.expect(':')?;
            out.push((name, self.value(false)?));
        }
        Ok(out)
    }

    fn value(&mut self, constant: bool) -> Result<Val, String> {
        match self.next() {
            Some(Tok::Punct('$')) if !constant => Ok(Val::Var(self.name()?)),
            Some(Tok::Int(i)) => Ok(Val::Int(i)),
            Some(Tok::Float(f)) => Ok(Val::Float(f)),
            Some(Tok::Str(s)) => Ok(Val::Str(s)),
            Some(Tok::Name(n)) => Ok(match n.as_str() {
                "true" => Val::Bool(true),
                "false" => Val::Bool(false),
                "null" => Val::Null,
                _ => Val::Enum(n),
            }),
            Some(Tok::Punct('[')) => {
                let mut items = Vec::new();
                while !self.eat(']') {
                    items.push(self.value(constant)?);
                }
                Ok(Val::List(items))
            }
            Some(Tok::Punct('{')) => {
                let mut fields = Vec::new();
                while !self.eat('}') {
                    let k = self.name()?;
                    self.expect(':')?;
                    fields.push((k, self.value(constant)?));
                }
                Ok(Val::Obj(fields))
            }
            _ => {
                self.pos -= 1;
                self.err("a value")
            }
        }
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, String> {
        self.expect('{')?;
        let mut out = Vec::new();
        while !self.eat('}') {
            if self.peek().is_none() {
                return self.err("'}'");
            }
            out.push(self.selection()?);
        }
        Ok(out)
    }

    fn selection(&mut self) -> Result<Selection, String> {
        if self.peek() == Some(&Tok::Spread) {
            self.pos += 1;
            match self.peek() {
                Some(Tok::Name(n)) if n == "on" => {
                    self.pos += 1;
                    let type_cond = Some(self.name()?);
                    let directives = self.directives()?;
                    let selections = self.selection_set()?;
                    Ok(Selection::Inline {
                        type_cond,
                        directives,
                        selections,
                    })
                }
                Some(Tok::Name(_)) => {
                    let name = self.name()?;
                    let directives = self.directives()?;
                    Ok(Selection::Spread { name, directives })
                }
                _ => {
                    let directives = self.directives()?;
                    let selections = self.selection_set()?;
                    Ok(Selection::Inline {
                        type_cond: None,
                        directives,
                        selections,
                    })
                }
            }
        } else {
            let (line, column) = self.here();
            let first = self.name()?;
            let (alias, name) = if self.eat(':') {
                (Some(first), self.name()?)
            } else {
                (None, first)
            };
            let args = if self.peek() == Some(&Tok::Punct('(')) {
                self.arguments()?
            } else {
                Vec::new()
            };
            let directives = self.directives()?;
            let selections = if self.peek() == Some(&Tok::Punct('{')) {
                self.selection_set()?
            } else {
                Vec::new()
            };
            Ok(Selection::Field(Field {
                alias,
                name,
                args,
                directives,
                selections,
                line,
                column,
            }))
        }
    }
}

pub fn parse(src: &str) -> Result<Document, String> {
    let toks = Lexer::new(src).tokens()?;
    Parser { toks, pos: 0 }.document()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shurcool_style_queries() {
        let q = r#"query($after:String$first:Int!$owner:String!$repo:String!){repository(owner: $owner, name: $repo){issues(first: $first, after: $after, states: [OPEN], orderBy: {field: CREATED_AT, direction: DESC}){nodes{number,title,labels(first: 100){nodes{name}}},pageInfo{hasNextPage,endCursor},totalCount},isPrivate}}"#;
        let doc = parse(q).unwrap();
        let op = &doc.operations[0];
        assert_eq!(op.kind, OpKind::Query);
        assert_eq!(op.var_defaults.len(), 4);
        let Selection::Field(repo) = &op.selections[0] else {
            panic!()
        };
        assert_eq!(repo.name, "repository");
        assert_eq!(repo.args[0], ("owner".into(), Val::Var("owner".into())));
        let Selection::Field(issues) = &repo.selections[0] else {
            panic!()
        };
        assert_eq!(
            issues.args[3].1,
            Val::Obj(vec![
                ("field".into(), Val::Enum("CREATED_AT".into())),
                ("direction".into(), Val::Enum("DESC".into()))
            ])
        );
    }

    #[test]
    fn parses_mutations_aliases_and_fragments() {
        let q = r#"mutation($input:CloseIssueInput!){closeIssue(input: $input){issue{id,...on Issue{number}}} item1: closeIssue(input: $input){clientMutationId}}"#;
        let doc = parse(q).unwrap();
        assert_eq!(doc.operations[0].kind, OpKind::Mutation);
        let Selection::Field(f) = &doc.operations[0].selections[1] else {
            panic!()
        };
        assert_eq!(f.key(), "item1");
        let doc = parse("query Q { ...F } fragment F on Query { viewer { login } }").unwrap();
        assert!(doc.fragments.contains_key("F"));
        assert!(parse("{ viewer { login }").is_err());
    }

    #[test]
    fn strings_and_numbers() {
        let doc =
            parse(r#"{ a(s: "x\"y\n", f: -1.5e2, i: 42, l: [1 2], b: true, n: null) }"#).unwrap();
        let Selection::Field(f) = &doc.operations[0].selections[0] else {
            panic!()
        };
        let vars = Map::new();
        assert_eq!(f.args[0].1.eval(&vars), Value::String("x\"y\n".into()));
        assert_eq!(f.args[1].1.eval(&vars), serde_json::json!(-150.0));
        assert_eq!(f.args[2].1.eval(&vars), serde_json::json!(42));
        assert_eq!(f.args[3].1.eval(&vars), serde_json::json!([1, 2]));
    }
}
