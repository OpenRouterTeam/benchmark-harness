//! Executes a parsed document against the store: variable coercion (with Shopify's
//! input-object and global-ID checks), root query/mutation fields, and projection of the
//! object views in `model` onto the requested selection sets.

use mock_core::store::Store;
use serde_json::{Map, Value, json};

use crate::{
    b64,
    graphql::{Directive, Document, Field, Lit, OpKind, Operation, Pos, Selection, lit_value},
    model::{self, id_of, parse_gid, reference},
    mutations, schema,
    search::{self, FieldValues},
};

pub const MAX_PAGE: i64 = 250;

#[derive(Debug, Clone, PartialEq)]
pub struct GqlError {
    pub message: String,
    pub location: Pos,
    pub path: Option<Vec<Value>>,
    pub extensions: Option<Box<Value>>,
}

impl GqlError {
    pub fn new(message: impl Into<String>, pos: Pos) -> Self {
        Self {
            message: message.into(),
            location: pos,
            path: None,
            extensions: None,
        }
    }

    fn with_path(mut self, path: &[Value]) -> Self {
        self.path = Some(path.to_vec());
        self
    }

    fn with_extensions(mut self, ext: Value) -> Self {
        self.extensions = Some(Box::new(ext));
        self
    }

    pub fn to_json(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("message".into(), json!(self.message));
        obj.insert(
            "locations".into(),
            json!([{"line": self.location.line, "column": self.location.column}]),
        );
        if let Some(path) = &self.path {
            obj.insert("path".into(), Value::Array(path.clone()));
        }
        if let Some(ext) = &self.extensions {
            obj.insert("extensions".into(), (**ext).clone());
        }
        Value::Object(obj)
    }
}

pub struct Outcome {
    pub body: Value,
    /// The new store, for mutations that ran.
    pub commit: Option<Store>,
}

pub fn run(
    doc: &Document,
    operation_name: Option<&str>,
    variables: &Value,
    store: Store,
    now: &str,
) -> Outcome {
    let errors = |e: GqlError| Outcome {
        body: json!({ "errors": [e.to_json()] }),
        commit: None,
    };
    let op = match select_operation(doc, operation_name) {
        Ok(op) => op,
        Err(e) => return errors(e),
    };
    let vars = match coerce_variables(op, variables) {
        Ok(v) => v,
        Err(e) => return errors(e),
    };
    let label = match (&op.kind, &op.name) {
        (OpKind::Query, Some(n)) => format!("query {n}"),
        (OpKind::Mutation, Some(n)) => format!("mutation {n}"),
        (OpKind::Subscription, Some(n)) => format!("subscription {n}"),
        (OpKind::Query, None) => "query".to_owned(),
        (OpKind::Mutation, None) => "mutation".to_owned(),
        (OpKind::Subscription, None) => "subscription".to_owned(),
    };
    let mut exec = Exec {
        doc,
        vars,
        store,
        now: now.to_owned(),
        cost: 0,
    };
    let result = match op.kind {
        OpKind::Query => exec.root(op, "QueryRoot", &label, false),
        OpKind::Mutation => exec.root(op, "Mutation", &label, true),
        OpKind::Subscription => Err(GqlError::new("Subscriptions are not supported", op.pos)),
    };
    match result {
        Ok(data) => {
            let cost = exec.cost.max(1);
            let body = json!({
                "data": data,
                "extensions": {"cost": {
                    "requestedQueryCost": cost,
                    "actualQueryCost": cost,
                    "throttleStatus": {"maximumAvailable": 2000.0, "currentlyAvailable": 2000 - cost.min(2000), "restoreRate": 100.0}
                }}
            });
            Outcome {
                body,
                commit: (op.kind == OpKind::Mutation).then_some(exec.store),
            }
        }
        Err(e) => errors(e),
    }
}

fn select_operation<'d>(doc: &'d Document, name: Option<&str>) -> Result<&'d Operation, GqlError> {
    let start = Pos { line: 1, column: 1 };
    match name.filter(|n| !n.is_empty()) {
        Some(n) => doc
            .operations
            .iter()
            .find(|op| op.name.as_deref() == Some(n))
            .ok_or_else(|| GqlError::new(format!("No operation named \"{n}\""), start)),
        None => match doc.operations.as_slice() {
            [op] => Ok(op),
            [] => Err(GqlError::new("No operations in query document", start)),
            _ => Err(GqlError::new(
                "An operation name is required when sending multiple operations.",
                start,
            )),
        },
    }
}

