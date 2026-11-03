//! Private registry (modules, providers, no-code modules) of HCP Terraform organizations.

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use mock_core::{http::json as plain_json, world::SharedWorld};
use serde_json::{Value, json};

use crate::{
    NAME,
    common::{
        Ctx, Db, attributes, body_json, contains_ci, data, data_with_included, invalid,
        list_response, new_id, not_found, opt_str, relationship_id, str_field, str_list,
    },
    public_registry, resources,
};

pub fn routes() -> Router<SharedWorld> {
    Router::new()
        .route(
            "/api/v2/organizations/{org}/registry-modules",
            get(list_modules),
        )
        .route(
            "/api/v2/organizations/{org}/registry-modules/{registry}/{ns}/{name}/{provider}",
            get(read_module),
        )
        .route("/api/v2/registry-modules/{id}", get(read_module_by_id))
        .route(
            "/api/registry/v1/modules/{ns}/{name}/{provider}",
            get(private_module_latest),
        )
        .route(
            "/api/registry/v1/modules/{ns}/{name}/{provider}/{version}",
            get(private_module_version),
        )
        .route(
            "/api/registry/public/v1/modules/{ns}/{name}/{provider}",
            get(public_module_latest),
        )
        .route(
            "/api/registry/public/v1/modules/{ns}/{name}/{provider}/{version}",
            get(public_module_version),
        )
        .route(
            "/api/registry/private/v2/modules/{ns}/{name}/{provider}/metadata/{version}",
            get(module_metadata),
        )
        .route(
            "/api/v2/organizations/{org}/registry-providers",
            get(list_providers),
        )
        .route(
            "/api/v2/organizations/{org}/registry-providers/{registry}/{ns}/{name}",
            get(read_provider),
        )
        .route("/api/v2/no-code-modules/{id}", get(read_no_code_module))
        .route(
            "/api/v2/no-code-modules/{id}/workspaces",
            post(create_no_code_workspace),
        )
}

fn registry_errors_404() -> Response {
    plain_json(StatusCode::NOT_FOUND, json!({ "errors": ["Not Found"] }))
}

fn module_included(db: &Db, module: &Value) -> Vec<Value> {
    db.filter("no_code_modules", |n| {
        str_field(n, "registry-module") == str_field(module, "id")
    })
    .into_iter()
    .map(resources::no_code_module)
    .collect()
}

async fn list_modules(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(o) = db.org(&org) else {
        return not_found();
    };
    let org_name = str_field(o, "name");
    let q = ctx.q.value("q").map(str::to_owned);
    let provider = ctx.q.value("filter[provider]").map(str::to_owned);
    let registry = ctx.q.value("filter[registry_name]").map(str::to_owned);
    let modules = db.filter("registry_modules", |m| {
        str_field(m, "organization") == org_name
            && q.as_deref().is_none_or(|q| {
                contains_ci(str_field(m, "name"), q)
                    || contains_ci(str_field(m, "namespace"), q)
                    || contains_ci(str_field(m, "provider"), q)
            })
            && provider
                .as_deref()
                .is_none_or(|p| str_field(m, "provider") == p)
            && registry
                .as_deref()
                .is_none_or(|r| opt_str(m, "registry-name").unwrap_or("private") == r)
    });
    let mut included = Vec::new();
    if ctx.q.has_include("no-code-modules") {
        for m in &modules {
            included.extend(module_included(&db, m));
        }
    }
    let items = modules
        .into_iter()
        .map(|m| resources::registry_module(&db, m))
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/registry-modules"),
        &ctx.q,
        items,
        included,
    )
}

fn find_module<'a>(
    db: &Db<'a>,
    org: &str,
    registry: &str,
    ns: &str,
    name: &str,
    provider: &str,
) -> Option<&'a Value> {
    db.all("registry_modules").into_iter().find(|m| {
        str_field(m, "organization").eq_ignore_ascii_case(org)
            && opt_str(m, "registry-name").unwrap_or("private") == registry
            && str_field(m, "namespace").eq_ignore_ascii_case(ns)
            && str_field(m, "name") == name
            && str_field(m, "provider") == provider
    })
}

async fn read_module(
    State(world): State<SharedWorld>,
    Path((org, registry, ns, name, provider)): Path<(String, String, String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match find_module(&db, &org, &registry, &ns, &name, &provider) {
        Some(m) => data_with_included(resources::registry_module(&db, m), module_included(&db, m)),
        None => not_found(),
    }
}

async fn read_module_by_id(State(world): State<SharedWorld>, Path(id): Path<String>) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("registry_modules", &id) {
        Some(m) => data(StatusCode::OK, resources::registry_module(&db, m)),
        None => not_found(),
    }
}

