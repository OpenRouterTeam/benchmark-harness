//! A small GraphQL document parser: operations, variables, fields with arguments and
//! aliases, fragments (named and inline) and directives. Enough for every document the
//! Shopify Admin clients send; no schema or type-system definitions.

use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Var(String),
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Enum(String),
    List(Vec<Lit>),
    Obj(Vec<(String, Lit)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(n) => write!(f, "{n}"),
            Self::List(inner) => write!(f, "[{inner}]"),
            Self::NonNull(inner) => write!(f, "{inner}!"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub name: String,
    pub args: Vec<(String, Lit)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, Lit)>,
    pub directives: Vec<Directive>,
    pub selection: Vec<Selection>,
    pub pos: Pos,
}

impl Field {
    /// The response key: the alias if present, else the field name.
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Field(Field),
    Spread {
        name: String,
        directives: Vec<Directive>,
        pos: Pos,
    },
    Inline {
        type_condition: Option<String>,
        directives: Vec<Directive>,
        selection: Vec<Selection>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Query,
    Mutation,
    Subscription,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDef {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<Lit>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub kind: OpKind,
    pub name: Option<String>,
    pub vars: Vec<VarDef>,
    pub selection: Vec<Selection>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub name: String,
    pub type_condition: String,
    pub selection: Vec<Selection>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub operations: Vec<Operation>,
    pub fragments: Vec<Fragment>,
}

impl Document {
    pub fn fragment(&self, name: &str) -> Option<&Fragment> {
        self.fragments.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Punct(char),
    Spread,
    Name(String),
    Int(i64),
    Float(f64),
    Str(String),
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Self::Punct(c) => format!("\"{c}\""),
            Self::Spread => "\"...\"".to_owned(),
            Self::Name(n) => format!("\"{n}\""),
            Self::Int(i) => format!("\"{i}\""),
            Self::Float(f) => format!("\"{f}\""),
            Self::Str(s) => format!("\"{s}\""),
            Self::Eof => "end of file".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    pos: Pos,
}

fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let advance = |i: &mut usize, line: &mut usize, col: &mut usize, c: char| {
        *i += 1;
        if c == '\n' {
            *line += 1;
            *col = 1;
        } else {
            *col += 1;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let pos = Pos { line, column: col };
        match c {
            ' ' | '\t' | '\n' | '\r' | ',' | '\u{feff}' => advance(&mut i, &mut line, &mut col, c),
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    {
                        let ch = chars[i];
                        advance(&mut i, &mut line, &mut col, ch);
                    }
                }
            }
            '!' | '$' | '(' | ')' | ':' | '=' | '@' | '[' | ']' | '{' | '|' | '}' | '&' => {
                out.push(Token {
                    tok: Tok::Punct(c),
                    pos,
                });
                advance(&mut i, &mut line, &mut col, c);
            }
            '.' => {
                if chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
                    for _ in 0..3 {
                        advance(&mut i, &mut line, &mut col, '.');
                    }
                    out.push(Token {
                        tok: Tok::Spread,
                        pos,
                    });
                } else {
                    return Err(ParseError {
                        message: format!("Parse error on \".\" at [{line}, {col}]"),
                        pos,
                    });
                }
            }
            '"' => {
                let block = chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"');
                let mut s = String::new();
                if block {
                    for _ in 0..3 {
                        advance(&mut i, &mut line, &mut col, '"');
                    }
                    loop {
                        if i >= chars.len() {
                            return Err(unterminated(pos));
                        }
                        if chars[i] == '"'
                            && chars.get(i + 1) == Some(&'"')
                            && chars.get(i + 2) == Some(&'"')
                        {
                            for _ in 0..3 {
                                advance(&mut i, &mut line, &mut col, '"');
                            }
                            break;
                        }
                        s.push(chars[i]);
                        {
                            let ch = chars[i];
                            advance(&mut i, &mut line, &mut col, ch);
                        }
                    }
                    s = dedent_block(&s);
                } else {
                    advance(&mut i, &mut line, &mut col, '"');
                    loop {
                        let Some(&ch) = chars.get(i) else {
                            return Err(unterminated(pos));
                        };
                        if ch == '\n' {
                            return Err(unterminated(pos));
                        }
                        advance(&mut i, &mut line, &mut col, ch);
                        match ch {
                            '"' => break,
                            '\\' => {
                                let Some(&esc) = chars.get(i) else {
                                    return Err(unterminated(pos));
                                };
                                advance(&mut i, &mut line, &mut col, esc);
                                match esc {
                                    'n' => s.push('\n'),
                                    't' => s.push('\t'),
                                    'r' => s.push('\r'),
                                    'b' => s.push('\u{8}'),
                                    'f' => s.push('\u{c}'),
                                    'u' => {
                                        let hex: String =
                                            chars.iter().skip(i).take(4).collect::<String>();
                                        let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                            ParseError {
                                                message: format!(
                                                    "Parse error on bad Unicode escape sequence at [{line}, {col}]"
                                                ),
                                                pos,
                                            }
                                        })?;
                                        for _ in 0..4 {
                                            advance(&mut i, &mut line, &mut col, 'x');
                                        }
                                        s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                                    }
                                    other => s.push(other),
                                }
                            }
                            other => s.push(other),
                        }
                    }
                }
                out.push(Token {
                    tok: Tok::Str(s),
                    pos,
                });
            }
            c if c == '-' || c.is_ascii_digit() => {
                let start = i;
                let mut is_float = false;
                advance(&mut i, &mut line, &mut col, c);
                while let Some(&d) = chars.get(i) {
                    if d.is_ascii_digit() {
                        advance(&mut i, &mut line, &mut col, d);
                    } else if d == '.' || d == 'e' || d == 'E' {
                        is_float = true;
                        advance(&mut i, &mut line, &mut col, d);
                        if let Some(&sign) = chars.get(i)
                            && (sign == '+' || sign == '-')
                        {
                            advance(&mut i, &mut line, &mut col, sign);
                        }
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let bad = || ParseError {
                    message: format!(
                        "Parse error on \"{text}\" at [{}, {}]",
                        pos.line, pos.column
                    ),
                    pos,
                };
                let tok = if is_float {
                    Tok::Float(text.parse().map_err(|_| bad())?)
                } else {
                    Tok::Int(text.parse().map_err(|_| bad())?)
                };
                out.push(Token { tok, pos });
            }
            c if c == '_' || c.is_ascii_alphabetic() => {
                let start = i;
                while let Some(&d) = chars.get(i) {
                    if d == '_' || d.is_ascii_alphanumeric() {
                        advance(&mut i, &mut line, &mut col, d);
                    } else {
                        break;
                    }
                }
                out.push(Token {
                    tok: Tok::Name(chars[start..i].iter().collect()),
                    pos,
                });
            }
            other => {
                return Err(ParseError {
                    message: format!("Parse error on \"{other}\" at [{line}, {col}]"),
                    pos,
                });
            }
        }
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: Pos { line, column: col },
    });
    Ok(out)
}

