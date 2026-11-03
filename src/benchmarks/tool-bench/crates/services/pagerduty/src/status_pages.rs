//! Status pages: pages, their severities/impacts/statuses/services, posts and post updates.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::util::{ApiErr, Ctx, Req, classic, created, default, norm_time, ok, parse_time, run};

fn page_or_404(ctx: &Ctx, id: &str) -> Result<Value, ApiErr> {
    ctx.get("status_pages", id).ok_or(ApiErr::NotFound)
}

fn page_ref(id: &str) -> Value {
    json!({ "id": id, "type": "status_page" })
}

fn render_page(mut doc: Value) -> Value {
    default(&mut doc, "type", json!("status_page"));
    default(&mut doc, "published_at", Value::Null);
    default(&mut doc, "status_page_type", json!("public"));
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    default(
        &mut doc,
        "url",
        json!(format!("https://status.example.com/{id}")),
    );
    doc
}

/// Items of a per-page collection (`status_page.id` scoped), rendered with `self`.
fn page_items(ctx: &Ctx, collection: &str, page: &str, segment: &str, kind: &str) -> Vec<Value> {
    ctx.docs(collection)
        .into_iter()
        .filter(|d| d["status_page"]["id"] == page)
        .map(|mut d| {
            let id = d["id"].as_str().unwrap_or_default().to_owned();
            default(&mut d, "type", json!(kind));
            default(
                &mut d,
                "self",
                json!(format!("{}/status_pages/{page}/{segment}/{id}", ctx.api)),
            );
            default(&mut d, "description", json!(""));
            d["status_page"] = page_ref(page);
            d
        })
        .collect()
}

pub async fn list_pages(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let kind = req.query.get("status_page_type");
        let items: Vec<Value> = ctx
            .docs("status_pages")
            .into_iter()
            .map(render_page)
            .filter(|p| kind.is_none_or(|k| p["status_page_type"] == k))
            .collect();
        classic(&req.query, "status_pages", items)
    })
}

macro_rules! page_list {
    ($name:ident, $collection:literal, $segment:literal, $kind:literal) => {
        pub async fn $name(
            State(world): State<SharedWorld>,
            Path(id): Path<String>,
            req: Req,
        ) -> Response {
            run(&world, &req, |ctx| {
                page_or_404(ctx, &id)?;
                let post_type = req.query.get("post_type");
                let items: Vec<Value> = page_items(ctx, $collection, &id, $segment, $kind)
                    .into_iter()
                    .filter(|d| post_type.is_none_or(|t| d["post_type"] == t))
                    .collect();
                classic(&req.query, $segment, items)
            })
        }
    };
}

page_list!(
    list_severities,
    "status_page_severities",
    "severities",
    "status_page_severity"
);
page_list!(
    list_impacts,
    "status_page_impacts",
    "impacts",
    "status_page_impact"
);
page_list!(
    list_statuses,
    "status_page_statuses",
    "statuses",
    "status_page_status"
);
page_list!(
    list_page_services,
    "status_page_services",
    "services",
    "status_page_service"
);

fn render_update(ctx: &Ctx, page: &str, mut u: Value) -> Value {
    let id = u["id"].as_str().unwrap_or_default().to_owned();
    let post = u["post"]["id"].as_str().unwrap_or_default().to_owned();
    default(&mut u, "type", json!("status_page_post_update"));
    default(
        &mut u,
        "self",
        json!(format!(
            "{}/status_pages/{page}/posts/{post}/post_updates/{id}",
            ctx.api
        )),
    );
    default(&mut u, "reviewed_status", json!("approved"));
    default(&mut u, "impacted_services", json!([]));
    default(&mut u, "update_frequency_ms", Value::Null);
    default(&mut u, "notify_subscribers", json!(false));
    u
}

