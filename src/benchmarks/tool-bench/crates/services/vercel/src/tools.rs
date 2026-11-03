//! The 15 tools of nganiet/mcp-vercel @e6f9173, answered with the same text the server
//! produces: its argument validation (zod), the query/body it sends, and its formatting of
//! the API response. Where the server's `vercelFetch` swallows an API error and returns
//! `null`, the handler's null branch is reproduced (including the misleading ones).
//!
//! Two faithful oddities: `isError: true` sits on the content block, never on the result;
//! and some handlers print `Error: ZodError: ...` (template string of the error) while
//! others print `Error: ...` (its message).
//!
//! One deliberate deviation: the pinned `index.ts` destructures `request.params.args`
//! (always undefined), so the real server ignores every argument except for
//! `vercel-get-environments`. Here arguments are honoured, as the tool schemas intend.

use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::{
    api,
    model::{ApiResult, Query},
    zod::{self, Field, Kind, STR, opt, req, str_min},
};

fn block(text: impl Into<String>) -> Value {
    json!({ "type": "text", "text": text.into() })
}

fn error_block(text: impl Into<String>) -> Value {
    json!({ "type": "text", "text": text.into(), "isError": true })
}

fn result(blocks: Vec<Value>) -> Value {
    json!({ "content": blocks })
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).expect("JSON value serializes")
}

/// JS truthiness of an argument value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// JS `String(v)`.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        Value::Array(items) => items
            .iter()
            .map(|i| {
                if i.is_null() {
                    String::new()
                } else {
                    js_string(i)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// Query parameters from the arguments the handler copies when truthy.
fn query_from(args: &Value, keys: &[&str]) -> Query {
    keys.iter()
        .filter(|k| truthy(args.get(**k)))
        .map(|k| ((*k).to_owned(), js_string(&args[*k])))
        .collect()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Dispatches one `tools/call`.
pub fn call(world: &mut WorldState, name: &str, args: &Value) -> Value {
    match name {
        "vercel-list-all-deployments" => list_all_deployments(world, args),
        "vercel-get-environments" => get_environments(world, args),
        "vercel-get-deployment" => get_deployment(world, args),
        "vercel-list-deployment-files" => list_deployment_files(world, args),
        "vercel-create-deployment" => create_deployment(world, args),
        "vercel-create-project" => create_project(world, args),
        "vercel-list-all-teams" => list_all_teams(world, args),
        "vercel-create-team" => create_team(world, args),
        "vercel-create-environment-variables" => create_environment_variables(world, args),
        "vercel-create-custom-environment" => create_custom_environment(world, args),
        "vercel-list-projects" => list_projects(world, args),
        "vercel-find-project" => find_project(world, args),
        "vercel-get-project-domain" => get_project_domain(world, args),
        "vercel-delete-project" => delete_project(world, args),
        "vercel-remove-environment-variable" => remove_environment_variable(world, args),
        other => result(vec![error_block(format!("Error: Unknown tool: {other}"))]),
    }
}

fn data(r: ApiResult) -> Option<Value> {
    r.ok().map(|(_, v)| v)
}

const LIST_DEPLOYMENTS: &[Field] = &[
    opt("app", STR),
    opt("limit", Kind::Num),
    opt("projectId", STR),
    opt("state", STR),
    opt("target", STR),
    opt("teamId", STR),
];

fn list_all_deployments(world: &mut WorldState, args: &Value) -> Value {
    if let Err(e) = zod::parse(args, LIST_DEPLOYMENTS) {
        return result(vec![block(format!("Error: ZodError: {e}"))]);
    }
    let mut query = query_from(args, &["app", "projectId", "state", "target", "teamId"]);
    let limit = if truthy(args.get("limit")) {
        js_string(&args["limit"])
    } else {
        "50".into()
    };
    query.insert("limit".into(), limit);
    match data(api::list_deployments(world, &query)) {
        Some(d) => result(vec![block(pretty(&d["deployments"]))]),
        None => result(vec![block("Failed to retrieve deployments")]),
    }
}

fn get_environments(world: &mut WorldState, args: &Value) -> Value {
    if !truthy(Some(args)) {
        return result(vec![block("Invalid request: Missing required arguments")]);
    }
    let id_or_name = match args.get("idOrName") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        other => {
            let received = other.map_or("undefined".to_owned(), |v| v.to_string());
            return result(vec![block(format!(
                "Invalid request: idOrName parameter must be a non-empty string, received: {received}"
            ))]);
        }
    };
    match data(api::list_env(world, &id_or_name, &Query::new())) {
        Some(d) if d["envs"].is_array() => {
            let n = d["envs"].as_array().map_or(0, Vec::len);
            let plural = if n == 1 { "" } else { "s" };
            result(vec![
                block(format!(
                    "Retrieving {n} environment variable{plural} for project: {id_or_name}"
                )),
                block(pretty(&d["envs"])),
            ])
        }
        _ => result(vec![block(format!(
            "Failed to retrieve environment variables for project: {id_or_name}"
        ))]),
    }
}

fn get_deployment(world: &mut WorldState, args: &Value) -> Value {
    if let Err(e) = zod::parse(args, &[req("idOrUrl", STR), opt("teamId", STR)]) {
        return result(vec![block(format!("Error: ZodError: {e}"))]);
    }
    let query = query_from(args, &["teamId"]);
    match data(api::get_deployment(world, arg_str(args, "idOrUrl"), &query)) {
        Some(d) => result(vec![block(pretty(&d))]),
        None => result(vec![block("Failed to retrieve deployment")]),
    }
}

fn list_deployment_files(world: &mut WorldState, args: &Value) -> Value {
    let fields = [req("id", STR), opt("teamId", STR), opt("slug", STR)];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!(
            "Error listing deployment files: {e}"
        ))]);
    }
    let id = arg_str(args, "id");
    let query = query_from(args, &["teamId", "slug"]);
    // The API answers with a bare array; the server reads `data.files`.
    match data(api::deployment_files(world, id, &query)) {
        Some(d) if d.get("files").is_some_and(|f| !f.is_null()) => {
            let files = &d["files"];
            let n = files.as_array().map_or(0, Vec::len);
            result(vec![
                block(format!("Found {n} files in deployment {id}")),
                block(pretty(files)),
            ])
        }
        _ => result(vec![error_block(
            "Failed to retrieve deployment files or no files found",
        )]),
    }
}

