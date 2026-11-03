//! CRM v3 objects: list, get, search and the batch read/create/update endpoints.
//!
//! Records live in one collection per object type (`contacts`, `deals`, a custom object's
//! name, ...), keyed by id, in HubSpot's own shape:
//! `{id, properties, createdAt, updatedAt, archived}`.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use mock_core::{http::json, store::Store, world::ServiceState, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    assoc,
    types::{
        ENGAGEMENT_TYPES, ObjectType, association_defs, default_properties, property_definitions,
        resolve, searchable_properties,
    },
    util::{
        bad_json, base_url, error, id_order, iso_ms, iso_secs, not_found, numeric, page_window,
        paging, parse_ms, query_bool, query_list, query_one, scalar_string, validation,
        with_service,
    },
};

pub fn unknown_type(svc: &mut ServiceState, raw: &str) -> Response {
    validation(svc, &format!("Unable to infer object type from: {raw}"))
}

/// Every property of a record, including the computed id and timestamp properties.
pub fn all_props(ty: &ObjectType, doc: &Value) -> Map<String, Value> {
    let mut props = doc["properties"].as_object().cloned().unwrap_or_default();
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    props.insert("hs_object_id".into(), Value::String(id));
    if !props.contains_key(ty.created_key())
        && let Some(c) = doc["createdAt"].as_str()
    {
        props.insert(ty.created_key().into(), Value::String(c.to_owned()));
    }
    if !props.contains_key(ty.modified_key())
        && let Some(u) = doc["updatedAt"].as_str()
    {
        props.insert(ty.modified_key().into(), Value::String(u.to_owned()));
    }
    props
}

/// Which properties a response carries.
pub enum Selection {
    Default,
    /// Requested names (plus the always-returned id/timestamps).
    Only(Vec<String>),
    /// Defaults plus everything the record has set (create/update responses).
    Written(Vec<String>),
}

impl Selection {
    pub fn from_list(list: Vec<String>) -> Self {
        if list.is_empty() {
            Self::Default
        } else {
            Self::Only(list)
        }
    }
}

/// Renders records for one type; knows the defined property names once per request.
pub struct Renderer {
    ty: ObjectType,
    known: BTreeSet<String>,
    defaults: Vec<String>,
}

impl Renderer {
    pub fn new(store: &Store, ty: &ObjectType) -> Self {
        Self {
            ty: ty.clone(),
            known: known_properties(store, ty),
            defaults: default_properties(store, ty),
        }
    }

    pub fn render(&self, doc: &Value, sel: &Selection) -> Value {
        let all = all_props(&self.ty, doc);
        let always = [
            "hs_object_id",
            self.ty.created_key(),
            self.ty.modified_key(),
        ];
        let names: Vec<String> = match sel {
            Selection::Default => self.defaults.clone(),
            Selection::Only(list) => list
                .iter()
                .cloned()
                .chain(always.iter().map(|s| (*s).to_owned()))
                .collect(),
            Selection::Written(extra) => self
                .defaults
                .iter()
                .cloned()
                .chain(extra.iter().cloned())
                .collect(),
        };
        let mut props = BTreeMap::new();
        for name in names {
            match all.get(&name) {
                Some(v) => {
                    props.insert(name, v.clone());
                }
                None if self.known.contains(&name) => {
                    props.insert(name, Value::Null);
                }
                None => {}
            }
        }
        let mut out = Map::new();
        out.insert("id".into(), doc["id"].clone());
        out.insert(
            "properties".into(),
            Value::Object(props.into_iter().collect()),
        );
        out.insert(
            "createdAt".into(),
            all.get(self.ty.created_key())
                .cloned()
                .unwrap_or(Value::Null),
        );
        out.insert(
            "updatedAt".into(),
            all.get(self.ty.modified_key())
                .cloned()
                .unwrap_or(Value::Null),
        );
        out.insert(
            "archived".into(),
            Value::Bool(doc["archived"].as_bool().unwrap_or(false)),
        );
        if let Some(at) = doc.get("archivedAt") {
            out.insert("archivedAt".into(), at.clone());
        }
        Value::Object(out)
    }
}

