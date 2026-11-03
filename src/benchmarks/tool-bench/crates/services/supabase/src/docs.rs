//! The docs Content API (`/docs/api/graphql`): a tiny GraphQL executor over a pinned docs
//! corpus. Root fields: `schema`, `searchDocs(query, limit)`, `error(code, service)` and
//! `errors(first, after, service, code)`. Results are projected onto the query's selection
//! sets (aliases, inline fragments, named fragments and `__typename` included).

use std::collections::HashMap;

use serde_json::{Map, Value, json};

pub const CONTENT_API_SCHEMA: &str = include_str!("../assets/content-api-schema.graphql");
const DOCS: &str = include_str!("../assets/docs.json");

#[derive(Debug, Clone)]
enum Sel {
    Field {
        alias: Option<String>,
        name: String,
        args: Vec<(String, Value)>,
        sels: Vec<Sel>,
    },
    Inline(Option<String>, Vec<Sel>),
    Spread(String),
}

struct P<'a> {
    toks: Vec<String>,
    pos: usize,
    vars: &'a Value,
}

fn lex(q: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = q.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == ',' {
            i += 1;
        } else if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '"' {
            let block = chars.get(i + 1) == Some(&'"') && chars.get(i + 2) == Some(&'"');
            let mut s = String::from("\"");
            i += if block { 3 } else { 1 };
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err("Syntax Error: Unterminated string.".to_owned());
                };
                if block
                    && ch == '"'
                    && chars.get(i + 1) == Some(&'"')
                    && chars.get(i + 2) == Some(&'"')
                {
                    i += 3;
                    break;
                }
                if !block && ch == '"' {
                    i += 1;
                    break;
                }
                if !block
                    && ch == '\\'
                    && let Some(&n) = chars.get(i + 1)
                {
                    {
                        s.push(match n {
                            'n' => '\n',
                            't' => '\t',
                            o => o,
                        });
                        i += 2;
                        continue;
                    }
                }
                s.push(ch);
                i += 1;
            }
            out.push(s);
        } else if c == '.' && chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
            out.push("...".to_owned());
            i += 3;
        } else if "{}():$!=[]@".contains(c) {
            out.push(c.to_string());
            i += 1;
        } else if c.is_alphanumeric() || c == '_' || c == '-' {
            let mut s = String::new();
            while i < chars.len()
                && (chars[i].is_alphanumeric()
                    || chars[i] == '_'
                    || chars[i] == '-'
                    || chars[i] == '.')
            {
                s.push(chars[i]);
                i += 1;
            }
            out.push(s);
        } else {
            return Err(format!("Syntax Error: Unexpected character: \"{c}\"."));
        }
    }
    Ok(out)
}

impl P<'_> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Result<String, String> {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or("Syntax Error: Expected Name, found <EOF>.")?;
        self.pos += 1;
        Ok(t)
    }

    fn expect(&mut self, t: &str) -> Result<(), String> {
        match self.peek() {
            Some(x) if x == t => {
                self.pos += 1;
                Ok(())
            }
            Some(x) => Err(format!("Syntax Error: Expected \"{t}\", found \"{x}\".")),
            None => Err(format!("Syntax Error: Expected \"{t}\", found <EOF>.")),
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        let t = self.next()?;
        if t == "$" {
            let name = self.next()?;
            return Ok(self.vars.get(&name).cloned().unwrap_or(Value::Null));
        }
        if t == "[" {
            let mut items = Vec::new();
            while self.peek() != Some("]") {
                items.push(self.value()?);
            }
            self.expect("]")?;
            return Ok(Value::Array(items));
        }
        if let Some(s) = t.strip_prefix('"') {
            return Ok(Value::String(s.to_owned()));
        }
        if let Ok(i) = t.parse::<i64>() {
            return Ok(Value::from(i));
        }
        Ok(match t.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "null" => Value::Null,
            _ => Value::String(t),
        })
    }

    fn selections(&mut self) -> Result<Vec<Sel>, String> {
        self.expect("{")?;
        let mut out = Vec::new();
        while self.peek() != Some("}") {
            if self.peek().is_none() {
                return Err("Syntax Error: Expected Name, found <EOF>.".to_owned());
            }
            if self.peek() == Some("...") {
                self.pos += 1;
                if self.peek() == Some("on") {
                    self.pos += 1;
                    let ty = self.next()?;
                    out.push(Sel::Inline(Some(ty), self.selections()?));
                } else if self.peek() == Some("{") {
                    out.push(Sel::Inline(None, self.selections()?));
                } else {
                    out.push(Sel::Spread(self.next()?));
                }
                continue;
            }
            let mut name = self.next()?;
            let mut alias = None;
            if self.peek() == Some(":") {
                self.pos += 1;
                alias = Some(name);
                name = self.next()?;
            }
            let mut args = Vec::new();
            if self.peek() == Some("(") {
                self.pos += 1;
                while self.peek() != Some(")") {
                    let k = self.next()?;
                    self.expect(":")?;
                    args.push((k, self.value()?));
                }
                self.expect(")")?;
            }
            while self.peek() == Some("@") {
                self.pos += 1;
                self.next()?;
                if self.peek() == Some("(") {
                    while self.peek() != Some(")") {
                        self.pos += 1;
                    }
                    self.pos += 1;
                }
            }
            let sels = if self.peek() == Some("{") {
                self.selections()?
            } else {
                Vec::new()
            };
            out.push(Sel::Field {
                alias,
                name,
                args,
                sels,
            });
        }
        self.expect("}")?;
        Ok(out)
    }
}

