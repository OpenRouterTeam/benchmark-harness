//! Repositories: metadata, contents, Git data, commits, branches, tags, releases,
//! collaborators, forks, custom properties, rulesets, and raw file downloads.

use std::collections::BTreeMap;

use axum::{
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};

use crate::{
    Ctx,
    git::{self, FileEntry, FileMap},
    render::{
        self, account, all, commit_json, get, git_commit_json, is_org, repo_json, repo_key, user,
        with_prefix,
    },
    sha1::blob_sha,
    util::{
        arr, as_i64, b, b64_decode, b64_wrapped, created, error, login_of, names_of, next_id,
        no_content, node_id, not_found, ok, opt_s, paged, percent_encode_path, s, status_json,
        str_list, u, unix, unprocessable, validation, validation_msg,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let get_ = m == Method::GET;
    // Account-level repository endpoints.
    match segs {
        ["user", "repos"] if get_ => {
            let v = ctx.viewer();
            return Some(list_repos(ctx, &v, true));
        }
        ["user", "repos"] if m == Method::POST => {
            let v = ctx.viewer();
            return Some(create_repo(ctx, &v));
        }
        ["users", login, "repos"] if get_ => {
            if account(ctx.svc, login).is_none() {
                return Some(not_found());
            }
            return Some(list_repos(ctx, login, false));
        }
        ["orgs", org, "repos"] if get_ => {
            if !is_org(ctx.svc, org) {
                return Some(not_found());
            }
            return Some(list_repos(ctx, org, true));
        }
        ["orgs", org, "repos"] if m == Method::POST => {
            if !is_org(ctx.svc, org) {
                return Some(not_found());
            }
            let org = account(ctx.svc, org)
                .map(|o| s(o, "login").to_owned())
                .unwrap_or_else(|| (*org).to_owned());
            return Some(create_repo(ctx, &org));
        }
        ["orgs", org, "properties", "schema"] => {
            return Some(org_property_schema(ctx, "org", org, &m));
        }
        ["enterprises", ent, "properties", "schema"] => {
            return Some(org_property_schema(ctx, "enterprise", ent, &m));
        }
        ["orgs", org, "rulesets", rest @ ..] => {
            return Some(owner_rulesets(ctx, "org", org, rest, &m));
        }
        ["enterprises", ent, "rulesets", rest @ ..] => {
            return Some(owner_rulesets(ctx, "enterprise", ent, rest, &m));
        }
        _ => {}
    }

    let ["repos", o, r, rest @ ..] = segs else {
        return None;
    };
    // Routes owned by other modules under /repos/{o}/{r}.
    if let Some(first) = rest.first()
        && matches!(
            *first,
            "issues"
                | "labels"
                | "milestones"
                | "assignees"
                | "issue-types"
                | "pulls"
                | "actions"
                | "statuses"
                | "check-runs"
                | "check-suites"
                | "code-scanning"
                | "dependabot"
                | "secret-scanning"
                | "security-advisories"
                | "code-quality"
                | "notifications"
                | "subscription"
                | "stargazers"
        )
    {
        return None;
    }
    if rest.len() >= 3
        && rest[0] == "commits"
        && matches!(
            rest[rest.len() - 1],
            "status" | "statuses" | "check-runs" | "check-suites" | "pulls"
        )
    {
        return None;
    }
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        // Forks may be created at a new name; everything else is a 404.
        if rest == ["forks"] && m == Method::POST {
            return Some(not_found());
        }
        return Some(not_found());
    };
    Some(match rest {
        [] if get_ => ok(repo_json(ctx.svc, ctx.base(), &repo)),
        [] if m == Method::PATCH => update_repo(ctx, &repo),
        [] if m == Method::DELETE => {
            ctx.svc.store.collection_mut("repos").remove(&repo);
            no_content()
        }
        ["forks"] if m == Method::POST => fork(ctx, &repo),
        ["forks"] if get_ => {
            let forks: Vec<Value> = all(ctx.svc, "repos")
                .iter()
                .filter(|(_, d)| opt_s(d, "parent").as_deref() == Some(repo.as_str()))
                .map(|(k, _)| repo_json(ctx.svc, ctx.base(), k))
                .collect();
            paged(ctx.req, forks)
        }
        ["topics"] if get_ => ok(
            json!({"names": get(ctx.svc, "repos", &repo).map(|d| d.get("topics").cloned().unwrap_or(json!([]))).unwrap_or(json!([]))}),
        ),
        ["languages"] if get_ => ok(languages(ctx, &repo)),
        ["contents", path @ ..] => contents(ctx, &repo, &path.join("/"), &m),
        ["readme"] if get_ => readme(ctx, &repo),
        ["branches"] if get_ => list_branches(ctx, &repo),
        ["branches", branch @ ..] if get_ && !branch.is_empty() => {
            get_branch(ctx, &repo, &branch.join("/"))
        }
        ["tags"] if get_ => list_tags(ctx, &repo),
        ["releases"] if get_ => list_releases(ctx, &repo),
        ["releases"] if m == Method::POST => create_release(ctx, &repo),
        ["releases", "latest"] if get_ => latest_release(ctx, &repo),
        ["releases", "tags", tag] if get_ => release_by_tag(ctx, &repo, tag),
        ["releases", id] if get_ => release_by_id(ctx, &repo, id),
        ["commits"] if get_ => list_commits(ctx, &repo),
        ["commits", reference @ ..] if get_ && !reference.is_empty() => {
            get_commit(ctx, &repo, &reference.join("/"))
        }
        ["compare", spec @ ..] if get_ && !spec.is_empty() => compare(ctx, &repo, &spec.join("/")),
        ["git", "ref", reference @ ..] if get_ => get_ref(ctx, &repo, &reference.join("/")),
        ["git", "refs"] if get_ => list_refs(ctx, &repo, ""),
        ["git", "matching-refs", reference @ ..] if get_ => {
            list_refs(ctx, &repo, &reference.join("/"))
        }
        ["git", "refs"] if m == Method::POST => create_ref(ctx, &repo),
        ["git", "refs", reference @ ..] if m == Method::PATCH => {
            update_ref(ctx, &repo, &reference.join("/"))
        }
        ["git", "refs", reference @ ..] if m == Method::DELETE => {
            delete_ref(ctx, &repo, &reference.join("/"))
        }
        ["git", "refs", reference @ ..] if get_ => get_ref(ctx, &repo, &reference.join("/")),
        ["git", "commits", sha] if get_ => match git::commit(ctx.svc, &repo, sha) {
            Some(doc) => ok(git_commit_json(ctx.svc, ctx.base(), &repo, doc)),
            None => not_found(),
        },
        ["git", "commits"] if m == Method::POST => create_git_commit(ctx, &repo),
        ["git", "trees"] if m == Method::POST => create_tree(ctx, &repo),
        ["git", "trees", treeish @ ..] if get_ => get_tree(ctx, &repo, &treeish.join("/")),
        ["git", "blobs"] if m == Method::POST => create_blob(ctx, &repo),
        ["git", "blobs", sha] if get_ => get_blob(ctx, &repo, sha),
        ["git", "tags", sha] if get_ => get_tag_object(ctx, &repo, sha),
        ["collaborators"] if get_ => list_collaborators(ctx, &repo),
        ["collaborators", login, "permission"] if get_ => permission(ctx, &repo, login),
        ["collaborators", login] if get_ => {
            if collaborator_role(ctx, &repo, login).is_some() {
                no_content()
            } else {
                not_found()
            }
        }
        ["teams"] if get_ => repo_teams(ctx, &repo),
        ["properties", "values"] => repo_property_values(ctx, &repo, &m),
        ["rulesets"] if get_ => list_rulesets(ctx, "repo", &repo),
        ["rulesets"] if m == Method::POST => create_ruleset(ctx, "repo", &repo),
        ["rulesets", "rule-suites"] if get_ => list_rule_suites(ctx, "repo", &repo),
        ["rulesets", "rule-suites", id] if get_ => get_rule_suite(ctx, "repo", &repo, id),
        ["rulesets", id] if get_ => get_ruleset(ctx, "repo", &repo, id),
        ["rules", "branches", branch @ ..] if get_ => {
            rules_for_branch(ctx, &repo, &branch.join("/"))
        }
        _ => not_found(),
    })
}

// ---------------------------------------------------------------- repositories

