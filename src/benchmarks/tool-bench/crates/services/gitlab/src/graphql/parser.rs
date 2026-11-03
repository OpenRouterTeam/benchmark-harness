//! A compact GraphQL document parser: operations, variables, fields with aliases and
//! arguments, inline fragments, named fragments and `@include`/`@skip` directives.

use std::collections::HashMap;

use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub enum Ast {
    Var(String),
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Enum(String),
    List(Vec<Ast>),
    Object(Vec<(String, Ast)>),
}

#[derive(Debug, Clone)]
pub struct Directive {
    pub name: String,
    pub args: Vec<(String, Ast)>,
}

#[derive(Debug, Clone)]
pub enum Selection {
    Field(Field),
    Inline {
        type_cond: Option<String>,
        directives: Vec<Directive>,
        selection: Vec<Selection>,
    },
    Spread {
        name: String,
        directives: Vec<Directive>,
    },
}

#[derive(Debug, Clone)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, Ast)>,
    pub directives: Vec<Directive>,
    pub selection: Vec<Selection>,
}

impl Field {
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub kind: String,
    pub name: Option<String>,
    pub defaults: Vec<(String, Option<Ast>)>,
    pub selection: Vec<Selection>,
}

#[derive(Debug, Clone)]
pub struct Fragment {
    pub type_cond: String,
    pub selection: Vec<Selection>,
}

#[derive(Debug, Clone, Default)]
pub struct Document {
    pub operations: Vec<Operation>,
    pub fragments: HashMap<String, Fragment>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Str(String),
    Int(i64),
    Float(f64),
    Punct(char),
    Spread,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == ',' || c == '\u{feff}' {
            i += 1;
        } else if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '.' {
            if chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
                out.push(Tok::Spread);
                i += 3;
            } else {
                return Err(format!(
                    "Syntax Error: Unexpected character: \".\" at position {i}"
                ));
            }
        } else if "{}()[]:!$@=|&".contains(c) {
            out.push(Tok::Punct(c));
            i += 1;
        } else if c == '"' {
            if chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"') {
                i += 3;
                let mut s = String::new();
                while i < chars.len()
                    && !(chars[i] == '"'
                        && chars.get(i + 1) == Some(&'"')
                        && chars.get(i + 2) == Some(&'"'))
                {
                    s.push(chars[i]);
                    i += 1;
                }
                i += 3;
                out.push(Tok::Str(s.trim().to_owned()));
            } else {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        match chars[i] {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            'u' => {
                                let hex: String =
                                    chars[i + 1..(i + 5).min(chars.len())].iter().collect();
                                if let Some(ch) =
                                    u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                                {
                                    s.push(ch);
                                }
                                i += 4;
                            }
                            other => s.push(other),
                        }
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("Syntax Error: Unterminated string.".into());
                }
                i += 1;
                out.push(Tok::Str(s));
            }
        } else if c == '-' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            let mut float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || ".eE+-".contains(chars[i])) {
                if ".eE".contains(chars[i]) {
                    float = true;
                }
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if float {
                out.push(Tok::Float(
                    text.parse()
                        .map_err(|_| format!("Syntax Error: Invalid number, {text}"))?,
                ));
            } else {
                out.push(Tok::Int(
                    text.parse()
                        .map_err(|_| format!("Syntax Error: Invalid number, {text}"))?,
                ));
            }
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
        } else {
            return Err(format!("Syntax Error: Unexpected character: \"{c}\"."));
        }
    }
    Ok(out)
}

struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn is(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Punct(c))
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        match self.next() {
            Some(Tok::Punct(x)) if x == c => Ok(()),
            other => Err(format!(
                "Syntax Error: Expected \"{c}\", found {}.",
                describe(other.as_ref())
            )),
        }
    }

    fn name(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Name(n)) => Ok(n),
            other => Err(format!(
                "Syntax Error: Expected Name, found {}.",
                describe(other.as_ref())
            )),
        }
    }

    fn document(&mut self) -> Result<Document, String> {
        let mut doc = Document::default();
        while let Some(t) = self.peek().cloned() {
            match t {
                Tok::Punct('{') => {
                    let selection = self.selection_set()?;
                    doc.operations.push(Operation {
                        kind: "query".into(),
                        name: None,
                        defaults: vec![],
                        selection,
                    });
                }
                Tok::Name(n) if n == "query" || n == "mutation" || n == "subscription" => {
                    self.next();
                    let name = match self.peek() {
                        Some(Tok::Name(_)) => Some(self.name()?),
                        _ => None,
                    };
                    let mut defaults = Vec::new();
                    if self.is('(') {
                        self.next();
                        while !self.is(')') {
                            self.expect('$')?;
                            let var = self.name()?;
                            self.expect(':')?;
                            self.type_ref()?;
                            let default = if self.is('=') {
                                self.next();
                                Some(self.value(true)?)
                            } else {
                                None
                            };
                            while self.is('@') {
                                self.directive()?;
                            }
                            defaults.push((var, default));
                        }
                        self.expect(')')?;
                    }
                    while self.is('@') {
                        self.directive()?;
                    }
                    let selection = self.selection_set()?;
                    doc.operations.push(Operation {
                        kind: n,
                        name,
                        defaults,
                        selection,
                    });
                }
                Tok::Name(n) if n == "fragment" => {
                    self.next();
                    let name = self.name()?;
                    match self.next() {
                        Some(Tok::Name(on)) if on == "on" => {}
                        other => {
                            return Err(format!(
                                "Syntax Error: Expected \"on\", found {}.",
                                describe(other.as_ref())
                            ));
                        }
                    }
                    let type_cond = self.name()?;
                    while self.is('@') {
                        self.directive()?;
                    }
                    let selection = self.selection_set()?;
                    doc.fragments.insert(
                        name,
                        Fragment {
                            type_cond,
                            selection,
                        },
                    );
                }
                other => {
                    return Err(format!(
                        "Syntax Error: Unexpected {}.",
                        describe(Some(&other))
                    ));
                }
            }
        }
        if doc.operations.is_empty() {
            return Err("Syntax Error: Unexpected <EOF>.".into());
        }
        Ok(doc)
    }

    fn type_ref(&mut self) -> Result<(), String> {
        if self.is('[') {
            self.next();
            self.type_ref()?;
            self.expect(']')?;
        } else {
            self.name()?;
        }
        if self.is('!') {
            self.next();
        }
        Ok(())
    }

    fn directive(&mut self) -> Result<Directive, String> {
        self.expect('@')?;
        let name = self.name()?;
        let args = if self.is('(') {
            self.arguments()?
        } else {
            vec![]
        };
        Ok(Directive { name, args })
    }

    fn arguments(&mut self) -> Result<Vec<(String, Ast)>, String> {
        self.expect('(')?;
        let mut out = Vec::new();
        while !self.is(')') {
            let n = self.name()?;
            self.expect(':')?;
            out.push((n, self.value(false)?));
        }
        self.expect(')')?;
        Ok(out)
    }

    fn value(&mut self, constant: bool) -> Result<Ast, String> {
        match self.next() {
            Some(Tok::Punct('$')) if !constant => Ok(Ast::Var(self.name()?)),
            Some(Tok::Int(n)) => Ok(Ast::Int(n)),
            Some(Tok::Float(f)) => Ok(Ast::Float(f)),
            Some(Tok::Str(s)) => Ok(Ast::Str(s)),
            Some(Tok::Name(n)) => Ok(match n.as_str() {
                "true" => Ast::Bool(true),
                "false" => Ast::Bool(false),
                "null" => Ast::Null,
                _ => Ast::Enum(n),
            }),
            Some(Tok::Punct('[')) => {
                let mut items = Vec::new();
                while !self.is(']') {
                    items.push(self.value(constant)?);
                }
                self.expect(']')?;
                Ok(Ast::List(items))
            }
            Some(Tok::Punct('{')) => {
                let mut fields = Vec::new();
                while !self.is('}') {
                    let n = self.name()?;
                    self.expect(':')?;
                    fields.push((n, self.value(constant)?));
                }
                self.expect('}')?;
                Ok(Ast::Object(fields))
            }
            other => Err(format!(
                "Syntax Error: Unexpected {}.",
                describe(other.as_ref())
            )),
        }
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, String> {
        self.expect('{')?;
        let mut out = Vec::new();
        while !self.is('}') {
            if self.peek().is_none() {
                return Err("Syntax Error: Expected Name, found <EOF>.".into());
            }
            if self.peek() == Some(&Tok::Spread) {
                self.next();
                match self.peek().cloned() {
                    Some(Tok::Name(n)) if n == "on" => {
                        self.next();
                        let type_cond = Some(self.name()?);
                        let mut directives = Vec::new();
                        while self.is('@') {
                            directives.push(self.directive()?);
                        }
                        let selection = self.selection_set()?;
                        out.push(Selection::Inline {
                            type_cond,
                            directives,
                            selection,
                        });
                    }
                    Some(Tok::Name(_)) => {
                        let name = self.name()?;
                        let mut directives = Vec::new();
                        while self.is('@') {
                            directives.push(self.directive()?);
                        }
                        out.push(Selection::Spread { name, directives });
                    }
                    _ => {
                        let mut directives = Vec::new();
                        while self.is('@') {
                            directives.push(self.directive()?);
                        }
                        let selection = self.selection_set()?;
                        out.push(Selection::Inline {
                            type_cond: None,
                            directives,
                            selection,
                        });
                    }
                }
                continue;
            }
            let first = self.name()?;
            let (alias, name) = if self.is(':') {
                self.next();
                (Some(first), self.name()?)
            } else {
                (None, first)
            };
            let args = if self.is('(') {
                self.arguments()?
            } else {
                vec![]
            };
            let mut directives = Vec::new();
            while self.is('@') {
                directives.push(self.directive()?);
            }
            let selection = if self.is('{') {
                self.selection_set()?
            } else {
                vec![]
            };
            out.push(Selection::Field(Field {
                alias,
                name,
                args,
                directives,
                selection,
            }));
        }
        self.expect('}')?;
        Ok(out)
    }
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "<EOF>".into(),
        Some(Tok::Name(n)) => format!("Name \"{n}\""),
        Some(Tok::Str(s)) => format!("String \"{s}\""),
        Some(Tok::Int(n)) => format!("Int \"{n}\""),
        Some(Tok::Float(f)) => format!("Float \"{f}\""),
        Some(Tok::Punct(c)) => format!("\"{c}\""),
        Some(Tok::Spread) => "\"...\"".into(),
    }
}