struct Doc {
    operation: Vec<Sel>,
    fragments: HashMap<String, (String, Vec<Sel>)>,
}

fn parse(query: &str, vars: &Value) -> Result<Doc, String> {
    let mut p = P {
        toks: lex(query)?,
        pos: 0,
        vars,
    };
    let mut operation = None;
    let mut fragments = HashMap::new();
    while let Some(t) = p.peek() {
        match t {
            "{" => operation = Some(p.selections()?),
            "query" => {
                p.pos += 1;
                if p.peek() != Some("{") && p.peek() != Some("(") {
                    p.next()?;
                }
                if p.peek() == Some("(") {
                    while p.peek() != Some(")") {
                        p.next()?;
                    }
                    p.pos += 1;
                }
                operation = Some(p.selections()?);
            }
            "fragment" => {
                p.pos += 1;
                let name = p.next()?;
                p.expect("on")?;
                let ty = p.next()?;
                fragments.insert(name, (ty, p.selections()?));
            }
            "mutation" | "subscription" => {
                return Err(format!(
                    "Schema is not configured to execute {t} operation."
                ));
            }
            other => return Err(format!("Syntax Error: Unexpected Name \"{other}\".")),
        }
    }
    Ok(Doc {
        operation: operation.ok_or("Must provide an operation.")?,
        fragments,
    })
}

const INTERFACES: [(&str, &[&str]); 1] = [(
    "SearchResult",
    &[
        "Guide",
        "CLICommandReference",
        "ManagementApiReference",
        "ClientLibraryFunctionReference",
        "TroubleshootingGuide",
    ],
)];

fn type_matches(cond: &str, typename: &str) -> bool {
    cond == typename
        || INTERFACES
            .iter()
            .any(|(i, members)| *i == cond && members.contains(&typename))
}