fn list_repos(ctx: &Ctx, owner: &str, include_private: bool) -> Response {
    let viewer = ctx.viewer();
    let kind = ctx.req.q("type").unwrap_or("all");
    let is_viewer = owner.eq_ignore_ascii_case(&viewer);
    let mut repos: Vec<(&str, &Value)> = all(ctx.svc, "repos")
        .into_iter()
        .filter(|(_, r)| {
            let own = render::repo_owner(r).eq_ignore_ascii_case(owner);
            let collab = is_viewer
                && arr(r, "collaborators")
                    .iter()
                    .any(|c| login_of(c).eq_ignore_ascii_case(&viewer));
            own || (collab && kind != "owner")
        })
        .filter(|(_, r)| include_private || !b(r, "private"))
        .filter(|(_, r)| match kind {
            "public" => !b(r, "private"),
            "private" => b(r, "private"),
            "forks" => b(r, "fork"),
            "sources" => !b(r, "fork"),
            _ => true,
        })
        .collect();
    let sort = ctx.req.q("sort").unwrap_or("full_name");
    match sort {
        "created" | "updated" | "pushed" => {
            let field = format!("{sort}_at");
            repos.sort_by_key(|(_, r)| std::cmp::Reverse(unix(s(r, &field)).unwrap_or(0)));
        }
        _ => repos.sort_by_key(|(k, _)| k.to_ascii_lowercase()),
    }
    if ctx.req.q("direction") == Some("asc") && sort != "full_name" {
        repos.reverse();
    }
    let out: Vec<Value> = repos
        .iter()
        .map(|(k, _)| repo_json(ctx.svc, ctx.base(), k))
        .collect();
    paged(ctx.req, out)
}

fn create_repo(ctx: &mut Ctx, owner: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let name = s(&body, "name").trim().to_owned();
    if name.is_empty() {
        return validation("Repository", "name", "missing_field");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return validation_msg("Repository", "name is invalid");
    }
    let key = format!("{owner}/{name}");
    if repo_key(ctx.svc, owner, &name).is_some() {
        return validation_msg("Repository", "name already exists on this account");
    }
    let id = next_id(ctx.svc, "repos", "id", 1000);
    let private = body
        .get("private")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let doc = json!({
        "id": id,
        "node_id": node_id("R", &key),
        "name": name,
        "full_name": key,
        "owner": owner,
        "private": private,
        "visibility": if private { "private" } else { "public" },
        "description": body.get("description").cloned().filter(|d| d.as_str() != Some("")).unwrap_or(Value::Null),
        "homepage": body.get("homepage").cloned().unwrap_or(Value::Null),
        "default_branch": "main",
        "created_at": ctx.now,
        "updated_at": ctx.now,
        "pushed_at": ctx.now,
        "has_issues": body.get("has_issues").cloned().unwrap_or(json!(true)),
        "has_wiki": body.get("has_wiki").cloned().unwrap_or(json!(true)),
        "topics": [],
        "stargazers_count": 0,
        "forks_count": 0,
        "collaborators": [{"login": ctx.viewer(), "permission": "admin"}],
    });
    ctx.svc
        .store
        .collection_mut("repos")
        .insert(key.clone(), doc);
    if b(&body, "auto_init") {
        let mut map = FileMap::new();
        map.insert(
            "README.md".into(),
            FileEntry {
                content: format!(
                    "# {name}\n{}",
                    body.get("description")
                        .and_then(Value::as_str)
                        .map(|d| format!("{d}\n"))
                        .unwrap_or_default()
                )
                .into_bytes(),
                mode: "100644".into(),
            },
        );
        let author = viewer_person(ctx);
        let viewer = ctx.viewer();
        let now = ctx.now.clone();
        let sha = git::write_commit(
            ctx.svc,
            &key,
            &[],
            &map,
            "Initial commit",
            &author,
            Some(&viewer),
            &now,
        );
        git::set_branch(ctx.svc, &key, "main", &sha);
    }
    let url = format!("{}/repos/{key}", ctx.req.api());
    crate::util::with_header(
        created(repo_json(ctx.svc, ctx.base(), &key)),
        header::LOCATION,
        &url,
    )
}

fn update_repo(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut("repos").get_mut(repo) {
        for k in [
            "description",
            "homepage",
            "private",
            "has_issues",
            "has_wiki",
            "has_projects",
            "has_discussions",
            "default_branch",
            "archived",
            "visibility",
        ] {
            if let Some(v) = body.get(k) {
                d[k] = v.clone();
            }
        }
        d["updated_at"] = json!(now);
    }
    ok(repo_json(ctx.svc, ctx.base(), repo))
}

fn fork(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let src = get(ctx.svc, "repos", repo).cloned().unwrap_or_default();
    let owner = opt_s(&body, "organization").unwrap_or_else(|| ctx.viewer());
    if opt_s(&body, "organization").is_some() && !is_org(ctx.svc, &owner) {
        return validation_msg("Fork", "Invalid organization");
    }
    let name = opt_s(&body, "name").unwrap_or_else(|| s(&src, "name").to_owned());
    let name = if name.is_empty() {
        repo.split('/').nth(1).unwrap_or("fork").to_owned()
    } else {
        name
    };
    let key = format!("{owner}/{name}");
    if get(ctx.svc, "repos", &key).is_none() {
        let id = next_id(ctx.svc, "repos", "id", 1000);
        let mut doc = src.clone();
        doc["id"] = json!(id);
        doc["node_id"] = json!(node_id("R", &key));
        doc["name"] = json!(name);
        doc["full_name"] = json!(key);
        doc["owner"] = json!(owner);
        doc["fork"] = json!(true);
        doc["parent"] = json!(repo);
        doc["created_at"] = json!(ctx.now);
        doc["updated_at"] = json!(ctx.now);
        doc["stargazers_count"] = json!(0);
        doc["forks_count"] = json!(0);
        doc["collaborators"] = json!([{"login": ctx.viewer(), "permission": "admin"}]);
        ctx.svc
            .store
            .collection_mut("repos")
            .insert(key.clone(), doc);
        // Share history: copy commits and branches.
        let only_default = b(&body, "default_branch_only");
        let default = opt_s(&src, "default_branch").unwrap_or_else(|| "main".into());
        let commits: Vec<(String, Value)> = with_prefix(ctx.svc, "commits", &format!("{repo}@"))
            .into_iter()
            .map(|(k, v)| (k.replacen(repo, &key, 1), v.clone()))
            .collect();
        for (k, v) in commits {
            ctx.svc.store.collection_mut("commits").insert(k, v);
        }
        let branches: Vec<(String, Value)> = with_prefix(ctx.svc, "branches", &format!("{repo}:"))
            .into_iter()
            .filter(|(_, v)| !only_default || s(v, "name") == default)
            .map(|(k, v)| (k.replacen(repo, &key, 1), v.clone()))
            .collect();
        for (k, v) in branches {
            ctx.svc.store.collection_mut("branches").insert(k, v);
        }
        if let Some(d) = ctx.svc.store.collection_mut("repos").get_mut(repo) {
            d["forks_count"] = json!(u(d, "forks_count") + 1);
        }
    }
    status_json(StatusCode::ACCEPTED, repo_json(ctx.svc, ctx.base(), &key))
}

fn languages(ctx: &Ctx, repo: &str) -> Value {
    let mut out = Map::new();
    if let Some(sha) = git::resolve_ref(ctx.svc, repo, "")
        && let Some(map) = git::file_map(ctx.svc, repo, &sha)
    {
        for (path, e) in &map {
            let lang = match path.rsplit('.').next().unwrap_or("") {
                "rs" => "Rust",
                "go" => "Go",
                "py" => "Python",
                "ts" | "tsx" => "TypeScript",
                "js" | "jsx" => "JavaScript",
                "rb" => "Ruby",
                "java" => "Java",
                "sh" => "Shell",
                "tf" => "HCL",
                "css" => "CSS",
                "html" => "HTML",
                _ => continue,
            };
            let n = out.get(lang).and_then(Value::as_u64).unwrap_or(0);
            out.insert(lang.into(), json!(n + e.content.len() as u64));
        }
    }
    Value::Object(out)
}

pub fn viewer_person(ctx: &Ctx) -> Value {
    let v = ctx.viewer();
    let doc = account(ctx.svc, &v).cloned().unwrap_or_default();
    json!({
        "name": opt_s(&doc, "name").unwrap_or_else(|| v.clone()),
        "email": opt_s(&doc, "email").unwrap_or_else(|| format!("{v}@users.noreply.github.octo-corp.com")),
        "date": ctx.now,
    })
}

// ---------------------------------------------------------------- contents

fn default_branch(ctx: &Ctx, repo: &str) -> String {
    get(ctx.svc, "repos", repo)
        .and_then(|d| opt_s(d, "default_branch"))
        .unwrap_or_else(|| "main".into())
}

fn empty_repo(ctx: &Ctx, repo: &str) -> bool {
    with_prefix(ctx.svc, "branches", &format!("{repo}:")).is_empty()
}

fn repo_empty_error() -> Response {
    error(StatusCode::CONFLICT, "Git Repository is empty.")
}