/// `CreateDeploymentArgumentsSchema`'s transform: picks and coerces known keys.
fn deployment_body(args: &Map<String, Value>) -> Map<String, Value> {
    let clean: Map<String, Value> = args
        .iter()
        .map(|(k, v)| (k.replace('`', ""), v.clone()))
        .collect();
    let mut out = Map::new();
    let get = |k: &str| clean.get(k);
    for k in [
        "name",
        "project",
        "deploymentId",
        "slug",
        "teamId",
        "customEnvironmentSlugOrId",
    ] {
        if truthy(get(k)) {
            out.insert(k.into(), json!(js_string(&clean[k])));
        }
    }
    let target = if truthy(get("target")) {
        js_string(&clean["target"])
    } else {
        "production".into()
    };
    out.insert("target".into(), json!(target));
    for k in ["regions", "functions", "routes"] {
        if truthy(get(k)) {
            out.insert(k.into(), clean[k].clone());
        }
    }
    for k in ["cleanUrls", "trailingSlash", "public"] {
        if let Some(v) = get(k) {
            out.insert(k.into(), json!(truthy(Some(v))));
        }
    }
    if truthy(get("ignoreCommand")) {
        out.insert(
            "ignoreCommand".into(),
            json!(js_string(&clean["ignoreCommand"])),
        );
    }
    for k in ["gitSource", "gitMetadata", "projectSettings", "meta"] {
        if truthy(get(k)) {
            out.insert(k.into(), clean[k].clone());
        }
    }
    if truthy(get("monorepoManager")) {
        out.insert(
            "monorepoManager".into(),
            json!(js_string(&clean["monorepoManager"])),
        );
    }
    if truthy(get("files")) {
        out.insert("files".into(), clean["files"].clone());
    }
    if truthy(get("forceNew")) {
        out.insert("forceNew".into(), json!(1));
    }
    for k in ["withCache", "autoAssignCustomDomains", "withLatestCommit"] {
        if let Some(v) = get(k) {
            out.insert(k.into(), json!(truthy(Some(v))));
        }
    }
    out
}