/// Terraform-registry-protocol module details (the shape `/v1/modules/...` returns).
fn module_details(module: &Value, version: Option<&str>) -> Option<Value> {
    let details = module.get("details").cloned().unwrap_or(json!({}));
    let mut versions = str_list(&details, "versions");
    if versions.is_empty() {
        versions = module
            .get("version-statuses")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .map(|s| str_field(s, "version").to_owned())
                    .collect()
            })
            .unwrap_or_default();
    }
    let latest = versions
        .last()
        .cloned()
        .unwrap_or_else(|| "1.0.0".to_owned());
    let version = match version {
        Some(v) if versions.iter().any(|x| x == v) => v.to_owned(),
        Some(_) => return None,
        None => latest,
    };
    let ns = str_field(module, "namespace");
    let name = str_field(module, "name");
    let provider = str_field(module, "provider");
    let mut root = details.get("root").cloned().unwrap_or(json!({}));
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
    root["name"] = json!(name);
    root["empty"] = json!(false);
    if root.get("readme").is_none() {
        root["readme"] = json!(format!(
            "# {name}\n\n{}",
            str_field(&details, "description")
        ));
    }
    Some(json!({
        "id": format!("{ns}/{name}/{provider}/{version}"),
        "owner": details.get("owner").cloned().unwrap_or(json!("")),
        "namespace": ns,
        "name": name,
        "version": version,
        "provider": provider,
        "provider_logo_url": format!("/images/providers/{provider}.png"),
        "description": details.get("description").cloned().unwrap_or(json!("")),
        "source": details.get("source").cloned().unwrap_or_else(|| {
            module.pointer("/vcs-repo/repository-http-url").cloned().unwrap_or(json!(""))
        }),
        "tag": format!("v{version}"),
        "published_at": module.get("updated-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
        "downloads": details.get("downloads").cloned().unwrap_or(json!(0)),
        "verified": false,
        "root": root,
        "submodules": details.get("submodules").cloned().unwrap_or(json!([])),
        "examples": details.get("examples").cloned().unwrap_or(json!([])),
        "providers": [provider],
        "versions": versions,
        "deprecation": null
    }))
}

fn private_details(
    world: &SharedWorld,
    ns: &str,
    name: &str,
    provider: &str,
    version: Option<&str>,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let module = db.all("registry_modules").into_iter().find(|m| {
        opt_str(m, "registry-name").unwrap_or("private") == "private"
            && str_field(m, "namespace").eq_ignore_ascii_case(ns)
            && str_field(m, "name") == name
            && str_field(m, "provider") == provider
    });
    match module.and_then(|m| module_details(m, version)) {
        Some(d) => plain_json(StatusCode::OK, d),
        None => registry_errors_404(),
    }
}

async fn private_module_latest(
    State(world): State<SharedWorld>,
    Path((ns, name, provider)): Path<(String, String, String)>,
) -> Response {
    private_details(&world, &ns, &name, &provider, None)
}

async fn private_module_version(
    State(world): State<SharedWorld>,
    Path((ns, name, provider, version)): Path<(String, String, String, String)>,
) -> Response {
    private_details(&world, &ns, &name, &provider, Some(&version))
}

async fn public_module_latest(
    State(world): State<SharedWorld>,
    Path((ns, name, provider)): Path<(String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    public_registry::module_details_response(&Db::new(&world), &ns, &name, &provider, None)
}

async fn public_module_version(
    State(world): State<SharedWorld>,
    Path((ns, name, provider, version)): Path<(String, String, String, String)>,
) -> Response {
    let world = world.lock().expect("world lock");
    public_registry::module_details_response(
        &Db::new(&world),
        &ns,
        &name,
        &provider,
        Some(&version),
    )
}

async fn module_metadata(
    State(world): State<SharedWorld>,
    Path((ns, name, provider, version)): Path<(String, String, String, String)>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let org = ctx
        .q
        .value("organization_name")
        .map(str::to_owned)
        .unwrap_or_else(|| ns.clone());
    let Some(module) = find_module(&db, &org, "private", &ns, &name, &provider) else {
        return not_found();
    };
    let Some(details) = module_details(module, Some(&version)) else {
        return not_found();
    };
    let inputs: Vec<Value> = details["root"]["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            json!({
                "name": str_field(i, "name"),
                "type": str_field(i, "type"),
                "description": str_field(i, "description"),
                "required": i.get("required").cloned().unwrap_or(json!(false)),
                "sensitive": i.get("sensitive").cloned().unwrap_or(json!(false)),
            })
        })
        .collect();
    let source = module
        .pointer("/vcs-repo/repository-http-url")
        .cloned()
        .unwrap_or(json!(""));
    jsonapi_doc(json!({
        "type": "module-version-metadata",
        "id": format!("modver-{}", &crate::common::hex_digest(&format!("{}{version}", str_field(module, "id")))[..16]),
        "attributes": {
            "git-ref-tag": format!("v{version}"),
            "git-repo-url": source,
            "input-variables": inputs,
            "name": name,
            "source-url": source,
            "version": version,
            "no-code": !db.filter("no_code_modules", |n| str_field(n, "registry-module") == str_field(module, "id")).is_empty(),
        }
    }))
}

