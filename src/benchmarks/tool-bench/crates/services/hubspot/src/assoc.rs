//! Associations. Each pair of records is one document in the `associations` collection,
//! keyed in a canonical orientation (`<typeA>/<idA>/<typeB>/<idB>`, types in name order)
//! so the grader finds it whichever side the agent created it from. `types` are stored
//! from the canonical `from` side and inverted on read.

use std::cmp::Ordering;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use mock_core::{http::json, store::Store, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    types::{AssocDef, ObjectType, association_defs, resolve},
    util::{
        bad_json, base_url, error, id_order, iso_secs, page_window, paging, validation,
        with_service,
    },
};

/// One associated record seen from the querying side.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub to_id: String,
    /// (category, typeId, label)
    pub types: Vec<(String, u64, Option<String>)>,
}

fn canonical(a: (&str, &str), b: (&str, &str)) -> bool {
    match a.0.cmp(b.0) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => id_order(a.1, b.1) != Ordering::Greater,
    }
}

pub fn key(from: &str, from_id: &str, to: &str, to_id: &str) -> String {
    if canonical((from, from_id), (to, to_id)) {
        format!("{from}/{from_id}/{to}/{to_id}")
    } else {
        format!("{to}/{to_id}/{from}/{from_id}")
    }
}

pub fn object_exists(store: &Store, ty: &ObjectType, id: &str) -> bool {
    store
        .collection(&ty.name)
        .and_then(|c| c.get(id))
        .is_some_and(|d| !d["archived"].as_bool().unwrap_or(false))
}

fn invert(defs_rev: &[AssocDef], type_id: u64) -> u64 {
    defs_rev
        .iter()
        .find(|d| d.type_id == type_id)
        .map_or(type_id, |d| d.inverse)
}

fn stored_types(doc: &Value) -> Vec<(String, u64)> {
    doc["types"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t["associationCategory"]
                    .as_str()
                    .unwrap_or("HUBSPOT_DEFINED")
                    .to_owned(),
                t["associationTypeId"].as_u64()?,
            ))
        })
        .collect()
}

/// Records of type `to` associated with `from/from_id`, in id order.
pub fn links(store: &Store, from: &ObjectType, from_id: &str, to: &ObjectType) -> Vec<Link> {
    let defs = association_defs(store, from, to);
    let defs_rev = association_defs(store, to, from);
    let label = |id: u64| {
        defs.iter()
            .find(|d| d.type_id == id)
            .and_then(|d| d.label.clone())
    };
    let mut out: Vec<Link> = Vec::new();
    let Some(coll) = store.collection("associations") else {
        return out;
    };
    for doc in coll.values() {
        let f = doc["fromObjectType"].as_str().unwrap_or_default();
        let fid = id_str(&doc["fromObjectId"]);
        let t = doc["toObjectType"].as_str().unwrap_or_default();
        let tid = id_str(&doc["toObjectId"]);
        let (other, types) = if f == from.name && fid == from_id && t == to.name {
            (tid, stored_types(doc))
        } else if t == from.name && tid == from_id && f == to.name {
            let types = stored_types(doc)
                .into_iter()
                .map(|(c, id)| (c, invert(&defs_rev, id)))
                .collect();
            (fid, types)
        } else {
            continue;
        };
        if !object_exists(store, to, &other) {
            continue;
        }
        out.push(Link {
            to_id: other,
            types: types
                .into_iter()
                .map(|(c, id)| {
                    let l = label(id);
                    (c, id, l)
                })
                .collect(),
        });
    }
    out.sort_by(|a, b| id_order(&a.to_id, &b.to_id));
    out
}