fn create_deployment(world: &mut WorldState, args: &Value) -> Value {
    let Value::Object(obj) = args else {
        let issue = json!([{
            "code": "invalid_type",
            "expected": "object",
            "received": zod::type_name(Some(args)),
            "path": [],
            "message": format!("Expected object, received {}", zod::type_name(Some(args))),
        }]);
        return result(vec![error_block(format!(
            "Error creating deployment: {}",
            pretty(&issue)
        ))]);
    };
    let mut body = deployment_body(obj);
    let mut query = Query::new();
    if let Some(team) = body.remove("teamId") {
        query.insert("teamId".into(), js_string(&team));
    }
    match data(api::create_deployment(world, &query, &Value::Object(body))) {
        Some(d) => result(vec![
            block(format!(
                "Deployment created successfully: {}",
                d["url"].as_str().unwrap_or("undefined")
            )),
            block(pretty(&d)),
        ]),
        None => result(vec![error_block("Failed to create deployment")]),
    }
}

const CREATE_PROJECT: &[Field] = &[
    req("name", str_min("Project name is required")),
    opt("framework", STR),
    opt("buildCommand", STR),
    opt("devCommand", STR),
    opt("installCommand", STR),
    opt("outputDirectory", STR),
    opt("publicSource", Kind::Bool),
    opt("rootDirectory", STR),
    opt("serverlessFunctionRegion", STR),
    opt("skipGitConnectDuringLink", Kind::Bool),
    req("teamId", str_min("Team ID is required")),
];

fn create_project(world: &mut WorldState, args: &Value) -> Value {
    if let Err(e) = zod::parse(args, CREATE_PROJECT) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(args, &["teamId"]);
    let body: Map<String, Value> = CREATE_PROJECT
        .iter()
        .filter(|f| f.name != "teamId")
        .filter_map(|f| args.get(f.name).map(|v| (f.name.to_owned(), v.clone())))
        .collect();
    match data(api::create_project(world, &query, &Value::Object(body))) {
        Some(d) => result(vec![
            block(format!(
                "Project {} ({}) created successfully",
                js_string(&d["name"]),
                js_string(&d["id"])
            )),
            block(pretty(&d)),
        ]),
        None => result(vec![
            block("Project undefined (undefined) created successfully"),
            block("null"),
        ]),
    }
}

fn list_all_teams(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        opt("limit", Kind::Num),
        opt("since", Kind::Num),
        opt("until", Kind::Num),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(args, &["limit", "since", "until"]);
    match data(api::list_teams(world, &query)) {
        Some(d) => {
            let n = d["teams"].as_array().map_or(0, Vec::len);
            result(vec![
                block(format!("Found {n} teams")),
                block(pretty(&d["teams"])),
            ])
        }
        // `JSON.stringify(undefined)` leaves the block without text.
        None => result(vec![
            block("Found undefined teams"),
            json!({ "type": "text" }),
        ]),
    }
}

fn create_team(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        req(
            "slug",
            Kind::Str {
                min: Some((1, None)),
                max: Some(48),
            },
        ),
        opt("name", STR),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error creating team: {e}"))]);
    }
    let mut body = json!({ "slug": args["slug"] });
    if truthy(args.get("name")) {
        body["name"] = args["name"].clone();
    }
    match data(api::create_team(world, &body)) {
        Some(d) => result(vec![
            block(format!(
                "Team created successfully: {} ({})",
                js_string(&d["slug"]),
                js_string(&d["id"])
            )),
            block(pretty(&d)),
        ]),
        None => result(vec![error_block("Failed to create team")]),
    }
}

