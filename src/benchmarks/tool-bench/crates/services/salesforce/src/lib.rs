//! Mock Salesforce org for `@tsmztech/mcp-server-salesforce` 0.0.8 (jsforce 3.x, REST API
//! v50.0 by default, Tooling log bodies at v58.0). Any bearer token is accepted.
//!
//! State: one collection per sObject, named by its API name (`Account`, `Opportunity`,
//! `Task`, ...) and keyed by the 18-character Id, so graders address records as
//! `/services/salesforce/Opportunity/006.../StageName`. Records hold plain field values (no
//! `attributes`). Deletes are soft (`IsDeleted: true`, visible to `queryAll`). New Ids are
//! `<key prefix>8c00000<serial>` plus the case-safe suffix, minted from `ServiceState.ids`.
//! The API user (owner and creator of new records) is the first active `User`.
//! `Profile` and `PermissionSet` fall back to built-in standard profiles when the dataset
//! has none.
//!
//! Endpoints (under `/services/data`):
//! - `GET /` versions, `GET /vXX.X` resources, `GET /vXX.X/limits`
//! - `GET /query`, `/queryAll` (SOQL, see [`soql`]), `/query/{locator}` (`queryMore`; the
//!   batch size comes from `Sforce-Query-Options: batchSize=N`, default 2000)
//! - `GET /search` (SOSL, see [`sosl`])
//! - `GET /sobjects` (global describe), `GET|POST /sobjects/{type}`,
//!   `GET /sobjects/{type}/describe`, `GET|PATCH|DELETE /sobjects/{type}/{id}`,
//!   `GET|PATCH /sobjects/{type}/{extIdField}/{value}` (upsert),
//!   `GET /sobjects/ApexLog/{id}/Body`
//! - `POST|PATCH|DELETE /composite/sobjects`, `POST /composite/sobjects/{type}` (retrieve),
//!   `PATCH /composite/sobjects/{type}/{extIdField}` (upsert)
//! - the same sObject, query and describe routes under `/tooling`, plus
//!   `GET /tooling/executeAnonymous` (see [`tooling`])
//!
//! Metadata API: `POST /services/Soap/m/{version}` implements create/read/update/upsert/
//! delete for `CustomObject` and `CustomField`; definitions are stored in the
//! `CustomObject` and `CustomField` collections and applied to describe and SOQL (see
//! [`soap`]).

#![recursion_limit = "256"]

pub mod db;
pub mod records;
pub mod schema;
pub mod soap;
pub mod soql;
pub mod sosl;
pub mod tooling;
pub mod util;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use mock_core::{http::json, service::MockService, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    db::{Db, SfError},
    schema::Schema,
    util::{percent_decode, query_pairs},
};

pub const NAME: &str = "salesforce";
pub const PORT: u16 = 8122;

/// Lowest and highest REST API versions the org serves.
const MIN_VERSION: u32 = 31;
const MAX_VERSION: u32 = 62;
const DEFAULT_BATCH: usize = 2000;

pub struct Salesforce;

impl MockService for Salesforce {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route("/services/data", any(versions))
            .route("/services/data/", any(versions))
            .route("/services/data/{*rest}", any(data_api))
            .route("/services/Soap/m/{version}", post(metadata_api))
            .fallback(fallback)
    }
}

fn errors(e: &SfError) -> Response {
    json(e.status, Value::Array(vec![e.rest()]))
}

async fn fallback() -> Response {
    errors(&SfError::not_found())
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            let mut parts = v.splitn(2, ' ');
            let scheme = parts.next().unwrap_or("");
            let token = parts.next().unwrap_or("").trim();
            (scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("OAuth"))
                && !token.is_empty()
        })
}

fn session_invalid() -> Response {
    json(
        StatusCode::UNAUTHORIZED,
        json_value!([{ "message": "Session expired or invalid", "errorCode": "INVALID_SESSION_ID" }]),
    )
}