fn content_entry(
    ctx: &Ctx,
    repo: &str,
    reference: &str,
    path: &str,
    map: &FileMap,
) -> Option<Value> {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let enc = percent_encode_path(path);
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(e) = map.get(path) {
        let sha = blob_sha(&e.content);
        let self_url = format!(
            "{api}/contents/{enc}?ref={}",
            crate::util::percent_encode_query(reference)
        );
        let html = format!("{}/{repo}/blob/{reference}/{enc}", ctx.base());
        let git_url = format!("{api}/git/blobs/{sha}");
        let kind = if e.mode == "120000" {
            "symlink"
        } else {
            "file"
        };
        let mut v = json!({
            "type": kind,
            "size": e.content.len(),
            "name": name,
            "path": path,
            "sha": sha,
            "url": self_url,
            "git_url": git_url,
            "html_url": html,
            "download_url": format!("{}/raw/{repo}/{reference}/{enc}", ctx.base()),
            "_links": {"self": self_url, "git": git_url, "html": html},
        });
        if kind == "symlink" {
            v["target"] = json!(String::from_utf8_lossy(&e.content));
        }
        return Some(v);
    }
    let dir_prefix = format!("{path}/");
    if path.is_empty() || map.keys().any(|k| k.starts_with(&dir_prefix)) {
        let sha = git::tree_sha(map, path);
        let self_url = format!(
            "{api}/contents/{enc}?ref={}",
            crate::util::percent_encode_query(reference)
        );
        let html = format!("{}/{repo}/tree/{reference}/{enc}", ctx.base());
        let git_url = format!("{api}/git/trees/{sha}");
        return Some(json!({
            "type": "dir",
            "size": 0,
            "name": name,
            "path": path,
            "sha": sha,
            "url": self_url,
            "git_url": git_url,
            "html_url": html,
            "download_url": null,
            "_links": {"self": self_url, "git": git_url, "html": html},
        }));
    }
    None
}

fn contents(ctx: &mut Ctx, repo: &str, path: &str, m: &Method) -> Response {
    let path = path.trim_matches('/').to_owned();
    match *m {
        Method::GET => get_contents(ctx, repo, &path),
        Method::PUT => put_contents(ctx, repo, &path),
        Method::DELETE => delete_contents(ctx, repo, &path),
        _ => not_found(),
    }
}

fn get_contents(ctx: &Ctx, repo: &str, path: &str) -> Response {
    if empty_repo(ctx, repo) {
        return error(StatusCode::NOT_FOUND, "This repository is empty.");
    }
    let reference = ctx
        .req
        .q("ref")
        .map(str::to_owned)
        .unwrap_or_else(|| default_branch(ctx, repo));
    let Some(sha) = git::resolve_ref(ctx.svc, repo, &reference) else {
        return error(
            StatusCode::NOT_FOUND,
            &format!("No commit found for the ref {reference}"),
        );
    };
    let map = git::file_map(ctx.svc, repo, &sha).unwrap_or_default();
    let display_ref = reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/tags/"))
        .unwrap_or(&reference)
        .to_owned();
    if let Some(e) = map.get(path) {
        let mut v = content_entry(ctx, repo, &display_ref, path, &map).unwrap_or_default();
        v["content"] = json!(b64_wrapped(&e.content));
        v["encoding"] = json!("base64");
        if ctx.req.accept().contains("raw") {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/vnd.github.raw")],
                e.content.clone(),
            )
                .into_response();
        }
        return ok(v);
    }
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    if !path.is_empty() && !map.keys().any(|k| k.starts_with(&prefix)) {
        return not_found();
    }
    let entries: Vec<Value> = git::tree_entries(&map, path)
        .iter()
        .filter_map(|e| {
            let p = format!("{prefix}{}", e.path);
            content_entry(ctx, repo, &display_ref, &p, &map)
        })
        .collect();
    ok(Value::Array(entries))
}

fn decode_content(body: &Value) -> Result<Vec<u8>, Response> {
    let Some(c) = body.get("content").and_then(Value::as_str) else {
        return Err(validation("Content", "content", "missing_field"));
    };
    b64_decode(c).ok_or_else(|| unprocessable("content is not valid Base64"))
}

fn put_contents(ctx: &mut Ctx, repo: &str, path: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(message) = opt_s(&body, "message") else {
        return unprocessable("Invalid request.\n\n\"message\" wasn't supplied.");
    };
    let content = match decode_content(&body) {
        Ok(c) => c,
        Err(e) => return e,
    };
    if path.is_empty() {
        return not_found();
    }
    let branch = opt_s(&body, "branch").unwrap_or_else(|| default_branch(ctx, repo));
    let head = git::branch_sha(ctx.svc, repo, &branch);
    if head.is_none() && !empty_repo(ctx, repo) {
        return error(StatusCode::NOT_FOUND, &format!("Branch {branch} not found"));
    }
    let mut map = head
        .as_ref()
        .and_then(|h| git::file_map(ctx.svc, repo, h))
        .unwrap_or_default();
    let existing = map.get(path).map(|e| blob_sha(&e.content));
    let provided = opt_s(&body, "sha");
    match (&existing, &provided) {
        (Some(_), None) => {
            return unprocessable("Invalid request.\n\n\"sha\" wasn't supplied.");
        }
        (Some(cur), Some(p)) if cur != p => {
            return error(StatusCode::CONFLICT, &format!("{path} does not match {p}"));
        }
        _ => {}
    }
    if map.keys().any(|k| k.starts_with(&format!("{path}/"))) {
        return unprocessable(&format!("{path} is a directory"));
    }
    let is_update = existing.is_some();
    let mode = map
        .get(path)
        .map(|e| e.mode.clone())
        .unwrap_or_else(|| "100644".into());
    map.insert(path.to_owned(), FileEntry { content, mode });
    let author = body
        .get("author")
        .cloned()
        .unwrap_or_else(|| viewer_person(ctx));
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let parents: Vec<String> = head.into_iter().collect();
    let sha = git::write_commit(
        ctx.svc,
        repo,
        &parents,
        &map,
        &message,
        &author,
        Some(&viewer),
        &now,
    );
    git::set_branch(ctx.svc, repo, &branch, &sha);
    touch_repo(ctx, repo);
    let content_json = content_entry(ctx, repo, &branch, path, &map).unwrap_or_default();
    let commit = git::commit(ctx.svc, repo, &sha)
        .cloned()
        .unwrap_or_default();
    let commit_json = git_commit_json(ctx.svc, ctx.base(), repo, &commit);
    let out = json!({"content": content_json, "commit": commit_json});
    if is_update { ok(out) } else { created(out) }
}

fn delete_contents(ctx: &mut Ctx, repo: &str, path: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(message) = opt_s(&body, "message") else {
        return unprocessable("Invalid request.\n\n\"message\" wasn't supplied.");
    };
    let Some(provided) = opt_s(&body, "sha") else {
        return unprocessable("Invalid request.\n\n\"sha\" wasn't supplied.");
    };
    let branch = opt_s(&body, "branch").unwrap_or_else(|| default_branch(ctx, repo));
    let Some(head) = git::branch_sha(ctx.svc, repo, &branch) else {
        return error(StatusCode::NOT_FOUND, &format!("Branch {branch} not found"));
    };
    let mut map = git::file_map(ctx.svc, repo, &head).unwrap_or_default();
    let Some(e) = map.get(path) else {
        return not_found();
    };
    if blob_sha(&e.content) != provided {
        return error(
            StatusCode::CONFLICT,
            &format!("{path} does not match {provided}"),
        );
    }
    map.remove(path);
    let author = viewer_person(ctx);
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let sha = git::write_commit(
        ctx.svc,
        repo,
        &[head],
        &map,
        &message,
        &author,
        Some(&viewer),
        &now,
    );
    git::set_branch(ctx.svc, repo, &branch, &sha);
    touch_repo(ctx, repo);
    let commit = git::commit(ctx.svc, repo, &sha)
        .cloned()
        .unwrap_or_default();
    ok(json!({"content": null, "commit": git_commit_json(ctx.svc, ctx.base(), repo, &commit)}))
}

fn touch_repo(ctx: &mut Ctx, repo: &str) {
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut("repos").get_mut(repo) {
        d["pushed_at"] = json!(now);
        d["updated_at"] = json!(now);
    }
}

fn readme(ctx: &Ctx, repo: &str) -> Response {
    let reference = ctx
        .req
        .q("ref")
        .map(str::to_owned)
        .unwrap_or_else(|| default_branch(ctx, repo));
    let Some(sha) = git::resolve_ref(ctx.svc, repo, &reference) else {
        return not_found();
    };
    let map = git::file_map(ctx.svc, repo, &sha).unwrap_or_default();
    let Some(path) = map
        .keys()
        .find(|k| !k.contains('/') && k.to_ascii_lowercase().starts_with("readme"))
        .cloned()
    else {
        return not_found();
    };
    let mut v = content_entry(ctx, repo, &reference, &path, &map).unwrap_or_default();
    v["content"] = json!(b64_wrapped(&map[&path].content));
    v["encoding"] = json!("base64");
    ok(v)
}

/// `GET /raw/{owner}/{repo}/{ref...}/{path}` (GHES without subdomain isolation).
pub fn raw(ctx: &mut Ctx, segs: &[&str]) -> Response {
    let text_404 = || {
        (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "404: Not Found",
        )
            .into_response()
    };
    let [o, r, rest @ ..] = segs else {
        return text_404();
    };
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return text_404();
    };
    for split in 1..rest.len() {
        let reference = rest[..split].join("/");
        let path = rest[split..].join("/");
        let Some(sha) = git::resolve_ref(ctx.svc, &repo, &reference) else {
            continue;
        };
        if let Some(map) = git::file_map(ctx.svc, &repo, &sha)
            && let Some(e) = map.get(&path)
        {
            let ct = if std::str::from_utf8(&e.content).is_ok() {
                "text/plain; charset=utf-8"
            } else {
                "application/octet-stream"
            };
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, ct)],
                e.content.clone(),
            )
                .into_response();
        }
    }
    text_404()
}

