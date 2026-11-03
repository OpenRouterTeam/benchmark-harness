//! The Zendesk REST API (`/api/v2`) on the world port: tickets (+ comments, audits), users,
//! organizations, attachments and Help Center. Every path also answers with a `.json` suffix,
//! as zenpy calls them.

use std::collections::HashMap;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use mock_core::{http::json, store::Store, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    NAME,
    model::{self, ApiError, ApiResult, BASE, Page},
};

type Q = Query<HashMap<String, String>>;

pub fn routes(router: Router<SharedWorld>) -> Router<SharedWorld> {
    router
        .route("/api/v2/tickets", get(tickets_index).post(tickets_create))
        .route(
            "/api/v2/tickets.json",
            get(tickets_index).post(tickets_create),
        )
        .route("/api/v2/tickets/{id}", get(ticket_show).put(ticket_update))
        .route("/api/v2/tickets/{id}/{sub}", get(ticket_sub))
        .route("/api/v2/users", get(users_index))
        .route("/api/v2/users.json", get(users_index))
        .route("/api/v2/users/{id}", get(user_show))
        .route("/api/v2/users/{id}/tickets/{kind}", get(user_tickets))
        .route("/api/v2/organizations", get(orgs_index))
        .route("/api/v2/organizations.json", get(orgs_index))
        .route("/api/v2/organizations/{id}", get(org_show))
        .route("/api/v2/organizations/{id}/{sub}", get(org_sub))
        .route("/api/v2/attachments/{id}", get(attachment_show))
        .route("/attachments/token/{token}", get(attachment_content))
        .route("/attachments/token/{token}/", get(attachment_content))
        .route("/api/v2/help_center/articles", get(articles_index))
        .route("/api/v2/help_center/articles.json", get(articles_index))
        .route("/api/v2/help_center/articles/{id}", get(article_show))
        .route("/api/v2/help_center/sections", get(sections_index))
        .route("/api/v2/help_center/sections.json", get(sections_index))
        .route("/api/v2/help_center/sections/{id}", get(section_show))
        .route("/api/v2/help_center/sections/{id}/{sub}", get(section_sub))
        .route("/api/v2/help_center/categories", get(categories_index))
        .route("/api/v2/help_center/categories.json", get(categories_index))
        .route("/api/v2/help_center/categories/{id}", get(category_show))
        .route(
            "/api/v2/help_center/categories/{id}/{sub}",
            get(category_sub),
        )
}

fn error(e: ApiError) -> Response {
    json(e.status, e.body)
}

fn respond(result: ApiResult<Value>) -> Response {
    match result {
        Ok(body) => json(StatusCode::OK, body),
        Err(e) => error(e),
    }
}

fn with_store<T>(world: &SharedWorld, f: impl FnOnce(&Store) -> T) -> T {
    let world = world.lock().expect("world lock");
    match world.service(NAME) {
        Some(s) => f(&s.store),
        None => f(&Store::default()),
    }
}

fn strip(seg: &str) -> &str {
    seg.strip_suffix(".json").unwrap_or(seg)
}

fn id_param(seg: &str) -> ApiResult<u64> {
    model::parse_id_str(seg).ok_or_else(ApiError::not_found)
}

fn page_params(q: &HashMap<String, String>, default_per_page: usize) -> (usize, usize) {
    let page = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    let per_page = q
        .get("per_page")
        .and_then(|p| p.parse().ok())
        .unwrap_or(default_per_page);
    (page, per_page)
}

fn page_url(path: &str, page: usize, per_page: usize, extra: &[(&str, &str)]) -> String {
    let mut url = format!("{BASE}{path}.json?page={page}&per_page={per_page}");
    for (k, v) in extra {
        url.push_str(&format!("&{k}={}", model::url_encode(v)));
    }
    url
}

/// Support API offset pagination: `{<key>: [...], next_page, previous_page, count}`.
fn support_list(
    key: &str,
    items: Vec<Value>,
    q: &HashMap<String, String>,
    path: &str,
    extra: &[(&str, &str)],
) -> Value {
    let (page, per_page) = page_params(q, 100);
    let p = Page::of(items, page, per_page);
    let mut out = Map::new();
    out.insert(key.to_owned(), Value::Array(p.items.clone()));
    out.insert(
        "next_page".to_owned(),
        if p.has_next() {
            json_value!(page_url(path, p.page + 1, p.per_page, extra))
        } else {
            Value::Null
        },
    );
    out.insert(
        "previous_page".to_owned(),
        if p.has_previous() {
            json_value!(page_url(path, p.page - 1, p.per_page, extra))
        } else {
            Value::Null
        },
    );
    out.insert("count".to_owned(), json_value!(p.count));
    Value::Object(out)
}