fn release(v: u32) -> String {
    let k = v - 31;
    let season = ["Summer", "Winter", "Spring"][(k % 3) as usize];
    format!("{season} '{}", 14 + k.div_ceil(3))
}

async fn versions(method: Method) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return method_not_allowed(&method, "GET,HEAD");
    }
    let list: Vec<Value> = (MIN_VERSION..=MAX_VERSION)
        .map(|v| {
            json_value!({
                "label": release(v),
                "url": format!("/services/data/v{v}.0"),
                "version": format!("{v}.0"),
            })
        })
        .collect();
    json(StatusCode::OK, Value::Array(list))
}

fn method_not_allowed(method: &Method, allowed: &str) -> Response {
    json(
        StatusCode::METHOD_NOT_ALLOWED,
        json_value!([{
            "errorCode": "METHOD_NOT_ALLOWED",
            "message": format!("HTTP Method '{method}' not allowed. Allowed are {allowed}"),
        }]),
    )
}

/// `v50.0` -> `50.0` if served.
fn parse_version(seg: &str) -> Option<String> {
    let v = seg.strip_prefix('v')?;
    let (major, minor) = v.split_once('.')?;
    let major: u32 = major.parse().ok()?;
    (minor == "0" && (MIN_VERSION..=MAX_VERSION).contains(&major)).then(|| v.to_owned())
}

struct Req<'a> {
    method: Method,
    segs: Vec<String>,
    query: std::collections::BTreeMap<String, String>,
    headers: &'a HeaderMap,
    body: &'a [u8],
    version: String,
    tooling: bool,
}

async fn data_api(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers) {
        return session_invalid();
    }
    let rest = uri.path().strip_prefix("/services/data/").unwrap_or("");
    let mut segs: Vec<String> = rest.split('/').map(percent_decode).collect();
    while segs.len() > 1 && segs.last().is_some_and(String::is_empty) {
        segs.pop();
    }
    let Some(version) = segs.first().and_then(|s| parse_version(s)) else {
        return errors(&SfError::not_found());
    };
    segs.remove(0);
    let tooling = segs.first().is_some_and(|s| s == "tooling");
    if tooling {
        segs.remove(0);
    }
    let req = Req {
        method,
        segs,
        query: query_pairs(uri.query()),
        headers: &headers,
        body: &body,
        version,
        tooling,
    };
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now_ms = world.clock.now_unix() * 1000;
    let state = world.service_mut(NAME);
    match route(state, now_ms, &req) {
        Ok(r) => r,
        Err(e) => errors(&e),
    }
}

async fn metadata_api(State(world): State<SharedWorld>, uri: Uri, body: Bytes) -> Response {
    let version = uri.path().rsplit('/').next().unwrap_or("");
    if parse_version(&format!("v{version}")).is_none() {
        return soap::fault(
            "sf:UNSUPPORTED_API_VERSION",
            &format!("UNSUPPORTED_API_VERSION: Invalid Api version specified on URL: {version}"),
        );
    }
    let mut guard = world.lock().expect("world lock");
    let state = guard.service_mut(NAME);
    soap::handle(state, &String::from_utf8_lossy(&body))
}

fn parse_body(body: &[u8]) -> Result<Value, SfError> {
    if body.is_empty() {
        return Err(SfError::bad(
            "JSON_PARSER_ERROR",
            "The HTTP entity body is required, but this request has no entity body.",
        ));
    }
    serde_json::from_slice(body).map_err(|e| {
        SfError::bad(
            "JSON_PARSER_ERROR",
            format!("Unexpected character ('{}' (code {})): was expecting JSON value at [line:{}, column:{}]", char::from(body.get(e.column().saturating_sub(1)).copied().unwrap_or(b'?')), body.get(e.column().saturating_sub(1)).copied().unwrap_or(0), e.line(), e.column()),
        )
    })
}

fn object_map(v: &Value) -> Result<&Map<String, Value>, SfError> {
    v.as_object().ok_or_else(|| {
        SfError::bad(
            "JSON_PARSER_ERROR",
            "Cannot deserialize instance of SObject out of START_ARRAY token",
        )
    })
}