// ---------------------------------------------------------------- branches / tags

fn branch_list_json(ctx: &Ctx, repo: &str, d: &Value) -> Value {
    let sha = s(d, "sha");
    json!({
        "name": s(d, "name"),
        "commit": {"sha": sha, "url": format!("{}/repos/{repo}/commits/{sha}", ctx.req.api())},
        "protected": b(d, "protected"),
        "protection": {"enabled": b(d, "protected"), "required_status_checks": {"enforcement_level": if b(d, "protected") { "non_admins" } else { "off" }, "contexts": [], "checks": []}},
        "protection_url": format!("{}/repos/{repo}/branches/{}/protection", ctx.req.api(), s(d, "name")),
    })
}

fn list_branches(ctx: &Ctx, repo: &str) -> Response {
    let protected_only = ctx.req.q("protected") == Some("true");
    let mut branches: Vec<&Value> = with_prefix(ctx.svc, "branches", &format!("{repo}:"))
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| !protected_only || b(d, "protected"))
        .collect();
    branches.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
    let out: Vec<Value> = branches
        .iter()
        .map(|d| branch_list_json(ctx, repo, d))
        .collect();
    paged(ctx.req, out)
}

fn get_branch(ctx: &Ctx, repo: &str, branch: &str) -> Response {
    let Some(d) = get(ctx.svc, "branches", &format!("{repo}:{branch}")) else {
        return error(StatusCode::NOT_FOUND, "Branch not found");
    };
    let mut v = branch_list_json(ctx, repo, d);
    if let Some(c) = git::commit(ctx.svc, repo, s(d, "sha")) {
        v["commit"] = commit_json(ctx.svc, ctx.base(), repo, c, false);
    }
    v["_links"] = json!({
        "self": format!("{}/repos/{repo}/branches/{branch}", ctx.req.api()),
        "html": format!("{}/{repo}/tree/{branch}", ctx.base()),
    });
    ok(v)
}

fn list_tags(ctx: &Ctx, repo: &str) -> Response {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let mut tags: Vec<&Value> = with_prefix(ctx.svc, "tags", &format!("{repo}:"))
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    tags.sort_by(|a, b| {
        let ta =
            git::commit(ctx.svc, repo, s(a, "sha")).map(|c| unix(git::commit_date(c)).unwrap_or(0));
        let tb =
            git::commit(ctx.svc, repo, s(b, "sha")).map(|c| unix(git::commit_date(c)).unwrap_or(0));
        tb.cmp(&ta).then_with(|| s(b, "name").cmp(s(a, "name")))
    });
    let out: Vec<Value> = tags
        .iter()
        .map(|t| {
            let name = s(t, "name");
            let sha = s(t, "sha");
            json!({
                "name": name,
                "zipball_url": format!("{api}/zipball/refs/tags/{name}"),
                "tarball_url": format!("{api}/tarball/refs/tags/{name}"),
                "commit": {"sha": sha, "url": format!("{api}/commits/{sha}")},
                "node_id": node_id("REF", &format!("{repo}:refs/tags/{name}")),
            })
        })
        .collect();
    paged(ctx.req, out)
}

fn ref_json(ctx: &Ctx, repo: &str, full: &str, sha: &str, kind: &str) -> Value {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let obj_url = if kind == "tag" {
        format!("{api}/git/tags/{sha}")
    } else {
        format!("{api}/git/commits/{sha}")
    };
    json!({
        "ref": full,
        "node_id": node_id("REF", &format!("{repo}:{full}")),
        "url": format!("{api}/git/{full}"),
        "object": {"sha": sha, "type": kind, "url": obj_url},
    })
}

fn all_refs(ctx: &Ctx, repo: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut branches: Vec<&Value> = with_prefix(ctx.svc, "branches", &format!("{repo}:"))
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    branches.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
    for d in branches {
        out.push(ref_json(
            ctx,
            repo,
            &format!("refs/heads/{}", s(d, "name")),
            s(d, "sha"),
            "commit",
        ));
    }
    let mut tags: Vec<&Value> = with_prefix(ctx.svc, "tags", &format!("{repo}:"))
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    tags.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
    for t in tags {
        out.push(ref_json(
            ctx,
            repo,
            &format!("refs/tags/{}", s(t, "name")),
            &git::tag_object_sha(repo, t),
            "tag",
        ));
    }
    out
}

fn get_ref(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    if empty_repo(ctx, repo) && with_prefix(ctx.svc, "tags", &format!("{repo}:")).is_empty() {
        return repo_empty_error();
    }
    let full = if reference.starts_with("refs/") {
        reference.to_owned()
    } else {
        format!("refs/{reference}")
    };
    match all_refs(ctx, repo)
        .into_iter()
        .find(|r| s(r, "ref") == full)
    {
        Some(r) => ok(r),
        None => not_found(),
    }
}

fn list_refs(ctx: &Ctx, repo: &str, prefix: &str) -> Response {
    if empty_repo(ctx, repo) {
        return repo_empty_error();
    }
    let full = if prefix.is_empty() {
        "refs/".to_owned()
    } else if prefix.starts_with("refs/") {
        prefix.to_owned()
    } else {
        format!("refs/{prefix}")
    };
    let refs: Vec<Value> = all_refs(ctx, repo)
        .into_iter()
        .filter(|r| s(r, "ref").starts_with(&full))
        .collect();
    paged(ctx.req, refs)
}

fn create_ref(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let reference = s(&body, "ref").to_owned();
    let sha = s(&body, "sha").to_owned();
    if !reference.starts_with("refs/") || reference.matches('/').count() < 2 {
        return unprocessable("Reference name is invalid");
    }
    if git::commit(ctx.svc, repo, &sha).is_none() {
        return unprocessable("Object does not exist");
    }
    if all_refs(ctx, repo).iter().any(|r| s(r, "ref") == reference) {
        return unprocessable("Reference already exists");
    }
    if let Some(branch) = reference.strip_prefix("refs/heads/") {
        git::set_branch(ctx.svc, repo, branch, &sha);
        let url = format!("{}/repos/{repo}/git/{reference}", ctx.req.api());
        return crate::util::with_header(
            created(ref_json(ctx, repo, &reference, &sha, "commit")),
            header::LOCATION,
            &url,
        );
    }
    if let Some(tag) = reference.strip_prefix("refs/tags/") {
        let doc = json!({"name": tag, "sha": sha, "message": "", "lightweight": true});
        ctx.svc
            .store
            .collection_mut("tags")
            .insert(format!("{repo}:{tag}"), doc);
        return created(ref_json(ctx, repo, &reference, &sha, "commit"));
    }
    unprocessable("Reference name is invalid")
}

fn update_ref(ctx: &mut Ctx, repo: &str, reference: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let sha = s(&body, "sha").to_owned();
    let force = b(&body, "force");
    let full = if reference.starts_with("refs/") {
        reference.to_owned()
    } else {
        format!("refs/{reference}")
    };
    let Some(branch) = full.strip_prefix("refs/heads/") else {
        return unprocessable("Reference cannot be updated");
    };
    let Some(cur) = git::branch_sha(ctx.svc, repo, branch) else {
        return unprocessable("Reference does not exist");
    };
    if git::commit(ctx.svc, repo, &sha).is_none() {
        return unprocessable("Object does not exist");
    }
    if !force && !git::ancestors(ctx.svc, repo, &sha).contains(&cur) {
        return unprocessable("Update is not a fast forward");
    }
    git::set_branch(ctx.svc, repo, branch, &sha);
    touch_repo(ctx, repo);
    ok(ref_json(ctx, repo, &full, &sha, "commit"))
}

fn delete_ref(ctx: &mut Ctx, repo: &str, reference: &str) -> Response {
    let full = if reference.starts_with("refs/") {
        reference.to_owned()
    } else {
        format!("refs/{reference}")
    };
    let removed = if let Some(branch) = full.strip_prefix("refs/heads/") {
        ctx.svc
            .store
            .collection_mut("branches")
            .remove(&format!("{repo}:{branch}"))
    } else if let Some(tag) = full.strip_prefix("refs/tags/") {
        ctx.svc
            .store
            .collection_mut("tags")
            .remove(&format!("{repo}:{tag}"))
    } else {
        None
    };
    match removed {
        Some(_) => no_content(),
        None => unprocessable("Reference does not exist"),
    }
}