/// Help Center pagination: adds `page`, `per_page`, `page_count`, `sort_by`, `sort_order`.
fn hc_list(key: &str, items: Vec<Value>, q: &HashMap<String, String>, path: &str) -> Value {
    let (page, per_page) = page_params(q, 30);
    let p = Page::of(items, page, per_page);
    let link = |n: usize| json_value!(page_url(path, n, p.per_page, &[]));
    let mut out = Map::new();
    out.insert(key.to_owned(), Value::Array(p.items.clone()));
    out.insert("page".to_owned(), json_value!(p.page));
    out.insert(
        "previous_page".to_owned(),
        if p.has_previous() {
            link(p.page - 1)
        } else {
            Value::Null
        },
    );
    out.insert(
        "next_page".to_owned(),
        if p.has_next() {
            link(p.page + 1)
        } else {
            Value::Null
        },
    );
    out.insert("per_page".to_owned(), json_value!(p.per_page));
    out.insert("page_count".to_owned(), json_value!(p.page_count()));
    out.insert("count".to_owned(), json_value!(p.count));
    if key != "results" {
        out.insert("sort_by".to_owned(), json_value!("position"));
        out.insert("sort_order".to_owned(), json_value!("asc"));
    }
    Value::Object(out)
}

fn parse_root(body: &Bytes, root: &str) -> ApiResult<Map<String, Value>> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request("Unprocessable Entity", format!("Invalid JSON: {e}")))?;
    value
        .get(root)
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            ApiError::bad_request(
                "ParameterMissing",
                format!("param is missing or the value is empty: {root}"),
            )
        })
}

// ───────────────────────────── tickets ─────────────────────────────

fn sorted_tickets(
    store: &Store,
    q: &HashMap<String, String>,
    filter: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    let mut tickets: Vec<Value> = model::all_tickets(store)
        .into_iter()
        .filter(|t| filter(t))
        .collect();
    model::sort_tickets(
        &mut tickets,
        q.get("sort_by").map(String::as_str),
        q.get("sort_order").map(String::as_str),
    );
    tickets
}

fn sort_extra(q: &HashMap<String, String>) -> Vec<(&str, &str)> {
    ["sort_by", "sort_order"]
        .into_iter()
        .filter_map(|k| q.get(k).map(|v| (k, v.as_str())))
        .collect()
}

async fn tickets_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let tickets = sorted_tickets(store, &q, |_| true);
        support_list("tickets", tickets, &q, "/api/v2/tickets", &sort_extra(&q))
    });
    json(StatusCode::OK, body)
}

async fn tickets_create(State(world): State<SharedWorld>, body: Bytes) -> Response {
    let input = match parse_root(&body, "ticket") {
        Ok(i) => i,
        Err(e) => return error(e),
    };
    let mut world = world.lock().expect("world lock");
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    match model::create_ticket(store, &now, &input) {
        Ok((ticket, audit)) => {
            let location = ticket["url"].as_str().unwrap_or_default().to_owned();
            let mut resp = json(
                StatusCode::CREATED,
                json_value!({ "ticket": ticket, "audit": audit }),
            );
            if let Ok(v) = HeaderValue::from_str(&location) {
                resp.headers_mut().insert(header::LOCATION, v);
            }
            resp
        }
        Err(e) => error(e),
    }
}

async fn ticket_show(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| match strip(&id) {
        "show_many" => {
            let ids: Vec<u64> = q
                .get("ids")
                .map(|s| s.split(',').filter_map(|i| i.trim().parse().ok()).collect())
                .unwrap_or_default();
            let tickets: Vec<Value> = ids
                .iter()
                .filter_map(|i| model::get(store, model::TICKETS, *i))
                .map(|d| model::render_ticket(store, d))
                .collect();
            Ok(support_list(
                "tickets",
                tickets,
                &q,
                "/api/v2/tickets/show_many",
                &[],
            ))
        }
        "count" => Ok(
            json_value!({ "count": { "value": model::all_tickets(store).len(), "refreshed_at": null } }),
        ),
        _ => {
            let id = id_param(&id)?;
            let doc = model::get(store, model::TICKETS, id).ok_or_else(ApiError::not_found)?;
            Ok(json_value!({ "ticket": model::render_ticket(store, doc) }))
        }
    }))
}

async fn ticket_update(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let id = match id_param(&id) {
        Ok(id) => id,
        Err(e) => return error(e),
    };
    let input = match parse_root(&body, "ticket") {
        Ok(i) => i,
        Err(e) => return error(e),
    };
    let mut world = world.lock().expect("world lock");
    let now = world.clock.now_rfc3339();
    let store = &mut world.service_mut(NAME).store;
    respond(
        model::update_ticket(store, &now, id, &input)
            .map(|(ticket, audit)| json_value!({ "ticket": ticket, "audit": audit })),
    )
}