fn id_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Adds association types (given from `from`'s side). Labeled types also get the pair's
/// unlabeled default, and a new Primary company replaces the record's previous one.
pub fn add(
    store: &mut Store,
    from: &ObjectType,
    from_id: &str,
    to: &ObjectType,
    to_id: &str,
    types: &[(String, u64)],
) {
    let defs = association_defs(store, from, to);
    let mut wanted: Vec<(String, u64)> = types.to_vec();
    let labeled = types
        .iter()
        .any(|(_, id)| defs.iter().any(|d| d.type_id == *id && d.label.is_some()));
    if labeled
        && let Some(default) = defs
            .iter()
            .find(|d| d.label.is_none() && d.category == "HUBSPOT_DEFINED")
        && !wanted.iter().any(|(_, id)| *id == default.type_id)
    {
        wanted.push((default.category.clone(), default.type_id));
    }
    let primary = defs.iter().any(|d| {
        d.label.as_deref() == Some("Primary") && types.iter().any(|(_, id)| *id == d.type_id)
    });
    store_types(store, from, from_id, to, to_id, &wanted);

    if primary {
        let (owner, owner_id, company, company_id) = if to.name == "companies" {
            (from, from_id, to, to_id)
        } else {
            (to, to_id, from, from_id)
        };
        let owner_defs = association_defs(store, owner, company);
        let Some(owner_primary) = owner_defs
            .iter()
            .find(|d| d.label.as_deref() == Some("Primary"))
            .map(|d| d.type_id)
        else {
            return;
        };
        for link in links(store, owner, owner_id, company) {
            if link.to_id != company_id && link.types.iter().any(|t| t.1 == owner_primary) {
                remove_type(store, owner, owner_id, company, &link.to_id, owner_primary);
            }
        }
    }
}

fn store_types(
    store: &mut Store,
    from: &ObjectType,
    from_id: &str,
    to: &ObjectType,
    to_id: &str,
    types: &[(String, u64)],
) {
    let forward = canonical((&from.name, from_id), (&to.name, to_id));
    let defs = association_defs(store, from, to);
    let oriented: Vec<(String, u64)> = if forward {
        types.to_vec()
    } else {
        types
            .iter()
            .map(|(c, id)| (c.clone(), invert(&defs, *id)))
            .collect()
    };
    let k = key(&from.name, from_id, &to.name, to_id);
    let coll = store.collection_mut("associations");
    let (cf, cfid, ct, ctid) = if forward {
        (&from.name, from_id, &to.name, to_id)
    } else {
        (&to.name, to_id, &from.name, from_id)
    };
    let mut existing = coll.get(&k).map(stored_types).unwrap_or_default();
    for t in oriented {
        if !existing.iter().any(|e| e.1 == t.1) {
            existing.push(t);
        }
    }
    let types: Vec<Value> = existing
        .into_iter()
        .map(|(c, id)| json_value!({"associationCategory": c, "associationTypeId": id}))
        .collect();
    coll.insert(
        k,
        json_value!({
            "fromObjectType": cf,
            "fromObjectId": cfid,
            "toObjectType": ct,
            "toObjectId": ctid,
            "types": types,
        }),
    );
}

fn remove_type(
    store: &mut Store,
    from: &ObjectType,
    from_id: &str,
    to: &ObjectType,
    to_id: &str,
    type_id: u64,
) {
    let forward = canonical((&from.name, from_id), (&to.name, to_id));
    let stored_id = if forward {
        type_id
    } else {
        invert(&association_defs(store, from, to), type_id)
    };
    let k = key(&from.name, from_id, &to.name, to_id);
    if let Some(doc) = store.collection_mut("associations").get_mut(&k)
        && let Some(types) = doc["types"].as_array_mut()
    {
        types.retain(|t| t["associationTypeId"].as_u64() != Some(stored_id));
    }
}

/// The v3 `associations` block for one record: `{"companies": {"results": [{id, type}]}}`.
pub fn v3_block(
    store: &Store,
    from: &ObjectType,
    from_id: &str,
    to_types: &[String],
) -> Option<Value> {
    let mut out = Map::new();
    for raw in to_types {
        let Some(to) = resolve(store, raw) else {
            continue;
        };
        let defs = association_defs(store, from, &to);
        let results: Vec<Value> = links(store, from, from_id, &to)
            .into_iter()
            .flat_map(|l| {
                let defs = &defs;
                l.types.into_iter().map(move |(_, id, _)| {
                    let name = defs
                        .iter()
                        .find(|d| d.type_id == id)
                        .map_or_else(|| id.to_string(), |d| d.v3_name.clone());
                    json_value!({"id": l.to_id, "type": name})
                })
            })
            .collect();
        if !results.is_empty() {
            out.insert(to.name.clone(), json_value!({ "results": results }));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn num_or_str(id: &str) -> Value {
    id.parse::<u64>()
        .map_or_else(|_| Value::String(id.to_owned()), Value::from)
}

fn type_json((category, id, label): &(String, u64, Option<String>)) -> Value {
    json_value!({"category": category, "typeId": id, "label": label})
}

/// `GET /crm/v4/objects/{type}/{id}/associations/{toType}`
pub async fn list_v4(
    State(world): State<SharedWorld>,
    Path((raw_from, id, raw_to)): Path<(String, String, String)>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    with_service(&world, |svc, _| {
        let (Some(from), Some(to)) = (resolve(&svc.store, &raw_from), resolve(&svc.store, &raw_to))
        else {
            let bad = if resolve(&svc.store, &raw_from).is_none() {
                raw_from.clone()
            } else {
                raw_to.clone()
            };
            return crate::objects::unknown_type(svc, &bad);
        };
        let all = links(&svc.store, &from, &id, &to);
        let (limit, after) = page_window(&query, 500, 500);
        let results: Vec<Value> = all
            .iter()
            .skip(after)
            .take(limit)
            .map(|l| {
                json_value!({
                    "toObjectId": num_or_str(&l.to_id),
                    "associationTypes": l.types.iter().map(type_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut body = json_value!({ "results": results });
        let link = format!(
            "{}/crm/v4/objects/{raw_from}/{id}/associations/{raw_to}?limit={limit}",
            base_url(&headers)
        );
        if let Some(p) = paging(all.len(), after, limit, Some(&link)) {
            body["paging"] = p;
        }
        json(StatusCode::OK, body)
    })
}

/// `GET /crm/v4/associations/{from}/{to}/labels`
pub async fn labels(
    State(world): State<SharedWorld>,
    Path((raw_from, raw_to)): Path<(String, String)>,
) -> Response {
    with_service(&world, |svc, _| {
        let (Some(from), Some(to)) = (resolve(&svc.store, &raw_from), resolve(&svc.store, &raw_to))
        else {
            let bad = if resolve(&svc.store, &raw_from).is_none() {
                raw_from.clone()
            } else {
                raw_to.clone()
            };
            return crate::objects::unknown_type(svc, &bad);
        };
        let results: Vec<Value> = association_defs(&svc.store, &from, &to)
            .into_iter()
            .map(|d| json_value!({"category": d.category, "typeId": d.type_id, "label": d.label}))
            .collect();
        json(StatusCode::OK, json_value!({ "results": results }))
    })
}

/// `POST /crm/v4/associations/{from}/{to}/batch/create`
pub async fn batch_create(
    State(world): State<SharedWorld>,
    Path((raw_from, raw_to)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    with_service(&world, |svc, now| {
        let Ok(input) = serde_json::from_slice::<Value>(&body) else {
            return bad_json(svc);
        };
        let (Some(from), Some(to)) = (resolve(&svc.store, &raw_from), resolve(&svc.store, &raw_to))
        else {
            let bad = if resolve(&svc.store, &raw_from).is_none() {
                raw_from.clone()
            } else {
                raw_to.clone()
            };
            return crate::objects::unknown_type(svc, &bad);
        };
        let Some(inputs) = input["inputs"].as_array() else {
            return validation(svc, "Invalid input JSON: \"inputs\" is required");
        };
        let defs = association_defs(&svc.store, &from, &to);
        let mut planned = Vec::new();
        let mut errors = Vec::new();
        for item in inputs {
            let from_id = id_str(&item["from"]["id"]);
            let to_id = id_str(&item["to"]["id"]);
            let mut types = Vec::new();
            for t in item["types"].as_array().into_iter().flatten() {
                let category = t["associationCategory"]
                    .as_str()
                    .unwrap_or("HUBSPOT_DEFINED");
                let Some(type_id) = t["associationTypeId"].as_u64() else {
                    return validation(svc, "associationTypeId is required");
                };
                if !defs
                    .iter()
                    .any(|d| d.type_id == type_id && d.category == category)
                {
                    let message = format!(
                        "{category} association type {type_id} is not valid between {} and {}",
                        from.name, to.name
                    );
                    return error(svc, StatusCode::BAD_REQUEST, "VALIDATION_ERROR", &message);
                }
                types.push((category.to_owned(), type_id));
            }
            if types.is_empty() {
                return validation(svc, "At least one association type is required");
            }
            let mut missing = Vec::new();
            if !object_exists(&svc.store, &from, &from_id) {
                missing.push(("fromObjectId", from_id.clone(), from.singular.clone()));
            }
            if !object_exists(&svc.store, &to, &to_id) {
                missing.push(("toObjectId", to_id.clone(), to.singular.clone()));
            }
            if missing.is_empty() {
                planned.push((from_id, to_id, types));
            } else {
                for (field, id, kind) in missing {
                    errors.push(json_value!({
                        "status": "error",
                        "category": "VALIDATION_ERROR",
                        "subCategory": "crm.associations.INVALID_OBJECT_IDS",
                        "context": { field: [id.clone()] },
                        "message": format!("No {kind} with id {id} exists"),
                    }));
                }
            }
        }
        let mut results = Vec::new();
        for (from_id, to_id, types) in planned {
            add(&mut svc.store, &from, &from_id, &to, &to_id, &types);
            let labels: Vec<Value> = types
                .iter()
                .filter_map(|(_, id)| {
                    defs.iter()
                        .find(|d| d.type_id == *id)
                        .and_then(|d| d.label.clone())
                })
                .map(Value::String)
                .collect();
            results.push(json_value!({
                "fromObjectTypeId": from.type_id,
                "fromObjectId": num_or_str(&from_id),
                "toObjectTypeId": to.type_id,
                "toObjectId": num_or_str(&to_id),
                "labels": labels,
            }));
        }
        let ts = iso_secs(now);
        let mut body = json_value!({"status": "COMPLETE", "results": results});
        let status = if errors.is_empty() {
            StatusCode::CREATED
        } else {
            body["numErrors"] = Value::from(errors.len());
            body["errors"] = Value::Array(errors);
            StatusCode::MULTI_STATUS
        };
        body["startedAt"] = Value::String(ts.clone());
        body["completedAt"] = Value::String(ts);
        json(status, body)
    })
}

/// Whether `ty/id` is associated with `other/other_id` (search filters on
/// `associations.<type>`).
pub fn associated_with(
    store: &Store,
    ty: &ObjectType,
    id: &str,
    other: &str,
    other_id: &str,
) -> bool {
    let Some(other_ty) = resolve(store, other) else {
        return false;
    };
    links(store, ty, id, &other_ty)
        .iter()
        .any(|l| l.to_id == other_id)
}
