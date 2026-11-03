//! Vercel REST operations over the store. The REST routes and the replica tools both call
//! these, so tool calls and direct API calls see one coherent state.

use axum::http::StatusCode;
use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::model::*;

fn limit(query: &Query, default: usize, max: usize) -> Result<usize, ApiError> {
    match q(query, "limit") {
        None => Ok(default),
        Some(l) => l
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 1.0)
            .map(|n| (n as usize).min(max))
            .ok_or_else(|| bad_request("Invalid request: `limit` should be >= 1.")),
    }
}

fn ms_param(query: &Query, key: &str) -> Result<Option<i64>, ApiError> {
    q(query, key)
        .map(|v| {
            v.parse::<f64>()
                .map(|n| n as i64)
                .map_err(|_| bad_request(format!("Invalid request: `{key}` should be number.")))
        })
        .transpose()
}

fn pagination(count: usize, next: Option<i64>, prev: Option<i64>) -> Value {
    json!({ "count": count, "next": next, "prev": prev })
}

// ───────────────────────────── deployments ─────────────────────────────

/// `GET /v6/deployments`
pub fn list_deployments(world: &mut WorldState, query: &Query) -> crate::model::ApiResult {
    let s = svc(world);
    let scope = scope(s, query)?;
    let limit = limit(query, 20, 100)?;
    let until = ms_param(query, "until")?;
    let since = ms_param(query, "since")?;
    let project = match q(query, "projectId") {
        Some(p) => match find_project(s, p, &scope) {
            Some(key) => Some(key),
            None => {
                return Ok((
                    StatusCode::OK,
                    json!({ "deployments": [], "pagination": pagination(0, None, None) }),
                ));
            }
        },
        None => None,
    };
    let states: Option<Vec<String>> =
        q(query, "state").map(|v| v.split(',').map(|x| x.trim().to_uppercase()).collect());
    let projects = projects_in_scope(s, &scope);
    let mut all: Vec<Value> = docs(s, DEPLOYMENTS)
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| {
            let pid = str_of(d, "projectId").unwrap_or_default();
            projects.iter().any(|p| str_of(p, "id") == Some(pid))
                && project.as_deref().is_none_or(|p| p == pid)
                && q(query, "app").is_none_or(|a| str_of(d, "name") == Some(a))
                && states.as_ref().is_none_or(|st| {
                    st.iter()
                        .any(|x| Some(x.as_str()) == str_of(d, "readyState"))
                })
                && q(query, "target").is_none_or(|t| match t {
                    "production" => str_of(d, "target") == Some("production"),
                    "preview" => d["target"].is_null() || str_of(d, "target") == Some("preview"),
                    other => str_of(d, "target") == Some(other),
                })
                && until.is_none_or(|u| int_of(d, "createdAt") < u)
                && since.is_none_or(|v| int_of(d, "createdAt") > v)
        })
        .collect();
    all.sort_by_key(|d| std::cmp::Reverse(int_of(d, "createdAt")));
    let more = all.len() > limit;
    all.truncate(limit);
    let items: Vec<Value> = all
        .iter()
        .map(|d| {
            let pid = str_of(d, "projectId").unwrap_or_default();
            let current = current_production(s, pid);
            deployment_v6(d, current.as_ref().and_then(|c| str_of(c, "id")))
        })
        .collect();
    let next = if more {
        all.last().map(|d| int_of(d, "createdAt"))
    } else {
        None
    };
    let prev = all.first().map(|d| int_of(d, "createdAt"));
    Ok((
        StatusCode::OK,
        json!({ "pagination": pagination(items.len(), next, prev), "deployments": items }),
    ))
}

fn deployment_in_scope(
    world: &mut WorldState,
    id_or_url: &str,
    query: &Query,
) -> Result<Value, ApiError> {
    let s = svc(world);
    let scope = scope(s, query)?;
    let d = find_deployment(s, id_or_url).ok_or_else(deployment_not_found)?;
    let pid = str_of(&d, "projectId").unwrap_or_default();
    if let Some(team) = scope
        && doc(s, PROJECTS, pid).is_none_or(|p| str_of(&p, "accountId") != Some(team.as_str()))
    {
        return Err(deployment_not_found());
    }
    Ok(d)
}

/// `GET /v13/deployments/:idOrUrl`
pub fn get_deployment(world: &mut WorldState, id_or_url: &str, query: &Query) -> ApiResult {
    let d = deployment_in_scope(world, id_or_url, query)?;
    Ok((StatusCode::OK, deployment_v13(svc(world), &d)))
}