const ENV_ITEM: &[Field] = &[
    req("key", str_min("Key is required")),
    req("value", str_min("Value is required")),
    req(
        "target",
        Kind::EnumArray(&["production", "preview", "development"]),
    ),
    req(
        "type",
        Kind::Enum(&["system", "encrypted", "plain", "sensitive"]),
    ),
    opt("gitBranch", STR),
];

const CREATE_ENV: &[Field] = &[
    req("projectId", str_min("Project ID is required")),
    opt("teamId", STR),
    req("environmentVariables", Kind::ObjectArray(ENV_ITEM)),
];

fn create_environment_variables(world: &mut WorldState, args: &Value) -> Value {
    if let Err(e) = zod::parse(args, CREATE_ENV) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(args, &["teamId"]);
    // zod strips unknown keys from each variable.
    let items: Vec<Value> = args["environmentVariables"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let m: Map<String, Value> = ENV_ITEM
                .iter()
                .filter_map(|f| item.get(f.name).map(|v| (f.name.to_owned(), v.clone())))
                .collect();
            Value::Object(m)
        })
        .collect();
    let project = arg_str(args, "projectId");
    match data(api::create_env(
        world,
        project,
        &query,
        &Value::Array(items),
    )) {
        Some(d) => {
            let n = d["created"]
                .as_array()
                .map_or("undefined".to_owned(), |c| c.len().to_string());
            result(vec![
                block(format!("Successfully created {n} environment variables")),
                block(pretty(&d)),
            ])
        }
        None => result(vec![
            block("Successfully created undefined environment variables"),
            block("null"),
        ]),
    }
}

const BRANCH_MATCHER: &[Field] = &[
    req(
        "type",
        Kind::Enum(&["startsWith", "endsWith", "contains", "exactMatch", "regex"]),
    ),
    req("pattern", STR),
];

fn create_custom_environment(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        req("idOrName", STR),
        req("name", STR),
        opt("description", STR),
        opt("branchMatcher", Kind::Object(BRANCH_MATCHER)),
        opt("teamId", STR),
        opt("slug", STR),
    ];
    let mut issues: Vec<String> = zod::issues(args, &fields)
        .iter()
        .map(|i| i["message"].as_str().unwrap_or_default().to_owned())
        .collect();
    // The `.refine` on `name` runs only when `name` is a string; zod reports it in field order.
    if let Some(name) = args.get("name").and_then(Value::as_str) {
        let lower = name.to_lowercase();
        if lower == "production" || lower == "preview" {
            let before = zod::issues(args, &fields[..1]).len();
            issues.insert(
                before,
                "Custom environment name cannot be 'Production' or 'Preview'".into(),
            );
        }
    }
    if !issues.is_empty() {
        return result(vec![block(format!(
            "Invalid request: {}",
            issues.join(", ")
        ))]);
    }
    let id_or_name = arg_str(args, "idOrName");
    let name = arg_str(args, "name");
    let mut body = json!({ "name": name });
    for k in ["description", "branchMatcher"] {
        if truthy(args.get(k)) {
            body[k] = args[k].clone();
        }
    }
    let query = query_from(args, &["teamId", "slug"]);
    match data(api::create_custom_env(world, id_or_name, &query, &body)) {
        Some(d) if truthy(d.get("id")) => result(vec![
            block(format!(
                "Successfully created custom environment '{name}' for project: {id_or_name}"
            )),
            block(pretty(&d)),
        ]),
        _ => result(vec![block(format!(
            "Failed to create custom environment for project: {id_or_name}"
        ))]),
    }
}

