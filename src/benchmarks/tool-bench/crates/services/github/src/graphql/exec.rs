//! Executes a parsed document against a [`Resolver`]. Objects are JSON values carrying
//! `__typename`; the resolver computes each selected field from its parent object.

use serde_json::{Map, Value, json};

use super::parse::{Directive, Document, Field, OpKind, Operation, Selection};

#[derive(Debug, Clone)]
pub struct FieldError {
    pub message: String,
    pub kind: Option<&'static str>,
}

impl FieldError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: Some("NOT_FOUND"),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: None,
        }
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: Some("UNPROCESSABLE"),
        }
    }
}

pub type FieldResult = Result<Value, FieldError>;

pub trait Resolver {
    /// Value of `field` on `parent` (whose `__typename` identifies its type). The root
    /// objects are `{"__typename": "Query"}` and `{"__typename": "Mutation"}`.
    fn resolve(&mut self, parent: &Value, field: &str, args: &Map<String, Value>) -> FieldResult;

    /// Whether an object of `typename` satisfies a fragment's type condition.
    fn type_matches(&self, typename: &str, cond: &str) -> bool {
        typename == cond
    }
}

pub struct Executor<'a, R: Resolver> {
    pub doc: &'a Document,
    pub vars: Map<String, Value>,
    pub resolver: R,
    pub errors: Vec<Value>,
    spreading: Vec<String>,
}

impl<'a, R: Resolver> Executor<'a, R> {
    pub fn new(doc: &'a Document, vars: Value, resolver: R) -> Self {
        let vars = match vars {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        Self {
            doc,
            vars,
            resolver,
            errors: Vec::new(),
            spreading: Vec::new(),
        }
    }

    pub fn operation(&self, name: Option<&str>) -> Result<&'a Operation, String> {
        match name {
            Some(n) => self
                .doc
                .operations
                .iter()
                .find(|o| o.name.as_deref() == Some(n))
                .ok_or_else(|| format!("No operation named \"{n}\"")),
            None if self.doc.operations.len() == 1 => Ok(&self.doc.operations[0]),
            None => Err(
                "An operation name is required when the document contains multiple operations"
                    .into(),
            ),
        }
    }

    pub fn run(&mut self, op: &'a Operation) -> Value {
        for (name, default) in &op.var_defaults {
            if !self.vars.contains_key(name)
                && let Some(d) = default
            {
                let v = d.eval(&Map::new());
                self.vars.insert(name.clone(), v);
            }
        }
        let root = match op.kind {
            OpKind::Mutation => json!({"__typename": "Mutation"}),
            _ => json!({"__typename": "Query"}),
        };
        let mut path = Vec::new();
        Value::Object(self.select(&root, &op.selections, &mut path))
    }

    fn skipped(&self, directives: &[Directive]) -> bool {
        for d in directives {
            let cond = d
                .args
                .iter()
                .find(|(k, _)| k == "if")
                .map(|(_, v)| v.eval(&self.vars))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match d.name.as_str() {
                "skip" if cond => return true,
                "include" if !cond => return true,
                _ => {}
            }
        }
        false
    }

    fn select(
        &mut self,
        parent: &Value,
        sels: &'a [Selection],
        path: &mut Vec<Value>,
    ) -> Map<String, Value> {
        let mut out = Map::new();
        let typename = parent
            .get("__typename")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        for sel in sels {
            match sel {
                Selection::Field(f) => {
                    if self.skipped(&f.directives) {
                        continue;
                    }
                    let key = f.key().to_owned();
                    path.push(Value::String(key.clone()));
                    let v = self.field(parent, &typename, f, path);
                    path.pop();
                    merge_into(&mut out, key, v);
                }
                Selection::Inline {
                    type_cond,
                    directives,
                    selections,
                } => {
                    if self.skipped(directives) {
                        continue;
                    }
                    if type_cond
                        .as_deref()
                        .is_none_or(|c| self.resolver.type_matches(&typename, c))
                    {
                        let inner = self.select(parent, selections, path);
                        for (k, v) in inner {
                            merge_into(&mut out, k, v);
                        }
                    }
                }
                Selection::Spread { name, directives } => {
                    if self.skipped(directives) {
                        continue;
                    }
                    let Some(frag) = self.doc.fragments.get(name) else {
                        self.errors.push(json!({"message": format!("Fragment {name} was used, but not defined")}));
                        continue;
                    };
                    if self.spreading.contains(name) {
                        self.errors.push(
                            json!({"message": format!("Fragment {name} cannot spread itself")}),
                        );
                        continue;
                    }
                    if self.resolver.type_matches(&typename, &frag.type_cond) {
                        self.spreading.push(name.clone());
                        let inner = self.select(parent, &frag.selections, path);
                        self.spreading.pop();
                        for (k, v) in inner {
                            merge_into(&mut out, k, v);
                        }
                    }
                }
            }
        }
        out
    }

    fn field(
        &mut self,
        parent: &Value,
        typename: &str,
        f: &'a Field,
        path: &mut Vec<Value>,
    ) -> Value {
        if f.name == "__typename" {
            return Value::String(typename.to_owned());
        }
        let args: Map<String, Value> = f
            .args
            .iter()
            .map(|(k, v)| (k.clone(), v.eval(&self.vars)))
            .collect();
        match self.resolver.resolve(parent, &f.name, &args) {
            Ok(v) => self.complete(v, &f.selections, path),
            Err(e) => {
                let mut err = json!({
                    "path": path.clone(),
                    "locations": [{"line": f.line, "column": f.column}],
                    "message": e.message,
                });
                if let Some(kind) = e.kind {
                    err["type"] = json!(kind);
                }
                self.errors.push(err);
                Value::Null
            }
        }
    }

    fn complete(&mut self, v: Value, sels: &'a [Selection], path: &mut Vec<Value>) -> Value {
        if sels.is_empty() {
            // Leaf: drop internal keys from accidental object leaves.
            return v;
        }
        match v {
            Value::Null => Value::Null,
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        path.push(json!(i));
                        let r = self.complete(item, sels, path);
                        path.pop();
                        r
                    })
                    .collect(),
            ),
            obj @ Value::Object(_) => Value::Object(self.select(&obj, sels, path)),
            scalar => scalar,
        }
    }
}

fn merge_into(out: &mut Map<String, Value>, key: String, v: Value) {
    match (out.get_mut(&key), v) {
        (Some(Value::Object(existing)), Value::Object(new)) => {
            for (k, nv) in new {
                merge_into(existing, k, nv);
            }
        }
        (Some(Value::Array(existing)), Value::Array(new)) if existing.len() == new.len() => {
            for (e, n) in existing.iter_mut().zip(new) {
                if let (Value::Object(eo), Value::Object(no)) = (&mut *e, &n) {
                    for (k, nv) in no.clone() {
                        merge_into(eo, k, nv);
                    }
                } else {
                    *e = n;
                }
            }
        }
        (_, v) => {
            out.insert(key, v);
        }
    }
}