fn coerce_variables(op: &Operation, provided: &Value) -> Result<Map<String, Value>, GqlError> {
    let mut out = Map::new();
    let empty = Map::new();
    let provided = provided.as_object().unwrap_or(&empty);
    for def in &op.vars {
        let value = match provided.get(&def.name) {
            Some(v) => v.clone(),
            None => def
                .default
                .as_ref()
                .map_or(Value::Null, |d| lit_value(d, &Map::new())),
        };
        let mut problems = Vec::new();
        schema::validate(&def.ty, &value, &mut Vec::new(), &mut problems);
        if let Some((path, explanation)) = problems.first() {
            let message = if path.is_empty() {
                format!(
                    "Variable ${} of type {} was provided invalid value",
                    def.name, def.ty
                )
            } else {
                format!(
                    "Variable ${} of type {} was provided invalid value for {} ({explanation})",
                    def.name,
                    def.ty,
                    path.iter()
                        .map(|p| match p {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                )
            };
            let problems: Vec<Value> = problems
                .iter()
                .map(|(path, explanation)| json!({"path": path, "explanation": explanation}))
                .collect();
            return Err(GqlError::new(message, def.pos).with_extensions(json!({
                "code": "INVALID_VARIABLE",
                "value": value,
                "problems": problems,
            })));
        }
        out.insert(def.name.clone(), value);
    }
    check_variable_usage(op, &out)?;
    Ok(out)
}

fn check_variable_usage(op: &Operation, declared: &Map<String, Value>) -> Result<(), GqlError> {
    fn lit_vars<'a>(lit: &'a Lit, out: &mut Vec<&'a str>) {
        match lit {
            Lit::Var(v) => out.push(v),
            Lit::List(items) => items.iter().for_each(|i| lit_vars(i, out)),
            Lit::Obj(fields) => fields.iter().for_each(|(_, v)| lit_vars(v, out)),
            _ => {}
        }
    }
    fn walk(
        sels: &[Selection],
        declared: &Map<String, Value>,
        label: &str,
    ) -> Result<(), GqlError> {
        for sel in sels {
            if let Selection::Field(f) = sel {
                let mut used = Vec::new();
                f.args.iter().for_each(|(_, l)| lit_vars(l, &mut used));
                f.directives
                    .iter()
                    .flat_map(|d| d.args.iter())
                    .for_each(|(_, l)| lit_vars(l, &mut used));
                if let Some(v) = used.iter().find(|v| !declared.contains_key(**v)) {
                    return Err(GqlError::new(
                        format!("Variable ${v} is used by {label} but not declared"),
                        f.pos,
                    )
                    .with_extensions(json!({"code": "variableNotDefined", "variableName": v})));
                }
                walk(&f.selection, declared, label)?;
            } else if let Selection::Inline { selection, .. } = sel {
                walk(selection, declared, label)?;
            }
        }
        Ok(())
    }
    let label = op
        .name
        .clone()
        .unwrap_or_else(|| "anonymous query".to_owned());
    walk(&op.selection, declared, &label)
}

struct Exec<'d> {
    doc: &'d Document,
    vars: Map<String, Value>,
    store: Store,
    now: String,
    cost: i64,
}

type Fields<'d> = Vec<(String, Vec<&'d Field>)>;

const INTERFACES: [&str; 8] = [
    "Node",
    "HasMetafields",
    "Publishable",
    "LegacyInteroperability",
    "HasEvents",
    "CommentEventSubject",
    "DisplayableError",
    "Navigable",
];

