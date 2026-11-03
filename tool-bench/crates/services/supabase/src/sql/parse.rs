//! A small PostgreSQL-flavoured SQL lexer and parser. It covers what agents typically send
//! through `execute_sql` / `apply_migration` (SELECT with joins, grouping, CTEs; INSERT,
//! UPDATE, DELETE with RETURNING; common DDL) plus the ClickHouse-style `map['key']`
//! subscripts used by the logs endpoint. Anything else is a Postgres-style syntax error or,
//! for DDL that has no visible effect in the mock (functions, grants, triggers), a no-op.

use serde_json::Value;

use super::SqlError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// `"Quoted"` identifier (case preserved, never a keyword).
    QIdent(String),
    Num(String),
    Str(String),
    Param(usize),
    Sym(&'static str),
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Ident(s) | Tok::QIdent(s) | Tok::Num(s) => s.clone(),
            Tok::Str(s) => format!("'{s}'"),
            Tok::Param(n) => format!("${n}"),
            Tok::Sym(s) => (*s).to_owned(),
        }
    }
}

const SYMBOLS: [&str; 24] = [
    "->>", "::", "<>", "!=", "<=", ">=", "||", "->", "(", ")", ",", ";", ".", "*", "+", "-", "/",
    "%", "=", "<", ">", "[", "]", ":",
];

pub fn lex(sql: &str) -> Result<Vec<(Tok, usize)>, SqlError> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        let start = i;
        if c == '\'' || ((c == 'E' || c == 'e') && chars.get(i + 1) == Some(&'\'')) {
            let escapes = c != '\'';
            i += if escapes { 2 } else { 1 };
            let mut s = String::new();
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err(SqlError::syntax_end());
                };
                if ch == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        s.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if escapes
                    && ch == '\\'
                    && let Some(&n) = chars.get(i + 1)
                {
                    {
                        s.push(match n {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                        i += 2;
                        continue;
                    }
                }
                s.push(ch);
                i += 1;
            }
            out.push((Tok::Str(s), start));
            continue;
        }
        if c == '$' {
            // $1 parameter, or $$ / $tag$ dollar quoting.
            if chars.get(i + 1).is_some_and(char::is_ascii_digit) {
                i += 1;
                let mut n = String::new();
                while i < chars.len() && chars[i].is_ascii_digit() {
                    n.push(chars[i]);
                    i += 1;
                }
                out.push((Tok::Param(n.parse().unwrap_or(0)), start));
                continue;
            }
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if chars.get(j) == Some(&'$') {
                let tag: String = chars[i..=j].iter().collect();
                let body_start = j + 1;
                let rest: String = chars[body_start..].iter().collect();
                let Some(end) = rest.find(&tag) else {
                    return Err(SqlError::new(
                        "42601",
                        format!("unterminated dollar-quoted string at or near \"{tag}\""),
                    ));
                };
                let body: String = rest[..end].to_owned();
                i = body_start + body.chars().count() + tag.chars().count();
                out.push((Tok::Str(body), start));
                continue;
            }
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() {
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        s.push('"');
                        i += 2;
                        continue;
                    }
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            out.push((Tok::QIdent(s), start));
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let mut s = String::new();
            let mut seen_dot = false;
            let mut seen_exp = false;
            while i < chars.len() {
                let ch = chars[i];
                if ch.is_ascii_digit() {
                    s.push(ch);
                } else if ch == '.' && !seen_dot && !seen_exp {
                    // `1..` never happens in SQL; `t.1` is not valid either.
                    seen_dot = true;
                    s.push(ch);
                } else if (ch == 'e' || ch == 'E') && !seen_exp {
                    seen_exp = true;
                    s.push(ch);
                    if matches!(chars.get(i + 1), Some('+') | Some('-')) {
                        s.push(chars[i + 1]);
                        i += 1;
                    }
                } else {
                    break;
                }
                i += 1;
            }
            out.push((Tok::Num(s), start));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                s.push(chars[i]);
                i += 1;
            }
            out.push((Tok::Ident(s), start));
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
        let Some(sym) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
            return Err(SqlError::new(
                "42601",
                format!("syntax error at or near \"{c}\""),
            ));
        };
        i += sym.chars().count();
        out.push((Tok::Sym(sym), start));
    }
    Ok(out)
}

// ───────────────────────────── AST ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ObjName {
    pub schema: Option<String>,
    pub name: String,
}