fn project(v: &Value, sels: &[Sel], doc: &Doc) -> Value {
    match v {
        Value::Array(items) => Value::Array(items.iter().map(|i| project(i, sels, doc)).collect()),
        Value::Object(obj) if !sels.is_empty() => {
            let mut out = Map::new();
            project_into(obj, sels, doc, &mut out);
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn project_into(obj: &Map<String, Value>, sels: &[Sel], doc: &Doc, out: &mut Map<String, Value>) {
    let typename = obj.get("__typename").and_then(Value::as_str).unwrap_or("");
    for sel in sels {
        match sel {
            Sel::Field {
                alias, name, sels, ..
            } => {
                let key = alias.clone().unwrap_or_else(|| name.clone());
                let value = obj.get(name).cloned().unwrap_or(Value::Null);
                out.insert(key, project(&value, sels, doc));
            }
            Sel::Inline(cond, sels) => {
                if cond.as_deref().is_none_or(|c| type_matches(c, typename)) {
                    project_into(obj, sels, doc, out);
                }
            }
            Sel::Spread(name) => {
                if let Some((ty, sels)) = doc.fragments.get(name)
                    && type_matches(ty, typename)
                {
                    project_into(obj, sels, doc, out);
                }
            }
        }
    }
}

fn corpus() -> Value {
    serde_json::from_str(DOCS).expect("pinned docs corpus is JSON")
}

fn document(mut d: Value) -> Value {
    if let Some(subs) = d.get("subsections").and_then(Value::as_array).cloned() {
        let nodes: Vec<Value> = subs
            .into_iter()
            .map(|mut s| {
                s["__typename"] = json!("Subsection");
                s
            })
            .collect();
        d["subsections"] = json!({
            "__typename": "SubsectionCollection",
            "edges": nodes.iter().map(|n| json!({ "__typename": "SubsectionEdge", "node": n })).collect::<Vec<_>>(),
            "nodes": nodes,
            "totalCount": nodes.len(),
        });
    }
    d
}

fn search(query: &str, limit: usize) -> Value {
    let terms: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(str::to_owned)
        .collect();
    let docs = corpus()["documents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut scored: Vec<(usize, usize, Value)> = docs
        .into_iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let title = d["title"].as_str().unwrap_or("").to_lowercase();
            let content = d["content"].as_str().unwrap_or("").to_lowercase();
            let score: usize = terms
                .iter()
                .map(|t| {
                    title.matches(t.as_str()).count() * 5 + content.matches(t.as_str()).count()
                })
                .sum();
            (score > 0).then_some((score, i, d))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let nodes: Vec<Value> = scored
        .into_iter()
        .take(limit)
        .map(|(_, _, d)| document(d))
        .collect();
    json!({
        "__typename": "SearchResultCollection",
        "edges": nodes.iter().map(|n| json!({ "__typename": "SearchResultEdge", "node": n })).collect::<Vec<_>>(),
        "nodes": nodes,
        "totalCount": nodes.len(),
    })
}

fn error_node(e: &Value) -> Value {
    let mut e = e.clone();
    e["__typename"] = json!("Error");
    e
}

fn errors(args: &HashMap<String, Value>) -> Value {
    let all = corpus()["errors"].as_array().cloned().unwrap_or_default();
    let filtered: Vec<Value> = all
        .iter()
        .filter(|e| {
            args.get("service")
                .is_none_or(|s| s.is_null() || e["service"] == *s)
        })
        .filter(|e| {
            args.get("code")
                .is_none_or(|c| c.is_null() || e["code"] == *c)
        })
        .map(error_node)
        .collect();
    let start = args
        .get("after")
        .and_then(Value::as_str)
        .and_then(|c| filtered.iter().position(|e| e["code"] == c).map(|i| i + 1))
        .unwrap_or(0);
    let first = args
        .get("first")
        .and_then(Value::as_u64)
        .map_or(filtered.len(), |n| n as usize);
    let page: Vec<Value> = filtered.iter().skip(start).take(first).cloned().collect();
    json!({
        "__typename": "ErrorCollection",
        "edges": page.iter().map(|n| json!({ "__typename": "ErrorEdge", "node": n, "cursor": n["code"] })).collect::<Vec<_>>(),
        "nodes": page,
        "pageInfo": {
            "__typename": "PageInfo",
            "hasNextPage": start + page.len() < filtered.len(),
            "hasPreviousPage": start > 0,
            "startCursor": page.first().map(|n| n["code"].clone()),
            "endCursor": page.last().map(|n| n["code"].clone()),
        },
        "totalCount": filtered.len(),
    })
}

/// Executes a Content API query: `Ok(data)` or `Err(messages)`.
pub fn execute(query: &str, variables: &Value) -> Result<Value, Vec<String>> {
    let doc = parse(query, variables).map_err(|e| vec![e])?;
    let mut data = Map::new();
    let mut errs = Vec::new();
    let root = doc.operation.clone();
    for sel in &root {
        let Sel::Field {
            alias,
            name,
            args,
            sels,
        } = sel
        else {
            continue;
        };
        let args: HashMap<String, Value> = args.iter().cloned().collect();
        let key = alias.clone().unwrap_or_else(|| name.clone());
        let value = match name.as_str() {
            "__typename" => json!("RootQueryType"),
            "schema" => json!(CONTENT_API_SCHEMA),
            "searchDocs" => match args.get("query").and_then(Value::as_str) {
                Some(q) => search(
                    q,
                    args.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize,
                ),
                None => {
                    errs.push("Field \"searchDocs\" argument \"query\" of type \"String!\" is required, but it was not provided.".to_owned());
                    continue;
                }
            },
            "error" => {
                let code = args.get("code").and_then(Value::as_str).unwrap_or("");
                let service = args.get("service").and_then(Value::as_str).unwrap_or("");
                corpus()["errors"]
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .find(|e| e["code"] == code && e["service"] == service)
                    })
                    .map_or(Value::Null, error_node)
            }
            "errors" => errors(&args),
            other => {
                errs.push(format!(
                    "Cannot query field \"{other}\" on type \"RootQueryType\"."
                ));
                continue;
            }
        };
        data.insert(key, project(&value, sels, &doc));
    }
    if errs.is_empty() {
        Ok(Value::Object(data))
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_projects_selection_and_fragments() {
        let data = execute(
            r#"query { searchDocs(query: "row level security", limit: 2) { nodes { title href ...on Guide { subsections { totalCount } } } } }"#,
            &Value::Null,
        )
        .unwrap();
        let nodes = data["searchDocs"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["title"], "Row Level Security");
        assert_eq!(nodes[0]["subsections"]["totalCount"], 2);
        assert!(nodes[0].get("content").is_none());
    }

    #[test]
    fn unknown_fields_are_graphql_errors() {
        let err = execute("{ nope }", &Value::Null).unwrap_err();
        assert_eq!(
            err,
            ["Cannot query field \"nope\" on type \"RootQueryType\"."]
        );
    }
}