fn get_tag_object(ctx: &Ctx, repo: &str, sha: &str) -> Response {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let Some(t) = with_prefix(ctx.svc, "tags", &format!("{repo}:"))
        .into_iter()
        .map(|(_, t)| t)
        .find(|t| git::tag_object_sha(repo, t) == sha)
    else {
        return not_found();
    };
    let tagger = t.get("tagger").cloned().unwrap_or_else(|| {
        let date = git::commit(ctx.svc, repo, s(t, "sha"))
            .map(|c| git::commit_date(c).to_owned())
            .unwrap_or_default();
        json!({"name": "GitHub Enterprise", "email": "noreply@github.octo-corp.com", "date": date})
    });
    let target = s(t, "sha");
    ok(json!({
        "node_id": node_id("TAG", &format!("{repo}:{sha}")),
        "tag": s(t, "name"),
        "sha": sha,
        "url": format!("{api}/git/tags/{sha}"),
        "message": s(t, "message"),
        "tagger": tagger,
        "object": {"type": "commit", "sha": target, "url": format!("{api}/git/commits/{target}")},
        "verification": {"verified": false, "reason": "unsigned", "signature": null, "payload": null, "verified_at": null},
    }))
}

// ---------------------------------------------------------------- git data

fn tree_json(ctx: &Ctx, repo: &str, map: &FileMap, dir: &str, recursive: bool) -> Value {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let mut entries = Vec::new();
    let mut stack = vec![(dir.to_owned(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        for e in git::tree_entries(map, &d) {
            let rel_path = if rel.is_empty() {
                e.path.clone()
            } else {
                format!("{rel}/{}", e.path)
            };
            let url = if e.kind == "tree" {
                format!("{api}/git/trees/{}", e.sha)
            } else {
                format!("{api}/git/blobs/{}", e.sha)
            };
            let mut v =
                json!({"path": rel_path, "mode": e.mode, "type": e.kind, "sha": e.sha, "url": url});
            if let Some(size) = e.size {
                v["size"] = json!(size);
            }
            entries.push(v);
            if recursive && e.kind == "tree" {
                let full = if d.is_empty() {
                    e.path.clone()
                } else {
                    format!("{d}/{}", e.path)
                };
                stack.push((full, rel_path));
            }
        }
    }
    if recursive {
        entries.sort_by(|a, b| s(a, "path").cmp(s(b, "path")));
    }
    let sha = git::tree_sha(map, dir);
    json!({"sha": sha, "url": format!("{api}/git/trees/{sha}"), "tree": entries, "truncated": false})
}

fn get_tree(ctx: &Ctx, repo: &str, treeish: &str) -> Response {
    let recursive = ctx
        .req
        .q("recursive")
        .is_some_and(|v| v != "0" && v != "false");
    if empty_repo(ctx, repo) {
        return repo_empty_error();
    }
    if let Some(sha) = git::resolve_ref(ctx.svc, repo, treeish)
        && let Some(map) = git::file_map(ctx.svc, repo, &sha)
    {
        return ok(tree_json(ctx, repo, &map, "", recursive));
    }
    match git::find_tree(ctx.svc, repo, treeish) {
        Some((map, dir)) => ok(tree_json(ctx, repo, &map, &dir, recursive)),
        None => not_found(),
    }
}

fn create_tree(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut map = match opt_s(&body, "base_tree") {
        Some(base) => match git::find_tree(ctx.svc, repo, &base) {
            Some((m, dir)) if dir.is_empty() => m,
            Some(_) => return unprocessable("base_tree must be a root tree"),
            None => match git::resolve_ref(ctx.svc, repo, &base)
                .and_then(|sha| git::file_map(ctx.svc, repo, &sha))
            {
                Some(m) => m,
                None => return unprocessable("Invalid tree info"),
            },
        },
        None => FileMap::new(),
    };
    let Some(entries) = body.get("tree").and_then(Value::as_array) else {
        return validation("Tree", "tree", "missing_field");
    };
    for e in entries {
        let path = s(e, "path").trim_matches('/').to_owned();
        if path.is_empty() {
            return unprocessable("tree.path is required");
        }
        let mode = opt_s(e, "mode").unwrap_or_else(|| "100644".into());
        let has_sha = e.get("sha").is_some();
        let sha = e.get("sha").and_then(Value::as_str);
        if let Some(content) = e.get("content").and_then(Value::as_str) {
            map.insert(
                path,
                FileEntry {
                    content: content.as_bytes().to_vec(),
                    mode,
                },
            );
        } else if has_sha && sha.is_none() {
            // `"sha": null` deletes the path (file or directory).
            let prefix = format!("{path}/");
            let existed = map.remove(&path).is_some();
            let before = map.len();
            map.retain(|k, _| !k.starts_with(&prefix));
            if !existed && before == map.len() {
                // GitHub ignores deletes of missing paths.
            }
        } else if let Some(sha) = sha {
            match git::find_blob(ctx.svc, repo, sha) {
                Some(content) => {
                    map.insert(path, FileEntry { content, mode });
                }
                None => return unprocessable(&format!("tree.sha {sha} is not a valid blob")),
            }
        } else {
            return unprocessable("Must supply tree.sha or tree.content");
        }
    }
    let sha = git::tree_sha(&map, "");
    ctx.svc
        .store
        .collection_mut("trees")
        .insert(git::commit_key(repo, &sha), git::snapshot_doc(&map));
    let v = tree_json(ctx, repo, &map, "", false);
    created(v)
}

fn create_git_commit(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(message) = opt_s(&body, "message") else {
        return validation("Commit", "message", "missing_field");
    };
    let tree = s(&body, "tree").to_owned();
    let Some((map, dir)) = git::find_tree(ctx.svc, repo, &tree) else {
        return unprocessable("Tree SHA does not exist");
    };
    if !dir.is_empty() {
        return unprocessable("Tree SHA does not exist");
    }
    let parents = str_list(body.get("parents").unwrap_or(&Value::Null));
    for p in &parents {
        if git::commit(ctx.svc, repo, p).is_none() {
            return unprocessable("Parent SHA does not exist or is not a commit object");
        }
    }
    let author = body
        .get("author")
        .cloned()
        .unwrap_or_else(|| viewer_person(ctx));
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let sha = git::write_commit(
        ctx.svc,
        repo,
        &parents,
        &map,
        &message,
        &author,
        Some(&viewer),
        &now,
    );
    let doc = git::commit(ctx.svc, repo, &sha)
        .cloned()
        .unwrap_or_default();
    created(git_commit_json(ctx.svc, ctx.base(), repo, &doc))
}

fn create_blob(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(content) = body.get("content").and_then(Value::as_str) else {
        return validation("Blob", "content", "missing_field");
    };
    let bytes = if s(&body, "encoding") == "base64" {
        match b64_decode(content) {
            Some(b) => b,
            None => return unprocessable("content is not valid Base64"),
        }
    } else {
        content.as_bytes().to_vec()
    };
    let sha = blob_sha(&bytes);
    let doc = match std::str::from_utf8(&bytes) {
        Ok(t) => json!({"content": t}),
        Err(_) => json!({"content_base64": crate::util::b64_encode(&bytes)}),
    };
    ctx.svc
        .store
        .collection_mut("blobs")
        .insert(git::commit_key(repo, &sha), doc);
    created(json!({"sha": sha, "url": format!("{}/repos/{repo}/git/blobs/{sha}", ctx.req.api())}))
}

fn get_blob(ctx: &Ctx, repo: &str, sha: &str) -> Response {
    let Some(content) = git::find_blob(ctx.svc, repo, sha) else {
        return not_found();
    };
    if ctx.req.accept().contains("raw") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/vnd.github.raw")],
            content,
        )
            .into_response();
    }
    ok(json!({
        "sha": sha,
        "node_id": node_id("B", &format!("{repo}@{sha}")),
        "size": content.len(),
        "url": format!("{}/repos/{repo}/git/blobs/{sha}", ctx.req.api()),
        "content": b64_wrapped(&content),
        "encoding": "base64",
    }))
}

// ---------------------------------------------------------------- commits