/// `GET /v6/deployments/:id/files` (a bare array of file-tree entries, as on Vercel).
pub fn deployment_files(world: &mut WorldState, id: &str, query: &Query) -> ApiResult {
    let d = deployment_in_scope(world, id, query)?;
    let id = str_of(&d, "id").unwrap_or_default();
    let files = doc(svc(world), FILES, id)
        .and_then(|f| f.get("files").cloned())
        .unwrap_or(json!([]));
    Ok((StatusCode::OK, files))
}

fn branch_slug(branch: &str) -> String {
    branch
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Meta for a git deployment of `sha`/`ref` in `project`, copied from an earlier deployment
/// of the same commit or branch head when one exists.
fn git_meta(
    s: &mut mock_core::world::ServiceState,
    project: &Value,
    git: &Value,
) -> Result<(Value, Value), ApiError> {
    let pid = str_of(project, "id").unwrap_or_default();
    let sha = git.get("sha").and_then(Value::as_str);
    let git_ref = git.get("ref").and_then(Value::as_str);
    let earlier = project_deployments(s, pid);
    let matching = match (sha, git_ref) {
        (Some(sha), _) => earlier
            .iter()
            .find(|d| {
                d["meta"]["githubCommitSha"]
                    .as_str()
                    .is_some_and(|x| x.starts_with(sha))
            })
            .cloned(),
        (None, Some(r)) => earlier
            .iter()
            .find(|d| d["meta"]["githubCommitRef"].as_str() == Some(r))
            .cloned(),
        (None, None) => None,
    };
    let link = &project["link"];
    if link.is_null() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "incorrect_git_source_info",
            "The project is not connected to a Git repository.",
        ));
    }
    match (matching, sha) {
        (Some(d), _) => Ok((
            d["meta"].clone(),
            d.get("gitSource").cloned().unwrap_or(Value::Null),
        )),
        (None, Some(sha)) => {
            let r = git_ref.unwrap_or(link["productionBranch"].as_str().unwrap_or("main"));
            let meta = json!({
                "githubCommitSha": sha,
                "githubCommitRef": r,
                "githubOrg": link["org"],
                "githubRepo": link["repo"],
                "githubRepoId": link["repoId"].to_string(),
            });
            let source =
                json!({ "type": "github", "repoId": link["repoId"], "ref": r, "sha": sha });
            Ok((meta, source))
        }
        (None, None) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "incorrect_git_source_info",
            "The provided GitHub repository does not contain the requested branch or commit reference. Please ensure the repository is not empty.",
        )),
    }
}