fn route(
    state: &mut mock_core::world::ServiceState,
    now_ms: i64,
    req: &Req,
) -> Result<Response, SfError> {
    let schema = Schema::load(&state.store);
    let segs: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let m = &req.method;
    let get = *m == Method::GET || *m == Method::HEAD;
    macro_rules! dbv {
        () => {
            Db {
                schema: &schema,
                store: &state.store,
                now_ms,
                version: &req.version,
                tooling: req.tooling,
            }
        };
    }
    let lookup = |name: &str| -> Result<schema::Object, SfError> {
        schema
            .get_in(name, req.tooling)
            .cloned()
            .ok_or_else(SfError::not_found)
    };
    match segs.as_slice() {
        [] | [""] if get => Ok(json(StatusCode::OK, resources(&req.version, req.tooling))),
        ["limits"] if get && !req.tooling => Ok(json(StatusCode::OK, limits())),
        ["query" | "queryAll"] if get => {
            let Some(q) = req.query.get("q") else {
                return Err(SfError::bad(
                    "MALFORMED_QUERY",
                    "A query string has to be specified",
                ));
            };
            let all = segs[0] == "queryAll";
            run_query(&dbv!(), q, all, 0, batch_size(req.headers))
        }
        ["query" | "queryAll", locator] if get => {
            let Some((soql, all, offset)) = decode_locator(locator) else {
                return Err(SfError::bad(
                    "INVALID_QUERY_LOCATOR",
                    "invalid query locator",
                ));
            };
            run_query(&dbv!(), &soql, all, offset, batch_size(req.headers))
        }
        ["search"] if get && !req.tooling => {
            let Some(q) = req.query.get("q") else {
                return Err(SfError::bad(
                    "MALFORMED_SEARCH",
                    "No search string provided",
                ));
            };
            Ok(json(StatusCode::OK, sosl::run(&dbv!(), q)?))
        }
        ["executeAnonymous"] if get && req.tooling => {
            let Some(code) = req.query.get("anonymousBody") else {
                return Err(SfError::bad(
                    "MISSING_ARGUMENT",
                    "anonymousBody: Required parameter is missing",
                ));
            };
            let result = tooling::check(code);
            tooling::maybe_log(state, now_ms, &req.version, code, &result);
            Ok(json(
                StatusCode::OK,
                json_value!({
                    "line": result.line,
                    "column": result.column,
                    "compiled": result.compiled,
                    "success": result.compiled,
                    "compileProblem": result.problem,
                    "exceptionStackTrace": null,
                    "exceptionMessage": null,
                }),
            ))
        }
        ["sobjects"] if get => Ok(json(
            StatusCode::OK,
            schema::global_describe(&schema, &req.version, req.tooling),
        )),
        ["sobjects"] => Ok(method_not_allowed(m, "GET,HEAD")),
        ["sobjects", ty] => {
            let o = lookup(ty)?;
            if get {
                let db = dbv!();
                let mut recent: Vec<&Value> = db
                    .rows(&o.name)
                    .into_iter()
                    .filter(|r| r["IsDeleted"] != true)
                    .collect();
                recent.sort_by(|a, b| {
                    b["LastModifiedDate"]
                        .as_str()
                        .cmp(&a["LastModifiedDate"].as_str())
                });
                let recent: Vec<Value> = recent
                    .into_iter()
                    .take(5)
                    .map(|r| {
                        let mut m = Map::new();
                        m.insert("attributes".into(), db.attributes(&o.name, r));
                        m.insert("Id".into(), r["Id"].clone());
                        if let Some(nf) = o.name_field() {
                            m.insert(nf.name.clone(), soql::field_value(r, &o.name, &nf.name));
                        }
                        Value::Object(m)
                    })
                    .collect();
                Ok(json(
                    StatusCode::OK,
                    json_value!({ "objectDescribe": schema::global_entry(&o, &req.version, req.tooling), "recentItems": recent }),
                ))
            } else if *m == Method::POST {
                let body = parse_body(req.body)?;
                let id = records::insert(
                    state,
                    &schema,
                    now_ms,
                    &req.version,
                    req.tooling,
                    &o,
                    object_map(&body)?,
                )?;
                let body = if req.tooling {
                    json_value!({ "id": id, "success": true, "errors": [], "warnings": [], "infos": [] })
                } else {
                    json_value!({ "id": id, "success": true, "errors": [] })
                };
                let mut resp = json(StatusCode::CREATED, body);
                let loc = dbv!().sobject_url(&o.name, &id);
                resp.headers_mut().insert(
                    header::LOCATION,
                    HeaderValue::from_str(&loc).expect("ascii"),
                );
                Ok(resp)
            } else {
                Ok(method_not_allowed(m, "GET,HEAD,POST"))
            }
        }
        ["sobjects", ty, "describe"] if get => {
            let o = lookup(ty)?;
            Ok(json(
                StatusCode::OK,
                schema::describe(&o, &req.version, req.tooling),
            ))
        }
        ["sobjects", ty, id, "Body"] if get && ty.eq_ignore_ascii_case("ApexLog") => {
            let db = dbv!();
            let rec = db.get("ApexLog", id).ok_or_else(SfError::not_found)?;
            let body = rec["Body"].as_str().unwrap_or("").to_owned();
            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain;charset=UTF-8")],
                body,
            )
                .into_response())
        }
        ["sobjects", ty, id] => {
            let o = lookup(ty)?;
            match *m {
                Method::GET | Method::HEAD => {
                    let full = records::locate(state, &schema, &o, id).map_err(|e| {
                        if e.code == "MALFORMED_ID" {
                            SfError::not_found()
                        } else {
                            e
                        }
                    })?;
                    let db = dbv!();
                    let rec = db.get(&o.name, &full).ok_or_else(SfError::not_found)?;
                    let fields: Option<Vec<String>> = req
                        .query
                        .get("fields")
                        .map(|f| f.split(',').map(str::to_owned).collect());
                    Ok(json(
                        StatusCode::OK,
                        records::full_record(&db, &o, rec, fields.as_deref())?,
                    ))
                }
                Method::PATCH => {
                    let body = parse_body(req.body)?;
                    if normalize_or_none(id).is_none() {
                        return Err(SfError::new(
                            StatusCode::NOT_FOUND,
                            "NOT_FOUND",
                            format!(
                                "Provided external ID field does not exist or is not accessible: {id}"
                            ),
                        ));
                    }
                    records::update(
                        state,
                        &schema,
                        now_ms,
                        &req.version,
                        req.tooling,
                        &o,
                        id,
                        object_map(&body)?,
                    )?;
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                Method::DELETE => {
                    records::delete(state, &schema, now_ms, req.tooling, &o, id)?;
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                _ => Ok(method_not_allowed(m, "GET,HEAD,PATCH,DELETE")),
            }
        }
        ["sobjects", ty, field, value] => {
            let o = lookup(ty)?;
            let Some(f) = o
                .field(field)
                .filter(|f| f.id_lookup() && (f.external_id || f.ty == schema::Ty::Id))
                .cloned()
            else {
                return Err(SfError::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!(
                        "Provided external ID field does not exist or is not accessible: {field}"
                    ),
                ));
            };
            let matches = records::find_by_external(&dbv!(), &o, &f, value);
            match *m {
                Method::GET | Method::HEAD => match matches.as_slice() {
                    [] => Err(SfError::not_found()),
                    [one] => {
                        let db = dbv!();
                        let rec = db.get(&o.name, one).ok_or_else(SfError::not_found)?;
                        Ok(json(
                            StatusCode::OK,
                            records::full_record(&db, &o, rec, None)?,
                        ))
                    }
                    many => Ok(json(
                        StatusCode::MULTIPLE_CHOICES,
                        Value::Array(
                            many.iter()
                                .map(|id| json_value!(dbv!().sobject_url(&o.name, id)))
                                .collect(),
                        ),
                    )),
                },
                Method::PATCH => {
                    let body = parse_body(req.body)?;
                    let input = object_map(&body)?;
                    let (id, created) =
                        upsert_one(state, &schema, now_ms, req, &o, &f, value, input, matches)?;
                    let status = if created {
                        StatusCode::CREATED
                    } else {
                        StatusCode::OK
                    };
                    Ok(json(
                        status,
                        json_value!({ "id": id, "success": true, "errors": [], "created": created }),
                    ))
                }
                _ => Ok(method_not_allowed(m, "GET,HEAD,PATCH")),
            }
        }
        ["composite", "sobjects"] => composite(state, &schema, now_ms, req),
        ["composite", "sobjects", ty] if *m == Method::POST => {
            let o = lookup(ty)?;
            let body = parse_body(req.body)?;
            let ids: Vec<String> = body["ids"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let fields: Option<Vec<String>> = body["fields"].as_array().map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            });
            let db = dbv!();
            let out: Vec<Value> = ids
                .iter()
                .map(
                    |id| match db.get(&o.name, id).filter(|r| r["IsDeleted"] != true) {
                        Some(rec) => records::full_record(&db, &o, rec, fields.as_deref()),
                        None => Ok(Value::Null),
                    },
                )
                .collect::<Result<_, _>>()?;
            Ok(json(StatusCode::OK, Value::Array(out)))
        }
        ["composite", "sobjects", ty, field] if *m == Method::PATCH => {
            let o = lookup(ty)?;
            let Some(f) = o
                .field(field)
                .filter(|f| f.external_id || f.ty == schema::Ty::Id)
                .cloned()
            else {
                return Err(SfError::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    format!(
                        "Provided external ID field does not exist or is not accessible: {field}"
                    ),
                ));
            };
            let body = parse_body(req.body)?;
            let rows = body["records"].as_array().cloned().unwrap_or_default();
            let mut out = Vec::new();
            for r in rows {
                let input = r.as_object().cloned().unwrap_or_default();
                let value = match input.get(&f.name) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    _ => {
                        out.push(json_value!({ "success": false, "errors": [SfError::bad("MISSING_ARGUMENT", format!("{} not specified", f.name)).collection()] }));
                        continue;
                    }
                };
                let matches = records::find_by_external(&dbv!(), &o, &f, &value);
                let mut input = input;
                input.remove(&f.name);
                match upsert_one(state, &schema, now_ms, req, &o, &f, &value, &input, matches) {
                    Ok((id, created)) => out.push(json_value!({ "id": id, "success": true, "errors": [], "created": created })),
                    Err(e) => out.push(json_value!({ "success": false, "errors": [e.collection()] })),
                }
            }
            Ok(json(StatusCode::OK, Value::Array(out)))
        }
        _ => Err(SfError::not_found()),
    }
}