fn list_commits(ctx: &Ctx, repo: &str) -> Response {
    if empty_repo(ctx, repo) {
        return repo_empty_error();
    }
    let start = ctx.req.q("sha").unwrap_or("").to_owned();
    let Some(sha) = git::resolve_ref(ctx.svc, repo, &start) else {
        return error(
            StatusCode::NOT_FOUND,
            &format!("No commit found for SHA: {start}"),
        );
    };
    let path = ctx.req.q("path").map(|p| p.trim_matches('/').to_owned());
    let author = ctx.req.q("author").map(str::to_ascii_lowercase);
    let since = ctx.req.q("since").and_then(unix);
    let until = ctx.req.q("until").and_then(unix);
    let commits: Vec<Value> = git::log(ctx.svc, repo, &sha)
        .into_iter()
        .filter(|c| {
            path.as_ref().is_none_or(|p| {
                git::commit_changes(ctx.svc, repo, s(c, "sha"))
                    .iter()
                    .any(|f| f.filename == *p || f.filename.starts_with(&format!("{p}/")))
            })
        })
        .filter(|c| {
            author.as_ref().is_none_or(|a| {
                let login = render::commit_login(ctx.svc, c, "author")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let email = c
                    .get("author")
                    .and_then(|p| p.get("email"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let name = c
                    .get("author")
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                login == *a || email == *a || name == *a
            })
        })
        .filter(|c| {
            let t = unix(git::commit_date(c)).unwrap_or(0);
            since.is_none_or(|x| t >= x) && until.is_none_or(|x| t <= x)
        })
        .map(|c| commit_json(ctx.svc, ctx.base(), repo, &c, false))
        .collect();
    paged(ctx.req, commits)
}

fn get_commit(ctx: &Ctx, repo: &str, reference: &str) -> Response {
    let Some(sha) = git::resolve_ref(ctx.svc, repo, reference) else {
        return unprocessable(&format!("No commit found for SHA: {reference}"));
    };
    let Some(doc) = git::commit(ctx.svc, repo, &sha) else {
        return unprocessable(&format!("No commit found for SHA: {reference}"));
    };
    let mut v = commit_json(ctx.svc, ctx.base(), repo, doc, true);
    // Files are paginated (default 300 on GitHub; honour page/per_page if given).
    if ctx.req.q("per_page").is_some()
        && let Some(files) = v.get("files").and_then(Value::as_array).cloned()
    {
        let params = ctx.req.page();
        v["files"] = Value::Array(mock_core::http::paginate(&files, params));
    }
    ok(v)
}

fn compare(ctx: &Ctx, repo: &str, spec: &str) -> Response {
    let (base_ref, head_ref) = spec
        .split_once("...")
        .or_else(|| spec.split_once(".."))
        .unwrap_or((spec, ""));
    let (Some(base), Some(head)) = (
        git::resolve_ref(ctx.svc, repo, base_ref),
        git::resolve_ref(ctx.svc, repo, head_ref),
    ) else {
        return not_found();
    };
    let mb = git::merge_base(ctx.svc, repo, &base, &head).unwrap_or_else(|| base.clone());
    let ahead = git::commits_between(ctx.svc, repo, &base, &head);
    let behind = git::commits_between(ctx.svc, repo, &head, &base);
    let status = match (ahead.is_empty(), behind.is_empty()) {
        (true, true) => "identical",
        (false, true) => "ahead",
        (true, false) => "behind",
        (false, false) => "diverged",
    };
    let old = git::file_map(ctx.svc, repo, &mb).unwrap_or_default();
    let new = git::file_map(ctx.svc, repo, &head).unwrap_or_default();
    let changes = git::diff_maps(&old, &new, &BTreeMap::new());
    let base_doc = git::commit(ctx.svc, repo, &base)
        .cloned()
        .unwrap_or_default();
    let mb_doc = git::commit(ctx.svc, repo, &mb).cloned().unwrap_or_default();
    let api = format!("{}/repos/{repo}", ctx.req.api());
    ok(json!({
        "url": format!("{api}/compare/{spec}"),
        "html_url": format!("{}/{repo}/compare/{spec}", ctx.base()),
        "permalink_url": format!("{}/{repo}/compare/{}...{}", ctx.base(), &base[..7.min(base.len())], &head[..7.min(head.len())]),
        "diff_url": format!("{}/{repo}/compare/{spec}.diff", ctx.base()),
        "patch_url": format!("{}/{repo}/compare/{spec}.patch", ctx.base()),
        "base_commit": commit_json(ctx.svc, ctx.base(), repo, &base_doc, false),
        "merge_base_commit": commit_json(ctx.svc, ctx.base(), repo, &mb_doc, false),
        "status": status,
        "ahead_by": ahead.len(),
        "behind_by": behind.len(),
        "total_commits": ahead.len(),
        "commits": ahead.iter().map(|c| commit_json(ctx.svc, ctx.base(), repo, c, false)).collect::<Vec<_>>(),
        "files": changes.iter().map(|c| render::file_change_json(ctx.base(), repo, &head, c)).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------- releases

fn release_json(ctx: &Ctx, repo: &str, r: &Value) -> Value {
    let api = format!("{}/repos/{repo}", ctx.req.api());
    let id = u(r, "id");
    let tag = s(r, "tag_name");
    json!({
        "url": format!("{api}/releases/{id}"),
        "assets_url": format!("{api}/releases/{id}/assets"),
        "upload_url": format!("{}/api/uploads/repos/{repo}/releases/{id}/assets{{?name,label}}", ctx.base()),
        "html_url": format!("{}/{repo}/releases/tag/{tag}", ctx.base()),
        "id": id,
        "author": render::user_ref(ctx.svc, ctx.base(), r.get("author").unwrap_or(&Value::Null)),
        "node_id": opt_s(r, "node_id").unwrap_or_else(|| node_id("RE", &id.to_string())),
        "tag_name": tag,
        "target_commitish": opt_s(r, "target_commitish").unwrap_or_else(|| "main".into()),
        "name": r.get("name").cloned().unwrap_or(Value::Null),
        "draft": b(r, "draft"),
        "immutable": false,
        "prerelease": b(r, "prerelease"),
        "created_at": r.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": r.get("updated_at").cloned().or_else(|| r.get("published_at").cloned()).unwrap_or(Value::Null),
        "published_at": r.get("published_at").cloned().unwrap_or(Value::Null),
        "assets": r.get("assets").cloned().unwrap_or(json!([])),
        "tarball_url": format!("{api}/tarball/{tag}"),
        "zipball_url": format!("{api}/zipball/{tag}"),
        "body": r.get("body").cloned().unwrap_or(Value::Null),
    })
}

fn repo_releases<'a>(ctx: &'a Ctx, repo: &str) -> Vec<&'a Value> {
    let mut v: Vec<&Value> = all(ctx.svc, "releases")
        .into_iter()
        .map(|(_, r)| r)
        .filter(|r| s(r, "repo") == repo)
        .collect();
    v.sort_by_key(|r| {
        std::cmp::Reverse(
            unix(s(r, "published_at"))
                .or_else(|| unix(s(r, "created_at")))
                .unwrap_or(0),
        )
    });
    v
}

fn list_releases(ctx: &Ctx, repo: &str) -> Response {
    let out: Vec<Value> = repo_releases(ctx, repo)
        .iter()
        .map(|r| release_json(ctx, repo, r))
        .collect();
    paged(ctx.req, out)
}

fn latest_release(ctx: &Ctx, repo: &str) -> Response {
    match repo_releases(ctx, repo)
        .into_iter()
        .find(|r| !b(r, "draft") && !b(r, "prerelease"))
    {
        Some(r) => ok(release_json(ctx, repo, r)),
        None => not_found(),
    }
}

fn release_by_tag(ctx: &Ctx, repo: &str, tag: &str) -> Response {
    match repo_releases(ctx, repo)
        .into_iter()
        .find(|r| s(r, "tag_name") == tag)
    {
        Some(r) => ok(release_json(ctx, repo, r)),
        None => not_found(),
    }
}

fn release_by_id(ctx: &Ctx, repo: &str, id: &str) -> Response {
    match repo_releases(ctx, repo)
        .into_iter()
        .find(|r| u(r, "id").to_string() == id)
    {
        Some(r) => ok(release_json(ctx, repo, r)),
        None => not_found(),
    }
}

fn create_release(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(tag) = opt_s(&body, "tag_name") else {
        return validation("Release", "tag_name", "missing_field");
    };
    if repo_releases(ctx, repo)
        .iter()
        .any(|r| s(r, "tag_name") == tag)
    {
        return validation("Release", "tag_name", "already_exists");
    }
    let id = next_id(ctx.svc, "releases", "id", 1);
    let target = opt_s(&body, "target_commitish").unwrap_or_else(|| default_branch(ctx, repo));
    let doc = json!({
        "id": id,
        "repo": repo,
        "tag_name": tag,
        "target_commitish": target,
        "name": body.get("name").cloned().unwrap_or(Value::Null),
        "body": body.get("body").cloned().unwrap_or(Value::Null),
        "draft": b(&body, "draft"),
        "prerelease": b(&body, "prerelease"),
        "author": ctx.viewer(),
        "created_at": ctx.now,
        "published_at": if b(&body, "draft") { Value::Null } else { json!(ctx.now) },
    });
    if get(ctx.svc, "tags", &format!("{repo}:{tag}")).is_none()
        && let Some(sha) = git::resolve_ref(ctx.svc, repo, &target)
    {
        ctx.svc.store.collection_mut("tags").insert(
            format!("{repo}:{tag}"),
            json!({"name": tag, "sha": sha, "message": ""}),
        );
    }
    ctx.svc
        .store
        .collection_mut("releases")
        .insert(id.to_string(), doc.clone());
    created(release_json(ctx, repo, &doc))
}

// ---------------------------------------------------------------- collaborators / teams

fn collaborator_role(ctx: &Ctx, repo: &str, login: &str) -> Option<String> {
    let owner = repo.split('/').next().unwrap_or("");
    if owner.eq_ignore_ascii_case(login) {
        return Some("admin".into());
    }
    let r = get(ctx.svc, "repos", repo)?;
    arr(r, "collaborators")
        .iter()
        .find(|c| login_of(c).eq_ignore_ascii_case(login))
        .map(|c| opt_s(c, "permission").unwrap_or_else(|| "write".into()))
}

fn permissions_for(role: &str) -> Value {
    let (admin, maintain, push, triage) = match role {
        "admin" => (true, true, true, true),
        "maintain" => (false, true, true, true),
        "write" | "push" => (false, false, true, true),
        "triage" => (false, false, false, true),
        _ => (false, false, false, false),
    };
    json!({"admin": admin, "maintain": maintain, "push": push, "triage": triage, "pull": true})
}

fn list_collaborators(ctx: &Ctx, repo: &str) -> Response {
    let Some(r) = get(ctx.svc, "repos", repo) else {
        return not_found();
    };
    let affiliation = ctx.req.q("affiliation").unwrap_or("all");
    let mut out = Vec::new();
    for c in arr(r, "collaborators") {
        let login = login_of(c);
        if affiliation == "outside" && !b(c, "outside") {
            continue;
        }
        if affiliation == "direct" && b(c, "via_team") {
            continue;
        }
        let role = opt_s(c, "permission").unwrap_or_else(|| "write".into());
        let mut uj = user(ctx.svc, ctx.base(), &login);
        uj["permissions"] = permissions_for(&role);
        uj["role_name"] = json!(role);
        out.push(uj);
    }
    paged(ctx.req, out)
}

fn permission(ctx: &Ctx, repo: &str, login: &str) -> Response {
    if account(ctx.svc, login).is_none() {
        return not_found();
    }
    let role = collaborator_role(ctx, repo, login).unwrap_or_else(|| "read".into());
    let permission = match role.as_str() {
        "maintain" | "push" => "write",
        "triage" => "read",
        r => r,
    };
    let mut uj = user(ctx.svc, ctx.base(), login);
    uj["permissions"] = permissions_for(&role);
    uj["role_name"] = json!(role);
    ok(json!({"permission": permission, "role_name": role, "user": uj}))
}

fn repo_teams(ctx: &Ctx, repo: &str) -> Response {
    let owner = repo.split('/').next().unwrap_or("");
    let teams: Vec<Value> = with_prefix(ctx.svc, "teams", &format!("{owner}/"))
        .iter()
        .filter(|(_, t)| names_of(arr(t, "repos")).iter().any(|r| r == repo))
        .map(|(_, t)| render::team_json(ctx.svc, ctx.base(), owner, s(t, "slug")))
        .collect();
    paged(ctx.req, teams)
}

// ---------------------------------------------------------------- custom properties

fn schema_owner_ok(ctx: &Ctx, kind: &str, owner: &str) -> bool {
    match kind {
        "org" => is_org(ctx.svc, owner),
        _ => get(ctx.svc, "enterprises", owner).is_some(),
    }
}

fn property_defs<'a>(ctx: &'a Ctx, kind: &str, owner: &str) -> Vec<&'a Value> {
    let prefix = format!("{kind}:{owner}:");
    with_prefix(ctx.svc, "custom_properties", &prefix)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

fn property_json(kind: &str, owner: &str, p: &Value) -> Value {
    let mut v = json!({
        "property_name": s(p, "property_name"),
        "url": format!("/{}/{owner}/properties/schema/{}", if kind == "org" { "orgs" } else { "enterprises" }, s(p, "property_name")),
        "source_type": if kind == "org" { "organization" } else { "enterprise" },
        "value_type": opt_s(p, "value_type").unwrap_or_else(|| "string".into()),
        "required": b(p, "required"),
        "default_value": p.get("default_value").cloned().unwrap_or(Value::Null),
        "description": p.get("description").cloned().unwrap_or(Value::Null),
        "allowed_values": p.get("allowed_values").cloned().unwrap_or(Value::Null),
        "values_editable_by": opt_s(p, "values_editable_by").unwrap_or_else(|| "org_actors".into()),
    });
    if let Some(r) = p.get("require_explicit_values") {
        v["require_explicit_values"] = r.clone();
    }
    v
}

fn org_property_schema(ctx: &mut Ctx, kind: &str, owner: &str, m: &Method) -> Response {
    if !schema_owner_ok(ctx, kind, owner) {
        return not_found();
    }
    match *m {
        Method::GET => {
            let out: Vec<Value> = property_defs(ctx, kind, owner)
                .iter()
                .map(|p| property_json(kind, owner, p))
                .collect();
            ok(Value::Array(out))
        }
        Method::PATCH => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let Some(props) = body.get("properties").and_then(Value::as_array) else {
                return validation("CustomProperty", "properties", "missing_field");
            };
            let mut updated = Vec::new();
            for p in props {
                let name = s(p, "property_name").to_owned();
                if name.is_empty() {
                    return validation("CustomProperty", "property_name", "missing_field");
                }
                let vt = s(p, "value_type");
                if ![
                    "string",
                    "single_select",
                    "multi_select",
                    "true_false",
                    "url",
                ]
                .contains(&vt)
                {
                    return validation("CustomProperty", "value_type", "invalid");
                }
                let mut doc = p.clone();
                doc["property_name"] = json!(name);
                ctx.svc
                    .store
                    .collection_mut("custom_properties")
                    .insert(format!("{kind}:{owner}:{name}"), doc.clone());
                updated.push(property_json(kind, owner, &doc));
            }
            ok(Value::Array(updated))
        }
        _ => not_found(),
    }
}

fn repo_property_values(ctx: &mut Ctx, repo: &str, m: &Method) -> Response {
    let owner = repo.split('/').next().unwrap_or("").to_owned();
    match *m {
        Method::GET => {
            let values = get(ctx.svc, "repos", repo)
                .and_then(|r| r.get("custom_properties").cloned())
                .unwrap_or(json!({}));
            let mut out: Vec<Value> = Vec::new();
            for p in property_defs(ctx, "org", &owner) {
                let name = s(p, "property_name");
                let v = values
                    .get(name)
                    .cloned()
                    .or_else(|| p.get("default_value").cloned())
                    .unwrap_or(Value::Null);
                out.push(json!({"property_name": name, "value": v}));
            }
            if let Some(o) = values.as_object() {
                for (k, v) in o {
                    if !out.iter().any(|e| s(e, "property_name") == k) {
                        out.push(json!({"property_name": k, "value": v}));
                    }
                }
            }
            ok(Value::Array(out))
        }
        Method::PATCH => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let Some(props) = body.get("properties").and_then(Value::as_array) else {
                return validation("CustomPropertyValue", "properties", "missing_field");
            };
            let defs: Vec<Value> = property_defs(ctx, "org", &owner)
                .into_iter()
                .cloned()
                .collect();
            let mut updates = Vec::new();
            for p in props {
                let name = s(p, "property_name").to_owned();
                let Some(def) = defs.iter().find(|d| s(d, "property_name") == name) else {
                    return unprocessable(&format!("Property {name} does not exist"));
                };
                let value = p.get("value").cloned().unwrap_or(Value::Null);
                if let Some(allowed) = def.get("allowed_values").and_then(Value::as_array)
                    && let Some(v) = value.as_str()
                    && !allowed.iter().any(|a| a.as_str() == Some(v))
                {
                    return unprocessable(&format!("{v} is not a valid value for {name}"));
                }
                updates.push((name, value));
            }
            if let Some(r) = ctx.svc.store.collection_mut("repos").get_mut(repo) {
                if !r.get("custom_properties").is_some_and(Value::is_object) {
                    r["custom_properties"] = json!({});
                }
                for (name, value) in updates {
                    if value.is_null() {
                        r["custom_properties"]
                            .as_object_mut()
                            .map(|o| o.remove(&name));
                    } else {
                        r["custom_properties"][name] = value;
                    }
                }
            }
            no_content()
        }
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- rulesets

fn ruleset_source(kind: &str) -> &'static str {
    match kind {
        "repo" => "Repository",
        "org" => "Organization",
        _ => "Enterprise",
    }
}

fn ruleset_json(ctx: &Ctx, d: &Value, full: bool) -> Value {
    let id = u(d, "id");
    let kind = s(d, "scope");
    let source = s(d, "source");
    let api = ctx.req.api();
    let self_url = match kind {
        "repo" => format!("{api}/repos/{source}/rulesets/{id}"),
        "org" => format!("{api}/orgs/{source}/rulesets/{id}"),
        _ => format!("{api}/enterprises/{source}/rulesets/{id}"),
    };
    let mut v = json!({
        "id": id,
        "name": s(d, "name"),
        "target": opt_s(d, "target").unwrap_or_else(|| "branch".into()),
        "source_type": ruleset_source(kind),
        "source": source,
        "enforcement": opt_s(d, "enforcement").unwrap_or_else(|| "active".into()),
        "node_id": opt_s(d, "node_id").unwrap_or_else(|| node_id("RRS", &id.to_string())),
        "_links": {"self": {"href": self_url}, "html": {"href": format!("{}/{}/rules/{id}", ctx.base(), source)}},
        "created_at": d.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": d.get("updated_at").cloned().unwrap_or(Value::Null),
    });
    if full {
        v["bypass_actors"] = d.get("bypass_actors").cloned().unwrap_or(json!([]));
        v["current_user_can_bypass"] = json!("never");
        v["conditions"] = d.get("conditions").cloned().unwrap_or(Value::Null);
        v["rules"] = d.get("rules").cloned().unwrap_or(json!([]));
    }
    v
}

fn rulesets_for<'a>(ctx: &'a Ctx, kind: &str, source: &str) -> Vec<&'a Value> {
    all(ctx.svc, "rulesets")
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| s(d, "scope") == kind && s(d, "source").eq_ignore_ascii_case(source))
        .collect()
}