impl ObjName {
    pub fn display(&self) -> String {
        match &self.schema {
            Some(s) => format!("{s}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(Option<String>, String),
    Literal(Value),
    Param(usize),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    IsNull(Box<Expr>, bool),
    IsBool(Box<Expr>, bool, bool),
    InList(Box<Expr>, Vec<Expr>, bool),
    InSelect(Box<Expr>, Box<Query>, bool),
    Between(Box<Expr>, Box<Expr>, Box<Expr>, bool),
    Like(Box<Expr>, Box<Expr>, bool, bool),
    Func {
        name: String,
        args: Vec<Expr>,
        distinct: bool,
        star: bool,
        filter: Option<Box<Expr>>,
    },
    Cast(Box<Expr>, String),
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Subscript(Box<Expr>, Box<Expr>),
    Exists(Box<Query>, bool),
    Scalar(Box<Query>),
    Interval(String),
    Array(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Wildcard,
    QualifiedWildcard(String),
    Expr(Expr, Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableSource {
    Table(ObjName),
    Subquery(Box<Query>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FromItem {
    pub source: TableSource,
    pub alias: Option<String>,
    /// `None` for the first item of the FROM list.
    pub join: Option<(JoinKind, Option<Expr>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderItem {
    pub expr: Expr,
    pub desc: bool,
    pub nulls_first: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub distinct: bool,
    pub items: Vec<SelectItem>,
    pub from: Vec<FromItem>,
    pub filter: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetBody {
    Select(Box<Select>),
    Values(Vec<Vec<Expr>>),
    Union(Box<SetBody>, Box<SetBody>, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub ctes: Vec<(String, Query)>,
    pub body: SetBody,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub primary_key: bool,
    pub unique: bool,
    pub default: Option<String>,
    pub identity: Option<String>,
    pub references: Option<(ObjName, Option<String>)>,
    pub check: Option<String>,
    pub on_delete: Option<String>,
}

/// `(constraint name, columns, target table, target columns)`
pub type TableForeignKey = (Option<String>, Vec<String>, ObjName, Vec<String>);

#[derive(Debug, Clone, PartialEq)]
pub enum AlterAction {
    AddColumn(ColumnDef, bool),
    DropColumn(String, bool),
    RenameColumn(String, String),
    RenameTo(String),
    SetRls(bool),
    ForceRls(bool),
    SetDefault(String, Option<String>),
    SetNotNull(String, bool),
    AddPrimaryKey(Vec<String>),
    AddForeignKey(Option<String>, Vec<String>, ObjName, Vec<String>),
    AddUnique(Vec<String>),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Query(Query),
    Insert {
        table: ObjName,
        columns: Vec<String>,
        source: Query,
        on_conflict: Option<OnConflict>,
        returning: Option<Vec<SelectItem>>,
    },
    Update {
        table: ObjName,
        alias: Option<String>,
        sets: Vec<(String, Expr)>,
        from: Vec<FromItem>,
        filter: Option<Expr>,
        returning: Option<Vec<SelectItem>>,
    },
    Delete {
        table: ObjName,
        alias: Option<String>,
        filter: Option<Expr>,
        returning: Option<Vec<SelectItem>>,
    },
    CreateTable {
        name: ObjName,
        if_not_exists: bool,
        columns: Vec<ColumnDef>,
        primary_key: Vec<String>,
        foreign_keys: Vec<TableForeignKey>,
        uniques: Vec<Vec<String>>,
    },
    AlterTable {
        name: ObjName,
        if_exists: bool,
        actions: Vec<AlterAction>,
    },
    DropTable {
        names: Vec<ObjName>,
        if_exists: bool,
        cascade: bool,
    },
    CreateIndex {
        name: Option<String>,
        table: ObjName,
        columns: Vec<String>,
        unique: bool,
        if_not_exists: bool,
    },
    DropIndex {
        names: Vec<ObjName>,
        if_exists: bool,
    },
    CreatePolicy {
        name: String,
        table: ObjName,
        command: String,
        roles: Vec<String>,
        using: Option<String>,
        check: Option<String>,
    },
    DropPolicy {
        name: String,
        table: ObjName,
        if_exists: bool,
    },
    CreateExtension {
        name: String,
        schema: Option<String>,
        if_not_exists: bool,
    },
    DropExtension {
        name: String,
        if_exists: bool,
    },
    Comment {
        table: ObjName,
        column: Option<String>,
        text: Option<String>,
    },
    Truncate(Vec<ObjName>),
    /// Accepted with no visible effect (BEGIN, GRANT, CREATE FUNCTION, ...).
    Noop(String),
}

impl Stmt {
    pub fn mutates(&self) -> bool {
        !matches!(self, Stmt::Query(_) | Stmt::Noop(_))
    }

    pub fn verb(&self) -> &'static str {
        match self {
            Stmt::Query(_) => "SELECT",
            Stmt::Insert { .. } => "INSERT",
            Stmt::Update { .. } => "UPDATE",
            Stmt::Delete { .. } => "DELETE",
            Stmt::CreateTable { .. } => "CREATE TABLE",
            Stmt::AlterTable { .. } => "ALTER TABLE",
            Stmt::DropTable { .. } => "DROP TABLE",
            Stmt::CreateIndex { .. } => "CREATE INDEX",
            Stmt::DropIndex { .. } => "DROP INDEX",
            Stmt::CreatePolicy { .. } => "CREATE POLICY",
            Stmt::DropPolicy { .. } => "DROP POLICY",
            Stmt::CreateExtension { .. } => "CREATE EXTENSION",
            Stmt::DropExtension { .. } => "DROP EXTENSION",
            Stmt::Comment { .. } => "COMMENT",
            Stmt::Truncate(_) => "TRUNCATE TABLE",
            Stmt::Noop(_) => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    pub columns: Vec<String>,
    /// `None` = DO NOTHING.
    pub update: Option<Vec<(String, Expr)>>,
}

// ───────────────────────────── Parser ─────────────────────────────

const RESERVED: [&str; 44] = [
    "select",
    "from",
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "on",
    "and",
    "or",
    "not",
    "as",
    "union",
    "returning",
    "set",
    "values",
    "into",
    "using",
    "with",
    "when",
    "then",
    "else",
    "end",
    "case",
    "is",
    "in",
    "between",
    "like",
    "ilike",
    "outer",
    "natural",
    "lateral",
    "except",
    "intersect",
    "window",
    "fetch",
    "for",
];

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

pub fn parse_script(sql: &str) -> Result<Vec<Stmt>, SqlError> {
    let mut p = Parser {
        src: sql,
        toks: lex(sql)?,
        pos: 0,
    };
    let mut stmts = Vec::new();
    loop {
        while p.eat_sym(";") {}
        if p.at_end() {
            break;
        }
        stmts.push(p.statement()?);
        if !p.at_end() && !p.eat_sym(";") {
            return Err(p.unexpected());
        }
    }
    Ok(stmts)
}

/// Parses one expression (used for column defaults stored as SQL text).
pub fn parse_expr(sql: &str) -> Result<Expr, SqlError> {
    let mut p = Parser {
        src: sql,
        toks: lex(sql)?,
        pos: 0,
    };
    let e = p.expr()?;
    if !p.at_end() {
        return Err(p.unexpected());
    }
    Ok(e)
}

impl Parser<'_> {
    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n).map(|(t, _)| t)
    }

    fn unexpected(&self) -> SqlError {
        match self.peek() {
            Some(t) => SqlError::new("42601", format!("syntax error at or near \"{}\"", t.text())),
            None => SqlError::syntax_end(),
        }
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn is_kw_at(&self, n: usize, kw: &str) -> bool {
        matches!(self.peek_at(n), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_kws(&mut self, kws: &[&str]) -> bool {
        if kws.iter().enumerate().all(|(i, k)| self.is_kw_at(i, k)) {
            self.pos += kws.len();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), SqlError> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn is_sym(&self, sym: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym)
    }

    fn eat_sym(&mut self, sym: &str) -> bool {
        if self.is_sym(sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, sym: &str) -> Result<(), SqlError> {
        if self.eat_sym(sym) {
            Ok(())
        } else {
            Err(self.unexpected())
        }
    }

    fn ident(&mut self) -> Result<String, SqlError> {
        match self.peek().cloned() {
            Some(Tok::Ident(s)) => {
                self.pos += 1;
                Ok(s.to_lowercase())
            }
            Some(Tok::QIdent(s)) => {
                self.pos += 1;
                Ok(s)
            }
            _ => Err(self.unexpected()),
        }
    }

    fn obj_name(&mut self) -> Result<ObjName, SqlError> {
        let first = self.ident()?;
        if self.eat_sym(".") {
            let second = self.ident()?;
            Ok(ObjName {
                schema: Some(first),
                name: second,
            })
        } else {
            Ok(ObjName {
                schema: None,
                name: first,
            })
        }
    }

    fn ident_list(&mut self) -> Result<Vec<String>, SqlError> {
        self.expect_sym("(")?;
        let mut out = vec![self.ident()?];
        while self.eat_sym(",") {
            out.push(self.ident()?);
        }
        self.expect_sym(")")?;
        Ok(out)
    }

    /// Source text from token `from` up to (not including) the current token.
    fn text_since(&self, from: usize) -> String {
        let start = self.toks.get(from).map_or(self.src.len(), |t| t.1);
        let end = self.toks.get(self.pos).map_or(self.src.len(), |t| t.1);
        self.src[start..end].trim().to_owned()
    }

    /// Skips to the end of the current statement, respecting parentheses.
    fn skip_statement(&mut self) {
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            match t {
                Tok::Sym("(") => depth += 1,
                Tok::Sym(")") => depth -= 1,
                Tok::Sym(";") if depth <= 0 => break,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Skips a balanced token run until one of `stops` at depth 0 (or `;` / end).
    fn skip_until(&mut self, stops: &[&str]) {
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            match t {
                Tok::Sym("(") => depth += 1,
                Tok::Sym(")") => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Sym(",") if depth == 0 && stops.contains(&",") => break,
                Tok::Sym(";") if depth == 0 => break,
                Tok::Ident(s) if depth == 0 && stops.iter().any(|k| s.eq_ignore_ascii_case(k)) => {
                    break;
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn statement(&mut self) -> Result<Stmt, SqlError> {
        let start = self.pos;
        if self.is_kw("select") || self.is_kw("with") || self.is_kw("values") || self.is_sym("(") {
            return Ok(Stmt::Query(self.query()?));
        }
        if self.eat_kw("insert") {
            return self.insert();
        }
        if self.eat_kw("update") {
            return self.update();
        }
        if self.eat_kw("delete") {
            return self.delete();
        }
        if self.eat_kw("truncate") {
            self.eat_kw("table");
            let mut names = vec![self.obj_name()?];
            while self.eat_sym(",") {
                names.push(self.obj_name()?);
            }
            self.skip_statement();
            return Ok(Stmt::Truncate(names));
        }
        if self.is_kw("create") {
            return self.create(start);
        }
        if self.eat_kw("alter") {
            if self.eat_kw("table") {
                return self.alter_table();
            }
            self.skip_statement();
            return Ok(Stmt::Noop(self.text_since(start)));
        }
        if self.eat_kw("drop") {
            return self.drop(start);
        }
        if self.eat_kw("comment") {
            return self.comment(start);
        }
        const NOOPS: [&str; 18] = [
            "begin",
            "commit",
            "rollback",
            "start",
            "end",
            "grant",
            "revoke",
            "set",
            "reset",
            "notify",
            "analyze",
            "vacuum",
            "do",
            "reindex",
            "refresh",
            "savepoint",
            "release",
            "show",
        ];
        if NOOPS.iter().any(|k| self.is_kw(k)) {
            self.skip_statement();
            return Ok(Stmt::Noop(self.text_since(start)));
        }
        Err(self.unexpected())
    }

    fn create(&mut self, start: usize) -> Result<Stmt, SqlError> {
        self.expect_kw("create")?;
        self.eat_kws(&["or", "replace"]);
        if self.eat_kw("table") {
            return self.create_table();
        }
        let unique = self.eat_kw("unique");
        if self.eat_kw("index") {
            self.eat_kw("concurrently");
            let if_not_exists = self.eat_kws(&["if", "not", "exists"]);
            let name = if self.is_kw("on") {
                None
            } else {
                Some(self.ident()?)
            };
            self.expect_kw("on")?;
            self.eat_kw("only");
            let table = self.obj_name()?;
            if self.eat_kw("using") {
                self.ident()?;
            }
            self.expect_sym("(")?;
            let mut columns = Vec::new();
            loop {
                let col_start = self.pos;
                self.skip_until(&[","]);
                columns.push(self.text_since(col_start));
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
            self.skip_statement();
            return Ok(Stmt::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            });
        }
        if self.eat_kw("policy") {
            let name = self.ident()?;
            self.expect_kw("on")?;
            let table = self.obj_name()?;
            let mut command = "ALL".to_owned();
            let mut roles = vec!["public".to_owned()];
            let mut using = None;
            let mut check = None;
            while !self.at_end() && !self.is_sym(";") {
                if self.eat_kw("as") {
                    self.ident()?;
                } else if self.eat_kw("for") {
                    command = self.ident()?.to_uppercase();
                } else if self.eat_kw("to") {
                    roles = vec![self.ident()?];
                    while self.eat_sym(",") {
                        roles.push(self.ident()?);
                    }
                } else if self.eat_kw("using") {
                    self.expect_sym("(")?;
                    let s = self.pos;
                    self.skip_until(&[]);
                    using = Some(self.text_since(s));
                    self.expect_sym(")")?;
                } else if self.eat_kws(&["with", "check"]) {
                    self.expect_sym("(")?;
                    let s = self.pos;
                    self.skip_until(&[]);
                    check = Some(self.text_since(s));
                    self.expect_sym(")")?;
                } else {
                    return Err(self.unexpected());
                }
            }
            return Ok(Stmt::CreatePolicy {
                name,
                table,
                command,
                roles,
                using,
                check,
            });
        }
        if self.eat_kw("extension") {
            let if_not_exists = self.eat_kws(&["if", "not", "exists"]);
            let name = self.ident()?;
            let mut schema = None;
            while !self.at_end() && !self.is_sym(";") {
                if self.eat_kw("schema") {
                    schema = Some(self.ident()?);
                } else {
                    self.pos += 1;
                }
            }
            return Ok(Stmt::CreateExtension {
                name,
                schema,
                if_not_exists,
            });
        }
        // Functions, triggers, types, schemas, views: accepted, no visible effect.
        self.skip_statement();
        Ok(Stmt::Noop(self.text_since(start)))
    }

    fn drop(&mut self, start: usize) -> Result<Stmt, SqlError> {
        if self.eat_kw("table") {
            let if_exists = self.eat_kws(&["if", "exists"]);
            let mut names = vec![self.obj_name()?];
            while self.eat_sym(",") {
                names.push(self.obj_name()?);
            }
            let cascade = self.eat_kw("cascade");
            self.eat_kw("restrict");
            return Ok(Stmt::DropTable {
                names,
                if_exists,
                cascade,
            });
        }
        if self.eat_kw("index") {
            self.eat_kw("concurrently");
            let if_exists = self.eat_kws(&["if", "exists"]);
            let mut names = vec![self.obj_name()?];
            while self.eat_sym(",") {
                names.push(self.obj_name()?);
            }
            self.skip_statement();
            return Ok(Stmt::DropIndex { names, if_exists });
        }
        if self.eat_kw("policy") {
            let if_exists = self.eat_kws(&["if", "exists"]);
            let name = self.ident()?;
            self.expect_kw("on")?;
            let table = self.obj_name()?;
            self.skip_statement();
            return Ok(Stmt::DropPolicy {
                name,
                table,
                if_exists,
            });
        }
        if self.eat_kw("extension") {
            let if_exists = self.eat_kws(&["if", "exists"]);
            let name = self.ident()?;
            self.skip_statement();
            return Ok(Stmt::DropExtension { name, if_exists });
        }
        self.skip_statement();
        Ok(Stmt::Noop(self.text_since(start)))
    }

    fn comment(&mut self, start: usize) -> Result<Stmt, SqlError> {
        self.expect_kw("on")?;
        let (table, column) = if self.eat_kw("table") {
            (self.obj_name()?, None)
        } else if self.eat_kw("column") {
            let a = self.ident()?;
            self.expect_sym(".")?;
            let b = self.ident()?;
            if self.eat_sym(".") {
                let c = self.ident()?;
                (
                    ObjName {
                        schema: Some(a),
                        name: b,
                    },
                    Some(c),
                )
            } else {
                (
                    ObjName {
                        schema: None,
                        name: a,
                    },
                    Some(b),
                )
            }
        } else {
            self.skip_statement();
            return Ok(Stmt::Noop(self.text_since(start)));
        };
        self.expect_kw("is")?;
        let text = match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Some(s)
            }
            _ => {
                self.expect_kw("null")?;
                None
            }
        };
        Ok(Stmt::Comment {
            table,
            column,
            text,
        })
    }

    fn create_table(&mut self) -> Result<Stmt, SqlError> {
        let if_not_exists = self.eat_kws(&["if", "not", "exists"]);
        let name = self.obj_name()?;
        self.expect_sym("(")?;
        let mut columns = Vec::new();
        let mut primary_key = Vec::new();
        let mut foreign_keys = Vec::new();
        let mut uniques = Vec::new();
        loop {
            if self.eat_kw("constraint") {
                self.ident()?;
            }
            if self.eat_kws(&["primary", "key"]) {
                primary_key = self.ident_list()?;
            } else if self.eat_kws(&["foreign", "key"]) {
                let cols = self.ident_list()?;
                self.expect_kw("references")?;
                let target = self.obj_name()?;
                let target_cols = if self.is_sym("(") {
                    self.ident_list()?
                } else {
                    vec!["id".to_owned()]
                };
                self.skip_until(&[","]);
                foreign_keys.push((None, cols, target, target_cols));
            } else if self.eat_kw("unique") {
                uniques.push(self.ident_list()?);
            } else if self.eat_kw("check") || self.eat_kw("exclude") {
                self.skip_until(&[","]);
            } else {
                columns.push(self.column_def()?);
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        self.skip_statement();
        Ok(Stmt::CreateTable {
            name,
            if_not_exists,
            columns,
            primary_key,
            foreign_keys,
            uniques,
        })
    }

    fn data_type(&mut self) -> Result<String, SqlError> {
        let mut words = vec![self.ident()?];
        // Multi-word types.
        while let Some(Tok::Ident(s)) = self.peek() {
            let next = s.to_lowercase();
            let last = words.last().cloned().unwrap_or_default();
            let joins = matches!(
                (last.as_str(), next.as_str()),
                ("double", "precision")
                    | ("character", "varying")
                    | ("timestamp", "with")
                    | ("timestamp", "without")
                    | ("time", "with")
                    | ("time", "without")
                    | ("with", "time")
                    | ("without", "time")
                    | ("time", "zone")
            );
            if !joins {
                break;
            }
            self.pos += 1;
            words.push(next);
        }
        if self.eat_sym(".") {
            // schema-qualified type (e.g. public.order_status)
            words = vec![self.ident()?];
        }
        let mut ty = words.join(" ");
        if self.eat_sym("(") {
            let s = self.pos;
            self.skip_until(&[]);
            ty = format!("{ty}({})", self.text_since(s));
            self.expect_sym(")")?;
        }
        while self.eat_sym("[") {
            self.expect_sym("]")?;
            ty.push_str("[]");
        }
        Ok(ty)
    }

    fn column_def(&mut self) -> Result<ColumnDef, SqlError> {
        let name = self.ident()?;
        let data_type = self.data_type()?;
        let mut col = ColumnDef {
            name,
            data_type,
            not_null: false,
            primary_key: false,
            unique: false,
            default: None,
            identity: None,
            references: None,
            check: None,
            on_delete: None,
        };
        loop {
            if self.eat_kw("constraint") {
                self.ident()?;
            }
            if self.eat_kws(&["not", "null"]) {
                col.not_null = true;
            } else if self.eat_kw("null") {
            } else if self.eat_kws(&["primary", "key"]) {
                col.primary_key = true;
                col.not_null = true;
            } else if self.eat_kw("unique") {
                col.unique = true;
            } else if self.eat_kw("default") {
                let s = self.pos;
                self.default_expr()?;
                col.default = Some(self.text_since(s));
            } else if self.eat_kw("generated") {
                let kind = if self.eat_kw("always") {
                    "ALWAYS"
                } else {
                    self.expect_kw("by")?;
                    self.expect_kw("default")?;
                    "BY DEFAULT"
                };
                if self.eat_kws(&["as", "identity"]) {
                    col.identity = Some(kind.to_owned());
                    col.not_null = true;
                    if self.is_sym("(") {
                        self.pos += 1;
                        self.skip_until(&[]);
                        self.expect_sym(")")?;
                    }
                } else {
                    self.expect_kw("as")?;
                    self.expect_sym("(")?;
                    self.skip_until(&[]);
                    self.expect_sym(")")?;
                    self.eat_kw("stored");
                }
            } else if self.eat_kw("references") {
                let target = self.obj_name()?;
                let tcol = if self.is_sym("(") {
                    Some(self.ident_list()?.remove(0))
                } else {
                    None
                };
                col.references = Some((target, tcol));
                while self.eat_kw("on") {
                    let event = self.ident()?;
                    let mut action = String::new();
                    if self.eat_kw("set") {
                        action.push_str("set ");
                    } else if self.eat_kw("no") {
                        action.push_str("no ");
                    }
                    action.push_str(&self.ident()?);
                    if event == "delete" {
                        col.on_delete = Some(action);
                    }
                }
            } else if self.eat_kw("check") {
                self.expect_sym("(")?;
                let s = self.pos;
                self.skip_until(&[]);
                col.check = Some(self.text_since(s));
                self.expect_sym(")")?;
            } else if self.eat_kw("collate") {
                self.ident()?;
            } else {
                break;
            }
        }
        if col.data_type == "serial"
            || col.data_type == "bigserial"
            || col.data_type == "smallserial"
        {
            let base = match col.data_type.as_str() {
                "bigserial" => "bigint",
                "smallserial" => "smallint",
                _ => "integer",
            };
            col.data_type = base.to_owned();
            col.not_null = true;
            col.default = Some("nextval".to_owned());
        }
        Ok(col)
    }

    fn default_expr(&mut self) -> Result<(), SqlError> {
        // A default is an expression; stop at column-constraint keywords.
        self.expr_bp(0, true)?;
        Ok(())
    }

    fn alter_table(&mut self) -> Result<Stmt, SqlError> {
        let if_exists = self.eat_kws(&["if", "exists"]);
        self.eat_kw("only");
        let name = self.obj_name()?;
        let mut actions = Vec::new();
        loop {
            actions.push(self.alter_action()?);
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(Stmt::AlterTable {
            name,
            if_exists,
            actions,
        })
    }

    fn alter_action(&mut self) -> Result<AlterAction, SqlError> {
        if self.eat_kw("enable") {
            self.expect_kw("row")?;
            self.expect_kw("level")?;
            self.expect_kw("security")?;
            return Ok(AlterAction::SetRls(true));
        }
        if self.eat_kw("disable") {
            self.expect_kw("row")?;
            self.expect_kw("level")?;
            self.expect_kw("security")?;
            return Ok(AlterAction::SetRls(false));
        }
        if self.eat_kws(&["force", "row", "level", "security"]) {
            return Ok(AlterAction::ForceRls(true));
        }
        if self.eat_kws(&["no", "force", "row", "level", "security"]) {
            return Ok(AlterAction::ForceRls(false));
        }
        if self.eat_kw("add") {
            if self.eat_kw("constraint") {
                let cname = self.ident()?;
                return self.add_constraint(Some(cname));
            }
            if self.is_kw("primary")
                || self.is_kw("foreign")
                || self.is_kw("unique")
                || self.is_kw("check")
            {
                return self.add_constraint(None);
            }
            self.eat_kw("column");
            let if_not_exists = self.eat_kws(&["if", "not", "exists"]);
            return Ok(AlterAction::AddColumn(self.column_def()?, if_not_exists));
        }
        if self.eat_kw("drop") {
            if self.eat_kw("constraint") {
                self.skip_until(&[","]);
                return Ok(AlterAction::Other);
            }
            self.eat_kw("column");
            let if_exists = self.eat_kws(&["if", "exists"]);
            let col = self.ident()?;
            self.eat_kw("cascade");
            return Ok(AlterAction::DropColumn(col, if_exists));
        }
        if self.eat_kw("rename") {
            if self.eat_kw("to") {
                return Ok(AlterAction::RenameTo(self.ident()?));
            }
            self.eat_kw("column");
            let from = self.ident()?;
            self.expect_kw("to")?;
            return Ok(AlterAction::RenameColumn(from, self.ident()?));
        }
        if self.eat_kw("alter") {
            self.eat_kw("column");
            let col = self.ident()?;
            if self.eat_kws(&["set", "default"]) {
                let s = self.pos;
                self.default_expr()?;
                return Ok(AlterAction::SetDefault(col, Some(self.text_since(s))));
            }
            if self.eat_kws(&["drop", "default"]) {
                return Ok(AlterAction::SetDefault(col, None));
            }
            if self.eat_kws(&["set", "not", "null"]) {
                return Ok(AlterAction::SetNotNull(col, true));
            }
            if self.eat_kws(&["drop", "not", "null"]) {
                return Ok(AlterAction::SetNotNull(col, false));
            }
            self.skip_until(&[","]);
            return Ok(AlterAction::Other);
        }
        if self.is_kw("owner") || self.is_kw("replica") || self.is_kw("set") {
            self.skip_until(&[","]);
            return Ok(AlterAction::Other);
        }
        Err(self.unexpected())
    }

    fn add_constraint(&mut self, cname: Option<String>) -> Result<AlterAction, SqlError> {
        if self.eat_kws(&["primary", "key"]) {
            return Ok(AlterAction::AddPrimaryKey(self.ident_list()?));
        }
        if self.eat_kws(&["foreign", "key"]) {
            let cols = self.ident_list()?;
            self.expect_kw("references")?;
            let target = self.obj_name()?;
            let tcols = if self.is_sym("(") {
                self.ident_list()?
            } else {
                vec!["id".to_owned()]
            };
            self.skip_until(&[","]);
            return Ok(AlterAction::AddForeignKey(cname, cols, target, tcols));
        }
        if self.eat_kw("unique") {
            return Ok(AlterAction::AddUnique(self.ident_list()?));
        }
        self.skip_until(&[","]);
        Ok(AlterAction::Other)
    }

    fn returning(&mut self) -> Result<Option<Vec<SelectItem>>, SqlError> {
        if self.eat_kw("returning") {
            Ok(Some(self.select_items()?))
        } else {
            Ok(None)
        }
    }

    fn insert(&mut self) -> Result<Stmt, SqlError> {
        self.expect_kw("into")?;
        let table = self.obj_name()?;
        if self.eat_kw("as") {
            self.ident()?;
        }
        let columns = if self.is_sym("(") && !self.is_kw_at(1, "select") {
            self.ident_list()?
        } else {
            Vec::new()
        };
        let source = if self.eat_kws(&["default", "values"]) {
            Query {
                ctes: Vec::new(),
                body: SetBody::Values(vec![Vec::new()]),
                order_by: Vec::new(),
                limit: None,
                offset: None,
            }
        } else {
            self.query()?
        };
        let mut on_conflict = None;
        if self.eat_kws(&["on", "conflict"]) {
            let cols = if self.is_sym("(") {
                self.ident_list()?
            } else {
                Vec::new()
            };
            self.expect_kw("do")?;
            if self.eat_kw("nothing") {
                on_conflict = Some(OnConflict {
                    columns: cols,
                    update: None,
                });
            } else {
                self.expect_kw("update")?;
                self.expect_kw("set")?;
                on_conflict = Some(OnConflict {
                    columns: cols,
                    update: Some(self.assignments()?),
                });
            }
        }
        let returning = self.returning()?;
        Ok(Stmt::Insert {
            table,
            columns,
            source,
            on_conflict,
            returning,
        })
    }

    fn assignments(&mut self) -> Result<Vec<(String, Expr)>, SqlError> {
        let mut sets = Vec::new();
        loop {
            let mut col = self.ident()?;
            if self.eat_sym(".") {
                col = self.ident()?;
            }
            self.expect_sym("=")?;
            let value = if self.eat_kw("default") {
                Expr::Func {
                    name: "__default".to_owned(),
                    args: Vec::new(),
                    distinct: false,
                    star: false,
                    filter: None,
                }
            } else {
                self.expr()?
            };
            sets.push((col, value));
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(sets)
    }

    fn table_alias(&mut self) -> Result<Option<String>, SqlError> {
        if self.eat_kw("as") {
            return Ok(Some(self.ident()?));
        }
        match self.peek() {
            Some(Tok::Ident(s)) if !RESERVED.contains(&s.to_lowercase().as_str()) => {
                Ok(Some(self.ident()?))
            }
            Some(Tok::QIdent(_)) => Ok(Some(self.ident()?)),
            _ => Ok(None),
        }
    }

    fn update(&mut self) -> Result<Stmt, SqlError> {
        self.eat_kw("only");
        let table = self.obj_name()?;
        let alias = if self.is_kw("set") {
            None
        } else {
            self.table_alias()?
        };
        self.expect_kw("set")?;
        let sets = self.assignments()?;
        let from = if self.eat_kw("from") {
            self.parse_from_list()?
        } else {
            Vec::new()
        };
        let filter = if self.eat_kw("where") {
            Some(self.expr()?)
        } else {
            None
        };
        let returning = self.returning()?;
        Ok(Stmt::Update {
            table,
            alias,
            sets,
            from,
            filter,
            returning,
        })
    }

    fn delete(&mut self) -> Result<Stmt, SqlError> {
        self.expect_kw("from")?;
        self.eat_kw("only");
        let table = self.obj_name()?;
        let alias = if self.is_kw("where") || self.is_kw("returning") {
            None
        } else {
            self.table_alias()?
        };
        let filter = if self.eat_kw("where") {
            Some(self.expr()?)
        } else {
            None
        };
        let returning = self.returning()?;
        Ok(Stmt::Delete {
            table,
            alias,
            filter,
            returning,
        })
    }

    pub fn query(&mut self) -> Result<Query, SqlError> {
        let mut ctes = Vec::new();
        if self.eat_kw("with") {
            self.eat_kw("recursive");
            loop {
                let name = self.ident()?;
                if self.is_sym("(") {
                    self.ident_list()?;
                }
                self.expect_kw("as")?;
                self.eat_kw("materialized");
                self.eat_kws(&["not", "materialized"]);
                self.expect_sym("(")?;
                let q = self.query()?;
                self.expect_sym(")")?;
                ctes.push((name, q));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let mut body = self.set_term()?;
        while self.eat_kw("union") {
            let all = self.eat_kw("all");
            self.eat_kw("distinct");
            let rhs = self.set_term()?;
            body = SetBody::Union(Box::new(body), Box::new(rhs), all);
        }
        let mut order_by = Vec::new();
        if self.eat_kws(&["order", "by"]) {
            loop {
                let expr = self.expr()?;
                let desc = if self.eat_kw("desc") {
                    true
                } else {
                    self.eat_kw("asc");
                    false
                };
                let nulls_first = if self.eat_kw("nulls") {
                    if self.eat_kw("first") {
                        Some(true)
                    } else {
                        self.expect_kw("last")?;
                        Some(false)
                    }
                } else {
                    None
                };
                order_by.push(OrderItem {
                    expr,
                    desc,
                    nulls_first,
                });
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let mut limit = None;
        let mut offset = None;
        for _ in 0..2 {
            if self.eat_kw("limit") {
                limit = if self.eat_kw("all") {
                    None
                } else {
                    Some(self.expr()?)
                };
            } else if self.eat_kw("offset") {
                offset = Some(self.expr()?);
                self.eat_kw("rows");
                self.eat_kw("row");
            } else if self.eat_kw("fetch") {
                self.eat_kw("first");
                self.eat_kw("next");
                limit = Some(self.expr()?);
                self.eat_kw("rows");
                self.eat_kw("row");
                self.expect_kw("only")?;
            }
        }
        Ok(Query {
            ctes,
            body,
            order_by,
            limit,
            offset,
        })
    }

    fn set_term(&mut self) -> Result<SetBody, SqlError> {
        if self.is_sym("(") && (self.is_kw_at(1, "select") || self.is_kw_at(1, "with")) {
            self.pos += 1;
            let q = self.query()?;
            self.expect_sym(")")?;
            if q.ctes.is_empty() && q.order_by.is_empty() && q.limit.is_none() && q.offset.is_none()
            {
                return Ok(q.body);
            }
            return Ok(SetBody::Select(Box::new(Select {
                distinct: false,
                items: vec![SelectItem::Wildcard],
                from: vec![FromItem {
                    source: TableSource::Subquery(Box::new(q)),
                    alias: Some("_sub".to_owned()),
                    join: None,
                }],
                filter: None,
                group_by: Vec::new(),
                having: None,
            })));
        }
        if self.eat_kw("values") {
            let mut rows = Vec::new();
            loop {
                self.expect_sym("(")?;
                let mut row = Vec::new();
                if !self.is_sym(")") {
                    loop {
                        if self.eat_kw("default") {
                            row.push(Expr::Func {
                                name: "__default".to_owned(),
                                args: Vec::new(),
                                distinct: false,
                                star: false,
                                filter: None,
                            });
                        } else {
                            row.push(self.expr()?);
                        }
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                }
                self.expect_sym(")")?;
                rows.push(row);
                if !self.eat_sym(",") {
                    break;
                }
            }
            return Ok(SetBody::Values(rows));
        }
        self.expect_kw("select")?;
        Ok(SetBody::Select(Box::new(self.select_rest()?)))
    }

    fn select_items(&mut self) -> Result<Vec<SelectItem>, SqlError> {
        let mut items = Vec::new();
        loop {
            if self.eat_sym("*") {
                items.push(SelectItem::Wildcard);
            } else if matches!(self.peek(), Some(Tok::Ident(_)) | Some(Tok::QIdent(_)))
                && matches!(self.peek_at(1), Some(Tok::Sym(".")))
                && matches!(self.peek_at(2), Some(Tok::Sym("*")))
            {
                let q = self.ident()?;
                self.pos += 2;
                items.push(SelectItem::QualifiedWildcard(q));
            } else {
                let e = self.expr()?;
                let alias = if self.eat_kw("as") {
                    Some(self.ident()?)
                } else {
                    match self.peek() {
                        Some(Tok::Ident(s)) if !RESERVED.contains(&s.to_lowercase().as_str()) => {
                            Some(self.ident()?)
                        }
                        Some(Tok::QIdent(_)) => Some(self.ident()?),
                        _ => None,
                    }
                };
                items.push(SelectItem::Expr(e, alias));
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(items)
    }

    fn select_rest(&mut self) -> Result<Select, SqlError> {
        let mut distinct = false;
        if self.eat_kw("distinct") {
            distinct = true;
            if self.eat_kw("on") {
                // DISTINCT ON (...) is treated as DISTINCT over the listed rows.
                self.expect_sym("(")?;
                self.skip_until(&[]);
                self.expect_sym(")")?;
            }
        } else {
            self.eat_kw("all");
        }
        let items = if self.is_kw("from") {
            Vec::new()
        } else {
            self.select_items()?
        };
        let from = if self.eat_kw("from") {
            self.parse_from_list()?
        } else {
            Vec::new()
        };
        let filter = if self.eat_kw("where") {
            Some(self.expr()?)
        } else {
            None
        };
        let mut group_by = Vec::new();
        if self.eat_kws(&["group", "by"]) {
            loop {
                group_by.push(self.expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let having = if self.eat_kw("having") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Select {
            distinct,
            items,
            from,
            filter,
            group_by,
            having,
        })
    }

    fn parse_from_list(&mut self) -> Result<Vec<FromItem>, SqlError> {
        let mut items = vec![self.parse_from_item(None)?];
        loop {
            if self.eat_sym(",") {
                items.push(self.parse_from_item(Some((JoinKind::Cross, None)))?);
                continue;
            }
            let kind = if self.eat_kws(&["cross", "join"]) {
                JoinKind::Cross
            } else if self.eat_kw("join") || self.eat_kws(&["inner", "join"]) {
                JoinKind::Inner
            } else if self.eat_kws(&["left", "join"]) || self.eat_kws(&["left", "outer", "join"]) {
                JoinKind::Left
            } else if self.eat_kws(&["right", "join"]) || self.eat_kws(&["right", "outer", "join"])
            {
                JoinKind::Right
            } else if self.eat_kws(&["full", "join"]) || self.eat_kws(&["full", "outer", "join"]) {
                JoinKind::Full
            } else {
                break;
            };
            let mut item = self.parse_from_item(None)?;
            let on = if kind == JoinKind::Cross {
                None
            } else if self.eat_kw("on") {
                Some(self.expr()?)
            } else if self.eat_kw("using") {
                let cols = self.ident_list()?;
                let left = items.last().map(from_alias).unwrap_or_default();
                let right = from_alias(&item);
                let mut cond: Option<Expr> = None;
                for c in cols {
                    let eq = Expr::Binary(
                        "=".to_owned(),
                        Box::new(Expr::Column(Some(left.clone()), c.clone())),
                        Box::new(Expr::Column(Some(right.clone()), c)),
                    );
                    cond = Some(match cond {
                        Some(prev) => Expr::Binary("and".to_owned(), Box::new(prev), Box::new(eq)),
                        None => eq,
                    });
                }
                cond
            } else {
                return Err(self.unexpected());
            };
            item.join = Some((kind, on));
            items.push(item);
        }
        Ok(items)
    }

    fn parse_from_item(
        &mut self,
        join: Option<(JoinKind, Option<Expr>)>,
    ) -> Result<FromItem, SqlError> {
        self.eat_kw("lateral");
        let source = if self.eat_sym("(") {
            let q = self.query()?;
            self.expect_sym(")")?;
            TableSource::Subquery(Box::new(q))
        } else {
            self.eat_kw("only");
            TableSource::Table(self.obj_name()?)
        };
        let alias = self.table_alias()?;
        if alias.is_some() && self.is_sym("(") {
            // column alias list: ignored
            self.ident_list()?;
        }
        Ok(FromItem {
            source,
            alias,
            join,
        })
    }

    pub fn expr(&mut self) -> Result<Expr, SqlError> {
        self.expr_bp(0, false)
    }

    /// Pratt parser. `in_default` stops at column-constraint keywords.
    fn expr_bp(&mut self, min_bp: u8, in_default: bool) -> Result<Expr, SqlError> {
        let mut lhs = self.prefix(in_default)?;
        loop {
            // Postfix / special operators.
            if self.is_sym("::") {
                self.pos += 1;
                let ty = self.data_type()?;
                lhs = Expr::Cast(Box::new(lhs), ty);
                continue;
            }
            if self.is_sym("[") {
                self.pos += 1;
                let key = self.expr()?;
                self.expect_sym("]")?;
                lhs = Expr::Subscript(Box::new(lhs), Box::new(key));
                continue;
            }
            let (op, bp): (String, u8) = match self.peek() {
                Some(Tok::Ident(s)) => {
                    let k = s.to_lowercase();
                    match k.as_str() {
                        "or" => ("or".into(), 1),
                        "and" => ("and".into(), 2),
                        "is" | "in" | "between" | "like" | "ilike" | "not" => (k, 4),
                        _ => break,
                    }
                }
                Some(Tok::Sym(s)) => match *s {
                    "=" | "<>" | "!=" | "<" | "<=" | ">" | ">=" => ((*s).to_owned(), 5),
                    "||" => ("||".into(), 6),
                    "+" | "-" => ((*s).to_owned(), 7),
                    "*" | "/" | "%" => ((*s).to_owned(), 8),
                    "->" | "->>" => ((*s).to_owned(), 9),
                    _ => break,
                },
                _ => break,
            };
            if bp < min_bp || (bp == min_bp && bp != 4) {
                break;
            }
            if in_default && matches!(op.as_str(), "not") {
                break;
            }
            self.pos += 1;
            lhs = match op.as_str() {
                "is" => {
                    let neg = self.eat_kw("not");
                    if self.eat_kw("null") {
                        Expr::IsNull(Box::new(lhs), neg)
                    } else if self.eat_kw("true") {
                        Expr::IsBool(Box::new(lhs), true, neg)
                    } else if self.eat_kw("false") {
                        Expr::IsBool(Box::new(lhs), false, neg)
                    } else if self.eat_kws(&["distinct", "from"]) {
                        let rhs = self.expr_bp(5, in_default)?;
                        let op = if neg {
                            "is not distinct"
                        } else {
                            "is distinct"
                        };
                        Expr::Binary(op.to_owned(), Box::new(lhs), Box::new(rhs))
                    } else {
                        return Err(self.unexpected());
                    }
                }
                "not" => {
                    if self.eat_kw("in") {
                        self.in_rhs(lhs, true)?
                    } else if self.eat_kw("between") {
                        self.between_rhs(lhs, true, in_default)?
                    } else if self.eat_kw("like") {
                        let rhs = self.expr_bp(5, in_default)?;
                        Expr::Like(Box::new(lhs), Box::new(rhs), true, false)
                    } else if self.eat_kw("ilike") {
                        let rhs = self.expr_bp(5, in_default)?;
                        Expr::Like(Box::new(lhs), Box::new(rhs), true, true)
                    } else {
                        return Err(self.unexpected());
                    }
                }
                "in" => self.in_rhs(lhs, false)?,
                "between" => self.between_rhs(lhs, false, in_default)?,
                "like" | "ilike" => {
                    let rhs = self.expr_bp(5, in_default)?;
                    Expr::Like(Box::new(lhs), Box::new(rhs), false, op == "ilike")
                }
                _ => {
                    // Quantified comparisons: = ANY(array)
                    if matches!(op.as_str(), "=" | "<>" | "!=")
                        && (self.is_kw("any") || self.is_kw("all"))
                    {
                        let all = self.is_kw("all");
                        self.pos += 1;
                        self.expect_sym("(")?;
                        let rhs = if self.is_kw("select") || self.is_kw("with") {
                            let q = self.query()?;
                            self.expect_sym(")")?;
                            Expr::InSelect(Box::new(lhs), Box::new(q), op != "=")
                        } else {
                            let arr = self.expr()?;
                            self.expect_sym(")")?;
                            let list = match arr {
                                Expr::Array(items) => items,
                                other => vec![Expr::Func {
                                    name: "__unnest".into(),
                                    args: vec![other],
                                    distinct: false,
                                    star: false,
                                    filter: None,
                                }],
                            };
                            Expr::InList(Box::new(lhs), list, op != "=" || all)
                        };
                        lhs = rhs;
                        continue;
                    }
                    let rhs = self.expr_bp(bp + 1, in_default)?;
                    Expr::Binary(op, Box::new(lhs), Box::new(rhs))
                }
            };
        }
        Ok(lhs)
    }

    fn in_rhs(&mut self, lhs: Expr, neg: bool) -> Result<Expr, SqlError> {
        self.expect_sym("(")?;
        if self.is_kw("select") || self.is_kw("with") {
            let q = self.query()?;
            self.expect_sym(")")?;
            return Ok(Expr::InSelect(Box::new(lhs), Box::new(q), neg));
        }
        let mut list = vec![self.expr()?];
        while self.eat_sym(",") {
            list.push(self.expr()?);
        }
        self.expect_sym(")")?;
        Ok(Expr::InList(Box::new(lhs), list, neg))
    }

    fn between_rhs(&mut self, lhs: Expr, neg: bool, in_default: bool) -> Result<Expr, SqlError> {
        self.eat_kw("symmetric");
        let lo = self.expr_bp(5, in_default)?;
        self.expect_kw("and")?;
        let hi = self.expr_bp(5, in_default)?;
        Ok(Expr::Between(
            Box::new(lhs),
            Box::new(lo),
            Box::new(hi),
            neg,
        ))
    }

    fn prefix(&mut self, in_default: bool) -> Result<Expr, SqlError> {
        let Some(tok) = self.peek().cloned() else {
            return Err(SqlError::syntax_end());
        };
        match tok {
            Tok::Num(n) => {
                self.pos += 1;
                Ok(Expr::Literal(number_value(&n)))
            }
            Tok::Str(s) => {
                self.pos += 1;
                Ok(Expr::Literal(Value::String(s)))
            }
            Tok::Param(n) => {
                self.pos += 1;
                Ok(Expr::Param(n))
            }
            Tok::Sym("(") => {
                self.pos += 1;
                if self.is_kw("select") || self.is_kw("with") {
                    let q = self.query()?;
                    self.expect_sym(")")?;
                    return Ok(Expr::Scalar(Box::new(q)));
                }
                let e = self.expr()?;
                if self.eat_sym(",") {
                    // Row constructor: treat as array for IN comparisons.
                    let mut items = vec![e];
                    loop {
                        items.push(self.expr()?);
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    self.expect_sym(")")?;
                    return Ok(Expr::Array(items));
                }
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Sym("-") => {
                self.pos += 1;
                let e = self.expr_bp(10, in_default)?;
                Ok(match e {
                    Expr::Literal(Value::Number(n)) => {
                        Expr::Literal(number_value(&format!("-{n}")))
                    }
                    other => Expr::Neg(Box::new(other)),
                })
            }
            Tok::Sym("+") => {
                self.pos += 1;
                self.expr_bp(10, in_default)
            }
            Tok::QIdent(_) => self.column_or_func(),
            Tok::Ident(word) => {
                let k = word.to_lowercase();
                match k.as_str() {
                    "null" => {
                        self.pos += 1;
                        Ok(Expr::Literal(Value::Null))
                    }
                    "true" => {
                        self.pos += 1;
                        Ok(Expr::Literal(Value::Bool(true)))
                    }
                    "false" => {
                        self.pos += 1;
                        Ok(Expr::Literal(Value::Bool(false)))
                    }
                    "not" => {
                        self.pos += 1;
                        if self.eat_kw("exists") {
                            self.expect_sym("(")?;
                            let q = self.query()?;
                            self.expect_sym(")")?;
                            return Ok(Expr::Exists(Box::new(q), true));
                        }
                        let e = self.expr_bp(3, in_default)?;
                        Ok(Expr::Not(Box::new(e)))
                    }
                    "exists" => {
                        self.pos += 1;
                        self.expect_sym("(")?;
                        let q = self.query()?;
                        self.expect_sym(")")?;
                        Ok(Expr::Exists(Box::new(q), false))
                    }
                    "case" => {
                        self.pos += 1;
                        let operand = if self.is_kw("when") {
                            None
                        } else {
                            Some(Box::new(self.expr()?))
                        };
                        let mut whens = Vec::new();
                        while self.eat_kw("when") {
                            let w = self.expr()?;
                            self.expect_kw("then")?;
                            let t = self.expr()?;
                            whens.push((w, t));
                        }
                        let otherwise = if self.eat_kw("else") {
                            Some(Box::new(self.expr()?))
                        } else {
                            None
                        };
                        self.expect_kw("end")?;
                        Ok(Expr::Case {
                            operand,
                            whens,
                            otherwise,
                        })
                    }
                    "interval" if matches!(self.peek_at(1), Some(Tok::Str(_))) => {
                        self.pos += 1;
                        let Some(Tok::Str(s)) = self.peek().cloned() else {
                            unreachable!()
                        };
                        self.pos += 1;
                        Ok(Expr::Interval(s))
                    }
                    "interval"
                        if matches!(self.peek_at(1), Some(Tok::Num(_)))
                            && matches!(self.peek_at(2), Some(Tok::Ident(_))) =>
                    {
                        // ClickHouse style: INTERVAL 1 HOUR
                        let (Some(Tok::Num(n)), Some(Tok::Ident(unit))) =
                            (self.peek_at(1).cloned(), self.peek_at(2).cloned())
                        else {
                            unreachable!()
                        };
                        self.pos += 3;
                        Ok(Expr::Interval(format!("{n} {unit}")))
                    }
                    "date" | "timestamp" | "timestamptz" | "time"
                        if matches!(self.peek_at(1), Some(Tok::Str(_))) =>
                    {
                        self.pos += 1;
                        let Some(Tok::Str(s)) = self.peek().cloned() else {
                            unreachable!()
                        };
                        self.pos += 1;
                        Ok(Expr::Cast(Box::new(Expr::Literal(Value::String(s))), k))
                    }
                    "array" if matches!(self.peek_at(1), Some(Tok::Sym("["))) => {
                        self.pos += 2;
                        let mut items = Vec::new();
                        if !self.is_sym("]") {
                            loop {
                                items.push(self.expr()?);
                                if !self.eat_sym(",") {
                                    break;
                                }
                            }
                        }
                        self.expect_sym("]")?;
                        Ok(Expr::Array(items))
                    }
                    "cast" if matches!(self.peek_at(1), Some(Tok::Sym("("))) => {
                        self.pos += 2;
                        let e = self.expr()?;
                        self.expect_kw("as")?;
                        let ty = self.data_type()?;
                        self.expect_sym(")")?;
                        Ok(Expr::Cast(Box::new(e), ty))
                    }
                    "extract" if matches!(self.peek_at(1), Some(Tok::Sym("("))) => {
                        self.pos += 2;
                        let field = self.ident()?;
                        self.expect_kw("from")?;
                        let e = self.expr()?;
                        self.expect_sym(")")?;
                        Ok(Expr::Func {
                            name: "date_part".into(),
                            args: vec![Expr::Literal(Value::String(field)), e],
                            distinct: false,
                            star: false,
                            filter: None,
                        })
                    }
                    "current_date" | "current_timestamp" | "localtimestamp" | "current_user"
                    | "session_user" | "current_schema" => {
                        self.pos += 1;
                        if self.is_sym("(") && self.is_sym_at(1, ")") {
                            self.pos += 2;
                        }
                        Ok(Expr::Func {
                            name: k,
                            args: Vec::new(),
                            distinct: false,
                            star: false,
                            filter: None,
                        })
                    }
                    _ if RESERVED.contains(&k.as_str()) && k != "left" && k != "right" => {
                        Err(self.unexpected())
                    }
                    _ => self.column_or_func(),
                }
            }
            _ => Err(self.unexpected()),
        }
    }

    fn is_sym_at(&self, n: usize, sym: &str) -> bool {
        matches!(self.peek_at(n), Some(Tok::Sym(s)) if *s == sym)
    }

    fn column_or_func(&mut self) -> Result<Expr, SqlError> {
        let first = self.ident()?;
        if self.is_sym("(") {
            return self.func_call(first);
        }
        if self.eat_sym(".") {
            let second = self.ident()?;
            if self.is_sym("(") {
                // schema-qualified function, e.g. pg_catalog.now() or extensions.uuid_generate_v4()
                return self.func_call(second);
            }
            if self.eat_sym(".") {
                // schema.table.column
                let third = self.ident()?;
                return Ok(Expr::Column(Some(second), third));
            }
            return Ok(Expr::Column(Some(first), second));
        }
        Ok(Expr::Column(None, first))
    }

    fn func_call(&mut self, name: String) -> Result<Expr, SqlError> {
        self.expect_sym("(")?;
        let mut args = Vec::new();
        let mut distinct = false;
        let mut star = false;
        if self.eat_sym("*") {
            star = true;
        } else if !self.is_sym(")") {
            distinct = self.eat_kw("distinct");
            loop {
                args.push(self.expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            if self.eat_kws(&["order", "by"]) {
                // ordered-set argument (string_agg(x, ',' order by x)): ignored
                self.skip_until(&[]);
            }
        }
        self.expect_sym(")")?;
        let filter = if self.is_kw("filter") && self.is_sym_at(1, "(") {
            self.pos += 2;
            self.expect_kw("where")?;
            let f = self.expr()?;
            self.expect_sym(")")?;
            Some(Box::new(f))
        } else {
            None
        };
        if self.eat_kw("over") {
            return Err(SqlError::new(
                "0A000",
                "window functions are not supported by this database".to_owned(),
            ));
        }
        Ok(Expr::Func {
            name,
            args,
            distinct,
            star,
            filter,
        })
    }
}

fn from_alias(item: &FromItem) -> String {
    item.alias.clone().unwrap_or_else(|| match &item.source {
        TableSource::Table(n) => n.name.clone(),
        TableSource::Subquery(_) => String::new(),
    })
}

pub fn number_value(n: &str) -> Value {
    if let Ok(i) = n.parse::<i64>() {
        return Value::from(i);
    }
    serde_json::from_str::<Value>(n)
        .ok()
        .filter(Value::is_number)
        .unwrap_or_else(|| Value::from(n.parse::<f64>().unwrap_or(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_select_with_join_group_order() {
        let stmts = parse_script(
            "select c.name, sum(o.total) as spent from orders o join customers c on c.id = o.customer_id \
             where o.status = 'paid' group by c.name order by spent desc limit 3;",
        )
        .unwrap();
        assert_eq!(stmts.len(), 1);
        let Stmt::Query(q) = &stmts[0] else { panic!() };
        assert!(q.limit.is_some());
        let SetBody::Select(s) = &q.body else {
            panic!()
        };
        assert_eq!(s.from.len(), 2);
        assert_eq!(s.group_by.len(), 1);
    }

    #[test]
    fn parses_dml_and_ddl() {
        let stmts = parse_script(
            "insert into t (a, b) values (1, 'x'), (2, 'y') returning *;\
             update t set b = 'z' where a = 1;\
             delete from t where a in (1, 2);\
             create table if not exists public.notes (id bigint generated by default as identity primary key, body text not null default '', created_at timestamptz default now());\
             alter table public.notes enable row level security;\
             create policy \"read own\" on public.notes for select to authenticated using (auth.uid() = owner);\
             create or replace function f() returns int language sql as $$ select 1 $$;",
        )
        .unwrap();
        assert_eq!(stmts.len(), 7);
        assert!(matches!(stmts[6], Stmt::Noop(_)));
        let Stmt::CreateTable { columns, .. } = &stmts[3] else {
            panic!()
        };
        assert_eq!(columns[0].identity.as_deref(), Some("BY DEFAULT"));
        assert_eq!(columns[2].default.as_deref(), Some("now()"));
    }

    #[test]
    fn syntax_errors_name_the_token() {
        let err = parse_script("selec 1").unwrap_err();
        assert_eq!(err.message, "syntax error at or near \"selec\"");
    }
}