fn normalize_or_none(id: &str) -> Option<String> {
    util::normalize_id(id)
}

#[allow(clippy::too_many_arguments)]
fn upsert_one(
    state: &mut mock_core::world::ServiceState,
    schema: &Schema,
    now_ms: i64,
    req: &Req,
    o: &schema::Object,
    f: &schema::Field,
    value: &str,
    input: &Map<String, Value>,
    matches: Vec<String>,
) -> Result<(String, bool), SfError> {
    match matches.as_slice() {
        [] => {
            let mut input = input.clone();
            if f.ty != schema::Ty::Id {
                input.insert(f.name.clone(), Value::String(value.to_owned()));
            } else {
                return Err(SfError::not_found());
            }
            Ok((
                records::insert(state, schema, now_ms, &req.version, req.tooling, o, &input)?,
                true,
            ))
        }
        [one] => Ok((
            records::update(
                state,
                schema,
                now_ms,
                &req.version,
                req.tooling,
                o,
                one,
                input,
            )?,
            false,
        )),
        _ => Err(SfError::new(
            StatusCode::MULTIPLE_CHOICES,
            "MULTIPLE_CHOICES",
            format!("Multiple records found for {}: {value}", f.name),
        )),
    }
}

/// sObject Collections: create (POST), update (PATCH), delete (DELETE ?ids=).
fn composite(
    state: &mut mock_core::world::ServiceState,
    schema: &Schema,
    now_ms: i64,
    req: &Req,
) -> Result<Response, SfError> {
    let rolled_back = || {
        SfError::bad(
            "ALL_OR_NONE_OPERATION_ROLLED_BACK",
            "Record rolled back because not all records were valid and the request was using AllOrNone header",
        )
    };
    match req.method {
        Method::POST | Method::PATCH => {
            let body = parse_body(req.body)?;
            let all_or_none = body["allOrNone"] == true;
            let rows = body["records"]
                .as_array()
                .cloned()
                .ok_or_else(|| SfError::bad("JSON_PARSER_ERROR", "records is required"))?;
            if rows.len() > 200 {
                return Err(SfError::bad(
                    "EXCEEDED_ID_LIMIT",
                    "record limit reached. cannot submit more than 200 records into this call",
                ));
            }
            let snapshot = state.clone();
            let mut results: Vec<Result<String, SfError>> = Vec::new();
            for r in &rows {
                let input = r.as_object().cloned().unwrap_or_default();
                let ty = r
                    .pointer("/attributes/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let outcome = (|| {
                    let o = schema.get_in(ty, req.tooling).cloned().ok_or_else(|| {
                        SfError::bad(
                            "INVALID_TYPE",
                            format!("sObject type '{ty}' is not supported."),
                        )
                    })?;
                    if req.method == Method::POST {
                        if input.contains_key("id") || input.contains_key("Id") {
                            return Err(SfError::bad(
                                "INVALID_FIELD",
                                "cannot specify Id in an insert call",
                            )
                            .with_fields(&["Id"]));
                        }
                        records::insert(
                            state,
                            schema,
                            now_ms,
                            &req.version,
                            req.tooling,
                            &o,
                            &input,
                        )
                    } else {
                        let Some(id) = input
                            .get("id")
                            .or_else(|| input.get("Id"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        else {
                            return Err(SfError::bad(
                                "MISSING_ARGUMENT",
                                "Id not specified in an update call",
                            ));
                        };
                        let mut input = input.clone();
                        input.remove("id");
                        records::update(
                            state,
                            schema,
                            now_ms,
                            &req.version,
                            req.tooling,
                            &o,
                            &id,
                            &input,
                        )
                        .map_err(|e| {
                            if e.code == "NOT_FOUND" {
                                SfError::bad(
                                    "INVALID_CROSS_REFERENCE_KEY",
                                    "invalid cross reference id",
                                )
                            } else if e.code == "MALFORMED_ID" {
                                SfError::bad("MALFORMED_ID", format!("malformed id {id}"))
                            } else {
                                e
                            }
                        })
                    }
                })();
                results.push(outcome);
            }
            if all_or_none && results.iter().any(Result::is_err) {
                *state = snapshot;
                let out: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        let e = match r {
                            Err(e) => e.clone(),
                            Ok(_) => rolled_back(),
                        };
                        json_value!({ "success": false, "errors": [e.collection()] })
                    })
                    .collect();
                return Ok(json(StatusCode::OK, Value::Array(out)));
            }
            Ok(json(
                StatusCode::OK,
                Value::Array(results.into_iter().map(collection_result).collect()),
            ))
        }
        Method::DELETE => {
            let ids: Vec<String> = req
                .query
                .get("ids")
                .map(|s| {
                    s.split(',')
                        .filter(|x| !x.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                return Err(SfError::bad(
                    "MISSING_ARGUMENT",
                    "ids parameter is required",
                ));
            }
            if ids.len() > 200 {
                return Err(SfError::bad(
                    "EXCEEDED_ID_LIMIT",
                    "record limit reached. cannot submit more than 200 records into this call",
                ));
            }
            let all_or_none = req.query.get("allOrNone").is_some_and(|v| v == "true");
            let snapshot = state.clone();
            let mut results = Vec::new();
            for id in &ids {
                let outcome = (|| {
                    let o = util::normalize_id(id)
                        .and_then(|full| schema.by_prefix(&full).cloned())
                        .ok_or_else(|| {
                            SfError::bad("MALFORMED_ID", format!("malformed id {id}"))
                        })?;
                    records::delete(state, schema, now_ms, req.tooling, &o, id).map_err(|e| match e
                        .code
                        .as_str()
                    {
                        "NOT_FOUND" => SfError::bad(
                            "INVALID_CROSS_REFERENCE_KEY",
                            "invalid cross reference id",
                        ),
                        "ENTITY_IS_DELETED" => {
                            SfError::bad("ENTITY_IS_DELETED", "entity is deleted")
                        }
                        _ => e,
                    })
                })();
                results.push(outcome);
            }
            if all_or_none && results.iter().any(Result::is_err) {
                *state = snapshot;
                let out: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        let e = match r {
                            Err(e) => e.clone(),
                            Ok(_) => rolled_back(),
                        };
                        json_value!({ "success": false, "errors": [e.collection()] })
                    })
                    .collect();
                return Ok(json(StatusCode::OK, Value::Array(out)));
            }
            Ok(json(
                StatusCode::OK,
                Value::Array(results.into_iter().map(collection_result).collect()),
            ))
        }
        _ => Ok(method_not_allowed(&req.method, "POST,PATCH,DELETE")),
    }
}