pub fn parse(src: &str) -> Result<Document, String> {
    let toks = lex(src)?;
    P { toks, pos: 0 }.document()
}

/// Evaluates an argument AST against the request variables.
pub fn eval(ast: &Ast, vars: &Map<String, Value>) -> Value {
    match ast {
        Ast::Var(n) => vars.get(n).cloned().unwrap_or(Value::Null),
        Ast::Int(n) => Value::from(*n),
        Ast::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Ast::Str(s) | Ast::Enum(s) => Value::String(s.clone()),
        Ast::Bool(b) => Value::Bool(*b),
        Ast::Null => Value::Null,
        Ast::List(items) => Value::Array(items.iter().map(|i| eval(i, vars)).collect()),
        Ast::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), eval(v, vars)))
                .collect(),
        ),
    }
}

pub fn args_map(args: &[(String, Ast)], vars: &Map<String, Value>) -> Map<String, Value> {
    args.iter()
        .map(|(k, v)| (k.clone(), eval(v, vars)))
        .collect()
}

/// `@include(if:)` / `@skip(if:)`.
pub fn included(directives: &[Directive], vars: &Map<String, Value>) -> bool {
    for d in directives {
        let cond = d
            .args
            .iter()
            .find(|(k, _)| k == "if")
            .map(|(_, v)| eval(v, vars).as_bool().unwrap_or(false))
            .unwrap_or(false);
        match d.name.as_str() {
            "include" if !cond => return false,
            "skip" if cond => return false,
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operations_fragments_and_directives() {
        let doc = parse(
            r#"query Q($path: ID!, $n: Int = 5) {
              namespace(fullPath: $path) { a: workItem(iid: "1") { id ... on WorkItem { title } } }
              x @include(if: true) ...F
            }
            fragment F on Query { currentUser { username } }"#,
        )
        .unwrap();
        assert_eq!(doc.operations.len(), 1);
        assert_eq!(doc.operations[0].defaults.len(), 2);
        assert!(doc.fragments.contains_key("F"));
        let Selection::Field(ns) = &doc.operations[0].selection[0] else {
            panic!()
        };
        assert_eq!(ns.name, "namespace");
        let Selection::Field(wi) = &ns.selection[0] else {
            panic!()
        };
        assert_eq!(wi.key(), "a");
    }

    #[test]
    fn reports_syntax_errors() {
        assert!(parse("query { a ").is_err());
        assert!(parse("").is_err());
    }
}