async fn ticket_sub(
    State(world): State<SharedWorld>,
    Path((id, sub)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        if model::get(store, model::TICKETS, id).is_none() {
            return Err(ApiError::not_found());
        }
        let path = format!("/api/v2/tickets/{id}/{}", strip(&sub));
        match strip(&sub) {
            "comments" => {
                let mut comments: Vec<Value> = model::ticket_comments(store, id)
                    .into_iter()
                    .map(model::render_comment)
                    .collect();
                if q.get("sort_order").map(String::as_str) == Some("desc") {
                    comments.reverse();
                }
                Ok(support_list("comments", comments, &q, &path, &[]))
            }
            "audits" => {
                let audits: Vec<Value> = store
                    .collection(model::AUDITS)
                    .map(|c| {
                        c.values()
                            .filter(|a| a["ticket_id"] == id)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(support_list("audits", audits, &q, &path, &[]))
            }
            "collaborators" | "followers" | "email_ccs" => Ok(
                json_value!({ "users": [], "next_page": null, "previous_page": null, "count": 0 }),
            ),
            "tags" => {
                let t = model::render_ticket(
                    store,
                    model::get(store, model::TICKETS, id).expect("exists"),
                );
                Ok(json_value!({ "tags": t["tags"] }))
            }
            _ => Err(ApiError::invalid_endpoint()),
        }
    }))
}

// ───────────────────────────── users / organizations ─────────────────────────────

async fn users_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let role = q.get("role");
        let users: Vec<Value> = model::all(store, model::USERS, model::render_user)
            .into_iter()
            .filter(|u| role.is_none_or(|r| u["role"] == r.as_str()))
            .collect();
        support_list("users", users, &q, "/api/v2/users", &[])
    });
    json(StatusCode::OK, body)
}

async fn user_show(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| match strip(&id) {
        "me" => {
            let me = model::current_user(store).ok_or_else(ApiError::not_found)?;
            let doc = model::get(store, model::USERS, me).ok_or_else(ApiError::not_found)?;
            Ok(json_value!({ "user": model::render_user(doc) }))
        }
        "search" => {
            let query = q
                .get("query")
                .or_else(|| q.get("email"))
                .cloned()
                .unwrap_or_default();
            let users = model::search_users(store, &query);
            Ok(support_list(
                "users",
                users,
                &q,
                "/api/v2/users/search",
                &[("query", &query)],
            ))
        }
        _ => {
            let id = id_param(&id)?;
            let doc = model::get(store, model::USERS, id).ok_or_else(ApiError::not_found)?;
            Ok(json_value!({ "user": model::render_user(doc) }))
        }
    }))
}

async fn user_tickets(
    State(world): State<SharedWorld>,
    Path((id, kind)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        if model::get(store, model::USERS, id).is_none() {
            return Err(ApiError::not_found());
        }
        let field = match strip(&kind) {
            "requested" => "requester_id",
            "assigned" => "assignee_id",
            "ccd" => "collaborator_ids",
            _ => return Err(ApiError::invalid_endpoint()),
        };
        let tickets = sorted_tickets(store, &q, |t| {
            t[field] == id
                || t[field]
                    .as_array()
                    .is_some_and(|a| a.contains(&json_value!(id)))
        });
        let path = format!("/api/v2/users/{id}/tickets/{}", strip(&kind));
        Ok(support_list("tickets", tickets, &q, &path, &sort_extra(&q)))
    }))
}

async fn orgs_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let orgs = model::all(store, model::ORGANIZATIONS, model::render_organization);
        support_list("organizations", orgs, &q, "/api/v2/organizations", &[])
    });
    json(StatusCode::OK, body)
}

async fn org_show(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        let doc = model::get(store, model::ORGANIZATIONS, id).ok_or_else(ApiError::not_found)?;
        Ok(json_value!({ "organization": model::render_organization(doc) }))
    }))
}

async fn org_sub(
    State(world): State<SharedWorld>,
    Path((id, sub)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        if model::get(store, model::ORGANIZATIONS, id).is_none() {
            return Err(ApiError::not_found());
        }
        let path = format!("/api/v2/organizations/{id}/{}", strip(&sub));
        match strip(&sub) {
            "tickets" => {
                let tickets = sorted_tickets(store, &q, |t| t["organization_id"] == id);
                Ok(support_list("tickets", tickets, &q, &path, &sort_extra(&q)))
            }
            "users" => {
                let users: Vec<Value> = model::all(store, model::USERS, model::render_user)
                    .into_iter()
                    .filter(|u| u["organization_id"] == id)
                    .collect();
                Ok(support_list("users", users, &q, &path, &[]))
            }
            _ => Err(ApiError::invalid_endpoint()),
        }
    }))
}