/// `POST /v13/deployments` — new deployments and redeploys (`deploymentId`).
pub fn create_deployment(world: &mut WorldState, query: &Query, body: &Value) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let scope = scope(s, query)?;
    let Some(body) = body.as_object() else {
        return Err(bad_request("Invalid request: should be object."));
    };
    let target_in = body.get("target").and_then(Value::as_str);
    let target = match target_in {
        None | Some("production") => Some("production"),
        Some("preview") => None,
        Some(other) => {
            return Err(bad_request(format!(
                "Invalid request: `target` should be equal to one of the allowed values \"staging, production\" or a custom environment, received \"{other}\"."
            )));
        }
    };

    let source_deployment = match body.get("deploymentId").and_then(Value::as_str) {
        Some(id) => {
            let d = find_deployment(s, id).ok_or_else(deployment_not_found)?;
            Some(d)
        }
        None => None,
    };
    let project_ref = body
        .get("project")
        .and_then(Value::as_str)
        .or_else(|| body.get("name").and_then(Value::as_str));
    let project_key = match (&source_deployment, project_ref) {
        (Some(d), _) => str_of(d, "projectId").map(str::to_owned),
        (None, Some(p)) => find_project(s, p, &scope),
        (None, None) => {
            return Err(bad_request(
                "Invalid request: missing required property `name`.",
            ));
        }
    }
    .ok_or_else(project_not_found)?;
    let project = doc(s, PROJECTS, &project_key).ok_or_else(project_not_found)?;
    if let Some(team) = &scope
        && str_of(&project, "accountId") != Some(team.as_str())
    {
        return Err(project_not_found());
    }

    let custom_env = match body
        .get("customEnvironmentSlugOrId")
        .and_then(Value::as_str)
    {
        Some(c) => {
            let found = docs(s, CUSTOM_ENVS).into_iter().find(|(k, e)| {
                k.starts_with(&format!("{project_key}/"))
                    && (str_of(e, "id") == Some(c) || str_of(e, "slug") == Some(c))
            });
            Some(found.map(|(_, e)| e).ok_or_else(|| {
                api_error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("Custom environment \"{c}\" not found."),
                )
            })?)
        }
        None => None,
    };
    let target = if custom_env.is_some() { None } else { target };

    let (mut meta, git_source, failed) = if let Some(d) = &source_deployment {
        let failed = (str_of(d, "readyState") == Some("ERROR")).then(|| {
            (
                d["errorCode"].clone(),
                d["errorMessage"].clone(),
                d["errorStep"].clone(),
            )
        });
        (
            d.get("meta").cloned().unwrap_or(json!({})),
            d.get("gitSource").cloned().unwrap_or(Value::Null),
            failed,
        )
    } else if let Some(git) = body.get("gitSource").filter(|g| g.is_object()) {
        let (meta, source) = git_meta(s, &project, git)?;
        (meta, source, None)
    } else if body
        .get("files")
        .and_then(Value::as_array)
        .is_some_and(|f| !f.is_empty())
    {
        (json!({}), Value::Null, None)
    } else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "missing_files",
            "Missing files",
        ));
    };
    if let Some(gm) = body.get("gitMetadata").and_then(Value::as_object) {
        let m = meta.as_object_mut().expect("meta object");
        for (from, to) in [
            ("commitSha", "githubCommitSha"),
            ("commitMessage", "githubCommitMessage"),
            ("commitRef", "githubCommitRef"),
            ("commitAuthorName", "githubCommitAuthorName"),
        ] {
            if let Some(v) = gm.get(from) {
                m.insert(to.into(), v.clone());
            }
        }
    }
    if let Some(extra) = body.get("meta").and_then(Value::as_object) {
        let m = meta.as_object_mut().expect("meta object");
        for (k, v) in extra {
            m.insert(k.clone(), v.clone());
        }
    }
    if let Some(d) = &source_deployment {
        meta["action"] = json!("redeploy");
        meta["originalDeploymentId"] = d["id"].clone();
    }

    let project_name = str_of(&project, "name").unwrap_or_default().to_owned();
    let team_id = str_of(&project, "accountId").unwrap_or_default().to_owned();
    let team = team_slug(s, &team_id);
    let id = s.ids.next_prefixed("deployment", "dpl_", 28);
    let hash: String = id[4..13].to_lowercase();
    let url = format!("{project_name}-{hash}-{team}.vercel.app");
    let regions = body
        .get("regions")
        .cloned()
        .or_else(|| {
            source_deployment
                .as_ref()
                .and_then(|d| d.get("regions").cloned())
        })
        .unwrap_or(json!(["iad1"]));
    let ready_state = if failed.is_some() { "ERROR" } else { "READY" };
    let branch = meta["githubCommitRef"].as_str().map(str::to_owned);

    let mut alias: Vec<Value> = Vec::new();
    if failed.is_none() {
        if target == Some("production") {
            let prod_domains: Vec<String> = docs(s, DOMAINS)
                .into_iter()
                .map(|(_, d)| d)
                .filter(|d| {
                    str_of(d, "projectId") == Some(project_key.as_str())
                        && d["redirect"].is_null()
                        && d["gitBranch"].is_null()
                        && d["customEnvironmentId"].is_null()
                })
                .filter_map(|d| str_of(&d, "name").map(str::to_owned))
                .collect();
            // Production domains move from the previous production deployment to this one.
            let holders: Vec<String> = docs(s, DEPLOYMENTS)
                .into_iter()
                .filter(|(_, d)| str_of(d, "projectId") == Some(project_key.as_str()))
                .map(|(k, _)| k)
                .collect();
            let deployments = s.store.collection_mut(DEPLOYMENTS);
            for k in holders {
                if let Some(d) = deployments.get_mut(&k)
                    && let Some(a) = d["alias"].as_array_mut()
                {
                    a.retain(|x| {
                        !x.as_str()
                            .is_some_and(|n| prod_domains.iter().any(|p| p == n))
                    });
                }
            }
            alias.extend(prod_domains.into_iter().map(Value::String));
        } else if let Some(b) = &branch {
            alias.push(json!(format!(
                "{project_name}-git-{}-{team}.vercel.app",
                branch_slug(b)
            )));
        }
    }

    let mut d = Map::new();
    d.insert("id".into(), json!(id));
    d.insert("name".into(), json!(project_name));
    d.insert("url".into(), json!(url));
    d.insert("projectId".into(), json!(project_key));
    d.insert("target".into(), json!(target));
    if let Some(c) = &custom_env {
        d.insert(
            "customEnvironment".into(),
            json!({ "id": c["id"], "slug": c["slug"] }),
        );
    }
    d.insert("readyState".into(), json!(ready_state));
    d.insert("type".into(), json!("LAMBDAS"));
    d.insert(
        "source".into(),
        json!(if source_deployment.is_some() {
            "redeploy"
        } else if git_source.is_null() {
            "cli"
        } else {
            "git"
        }),
    );
    d.insert("createdAt".into(), json!(now));
    d.insert("buildingAt".into(), json!(now));
    d.insert("ready".into(), json!(now));
    d.insert("creator".into(), creator(s));
    d.insert("meta".into(), meta);
    d.insert("gitSource".into(), git_source);
    d.insert("alias".into(), Value::Array(alias));
    d.insert("regions".into(), regions);
    d.insert(
        "inspectorUrl".into(),
        json!(format!(
            "https://vercel.com/{team}/{project_name}/{}",
            &id[4..]
        )),
    );
    if let Some((code, message, step)) = failed {
        d.insert("errorCode".into(), code);
        d.insert("errorMessage".into(), message);
        d.insert("errorStep".into(), step);
    }
    let stored = Value::Object(d);
    s.store
        .collection_mut(DEPLOYMENTS)
        .insert(id.clone(), stored.clone());
    // Persist the project's deployment pointers so the final state shows what production
    // serves (reads recompute them the same way).
    let latest: Vec<Value> = project_deployments(s, &project_key)
        .iter()
        .take(3)
        .map(deployment_summary)
        .collect();
    let production = current_production(s, &project_key).map(|d| deployment_summary(&d));
    if let Some(p) = s.store.collection_mut(PROJECTS).get_mut(&project_key) {
        p["updatedAt"] = json!(now);
        p["latestDeployments"] = Value::Array(latest);
        p["targets"] = match production {
            Some(prod) => json!({ "production": prod }),
            None => json!({}),
        };
    }

    // The create response reports the build as just queued; the stored deployment is
    // already finished (builds complete instantly in this world).
    let mut response = deployment_v13(s, &stored);
    for k in ["readyState", "status"] {
        response[k] = json!("QUEUED");
    }
    response["alias"] = json!([]);
    response["aliasAssigned"] = Value::Null;
    if let Some(o) = response.as_object_mut() {
        o.shift_remove("ready");
        o.shift_remove("errorCode");
        o.shift_remove("errorMessage");
        o.shift_remove("errorStep");
    }
    Ok((StatusCode::OK, response))
}