fn collection_result(r: Result<String, SfError>) -> Value {
    match r {
        Ok(id) => json_value!({ "id": id, "success": true, "errors": [] }),
        Err(e) => json_value!({ "success": false, "errors": [e.collection()] }),
    }
}

fn batch_size(headers: &HeaderMap) -> usize {
    headers
        .get("sforce-query-options")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',').find_map(|p| {
                p.trim()
                    .strip_prefix("batchSize=")
                    .and_then(|n| n.trim().parse::<usize>().ok())
            })
        })
        .map_or(DEFAULT_BATCH, |n| n.clamp(200, 2000))
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// Query locators carry the query itself: `01g<flag><hex soql>-<offset>`.
fn encode_locator(soql: &str, all: bool, offset: usize) -> String {
    format!("01g{}{}-{offset}", if all { 'A' } else { 'Q' }, hex(soql))
}

fn decode_locator(loc: &str) -> Option<(String, bool, usize)> {
    let rest = loc.strip_prefix("01g")?;
    let (body, offset) = rest.rsplit_once('-')?;
    let all = match body.chars().next()? {
        'A' => true,
        'Q' => false,
        _ => return None,
    };
    Some((unhex(&body[1..])?, all, offset.parse().ok()?))
}

fn run_query(
    db: &Db,
    soql_text: &str,
    all: bool,
    offset: usize,
    batch: usize,
) -> Result<Response, SfError> {
    let parsed = soql::parse(soql_text)?;
    let engine = soql::Engine {
        db,
        src: soql_text,
        all_rows: all,
        restrict: None,
    };
    let compiled = engine.compile(&parsed, None)?;
    let out = engine.run(&compiled)?;
    let available = out.records.len();
    let end = (offset + batch).min(available);
    let page: Vec<Value> = out
        .records
        .into_iter()
        .skip(offset)
        .take(end.saturating_sub(offset))
        .collect();
    let mut body = Map::new();
    body.insert("totalSize".into(), json_value!(out.total));
    let done = end >= available;
    body.insert("done".into(), json_value!(done));
    if !done {
        let base = if db.tooling {
            format!("/services/data/v{}/tooling", db.version)
        } else {
            format!("/services/data/v{}", db.version)
        };
        body.insert(
            "nextRecordsUrl".into(),
            json_value!(format!(
                "{base}/query/{}",
                encode_locator(soql_text, all, end)
            )),
        );
    }
    body.insert("records".into(), Value::Array(page));
    Ok(json(StatusCode::OK, Value::Object(body)))
}

