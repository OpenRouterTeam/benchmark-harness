//! Executes parsed statements against an in-memory [`Db`]. Results are JSON objects in
//! select-list order, as the Management API returns them (`json_agg` of the rows).

use std::{cmp::Ordering, collections::HashMap};

use mock_core::ids::IdGen;
use serde_json::{Map, Value, json};

use super::{
    ColumnInfo, ForeignKey, IndexDef, PolicyDef, SqlError, TableDef,
    parse::{
        AlterAction, ColumnDef, Expr, FromItem, JoinKind, ObjName, OnConflict, OrderItem, Query,
        Select, SelectItem, SetBody, Stmt, TableSource, parse_expr, parse_script,
    },
    value::{
        self, Interval, add_interval, as_f64, cast, compare, data_type_name, date_part, date_trunc,
        equal, format_date, format_interval, format_tstz, group_key, interval_between, number,
        parse_interval, parse_ts, scale_of, sort_cmp, to_char, to_text, type_format,
    },
};

#[derive(Debug, Clone)]
pub struct Table {
    pub def: TableDef,
    /// Values aligned with `def.columns`.
    pub rows: Vec<Vec<Value>>,
}

/// One project's database.
#[derive(Debug, Clone)]
pub struct Db {
    pub tables: Vec<Table>,
    /// `{name, schema, default_version, installed_version, comment}` (pg_available_extensions).
    pub extensions: Vec<Value>,
    /// Logical now, timestamptz text.
    pub now: String,
    pub ids: IdGen,
    pub next_oid: i64,
    pub dirty: bool,
}

impl Db {
    pub fn find(&self, schema: &str, name: &str) -> Option<usize> {
        self.tables
            .iter()
            .position(|t| t.def.schema == schema && t.def.name == name)
    }
}

/// Runs a script and returns the rows of its last statement.
pub fn execute(
    db: &mut Db,
    sql: &str,
    params: &[Value],
    read_only: bool,
) -> Result<Vec<Value>, SqlError> {
    let stmts = parse_script(sql)?;
    let mut out = Vec::new();
    for stmt in &stmts {
        if read_only && stmt.mutates() {
            return Err(SqlError::new(
                "25006",
                format!("cannot execute {} in a read-only transaction", stmt.verb()),
            ));
        }
        let mut ex = Exec {
            db: &mut *db,
            params,
            ctes: Vec::new(),
        };
        out = ex.statement(stmt)?;
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
struct Rel {
    header: Vec<(String, String)>,
    rows: Vec<Vec<Value>>,
    /// (qualifier, primary key columns) of base tables, for GROUP BY functional dependency.
    keys: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Copy)]
struct Env<'e> {
    header: &'e [(String, String)],
    row: &'e [Value],
    group: Option<&'e [Vec<Value>]>,
    parent: Option<&'e Env<'e>>,
}

const EMPTY_ENV: Env<'static> = Env {
    header: &[],
    row: &[],
    group: None,
    parent: None,
};

const AGGREGATES: [&str; 17] = [
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "array_agg",
    "string_agg",
    "json_agg",
    "jsonb_agg",
    "bool_and",
    "bool_or",
    "every",
    "group_concat",
    "countif",
    "uniq",
    "uniqexact",
    "any",
];

fn is_agg(name: &str) -> bool {
    AGGREGATES.contains(&name.to_lowercase().as_str())
}

fn contains_agg(e: &Expr) -> bool {
    match e {
        Expr::Func { name, args, .. } => is_agg(name) || args.iter().any(contains_agg),
        Expr::Neg(a)
        | Expr::Not(a)
        | Expr::IsNull(a, _)
        | Expr::IsBool(a, _, _)
        | Expr::Cast(a, _) => contains_agg(a),
        Expr::Binary(_, a, b) | Expr::Like(a, b, _, _) | Expr::Subscript(a, b) => {
            contains_agg(a) || contains_agg(b)
        }
        Expr::Between(a, b, c, _) => contains_agg(a) || contains_agg(b) || contains_agg(c),
        Expr::InList(a, l, _) => contains_agg(a) || l.iter().any(contains_agg),
        Expr::InSelect(a, _, _) => contains_agg(a),
        Expr::Case {
            operand,
            whens,
            otherwise,
        } => {
            operand.as_deref().is_some_and(contains_agg)
                || whens
                    .iter()
                    .any(|(w, t)| contains_agg(w) || contains_agg(t))
                || otherwise.as_deref().is_some_and(contains_agg)
        }
        Expr::Array(items) => items.iter().any(contains_agg),
        _ => false,
    }
}

fn interval_value(iv: Interval) -> Value {
    json!({ "__interval__": [iv.months, iv.days, iv.micros] })
}

fn as_interval(v: &Value) -> Option<Interval> {
    let arr = v.get("__interval__")?.as_array()?;
    Some(Interval {
        months: arr.first()?.as_i64()?,
        days: arr.get(1)?.as_i64()?,
        micros: arr.get(2)?.as_i64()?,
    })
}

/// Output form of a value (intervals become text).
fn output(v: Value) -> Value {
    match as_interval(&v) {
        Some(iv) => Value::String(format_interval(iv)),
        None => v,
    }
}

fn truthy(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}

fn pg_type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "unknown",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() => "integer",
        Value::Number(_) => "numeric",
        Value::String(_) => "text",
        Value::Array(_) => "anyarray",
        Value::Object(_) => "jsonb",
    }
}

fn like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    fn go(t: &[char], p: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('%') => (0..=t.len()).any(|i| go(&t[i..], &p[1..])),
            Some('_') => !t.is_empty() && go(&t[1..], &p[1..]),
            Some('\\') if p.len() > 1 => t.first() == Some(&p[1]) && go(&t[1..], &p[2..]),
            Some(c) => t.first() == Some(c) && go(&t[1..], &p[1..]),
        }
    }
    go(&t, &p)
}

fn expr_name(e: &Expr) -> String {
    match e {
        Expr::Column(_, n) => n.clone(),
        Expr::Func { name, .. } => match name.as_str() {
            "date_part" => "date_part".to_owned(),
            n => n.to_lowercase(),
        },
        Expr::Cast(inner, ty) => match inner.as_ref() {
            Expr::Literal(_) | Expr::Param(_) => {
                let f = type_format(ty);
                match f.as_str() {
                    "int4" | "int8" | "int2" | "bool" | "timestamptz" | "timestamp" | "float8" => f,
                    _ => ty.split('(').next().unwrap_or(ty).to_owned(),
                }
            }
            other => expr_name(other),
        },
        Expr::Case { .. } => "case".to_owned(),
        Expr::Subscript(inner, _) => expr_name(inner),
        Expr::Exists(..) => "exists".to_owned(),
        Expr::Array(_) => "array".to_owned(),
        Expr::Interval(_) => "interval".to_owned(),
        Expr::Scalar(q) => match &q.body {
            SetBody::Select(s) => match s.items.first() {
                Some(SelectItem::Expr(e, alias)) => alias.clone().unwrap_or_else(|| expr_name(e)),
                _ => "?column?".to_owned(),
            },
            _ => "?column?".to_owned(),
        },
        _ => "?column?".to_owned(),
    }
}

struct Exec<'a> {
    db: &'a mut Db,
    params: &'a [Value],
    ctes: Vec<(String, Rel)>,
}

type Output = (Vec<String>, Vec<Vec<Value>>);
/// Output plus per-row ORDER BY keys.
type SelectOutput = (Vec<String>, Vec<Vec<Value>>, Vec<Vec<Value>>);