// ───────────────────────────── projects ─────────────────────────────

/// `GET /v9/projects`
pub fn list_projects(world: &mut WorldState, query: &Query) -> ApiResult {
    let s = svc(world);
    let scope = scope(s, query)?;
    let limit = limit(query, 20, 100)?;
    let from = ms_param(query, "from")?;
    let search = q(query, "search").map(str::to_lowercase);
    let repo_url = q(query, "repoUrl").map(|u| {
        u.trim_end_matches(".git")
            .trim_end_matches('/')
            .to_lowercase()
    });
    let mut projects: Vec<Value> = projects_in_scope(s, &scope)
        .into_iter()
        .filter(|p| {
            search.as_ref().is_none_or(|n| {
                str_of(p, "name").is_some_and(|name| name.to_lowercase().contains(n))
            }) && repo_url.as_ref().is_none_or(|u| {
                let link = &p["link"];
                let full = format!(
                    "https://github.com/{}/{}",
                    link["org"].as_str().unwrap_or_default(),
                    link["repo"].as_str().unwrap_or_default()
                )
                .to_lowercase();
                !link.is_null() && &full == u
            }) && from.is_none_or(|f| int_of(p, "updatedAt") < f)
        })
        .collect();
    projects.sort_by_key(|p| std::cmp::Reverse(int_of(p, "updatedAt")));
    let more = projects.len() > limit;
    projects.truncate(limit);
    let next = if more {
        projects.last().map(|p| int_of(p, "updatedAt"))
    } else {
        None
    };
    let prev = projects.first().map(|p| int_of(p, "updatedAt"));
    let views: Vec<Value> = projects.iter().map(|p| project_view(s, p, false)).collect();
    Ok((
        StatusCode::OK,
        json!({ "projects": views, "pagination": pagination(projects.len(), next, prev) }),
    ))
}

/// `GET /v9/projects/:idOrName`
pub fn get_project(world: &mut WorldState, id_or_name: &str, query: &Query) -> ApiResult {
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let p = doc(s, PROJECTS, &key).ok_or_else(project_not_found)?;
    Ok((StatusCode::OK, project_view(s, &p, true)))
}