fn list_projects(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        opt("limit", Kind::PosInt),
        opt("from", Kind::Int),
        opt("teamId", STR),
        opt("search", STR),
        opt("repoUrl", STR),
        opt("gitForkProtection", Kind::Enum(&["0", "1"])),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(
        args,
        &[
            "limit",
            "from",
            "teamId",
            "search",
            "repoUrl",
            "gitForkProtection",
        ],
    );
    match data(api::list_projects(world, &query)) {
        Some(d) if d["projects"].is_array() => {
            let n = d["projects"].as_array().map_or(0, Vec::len);
            result(vec![
                block(format!("Found {n} projects")),
                block(pretty(&d["projects"])),
            ])
        }
        _ => result(vec![error_block(
            "No projects found or invalid response from API",
        )]),
    }
}

const PROJECT_ID: Kind = Kind::Str {
    min: Some((1, Some("Project ID or name is required"))),
    max: None,
};

fn find_project(world: &mut WorldState, args: &Value) -> Value {
    if let Err(e) = zod::parse(args, &[req("idOrName", PROJECT_ID), opt("teamId", STR)]) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(args, &["teamId"]);
    match data(api::get_project(world, arg_str(args, "idOrName"), &query)) {
        Some(d) => result(vec![
            block(format!(
                "Found project: {} ({})",
                js_string(&d["name"]),
                js_string(&d["id"])
            )),
            block(pretty(&d)),
        ]),
        None => result(vec![error_block("Project not found")]),
    }
}

fn get_project_domain(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        req("idOrName", PROJECT_ID),
        req("domain", str_min("Domain name is required")),
        opt("teamId", STR),
        opt("slug", STR),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let query = query_from(args, &["teamId", "slug"]);
    let r = api::get_project_domain(
        world,
        arg_str(args, "idOrName"),
        arg_str(args, "domain"),
        &query,
    );
    match data(r) {
        Some(d) => result(vec![
            block(format!(
                "Found domain: {} (verified: {})",
                js_string(&d["name"]),
                d.get("verified").map_or("undefined".to_owned(), js_string)
            )),
            block(pretty(&d)),
        ]),
        None => result(vec![error_block("Domain not found")]),
    }
}

fn delete_project(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        req("idOrName", PROJECT_ID),
        opt("teamId", STR),
        opt("slug", STR),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let id_or_name = arg_str(args, "idOrName");
    let query = query_from(args, &["teamId", "slug"]);
    // The server reports success whatever the API answered.
    let _ = api::delete_project(world, id_or_name, &query);
    result(vec![block(format!(
        "Project {id_or_name} deleted successfully"
    ))])
}

fn remove_environment_variable(world: &mut WorldState, args: &Value) -> Value {
    let fields = [
        req("idOrName", PROJECT_ID),
        req("id", str_min("Environment variable ID is required")),
        opt("customEnvironmentId", STR),
        opt("teamId", STR),
        opt("slug", STR),
    ];
    if let Err(e) = zod::parse(args, &fields) {
        return result(vec![error_block(format!("Error: {e}"))]);
    }
    let id_or_name = arg_str(args, "idOrName");
    let id = arg_str(args, "id");
    let query = query_from(args, &["customEnvironmentId", "teamId", "slug"]);
    let d = data(api::delete_env(world, id_or_name, id, &query)).unwrap_or(Value::Null);
    result(vec![
        block(format!(
            "Environment variable {id} removed successfully from project {id_or_name}"
        )),
        block(pretty(&d)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_matches_javascript() {
        assert_eq!(js_string(&json!(5)), "5");
        assert_eq!(js_string(&json!(5.0)), "5");
        assert_eq!(js_string(&json!(2.5)), "2.5");
        assert_eq!(js_string(&json!(["a", "b"])), "a,b");
        assert_eq!(js_string(&json!({"a": 1})), "[object Object]");
    }

    #[test]
    fn deployment_body_defaults_target_and_coerces() {
        let args = json!({"`name`": "web", "forceNew": true, "public": null, "teamId": 7});
        let body = deployment_body(args.as_object().unwrap());
        assert_eq!(
            Value::Object(body),
            json!({"name": "web", "teamId": "7", "target": "production", "public": false, "forceNew": 1})
        );
    }
}