fn list_rulesets(ctx: &Ctx, kind: &str, source: &str) -> Response {
    let mut sets: Vec<&Value> = rulesets_for(ctx, kind, source);
    if kind == "repo" && ctx.req.q("includes_parents") != Some("false") {
        let owner = source.split('/').next().unwrap_or("");
        sets.extend(
            rulesets_for(ctx, "org", owner)
                .into_iter()
                .filter(|d| ruleset_targets_repo(d, source)),
        );
    }
    let out: Vec<Value> = sets.iter().map(|d| ruleset_json(ctx, d, false)).collect();
    paged(ctx.req, out)
}

fn get_ruleset(ctx: &Ctx, kind: &str, source: &str, id: &str) -> Response {
    let mut sets = rulesets_for(ctx, kind, source);
    if kind == "repo" && ctx.req.q("includes_parents") != Some("false") {
        let owner = source.split('/').next().unwrap_or("");
        sets.extend(rulesets_for(ctx, "org", owner));
    }
    match sets.into_iter().find(|d| u(d, "id").to_string() == id) {
        Some(d) => ok(ruleset_json(ctx, d, true)),
        None => not_found(),
    }
}

fn create_ruleset(ctx: &mut Ctx, kind: &str, source: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(name) = opt_s(&body, "name") else {
        return validation("Ruleset", "name", "missing_field");
    };
    let enforcement = opt_s(&body, "enforcement").unwrap_or_default();
    if !["disabled", "active", "evaluate"].contains(&enforcement.as_str()) {
        return validation("Ruleset", "enforcement", "invalid");
    }
    if rulesets_for(ctx, kind, source)
        .iter()
        .any(|d| s(d, "name") == name)
    {
        return validation_msg("Ruleset", "Name must be unique");
    }
    let id = next_id(ctx.svc, "rulesets", "id", 1);
    let doc = json!({
        "id": id,
        "scope": kind,
        "source": source,
        "name": name,
        "target": opt_s(&body, "target").unwrap_or_else(|| "branch".into()),
        "enforcement": enforcement,
        "bypass_actors": body.get("bypass_actors").cloned().unwrap_or(json!([])),
        "conditions": body.get("conditions").cloned().unwrap_or(Value::Null),
        "rules": body.get("rules").cloned().unwrap_or(json!([])),
        "created_at": ctx.now,
        "updated_at": ctx.now,
    });
    ctx.svc
        .store
        .collection_mut("rulesets")
        .insert(id.to_string(), doc.clone());
    created(ruleset_json(ctx, &doc, true))
}