const FRAMEWORKS: &[&str] = &[
    "blitzjs",
    "nextjs",
    "gatsby",
    "remix",
    "react-router",
    "astro",
    "hexo",
    "eleventy",
    "docusaurus-2",
    "docusaurus",
    "preact",
    "solidstart-1",
    "solidstart",
    "dojo",
    "ember",
    "vue",
    "scully",
    "ionic-angular",
    "angular",
    "polymer",
    "svelte",
    "sveltekit",
    "sveltekit-1",
    "ionic-react",
    "create-react-app",
    "gridsome",
    "umijs",
    "sapper",
    "saber",
    "stencil",
    "nuxtjs",
    "redwoodjs",
    "hugo",
    "jekyll",
    "brunch",
    "middleman",
    "zola",
    "hydrogen",
    "vite",
    "vitepress",
    "vuepress",
    "parcel",
    "fasthtml",
    "sanity-v3",
    "sanity",
    "storybook",
    "nitro",
    "hono",
    "express",
    "h3",
    "nestjs",
    "fastify",
    "xmcp",
];

fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !name.contains("---")
}

/// `POST /v11/projects`
pub fn create_project(world: &mut WorldState, query: &Query, body: &Value) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let team = match scope(s, query)? {
        Some(t) => t,
        None => viewer(s)["defaultTeamId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| docs(s, TEAMS).first().map(|(k, _)| k.clone()))
            .unwrap_or_default(),
    };
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return Err(bad_request(
            "Invalid request: missing required property `name`.",
        ));
    };
    if !valid_project_name(name) {
        return Err(bad_request(
            "The name of a Project can only contain up to 100 alphanumeric lowercase characters and hyphens",
        ));
    }
    let framework = body.get("framework").cloned().unwrap_or(Value::Null);
    if let Some(f) = framework.as_str()
        && !FRAMEWORKS.contains(&f)
    {
        return Err(bad_request(format!(
            "Invalid request: `framework` should be equal to one of the allowed values \"{}\".",
            FRAMEWORKS.join(", ")
        )));
    }
    if find_project(s, name, &Some(team.clone())).is_some() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "conflict",
            format!("A project with the name \"{name}\" already exists."),
        ));
    }
    let id = s.ids.next_prefixed("project", "prj_", 28);
    let field = |k: &str| body.get(k).cloned().unwrap_or(Value::Null);
    let project = json!({
        "accountId": team,
        "autoExposeSystemEnvs": true,
        "buildCommand": field("buildCommand"),
        "createdAt": now,
        "devCommand": field("devCommand"),
        "directoryListing": false,
        "env": [],
        "framework": framework,
        "gitForkProtection": true,
        "id": id,
        "installCommand": field("installCommand"),
        "name": name,
        "nodeVersion": "22.x",
        "outputDirectory": field("outputDirectory"),
        "publicSource": field("publicSource"),
        "rootDirectory": field("rootDirectory"),
        "serverlessFunctionRegion": body.get("serverlessFunctionRegion").cloned().unwrap_or(json!("iad1")),
        "sourceFilesOutsideRootDirectory": true,
        "updatedAt": now,
        "live": false,
        "passwordProtection": null,
    });
    s.store
        .collection_mut(PROJECTS)
        .insert(id.clone(), project.clone());
    rebuild_env_index(s);
    Ok((StatusCode::OK, project_view(s, &project, true)))
}

/// `DELETE /v9/projects/:idOrName` (204, and the project's deployments and domains go too).
pub fn delete_project(world: &mut WorldState, id_or_name: &str, query: &Query) -> ApiResult {
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    s.store.collection_mut(PROJECTS).remove(&key);
    for (coll, field) in [(DEPLOYMENTS, "projectId"), (DOMAINS, "projectId")] {
        let doomed: Vec<String> = docs(s, coll)
            .into_iter()
            .filter(|(_, d)| str_of(d, field) == Some(key.as_str()))
            .map(|(k, _)| k)
            .collect();
        for k in doomed {
            s.store.collection_mut(coll).remove(&k);
            s.store.collection_mut(FILES).remove(&k);
        }
    }
    let envs: Vec<String> = docs(s, CUSTOM_ENVS)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(&format!("{key}/")))
        .collect();
    for k in envs {
        s.store.collection_mut(CUSTOM_ENVS).remove(&k);
    }
    rebuild_env_index(s);
    Ok((StatusCode::NO_CONTENT, Value::Null))
}

// ───────────────────────────── environment variables ─────────────────────────────

/// `GET /v10/projects/:idOrName/env`
pub fn list_env(world: &mut WorldState, id_or_name: &str, query: &Query) -> ApiResult {
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let p = doc(s, PROJECTS, &key).ok_or_else(project_not_found)?;
    let decrypt = q(query, "decrypt") == Some("true");
    let envs: Vec<Value> = p["env"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| q(query, "gitBranch").is_none_or(|b| str_of(e, "gitBranch") == Some(b)))
        .map(|e| env_view(e, decrypt))
        .collect();
    Ok((StatusCode::OK, json!({ "envs": envs })))
}