/// Defined property names: definitions plus anything a record of the type has set.
pub fn known_properties(store: &Store, ty: &ObjectType) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = property_definitions(store, ty)
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect();
    if let Some(coll) = store.collection(&ty.name) {
        for doc in coll.values() {
            if let Some(props) = doc["properties"].as_object() {
                known.extend(props.keys().cloned());
            }
        }
    }
    known
}

/// Records of a type in id order, archived or not.
fn records<'a>(store: &'a Store, ty: &ObjectType, archived: bool) -> Vec<&'a Value> {
    let mut docs: Vec<&Value> = store
        .collection(&ty.name)
        .map(|c| {
            c.values()
                .filter(|d| d["archived"].as_bool().unwrap_or(false) == archived)
                .collect()
        })
        .unwrap_or_default();
    docs.sort_by(|a, b| {
        id_order(
            a["id"].as_str().unwrap_or_default(),
            b["id"].as_str().unwrap_or_default(),
        )
    });
    docs
}

/// Finds a live record by id, or by a unique property (`idProperty`, e.g. `email`).
fn find<'a>(
    store: &'a Store,
    ty: &ObjectType,
    id: &str,
    id_property: Option<&str>,
    archived: bool,
) -> Option<&'a Value> {
    let coll = store.collection(&ty.name)?;
    let live = |d: &&Value| d["archived"].as_bool().unwrap_or(false) == archived;
    match id_property {
        None | Some("hs_object_id") => coll.get(id).filter(live),
        Some(prop) => coll.values().filter(live).find(|d| {
            d["properties"][prop]
                .as_str()
                .is_some_and(|v| v.eq_ignore_ascii_case(id))
        }),
    }
}

/// `GET /crm/v3/objects/{objectType}`
pub async fn list(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    with_service(&world, |svc, _| {
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let (limit, after) = page_window(&query, 10, 100);
        let archived = query_bool(&query, "archived");
        let sel = Selection::from_list(query_list(&query, "properties"));
        let assoc_types = query_list(&query, "associations");
        let renderer = Renderer::new(&svc.store, &ty);
        let docs = records(&svc.store, &ty, archived);
        let results: Vec<Value> = docs
            .iter()
            .skip(after)
            .take(limit)
            .map(|d| with_associations(&svc.store, &ty, &renderer, d, &sel, &assoc_types))
            .collect();
        let mut body = json_value!({ "results": results });
        let mut link = format!("{}/crm/v3/objects/{raw}?limit={limit}", base_url(&headers));
        for (k, v) in &query {
            if k == "properties" || k == "associations" || k == "archived" {
                link.push_str(&format!("&{k}={v}"));
            }
        }
        if let Some(p) = paging(docs.len(), after, limit, Some(&link)) {
            body["paging"] = p;
        }
        json(StatusCode::OK, body)
    })
}

fn with_associations(
    store: &Store,
    ty: &ObjectType,
    renderer: &Renderer,
    doc: &Value,
    sel: &Selection,
    assoc_types: &[String],
) -> Value {
    let mut out = renderer.render(doc, sel);
    if !assoc_types.is_empty()
        && let Some(block) = assoc::v3_block(
            store,
            ty,
            doc["id"].as_str().unwrap_or_default(),
            assoc_types,
        )
    {
        out["associations"] = block;
    }
    out
}

/// `GET /crm/v3/objects/{objectType}/{objectId}`
pub async fn get_one(
    State(world): State<SharedWorld>,
    Path((raw, id)): Path<(String, String)>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    with_service(&world, |svc, _| {
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let archived = query_bool(&query, "archived");
        let Some(doc) = find(
            &svc.store,
            &ty,
            &id,
            query_one(&query, "idProperty"),
            archived,
        )
        .cloned() else {
            return not_found(svc, "resource not found");
        };
        let sel = Selection::from_list(query_list(&query, "properties"));
        let assoc_types = query_list(&query, "associations");
        let renderer = Renderer::new(&svc.store, &ty);
        let body = with_associations(&svc.store, &ty, &renderer, &doc, &sel, &assoc_types);
        json(StatusCode::OK, body)
    })
}