fn render_post(ctx: &Ctx, page: &str, mut p: Value) -> Value {
    let id = p["id"].as_str().unwrap_or_default().to_owned();
    default(&mut p, "type", json!("status_page_post"));
    default(
        &mut p,
        "self",
        json!(format!("{}/status_pages/{page}/posts/{id}", ctx.api)),
    );
    p["status_page"] = page_ref(page);
    default(&mut p, "linked_resource", Value::Null);
    default(&mut p, "postmortem", Value::Null);
    let updates: Vec<Value> = updates_of(ctx, &id)
        .iter()
        .map(|u| json!({ "id": u["id"], "type": "status_page_post_update" }))
        .collect();
    p["updates"] = json!(updates);
    p
}

fn updates_of(ctx: &Ctx, post: &str) -> Vec<Value> {
    let mut items: Vec<Value> = ctx
        .docs("status_page_post_updates")
        .into_iter()
        .filter(|u| u["post"]["id"] == post)
        .collect();
    items.sort_by_key(|u| u["reported_at"].as_str().and_then(parse_time));
    items
}

fn post_or_404(ctx: &Ctx, page: &str, post: &str) -> Result<Value, ApiErr> {
    ctx.get("status_page_posts", post)
        .filter(|p| p["status_page"]["id"] == page)
        .ok_or(ApiErr::NotFound)
}

fn on_page(ctx: &Ctx, collection: &str, page: &str, id: Option<&str>) -> Option<Value> {
    let id = id?;
    ctx.docs(collection)
        .into_iter()
        .find(|d| d["id"] == id && d["status_page"]["id"] == page)
}

/// Validates one post update payload against the page's statuses, severities and services.
fn update_input(
    ctx: &Ctx,
    page: &str,
    post_type: &str,
    input: &Value,
    errors: &mut Vec<String>,
) -> Value {
    let message = input["message"].as_str().unwrap_or_default();
    if message.trim().is_empty() {
        errors.push("message can't be blank".to_owned());
    }
    for (field, collection) in [
        ("status", "status_page_statuses"),
        ("severity", "status_page_severities"),
    ] {
        match on_page(ctx, collection, page, input[field]["id"].as_str()) {
            Some(d) if d["post_type"] == post_type => {}
            Some(_) => errors.push(format!("{field} does not apply to {post_type} posts")),
            None => errors.push(format!("{field} not found on this status page")),
        }
    }
    let impacted: Vec<Value> = input["impacted_services"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for item in &impacted {
        if on_page(
            ctx,
            "status_page_services",
            page,
            item["service"]["id"].as_str(),
        )
        .is_none()
        {
            errors.push(format!(
                "impacted service {} not found on this status page",
                item["service"]["id"]
            ));
        }
        if on_page(
            ctx,
            "status_page_impacts",
            page,
            item["impact"]["id"].as_str(),
        )
        .is_none()
        {
            errors.push(format!(
                "impact {} not found on this status page",
                item["impact"]["id"]
            ));
        }
    }
    let reported = match input.get("reported_at").filter(|v| !v.is_null()) {
        Some(v) => norm_time(v).unwrap_or_else(|| {
            errors.push("reported_at is not a valid date/time".to_owned());
            String::new()
        }),
        None => ctx.now_str(),
    };
    json!({
        "type": "status_page_post_update",
        "message": message,
        "reviewed_status": "approved",
        "status": { "id": input["status"]["id"], "type": "status_page_status" },
        "severity": { "id": input["severity"]["id"], "type": "status_page_severity" },
        "impacted_services": impacted.iter().map(|i| json!({
            "service": { "id": i["service"]["id"], "type": "status_page_service" },
            "impact": { "id": i["impact"]["id"], "type": "status_page_impact" },
        })).collect::<Vec<_>>(),
        "update_frequency_ms": input.get("update_frequency_ms").cloned().unwrap_or(Value::Null),
        "notify_subscribers": input["notify_subscribers"].as_bool().unwrap_or(false),
        "reported_at": reported,
    })
}

pub async fn list_posts(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        page_or_404(ctx, &id)?;
        let post_type = req.query.get("post_type");
        let items: Vec<Value> = ctx
            .docs("status_page_posts")
            .into_iter()
            .filter(|p| p["status_page"]["id"] == id.as_str())
            .filter(|p| post_type.is_none_or(|t| p["post_type"] == t))
            .map(|p| render_post(ctx, &id, p))
            .collect();
        classic(&req.query, "posts", items)
    })
}