fn jsonapi_doc(data_value: Value) -> Response {
    data(StatusCode::OK, data_value)
}

async fn list_providers(
    State(world): State<SharedWorld>,
    Path(org): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let Some(o) = db.org(&org) else {
        return not_found();
    };
    let org_name = str_field(o, "name");
    let q = ctx.q.value("q").map(str::to_owned);
    let registry = ctx.q.value("filter[registry_name]").map(str::to_owned);
    let providers = db.filter("registry_providers", |p| {
        str_field(p, "organization") == org_name
            && q.as_deref().is_none_or(|q| {
                contains_ci(str_field(p, "name"), q) || contains_ci(str_field(p, "namespace"), q)
            })
            && registry
                .as_deref()
                .is_none_or(|r| opt_str(p, "registry-name").unwrap_or("private") == r)
    });
    let mut included = Vec::new();
    if ctx.q.has_include("registry-provider-versions") {
        for p in &providers {
            included.extend(resources::registry_provider_versions(p));
        }
    }
    let items = providers
        .into_iter()
        .map(resources::registry_provider)
        .collect();
    list_response(
        &ctx.base,
        &format!("/api/v2/organizations/{org_name}/registry-providers"),
        &ctx.q,
        items,
        included,
    )
}

async fn read_provider(
    State(world): State<SharedWorld>,
    Path((org, registry, ns, name)): Path<(String, String, String, String)>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    let provider = db.all("registry_providers").into_iter().find(|p| {
        str_field(p, "organization").eq_ignore_ascii_case(&org)
            && opt_str(p, "registry-name").unwrap_or("private") == registry
            && str_field(p, "namespace").eq_ignore_ascii_case(&ns)
            && str_field(p, "name") == name
    });
    match provider {
        Some(p) => {
            let included = if ctx.q.has_include("registry-provider-versions") {
                resources::registry_provider_versions(p)
            } else {
                vec![]
            };
            data_with_included(resources::registry_provider(p), included)
        }
        None => not_found(),
    }
}

async fn read_no_code_module(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    ctx: Ctx,
) -> Response {
    let world = world.lock().expect("world lock");
    let db = Db::new(&world);
    match db.get("no_code_modules", &id) {
        Some(n) => {
            let included = if ctx.q.has_include("variable_options") {
                resources::no_code_variable_options(n)
            } else {
                vec![]
            };
            data_with_included(resources::no_code_module(n), included)
        }
        None => not_found(),
    }
}

async fn create_no_code_workspace(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let doc = match body_json(&body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let attrs = attributes(&doc);
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let (org, module_id) = match Db::new(world).get("no_code_modules", &id) {
        Some(n) => (
            str_field(n, "organization").to_owned(),
            str_field(n, "registry-module").to_owned(),
        ),
        None => return not_found(),
    };
    let name = attrs
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if name.is_empty() {
        return invalid("Name can't be blank", "name");
    }
    if Db::new(world).workspace_by_name(&org, &name).is_some() {
        return invalid("Name has already been taken", "name");
    }
    let project = relationship_id(&doc, "project");
    if let Some(p) = &project
        && Db::new(world).get("projects", p).is_none()
    {
        return not_found();
    }
    let now = world.clock.now_rfc3339();
    let ws_id = new_id(world, "workspaces", "workspace", "ws-");
    let ws = json!({
        "id": ws_id,
        "name": name,
        "organization": org,
        "project": project,
        "created-at": now,
        "updated-at": now,
        "locked": false,
        "auto-apply": attrs.get("auto-apply").cloned().unwrap_or(json!(false)),
        "execution-mode": attrs.get("execution-mode").cloned().unwrap_or(json!("remote")),
        "terraform-version": attrs.get("terraform-version").cloned().unwrap_or(json!("1.9.5")),
        "description": attrs.get("description").cloned().unwrap_or(json!("")),
        "source": "tfe-module",
        "source-module-id": module_id,
        "source-name": attrs.get("source-name").cloned().unwrap_or(Value::Null),
        "resource-count": 0,
        "tag-names": [],
        "tag-bindings": [],
    });
    world
        .service_mut(NAME)
        .store
        .collection_mut("workspaces")
        .insert(ws_id, ws.clone());
    data(
        StatusCode::CREATED,
        resources::workspace(&Db::new(world), &ws),
    )
}