// ───────────────────────────── attachments ─────────────────────────────

async fn attachment_show(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        let a = model::attachment_by_id(store, id).ok_or_else(ApiError::not_found)?;
        Ok(json_value!({ "attachment": a }))
    }))
}

async fn attachment_content(
    State(world): State<SharedWorld>,
    Path(token): Path<String>,
) -> Response {
    with_store(&world, |store| {
        let url = format!("{BASE}/attachments/token/{token}/");
        match model::attachment_by_url(store, &url) {
            Some(a) => {
                let bytes = model::attachment_bytes(store, &a);
                let content_type = a["content_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, content_type)],
                    bytes,
                )
                    .into_response()
            }
            None => (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned())],
                b"<html><body>Not Found</body></html>".to_vec(),
            )
                .into_response(),
        }
    })
}

// ───────────────────────────── help center ─────────────────────────────

async fn articles_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let articles = model::all(store, model::ARTICLES, model::render_article);
        hc_list("articles", articles, &q, "/api/v2/help_center/articles")
    });
    json(StatusCode::OK, body)
}

async fn article_show(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        if strip(&id) == "search" {
            let query = q.get("query").cloned().unwrap_or_default();
            let results = model::search_articles(store, &query);
            return Ok(hc_list(
                "results",
                results,
                &q,
                "/api/v2/help_center/articles/search",
            ));
        }
        let id = id_param(&id)?;
        let doc = model::get(store, model::ARTICLES, id).ok_or_else(ApiError::not_found)?;
        Ok(json_value!({ "article": model::render_article(doc) }))
    }))
}

async fn sections_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let sections = model::all(store, model::SECTIONS, model::render_section);
        hc_list("sections", sections, &q, "/api/v2/help_center/sections")
    });
    json(StatusCode::OK, body)
}

async fn section_show(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        let doc = model::get(store, model::SECTIONS, id).ok_or_else(ApiError::not_found)?;
        Ok(json_value!({ "section": model::render_section(doc) }))
    }))
}

async fn section_sub(
    State(world): State<SharedWorld>,
    Path((id, sub)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        if model::get(store, model::SECTIONS, id).is_none() {
            return Err(ApiError::not_found());
        }
        if strip(&sub) != "articles" {
            return Err(ApiError::invalid_endpoint());
        }
        let articles: Vec<Value> = model::all(store, model::ARTICLES, model::render_article)
            .into_iter()
            .filter(|a| a["section_id"] == id)
            .collect();
        let path = format!("/api/v2/help_center/sections/{id}/articles");
        Ok(hc_list("articles", articles, &q, &path))
    }))
}

async fn categories_index(State(world): State<SharedWorld>, Query(q): Q) -> Response {
    let body = with_store(&world, |store| {
        let categories = model::all(store, model::CATEGORIES, model::render_category);
        hc_list(
            "categories",
            categories,
            &q,
            "/api/v2/help_center/categories",
        )
    });
    json(StatusCode::OK, body)
}

async fn category_show(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        let doc = model::get(store, model::CATEGORIES, id).ok_or_else(ApiError::not_found)?;
        Ok(json_value!({ "category": model::render_category(doc) }))
    }))
}

async fn category_sub(
    State(world): State<SharedWorld>,
    Path((id, sub)): Path<(String, String)>,
    Query(q): Q,
) -> Response {
    respond(with_store(&world, |store| {
        let id = id_param(&id)?;
        if model::get(store, model::CATEGORIES, id).is_none() {
            return Err(ApiError::not_found());
        }
        let sections: Vec<Value> = model::all(store, model::SECTIONS, model::render_section)
            .into_iter()
            .filter(|s| s["category_id"] == id)
            .collect();
        let path = format!("/api/v2/help_center/categories/{id}/{}", strip(&sub));
        match strip(&sub) {
            "sections" => Ok(hc_list("sections", sections, &q, &path)),
            "articles" => {
                let ids: Vec<Value> = sections.iter().map(|s| s["id"].clone()).collect();
                let articles: Vec<Value> =
                    model::all(store, model::ARTICLES, model::render_article)
                        .into_iter()
                        .filter(|a| ids.contains(&a["section_id"]))
                        .collect();
                Ok(hc_list("articles", articles, &q, &path))
            }
            _ => Err(ApiError::invalid_endpoint()),
        }
    }))
}