pub async fn create_post(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        page_or_404(ctx, &id)?;
        let body = req.json()?;
        let input = body.get("post").unwrap_or(&body);
        let mut errors = Vec::new();
        let title = input["title"].as_str().unwrap_or_default();
        if title.trim().is_empty() {
            errors.push("title can't be blank".to_owned());
        }
        let post_type = input["post_type"].as_str().unwrap_or_default().to_owned();
        if !["incident", "maintenance"].contains(&post_type.as_str()) {
            errors.push("post_type must be incident or maintenance".to_owned());
        }
        let starts = norm_time(&input["starts_at"]);
        let ends = norm_time(&input["ends_at"]);
        match (&starts, &ends) {
            (Some(s), Some(e)) if parse_time(e) < parse_time(s) => {
                errors.push("ends_at must be after starts_at".to_owned())
            }
            (Some(_), Some(_)) => {}
            _ => errors.push("starts_at and ends_at are required".to_owned()),
        }
        let updates_in = input["updates"].as_array().cloned().unwrap_or_default();
        if updates_in.is_empty() {
            errors.push("updates must contain at least one post update".to_owned());
        }
        let updates: Vec<Value> = updates_in
            .iter()
            .map(|u| update_input(ctx, &id, &post_type, u, &mut errors))
            .collect();
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let post_id = ctx.new_id("status_page_posts");
        let post = json!({
            "id": post_id,
            "type": "status_page_post",
            "post_type": post_type,
            "status_page": page_ref(&id),
            "linked_resource": input.get("linked_resource").cloned().unwrap_or(Value::Null),
            "postmortem": null,
            "title": title.trim(),
            "starts_at": starts,
            "ends_at": ends,
        });
        ctx.put("status_page_posts", &post_id, post.clone());
        for mut u in updates {
            let uid = ctx.new_id("status_page_post_updates");
            u["id"] = json!(uid);
            u["post"] = json!({ "id": post_id, "type": "status_page_post" });
            ctx.put("status_page_post_updates", &uid, u);
        }
        created(json!({ "post": render_post(ctx, &id, post) }))
    })
}

pub async fn get_post(
    State(world): State<SharedWorld>,
    Path((id, post)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        page_or_404(ctx, &id)?;
        let p = post_or_404(ctx, &id, &post)?;
        ok(json!({ "post": render_post(ctx, &id, p) }))
    })
}

pub async fn list_post_updates(
    State(world): State<SharedWorld>,
    Path((id, post)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        page_or_404(ctx, &id)?;
        post_or_404(ctx, &id, &post)?;
        let reviewed = req.query.get("reviewed_status");
        let items: Vec<Value> = updates_of(ctx, &post)
            .into_iter()
            .map(|u| render_update(ctx, &id, u))
            .filter(|u| reviewed.is_none_or(|r| u["reviewed_status"] == r))
            .collect();
        classic(&req.query, "post_updates", items)
    })
}

pub async fn create_post_update(
    State(world): State<SharedWorld>,
    Path((id, post)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        page_or_404(ctx, &id)?;
        let p = post_or_404(ctx, &id, &post)?;
        let body = req.json()?;
        let input = body.get("post_update").unwrap_or(&body);
        let mut errors = Vec::new();
        if let Some(given) = input["post"]["id"].as_str()
            && given != post
        {
            errors.push("post.id does not match the post in the path".to_owned());
        }
        let post_type = p["post_type"].as_str().unwrap_or("incident").to_owned();
        let mut update = update_input(ctx, &id, &post_type, input, &mut errors);
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let uid = ctx.new_id("status_page_post_updates");
        update["id"] = json!(uid);
        update["post"] = json!({ "id": post, "type": "status_page_post" });
        ctx.put("status_page_post_updates", &uid, update.clone());
        created(json!({ "post_update": render_update(ctx, &id, update) }))
    })
}