fn unterminated(pos: Pos) -> ParseError {
    ParseError {
        message: format!(
            "Parse error on unterminated string at [{}, {}]",
            pos.line, pos.column
        ),
        pos,
    }
}

fn dedent_block(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(n, l)| {
            if n == 0 {
                (*l).to_owned()
            } else {
                l.chars().skip(indent).collect()
            }
        })
        .collect();
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n").replace("\\\"\"\"", "\"\"\"")
}

struct Parser {
    toks: Vec<Token>,
    at: usize,
}

pub fn parse(src: &str) -> Result<Document, ParseError> {
    let mut p = Parser {
        toks: lex(src)?,
        at: 0,
    };
    let mut doc = Document::default();
    loop {
        let t = p.peek().clone();
        match &t.tok {
            Tok::Eof => break,
            Tok::Punct('{') => {
                let selection = p.selection_set()?;
                doc.operations.push(Operation {
                    kind: OpKind::Query,
                    name: None,
                    vars: Vec::new(),
                    selection,
                    pos: t.pos,
                });
            }
            Tok::Name(n) if n == "query" || n == "mutation" || n == "subscription" => {
                p.bump();
                let kind = match n.as_str() {
                    "query" => OpKind::Query,
                    "mutation" => OpKind::Mutation,
                    _ => OpKind::Subscription,
                };
                let name = match &p.peek().tok {
                    Tok::Name(n) => {
                        let n = n.clone();
                        p.bump();
                        Some(n)
                    }
                    _ => None,
                };
                let vars = if p.is_punct('(') {
                    p.var_defs()?
                } else {
                    Vec::new()
                };
                p.directives()?;
                let selection = p.selection_set()?;
                doc.operations.push(Operation {
                    kind,
                    name,
                    vars,
                    selection,
                    pos: t.pos,
                });
            }
            Tok::Name(n) if n == "fragment" => {
                p.bump();
                let name = p.name()?;
                let on = p.name()?;
                if on != "on" {
                    return Err(p.error_at(p.at - 1));
                }
                let type_condition = p.name()?;
                p.directives()?;
                let selection = p.selection_set()?;
                doc.fragments.push(Fragment {
                    name,
                    type_condition,
                    selection,
                });
            }
            _ => return Err(p.error_at(p.at)),
        }
    }
    Ok(doc)
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.toks[self.at]
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.at].clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }

    fn is_punct(&self, c: char) -> bool {
        self.peek().tok == Tok::Punct(c)
    }

    fn error_at(&self, at: usize) -> ParseError {
        let t = &self.toks[at.min(self.toks.len() - 1)];
        ParseError {
            message: format!(
                "Parse error on {} at [{}, {}]",
                t.tok.describe(),
                t.pos.line,
                t.pos.column
            ),
            pos: t.pos,
        }
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        if self.is_punct(c) {
            self.bump();
            Ok(())
        } else {
            Err(self.error_at(self.at))
        }
    }

    fn name(&mut self) -> Result<String, ParseError> {
        match &self.peek().tok {
            Tok::Name(n) => {
                let n = n.clone();
                self.bump();
                Ok(n)
            }
            _ => Err(self.error_at(self.at)),
        }
    }

    fn var_defs(&mut self) -> Result<Vec<VarDef>, ParseError> {
        self.expect('(')?;
        let mut defs = Vec::new();
        while !self.is_punct(')') {
            let pos = self.peek().pos;
            self.expect('$')?;
            let name = self.name()?;
            self.expect(':')?;
            let ty = self.type_ref()?;
            let default = if self.is_punct('=') {
                self.bump();
                Some(self.value()?)
            } else {
                None
            };
            self.directives()?;
            defs.push(VarDef {
                name,
                ty,
                default,
                pos,
            });
        }
        self.expect(')')?;
        Ok(defs)
    }

    fn type_ref(&mut self) -> Result<TypeRef, ParseError> {
        let base = if self.is_punct('[') {
            self.bump();
            let inner = self.type_ref()?;
            self.expect(']')?;
            TypeRef::List(Box::new(inner))
        } else {
            TypeRef::Named(self.name()?)
        };
        if self.is_punct('!') {
            self.bump();
            Ok(TypeRef::NonNull(Box::new(base)))
        } else {
            Ok(base)
        }
    }

    fn directives(&mut self) -> Result<Vec<Directive>, ParseError> {
        let mut out = Vec::new();
        while self.is_punct('@') {
            self.bump();
            let name = self.name()?;
            let args = if self.is_punct('(') {
                self.args()?
            } else {
                Vec::new()
            };
            out.push(Directive { name, args });
        }
        Ok(out)
    }

    fn args(&mut self) -> Result<Vec<(String, Lit)>, ParseError> {
        self.expect('(')?;
        let mut out = Vec::new();
        while !self.is_punct(')') {
            let name = self.name()?;
            self.expect(':')?;
            out.push((name, self.value()?));
        }
        self.expect(')')?;
        Ok(out)
    }

    fn value(&mut self) -> Result<Lit, ParseError> {
        let t = self.bump();
        Ok(match t.tok {
            Tok::Punct('$') => Lit::Var(self.name()?),
            Tok::Int(i) => Lit::Int(i),
            Tok::Float(f) => Lit::Float(f),
            Tok::Str(s) => Lit::Str(s),
            Tok::Name(n) => match n.as_str() {
                "true" => Lit::Bool(true),
                "false" => Lit::Bool(false),
                "null" => Lit::Null,
                _ => Lit::Enum(n),
            },
            Tok::Punct('[') => {
                let mut items = Vec::new();
                while !self.is_punct(']') {
                    items.push(self.value()?);
                }
                self.expect(']')?;
                Lit::List(items)
            }
            Tok::Punct('{') => {
                let mut fields = Vec::new();
                while !self.is_punct('}') {
                    let name = self.name()?;
                    self.expect(':')?;
                    fields.push((name, self.value()?));
                }
                self.expect('}')?;
                Lit::Obj(fields)
            }
            _ => return Err(self.error_at(self.at.saturating_sub(1))),
        })
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, ParseError> {
        self.expect('{')?;
        let mut out = Vec::new();
        while !self.is_punct('}') {
            let t = self.peek().clone();
            match &t.tok {
                Tok::Spread => {
                    self.bump();
                    match &self.peek().tok {
                        Tok::Name(n) if n == "on" => {
                            self.bump();
                            let type_condition = Some(self.name()?);
                            let directives = self.directives()?;
                            let selection = self.selection_set()?;
                            out.push(Selection::Inline {
                                type_condition,
                                directives,
                                selection,
                            });
                        }
                        Tok::Name(_) => {
                            let name = self.name()?;
                            let directives = self.directives()?;
                            out.push(Selection::Spread {
                                name,
                                directives,
                                pos: t.pos,
                            });
                        }
                        _ => {
                            let directives = self.directives()?;
                            let selection = self.selection_set()?;
                            out.push(Selection::Inline {
                                type_condition: None,
                                directives,
                                selection,
                            });
                        }
                    }
                }
                Tok::Name(_) => {
                    let first = self.name()?;
                    let (alias, name) = if self.is_punct(':') {
                        self.bump();
                        (Some(first), self.name()?)
                    } else {
                        (None, first)
                    };
                    let args = if self.is_punct('(') {
                        self.args()?
                    } else {
                        Vec::new()
                    };
                    let directives = self.directives()?;
                    let selection = if self.is_punct('{') {
                        self.selection_set()?
                    } else {
                        Vec::new()
                    };
                    out.push(Selection::Field(Field {
                        alias,
                        name,
                        args,
                        directives,
                        selection,
                        pos: t.pos,
                    }));
                }
                _ => return Err(self.error_at(self.at)),
            }
        }
        self.expect('}')?;
        Ok(out)
    }
}