const ENV_TYPES: &[&str] = &["system", "secret", "encrypted", "plain", "sensitive"];
const ENV_TARGETS: &[&str] = &["production", "preview", "development"];

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Schema checks Vercel applies to the whole request (any failure is a 400).
fn validate_env_input(e: &Value) -> Result<(), ApiError> {
    let Some(obj) = e.as_object() else {
        return Err(bad_request("Invalid request: should be object."));
    };
    let key = obj.get("key").and_then(Value::as_str);
    let Some(key) = key else {
        return Err(bad_request(
            "Invalid request: missing required property `key`.",
        ));
    };
    if !valid_env_key(key) {
        return Err(bad_request(
            "The name of your Environment Variable contains invalid characters. Only letters, digits, and underscores are allowed. Furthermore, the name should not start with a digit.",
        ));
    }
    if obj.get("value").and_then(Value::as_str).is_none() {
        return Err(bad_request(
            "Invalid request: missing required property `value`.",
        ));
    }
    match obj.get("type").and_then(Value::as_str) {
        Some(t) if ENV_TYPES.contains(&t) => {}
        Some(_) => {
            return Err(bad_request(format!(
                "Invalid request: `type` should be equal to one of the allowed values \"{}\".",
                ENV_TYPES.join(", ")
            )));
        }
        None => {
            return Err(bad_request(
                "Invalid request: missing required property `type`.",
            ));
        }
    }
    let targets = obj.get("target").and_then(Value::as_array);
    let Some(targets) = targets.filter(|t| !t.is_empty()) else {
        return Err(bad_request(
            "Invalid request: missing required property `target`.",
        ));
    };
    if targets
        .iter()
        .any(|t| !t.as_str().is_some_and(|t| ENV_TARGETS.contains(&t)))
    {
        return Err(bad_request(format!(
            "Invalid request: `target` should be equal to one of the allowed values \"{}\".",
            ENV_TARGETS.join(", ")
        )));
    }
    if obj.get("gitBranch").and_then(Value::as_str).is_some()
        && !(targets.len() == 1 && targets[0] == "preview")
    {
        return Err(bad_request(
            "The `gitBranch` property can only be used with the `preview` target.",
        ));
    }
    if obj.get("type").and_then(Value::as_str) == Some("sensitive")
        && targets.iter().any(|t| t == "development")
    {
        return Err(bad_request(
            "Sensitive Environment Variables cannot be created in the Development environment.",
        ));
    }
    Ok(())
}

fn overlapping_target(a: &Value, b: &Value) -> Option<String> {
    let bt = b["target"].as_array()?;
    a["target"]
        .as_array()?
        .iter()
        .find(|t| bt.contains(t))
        .and_then(|t| t.as_str().map(str::to_owned))
}

/// `POST /v10/projects/:idOrName/env` — one object or an array. Conflicts with existing
/// variables (same key, overlapping target, same branch) are reported in `failed` unless
/// `upsert=true`.
pub fn create_env(
    world: &mut WorldState,
    id_or_name: &str,
    query: &Query,
    body: &Value,
) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let (items, single) = match body {
        Value::Array(items) => (items.clone(), false),
        other => (vec![other.clone()], true),
    };
    for item in &items {
        validate_env_input(item)?;
    }
    let upsert = q(query, "upsert") == Some("true");
    let user = viewer(s)["uid"].clone();
    let mut created = Vec::new();
    let mut failed = Vec::new();
    for item in items {
        let mut env = doc(s, PROJECTS, &key).ok_or_else(project_not_found)?["env"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let conflict = env.iter().position(|e| {
            str_of(e, "key") == str_of(&item, "key")
                && str_of(e, "gitBranch") == str_of(&item, "gitBranch")
                && overlapping_target(e, &item).is_some()
        });
        let mut record = Map::new();
        record.insert("type".into(), item["type"].clone());
        record.insert("value".into(), item["value"].clone());
        record.insert("target".into(), item["target"].clone());
        record.insert("configurationId".into(), Value::Null);
        record.insert(
            "comment".into(),
            item.get("comment").cloned().unwrap_or(json!("")),
        );
        match conflict {
            Some(i) if upsert => {
                let existing = env[i].as_object_mut().expect("env object");
                for (k, v) in record {
                    existing.insert(k, v);
                }
                if let Some(b) = item.get("gitBranch") {
                    existing.insert("gitBranch".into(), b.clone());
                }
                existing.insert("updatedAt".into(), json!(now));
                existing.insert("updatedBy".into(), user.clone());
                created.push(env_view(&env[i], false));
            }
            Some(i) => {
                let target = overlapping_target(&env[i], &item).unwrap_or_default();
                let k = str_of(&item, "key").unwrap_or_default();
                let branch = str_of(&item, "gitBranch").unwrap_or("undefined");
                failed.push(json!({ "error": {
                    "code": "ENV_CONFLICT",
                    "message": format!("A variable with the name `{k}` already exists for the target {target} on branch {branch}"),
                    "key": k,
                    "envVarId": env[i]["id"],
                    "envVarKey": k,
                    "target": item["target"],
                    "gitBranch": item.get("gitBranch").cloned().unwrap_or(Value::Null),
                }}));
                continue;
            }
            None => {
                let id = s.ids.next_prefixed("env", "", 16);
                record.insert("id".into(), json!(id));
                record.insert("key".into(), item["key"].clone());
                if let Some(b) = item.get("gitBranch").filter(|b| b.is_string()) {
                    record.insert("gitBranch".into(), b.clone());
                }
                record.insert("createdAt".into(), json!(now));
                record.insert("updatedAt".into(), json!(now));
                record.insert("createdBy".into(), user.clone());
                record.insert("updatedBy".into(), Value::Null);
                let record = Value::Object(record);
                created.push(env_view(&record, false));
                env.push(record);
            }
        }
        let p = s
            .store
            .collection_mut(PROJECTS)
            .get_mut(&key)
            .expect("project");
        p["env"] = Value::Array(env);
        p["updatedAt"] = json!(now);
    }
    rebuild_env_index(s);
    if single {
        if let Some(f) = failed.into_iter().next() {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                body: f,
            });
        }
        let c = created.into_iter().next().unwrap_or(Value::Null);
        return Ok((StatusCode::CREATED, json!({ "created": c, "failed": [] })));
    }
    Ok((
        StatusCode::CREATED,
        json!({ "created": created, "failed": failed }),
    ))
}