// ---------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------

const OPERATORS: &[&str] = &[
    "EQ",
    "NEQ",
    "LT",
    "LTE",
    "GT",
    "GTE",
    "BETWEEN",
    "IN",
    "NOT_IN",
    "HAS_PROPERTY",
    "NOT_HAS_PROPERTY",
    "CONTAINS_TOKEN",
    "NOT_CONTAINS_TOKEN",
];

fn values_equal(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

fn compare(a: &str, b: &str) -> Ordering {
    match (numeric(a), numeric(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// `*` wildcard match, case-insensitive.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn contains_token(value: &str, needle: &str) -> bool {
    if glob(needle, value) {
        return true;
    }
    value
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"'))
        .filter(|t| !t.is_empty())
        .any(|token| {
            glob(needle, token) || glob(needle, token.trim_matches(|c: char| !c.is_alphanumeric()))
        })
}

struct Filter {
    property: String,
    operator: String,
    value: Option<String>,
    high: Option<String>,
    values: Vec<String>,
}

fn parse_filter(f: &Value) -> Result<Filter, String> {
    let property = f["propertyName"]
        .as_str()
        .ok_or("propertyName is required")?
        .to_owned();
    let operator = f["operator"].as_str().unwrap_or("EQ").to_uppercase();
    if !OPERATORS.contains(&operator.as_str()) {
        return Err(format!(
            "Invalid operator {operator}. Supported operators: {}",
            OPERATORS.join(", ")
        ));
    }
    let values = f["values"]
        .as_array()
        .map(|a| a.iter().filter_map(scalar_string).collect())
        .unwrap_or_default();
    Ok(Filter {
        property,
        operator,
        value: scalar_string(&f["value"]),
        high: scalar_string(&f["highValue"]),
        values,
    })
}

fn filter_matches(
    store: &Store,
    ty: &ObjectType,
    id: &str,
    props: &Map<String, Value>,
    f: &Filter,
) -> bool {
    if let Some(other) = f.property.strip_prefix("associations.") {
        let targets: Vec<&String> = f.value.iter().chain(f.values.iter()).collect();
        let hit = targets
            .iter()
            .any(|t| assoc::associated_with(store, ty, id, other, t));
        return match f.operator.as_str() {
            "NEQ" | "NOT_IN" => !hit,
            _ => hit,
        };
    }
    let actual = props
        .get(&f.property)
        .and_then(scalar_string)
        .filter(|s| !s.is_empty());
    let value = f.value.as_deref().unwrap_or_default();
    match f.operator.as_str() {
        "HAS_PROPERTY" => actual.is_some(),
        "NOT_HAS_PROPERTY" => actual.is_none(),
        "EQ" => actual.is_some_and(|a| values_equal(&a, value)),
        "NEQ" => actual.is_none_or(|a| !values_equal(&a, value)),
        "IN" => actual.is_some_and(|a| f.values.iter().any(|v| values_equal(&a, v))),
        "NOT_IN" => actual.is_none_or(|a| !f.values.iter().any(|v| values_equal(&a, v))),
        "LT" => actual.is_some_and(|a| compare(&a, value) == Ordering::Less),
        "LTE" => actual.is_some_and(|a| compare(&a, value) != Ordering::Greater),
        "GT" => actual.is_some_and(|a| compare(&a, value) == Ordering::Greater),
        "GTE" => actual.is_some_and(|a| compare(&a, value) != Ordering::Less),
        "BETWEEN" => actual.is_some_and(|a| {
            let high = f.high.as_deref().unwrap_or(value);
            compare(&a, value) != Ordering::Less && compare(&a, high) != Ordering::Greater
        }),
        "CONTAINS_TOKEN" => actual.is_some_and(|a| contains_token(&a, value)),
        "NOT_CONTAINS_TOKEN" => actual.is_none_or(|a| !contains_token(&a, value)),
        _ => false,
    }
}

/// `POST /crm/v3/objects/{objectType}/search`
pub async fn search(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, _| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let groups = req["filterGroups"].as_array().cloned().unwrap_or_default();
        if groups.len() > 5 {
            return validation(svc, "There are too many filter groups. The limit is 5.");
        }
        let mut parsed: Vec<Vec<Filter>> = Vec::new();
        let mut total_filters = 0;
        for g in &groups {
            let filters = g["filters"].as_array().cloned().unwrap_or_default();
            if filters.len() > 6 {
                return validation(
                    svc,
                    "There are too many filters in a filter group. The limit is 6.",
                );
            }
            total_filters += filters.len();
            let mut group = Vec::new();
            for f in &filters {
                match parse_filter(f) {
                    Ok(filter) => group.push(filter),
                    Err(message) => return validation(svc, &message),
                }
            }
            parsed.push(group);
        }
        if total_filters > 18 {
            return validation(svc, "There are too many filters. The limit is 18.");
        }
        let limit = req["limit"].as_u64().unwrap_or(10) as usize;
        if limit > 200 {
            return validation(
                svc,
                "Paging limit of 200 exceeded. Please use a lower limit.",
            );
        }
        let limit = limit.max(1);
        let after = req["after"]
            .as_str()
            .and_then(|a| a.parse::<usize>().ok())
            .or_else(|| req["after"].as_u64().map(|a| a as usize))
            .unwrap_or(0);
        let query_words: Vec<String> = req["query"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let searchable = searchable_properties(&svc.store, &ty);

        let store = &svc.store;
        let mut hits: Vec<(&Value, Map<String, Value>)> = records(store, &ty, false)
            .into_iter()
            .map(|d| (d, all_props(&ty, d)))
            .filter(|(d, props)| {
                let id = d["id"].as_str().unwrap_or_default();
                let groups_ok = parsed.is_empty()
                    || parsed
                        .iter()
                        .any(|g| g.iter().all(|f| filter_matches(store, &ty, id, props, f)));
                let text: String = searchable
                    .iter()
                    .filter_map(|p| props.get(p).and_then(scalar_string))
                    .chain(std::iter::once(id.to_owned()))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                groups_ok && query_words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();

        let sorts: Vec<(String, bool)> = req["sorts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| match s {
                Value::String(name) => Some(match name.strip_prefix('-') {
                    Some(n) => (n.to_owned(), true),
                    None => (name.clone(), false),
                }),
                Value::Object(_) => Some((
                    s["propertyName"].as_str()?.to_owned(),
                    s["direction"].as_str() == Some("DESCENDING"),
                )),
                _ => None,
            })
            .collect();
        if !sorts.is_empty() {
            hits.sort_by(|(_, a), (_, b)| {
                for (name, desc) in &sorts {
                    let av = a
                        .get(name)
                        .and_then(scalar_string)
                        .filter(|s| !s.is_empty());
                    let bv = b
                        .get(name)
                        .and_then(scalar_string)
                        .filter(|s| !s.is_empty());
                    let ord = match (av, bv) {
                        (Some(x), Some(y)) => {
                            let o = compare(&x, &y);
                            if *desc { o.reverse() } else { o }
                        }
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => Ordering::Equal,
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                Ordering::Equal
            });
        }

        let requested: Vec<String> = req["properties"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let sel = Selection::from_list(requested);
        let renderer = Renderer::new(store, &ty);
        let total = hits.len();
        let results: Vec<Value> = hits
            .iter()
            .skip(after)
            .take(limit)
            .map(|(d, _)| renderer.render(d, &sel))
            .collect();
        let mut body = json_value!({ "total": total, "results": results });
        if let Some(p) = paging(total, after, limit, None) {
            body["paging"] = p;
        }
        json(StatusCode::OK, body)
    })
}

// ---------------------------------------------------------------------------------------
// Property value validation
// ---------------------------------------------------------------------------------------

const READ_ONLY: &[&str] = &[
    "hs_object_id",
    "createdate",
    "hs_createdate",
    "lastmodifieddate",
    "hs_lastmodifieddate",
];

fn invalid(name: &str, error: &str, message: String) -> Value {
    json_value!({
        "isValid": false,
        "message": message,
        "error": error,
        "name": name,
        "localizedErrorMessage": message,
    })
}

/// Checks and normalises property values the way HubSpot does on write: unknown and
/// read-only properties are rejected, enumerations must use an option value, numbers
/// must parse, and dates/datetimes are stored in HubSpot's canonical form.
pub fn validate_properties(
    store: &Store,
    ty: &ObjectType,
    input: &Value,
) -> Result<Map<String, Value>, Vec<Value>> {
    let Some(map) = input.as_object() else {
        return Err(vec![invalid(
            "properties",
            "INVALID_INPUT",
            "properties must be an object".into(),
        )]);
    };
    let defs = property_definitions(store, ty);
    let known = known_properties(store, ty);
    let mut out = Map::new();
    let mut errors = Vec::new();
    for (name, raw) in map {
        let Some(value) = (match raw {
            Value::Null => Some(String::new()),
            other => scalar_string(other),
        }) else {
            errors.push(invalid(
                name,
                "INVALID_INPUT",
                format!("{raw} is not a valid value for {name}"),
            ));
            continue;
        };
        if READ_ONLY.contains(&name.as_str()) {
            errors.push(invalid(
                name,
                "READ_ONLY_VALUE",
                format!("{name} is a read only property; its value cannot be set."),
            ));
            continue;
        }
        if !known.contains(name) {
            errors.push(invalid(
                name,
                "PROPERTY_DOESNT_EXIST",
                format!("Property \"{name}\" does not exist"),
            ));
            continue;
        }
        if value.is_empty() {
            out.insert(name.clone(), Value::String(value));
            continue;
        }
        let def = defs.iter().find(|d| d["name"] == name.as_str());
        let kind = def.and_then(|d| d["type"].as_str()).unwrap_or("string");
        let normalized = match kind {
            "number" => match value.trim().parse::<f64>() {
                Ok(_) => Ok(value.trim().to_owned()),
                Err(_) => Err(invalid(
                    name,
                    "INVALID_FLOAT",
                    format!("{value} was not a valid number."),
                )),
            },
            "bool" => match value.to_lowercase().as_str() {
                "true" | "false" => Ok(value.to_lowercase()),
                _ => Err(invalid(
                    name,
                    "INVALID_BOOLEAN",
                    format!("{value} was not a valid boolean (true or false)."),
                )),
            },
            "datetime" | "date" => {
                let ms = value
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .or_else(|| parse_ms(value.trim()));
                match ms {
                    Some(ms) if kind == "date" => Ok(iso_ms(ms)[..10].to_owned()),
                    Some(ms) => Ok(iso_ms(ms)),
                    None => Err(invalid(
                        name,
                        "INVALID_DATE",
                        format!(
                            "{value} was not a valid date. Use a UNIX timestamp in milliseconds or an ISO 8601 date."
                        ),
                    )),
                }
            }
            "enumeration" => {
                let options: Vec<&str> = def
                    .and_then(|d| d["options"].as_array())
                    .map(|o| o.iter().filter_map(|x| x["value"].as_str()).collect())
                    .unwrap_or_default();
                let in_use = |v: &str| {
                    store.collection(&ty.name).is_some_and(|c| {
                        c.values()
                            .any(|d| d["properties"][name.as_str()].as_str() == Some(v))
                    })
                };
                let bad: Vec<&str> = value
                    .split(';')
                    .filter(|v| !v.is_empty() && !options.contains(v) && !in_use(v))
                    .collect();
                if options.is_empty() || bad.is_empty() {
                    Ok(value.clone())
                } else {
                    Err(invalid(
                        name,
                        "INVALID_OPTION",
                        format!(
                            "{} was not one of the allowed options: [{}]",
                            bad.join(";"),
                            options.join(", ")
                        ),
                    ))
                }
            }
            _ => Ok(value.clone()),
        };
        match normalized {
            Ok(v) => {
                out.insert(name.clone(), Value::String(v));
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

fn invalid_properties(svc: &mut ServiceState, errors: &[Value]) -> Response {
    let message = format!(
        "Property values were not valid: {}",
        Value::Array(errors.to_vec())
    );
    validation(svc, &message)
}

/// Next id for a new record: one past the highest id in the type's id space (activities
/// share one space, like HubSpot engagements).
pub fn next_id(store: &Store, ty: &ObjectType) -> String {
    let names: Vec<&str> = if ty.is_engagement() {
        ENGAGEMENT_TYPES.to_vec()
    } else {
        vec![ty.name.as_str()]
    };
    let max = names
        .iter()
        .filter_map(|n| store.collection(n))
        .flat_map(|c| c.values())
        .filter_map(|d| d["id"].as_str().and_then(|s| s.parse::<u64>().ok()))
        .max()
        .unwrap_or(1000);
    (max + 1).to_string()
}

// ---------------------------------------------------------------------------------------
// Batch endpoints
// ---------------------------------------------------------------------------------------

fn batch_envelope(results: Vec<Value>, errors: Vec<Value>, now: i64) -> (StatusCode, Value) {
    let ts = iso_secs(now);
    let mut body = json_value!({ "status": "COMPLETE", "results": results });
    let status = if errors.is_empty() {
        StatusCode::OK
    } else {
        body["numErrors"] = Value::from(errors.len());
        body["errors"] = Value::Array(errors);
        StatusCode::MULTI_STATUS
    };
    body["startedAt"] = Value::String(ts.clone());
    body["completedAt"] = Value::String(ts);
    (status, body)
}

fn input_id(v: &Value) -> String {
    scalar_string(&v["id"]).unwrap_or_default()
}

/// `POST /crm/v3/objects/{objectType}/batch/read`
pub async fn batch_read(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let Some(inputs) = req["inputs"].as_array() else {
            return validation(svc, "Invalid input JSON: \"inputs\" is required");
        };
        if inputs.len() > 100 {
            return validation(svc, "Batch input limit of 100 exceeded.");
        }
        let archived = query_bool(&query, "archived") || req["archived"].as_bool() == Some(true);
        let id_property = req["idProperty"].as_str();
        let list = |k: &str| -> Vec<String> {
            req[k]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let sel = Selection::from_list(list("properties"));
        let history = list("propertiesWithHistory");
        let renderer = Renderer::new(&svc.store, &ty);
        let mut results = Vec::new();
        let mut seen = BTreeSet::new();
        let mut missing = Vec::new();
        for input in inputs {
            let id = input_id(input);
            match find(&svc.store, &ty, &id, id_property, archived) {
                Some(doc) => {
                    let key = doc["id"].as_str().unwrap_or_default().to_owned();
                    if !seen.insert(key) {
                        continue;
                    }
                    let mut out = renderer.render(doc, &sel);
                    if !history.is_empty() {
                        let all = all_props(&ty, doc);
                        let updated = doc["updatedAt"].clone();
                        let mut h = Map::new();
                        for name in &history {
                            if let Some(v) = all.get(name) {
                                h.insert(
                                    name.clone(),
                                    json_value!([{"value": v, "timestamp": updated, "sourceType": "CRM_UI"}]),
                                );
                            }
                        }
                        out["propertiesWithHistory"] = Value::Object(h);
                    }
                    results.push(out);
                }
                None => missing.push(Value::String(id)),
            }
        }
        let errors = if missing.is_empty() {
            Vec::new()
        } else {
            vec![json_value!({
                "status": "error",
                "category": "OBJECT_NOT_FOUND",
                "message": format!("Could not get some {} objects, they may be deleted or not exist. Check that ids are valid.", ty.upper()),
                "context": { "ids": missing },
            })]
        };
        let (status, body) = batch_envelope(results, errors, now);
        json(status, body)
    })
}

/// `POST /crm/v3/objects/{objectType}/batch/create`
pub async fn batch_create(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let Some(inputs) = req["inputs"].as_array().cloned() else {
            return validation(svc, "Invalid input JSON: \"inputs\" is required");
        };
        if inputs.len() > 100 {
            return validation(svc, "Batch input limit of 100 exceeded.");
        }
        // Validate everything before writing anything (HubSpot fails the whole batch).
        let mut planned = Vec::new();
        let mut emails: Vec<String> = Vec::new();
        for input in &inputs {
            let props = match validate_properties(&svc.store, &ty, &input["properties"]) {
                Ok(p) => p,
                Err(errors) => return invalid_properties(svc, &errors),
            };
            if ty.is_engagement() && props.get("hs_timestamp").is_none_or(|v| v == "") {
                let e = invalid(
                    "hs_timestamp",
                    "REQUIRED_FIELD",
                    "hs_timestamp is required".into(),
                );
                return invalid_properties(svc, &[e]);
            }
            if ty.name == "contacts"
                && let Some(email) = props.get("email").and_then(Value::as_str)
                && !email.is_empty()
            {
                let existing = find(&svc.store, &ty, email, Some("email"), false)
                    .and_then(|d| d["id"].as_str().map(str::to_owned));
                if let Some(existing) = existing {
                    let message = format!("Contact already exists. Existing ID: {existing}");
                    return error(svc, StatusCode::CONFLICT, "CONFLICT", &message);
                }
                if emails.iter().any(|e| e.eq_ignore_ascii_case(email)) {
                    return error(
                        svc,
                        StatusCode::CONFLICT,
                        "CONFLICT",
                        &format!("Duplicate email {email} in batch input"),
                    );
                }
                emails.push(email.to_owned());
            }
            let mut assocs = Vec::new();
            for a in input["associations"].as_array().into_iter().flatten() {
                let to_id = scalar_string(&a["to"]["id"]).unwrap_or_default();
                let mut types = Vec::new();
                for t in a["types"].as_array().into_iter().flatten() {
                    let Some(type_id) = t["associationTypeId"].as_u64() else {
                        return validation(svc, "associationTypeId is required");
                    };
                    let category = t["associationCategory"]
                        .as_str()
                        .unwrap_or("HUBSPOT_DEFINED")
                        .to_owned();
                    types.push((category, type_id));
                }
                // The target type is implied by the type id.
                let target = target_type(&svc.store, &ty, &types);
                let Some(target) = target else {
                    let message = format!(
                        "Invalid association types {:?} for {}",
                        types.iter().map(|t| t.1).collect::<Vec<_>>(),
                        ty.name
                    );
                    return validation(svc, &message);
                };
                if !assoc::object_exists(&svc.store, &target, &to_id) {
                    let message = format!(
                        "No {} with id {to_id} exists for association",
                        target.singular
                    );
                    return validation(svc, &message);
                }
                assocs.push((target, to_id, types));
            }
            planned.push((props, assocs));
        }

        let ts = iso_secs(now);
        let renderer = Renderer::new(&svc.store, &ty);
        let mut results = Vec::new();
        for (props, assocs) in planned {
            let id = next_id(&svc.store, &ty);
            let mut properties = props;
            properties.insert(ty.created_key().into(), Value::String(ts.clone()));
            properties.insert(ty.modified_key().into(), Value::String(ts.clone()));
            let written: Vec<String> = properties.keys().cloned().collect();
            let doc = json_value!({
                "id": id,
                "properties": properties,
                "createdAt": ts,
                "updatedAt": ts,
                "archived": false,
            });
            svc.store
                .collection_mut(&ty.name)
                .insert(id.clone(), doc.clone());
            for (target, to_id, types) in assocs {
                assoc::add(&mut svc.store, &ty, &id, &target, &to_id, &types);
            }
            results.push(renderer.render(&doc, &Selection::Written(written)));
        }
        let (_, body) = batch_envelope(results, Vec::new(), now);
        json(StatusCode::CREATED, body)
    })
}

/// The object type a set of association type ids points at, from `from`'s side.
fn target_type(store: &Store, from: &ObjectType, types: &[(String, u64)]) -> Option<ObjectType> {
    let mut candidates: Vec<ObjectType> = crate::types::STANDARD_TYPES
        .iter()
        .filter_map(|(name, ..)| resolve(store, name))
        .collect();
    if let Some(schemas) = store.collection("schemas") {
        candidates.extend(
            schemas
                .values()
                .filter_map(|s| s["objectTypeId"].as_str())
                .filter_map(|id| resolve(store, id)),
        );
    }
    candidates.into_iter().find(|to| {
        let defs = association_defs(store, from, to);
        !types.is_empty()
            && types
                .iter()
                .all(|(c, id)| defs.iter().any(|d| d.type_id == *id && d.category == *c))
    })
}

/// `POST /crm/v3/objects/{objectType}/batch/update`
pub async fn batch_update(
    State(world): State<SharedWorld>,
    Path(raw): Path<String>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(req) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let Some(ty) = resolve(&svc.store, &raw) else {
            return unknown_type(svc, &raw);
        };
        let Some(inputs) = req["inputs"].as_array().cloned() else {
            return validation(svc, "Invalid input JSON: \"inputs\" is required");
        };
        if inputs.len() > 100 {
            return validation(svc, "Batch input limit of 100 exceeded.");
        }
        let mut planned = Vec::new();
        let mut missing = Vec::new();
        for input in &inputs {
            let id = input_id(input);
            let props = match validate_properties(&svc.store, &ty, &input["properties"]) {
                Ok(p) => p,
                Err(errors) => return invalid_properties(svc, &errors),
            };
            match find(&svc.store, &ty, &id, input["idProperty"].as_str(), false)
                .and_then(|d| d["id"].as_str().map(str::to_owned))
            {
                Some(key) => planned.push((key, props)),
                None => missing.push(Value::String(id)),
            }
        }
        let ts = iso_secs(now);
        let modified = ty.modified_key();
        let mut updated_ids = Vec::new();
        for (key, props) in planned {
            let coll = svc.store.collection_mut(&ty.name);
            let Some(doc) = coll.get_mut(&key) else {
                continue;
            };
            if !doc["properties"].is_object() {
                doc["properties"] = json_value!({});
            }
            let written: Vec<String> = props.keys().cloned().collect();
            for (k, v) in props {
                doc["properties"][k.as_str()] = v;
            }
            doc["properties"][modified] = Value::String(ts.clone());
            doc["updatedAt"] = Value::String(ts.clone());
            updated_ids.push((key, written));
        }
        let renderer = Renderer::new(&svc.store, &ty);
        let results: Vec<Value> = updated_ids
            .iter()
            .filter_map(|(key, written)| {
                let doc = svc.store.collection(&ty.name)?.get(key)?;
                Some(renderer.render(doc, &Selection::Written(written.clone())))
            })
            .collect();
        let errors = if missing.is_empty() {
            Vec::new()
        } else {
            vec![json_value!({
                "status": "error",
                "category": "OBJECT_NOT_FOUND",
                "message": format!("Could not find {} objects with the given ids.", ty.upper()),
                "context": { "ids": missing },
            })]
        };
        let (status, body) = batch_envelope(results, errors, now);
        json(status, body)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_and_tokens() {
        assert!(glob("*@hooli.com", "dana@hooli.com"));
        assert!(glob("pied*", "Pied Piper"));
        assert!(!glob("pied", "Pied Piper"));
        assert!(contains_token("Pied Piper Inc", "piper"));
        assert!(!contains_token("Pied Piper Inc", "pip"));
    }

    #[test]
    fn comparisons_are_numeric_or_dates_when_possible() {
        assert_eq!(compare("9000", "10000"), Ordering::Less);
        assert_eq!(
            compare("2026-11-20T00:00:00.000Z", "2026-11-01"),
            Ordering::Greater
        );
        assert!(values_equal("ClosedWon", "closedwon"));
        assert!(values_equal("5.0", "5"));
    }
}