fn owner_rulesets(ctx: &mut Ctx, kind: &str, owner: &str, rest: &[&str], m: &Method) -> Response {
    let exists = match kind {
        "org" => is_org(ctx.svc, owner),
        _ => get(ctx.svc, "enterprises", owner).is_some(),
    };
    if !exists {
        return not_found();
    }
    match (rest, m) {
        ([], &Method::GET) => list_rulesets(ctx, kind, owner),
        ([], &Method::POST) => create_ruleset(ctx, kind, owner),
        (["rule-suites"], &Method::GET) => list_rule_suites(ctx, kind, owner),
        (["rule-suites", id], &Method::GET) => get_rule_suite(ctx, kind, owner, id),
        ([id], &Method::GET) => get_ruleset(ctx, kind, owner, id),
        _ => not_found(),
    }
}

fn glob_match(pattern: &str, value: &str) -> bool {
    fn go(p: &[u8], v: &[u8]) -> bool {
        match p.first() {
            None => v.is_empty(),
            Some(b'*') => {
                let double = p.get(1) == Some(&b'*');
                let rest = if double { &p[2..] } else { &p[1..] };
                (0..=v.len()).any(|i| (double || !v[..i].contains(&b'/')) && go(rest, &v[i..]))
            }
            Some(c) => v.first() == Some(c) && go(&p[1..], &v[1..]),
        }
    }
    go(pattern.as_bytes(), value.as_bytes())
}

fn ref_matches(conditions: &Value, full_ref: &str, default_branch: &str) -> bool {
    let rn = conditions.get("ref_name").cloned().unwrap_or(json!({}));
    let includes = str_list(rn.get("include").unwrap_or(&Value::Null));
    let excludes = str_list(rn.get("exclude").unwrap_or(&Value::Null));
    let matches = |p: &str| match p {
        "~ALL" => true,
        "~DEFAULT_BRANCH" => full_ref == format!("refs/heads/{default_branch}"),
        p => glob_match(p, full_ref),
    };
    (includes.is_empty() || includes.iter().any(|p| matches(p)))
        && !excludes.iter().any(|p| matches(p))
}

fn ruleset_targets_repo(d: &Value, repo: &str) -> bool {
    let name = repo.split('/').nth(1).unwrap_or("");
    let Some(rn) = d.get("conditions").and_then(|c| c.get("repository_name")) else {
        return true;
    };
    let includes = str_list(rn.get("include").unwrap_or(&Value::Null));
    let excludes = str_list(rn.get("exclude").unwrap_or(&Value::Null));
    (includes.is_empty() || includes.iter().any(|p| p == "~ALL" || glob_match(p, name)))
        && !excludes.iter().any(|p| glob_match(p, name))
}

fn rules_for_branch(ctx: &Ctx, repo: &str, branch: &str) -> Response {
    let owner = repo.split('/').next().unwrap_or("");
    let default = default_branch(ctx, repo);
    let full = format!("refs/heads/{branch}");
    let mut out = Vec::new();
    let sets = rulesets_for(ctx, "repo", repo).into_iter().chain(
        rulesets_for(ctx, "org", owner)
            .into_iter()
            .filter(|d| ruleset_targets_repo(d, repo)),
    );
    for d in sets {
        if s(d, "enforcement") != "active" || opt_s(d, "target").is_some_and(|t| t != "branch") {
            continue;
        }
        if !ref_matches(d.get("conditions").unwrap_or(&json!({})), &full, &default) {
            continue;
        }
        for rule in arr(d, "rules") {
            let mut r = rule.clone();
            r["ruleset_source_type"] = json!(ruleset_source(s(d, "scope")));
            r["ruleset_source"] = json!(s(d, "source"));
            r["ruleset_id"] = json!(u(d, "id"));
            out.push(r);
        }
    }
    paged(ctx.req, out)
}

fn rule_suite_json(d: &Value, full: bool) -> Value {
    let mut v = json!({
        "id": u(d, "id"),
        "actor_id": u(d, "actor_id"),
        "actor_name": s(d, "actor_name"),
        "before_sha": s(d, "before_sha"),
        "after_sha": s(d, "after_sha"),
        "ref": s(d, "ref"),
        "repository_id": u(d, "repository_id"),
        "repository_name": s(d, "repository_name"),
        "pushed_at": d.get("pushed_at").cloned().unwrap_or(Value::Null),
        "result": opt_s(d, "result").unwrap_or_else(|| "pass".into()),
        "evaluation_result": opt_s(d, "evaluation_result").unwrap_or_else(|| "pass".into()),
    });
    if full {
        v["rule_evaluations"] = d.get("rule_evaluations").cloned().unwrap_or(json!([]));
    }
    v
}

fn list_rule_suites(ctx: &Ctx, kind: &str, source: &str) -> Response {
    let reference = ctx.req.q("ref");
    let actor = ctx.req.q("actor_name");
    let result = ctx.req.q("rule_suite_result").filter(|r| *r != "all");
    let repo_name = ctx.req.q("repository_name");
    let out: Vec<Value> = all(ctx.svc, "rule_suites")
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| match kind {
            "repo" => {
                s(d, "repository_name") == source.split('/').nth(1).unwrap_or("")
                    && s(d, "owner") == source.split('/').next().unwrap_or("")
            }
            _ => s(d, "owner") == source,
        })
        .filter(|d| {
            reference.is_none_or(|r| s(d, "ref") == r || s(d, "ref") == format!("refs/heads/{r}"))
        })
        .filter(|d| actor.is_none_or(|a| s(d, "actor_name") == a))
        .filter(|d| result.is_none_or(|r| s(d, "result") == r))
        .filter(|d| repo_name.is_none_or(|r| s(d, "repository_name") == r))
        .map(|d| rule_suite_json(d, false))
        .collect();
    paged(ctx.req, out)
}

fn get_rule_suite(ctx: &Ctx, kind: &str, source: &str, id: &str) -> Response {
    let found = all(ctx.svc, "rule_suites")
        .into_iter()
        .map(|(_, d)| d)
        .find(|d| {
            u(d, "id").to_string() == id
                && match kind {
                    "repo" => s(d, "repository_name") == source.split('/').nth(1).unwrap_or(""),
                    _ => s(d, "owner") == source,
                }
        });
    match found {
        Some(d) => ok(rule_suite_json(d, true)),
        None => not_found(),
    }
}

#[allow(dead_code)]
fn as_number(v: &Value) -> Option<i64> {
    as_i64(v)
}