/// `DELETE /v9/projects/:idOrName/env/:id` — returns the removed variable.
pub fn delete_env(
    world: &mut WorldState,
    id_or_name: &str,
    env_id: &str,
    query: &Query,
) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let p = s
        .store
        .collection_mut(PROJECTS)
        .get_mut(&key)
        .expect("project");
    let Some(env) = p["env"].as_array_mut() else {
        return Err(env_not_found());
    };
    let Some(i) = env.iter().position(|e| str_of(e, "id") == Some(env_id)) else {
        return Err(env_not_found());
    };
    let removed = env.remove(i);
    p["updatedAt"] = json!(now);
    rebuild_env_index(s);
    Ok((StatusCode::OK, env_view(&removed, false)))
}

fn env_not_found() -> ApiError {
    api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "Environment Variable not found",
    )
}

// ───────────────────────────── domains ─────────────────────────────

fn project_domains(
    world: &mut WorldState,
    id_or_name: &str,
    query: &Query,
) -> Result<Vec<Value>, ApiError> {
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    Ok(docs(s, DOMAINS)
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| str_of(d, "projectId") == Some(key.as_str()))
        .collect())
}

/// `GET /v9/projects/:idOrName/domains`
pub fn list_project_domains(world: &mut WorldState, id_or_name: &str, query: &Query) -> ApiResult {
    let domains = project_domains(world, id_or_name, query)?;
    let n = domains.len();
    Ok((
        StatusCode::OK,
        json!({ "domains": domains, "pagination": pagination(n, None, None) }),
    ))
}

/// `GET /v9/projects/:idOrName/domains/:domain`
pub fn get_project_domain(
    world: &mut WorldState,
    id_or_name: &str,
    domain: &str,
    query: &Query,
) -> ApiResult {
    let name = domain.to_lowercase();
    project_domains(world, id_or_name, query)?
        .into_iter()
        .find(|d| str_of(d, "name") == Some(name.as_str()))
        .map(|d| (StatusCode::OK, d))
        .ok_or_else(|| {
            let mut e = api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("The domain \"{name}\" is not assigned to this project."),
            );
            e.body["error"]["domain"] = json!(name);
            e
        })
}

// ───────────────────────────── custom environments ─────────────────────────────

/// `GET /v9/projects/:idOrName/custom-environments`
pub fn list_custom_envs(world: &mut WorldState, id_or_name: &str, query: &Query) -> ApiResult {
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let envs: Vec<Value> = docs(s, CUSTOM_ENVS)
        .into_iter()
        .filter(|(k, _)| k.starts_with(&format!("{key}/")))
        .map(|(_, e)| e)
        .collect();
    Ok((
        StatusCode::OK,
        json!({ "accountLimit": { "total": 12 }, "environments": envs }),
    ))
}

