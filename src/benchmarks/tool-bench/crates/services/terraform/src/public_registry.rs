//! The public Terraform Registry (`registry.terraform.io`): provider and module protocol
//! (`/v1`), provider docs and policy libraries (`/v2`).
//!
//! Data comes from the scenario's `public_*` collections when present, otherwise from a small
//! built-in snapshot (`assets/public_registry.json`) of real providers, modules and policies.

use std::sync::OnceLock;

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use mock_core::{
    http::{json as plain_json, jsonapi},
    world::SharedWorld,
};
use serde_json::{Value, json};

use crate::common::{Ctx, Db, contains_ci, encode, page_params, str_field, str_list};

const DEFAULTS: &str = include_str!("../assets/public_registry.json");

fn defaults() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(DEFAULTS).expect("public registry asset is valid JSON")
    })
}

/// Records of one registry collection: the scenario's own, or the built-in snapshot.
fn records(db: &Db, collection: &str, default_key: &str) -> Vec<Value> {
    if db.has_collection(collection) {
        return db.all(collection).into_iter().cloned().collect();
    }
    defaults()
        .get(default_key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route("/v1/providers/{ns}/{name}", get(provider_latest))
        .route("/v1/providers/{ns}/{name}/versions", get(provider_versions))
        .route("/v1/providers/{ns}/{name}/{version}", get(provider_version))
        .route("/v2/providers", get(list_providers_v2))
        .route("/v2/providers/{ns}/{name}", get(provider_v2))
        .route("/v2/provider-versions/{id}", get(provider_version_v2))
        .route("/v2/provider-docs", get(list_provider_docs))
        .route("/v2/provider-docs/{id}", get(get_provider_doc))
        .route("/v1/modules", get(list_modules))
        .route("/v1/modules/search", get(search_modules))
        .route("/v1/modules/{ns}", get(list_namespace_modules))
        .route("/v1/modules/{ns}/{name}/{provider}", get(module_latest))
        .route(
            "/v1/modules/{ns}/{name}/{provider}/versions",
            get(module_versions),
        )
        .route(
            "/v1/modules/{ns}/{name}/{provider}/{version}",
            get(module_version),
        )
        .route("/v2/policies", get(list_policies))
        .route("/v2/policies/{ns}/{name}/{version}", get(policy_version))
}

fn not_found() -> Response {
    plain_json(StatusCode::NOT_FOUND, json!({ "errors": ["Not Found"] }))
}

fn jsonapi_not_found() -> Response {
    jsonapi(
        StatusCode::NOT_FOUND,
        json!({ "errors": [{ "status": "404", "title": "Not Found" }] }),
    )
}

// ---------------------------------------------------------------- providers

fn find_provider(db: &Db, ns: &str, name: &str) -> Option<Value> {
    records(db, "public_providers", "providers")
        .into_iter()
        .find(|p| {
            str_field(p, "namespace").eq_ignore_ascii_case(ns)
                && str_field(p, "name").eq_ignore_ascii_case(name)
        })
}

/// Versions oldest first, as the registry lists them.
fn version_list(provider: &Value) -> Vec<Value> {
    provider
        .get("versions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn provider_docs_of(db: &Db, provider: &Value) -> Vec<Value> {
    let full = format!(
        "{}/{}",
        str_field(provider, "namespace"),
        str_field(provider, "name")
    );
    records(db, "public_provider_docs", "provider_docs")
        .into_iter()
        .filter(|d| str_field(d, "provider").eq_ignore_ascii_case(&full))
        .collect()
}

fn provider_v1(db: &Db, provider: &Value, version: &Value) -> Value {
    let ns = str_field(provider, "namespace");
    let name = str_field(provider, "name");
    let v = str_field(version, "version");
    let docs: Vec<Value> = provider_docs_of(db, provider)
        .iter()
        .map(|d| {
            json!({
                "id": str_field(d, "id"),
                "title": str_field(d, "title"),
                "path": str_field(d, "path"),
                "slug": str_field(d, "slug"),
                "category": str_field(d, "category"),
                "subcategory": d.get("subcategory").cloned().unwrap_or(json!("")),
                "language": d.get("language").cloned().unwrap_or(json!("hcl")),
            })
        })
        .collect();
    let versions: Vec<String> = version_list(provider)
        .iter()
        .map(|v| str_field(v, "version").to_owned())
        .collect();
    json!({
        "id": format!("{ns}/{name}/{v}"),
        "owner": provider.get("owner-name").cloned().unwrap_or(json!(ns)),
        "namespace": ns,
        "name": name,
        "alias": provider.get("alias").cloned().unwrap_or(json!(name)),
        "version": v,
        "tag": format!("v{v}"),
        "description": provider.get("description").cloned().unwrap_or(json!("")),
        "source": provider.get("source").cloned().unwrap_or(json!("")),
        "published_at": version.get("published-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
        "downloads": provider.get("downloads").cloned().unwrap_or(json!(0)),
        "tier": provider.get("tier").cloned().unwrap_or(json!("official")),
        "logo_url": provider.get("logo-url").cloned().unwrap_or(json!("")),
        "versions": versions,
        "docs": docs,
    })
}

async fn provider_latest(
    State(world): State<SharedWorld>,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(provider) = find_provider(&db, &ns, &name) else {
        return not_found();
    };
    let Some(latest) = version_list(&provider).last().cloned() else {
        return not_found();
    };
    plain_json(StatusCode::OK, provider_v1(&db, &provider, &latest))
}

async fn provider_version(
    State(world): State<SharedWorld>,
    Path((ns, name, version)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(provider) = find_provider(&db, &ns, &name) else {
        return not_found();
    };
    let wanted = version.trim_start_matches('v');
    match version_list(&provider)
        .into_iter()
        .find(|v| str_field(v, "version") == wanted)
    {
        Some(v) => plain_json(StatusCode::OK, provider_v1(&db, &provider, &v)),
        None => not_found(),
    }
}

async fn provider_versions(
    State(world): State<SharedWorld>,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(provider) = find_provider(&db, &ns, &name) else {
        return not_found();
    };
    let versions: Vec<Value> = version_list(&provider)
        .iter()
        .map(|v| {
            json!({
                "version": str_field(v, "version"),
                "protocols": ["5.0"],
                "platforms": [
                    { "os": "darwin", "arch": "arm64" },
                    { "os": "linux", "arch": "amd64" },
                    { "os": "linux", "arch": "arm64" },
                    { "os": "windows", "arch": "amd64" }
                ]
            })
        })
        .collect();
    plain_json(
        StatusCode::OK,
        json!({
            "id": format!("{}/{}", str_field(&provider, "namespace"), str_field(&provider, "name")),
            "versions": versions,
            "warnings": null
        }),
    )
}

fn provider_resource(provider: &Value) -> Value {
    let id = str_field(provider, "id");
    let ns = str_field(provider, "namespace");
    let name = str_field(provider, "name");
    json!({
        "type": "providers",
        "id": id,
        "attributes": {
            "alias": provider.get("alias").cloned().unwrap_or(json!(name)),
            "description": provider.get("description").cloned().unwrap_or(json!("")),
            "downloads": provider.get("downloads").cloned().unwrap_or(json!(0)),
            "featured": provider.get("featured").cloned().unwrap_or(json!(false)),
            "full-name": format!("{ns}/{name}"),
            "logo-url": provider.get("logo-url").cloned().unwrap_or(json!("")),
            "name": name,
            "namespace": ns,
            "owner-name": provider.get("owner-name").cloned().unwrap_or(json!(ns)),
            "robots-noindex": false,
            "source": provider.get("source").cloned().unwrap_or(json!("")),
            "tier": provider.get("tier").cloned().unwrap_or(json!("official")),
            "unlisted": false,
            "warning": ""
        },
        "links": { "self": format!("/v2/providers/{id}") }
    })
}

fn provider_version_resource(v: &Value) -> Value {
    let id = str_field(v, "id");
    let version = str_field(v, "version");
    json!({
        "type": "provider-versions",
        "id": id,
        "attributes": {
            "description": v.get("description").cloned().unwrap_or(json!("")),
            "downloads": v.get("downloads").cloned().unwrap_or(json!(0)),
            "published-at": v.get("published-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
            "tag": format!("v{version}"),
            "version": version,
        },
        "links": { "self": format!("/v2/provider-versions/{id}") }
    })
}

async fn provider_v2(
    State(world): State<SharedWorld>,
    Path((ns, name)): Path<(String, String)>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(provider) = find_provider(&db, &ns, &name) else {
        return jsonapi_not_found();
    };
    let versions = version_list(&provider);
    let mut resource = provider_resource(&provider);
    resource["relationships"] = json!({
        "provider-versions": {
            "data": versions.iter().map(|v| json!({ "id": str_field(v, "id"), "type": "provider-versions" })).collect::<Vec<_>>(),
            "links": { "related": format!("/v2/providers/{}/provider-versions", str_field(&provider, "id")) }
        }
    });
    let mut body = json!({ "data": resource });
    if ctx.q.has_include("provider-versions") {
        body["included"] = Value::Array(versions.iter().map(provider_version_resource).collect());
    }
    jsonapi(StatusCode::OK, body)
}

async fn provider_version_v2(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    for p in records(&db, "public_providers", "providers") {
        if let Some(v) = version_list(&p)
            .into_iter()
            .find(|v| str_field(v, "id") == id)
        {
            return jsonapi(
                StatusCode::OK,
                json!({ "data": provider_version_resource(&v) }),
            );
        }
    }
    jsonapi_not_found()
}

/// JSON:API list with the registry's pagination links and meta.
fn registry_list(
    path: &str,
    ctx: &Ctx,
    default_size: usize,
    items: Vec<Value>,
    included: Vec<Value>,
) -> Response {
    let number = page_params(&ctx.q).0;
    let size = ctx
        .q
        .get("page[size]")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default_size)
        .clamp(1, 100);
    let total = items.len();
    let total_pages = total.div_ceil(size).max(1);
    let page: Vec<Value> = items
        .into_iter()
        .skip((number - 1) * size)
        .take(size)
        .collect();
    let others: Vec<String> = ctx
        .q
        .0
        .iter()
        .filter(|(k, _)| k != "page[number]" && k != "page[size]")
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    let link = |n: usize| {
        let mut parts = others.clone();
        parts.push(format!("page%5Bnumber%5D={n}"));
        parts.push(format!("page%5Bsize%5D={size}"));
        format!("{path}?{}", parts.join("&"))
    };
    let prev = (number > 1).then(|| number - 1);
    let next = (number < total_pages).then(|| number + 1);
    let mut body = json!({
        "data": page,
        "links": {
            "first": link(1),
            "last": link(total_pages),
            "next": next.map(link),
            "prev": prev.map(link),
        },
        "meta": { "pagination": {
            "page-size": size,
            "current-page": number,
            "next-page": next,
            "prev-page": prev,
            "total-pages": total_pages,
            "total-count": total,
        } }
    });
    if !included.is_empty() {
        body["included"] = Value::Array(included);
    }
    jsonapi(StatusCode::OK, body)
}

async fn list_providers_v2(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let tiers = ctx.q.list("filter[tier]");
    let namespace = ctx.q.value("filter[namespace]").map(str::to_owned);
    let mut providers: Vec<Value> = records(&db, "public_providers", "providers")
        .into_iter()
        .filter(|p| {
            (tiers.is_empty() || tiers.iter().any(|t| t == str_field(p, "tier")))
                && namespace
                    .as_deref()
                    .is_none_or(|n| str_field(p, "namespace").eq_ignore_ascii_case(n))
        })
        .collect();
    providers.sort_by_key(|p| {
        std::cmp::Reverse(p.get("downloads").and_then(Value::as_u64).unwrap_or(0))
    });
    let items = providers.iter().map(provider_resource).collect();
    registry_list("/v2/providers", &ctx, 15, items, vec![])
}

/// The provider a provider-version ID belongs to.
fn provider_of_version(db: &Db, version_id: &str) -> Option<Value> {
    records(db, "public_providers", "providers")
        .into_iter()
        .find(|p| {
            version_list(p)
                .iter()
                .any(|v| str_field(v, "id") == version_id)
        })
}

fn doc_resource(doc: &Value, with_content: bool) -> Value {
    let id = str_field(doc, "id");
    let mut attributes = json!({
        "category": str_field(doc, "category"),
        "language": doc.get("language").cloned().unwrap_or(json!("hcl")),
        "path": str_field(doc, "path"),
        "slug": str_field(doc, "slug"),
        "subcategory": doc.get("subcategory").cloned().unwrap_or(Value::Null),
        "title": str_field(doc, "title"),
        "truncated": false,
    });
    if with_content {
        attributes["content"] = doc.get("content").cloned().unwrap_or(json!(""));
    }
    json!({
        "type": "provider-docs",
        "id": id,
        "attributes": attributes,
        "links": { "self": format!("/v2/provider-docs/{id}") }
    })
}

async fn list_provider_docs(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(version_id) = ctx.q.value("filter[provider-version]").map(str::to_owned) else {
        return jsonapi(
            StatusCode::BAD_REQUEST,
            json!({ "errors": [{ "status": "400", "title": "Bad Request", "detail": "filter[provider-version] is required" }] }),
        );
    };
    let docs = provider_of_version(&db, &version_id)
        .map(|p| provider_docs_of(&db, &p))
        .unwrap_or_default();
    let category = ctx.q.value("filter[category]").map(str::to_owned);
    let slug = ctx.q.value("filter[slug]").map(str::to_owned);
    let language = ctx.q.value("filter[language]").map(str::to_owned);
    let items = docs
        .iter()
        .filter(|d| {
            category
                .as_deref()
                .is_none_or(|c| str_field(d, "category") == c)
                && slug.as_deref().is_none_or(|s| str_field(d, "slug") == s)
                && language
                    .as_deref()
                    .is_none_or(|l| d.get("language").and_then(Value::as_str).unwrap_or("hcl") == l)
        })
        .map(|d| doc_resource(d, false))
        .collect();
    registry_list("/v2/provider-docs", &ctx, 15, items, vec![])
}

async fn get_provider_doc(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match records(&db, "public_provider_docs", "provider_docs")
        .into_iter()
        .find(|d| str_field(d, "id") == id)
    {
        Some(d) => jsonapi(StatusCode::OK, json!({ "data": doc_resource(&d, true) })),
        None => jsonapi_not_found(),
    }
}

// ---------------------------------------------------------------- modules

fn latest_version(module: &Value) -> String {
    str_list(module, "versions")
        .last()
        .cloned()
        .unwrap_or_else(|| "1.0.0".to_owned())
}

fn module_summary(module: &Value) -> Value {
    let ns = str_field(module, "namespace");
    let name = str_field(module, "name");
    let provider = str_field(module, "provider");
    let version = latest_version(module);
    json!({
        "id": format!("{ns}/{name}/{provider}/{version}"),
        "owner": module.get("owner").cloned().unwrap_or(json!("")),
        "namespace": ns,
        "name": name,
        "version": version,
        "provider": provider,
        "provider_logo_url": format!("/images/providers/{provider}.png"),
        "description": module.get("description").cloned().unwrap_or(json!("")),
        "source": module.get("source").cloned().unwrap_or(json!("")),
        "tag": format!("v{version}"),
        "published_at": module.get("published_at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
        "downloads": module.get("downloads").cloned().unwrap_or(json!(0)),
        "verified": module.get("verified").cloned().unwrap_or(json!(false)),
    })
}

fn modules_page(modules: Vec<Value>, ctx: &Ctx, path: &str) -> Response {
    let offset = ctx
        .q
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = ctx
        .q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(15)
        .clamp(1, 100);
    let total = modules.len();
    let page: Vec<Value> = modules
        .iter()
        .skip(offset)
        .take(limit)
        .map(module_summary)
        .collect();
    let mut meta = json!({ "limit": limit, "current_offset": offset });
    let q = ctx
        .q
        .get("q")
        .map(|q| format!("&q={}", encode(q)))
        .unwrap_or_default();
    if offset + limit < total {
        meta["next_offset"] = json!(offset + limit);
        meta["next_url"] = json!(format!("{path}?limit={limit}&offset={}{q}", offset + limit));
    }
    if offset > 0 {
        let prev = offset.saturating_sub(limit);
        meta["prev_offset"] = json!(prev);
        meta["prev_url"] = json!(format!("{path}?limit={limit}&offset={prev}{q}"));
    }
    plain_json(StatusCode::OK, json!({ "meta": meta, "modules": page }))
}

fn module_filter(ctx: &Ctx) -> impl Fn(&Value) -> bool + '_ {
    move |m: &Value| {
        ctx.q
            .value("provider")
            .is_none_or(|p| str_field(m, "provider") == p)
            && ctx
                .q
                .value("namespace")
                .is_none_or(|n| str_field(m, "namespace").eq_ignore_ascii_case(n))
            && ctx.q.value("verified").is_none_or(|v| {
                (v == "true") == m.get("verified").and_then(Value::as_bool).unwrap_or(false)
            })
    }
}

fn by_downloads(mut modules: Vec<Value>) -> Vec<Value> {
    modules.sort_by_key(|m| {
        std::cmp::Reverse(m.get("downloads").and_then(Value::as_u64).unwrap_or(0))
    });
    modules
}

async fn list_modules(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let filter = module_filter(&ctx);
    let modules: Vec<Value> = records(&db, "public_modules", "modules")
        .into_iter()
        .filter(|m| filter(m))
        .collect();
    modules_page(by_downloads(modules), &ctx, "/v1/modules")
}

async fn list_namespace_modules(
    State(world): State<SharedWorld>,
    Path(ns): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let filter = module_filter(&ctx);
    let modules: Vec<Value> = records(&db, "public_modules", "modules")
        .into_iter()
        .filter(|m| str_field(m, "namespace").eq_ignore_ascii_case(&ns) && filter(m))
        .collect();
    modules_page(by_downloads(modules), &ctx, &format!("/v1/modules/{ns}"))
}

async fn search_modules(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let raw = ctx.q.get("q").unwrap_or("");
    let query = raw
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .trim()
        .to_lowercase();
    if query.is_empty() {
        return plain_json(
            StatusCode::BAD_REQUEST,
            json!({ "errors": ["q is required"] }),
        );
    }
    let terms: Vec<&str> = query.split_whitespace().collect();
    let filter = module_filter(&ctx);
    let modules: Vec<Value> = records(&db, "public_modules", "modules")
        .into_iter()
        .filter(|m| {
            let hay = format!(
                "{} {} {} {}",
                str_field(m, "namespace"),
                str_field(m, "name"),
                str_field(m, "provider"),
                str_field(m, "description")
            );
            terms.iter().all(|t| contains_ci(&hay, t)) && filter(m)
        })
        .collect();
    modules_page(by_downloads(modules), &ctx, "/v1/modules/search")
}

fn find_module(db: &Db, ns: &str, name: &str, provider: &str) -> Option<Value> {
    records(db, "public_modules", "modules")
        .into_iter()
        .find(|m| {
            str_field(m, "namespace").eq_ignore_ascii_case(ns)
                && str_field(m, "name").eq_ignore_ascii_case(name)
                && str_field(m, "provider").eq_ignore_ascii_case(provider)
        })
}

/// Full module version details, shared with HCP Terraform's public registry proxy.
pub fn module_details_response(
    db: &Db,
    ns: &str,
    name: &str,
    provider: &str,
    version: Option<&str>,
) -> Response {
    let Some(module) = find_module(db, ns, name, provider) else {
        return not_found();
    };
    let versions = str_list(&module, "versions");
    let version = match version {
        Some(v) => {
            let v = v.trim_start_matches('v');
            if !versions.iter().any(|x| x == v) {
                return not_found();
            }
            v.to_owned()
        }
        None => latest_version(&module),
    };
    let mut details = module_summary(&module);
    let module_name = str_field(&module, "name");
    let module_provider = str_field(&module, "provider");
    details["id"] = json!(format!(
        "{}/{module_name}/{module_provider}/{version}",
        str_field(&module, "namespace")
    ));
    details["version"] = json!(version);
    details["tag"] = json!(format!("v{version}"));
    let mut root = module.get("root").cloned().unwrap_or(json!({}));
    for key in [
        "inputs",
        "outputs",
        "dependencies",
        "provider_dependencies",
        "resources",
    ] {
        if root.get(key).is_none() {
            root[key] = json!([]);
        }
    }
    root["path"] = json!("");
    root["name"] = json!(module_name);
    root["empty"] = json!(false);
    if root.get("readme").is_none() {
        root["readme"] = json!("");
    }
    details["root"] = root;
    details["submodules"] = module.get("submodules").cloned().unwrap_or(json!([]));
    details["examples"] = module.get("examples").cloned().unwrap_or(json!([]));
    details["providers"] = json!([module_provider]);
    details["versions"] = json!(versions);
    details["deprecation"] = Value::Null;
    plain_json(StatusCode::OK, details)
}

async fn module_latest(
    State(world): State<SharedWorld>,
    Path((ns, name, provider)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    module_details_response(&Db::new(&world), &ns, &name, &provider, None)
}

async fn module_version(
    State(world): State<SharedWorld>,
    Path((ns, name, provider, version)): Path<(String, String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    module_details_response(&Db::new(&world), &ns, &name, &provider, Some(&version))
}

async fn module_versions(
    State(world): State<SharedWorld>,
    Path((ns, name, provider)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(module) = find_module(&db, &ns, &name, &provider) else {
        return not_found();
    };
    let versions: Vec<Value> = str_list(&module, "versions")
        .iter()
        .map(|v| json!({ "version": v, "root": { "providers": [], "dependencies": [] }, "submodules": [] }))
        .collect();
    plain_json(
        StatusCode::OK,
        json!({ "modules": [{
            "source": format!("{}/{}/{}", str_field(&module, "namespace"), str_field(&module, "name"), str_field(&module, "provider")),
            "versions": versions
        }] }),
    )
}

// ---------------------------------------------------------------- policies

fn policy_version_path(policy: &Value) -> String {
    format!(
        "/v2/policies/{}/{}/{}",
        str_field(policy, "namespace"),
        str_field(policy, "name"),
        policy
            .pointer("/latest-version/version")
            .and_then(Value::as_str)
            .unwrap_or("1.0.0")
    )
}

async fn list_policies(State(world): State<SharedWorld>, ctx: Ctx) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let policies = records(&db, "public_policies", "policies");
    let mut included = Vec::new();
    let items: Vec<Value> = policies
        .iter()
        .map(|p| {
            let latest = p.get("latest-version").cloned().unwrap_or(json!({}));
            let vid = str_field(&latest, "id").to_owned();
            if ctx.q.has_include("latest-version") {
                included.push(json!({
                    "type": "policy-library-versions",
                    "id": vid,
                    "attributes": {
                        "description": latest.get("description").cloned().unwrap_or(json!("")),
                        "downloads": latest.get("downloads").cloned().unwrap_or(json!(0)),
                        "published-at": latest.get("published-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
                        "readme": latest.get("readme").cloned().unwrap_or(json!("")),
                        "source": p.get("source").cloned().unwrap_or(json!("")),
                        "tag": format!("v{}", str_field(&latest, "version")),
                        "version": str_field(&latest, "version"),
                    },
                    "links": { "self": policy_version_path(p) }
                }));
            }
            json!({
                "type": "policy-libraries",
                "id": str_field(p, "id"),
                "attributes": {
                    "downloads": p.get("downloads").cloned().unwrap_or(json!(0)),
                    "full-name": format!("{}/{}", str_field(p, "namespace"), str_field(p, "name")),
                    "ingress": p.get("ingress").cloned().unwrap_or(json!("vcs")),
                    "name": str_field(p, "name"),
                    "namespace": str_field(p, "namespace"),
                    "owner-name": p.get("owner-name").cloned().unwrap_or(json!(str_field(p, "namespace"))),
                    "source": p.get("source").cloned().unwrap_or(json!("")),
                    "title": str_field(p, "title"),
                    "verified": p.get("verified").cloned().unwrap_or(json!(true)),
                },
                "relationships": { "latest-version": {
                    "data": { "id": vid, "type": "policy-library-versions" },
                    "links": { "related": policy_version_path(p) }
                } },
                "links": { "self": format!("/v2/policies/{}/{}", str_field(p, "namespace"), str_field(p, "name")) }
            })
        })
        .collect();
    registry_list("/v2/policies", &ctx, 15, items, included)
}

async fn policy_version(
    State(world): State<SharedWorld>,
    Path((ns, name, version)): Path<(String, String, String)>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(policy) = records(&db, "public_policies", "policies")
        .into_iter()
        .find(|p| {
            str_field(p, "namespace").eq_ignore_ascii_case(&ns)
                && str_field(p, "name").eq_ignore_ascii_case(&name)
        })
    else {
        return jsonapi_not_found();
    };
    let latest = policy.get("latest-version").cloned().unwrap_or(json!({}));
    let known: Vec<String> = {
        let mut v = str_list(&policy, "versions");
        v.push(str_field(&latest, "version").to_owned());
        v
    };
    if !known.iter().any(|v| v == &version) {
        return jsonapi_not_found();
    }
    let policies = policy
        .get("policies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let modules = policy
        .get("policy-modules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let full_name = format!(
        "{}/{}",
        str_field(&policy, "namespace"),
        str_field(&policy, "name")
    );
    let mut included = Vec::new();
    let wanted = ctx.q.list("include");
    if wanted.iter().any(|w| w == "policies") {
        for p in &policies {
            included.push(json!({
                "type": "policies",
                "id": str_field(p, "id"),
                "attributes": {
                    "description": p.get("description").cloned().unwrap_or(json!("")),
                    "downloads": p.get("downloads").cloned().unwrap_or(json!(0)),
                    "full-name": format!("{full_name}/{}", str_field(p, "name")),
                    "name": str_field(p, "name"),
                    "shasum": str_field(p, "shasum"),
                    "shasum-type": "sha256",
                    "title": str_field(p, "title"),
                },
                "links": { "self": format!("/v2/policies/{full_name}/{version}/policy/{}.sentinel", str_field(p, "name")) }
            }));
        }
    }
    if wanted.iter().any(|w| w == "policy-modules") {
        for m in &modules {
            included.push(json!({
                "type": "policy-modules",
                "id": str_field(m, "id"),
                "attributes": {
                    "description": m.get("description").cloned().unwrap_or(json!("")),
                    "downloads": m.get("downloads").cloned().unwrap_or(json!(0)),
                    "full-name": format!("{full_name}/{}", str_field(m, "name")),
                    "name": str_field(m, "name"),
                    "shasum": str_field(m, "shasum"),
                    "shasum-type": "sha256",
                    "title": str_field(m, "name"),
                },
                "links": { "self": format!("/v2/policies/{full_name}/{version}/policy-module/{}.sentinel", str_field(m, "name")) }
            }));
        }
    }
    if wanted.iter().any(|w| w == "policy-library") {
        included.push(json!({
            "type": "policy-libraries",
            "id": str_field(&policy, "id"),
            "attributes": {
                "name": str_field(&policy, "name"),
                "namespace": str_field(&policy, "namespace"),
                "full-name": full_name,
                "title": str_field(&policy, "title"),
                "downloads": policy.get("downloads").cloned().unwrap_or(json!(0)),
                "verified": policy.get("verified").cloned().unwrap_or(json!(true)),
            }
        }));
    }
    let body = json!({
        "data": {
            "type": "policy-library-versions",
            "id": str_field(&latest, "id"),
            "attributes": {
                "description": latest.get("description").cloned().unwrap_or(json!("")),
                "downloads": latest.get("downloads").cloned().unwrap_or(json!(0)),
                "published-at": latest.get("published-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
                "readme": latest.get("readme").cloned().unwrap_or(json!("")),
                "source": policy.get("source").cloned().unwrap_or(json!("")),
                "tag": format!("v{version}"),
                "version": version,
            },
            "relationships": {
                "policies": { "data": policies.iter().map(|p| json!({ "type": "policies", "id": str_field(p, "id") })).collect::<Vec<_>>() },
                "policy-library": { "data": { "type": "policy-libraries", "id": str_field(&policy, "id") } },
                "policy-modules": { "data": modules.iter().map(|m| json!({ "type": "policy-modules", "id": str_field(m, "id") })).collect::<Vec<_>>() },
            },
            "links": { "self": format!("/v2/policies/{full_name}/{version}") }
        },
        "included": included
    });
    jsonapi(StatusCode::OK, body)
}