fn resources(version: &str, tooling: bool) -> Value {
    let b = format!("/services/data/v{version}");
    if tooling {
        return json_value!({
            "tooling": format!("{b}/tooling"),
            "query": format!("{b}/tooling/query"),
            "search": format!("{b}/tooling/search"),
            "sobjects": format!("{b}/tooling/sobjects"),
            "executeAnonymous": format!("{b}/tooling/executeAnonymous"),
            "runTestsAsynchronous": format!("{b}/tooling/runTestsAsynchronous"),
            "runTestsSynchronous": format!("{b}/tooling/runTestsSynchronous"),
            "completions": format!("{b}/tooling/completions"),
        });
    }
    json_value!({
        "tooling": format!("{b}/tooling"),
        "metadata": format!("{b}/metadata"),
        "eclair": format!("{b}/eclair"),
        "folders": format!("{b}/folders"),
        "prechatForms": format!("{b}/prechatForms"),
        "contact-tracing": format!("{b}/contact-tracing"),
        "jsonxform": format!("{b}/jsonxform"),
        "chatter": format!("{b}/chatter"),
        "tabs": format!("{b}/tabs"),
        "appMenu": format!("{b}/appMenu"),
        "quickActions": format!("{b}/quickActions"),
        "queryAll": format!("{b}/queryAll"),
        "commerce": format!("{b}/commerce"),
        "wave": format!("{b}/wave"),
        "iot": format!("{b}/iot"),
        "analytics": format!("{b}/analytics"),
        "search": format!("{b}/search"),
        "identity": "https://toolbench.my.salesforce.com/id/00D8c0000000001EAA/0058c00000AbCdEAAV",
        "composite": format!("{b}/composite"),
        "parameterizedSearch": format!("{b}/parameterizedSearch"),
        "theme": format!("{b}/theme"),
        "nouns": format!("{b}/nouns"),
        "event": format!("{b}/event"),
        "recent": format!("{b}/recent"),
        "connect": format!("{b}/connect"),
        "licensing": format!("{b}/licensing"),
        "limits": format!("{b}/limits"),
        "process": format!("{b}/process"),
        "async-queries": format!("{b}/async-queries"),
        "query": format!("{b}/query"),
        "emailConnect": format!("{b}/emailConnect"),
        "compactLayouts": format!("{b}/compactLayouts"),
        "knowledgeManagement": format!("{b}/knowledgeManagement"),
        "sobjects": format!("{b}/sobjects"),
        "actions": format!("{b}/actions"),
        "support": format!("{b}/support"),
    })
}

fn limits() -> Value {
    let pair = |max: u64, used: u64| json_value!({ "Max": max, "Remaining": max - used });
    json_value!({
        "ConcurrentAsyncGetReportInstances": pair(200, 0),
        "ConcurrentSyncReportRuns": pair(20, 0),
        "DailyApiRequests": pair(15000, 127),
        "DailyAsyncApexExecutions": pair(250000, 0),
        "DailyBulkApiBatches": pair(15000, 0),
        "DailyBulkV2QueryJobs": pair(10000, 0),
        "DailyGenericStreamingApiEvents": pair(10000, 0),
        "DailyStreamingApiEvents": pair(10000, 0),
        "DailyWorkflowEmails": pair(390, 0),
        "DataStorageMB": pair(5, 1),
        "FileStorageMB": pair(20, 0),
        "HourlyTimeBasedWorkflow": pair(50, 0),
        "MassEmail": pair(10, 0),
        "SingleEmail": pair(15, 0),
        "StreamingApiConcurrentClients": pair(20, 0),
    })
}