impl Exec<'_> {
    fn statement(&mut self, stmt: &Stmt) -> Result<Vec<Value>, SqlError> {
        let (names, rows) = match stmt {
            Stmt::Query(q) => self.query(q, None)?,
            Stmt::Insert {
                table,
                columns,
                source,
                on_conflict,
                returning,
            } => self.insert(
                table,
                columns,
                source,
                on_conflict.as_ref(),
                returning.as_deref(),
            )?,
            Stmt::Update {
                table,
                alias,
                sets,
                from,
                filter,
                returning,
            } => self.update(
                table,
                alias.as_deref(),
                sets,
                from,
                filter.as_ref(),
                returning.as_deref(),
            )?,
            Stmt::Delete {
                table,
                alias,
                filter,
                returning,
            } => self.delete(
                table,
                alias.as_deref(),
                filter.as_ref(),
                returning.as_deref(),
            )?,
            other => {
                self.ddl(other)?;
                (Vec::new(), Vec::new())
            }
        };
        Ok(rows
            .into_iter()
            .map(|r| {
                let mut obj = Map::new();
                for (n, v) in names.iter().zip(r) {
                    obj.insert(n.clone(), output(v));
                }
                Value::Object(obj)
            })
            .collect())
    }

    // ───────────────────────── queries ─────────────────────────

    fn query(&mut self, q: &Query, parent: Option<&Env>) -> Result<Output, SqlError> {
        let saved = self.ctes.len();
        let result = self.query_inner(q, parent);
        self.ctes.truncate(saved);
        result
    }

    fn query_inner(&mut self, q: &Query, parent: Option<&Env>) -> Result<Output, SqlError> {
        for (name, cq) in &q.ctes {
            let (names, rows) = self.query(cq, parent)?;
            let header = names.into_iter().map(|n| (name.clone(), n)).collect();
            self.ctes.push((
                name.clone(),
                Rel {
                    header,
                    rows,
                    keys: Vec::new(),
                },
            ));
        }
        let (names, mut rows, keys) = match &q.body {
            SetBody::Select(s) => self.select(s, parent, &q.order_by)?,
            body => {
                let (names, rows) = self.set_body(body, parent)?;
                let mut keys = Vec::new();
                for r in &rows {
                    let mut k = Vec::new();
                    for o in &q.order_by {
                        let idx = match &o.expr {
                            Expr::Literal(Value::Number(n)) => n.as_u64().map(|i| i as usize - 1),
                            Expr::Column(None, c) => names.iter().position(|n| n == c),
                            _ => None,
                        }
                        .ok_or_else(|| {
                            SqlError::new(
                                "0A000",
                                "invalid UNION/INTERSECT/EXCEPT ORDER BY clause".to_owned(),
                            )
                        })?;
                        k.push(r.get(idx).cloned().unwrap_or(Value::Null));
                    }
                    keys.push(k);
                }
                (names, rows, keys)
            }
        };
        if !q.order_by.is_empty() {
            let mut idx: Vec<usize> = (0..rows.len()).collect();
            idx.sort_by(|&a, &b| {
                for (i, o) in q.order_by.iter().enumerate() {
                    let (x, y) = (&keys[a][i], &keys[b][i]);
                    let nulls_first = o.nulls_first.unwrap_or(o.desc);
                    let ord = match (x.is_null(), y.is_null()) {
                        (true, true) => Ordering::Equal,
                        (true, false) => {
                            if nulls_first {
                                Ordering::Less
                            } else {
                                Ordering::Greater
                            }
                        }
                        (false, true) => {
                            if nulls_first {
                                Ordering::Greater
                            } else {
                                Ordering::Less
                            }
                        }
                        _ => {
                            let c = sort_cmp(x, y);
                            if o.desc { c.reverse() } else { c }
                        }
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                Ordering::Equal
            });
            let mut sorted = Vec::with_capacity(rows.len());
            let mut taken: Vec<Option<Vec<Value>>> = rows.into_iter().map(Some).collect();
            for i in idx {
                sorted.push(taken[i].take().unwrap_or_default());
            }
            rows = sorted;
        }
        let env = parent.copied().unwrap_or(EMPTY_ENV);
        if let Some(off) = &q.offset {
            let n = self.int_arg(off, &env, "OFFSET")?;
            rows = rows.into_iter().skip(n).collect();
        }
        if let Some(lim) = &q.limit {
            let n = self.int_arg(lim, &env, "LIMIT")?;
            rows.truncate(n);
        }
        Ok((names, rows))
    }

    fn int_arg(&mut self, e: &Expr, env: &Env, what: &str) -> Result<usize, SqlError> {
        let v = self.eval(e, env)?;
        match as_f64(&v) {
            Some(x) if x >= 0.0 => Ok(x as usize),
            Some(_) => Err(SqlError::new(
                "2201W",
                format!("{what} must not be negative"),
            )),
            None => Err(SqlError::new(
                "22P02",
                format!(
                    "invalid input syntax for type bigint: \"{}\"",
                    to_text(&v).unwrap_or_default()
                ),
            )),
        }
    }

    fn set_body(&mut self, body: &SetBody, parent: Option<&Env>) -> Result<Output, SqlError> {
        match body {
            SetBody::Select(s) => {
                let (n, r, _) = self.select(s, parent, &[])?;
                Ok((n, r))
            }
            SetBody::Values(rows) => {
                let env = parent.copied().unwrap_or(EMPTY_ENV);
                let width = rows.first().map_or(0, Vec::len);
                let mut out = Vec::new();
                for r in rows {
                    if r.len() != width {
                        return Err(SqlError::new(
                            "42601",
                            "VALUES lists must all be the same length".to_owned(),
                        ));
                    }
                    let mut vals = Vec::new();
                    for e in r {
                        vals.push(self.eval(e, &env)?);
                    }
                    out.push(vals);
                }
                let names = (1..=width).map(|i| format!("column{i}")).collect();
                Ok((names, out))
            }
            SetBody::Union(a, b, all) => {
                let (names, mut rows) = self.set_body(a, parent)?;
                let (rnames, rrows) = self.set_body(b, parent)?;
                if rnames.len() != names.len() {
                    return Err(SqlError::new(
                        "42601",
                        "each UNION query must have the same number of columns".to_owned(),
                    ));
                }
                rows.extend(rrows);
                if !all {
                    let mut seen = std::collections::HashSet::new();
                    rows.retain(|r| seen.insert(r.iter().map(group_key).collect::<Vec<_>>()));
                }
                Ok((names, rows))
            }
        }
    }

    fn output_names(
        &self,
        items: &[SelectItem],
        header: &[(String, String)],
    ) -> Result<Vec<String>, SqlError> {
        let mut names = Vec::new();
        for item in items {
            match item {
                SelectItem::Wildcard => {
                    if header.is_empty() {
                        return Err(SqlError::new(
                            "42601",
                            "SELECT * with no tables specified is not valid".to_owned(),
                        ));
                    }
                    names.extend(header.iter().map(|(_, n)| n.clone()));
                }
                SelectItem::QualifiedWildcard(q) => {
                    let cols: Vec<String> = header
                        .iter()
                        .filter(|(hq, _)| hq == q)
                        .map(|(_, n)| n.clone())
                        .collect();
                    if cols.is_empty() {
                        return Err(SqlError::new(
                            "42P01",
                            format!("missing FROM-clause entry for table \"{q}\""),
                        ));
                    }
                    names.extend(cols);
                }
                SelectItem::Expr(e, alias) => {
                    names.push(alias.clone().unwrap_or_else(|| expr_name(e)))
                }
            }
        }
        Ok(names)
    }

    fn project(&mut self, items: &[SelectItem], env: &Env) -> Result<Vec<Value>, SqlError> {
        let mut vals = Vec::new();
        for item in items {
            match item {
                SelectItem::Wildcard => vals.extend(env.row.iter().cloned()),
                SelectItem::QualifiedWildcard(q) => {
                    for (i, (hq, _)) in env.header.iter().enumerate() {
                        if hq == q {
                            vals.push(env.row.get(i).cloned().unwrap_or(Value::Null));
                        }
                    }
                }
                SelectItem::Expr(e, _) => vals.push(self.eval(e, env)?),
            }
        }
        Ok(vals)
    }

    fn order_keys(
        &mut self,
        order: &[OrderItem],
        env: &Env,
        names: &[String],
        vals: &[Value],
    ) -> Result<Vec<Value>, SqlError> {
        let mut keys = Vec::new();
        for o in order {
            let v = match &o.expr {
                Expr::Literal(Value::Number(n)) if n.is_u64() => {
                    let i = n.as_u64().unwrap_or(0) as usize;
                    vals.get(i.wrapping_sub(1)).cloned().ok_or_else(|| {
                        SqlError::new(
                            "42P10",
                            format!("ORDER BY position {i} is not in select list"),
                        )
                    })?
                }
                Expr::Column(None, c) if names.contains(c) => {
                    vals[names.iter().position(|n| n == c).unwrap_or(0)].clone()
                }
                e => self.eval(e, env)?,
            };
            keys.push(v);
        }
        Ok(keys)
    }

    /// Replaces `GROUP BY 1` / `GROUP BY alias` with the select-list expression.
    fn resolve_group_expr(
        &self,
        g: &Expr,
        items: &[SelectItem],
        header: &[(String, String)],
    ) -> Expr {
        match g {
            Expr::Literal(Value::Number(n)) if n.is_u64() => {
                let i = n.as_u64().unwrap_or(0) as usize;
                match items.get(i.wrapping_sub(1)) {
                    Some(SelectItem::Expr(e, _)) => e.clone(),
                    _ => g.clone(),
                }
            }
            Expr::Column(None, c) if !header.iter().any(|(_, n)| n == c) => items
                .iter()
                .find_map(|it| match it {
                    SelectItem::Expr(e, Some(a)) if a == c => Some(e.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| g.clone()),
            _ => g.clone(),
        }
    }

    fn resolve_column<'h>(
        header: &'h [(String, String)],
        q: Option<&str>,
        n: &str,
    ) -> Option<&'h (String, String)> {
        header
            .iter()
            .find(|(hq, hn)| hn == n && q.is_none_or(|q| q == hq))
    }

    fn check_grouping(&self, e: &Expr, groups: &[Expr], rel: &Rel) -> Result<(), SqlError> {
        if groups.iter().any(|g| g == e) {
            return Ok(());
        }
        match e {
            Expr::Func { name, args, .. } => {
                if is_agg(name) {
                    return Ok(());
                }
                args.iter()
                    .try_for_each(|a| self.check_grouping(a, groups, rel))
            }
            Expr::Column(q, n) => {
                let Some((rq, rn)) = Self::resolve_column(&rel.header, q.as_deref(), n) else {
                    return Ok(());
                };
                let grouped = groups.iter().any(|g| match g {
                    Expr::Column(gq, gn) => Self::resolve_column(&rel.header, gq.as_deref(), gn)
                        .is_some_and(|(a, b)| a == rq && b == rn),
                    _ => false,
                });
                let by_key = rel.keys.iter().any(|(kq, pk)| {
                    kq == rq
                        && !pk.is_empty()
                        && pk.iter().all(|c| {
                            groups.iter().any(|g| match g {
                                Expr::Column(gq, gn) => {
                                    Self::resolve_column(&rel.header, gq.as_deref(), gn)
                                        .is_some_and(|(a, b)| a == kq && b == c)
                                }
                                _ => false,
                            })
                        })
                });
                if grouped || by_key {
                    Ok(())
                } else {
                    Err(SqlError::new(
                        "42803",
                        format!(
                            "column \"{}{rn}\" must appear in the GROUP BY clause or be used in an aggregate function",
                            if rq.is_empty() {
                                String::new()
                            } else {
                                format!("{rq}.")
                            }
                        ),
                    ))
                }
            }
            Expr::Neg(a)
            | Expr::Not(a)
            | Expr::IsNull(a, _)
            | Expr::IsBool(a, _, _)
            | Expr::Cast(a, _) => self.check_grouping(a, groups, rel),
            Expr::Binary(_, a, b) | Expr::Like(a, b, _, _) | Expr::Subscript(a, b) => {
                self.check_grouping(a, groups, rel)?;
                self.check_grouping(b, groups, rel)
            }
            Expr::Between(a, b, c, _) => {
                self.check_grouping(a, groups, rel)?;
                self.check_grouping(b, groups, rel)?;
                self.check_grouping(c, groups, rel)
            }
            Expr::InList(a, l, _) => {
                self.check_grouping(a, groups, rel)?;
                l.iter()
                    .try_for_each(|x| self.check_grouping(x, groups, rel))
            }
            Expr::Case {
                operand,
                whens,
                otherwise,
            } => {
                if let Some(o) = operand {
                    self.check_grouping(o, groups, rel)?;
                }
                for (w, t) in whens {
                    self.check_grouping(w, groups, rel)?;
                    self.check_grouping(t, groups, rel)?;
                }
                if let Some(o) = otherwise {
                    self.check_grouping(o, groups, rel)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn select(
        &mut self,
        s: &Select,
        parent: Option<&Env>,
        order: &[OrderItem],
    ) -> Result<SelectOutput, SqlError> {
        let rel = if s.from.is_empty() {
            Rel {
                header: Vec::new(),
                rows: vec![Vec::new()],
                keys: Vec::new(),
            }
        } else {
            self.from(&s.from, parent)?
        };
        let mut rows = Vec::new();
        for r in &rel.rows {
            if let Some(f) = &s.filter {
                if contains_agg(f) {
                    return Err(SqlError::new(
                        "42803",
                        "aggregate functions are not allowed in WHERE".to_owned(),
                    ));
                }
                let env = Env {
                    header: &rel.header,
                    row: r,
                    group: None,
                    parent,
                };
                if !truthy(&self.eval(f, &env)?) {
                    continue;
                }
            }
            rows.push(r.clone());
        }
        let names = self.output_names(&s.items, &rel.header)?;
        let group_exprs: Vec<Expr> = s
            .group_by
            .iter()
            .map(|g| self.resolve_group_expr(g, &s.items, &rel.header))
            .collect();
        let grouped = !group_exprs.is_empty()
            || s.having.is_some()
            || s.items
                .iter()
                .any(|i| matches!(i, SelectItem::Expr(e, _) if contains_agg(e)))
            || order.iter().any(|o| contains_agg(&o.expr));
        let mut out_rows = Vec::new();
        let mut keys = Vec::new();
        if grouped {
            for item in &s.items {
                match item {
                    SelectItem::Expr(e, _) => self.check_grouping(e, &group_exprs, &rel)?,
                    _ => {
                        for (q, n) in &rel.header {
                            self.check_grouping(
                                &Expr::Column(Some(q.clone()), n.clone()),
                                &group_exprs,
                                &rel,
                            )?;
                        }
                    }
                }
            }
            let mut order_of_groups: Vec<String> = Vec::new();
            let mut groups: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
            for r in rows {
                let env = Env {
                    header: &rel.header,
                    row: &r,
                    group: None,
                    parent,
                };
                let mut key = Vec::new();
                for g in &group_exprs {
                    key.push(group_key(&self.eval(g, &env)?));
                }
                let key = key.join("\u{1}");
                if !groups.contains_key(&key) {
                    order_of_groups.push(key.clone());
                }
                groups.entry(key).or_default().push(r);
            }
            if group_exprs.is_empty() && order_of_groups.is_empty() {
                order_of_groups.push(String::new());
                groups.insert(String::new(), Vec::new());
            }
            let null_row = vec![Value::Null; rel.header.len()];
            for key in &order_of_groups {
                let grows = &groups[key];
                let first = grows.first().unwrap_or(&null_row);
                let env = Env {
                    header: &rel.header,
                    row: first,
                    group: Some(grows),
                    parent,
                };
                if let Some(h) = &s.having
                    && !truthy(&self.eval(h, &env)?)
                {
                    continue;
                }
                let vals = self.project(&s.items, &env)?;
                let k = self.order_keys(order, &env, &names, &vals)?;
                out_rows.push(vals);
                keys.push(k);
            }
        } else {
            for r in &rows {
                let env = Env {
                    header: &rel.header,
                    row: r,
                    group: None,
                    parent,
                };
                let vals = self.project(&s.items, &env)?;
                let k = self.order_keys(order, &env, &names, &vals)?;
                out_rows.push(vals);
                keys.push(k);
            }
        }
        if s.distinct {
            let mut seen = std::collections::HashSet::new();
            let mut r2 = Vec::new();
            let mut k2 = Vec::new();
            for (r, k) in out_rows.into_iter().zip(keys) {
                if seen.insert(r.iter().map(group_key).collect::<Vec<_>>()) {
                    r2.push(r);
                    k2.push(k);
                }
            }
            out_rows = r2;
            keys = k2;
        }
        Ok((names, out_rows, keys))
    }

    fn from(&mut self, items: &[FromItem], parent: Option<&Env>) -> Result<Rel, SqlError> {
        let mut acc = self.scan(&items[0], parent)?;
        for item in &items[1..] {
            let right = self.scan(item, parent)?;
            let (kind, on) = item.join.clone().unwrap_or((JoinKind::Cross, None));
            let mut header = acc.header.clone();
            header.extend(right.header.iter().cloned());
            let mut rows = Vec::new();
            let mut right_matched = vec![false; right.rows.len()];
            for l in &acc.rows {
                let mut matched = false;
                for (ri, r) in right.rows.iter().enumerate() {
                    let mut combined = l.clone();
                    combined.extend(r.iter().cloned());
                    let ok = match &on {
                        Some(cond) => {
                            let env = Env {
                                header: &header,
                                row: &combined,
                                group: None,
                                parent,
                            };
                            truthy(&self.eval(cond, &env)?)
                        }
                        None => true,
                    };
                    if ok {
                        matched = true;
                        right_matched[ri] = true;
                        rows.push(combined);
                    }
                }
                if !matched && matches!(kind, JoinKind::Left | JoinKind::Full) {
                    let mut combined = l.clone();
                    combined.extend(std::iter::repeat_n(Value::Null, right.header.len()));
                    rows.push(combined);
                }
            }
            if matches!(kind, JoinKind::Right | JoinKind::Full) {
                for (ri, r) in right.rows.iter().enumerate() {
                    if !right_matched[ri] {
                        let mut combined = vec![Value::Null; acc.header.len()];
                        combined.extend(r.iter().cloned());
                        rows.push(combined);
                    }
                }
            }
            let mut keys = acc.keys.clone();
            keys.extend(right.keys);
            acc = Rel { header, rows, keys };
        }
        Ok(acc)
    }

    fn scan(&mut self, item: &FromItem, parent: Option<&Env>) -> Result<Rel, SqlError> {
        match &item.source {
            TableSource::Subquery(q) => {
                let (names, rows) = self.query(q, parent)?;
                let qual = item.alias.clone().unwrap_or_default();
                Ok(Rel {
                    header: names.into_iter().map(|n| (qual.clone(), n)).collect(),
                    rows,
                    keys: Vec::new(),
                })
            }
            TableSource::Table(name) => {
                let qual = item.alias.clone().unwrap_or_else(|| name.name.clone());
                if name.schema.is_none()
                    && let Some((_, rel)) = self.ctes.iter().rev().find(|(n, _)| *n == name.name)
                {
                    let mut rel = rel.clone();
                    for h in &mut rel.header {
                        h.0 = qual.clone();
                    }
                    return Ok(rel);
                }
                if let Some((cols, rows)) = self.virtual_table(name) {
                    return Ok(Rel {
                        header: cols
                            .into_iter()
                            .map(|c| (qual.clone(), c.to_owned()))
                            .collect(),
                        rows,
                        keys: Vec::new(),
                    });
                }
                let ti = self.table_idx(name)?;
                let t = &self.db.tables[ti];
                Ok(Rel {
                    header: t
                        .def
                        .columns
                        .iter()
                        .map(|c| (qual.clone(), c.name.clone()))
                        .collect(),
                    rows: t.rows.clone(),
                    keys: vec![(qual, t.def.primary_keys.clone())],
                })
            }
        }
    }

    fn table_idx(&self, name: &ObjName) -> Result<usize, SqlError> {
        let schema = name.schema.as_deref().unwrap_or("public");
        self.db.find(schema, &name.name).ok_or_else(|| {
            SqlError::new(
                "42P01",
                format!("relation \"{}\" does not exist", name.display()),
            )
        })
    }

    fn virtual_table(&self, name: &ObjName) -> Option<(Vec<&'static str>, Vec<Vec<Value>>)> {
        let schema = name.schema.as_deref();
        let tables = &self.db.tables;
        match (schema, name.name.as_str()) {
            (Some("information_schema"), "tables") => Some((
                vec!["table_catalog", "table_schema", "table_name", "table_type"],
                tables
                    .iter()
                    .map(|t| {
                        vec![
                            json!("postgres"),
                            json!(t.def.schema),
                            json!(t.def.name),
                            json!("BASE TABLE"),
                        ]
                    })
                    .collect(),
            )),
            (Some("information_schema"), "columns") => Some((
                vec![
                    "table_catalog",
                    "table_schema",
                    "table_name",
                    "column_name",
                    "ordinal_position",
                    "column_default",
                    "is_nullable",
                    "data_type",
                    "udt_name",
                ],
                tables
                    .iter()
                    .flat_map(|t| {
                        t.def.columns.iter().enumerate().map(|(i, c)| {
                            vec![
                                json!("postgres"),
                                json!(t.def.schema),
                                json!(t.def.name),
                                json!(c.name),
                                json!(i + 1),
                                c.default.clone().map_or(Value::Null, Value::String),
                                json!(if c.nullable { "YES" } else { "NO" }),
                                json!(data_type_name(&c.format)),
                                json!(c.format),
                            ]
                        })
                    })
                    .collect(),
            )),
            (None | Some("pg_catalog"), "pg_tables") => Some((
                vec![
                    "schemaname",
                    "tablename",
                    "tableowner",
                    "hasindexes",
                    "rowsecurity",
                ],
                tables
                    .iter()
                    .map(|t| {
                        vec![
                            json!(t.def.schema),
                            json!(t.def.name),
                            json!("postgres"),
                            json!(!t.def.primary_keys.is_empty() || !t.def.indexes.is_empty()),
                            json!(t.def.rls_enabled),
                        ]
                    })
                    .collect(),
            )),
            (None | Some("pg_catalog"), "pg_policies") => Some((
                vec![
                    "schemaname",
                    "tablename",
                    "policyname",
                    "permissive",
                    "roles",
                    "cmd",
                    "qual",
                    "with_check",
                ],
                tables
                    .iter()
                    .flat_map(|t| {
                        t.def.policies.iter().map(|p| {
                            vec![
                                json!(t.def.schema),
                                json!(t.def.name),
                                json!(p.name),
                                json!("PERMISSIVE"),
                                json!(p.roles),
                                json!(p.command),
                                p.using.clone().map_or(Value::Null, Value::String),
                                p.check.clone().map_or(Value::Null, Value::String),
                            ]
                        })
                    })
                    .collect(),
            )),
            (None | Some("pg_catalog"), "pg_indexes") => Some((
                vec!["schemaname", "tablename", "indexname", "indexdef"],
                tables
                    .iter()
                    .flat_map(|t| {
                        let pk = (!t.def.primary_keys.is_empty()).then(|| {
                            vec![
                                json!(t.def.schema),
                                json!(t.def.name),
                                json!(format!("{}_pkey", t.def.name)),
                                json!(format!(
                                    "CREATE UNIQUE INDEX {}_pkey ON {}.{} USING btree ({})",
                                    t.def.name,
                                    t.def.schema,
                                    t.def.name,
                                    t.def.primary_keys.join(", ")
                                )),
                            ]
                        });
                        pk.into_iter().chain(t.def.indexes.iter().map(|i| {
                            vec![
                                json!(t.def.schema),
                                json!(t.def.name),
                                json!(i.name),
                                json!(format!(
                                    "CREATE {}INDEX {} ON {}.{} USING btree ({})",
                                    if i.unique { "UNIQUE " } else { "" },
                                    i.name,
                                    t.def.schema,
                                    t.def.name,
                                    i.columns.join(", ")
                                )),
                            ]
                        }))
                    })
                    .collect(),
            )),
            (None | Some("pg_catalog"), "pg_extension") => Some((
                vec!["extname", "extversion"],
                self.db
                    .extensions
                    .iter()
                    .filter(|e| !e["installed_version"].is_null())
                    .map(|e| vec![e["name"].clone(), e["installed_version"].clone()])
                    .collect(),
            )),
            (None | Some("pg_catalog"), "pg_available_extensions") => Some((
                vec!["name", "default_version", "installed_version", "comment"],
                self.db
                    .extensions
                    .iter()
                    .map(|e| {
                        vec![
                            e["name"].clone(),
                            e["default_version"].clone(),
                            e["installed_version"].clone(),
                            e["comment"].clone(),
                        ]
                    })
                    .collect(),
            )),
            _ => None,
        }
    }

    // ───────────────────────── expressions ─────────────────────────

    fn lookup(&self, env: &Env, q: Option<&str>, n: &str) -> Result<Value, SqlError> {
        let mut frame = Some(env);
        let mut qual_seen = false;
        while let Some(f) = frame {
            let hits: Vec<usize> = f
                .header
                .iter()
                .enumerate()
                .filter(|(_, (hq, hn))| {
                    if q.is_some_and(|q| q == hq) {
                        qual_seen = true;
                    }
                    hn == n && q.is_none_or(|q| q == hq)
                })
                .map(|(i, _)| i)
                .collect();
            if hits.len() > 1 && q.is_none() {
                return Err(SqlError::new(
                    "42702",
                    format!("column reference \"{n}\" is ambiguous"),
                ));
            }
            if let Some(&i) = hits.first() {
                return Ok(f.row.get(i).cloned().unwrap_or(Value::Null));
            }
            frame = f.parent;
        }
        match q {
            Some(q) if !qual_seen => Err(SqlError::new(
                "42P01",
                format!("missing FROM-clause entry for table \"{q}\""),
            )),
            Some(q) => Err(SqlError::new(
                "42703",
                format!("column {q}.{n} does not exist"),
            )),
            None => Err(SqlError::new(
                "42703",
                format!("column \"{n}\" does not exist"),
            )),
        }
    }

    fn eval(&mut self, e: &Expr, env: &Env) -> Result<Value, SqlError> {
        Ok(match e {
            Expr::Column(q, n) => self.lookup(env, q.as_deref(), n)?,
            Expr::Literal(v) => v.clone(),
            Expr::Param(n) => self
                .params
                .get(n.wrapping_sub(1))
                .cloned()
                .ok_or_else(|| SqlError::new("42P02", format!("there is no parameter ${n}")))?,
            Expr::Neg(a) => {
                let v = self.eval(a, env)?;
                match &v {
                    Value::Null => Value::Null,
                    Value::Number(n) if n.is_i64() => Value::from(-n.as_i64().unwrap_or(0)),
                    _ => match as_f64(&v) {
                        Some(x) => number(-x, scale_of(&v)),
                        None => {
                            return Err(SqlError::new(
                                "42883",
                                format!("operator does not exist: - {}", pg_type_of(&v)),
                            ));
                        }
                    },
                }
            }
            Expr::Not(a) => match self.eval(a, env)? {
                Value::Bool(b) => Value::Bool(!b),
                Value::Null => Value::Null,
                other => {
                    return Err(SqlError::new(
                        "42804",
                        format!(
                            "argument of NOT must be type boolean, not type {}",
                            pg_type_of(&other)
                        ),
                    ));
                }
            },
            Expr::Binary(op, a, b) => self.binary(op, a, b, env)?,
            Expr::IsNull(a, neg) => Value::Bool(self.eval(a, env)?.is_null() != *neg),
            Expr::IsBool(a, want, neg) => {
                let v = self.eval(a, env)?;
                Value::Bool((v == Value::Bool(*want)) != *neg)
            }
            Expr::InList(a, list, neg) => {
                let v = self.eval(a, env)?;
                let mut candidates = Vec::new();
                for item in list {
                    match item {
                        Expr::Func { name, args, .. } if name == "__unnest" => {
                            match self.eval(&args[0], env)? {
                                Value::Array(items) => candidates.extend(items),
                                Value::Null => {}
                                other => candidates.push(other),
                            }
                        }
                        other => candidates.push(self.eval(other, env)?),
                    }
                }
                in_result(&v, &candidates, *neg)
            }
            Expr::InSelect(a, q, neg) => {
                let v = self.eval(a, env)?;
                let (_, rows) = self.query(q, Some(env))?;
                let candidates: Vec<Value> = rows
                    .into_iter()
                    .filter_map(|r| r.into_iter().next())
                    .collect();
                in_result(&v, &candidates, *neg)
            }
            Expr::Between(a, lo, hi, neg) => {
                let v = self.eval(a, env)?;
                let lo = self.eval(lo, env)?;
                let hi = self.eval(hi, env)?;
                match (compare(&v, &lo), compare(&v, &hi)) {
                    (Some(x), Some(y)) => {
                        Value::Bool((x != Ordering::Less && y != Ordering::Greater) != *neg)
                    }
                    _ => Value::Null,
                }
            }
            Expr::Like(a, p, neg, ci) => {
                let v = self.eval(a, env)?;
                let p = self.eval(p, env)?;
                match (to_text(&v), to_text(&p)) {
                    (Some(t), Some(p)) => {
                        let m = if *ci {
                            like_match(&t.to_lowercase(), &p.to_lowercase())
                        } else {
                            like_match(&t, &p)
                        };
                        Value::Bool(m != *neg)
                    }
                    _ => Value::Null,
                }
            }
            Expr::Func {
                name,
                args,
                distinct,
                star,
                filter,
            } => self.func(name, args, *distinct, *star, filter.as_deref(), env)?,
            Expr::Cast(a, ty) => {
                let v = self.eval(a, env)?;
                let f = type_format(ty);
                if f == "interval" {
                    if as_interval(&v).is_some() {
                        return Ok(v);
                    }
                    return Ok(interval_value(parse_interval(
                        &to_text(&v).unwrap_or_default(),
                    )?));
                }
                if matches!(f.as_str(), "timestamptz" | "timestamp" | "date")
                    && to_text(&v).is_some_and(|s| s.eq_ignore_ascii_case("now"))
                {
                    return cast(&Value::String(self.db.now.clone()), &f);
                }
                cast(&v, &f)?
            }
            Expr::Case {
                operand,
                whens,
                otherwise,
            } => {
                let op = match operand {
                    Some(o) => Some(self.eval(o, env)?),
                    None => None,
                };
                for (w, t) in whens {
                    let wv = self.eval(w, env)?;
                    let hit = match &op {
                        Some(o) => equal(o, &wv) == Some(true),
                        None => truthy(&wv),
                    };
                    if hit {
                        return self.eval(t, env);
                    }
                }
                match otherwise {
                    Some(o) => self.eval(o, env)?,
                    None => Value::Null,
                }
            }
            Expr::Subscript(a, k) => {
                let base = self.eval(a, env)?;
                let key = self.eval(k, env)?;
                match (&base, &key) {
                    (Value::Object(m), _) => m
                        .get(&to_text(&key).unwrap_or_default())
                        .cloned()
                        .unwrap_or(Value::Null),
                    (Value::Array(items), Value::Number(n)) => n
                        .as_u64()
                        .and_then(|i| items.get((i as usize).wrapping_sub(1)))
                        .cloned()
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                }
            }
            Expr::Exists(q, neg) => {
                let (_, rows) = self.query(q, Some(env))?;
                Value::Bool(rows.is_empty() == *neg)
            }
            Expr::Scalar(q) => {
                let (_, rows) = self.query(q, Some(env))?;
                if rows.len() > 1 {
                    return Err(SqlError::new(
                        "21000",
                        "more than one row returned by a subquery used as an expression".to_owned(),
                    ));
                }
                rows.into_iter()
                    .next()
                    .and_then(|r| r.into_iter().next())
                    .unwrap_or(Value::Null)
            }
            Expr::Interval(s) => interval_value(parse_interval(s)?),
            Expr::Array(items) => {
                let mut out = Vec::new();
                for i in items {
                    out.push(self.eval(i, env)?);
                }
                Value::Array(out)
            }
        })
    }

    fn binary(&mut self, op: &str, a: &Expr, b: &Expr, env: &Env) -> Result<Value, SqlError> {
        if op == "and" || op == "or" {
            let x = self.eval(a, env)?;
            let is_and = op == "and";
            if x == Value::Bool(!is_and) {
                return Ok(x);
            }
            let y = self.eval(b, env)?;
            return Ok(match (x, y) {
                (_, Value::Bool(v)) if v != is_and => Value::Bool(v),
                (Value::Bool(_), Value::Bool(v)) => Value::Bool(v),
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                (x, _) => {
                    return Err(SqlError::new(
                        "42804",
                        format!(
                            "argument of {} must be type boolean, not type {}",
                            op.to_uppercase(),
                            pg_type_of(&x)
                        ),
                    ));
                }
            });
        }
        let x = self.eval(a, env)?;
        let y = self.eval(b, env)?;
        match op {
            "is distinct" => return Ok(Value::Bool(!null_safe_eq(&x, &y))),
            "is not distinct" => return Ok(Value::Bool(null_safe_eq(&x, &y))),
            _ => {}
        }
        if x.is_null() || y.is_null() {
            return Ok(Value::Null);
        }
        Ok(match op {
            "=" | "<>" | "!=" | "<" | "<=" | ">" | ">=" => {
                let Some(ord) = compare(&x, &y) else {
                    return Ok(Value::Null);
                };
                Value::Bool(match op {
                    "=" => ord == Ordering::Equal,
                    "<>" | "!=" => ord != Ordering::Equal,
                    "<" => ord == Ordering::Less,
                    "<=" => ord != Ordering::Greater,
                    ">" => ord == Ordering::Greater,
                    _ => ord != Ordering::Less,
                })
            }
            "||" => match (&x, &y) {
                (Value::Array(p), Value::Array(q)) => {
                    Value::Array(p.iter().chain(q.iter()).cloned().collect())
                }
                (Value::Object(p), Value::Object(q)) => {
                    let mut m = p.clone();
                    for (k, v) in q {
                        m.insert(k.clone(), v.clone());
                    }
                    Value::Object(m)
                }
                _ => Value::String(format!(
                    "{}{}",
                    to_text(&x).unwrap_or_default(),
                    to_text(&y).unwrap_or_default()
                )),
            },
            "->" | "->>" => {
                let v = match (&x, &y) {
                    (Value::Object(m), _) => m
                        .get(&to_text(&y).unwrap_or_default())
                        .cloned()
                        .unwrap_or(Value::Null),
                    (Value::Array(items), Value::Number(n)) => n
                        .as_u64()
                        .and_then(|i| items.get(i as usize))
                        .cloned()
                        .unwrap_or(Value::Null),
                    (Value::String(s), _) => serde_json::from_str::<Value>(s)
                        .ok()
                        .and_then(|j| j.get(to_text(&y).unwrap_or_default()).cloned())
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                if op == "->>" {
                    match v {
                        Value::Null => Value::Null,
                        Value::String(s) => Value::String(s),
                        other => Value::String(other.to_string()),
                    }
                } else {
                    v
                }
            }
            "+" | "-" | "*" | "/" | "%" => self.arith(op, &x, &y)?,
            other => {
                return Err(SqlError::new(
                    "42883",
                    format!(
                        "operator does not exist: {} {other} {}",
                        pg_type_of(&x),
                        pg_type_of(&y)
                    ),
                ));
            }
        })
    }

    fn arith(&self, op: &str, x: &Value, y: &Value) -> Result<Value, SqlError> {
        // Timestamp arithmetic.
        let xt = x.as_str().and_then(parse_ts);
        let yt = y.as_str().and_then(parse_ts);
        if let (Some(t), Some(iv)) = (xt, as_interval(y)) {
            let sign = match op {
                "+" => 1,
                "-" => -1,
                _ => return Err(op_error(op, "timestamp with time zone", "interval")),
            };
            let date_only = x.as_str().is_some_and(|s| s.len() == 10);
            let r = add_interval(t, iv, sign);
            return Ok(Value::String(if date_only && iv.micros == 0 {
                format_date(r)
            } else {
                format_tstz(r)
            }));
        }
        if let (Some(iv), Some(t)) = (as_interval(x), yt)
            && op == "+"
        {
            return Ok(Value::String(format_tstz(add_interval(t, iv, 1))));
        }
        if let (Some(a), Some(b)) = (xt, yt)
            && op == "-"
        {
            let both_dates = x.as_str().is_some_and(|s| s.len() == 10)
                && y.as_str().is_some_and(|s| s.len() == 10);
            if both_dates {
                return Ok(Value::from((a - b).whole_days()));
            }
            return Ok(interval_value(interval_between(a, b)));
        }
        if let (Some(t), Value::Number(n)) = (xt, y)
            && x.as_str().is_some_and(|s| s.len() == 10)
            && (op == "+" || op == "-")
        {
            let days = n.as_i64().unwrap_or(0) * if op == "+" { 1 } else { -1 };
            return Ok(Value::String(format_date(t + time::Duration::days(days))));
        }
        if let (Some(a), Some(b)) = (as_interval(x), as_interval(y)) {
            let s = if op == "-" { -1 } else { 1 };
            return Ok(interval_value(Interval {
                months: a.months + s * b.months,
                days: a.days + s * b.days,
                micros: a.micros + s * b.micros,
            }));
        }
        let (Some(a), Some(b)) = (as_f64(x), as_f64(y)) else {
            return Err(op_error(op, pg_type_of(x), pg_type_of(y)));
        };
        let both_int = x.as_i64().is_some() && y.as_i64().is_some();
        if both_int {
            let (p, q) = (x.as_i64().unwrap_or(0), y.as_i64().unwrap_or(0));
            return Ok(Value::from(match op {
                "+" => p.wrapping_add(q),
                "-" => p.wrapping_sub(q),
                "*" => p.wrapping_mul(q),
                "/" | "%" => {
                    if q == 0 {
                        return Err(SqlError::new("22012", "division by zero".to_owned()));
                    }
                    if op == "/" { p / q } else { p % q }
                }
                _ => 0,
            }));
        }
        let (sa, sb) = (scale_of(x), scale_of(y));
        Ok(match op {
            "+" => number(a + b, sa.max(sb)),
            "-" => number(a - b, sa.max(sb)),
            "*" => number(a * b, (sa + sb).min(12)),
            "/" => {
                if b == 0.0 {
                    return Err(SqlError::new("22012", "division by zero".to_owned()));
                }
                number(a / b, 15)
            }
            _ => {
                if b == 0.0 {
                    return Err(SqlError::new("22012", "division by zero".to_owned()));
                }
                number(a % b, sa.max(sb))
            }
        })
    }

    fn func(
        &mut self,
        name: &str,
        args: &[Expr],
        distinct: bool,
        star: bool,
        filter: Option<&Expr>,
        env: &Env,
    ) -> Result<Value, SqlError> {
        let lname = name.to_lowercase();
        if is_agg(&lname) {
            return self.aggregate(&lname, args, distinct, star, filter, env);
        }
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.eval(a, env)?);
        }
        let arg = |i: usize| vals.get(i).cloned().unwrap_or(Value::Null);
        let text = |i: usize| to_text(&arg(i));
        let ts_arg = |i: usize| text(i).and_then(|s| parse_ts(&s));
        let unknown = || {
            SqlError::new(
                "42883",
                format!(
                    "function {lname}({}) does not exist",
                    vals.iter().map(pg_type_of).collect::<Vec<_>>().join(", ")
                ),
            )
        };
        Ok(match lname.as_str() {
            "now"
            | "current_timestamp"
            | "localtimestamp"
            | "transaction_timestamp"
            | "statement_timestamp"
            | "clock_timestamp" => Value::String(self.db.now.clone()),
            "current_date" => Value::String(self.db.now[..10].to_owned()),
            "current_user" | "session_user" | "user" => json!("postgres"),
            "current_schema" => json!("public"),
            "current_database" => json!("postgres"),
            "version" => json!(
                "PostgreSQL 15.8 on aarch64-unknown-linux-gnu, compiled by gcc (GCC) 13.2.0, 64-bit"
            ),
            "gen_random_uuid" | "uuid_generate_v4" => {
                Value::String(self.db.ids.next_uuid("pg_uuid").to_string())
            }
            "__default" => Value::Null,
            "lower" => text(0).map_or(Value::Null, |s| Value::String(s.to_lowercase())),
            "upper" => text(0).map_or(Value::Null, |s| Value::String(s.to_uppercase())),
            "initcap" => text(0).map_or(Value::Null, |s| {
                Value::String(
                    s.split(' ')
                        .map(|w| {
                            let mut c = w.chars();
                            c.next().map_or(String::new(), |f| {
                                f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }),
            "length" | "char_length" | "character_length" => match arg(0) {
                Value::Null => Value::Null,
                Value::Array(a) => Value::from(a.len()),
                v => Value::from(to_text(&v).unwrap_or_default().chars().count()),
            },
            "octet_length" => text(0).map_or(Value::Null, |s| Value::from(s.len())),
            "trim" | "btrim" => text(0).map_or(Value::Null, |s| Value::String(s.trim().to_owned())),
            "ltrim" => text(0).map_or(Value::Null, |s| Value::String(s.trim_start().to_owned())),
            "rtrim" => text(0).map_or(Value::Null, |s| Value::String(s.trim_end().to_owned())),
            "replace" => match (text(0), text(1), text(2)) {
                (Some(s), Some(a), Some(b)) => Value::String(s.replace(&a, &b)),
                _ => Value::Null,
            },
            "concat" => Value::String(vals.iter().filter_map(to_text).collect()),
            "concat_ws" => {
                let sep = text(0).unwrap_or_default();
                Value::String(
                    vals.iter()
                        .skip(1)
                        .filter_map(to_text)
                        .collect::<Vec<_>>()
                        .join(&sep),
                )
            }
            "substring" | "substr" => match text(0) {
                Some(s) => {
                    let chars: Vec<char> = s.chars().collect();
                    let start = as_f64(&arg(1)).unwrap_or(1.0) as i64;
                    let len = as_f64(&arg(2)).map(|l| l as i64);
                    let from = (start - 1).max(0) as usize;
                    let end = match len {
                        Some(l) => ((start - 1 + l).max(0) as usize).min(chars.len()),
                        None => chars.len(),
                    };
                    Value::String(
                        chars
                            .get(from..end.max(from))
                            .map(|c| c.iter().collect())
                            .unwrap_or_default(),
                    )
                }
                None => Value::Null,
            },
            "left" | "right" => match text(0) {
                Some(s) => {
                    let n = as_f64(&arg(1)).unwrap_or(0.0) as i64;
                    let chars: Vec<char> = s.chars().collect();
                    let k = if n >= 0 {
                        (n as usize).min(chars.len())
                    } else {
                        chars.len().saturating_sub((-n) as usize)
                    };
                    Value::String(if lname == "left" {
                        chars[..k].iter().collect()
                    } else {
                        chars[chars.len() - k..].iter().collect()
                    })
                }
                None => Value::Null,
            },
            "split_part" => match (text(0), text(1)) {
                (Some(s), Some(d)) => {
                    let n = as_f64(&arg(2)).unwrap_or(1.0) as usize;
                    Value::String(
                        s.split(d.as_str())
                            .nth(n.saturating_sub(1))
                            .unwrap_or("")
                            .to_owned(),
                    )
                }
                _ => Value::Null,
            },
            "position" | "strpos" => match (text(0), text(1)) {
                (Some(s), Some(sub)) => {
                    Value::from(s.find(&sub).map_or(0, |i| s[..i].chars().count() + 1))
                }
                _ => Value::Null,
            },
            "starts_with" => match (text(0), text(1)) {
                (Some(s), Some(p)) => Value::Bool(s.starts_with(&p)),
                _ => Value::Null,
            },
            "md5" => text(0).map_or(Value::Null, |s| {
                use sha2::{Digest, Sha256};
                let d = Sha256::digest(s.as_bytes());
                Value::String(d.iter().take(16).map(|b| format!("{b:02x}")).collect())
            }),
            "coalesce" => vals
                .into_iter()
                .find(|v| !v.is_null())
                .unwrap_or(Value::Null),
            "nullif" => {
                if equal(&arg(0), &arg(1)) == Some(true) {
                    Value::Null
                } else {
                    arg(0)
                }
            }
            "greatest" | "least" => {
                let mut best: Option<Value> = None;
                for v in vals.into_iter().filter(|v| !v.is_null()) {
                    best = Some(match best {
                        None => v,
                        Some(b) => {
                            let ord = sort_cmp(&v, &b);
                            if (lname == "greatest" && ord == Ordering::Greater)
                                || (lname == "least" && ord == Ordering::Less)
                            {
                                v
                            } else {
                                b
                            }
                        }
                    });
                }
                best.unwrap_or(Value::Null)
            }
            "abs" => match arg(0) {
                Value::Null => Value::Null,
                v => match v.as_i64() {
                    Some(i) => Value::from(i.abs()),
                    None => number(as_f64(&v).ok_or_else(unknown)?.abs(), scale_of(&v)),
                },
            },
            "round" => match arg(0) {
                Value::Null => Value::Null,
                v => {
                    let x = as_f64(&v).ok_or_else(unknown)?;
                    let d = as_f64(&arg(1)).unwrap_or(0.0).max(0.0) as usize;
                    number(round_half_away(x, d), d)
                }
            },
            "trunc" => match arg(0) {
                Value::Null => Value::Null,
                v => {
                    let x = as_f64(&v).ok_or_else(unknown)?;
                    let d = as_f64(&arg(1)).unwrap_or(0.0).max(0.0) as i32;
                    let f = 10f64.powi(d);
                    number((x * f).trunc() / f, d as usize)
                }
            },
            "floor" | "ceil" | "ceiling" => match arg(0) {
                Value::Null => Value::Null,
                v => {
                    let x = as_f64(&v).ok_or_else(unknown)?;
                    number(
                        if lname == "floor" {
                            x.floor()
                        } else {
                            x.ceil()
                        },
                        0,
                    )
                }
            },
            "sqrt" => as_f64(&arg(0)).map_or(Value::Null, |x| number(x.sqrt(), 15)),
            "power" | "pow" => match (as_f64(&arg(0)), as_f64(&arg(1))) {
                (Some(a), Some(b)) => number(a.powf(b), 15),
                _ => Value::Null,
            },
            "mod" => self.arith("%", &arg(0), &arg(1))?,
            "date_trunc" => match (text(0), ts_arg(1)) {
                (Some(f), Some(t)) => {
                    Value::String(format_tstz(date_trunc(&f, t).ok_or_else(|| {
                        SqlError::new(
                            "22023",
                            format!(
                                "unit \"{f}\" not recognized for type timestamp with time zone"
                            ),
                        )
                    })?))
                }
                _ => Value::Null,
            },
            "date_part" | "extract" => match (text(0), ts_arg(1)) {
                (Some(f), Some(t)) => date_part(&f, t).ok_or_else(|| {
                    SqlError::new(
                        "22023",
                        format!("unit \"{f}\" not recognized for type timestamp with time zone"),
                    )
                })?,
                _ => Value::Null,
            },
            "to_char" => match (ts_arg(0), text(1)) {
                (Some(t), Some(p)) => Value::String(to_char(t, &p)),
                _ => match (as_f64(&arg(0)), text(1)) {
                    (Some(x), Some(_)) => Value::String(x.to_string()),
                    _ => Value::Null,
                },
            },
            "tostartofhour" | "tostartofday" | "tostartofminute" | "tostartofmonth"
            | "tostartofweek" => {
                let unit = lname.trim_start_matches("tostartof");
                match ts_arg(0).and_then(|t| date_trunc(unit, t)) {
                    Some(t) => Value::String(format_tstz(t)),
                    None => Value::Null,
                }
            }
            "todate" => ts_arg(0).map_or(Value::Null, |t| Value::String(format_date(t))),
            "todatetime" => ts_arg(0).map_or(Value::Null, |t| Value::String(format_tstz(t))),
            "tostring" => text(0).map_or(Value::Null, Value::String),
            "to_date" => ts_arg(0).map_or(Value::Null, |t| Value::String(format_date(t))),
            "to_timestamp" => match arg(0) {
                Value::Number(n) => {
                    time::OffsetDateTime::from_unix_timestamp(n.as_f64().unwrap_or(0.0) as i64)
                        .map_or(Value::Null, |t| Value::String(format_tstz(t)))
                }
                v => to_text(&v)
                    .and_then(|s| parse_ts(&s))
                    .map_or(Value::Null, |t| Value::String(format_tstz(t))),
            },
            "age" => match (ts_arg(0), ts_arg(1)) {
                (Some(a), Some(b)) => interval_value(interval_between(a, b)),
                (Some(a), None) => {
                    let now = parse_ts(&self.db.now).ok_or_else(unknown)?;
                    interval_value(interval_between(now, a))
                }
                _ => Value::Null,
            },
            "array_length" | "cardinality" => match arg(0) {
                Value::Array(a) => Value::from(a.len()),
                _ => Value::Null,
            },
            "array_to_string" => match arg(0) {
                Value::Array(a) => Value::String(
                    a.iter()
                        .filter_map(to_text)
                        .collect::<Vec<_>>()
                        .join(&text(1).unwrap_or_default()),
                ),
                _ => Value::Null,
            },
            "json_build_object" | "jsonb_build_object" => {
                let mut m = Map::new();
                for pair in vals.chunks(2) {
                    m.insert(
                        to_text(&pair[0]).unwrap_or_default(),
                        pair.get(1).cloned().map(output).unwrap_or(Value::Null),
                    );
                }
                Value::Object(m)
            }
            "to_json" | "to_jsonb" | "row_to_json" => arg(0),
            "jsonb_array_length" | "json_array_length" => match arg(0) {
                Value::Array(a) => Value::from(a.len()),
                _ => Value::Null,
            },
            "pg_size_pretty" => {
                as_f64(&arg(0)).map_or(Value::Null, |b| Value::String(size_pretty(b as i64)))
            }
            "pg_total_relation_size" | "pg_relation_size" | "pg_table_size" => {
                let name = text(0).unwrap_or_default();
                let (schema, table) = name.split_once('.').unwrap_or(("public", name.as_str()));
                match self.db.find(schema, table) {
                    Some(i) => Value::from(table_bytes(self.db.tables[i].rows.len())),
                    None => {
                        return Err(SqlError::new(
                            "42P01",
                            format!("relation \"{name}\" does not exist"),
                        ));
                    }
                }
            }
            "random" => json!(0.5),
            "nextval" => {
                return Err(SqlError::new(
                    "42P01",
                    format!(
                        "relation \"{}\" does not exist",
                        text(0).unwrap_or_default()
                    ),
                ));
            }
            _ => return Err(unknown()),
        })
    }

    fn aggregate(
        &mut self,
        name: &str,
        args: &[Expr],
        distinct: bool,
        star: bool,
        filter: Option<&Expr>,
        env: &Env,
    ) -> Result<Value, SqlError> {
        let Some(group) = env.group else {
            return Err(SqlError::new(
                "42803",
                "aggregate functions are not allowed in WHERE".to_owned(),
            ));
        };
        let mut vals = Vec::new();
        for row in group {
            let child = Env {
                header: env.header,
                row,
                group: None,
                parent: env.parent,
            };
            if let Some(f) = filter
                && !truthy(&self.eval(f, &child)?)
            {
                continue;
            }
            if star {
                vals.push(Value::Bool(true));
            } else if let Some(a) = args.first() {
                vals.push(self.eval(a, &child)?);
            } else {
                vals.push(Value::Bool(true));
            }
        }
        if distinct {
            let mut seen = std::collections::HashSet::new();
            vals.retain(|v| seen.insert(group_key(v)));
        }
        let non_null: Vec<&Value> = vals.iter().filter(|v| !v.is_null()).collect();
        Ok(match name {
            "count" => Value::from(non_null.len()),
            "countif" => Value::from(non_null.iter().filter(|v| truthy(v)).count()),
            "uniq" | "uniqexact" => {
                let mut seen = std::collections::HashSet::new();
                Value::from(
                    non_null
                        .iter()
                        .filter(|v| seen.insert(group_key(v)))
                        .count(),
                )
            }
            "any" => non_null.first().map_or(Value::Null, |v| (*v).clone()),
            "sum" => {
                if non_null.is_empty() {
                    return Ok(Value::Null);
                }
                if non_null.iter().all(|v| v.as_i64().is_some()) {
                    Value::from(non_null.iter().filter_map(|v| v.as_i64()).sum::<i64>())
                } else {
                    let scale = non_null.iter().map(|v| scale_of(v)).max().unwrap_or(0);
                    let mut total = 0.0;
                    for v in &non_null {
                        total += as_f64(v).ok_or_else(|| {
                            SqlError::new(
                                "42883",
                                format!("function sum({}) does not exist", pg_type_of(v)),
                            )
                        })?;
                    }
                    number(total, scale)
                }
            }
            "avg" => {
                if non_null.is_empty() {
                    return Ok(Value::Null);
                }
                let mut total = 0.0;
                for v in &non_null {
                    total += as_f64(v).ok_or_else(|| {
                        SqlError::new(
                            "42883",
                            format!("function avg({}) does not exist", pg_type_of(v)),
                        )
                    })?;
                }
                number(total / non_null.len() as f64, 15)
            }
            "min" | "max" => {
                let mut best: Option<&Value> = None;
                for v in non_null {
                    best = Some(match best {
                        None => v,
                        Some(b) => {
                            let ord = sort_cmp(v, b);
                            if (name == "max" && ord == Ordering::Greater)
                                || (name == "min" && ord == Ordering::Less)
                            {
                                v
                            } else {
                                b
                            }
                        }
                    });
                }
                best.cloned().unwrap_or(Value::Null)
            }
            "bool_and" | "every" => {
                if non_null.is_empty() {
                    Value::Null
                } else {
                    Value::Bool(non_null.iter().all(|v| truthy(v)))
                }
            }
            "bool_or" => {
                if non_null.is_empty() {
                    Value::Null
                } else {
                    Value::Bool(non_null.iter().any(|v| truthy(v)))
                }
            }
            "string_agg" | "group_concat" => {
                if non_null.is_empty() {
                    return Ok(Value::Null);
                }
                let sep = match args.get(1) {
                    Some(e) => to_text(&self.eval(e, env)?).unwrap_or_default(),
                    None => ",".to_owned(),
                };
                Value::String(
                    non_null
                        .iter()
                        .filter_map(|v| to_text(v))
                        .collect::<Vec<_>>()
                        .join(&sep),
                )
            }
            _ => {
                if vals.is_empty() {
                    Value::Null
                } else {
                    Value::Array(vals.into_iter().map(output).collect())
                }
            }
        })
    }

    // ───────────────────────── DML ─────────────────────────

    fn table_header(&self, ti: usize, qual: &str) -> Vec<(String, String)> {
        self.db.tables[ti]
            .def
            .columns
            .iter()
            .map(|c| (qual.to_owned(), c.name.clone()))
            .collect()
    }

    fn coerce(&self, v: &Value, col: &ColumnInfo) -> Result<Value, SqlError> {
        if v.is_null() {
            return Ok(Value::Null);
        }
        if !col.enums.is_empty() {
            let s = to_text(v).unwrap_or_default();
            if !col.enums.contains(&s) {
                return Err(SqlError::new(
                    "22P02",
                    format!("invalid input value for enum {}: \"{s}\"", col.format),
                ));
            }
            return Ok(Value::String(s));
        }
        if let Some(iv) = as_interval(v) {
            return Ok(Value::String(format_interval(iv)));
        }
        if matches!(col.format.as_str(), "timestamptz" | "timestamp" | "date")
            && to_text(v).is_some_and(|s| s.eq_ignore_ascii_case("now"))
        {
            return cast(&Value::String(self.db.now.clone()), &col.format);
        }
        cast(v, &col.format)
    }

    fn default_value(&mut self, ti: usize, ci: usize) -> Result<Value, SqlError> {
        let col = self.db.tables[ti].def.columns[ci].clone();
        let serial = col.identity.is_some()
            || col
                .default
                .as_deref()
                .is_some_and(|d| d == "nextval" || d.contains("nextval("));
        if serial {
            let max = self.db.tables[ti]
                .rows
                .iter()
                .filter_map(|r| r.get(ci).and_then(Value::as_i64))
                .max()
                .unwrap_or(0);
            return Ok(Value::from(max + 1));
        }
        match &col.default {
            Some(d) => {
                let e = parse_expr(d)?;
                let v = self.eval(&e, &EMPTY_ENV)?;
                self.coerce(&v, &col)
            }
            None => Ok(Value::Null),
        }
    }

    fn check_row(&self, ti: usize, row: &[Value]) -> Result<(), SqlError> {
        let def = &self.db.tables[ti].def;
        for (c, v) in def.columns.iter().zip(row) {
            if !c.nullable && v.is_null() {
                return Err(SqlError::new(
                    "23502",
                    format!(
                        "null value in column \"{}\" of relation \"{}\" violates not-null constraint",
                        c.name, def.name
                    ),
                ));
            }
        }
        for fk in &def.foreign_keys {
            let vals: Vec<Value> = fk
                .columns
                .iter()
                .map(|c| def.column_index(c).map_or(Value::Null, |i| row[i].clone()))
                .collect();
            if vals.iter().any(Value::is_null) {
                continue;
            }
            let (ts, tn) = fk
                .target
                .split_once('.')
                .unwrap_or(("public", fk.target.as_str()));
            let Some(tti) = self.db.find(ts, tn) else {
                continue;
            };
            let target = &self.db.tables[tti];
            let idx: Vec<Option<usize>> = fk
                .target_columns
                .iter()
                .map(|c| target.def.column_index(c))
                .collect();
            let found = target.rows.iter().any(|r| {
                idx.iter()
                    .zip(&vals)
                    .all(|(i, v)| i.is_some_and(|i| equal(&r[i], v) == Some(true)))
            });
            if !found {
                return Err(SqlError::new(
                    "23503",
                    format!(
                        "insert or update on table \"{}\" violates foreign key constraint \"{}\"",
                        def.name, fk.name
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The row index and constraint name a row would duplicate, if any.
    fn find_conflict(
        &self,
        ti: usize,
        row: &[Value],
        skip: Option<usize>,
    ) -> Option<(usize, String)> {
        let t = &self.db.tables[ti];
        let mut constraints: Vec<(String, Vec<usize>)> = Vec::new();
        if !t.def.primary_keys.is_empty() {
            constraints.push((
                format!("{}_pkey", t.def.name),
                t.def
                    .primary_keys
                    .iter()
                    .filter_map(|c| t.def.column_index(c))
                    .collect(),
            ));
        }
        for (i, c) in t.def.columns.iter().enumerate() {
            if c.unique {
                constraints.push((format!("{}_{}_key", t.def.name, c.name), vec![i]));
            }
        }
        for ix in t.def.indexes.iter().filter(|i| i.unique) {
            let cols: Vec<usize> = ix
                .columns
                .iter()
                .filter_map(|c| t.def.column_index(c))
                .collect();
            if cols.len() == ix.columns.len() {
                constraints.push((ix.name.clone(), cols));
            }
        }
        for (name, cols) in constraints {
            if cols.iter().any(|&c| row[c].is_null()) {
                continue;
            }
            for (ri, other) in t.rows.iter().enumerate() {
                if Some(ri) == skip {
                    continue;
                }
                if cols
                    .iter()
                    .all(|&c| equal(&other[c], &row[c]) == Some(true))
                {
                    return Some((ri, name));
                }
            }
        }
        None
    }

    fn duplicate_error(constraint: &str) -> SqlError {
        SqlError::new(
            "23505",
            format!("duplicate key value violates unique constraint \"{constraint}\""),
        )
    }

    fn returning(
        &mut self,
        ti: usize,
        qual: &str,
        items: Option<&[SelectItem]>,
        rows: &[Vec<Value>],
    ) -> Result<Output, SqlError> {
        let Some(items) = items else {
            return Ok((Vec::new(), Vec::new()));
        };
        let header = self.table_header(ti, qual);
        let names = self.output_names(items, &header)?;
        let mut out = Vec::new();
        for r in rows {
            let env = Env {
                header: &header,
                row: r,
                group: None,
                parent: None,
            };
            out.push(self.project(items, &env)?);
        }
        Ok((names, out))
    }

    fn insert(
        &mut self,
        table: &ObjName,
        columns: &[String],
        source: &Query,
        on_conflict: Option<&OnConflict>,
        returning: Option<&[SelectItem]>,
    ) -> Result<Output, SqlError> {
        let ti = self.table_idx(table)?;
        let def = self.db.tables[ti].def.clone();
        let targets: Vec<usize> = if columns.is_empty() {
            (0..def.columns.len()).collect()
        } else {
            columns
                .iter()
                .map(|c| {
                    def.column_index(c).ok_or_else(|| {
                        SqlError::new(
                            "42703",
                            format!("column \"{c}\" of relation \"{}\" does not exist", def.name),
                        )
                    })
                })
                .collect::<Result<_, _>>()?
        };
        let mut source_rows: Vec<Vec<Option<Value>>> = Vec::new();
        match &source.body {
            SetBody::Values(rows) if source.ctes.is_empty() && source.order_by.is_empty() => {
                for r in rows {
                    if r.len() > targets.len() {
                        return Err(SqlError::new(
                            "42601",
                            "INSERT has more expressions than target columns".to_owned(),
                        ));
                    }
                    if !r.is_empty() && r.len() < targets.len() && !columns.is_empty() {
                        return Err(SqlError::new(
                            "42601",
                            "INSERT has more target columns than expressions".to_owned(),
                        ));
                    }
                    let mut vals = Vec::new();
                    for e in r {
                        vals.push(match e {
                            Expr::Func { name, .. } if name == "__default" => None,
                            e => Some(self.eval(e, &EMPTY_ENV)?),
                        });
                    }
                    source_rows.push(vals);
                }
            }
            _ => {
                let (names, rows) = self.query(source, None)?;
                if names.len() > targets.len() {
                    return Err(SqlError::new(
                        "42601",
                        "INSERT has more expressions than target columns".to_owned(),
                    ));
                }
                source_rows = rows
                    .into_iter()
                    .map(|r| r.into_iter().map(Some).collect())
                    .collect();
            }
        }
        let qual = table.name.clone();
        let mut affected = Vec::new();
        for src in source_rows {
            let mut row = vec![Value::Null; def.columns.len()];
            let mut provided = vec![false; def.columns.len()];
            for (i, &ci) in targets.iter().enumerate() {
                if let Some(Some(v)) = src.get(i) {
                    if def.columns[ci].identity.as_deref() == Some("ALWAYS") {
                        return Err(SqlError::new(
                            "428C9",
                            format!(
                                "cannot insert a non-DEFAULT value into column \"{}\"",
                                def.columns[ci].name
                            ),
                        ));
                    }
                    row[ci] = self.coerce(v, &def.columns[ci])?;
                    provided[ci] = true;
                }
            }
            for ci in 0..def.columns.len() {
                if !provided[ci] {
                    row[ci] = self.default_value(ti, ci)?;
                }
            }
            self.check_row(ti, &row)?;
            if let Some((ri, constraint)) = self.find_conflict(ti, &row, None) {
                match on_conflict {
                    None => return Err(Self::duplicate_error(&constraint)),
                    Some(OnConflict { update: None, .. }) => continue,
                    Some(OnConflict {
                        update: Some(sets), ..
                    }) => {
                        let mut header = self.table_header(ti, &qual);
                        header.extend(
                            def.columns
                                .iter()
                                .map(|c| ("excluded".to_owned(), c.name.clone())),
                        );
                        let mut combined = self.db.tables[ti].rows[ri].clone();
                        combined.extend(row.iter().cloned());
                        let mut new = self.db.tables[ti].rows[ri].clone();
                        for (c, e) in sets {
                            let ci = def.column_index(c).ok_or_else(|| {
                                SqlError::new(
                                    "42703",
                                    format!(
                                        "column \"{c}\" of relation \"{}\" does not exist",
                                        def.name
                                    ),
                                )
                            })?;
                            let env = Env {
                                header: &header,
                                row: &combined,
                                group: None,
                                parent: None,
                            };
                            let v = self.eval(e, &env)?;
                            new[ci] = self.coerce(&v, &def.columns[ci])?;
                        }
                        self.check_row(ti, &new)?;
                        self.db.tables[ti].rows[ri] = new.clone();
                        affected.push(new);
                        continue;
                    }
                }
            }
            self.db.tables[ti].rows.push(row.clone());
            affected.push(row);
        }
        self.db.dirty = true;
        self.returning(ti, &qual, returning, &affected)
    }

    fn update(
        &mut self,
        table: &ObjName,
        alias: Option<&str>,
        sets: &[(String, Expr)],
        from: &[FromItem],
        filter: Option<&Expr>,
        returning: Option<&[SelectItem]>,
    ) -> Result<Output, SqlError> {
        let ti = self.table_idx(table)?;
        let def = self.db.tables[ti].def.clone();
        let qual = alias.unwrap_or(&table.name).to_owned();
        let set_idx: Vec<(usize, &Expr)> = sets
            .iter()
            .map(|(c, e)| {
                def.column_index(c).map(|i| (i, e)).ok_or_else(|| {
                    SqlError::new(
                        "42703",
                        format!("column \"{c}\" of relation \"{}\" does not exist", def.name),
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        let from_rel = if from.is_empty() {
            None
        } else {
            Some(self.from(from, None)?)
        };
        let mut header = self.table_header(ti, &qual);
        if let Some(f) = &from_rel {
            header.extend(f.header.iter().cloned());
        }
        let rows = self.db.tables[ti].rows.clone();
        let mut changes = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            let combos: Vec<Vec<Value>> = match &from_rel {
                Some(f) => f
                    .rows
                    .iter()
                    .map(|fr| {
                        let mut c = row.clone();
                        c.extend(fr.iter().cloned());
                        c
                    })
                    .collect(),
                None => vec![row.clone()],
            };
            for combined in combos {
                let env = Env {
                    header: &header,
                    row: &combined,
                    group: None,
                    parent: None,
                };
                if let Some(f) = filter
                    && !truthy(&self.eval(f, &env)?)
                {
                    continue;
                }
                let mut new = row.clone();
                for (ci, e) in &set_idx {
                    let v = match e {
                        Expr::Func { name, .. } if name == "__default" => {
                            self.default_value(ti, *ci)?
                        }
                        e => {
                            let v = self.eval(e, &env)?;
                            self.coerce(&v, &def.columns[*ci])?
                        }
                    };
                    new[*ci] = v;
                }
                changes.push((ri, new));
                break;
            }
        }
        let mut affected = Vec::new();
        for (ri, new) in changes {
            self.check_row(ti, &new)?;
            self.db.tables[ti].rows[ri] = new.clone();
            if let Some((_, constraint)) = self.find_conflict(ti, &new, Some(ri)) {
                return Err(Self::duplicate_error(&constraint));
            }
            affected.push(new);
        }
        self.db.dirty = true;
        self.returning(ti, &qual, returning, &affected)
    }

    fn delete(
        &mut self,
        table: &ObjName,
        alias: Option<&str>,
        filter: Option<&Expr>,
        returning: Option<&[SelectItem]>,
    ) -> Result<Output, SqlError> {
        let ti = self.table_idx(table)?;
        let qual = alias.unwrap_or(&table.name).to_owned();
        let header = self.table_header(ti, &qual);
        let rows = self.db.tables[ti].rows.clone();
        let mut doomed = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            if let Some(f) = filter {
                let env = Env {
                    header: &header,
                    row,
                    group: None,
                    parent: None,
                };
                if !truthy(&self.eval(f, &env)?) {
                    continue;
                }
            }
            doomed.push(ri);
        }
        let deleted: Vec<Vec<Value>> = doomed.iter().map(|&i| rows[i].clone()).collect();
        self.delete_rows(ti, &doomed)?;
        self.db.dirty = true;
        self.returning(ti, &qual, returning, &deleted)
    }

    /// Deletes rows, enforcing (or cascading) foreign keys that reference them.
    fn delete_rows(&mut self, ti: usize, doomed: &[usize]) -> Result<(), SqlError> {
        if doomed.is_empty() {
            return Ok(());
        }
        let target = self.db.tables[ti].def.qualified();
        let target_name = self.db.tables[ti].def.name.clone();
        for oi in 0..self.db.tables.len() {
            let fks: Vec<ForeignKey> = self.db.tables[oi]
                .def
                .foreign_keys
                .iter()
                .filter(|fk| {
                    fk.target == target
                        || (fk.target == target_name && target.starts_with("public."))
                })
                .cloned()
                .collect();
            for fk in fks {
                let tcols: Vec<usize> = fk
                    .target_columns
                    .iter()
                    .filter_map(|c| self.db.tables[ti].def.column_index(c))
                    .collect();
                let scols: Vec<usize> = fk
                    .columns
                    .iter()
                    .filter_map(|c| self.db.tables[oi].def.column_index(c))
                    .collect();
                let keys: Vec<Vec<Value>> = doomed
                    .iter()
                    .map(|&d| {
                        tcols
                            .iter()
                            .map(|&c| self.db.tables[ti].rows[d][c].clone())
                            .collect()
                    })
                    .collect();
                let referencing: Vec<usize> = self.db.tables[oi]
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(ri, r)| {
                        !(oi == ti && doomed.contains(ri))
                            && keys.iter().any(|k| {
                                scols
                                    .iter()
                                    .zip(k)
                                    .all(|(&c, v)| equal(&r[c], v) == Some(true))
                            })
                    })
                    .map(|(i, _)| i)
                    .collect();
                if referencing.is_empty() {
                    continue;
                }
                match fk.on_delete.as_deref().map(str::to_lowercase).as_deref() {
                    Some("cascade") => self.delete_rows(oi, &referencing)?,
                    Some("set null") => {
                        for &r in &referencing {
                            for &c in &scols {
                                self.db.tables[oi].rows[r][c] = Value::Null;
                            }
                        }
                    }
                    _ => {
                        return Err(SqlError::new(
                            "23503",
                            format!(
                                "update or delete on table \"{}\" violates foreign key constraint \"{}\" on table \"{}\"",
                                target_name, fk.name, self.db.tables[oi].def.name
                            ),
                        ));
                    }
                }
            }
        }
        let mut sorted = doomed.to_vec();
        sorted.sort_unstable();
        for &i in sorted.iter().rev() {
            self.db.tables[ti].rows.remove(i);
        }
        Ok(())
    }

    // ───────────────────────── DDL ─────────────────────────

    fn column_info(&self, c: &ColumnDef) -> ColumnInfo {
        let format = type_format(&c.data_type);
        let enums = self
            .db
            .tables
            .iter()
            .flat_map(|t| t.def.columns.iter())
            .find(|col| col.format == format && !col.enums.is_empty())
            .map(|col| col.enums.clone())
            .unwrap_or_default();
        ColumnInfo {
            name: c.name.clone(),
            format,
            nullable: !(c.not_null || c.primary_key),
            default: c.default.clone(),
            identity: c.identity.clone(),
            unique: c.unique,
            enums,
            check: c.check.clone(),
            comment: None,
        }
    }

    fn ddl(&mut self, stmt: &Stmt) -> Result<(), SqlError> {
        match stmt {
            Stmt::CreateTable {
                name,
                if_not_exists,
                columns,
                primary_key,
                foreign_keys,
                uniques,
            } => {
                let schema = name.schema.clone().unwrap_or_else(|| "public".to_owned());
                if self.db.find(&schema, &name.name).is_some() {
                    if *if_not_exists {
                        return Ok(());
                    }
                    return Err(SqlError::new(
                        "42P07",
                        format!("relation \"{}\" already exists", name.name),
                    ));
                }
                let mut pk: Vec<String> = columns
                    .iter()
                    .filter(|c| c.primary_key)
                    .map(|c| c.name.clone())
                    .collect();
                if !primary_key.is_empty() {
                    pk = primary_key.clone();
                }
                let mut fks = Vec::new();
                for c in columns {
                    if let Some((target, tcol)) = &c.references {
                        fks.push(ForeignKey {
                            name: format!("{}_{}_fkey", name.name, c.name),
                            columns: vec![c.name.clone()],
                            target: format!(
                                "{}.{}",
                                target.schema.clone().unwrap_or_else(|| "public".into()),
                                target.name
                            ),
                            target_columns: vec![tcol.clone().unwrap_or_else(|| "id".to_owned())],
                            on_delete: c.on_delete.clone(),
                        });
                    }
                }
                for (cname, cols, target, tcols) in foreign_keys {
                    fks.push(ForeignKey {
                        name: cname
                            .clone()
                            .unwrap_or_else(|| format!("{}_{}_fkey", name.name, cols.join("_"))),
                        columns: cols.clone(),
                        target: format!(
                            "{}.{}",
                            target.schema.clone().unwrap_or_else(|| "public".into()),
                            target.name
                        ),
                        target_columns: tcols.clone(),
                        on_delete: None,
                    });
                }
                let mut infos: Vec<ColumnInfo> =
                    columns.iter().map(|c| self.column_info(c)).collect();
                for col in &mut infos {
                    if pk.contains(&col.name) {
                        col.nullable = false;
                    }
                }
                let indexes = uniques
                    .iter()
                    .map(|cols| IndexDef {
                        name: format!("{}_{}_key", name.name, cols.join("_")),
                        columns: cols.clone(),
                        unique: true,
                    })
                    .collect();
                let id = self.db.next_oid;
                self.db.next_oid += 1;
                self.db.tables.push(Table {
                    def: TableDef {
                        id,
                        schema,
                        name: name.name.clone(),
                        rls_enabled: false,
                        rls_forced: false,
                        comment: None,
                        columns: infos,
                        primary_keys: pk,
                        foreign_keys: fks,
                        indexes,
                        policies: Vec::new(),
                    },
                    rows: Vec::new(),
                });
            }
            Stmt::AlterTable {
                name,
                if_exists,
                actions,
            } => {
                let ti = match self.table_idx(name) {
                    Ok(ti) => ti,
                    Err(_) if *if_exists => return Ok(()),
                    Err(e) => return Err(e),
                };
                for action in actions {
                    self.alter(ti, action)?;
                }
            }
            Stmt::DropTable {
                names,
                if_exists,
                cascade,
            } => {
                for name in names {
                    let ti = match self.table_idx(name) {
                        Ok(ti) => ti,
                        Err(_) if *if_exists => continue,
                        Err(e) => return Err(e),
                    };
                    let q = self.db.tables[ti].def.qualified();
                    let dependents: Vec<usize> = (0..self.db.tables.len())
                        .filter(|&i| {
                            i != ti
                                && self.db.tables[i]
                                    .def
                                    .foreign_keys
                                    .iter()
                                    .any(|f| f.target == q)
                        })
                        .collect();
                    if !dependents.is_empty() && !cascade {
                        return Err(SqlError::new(
                            "2BP01",
                            format!(
                                "cannot drop table {} because other objects depend on it",
                                self.db.tables[ti].def.name
                            ),
                        ));
                    }
                    for i in dependents {
                        self.db.tables[i].def.foreign_keys.retain(|f| f.target != q);
                    }
                    self.db.tables.remove(ti);
                }
            }
            Stmt::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            } => {
                let ti = self.table_idx(table)?;
                let def = &self.db.tables[ti].def;
                let cols: Vec<String> = columns
                    .iter()
                    .map(|c| {
                        c.split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_matches('"')
                            .to_lowercase()
                    })
                    .collect();
                let iname = name
                    .clone()
                    .unwrap_or_else(|| format!("{}_{}_idx", def.name, cols.join("_")));
                if def.indexes.iter().any(|i| i.name == iname) {
                    if *if_not_exists {
                        return Ok(());
                    }
                    return Err(SqlError::new(
                        "42P07",
                        format!("relation \"{iname}\" already exists"),
                    ));
                }
                self.db.tables[ti].def.indexes.push(IndexDef {
                    name: iname,
                    columns: cols,
                    unique: *unique,
                });
            }
            Stmt::DropIndex { names, if_exists } => {
                for n in names {
                    let mut found = false;
                    for t in &mut self.db.tables {
                        let before = t.def.indexes.len();
                        t.def.indexes.retain(|i| i.name != n.name);
                        found |= t.def.indexes.len() != before;
                    }
                    if !found && !if_exists {
                        return Err(SqlError::new(
                            "42704",
                            format!("index \"{}\" does not exist", n.name),
                        ));
                    }
                }
            }
            Stmt::CreatePolicy {
                name,
                table,
                command,
                roles,
                using,
                check,
            } => {
                let ti = self.table_idx(table)?;
                let def = &mut self.db.tables[ti].def;
                if def.policies.iter().any(|p| &p.name == name) {
                    return Err(SqlError::new(
                        "42710",
                        format!(
                            "policy \"{name}\" for table \"{}\" already exists",
                            def.name
                        ),
                    ));
                }
                def.policies.push(PolicyDef {
                    name: name.clone(),
                    command: command.clone(),
                    roles: roles.clone(),
                    using: using.clone(),
                    check: check.clone(),
                });
            }
            Stmt::DropPolicy {
                name,
                table,
                if_exists,
            } => {
                let ti = self.table_idx(table)?;
                let def = &mut self.db.tables[ti].def;
                let before = def.policies.len();
                def.policies.retain(|p| &p.name != name);
                if before == def.policies.len() && !if_exists {
                    return Err(SqlError::new(
                        "42704",
                        format!(
                            "policy \"{name}\" for table \"{}\" does not exist",
                            def.name
                        ),
                    ));
                }
            }
            Stmt::CreateExtension {
                name,
                schema,
                if_not_exists,
            } => {
                let Some(ext) = self
                    .db
                    .extensions
                    .iter_mut()
                    .find(|e| e["name"] == name.as_str())
                else {
                    return Err(SqlError::new(
                        "0A000",
                        format!("extension \"{name}\" is not available"),
                    ));
                };
                if !ext["installed_version"].is_null() {
                    if *if_not_exists {
                        return Ok(());
                    }
                    return Err(SqlError::new(
                        "42710",
                        format!("extension \"{name}\" already exists"),
                    ));
                }
                ext["installed_version"] = ext["default_version"].clone();
                ext["schema"] = json!(schema.clone().unwrap_or_else(|| "extensions".to_owned()));
            }
            Stmt::DropExtension { name, if_exists } => {
                match self
                    .db
                    .extensions
                    .iter_mut()
                    .find(|e| e["name"] == name.as_str() && !e["installed_version"].is_null())
                {
                    Some(ext) => {
                        ext["installed_version"] = Value::Null;
                        ext["schema"] = Value::Null;
                    }
                    None if *if_exists => {}
                    None => {
                        return Err(SqlError::new(
                            "42704",
                            format!("extension \"{name}\" does not exist"),
                        ));
                    }
                }
            }
            Stmt::Comment {
                table,
                column,
                text,
            } => {
                let ti = self.table_idx(table)?;
                let def = &mut self.db.tables[ti].def;
                match column {
                    Some(c) => {
                        let Some(i) = def.column_index(c) else {
                            return Err(SqlError::new(
                                "42703",
                                format!(
                                    "column \"{c}\" of relation \"{}\" does not exist",
                                    def.name
                                ),
                            ));
                        };
                        def.columns[i].comment = text.clone();
                    }
                    None => def.comment = text.clone(),
                }
            }
            Stmt::Truncate(names) => {
                for n in names {
                    let ti = self.table_idx(n)?;
                    self.db.tables[ti].rows.clear();
                }
            }
            _ => return Ok(()),
        }
        self.db.dirty = true;
        Ok(())
    }

    fn alter(&mut self, ti: usize, action: &AlterAction) -> Result<(), SqlError> {
        let tname = self.db.tables[ti].def.name.clone();
        let no_column = |c: &str| {
            SqlError::new(
                "42703",
                format!("column \"{c}\" of relation \"{tname}\" does not exist"),
            )
        };
        match action {
            AlterAction::SetRls(on) => self.db.tables[ti].def.rls_enabled = *on,
            AlterAction::ForceRls(on) => self.db.tables[ti].def.rls_forced = *on,
            AlterAction::AddColumn(c, if_not_exists) => {
                if self.db.tables[ti].def.column_index(&c.name).is_some() {
                    if *if_not_exists {
                        return Ok(());
                    }
                    return Err(SqlError::new(
                        "42701",
                        format!(
                            "column \"{}\" of relation \"{tname}\" already exists",
                            c.name
                        ),
                    ));
                }
                let info = self.column_info(c);
                self.db.tables[ti].def.columns.push(info.clone());
                let ci = self.db.tables[ti].def.columns.len() - 1;
                for ri in 0..self.db.tables[ti].rows.len() {
                    let v = if info.identity.is_some() || info.default.as_deref() == Some("nextval")
                    {
                        Value::from(ri as i64 + 1)
                    } else {
                        match &info.default {
                            Some(d) => {
                                let e = parse_expr(d)?;
                                let v = self.eval(&e, &EMPTY_ENV)?;
                                self.coerce(&v, &info)?
                            }
                            None => Value::Null,
                        }
                    };
                    if v.is_null() && !info.nullable {
                        return Err(SqlError::new(
                            "23502",
                            format!(
                                "column \"{}\" of relation \"{tname}\" contains null values",
                                c.name
                            ),
                        ));
                    }
                    self.db.tables[ti].rows[ri].push(v);
                }
                let _ = ci;
                if let Some((target, tcol)) = &c.references {
                    self.db.tables[ti].def.foreign_keys.push(ForeignKey {
                        name: format!("{tname}_{}_fkey", c.name),
                        columns: vec![c.name.clone()],
                        target: format!(
                            "{}.{}",
                            target.schema.clone().unwrap_or_else(|| "public".into()),
                            target.name
                        ),
                        target_columns: vec![tcol.clone().unwrap_or_else(|| "id".to_owned())],
                        on_delete: None,
                    });
                }
                if c.primary_key {
                    self.db.tables[ti].def.primary_keys = vec![c.name.clone()];
                }
            }
            AlterAction::DropColumn(c, if_exists) => {
                let Some(ci) = self.db.tables[ti].def.column_index(c) else {
                    if *if_exists {
                        return Ok(());
                    }
                    return Err(no_column(c));
                };
                let t = &mut self.db.tables[ti];
                t.def.columns.remove(ci);
                for r in &mut t.rows {
                    r.remove(ci);
                }
                t.def.primary_keys.retain(|k| k != c);
                t.def.foreign_keys.retain(|f| !f.columns.contains(c));
                t.def.indexes.retain(|i| !i.columns.contains(c));
            }
            AlterAction::RenameColumn(from, to) => {
                let Some(ci) = self.db.tables[ti].def.column_index(from) else {
                    return Err(no_column(from));
                };
                let t = &mut self.db.tables[ti];
                t.def.columns[ci].name = to.clone();
                for k in &mut t.def.primary_keys {
                    if k == from {
                        *k = to.clone();
                    }
                }
            }
            AlterAction::RenameTo(to) => {
                let schema = self.db.tables[ti].def.schema.clone();
                let old = self.db.tables[ti].def.qualified();
                self.db.tables[ti].def.name = to.clone();
                let new = format!("{schema}.{to}");
                for t in &mut self.db.tables {
                    for f in &mut t.def.foreign_keys {
                        if f.target == old {
                            f.target = new.clone();
                        }
                    }
                }
            }
            AlterAction::SetDefault(c, d) => {
                let Some(ci) = self.db.tables[ti].def.column_index(c) else {
                    return Err(no_column(c));
                };
                self.db.tables[ti].def.columns[ci].default = d.clone();
            }
            AlterAction::SetNotNull(c, on) => {
                let Some(ci) = self.db.tables[ti].def.column_index(c) else {
                    return Err(no_column(c));
                };
                if *on && self.db.tables[ti].rows.iter().any(|r| r[ci].is_null()) {
                    return Err(SqlError::new(
                        "23502",
                        format!("column \"{c}\" of relation \"{tname}\" contains null values"),
                    ));
                }
                self.db.tables[ti].def.columns[ci].nullable = !on;
            }
            AlterAction::AddPrimaryKey(cols) => {
                for c in cols {
                    let ci = self.db.tables[ti]
                        .def
                        .column_index(c)
                        .ok_or_else(|| no_column(c))?;
                    self.db.tables[ti].def.columns[ci].nullable = false;
                }
                self.db.tables[ti].def.primary_keys = cols.clone();
            }
            AlterAction::AddForeignKey(cname, cols, target, tcols) => {
                let fk = ForeignKey {
                    name: cname
                        .clone()
                        .unwrap_or_else(|| format!("{tname}_{}_fkey", cols.join("_"))),
                    columns: cols.clone(),
                    target: format!(
                        "{}.{}",
                        target.schema.clone().unwrap_or_else(|| "public".into()),
                        target.name
                    ),
                    target_columns: tcols.clone(),
                    on_delete: None,
                };
                self.db.tables[ti].def.foreign_keys.push(fk);
                let rows = self.db.tables[ti].rows.clone();
                for r in &rows {
                    self.check_row(ti, r)?;
                }
            }
            AlterAction::AddUnique(cols) => {
                let name = format!("{tname}_{}_key", cols.join("_"));
                self.db.tables[ti].def.indexes.push(IndexDef {
                    name,
                    columns: cols.clone(),
                    unique: true,
                });
            }
            AlterAction::Other => {}
        }
        Ok(())
    }
}

fn op_error(op: &str, a: &str, b: &str) -> SqlError {
    SqlError::new("42883", format!("operator does not exist: {a} {op} {b}"))
}

fn null_safe_eq(a: &Value, b: &Value) -> bool {
    match (a.is_null(), b.is_null()) {
        (true, true) => true,
        (false, false) => equal(a, b) == Some(true),
        _ => false,
    }
}

fn in_result(v: &Value, candidates: &[Value], neg: bool) -> Value {
    if v.is_null() {
        return Value::Null;
    }
    let mut saw_null = false;
    for c in candidates {
        match equal(v, c) {
            Some(true) => return Value::Bool(!neg),
            None => saw_null = true,
            _ => {}
        }
    }
    if saw_null {
        Value::Null
    } else {
        Value::Bool(neg)
    }
}

fn round_half_away(x: f64, digits: usize) -> f64 {
    let f = 10f64.powi(digits as i32);
    let scaled = x * f;
    // Nudge to counter binary representation error (2.675 -> 2.68 like numeric).
    let nudged = scaled + scaled.signum() * 1e-9;
    nudged.round() / f
}

pub fn table_bytes(rows: usize) -> i64 {
    8192 * (2 + rows as i64 / 40)
}

pub fn size_pretty(bytes: i64) -> String {
    if bytes < 10 * 1024 {
        format!("{bytes} bytes")
    } else if bytes < 10 * 1024 * 1024 {
        format!("{} kB", (bytes + 512) / 1024)
    } else {
        format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
    }
}

/// Lets callers reuse the value helpers (e.g. for dataset coercion).
pub use value::rfc3339_to_tstz;