impl<'d> Exec<'d> {
    fn root(
        &mut self,
        op: &'d Operation,
        type_name: &str,
        label: &str,
        mutation: bool,
    ) -> Result<Value, GqlError> {
        let mut fields = Fields::new();
        self.collect(&[&op.selection], type_name, &mut fields)?;
        let mut out = Map::new();
        for (key, group) in fields {
            let f = group[0];
            let mut path = vec![json!(label), json!(key)];
            if f.name == "__typename" {
                out.insert(key, json!(type_name));
                continue;
            }
            let raw = if mutation {
                self.cost += 10;
                let args = self.args(f);
                let now = self.now.clone();
                match mutations::execute(&mut self.store, &f.name, &args, &now) {
                    Some(payload) => payload,
                    None => return Err(undefined_field(type_name, f, &path)),
                }
            } else {
                self.root_query(f, &path)?
            };
            let value = self.complete(raw, &group, &mut path)?;
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }

    fn args(&self, f: &Field) -> Map<String, Value> {
        f.args
            .iter()
            .map(|(k, v)| (k.clone(), lit_value(v, &self.vars)))
            .collect()
    }

    fn skipped(&self, directives: &[Directive]) -> bool {
        directives.iter().any(|d| {
            let cond = d
                .args
                .iter()
                .find(|(k, _)| k == "if")
                .map(|(_, v)| lit_value(v, &self.vars))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (d.name == "skip" && cond) || (d.name == "include" && !cond)
        })
    }

    fn type_matches(condition: Option<&str>, type_name: &str) -> bool {
        condition.is_none_or(|c| {
            c == type_name
                || INTERFACES.contains(&c)
                || (c == "UserError" && type_name.ends_with("UserError"))
        })
    }

    fn collect(
        &self,
        sets: &[&'d [Selection]],
        type_name: &str,
        out: &mut Fields<'d>,
    ) -> Result<(), GqlError> {
        for set in sets {
            for sel in *set {
                match sel {
                    Selection::Field(f) => {
                        if self.skipped(&f.directives) {
                            continue;
                        }
                        match out.iter_mut().find(|(k, _)| k == f.key()) {
                            Some((_, group)) => group.push(f),
                            None => out.push((f.key().to_owned(), vec![f])),
                        }
                    }
                    Selection::Spread {
                        name,
                        directives,
                        pos,
                    } => {
                        if self.skipped(directives) {
                            continue;
                        }
                        let fragment = self.doc.fragment(name).ok_or_else(|| {
                            GqlError::new(
                                format!("Fragment {name} was used, but not defined"),
                                *pos,
                            )
                            .with_extensions(
                                json!({"code": "useAndDefineFragment", "fragmentName": name}),
                            )
                        })?;
                        if Self::type_matches(Some(&fragment.type_condition), type_name) {
                            self.collect(&[&fragment.selection], type_name, out)?;
                        }
                    }
                    Selection::Inline {
                        type_condition,
                        directives,
                        selection,
                    } => {
                        if !self.skipped(directives)
                            && Self::type_matches(type_condition.as_deref(), type_name)
                        {
                            self.collect(&[selection], type_name, out)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn project(
        &mut self,
        obj: &Value,
        sets: &[&'d [Selection]],
        path: &mut Vec<Value>,
    ) -> Result<Value, GqlError> {
        self.cost += 1;
        let type_name = obj["__typename"].as_str().unwrap_or("Object").to_owned();
        let mut fields = Fields::new();
        self.collect(sets, &type_name, &mut fields)?;
        let mut out = Map::new();
        for (key, group) in fields {
            let f = group[0];
            if f.name == "__typename" {
                out.insert(key, json!(type_name));
                continue;
            }
            path.push(json!(key));
            let Some(raw) = obj.get(&f.name).filter(|_| !f.name.starts_with("__")) else {
                return Err(undefined_field(&type_name, f, path));
            };
            let value = self.complete(raw.clone(), &group, path)?;
            path.pop();
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }

    fn complete(
        &mut self,
        raw: Value,
        group: &[&'d Field],
        path: &mut Vec<Value>,
    ) -> Result<Value, GqlError> {
        let f = group[0];
        let sets: Vec<&'d [Selection]> = group.iter().map(|g| g.selection.as_slice()).collect();
        let has_selection = sets.iter().any(|s| !s.is_empty());
        match raw {
            Value::Null => Ok(Value::Null),
            Value::Object(ref m) if m.contains_key("__ref") => {
                let kind = m["__ref"][0].as_str().unwrap_or_default().to_owned();
                let id = m["__ref"][1].as_u64().unwrap_or(0);
                let resolved = model::resolve(&self.store, &kind, id);
                self.complete(resolved, group, path)
            }
            Value::Object(ref m) if m.contains_key("__connection") => {
                let kind = m["__connection"].as_str().unwrap_or_default().to_owned();
                let items = m["items"].as_array().cloned().unwrap_or_default();
                let conn = connection(&kind, items, &self.args(f), f, path)?;
                self.cost += 2;
                self.require_selection(f, &format!("{kind}Connection"), has_selection, path)?;
                self.project(&conn, &sets, path)
            }
            Value::Object(ref m) if m.contains_key("__metafield") => {
                let args = self.args(f);
                let ns = args.get("namespace").and_then(Value::as_str);
                let key = args.get("key").and_then(Value::as_str).unwrap_or_default();
                let found = m["__metafield"]
                    .as_array()
                    .and_then(|list| {
                        list.iter()
                            .find(|mf| {
                                mf["key"] == key && ns.is_none_or(|ns| mf["namespace"] == ns)
                            })
                            .cloned()
                    })
                    .unwrap_or(Value::Null);
                if found.is_null() {
                    return Ok(Value::Null);
                }
                self.require_selection(f, "Metafield", has_selection, path)?;
                self.project(&found, &sets, path)
            }
            Value::Object(ref m) => {
                let type_name = m
                    .get("__typename")
                    .and_then(Value::as_str)
                    .unwrap_or("Object")
                    .to_owned();
                self.require_selection(f, &type_name, has_selection, path)?;
                self.project(&raw, &sets, path)
            }
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.into_iter().enumerate() {
                    path.push(json!(i));
                    out.push(self.complete(item, group, path)?);
                    path.pop();
                }
                Ok(Value::Array(out))
            }
            scalar => {
                if has_selection {
                    let ty = match &scalar {
                        Value::Bool(_) => "Boolean",
                        Value::Number(n) if n.is_f64() => "Float",
                        Value::Number(_) => "Int",
                        _ => "String",
                    };
                    return Err(GqlError::new(
                        format!(
                            "Selections can't be made on scalars (field '{}' returns {ty} but has selections [{}])",
                            f.name,
                            sets.iter()
                                .flat_map(|s| s.iter())
                                .filter_map(|s| match s {
                                    Selection::Field(f) => Some(f.name.clone()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        f.pos,
                    )
                    .with_path(path)
                    .with_extensions(json!({"code": "selectionMismatch", "nodeName": format!("field '{}'", f.name), "typeName": ty})));
                }
                Ok(scalar)
            }
        }
    }

    fn require_selection(
        &self,
        f: &Field,
        type_name: &str,
        has_selection: bool,
        path: &[Value],
    ) -> Result<(), GqlError> {
        if has_selection {
            return Ok(());
        }
        Err(GqlError::new(
            format!(
                "Field must have selections (field '{}' returns {type_name} but has no selections. Did you mean '{} {{ ... }}'?)",
                f.name, f.name
            ),
            f.pos,
        )
        .with_path(path)
        .with_extensions(json!({"code": "selectionMismatch", "nodeName": format!("field '{}'", f.name), "typeName": type_name})))
    }

    fn gid_arg(
        &self,
        f: &Field,
        args: &Map<String, Value>,
        name: &str,
    ) -> Result<Option<(String, u64)>, GqlError> {
        let Some(value) = args.get(name).and_then(Value::as_str) else {
            return Err(GqlError::new(
                format!("Field '{}' is missing required arguments: {name}", f.name),
                f.pos,
            )
            .with_extensions(json!({"code": "missingRequiredArguments", "className": "Field", "name": f.name, "arguments": name})));
        };
        if !model::is_gid(value) {
            return Err(GqlError::new(format!("Invalid global id '{value}'"), f.pos)
                .with_extensions(
                    json!({"code": "argumentLiteralsIncompatible", "typeName": "CoercionError"}),
                ));
        }
        Ok(parse_gid(value).map(|(k, id)| (k.to_owned(), id)))
    }

    fn by_gid(&self, f: &Field, args: &Map<String, Value>, kind: &str) -> Result<Value, GqlError> {
        Ok(match self.gid_arg(f, args, "id")? {
            Some((k, id)) if k == kind => reference(kind, id),
            _ => Value::Null,
        })
    }

    fn root_query(&mut self, f: &Field, path: &[Value]) -> Result<Value, GqlError> {
        let args = self.args(f);
        let store = &self.store;
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let sort = args.get("sortKey").and_then(Value::as_str);
        let reverse = args
            .get("reverse")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(match f.name.as_str() {
            "shop" => model::shop(store),
            "products" | "productsCount" => {
                let ids = list_products(store, query, sort, reverse);
                if f.name == "productsCount" {
                    count(ids.len())
                } else {
                    model::connection(
                        "Product",
                        ids.into_iter().map(|id| reference("Product", id)).collect(),
                    )
                }
            }
            "product" => self.by_gid(f, &args, "Product")?,
            "productByHandle" | "productByIdentifier" => {
                let handle = args
                    .get("handle")
                    .or_else(|| args.get("identifier").and_then(|i| i.get("handle")))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                model::docs(store, "products")
                    .into_iter()
                    .find(|p| p["handle"] == handle)
                    .map_or(Value::Null, |p| reference("Product", id_of(p)))
            }
            "productVariant" => self.by_gid(f, &args, "ProductVariant")?,
            "productVariants" => {
                let q = search::parse(query);
                let mut items = Vec::new();
                for p in model::docs(store, "products") {
                    let mut p = p.clone();
                    model::normalize_product(&mut p);
                    for v in p["variants"].as_array().into_iter().flatten() {
                        let lookup = |name: &str| match name {
                            "sku" => {
                                FieldValues::exact(model::str_of(&v["sku"]).into_iter().collect())
                            }
                            "barcode" => FieldValues::exact(
                                model::str_of(&v["barcode"]).into_iter().collect(),
                            ),
                            "product_id" => FieldValues::exact(vec![id_of(&p).to_string()]),
                            "id" => FieldValues::exact(vec![id_of(v).to_string()]),
                            "title" => {
                                FieldValues::words(model::str_of(&v["title"]).into_iter().collect())
                            }
                            _ => None,
                        };
                        let text: Vec<String> = [&v["sku"], &v["title"], &p["title"]]
                            .iter()
                            .filter_map(|x| model::str_of(x))
                            .collect();
                        if q.matches(&lookup, &text) {
                            items.push(reference("ProductVariant", id_of(v)));
                        }
                    }
                }
                model::connection("ProductVariant", items)
            }
            "orders" | "ordersCount" => {
                let ids = list_orders(store, query, sort, reverse);
                if f.name == "ordersCount" {
                    count(ids.len())
                } else {
                    model::connection(
                        "Order",
                        ids.into_iter().map(|id| reference("Order", id)).collect(),
                    )
                }
            }
            "order" => self.by_gid(f, &args, "Order")?,
            "customers" | "customersCount" => {
                let ids = list_customers(store, query, sort, reverse);
                if f.name == "customersCount" {
                    count(ids.len())
                } else {
                    model::connection(
                        "Customer",
                        ids.into_iter()
                            .map(|id| reference("Customer", id))
                            .collect(),
                    )
                }
            }
            "customer" => self.by_gid(f, &args, "Customer")?,
            "collections" => {
                let q = search::parse(query);
                let items = model::docs(store, "collections")
                    .into_iter()
                    .filter(|c| {
                        let lookup = |name: &str| match name {
                            "title" => {
                                FieldValues::words(model::str_of(&c["title"]).into_iter().collect())
                            }
                            "handle" => FieldValues::exact(
                                model::str_of(&c["handle"]).into_iter().collect(),
                            ),
                            "id" => FieldValues::exact(vec![id_of(c).to_string()]),
                            _ => None,
                        };
                        q.matches(
                            &lookup,
                            &model::str_of(&c["title"]).into_iter().collect::<Vec<_>>(),
                        )
                    })
                    .map(|c| reference("Collection", id_of(c)))
                    .collect();
                model::connection("Collection", items)
            }
            "collection" => self.by_gid(f, &args, "Collection")?,
            "node" => match self.gid_arg(f, &args, "id")? {
                Some((k, id)) => reference(&k, id),
                None => Value::Null,
            },
            "nodes" => {
                let ids = args
                    .get("ids")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut out = Vec::new();
                for id in ids {
                    let s = id.as_str().unwrap_or_default();
                    if !model::is_gid(s) {
                        return Err(GqlError::new(format!("Invalid global id '{s}'"), f.pos)
                            .with_extensions(json!({"code": "argumentLiteralsIncompatible", "typeName": "CoercionError"})));
                    }
                    out.push(parse_gid(s).map_or(Value::Null, |(k, id)| reference(k, id)));
                }
                Value::Array(out)
            }
            _ => return Err(undefined_field("QueryRoot", f, path)),
        })
    }
}

fn count(n: usize) -> Value {
    json!({"__typename": "Count", "count": n, "precision": "EXACT"})
}

fn undefined_field(type_name: &str, f: &Field, path: &[Value]) -> GqlError {
    GqlError::new(
        format!("Field '{}' doesn't exist on type '{type_name}'", f.name),
        f.pos,
    )
    // Validation errors carry the static path (no list indices).
    .with_path(
        &path
            .iter()
            .filter(|p| !p.is_number())
            .cloned()
            .collect::<Vec<_>>(),
    )
    .with_extensions(json!({"code": "undefinedField", "typeName": type_name, "fieldName": f.name}))
}

fn cursor(id: u64) -> String {
    b64::encode(
        json!({"last_id": id, "last_value": id.to_string()})
            .to_string()
            .as_bytes(),
    )
}

fn cursor_id(cursor: &str) -> Option<u64> {
    let bytes = b64::decode(cursor)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["last_id"].as_u64()
}

fn item_id(item: &Value) -> u64 {
    if let Some(r) = item.get("__ref") {
        return r[1].as_u64().unwrap_or(0);
    }
    item["id"]
        .as_str()
        .and_then(parse_gid)
        .map_or(0, |(_, id)| id)
}

fn connection(
    kind: &str,
    items: Vec<Value>,
    args: &Map<String, Value>,
    f: &Field,
    path: &[Value],
) -> Result<Value, GqlError> {
    let first = args.get("first").and_then(Value::as_i64);
    let last = args.get("last").and_then(Value::as_i64);
    if first.is_none() && last.is_none() {
        return Err(GqlError::new("you must provide one of first or last", f.pos).with_path(path));
    }
    if first.is_some() && last.is_some() {
        return Err(
            GqlError::new("providing both first and last is not supported", f.pos).with_path(path),
        );
    }
    if let Some(n) = first.or(last)
        && !(0..=MAX_PAGE).contains(&n)
    {
        return Err(GqlError::new(
            format!(
                "{} must be between 0 and {MAX_PAGE}",
                if first.is_some() { "first" } else { "last" }
            ),
            f.pos,
        )
        .with_path(path)
        .with_extensions(json!({"code": "BAD_REQUEST"})));
    }
    let position = |name: &str| -> Result<Option<usize>, GqlError> {
        let Some(c) = args.get(name).and_then(Value::as_str) else {
            return Ok(None);
        };
        let invalid =
            || GqlError::new("Invalid cursor for current pagination sort.", f.pos).with_path(path);
        let id = cursor_id(c).ok_or_else(invalid)?;
        items
            .iter()
            .position(|i| item_id(i) == id)
            .map(Some)
            .ok_or_else(invalid)
    };
    let start = position("after")?.map_or(0, |p| p + 1);
    let end = position("before")?.unwrap_or(items.len()).max(start);
    let window = &items[start..end];
    let (slice, has_next, has_prev) = match (first, last) {
        (Some(n), _) => {
            let n = n as usize;
            (
                &window[..n.min(window.len())],
                window.len() > n || end < items.len(),
                start > 0,
            )
        }
        (_, Some(n)) => {
            let n = n as usize;
            let from = window.len().saturating_sub(n);
            (&window[from..], end < items.len(), start + from > 0)
        }
        _ => unreachable!("checked above"),
    };
    let edges: Vec<Value> = slice
        .iter()
        .map(|item| json!({"__typename": format!("{kind}Edge"), "cursor": cursor(item_id(item)), "node": item}))
        .collect();
    let start_cursor = slice.first().map(|i| cursor(item_id(i)));
    let end_cursor = slice.last().map(|i| cursor(item_id(i)));
    Ok(json!({
        "__typename": format!("{kind}Connection"),
        "edges": edges,
        "nodes": slice,
        "pageInfo": {
            "__typename": "PageInfo",
            "hasNextPage": has_next,
            "hasPreviousPage": has_prev,
            "startCursor": start_cursor,
            "endCursor": end_cursor,
        },
    }))
}

fn sort_ids<T: Ord>(mut items: Vec<(T, u64)>, reverse: bool) -> Vec<u64> {
    items.sort();
    if reverse {
        items.reverse();
    }
    items.into_iter().map(|(_, id)| id).collect()
}

fn opt(v: &Value) -> Vec<String> {
    model::str_of(v).into_iter().collect()
}

fn list_products(store: &Store, query: &str, sort: Option<&str>, reverse: bool) -> Vec<u64> {
    let q = search::parse(query);
    let items = model::docs(store, "products")
        .into_iter()
        .filter(|p| {
            let variants = p["variants"].as_array().cloned().unwrap_or_default();
            let inventory: i64 = variants
                .iter()
                .filter_map(|v| v["inventoryQuantity"].as_i64())
                .sum();
            let tags = model::strings(&p["tags"]);
            let skus: Vec<String> = variants
                .iter()
                .filter_map(|v| model::str_of(&v["sku"]))
                .collect();
            let lookup = |name: &str| match name {
                "title" => FieldValues::words(opt(&p["title"])),
                "handle" => FieldValues::exact(opt(&p["handle"])),
                "status" => {
                    FieldValues::exact(vec![p["status"].as_str().unwrap_or("ACTIVE").to_owned()])
                }
                "vendor" => FieldValues::words(opt(&p["vendor"])),
                "product_type" => FieldValues::words(opt(&p["productType"])),
                "tag" => FieldValues::exact(tags.clone()),
                "sku" => FieldValues::exact(skus.clone()),
                "barcode" => FieldValues::exact(
                    variants
                        .iter()
                        .filter_map(|v| model::str_of(&v["barcode"]))
                        .collect(),
                ),
                "id" => FieldValues::exact(vec![id_of(p).to_string()]),
                "created_at" => FieldValues::exact(opt(&p["createdAt"])),
                "updated_at" => FieldValues::exact(opt(if p["updatedAt"].is_null() {
                    &p["createdAt"]
                } else {
                    &p["updatedAt"]
                })),
                "published_at" => FieldValues::exact(opt(&p["publishedAt"])),
                "inventory_total" => FieldValues::exact(vec![inventory.to_string()]),
                "gift_card" => FieldValues::exact(vec!["false".into()]),
                "published_status" => FieldValues::exact(if p["publishedAt"].is_null() {
                    vec!["unpublished".into(), "any".into()]
                } else {
                    vec!["published".into(), "any".into()]
                }),
                _ => None,
            };
            let mut text = opt(&p["title"]);
            text.extend(opt(&p["vendor"]));
            text.extend(opt(&p["productType"]));
            text.extend(opt(&p["handle"]));
            text.extend(tags.iter().cloned());
            text.extend(skus.iter().cloned());
            q.matches(&lookup, &text)
        })
        .map(|p| {
            let key = match sort {
                Some("TITLE") => p["title"].as_str().unwrap_or_default().to_lowercase(),
                Some("CREATED_AT") => p["createdAt"].as_str().unwrap_or_default().to_owned(),
                Some("UPDATED_AT") => p["updatedAt"]
                    .as_str()
                    .or(p["createdAt"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                Some("VENDOR") => p["vendor"].as_str().unwrap_or_default().to_lowercase(),
                Some("PRODUCT_TYPE") => {
                    p["productType"].as_str().unwrap_or_default().to_lowercase()
                }
                Some("INVENTORY_TOTAL") => format!(
                    "{:020}",
                    p["variants"]
                        .as_array()
                        .map_or(0, |vs| vs
                            .iter()
                            .filter_map(|v| v["inventoryQuantity"].as_i64())
                            .sum::<i64>())
                        .max(0)
                ),
                _ => format!("{:020}", id_of(p)),
            };
            ((key, id_of(p)), id_of(p))
        })
        .collect();
    sort_ids(items, reverse)
}

fn order_statuses(o: &Value) -> Vec<String> {
    let mut out = vec!["any".to_owned()];
    if !o["cancelledAt"].is_null() {
        out.push("cancelled".into());
    }
    if !o["closedAt"].is_null() {
        out.push("closed".into());
    }
    if o["cancelledAt"].is_null() && o["closedAt"].is_null() {
        out.push("open".into());
    }
    out
}

fn fulfillment_values(status: &str) -> Vec<String> {
    match status {
        "FULFILLED" => vec!["shipped".into(), "fulfilled".into()],
        "UNFULFILLED" => vec!["unshipped".into(), "unfulfilled".into()],
        "PARTIALLY_FULFILLED" => vec!["partial".into(), "partially_fulfilled".into()],
        "ON_HOLD" => vec!["on_hold".into()],
        other => vec![other.to_ascii_lowercase()],
    }
}

fn list_orders(store: &Store, query: &str, sort: Option<&str>, reverse: bool) -> Vec<u64> {
    let q = search::parse(query);
    let items = model::docs(store, "orders")
        .into_iter()
        .filter(|o| {
            let customer = o["customerId"]
                .as_u64()
                .and_then(|c| model::find(store, "customers", c));
            let tags = model::strings(&o["tags"]);
            let skus: Vec<String> = o["lineItems"]
                .as_array()
                .map(|li| li.iter().filter_map(|l| model::str_of(&l["sku"])).collect())
                .unwrap_or_default();
            let name = o["name"].as_str().unwrap_or_default().to_owned();
            let lookup = |field: &str| match field {
                "status" => FieldValues::exact(order_statuses(o)),
                "financial_status" => FieldValues::exact(vec![
                    o["financialStatus"]
                        .as_str()
                        .unwrap_or("PAID")
                        .to_ascii_lowercase(),
                ]),
                "fulfillment_status" => FieldValues::exact(fulfillment_values(
                    o["fulfillmentStatus"].as_str().unwrap_or("UNFULFILLED"),
                )),
                "customer_id" => FieldValues::exact(
                    o["customerId"]
                        .as_u64()
                        .map(|c| c.to_string())
                        .into_iter()
                        .collect(),
                ),
                "email" => FieldValues::exact(opt(&o["email"])),
                "name" => {
                    FieldValues::exact(vec![name.clone(), name.trim_start_matches('#').to_owned()])
                }
                "tag" => FieldValues::exact(tags.clone()),
                "sku" => FieldValues::exact(skus.clone()),
                "id" => FieldValues::exact(vec![id_of(o).to_string()]),
                "created_at" => FieldValues::exact(opt(&o["createdAt"])),
                "processed_at" => FieldValues::exact(vec![model::processed_at(o)]),
                "updated_at" => FieldValues::exact(opt(if o["updatedAt"].is_null() {
                    &o["createdAt"]
                } else {
                    &o["updatedAt"]
                })),
                "test" => FieldValues::exact(vec!["false".into()]),
                _ => None,
            };
            let mut text = vec![name.clone()];
            text.extend(opt(&o["email"]));
            text.extend(tags.iter().cloned());
            text.extend(skus.iter().cloned());
            if let Some(c) = customer {
                text.extend(opt(&c["firstName"]));
                text.extend(opt(&c["lastName"]));
                text.extend(opt(&c["email"]));
            }
            q.matches(&lookup, &text)
        })
        .map(|o| {
            let key = match sort {
                Some("CREATED_AT") => o["createdAt"].as_str().unwrap_or_default().to_owned(),
                Some("UPDATED_AT") => o["updatedAt"]
                    .as_str()
                    .or(o["createdAt"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                Some("ID") => format!("{:020}", id_of(o)),
                Some("TOTAL_PRICE") => format!("{:020}", model::order_totals(o).total.max(0)),
                Some("ORDER_NUMBER") => format!(
                    "{:>20}",
                    o["name"]
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches('#')
                ),
                Some("CUSTOMER_NAME") => o["customerId"]
                    .as_u64()
                    .and_then(|c| model::find(store, "customers", c))
                    .map(|c| {
                        format!(
                            "{} {}",
                            c["firstName"].as_str().unwrap_or_default(),
                            c["lastName"].as_str().unwrap_or_default()
                        )
                        .to_lowercase()
                    })
                    .unwrap_or_default(),
                _ => model::processed_at(o),
            };
            ((key, id_of(o)), id_of(o))
        })
        .collect();
    sort_ids(items, reverse)
}

fn list_customers(store: &Store, query: &str, sort: Option<&str>, reverse: bool) -> Vec<u64> {
    let q = search::parse(query);
    let items = model::docs(store, "customers")
        .into_iter()
        .filter(|c| {
            let id = id_of(c);
            let tags = model::strings(&c["tags"]);
            let addresses = c["addresses"].as_array().cloned().unwrap_or_default();
            let orders = model::customer_orders(store, id).len();
            let spent = model::amount_spent(store, id);
            let lookup = |field: &str| match field {
                "email" => FieldValues::exact(opt(&c["email"])),
                "first_name" => FieldValues::words(opt(&c["firstName"])),
                "last_name" => FieldValues::words(opt(&c["lastName"])),
                "phone" => FieldValues::exact(opt(&c["phone"])),
                "tag" => FieldValues::exact(tags.clone()),
                "id" => FieldValues::exact(vec![id.to_string()]),
                "state" => FieldValues::exact(vec![
                    c["state"]
                        .as_str()
                        .unwrap_or("ENABLED")
                        .to_ascii_lowercase(),
                ]),
                "country" => FieldValues::words(
                    addresses
                        .iter()
                        .filter_map(|a| model::str_of(&a["country"]))
                        .collect(),
                ),
                "city" => FieldValues::words(
                    addresses
                        .iter()
                        .filter_map(|a| model::str_of(&a["city"]))
                        .collect(),
                ),
                "orders_count" | "number_of_orders" => FieldValues::exact(vec![orders.to_string()]),
                "total_spent" | "amount_spent" => FieldValues::exact(vec![model::money_str(spent)]),
                "created_at" => FieldValues::exact(opt(&c["createdAt"])),
                "updated_at" => FieldValues::exact(opt(if c["updatedAt"].is_null() {
                    &c["createdAt"]
                } else {
                    &c["updatedAt"]
                })),
                _ => None,
            };
            let mut text = opt(&c["firstName"]);
            text.extend(opt(&c["lastName"]));
            text.push(format!(
                "{} {}",
                c["firstName"].as_str().unwrap_or_default(),
                c["lastName"].as_str().unwrap_or_default()
            ));
            text.extend(opt(&c["email"]));
            text.extend(opt(&c["phone"]));
            text.extend(tags.iter().cloned());
            text.extend(
                addresses
                    .iter()
                    .filter_map(|a| model::str_of(&a["company"])),
            );
            q.matches(&lookup, &text)
        })
        .map(|c| {
            let key = match sort {
                Some("NAME") => format!(
                    "{} {}",
                    c["firstName"].as_str().unwrap_or_default(),
                    c["lastName"].as_str().unwrap_or_default()
                )
                .to_lowercase(),
                Some("CREATED_AT") => c["createdAt"].as_str().unwrap_or_default().to_owned(),
                Some("UPDATED_AT") => c["updatedAt"]
                    .as_str()
                    .or(c["createdAt"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                _ => format!("{:020}", id_of(c)),
            };
            ((key, id_of(c)), id_of(c))
        })
        .collect();
    sort_ids(items, reverse)
}