/// The JSON value of a literal, with variables substituted (absent variables are null).
pub fn lit_value(lit: &Lit, vars: &Map<String, Value>) -> Value {
    match lit {
        Lit::Var(name) => vars.get(name).cloned().unwrap_or(Value::Null),
        Lit::Int(i) => Value::from(*i),
        Lit::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Lit::Str(s) | Lit::Enum(s) => Value::String(s.clone()),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Null => Value::Null,
        Lit::List(items) => Value::Array(items.iter().map(|i| lit_value(i, vars)).collect()),
        Lit::Obj(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), lit_value(v, vars)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operation_with_variables_aliases_and_fragments() {
        let doc = parse(
            r#"
            # comment
            mutation productOptionsCreate($productId: ID!, $options: [OptionCreateInput!]!, $n: Int = 5) {
              productOptionsCreate(productId: $productId, options: $options, variantStrategy: LEAVE_AS_IS) {
                product { ...F }
                errs: userErrors { field message code }
              }
            }
            fragment F on Product { id ... on Product { title } }
            "#,
        )
        .unwrap();
        let op = &doc.operations[0];
        assert_eq!(op.kind, OpKind::Mutation);
        assert_eq!(op.name.as_deref(), Some("productOptionsCreate"));
        assert_eq!(op.vars.len(), 3);
        assert_eq!(op.vars[1].ty.to_string(), "[OptionCreateInput!]!");
        assert_eq!(op.vars[2].default, Some(Lit::Int(5)));
        let Selection::Field(root) = &op.selection[0] else {
            panic!()
        };
        assert_eq!(
            root.args[2],
            ("variantStrategy".into(), Lit::Enum("LEAVE_AS_IS".into()))
        );
        let Selection::Field(errs) = &root.selection[1] else {
            panic!()
        };
        assert_eq!(errs.key(), "errs");
        assert_eq!(errs.name, "userErrors");
        assert_eq!(doc.fragment("F").unwrap().type_condition, "Product");
    }

    #[test]
    fn reports_position_of_syntax_errors() {
        let err = parse("query {\n  products(first: 1) {\n }").unwrap_err();
        assert_eq!(err.message, "Parse error on end of file at [3, 3]");
        let err = parse("query { a(b: ) }").unwrap_err();
        assert_eq!(
            err.pos,
            Pos {
                line: 1,
                column: 14
            }
        );
    }

    #[test]
    fn literal_values_substitute_variables() {
        let doc = parse(r#"{ f(a: [1, "x", $v, {k: true, n: null}], e: ACTIVE) }"#).unwrap();
        let Selection::Field(f) = &doc.operations[0].selection[0] else {
            panic!()
        };
        let mut vars = Map::new();
        vars.insert("v".into(), Value::from(2.5));
        assert_eq!(
            lit_value(&f.args[0].1, &vars),
            serde_json::json!([1, "x", 2.5, {"k": true, "n": null}])
        );
        assert_eq!(lit_value(&f.args[1].1, &vars), "ACTIVE");
    }
}