/// `POST /v9/projects/:idOrName/custom-environments` (`slug`; `name` is accepted as an alias).
pub fn create_custom_env(
    world: &mut WorldState,
    id_or_name: &str,
    query: &Query,
    body: &Value,
) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let key = project_key(s, id_or_name, query)?;
    let Some(raw) = body
        .get("slug")
        .or_else(|| body.get("name"))
        .and_then(Value::as_str)
    else {
        return Err(bad_request(
            "Invalid request: missing required property `slug`.",
        ));
    };
    let slug: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() || slug.len() > 32 {
        return Err(bad_request(
            "Invalid request: `slug` should NOT be longer than 32 characters.",
        ));
    }
    if ENV_TARGETS.contains(&slug.as_str()) {
        return Err(bad_request(format!(
            "The custom environment name \"{slug}\" is reserved."
        )));
    }
    let doc_key = format!("{key}/{slug}");
    if doc(s, CUSTOM_ENVS, &doc_key).is_some() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "conflict",
            format!("A custom environment with the slug \"{slug}\" already exists."),
        ));
    }
    let branch_matcher = body.get("branchMatcher").cloned().unwrap_or(Value::Null);
    let id = s.ids.next_prefixed("custom_env", "env_", 28);
    let env = json!({
        "id": id,
        "slug": slug,
        "type": "preview",
        "description": body.get("description").cloned().unwrap_or(json!("")),
        "branchMatcher": branch_matcher,
        "domains": [],
        "currentDeploymentAliases": [],
        "createdAt": now,
        "updatedAt": now,
    });
    s.store
        .collection_mut(CUSTOM_ENVS)
        .insert(doc_key, env.clone());
    Ok((StatusCode::CREATED, env))
}

// ───────────────────────────── teams & user ─────────────────────────────

/// `GET /v2/teams`
pub fn list_teams(world: &mut WorldState, query: &Query) -> ApiResult {
    let s = svc(world);
    let limit = limit(query, 20, 100)?;
    let since = ms_param(query, "since")?;
    let until = ms_param(query, "until")?;
    let mut teams: Vec<Value> = docs(s, TEAMS)
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| {
            since.is_none_or(|v| int_of(t, "createdAt") > v)
                && until.is_none_or(|u| int_of(t, "createdAt") < u)
        })
        .collect();
    teams.sort_by_key(|t| std::cmp::Reverse(int_of(t, "createdAt")));
    let more = teams.len() > limit;
    teams.truncate(limit);
    let next = if more {
        teams.last().map(|t| int_of(t, "createdAt"))
    } else {
        None
    };
    let prev = teams.first().map(|t| int_of(t, "createdAt"));
    let n = teams.len();
    Ok((
        StatusCode::OK,
        json!({ "teams": teams, "pagination": pagination(n, next, prev) }),
    ))
}

/// `GET /v2/teams/:teamId`
pub fn get_team(world: &mut WorldState, id_or_slug: &str) -> ApiResult {
    find_team(svc(world), id_or_slug)
        .map(|t| (StatusCode::OK, t))
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "Team not found"))
}

/// `POST /v1/teams`
pub fn create_team(world: &mut WorldState, body: &Value) -> ApiResult {
    let now = now_ms(world);
    let s = svc(world);
    let Some(slug) = body.get("slug").and_then(Value::as_str) else {
        return Err(bad_request(
            "Invalid request: missing required property `slug`.",
        ));
    };
    let valid = !slug.is_empty()
        && slug.len() <= 48
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_slug",
            "The slug can only contain lowercase alphanumeric characters and hyphens, and must be at most 48 characters long.",
        ));
    }
    if find_team(s, slug).is_some() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "slug_in_use",
            format!("The team slug \"{slug}\" is already in use."),
        ));
    }
    let user = viewer(s);
    let id = s.ids.next_prefixed("team", "team_", 24);
    let name = body.get("name").and_then(Value::as_str).unwrap_or(slug);
    let team = json!({
        "id": id,
        "slug": slug,
        "name": name,
        "avatar": null,
        "createdAt": now,
        "creatorId": user["uid"],
        "updatedAt": now,
        "membership": { "role": "OWNER", "confirmed": true, "createdAt": now, "uid": user["uid"] },
    });
    s.store.collection_mut(TEAMS).insert(id.clone(), team);
    Ok((
        StatusCode::OK,
        json!({
            "id": id,
            "slug": slug,
            "billing": {
                "currency": "usd",
                "plan": "pro",
                "status": "trialing",
                "trial": { "start": now, "end": now + 14 * 24 * 3600 * 1000 },
                "email": user["email"],
            },
        }),
    ))
}

/// `GET /v2/user`
pub fn get_user(world: &mut WorldState) -> ApiResult {
    Ok((StatusCode::OK, json!({ "user": viewer(svc(world)) })))
}
