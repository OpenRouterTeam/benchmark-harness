//! Releases, tags, protected tags, snippets, webhooks and uploads.
//!
//! Collections: `releases` (`<project>@<tag>`: `tag_name, name, description, created_at,
//! released_at, author, milestones, links, evidences`), `tags` (`<project>:<tag>`),
//! `protected_tags` (`<project>:<name>`), `snippets` (id; `project` or null, `files`),
//! `hooks` (id; `project` or `group`), `hook_events` (id; `hook_id`), `uploads` (id).

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::{get as rget, post},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    let rel = "/projects/{id}/releases/{tag_name}";
    let mut router = Router::new()
        .route(
            "/projects/{id}/releases",
            rget(wrap(list_releases)).post(wrap(create_release)),
        )
        .route(
            rel,
            rget(wrap(get_release))
                .put(wrap(update_release))
                .delete(wrap(delete_release)),
        )
        .route(&format!("{rel}/evidence"), post(wrap(create_evidence)))
        .route(
            &format!("{rel}/downloads/{{*asset_path}}"),
            rget(wrap(download_asset)),
        )
        .route(
            &format!("{rel}/assets/links"),
            rget(wrap(list_links)).post(wrap(create_link)),
        )
        .route(
            "/projects/{id}/repository/tags",
            rget(wrap(list_tags)).post(wrap(create_tag)),
        )
        .route(
            "/projects/{id}/repository/tags/{tag_name}",
            rget(wrap(get_tag)).delete(wrap(delete_tag)),
        )
        .route(
            "/projects/{id}/repository/tags/{tag_name}/signature",
            rget(wrap(tag_signature)),
        )
        .route(
            "/projects/{id}/protected_tags",
            rget(wrap(list_protected_tags)).post(wrap(protect_tag)),
        )
        .route(
            "/projects/{id}/protected_tags/{tag_name}",
            rget(wrap(get_protected_tag)).delete(wrap(unprotect_tag)),
        )
        .route("/projects/{id}/uploads", post(wrap(upload)))
        .route(
            "/projects/{id}/uploads/{secret}/{filename}",
            rget(wrap(download_upload)),
        );
    for scope in ["projects", "groups"] {
        router = router
            .route(
                &format!("/{scope}/{{id}}/hooks"),
                rget(wrap(list_hooks)).post(wrap(create_hook)),
            )
            .route(
                &format!("/{scope}/{{id}}/hooks/{{hook_id}}"),
                rget(wrap(get_hook))
                    .put(wrap(update_hook))
                    .delete(wrap(delete_hook)),
            )
            .route(
                &format!("/{scope}/{{id}}/hooks/{{hook_id}}/events"),
                rget(wrap(hook_events)),
            );
    }
    for prefix in ["/snippets", "/projects/{id}/snippets"] {
        router = router
            .route(prefix, rget(wrap(list_snippets)).post(wrap(create_snippet)))
            .route(
                &format!("{prefix}/{{snippet_id}}"),
                rget(wrap(get_snippet))
                    .put(wrap(update_snippet))
                    .delete(wrap(delete_snippet)),
            )
            .route(
                &format!("{prefix}/{{snippet_id}}/raw"),
                rget(wrap(snippet_raw)),
            )
            .route(
                &format!("{prefix}/{{snippet_id}}/files/{{ref}}/{{file_path}}/raw"),
                rget(wrap(snippet_file_raw)),
            );
    }
    router.route("/snippets/public", rget(wrap(public_snippets)))
}

// ---------------------------------------------------------------------------
// Tags

fn tag_protected(st: &Store, project: &str, name: &str) -> bool {
    let prefix = format!("{project}:");
    all(st, "protected_tags").iter().any(|(k, _)| {
        k.strip_prefix(&prefix)
            .is_some_and(|pat| match pat.split_once('*') {
                None => pat == name,
                Some((a, b)) => name.starts_with(a) && name.ends_with(b),
            })
    })
}

fn render_tag(st: &Store, base: &str, project: &str, t: &Value) -> Value {
    let name = str_of(t, "name");
    let sha = str_of(t, "commit");
    let release = get(st, "releases", &format!("{project}@{name}"))
        .map(|r| json!({"tag_name": name, "description": r.get("description").cloned().unwrap_or(json!(""))}))
        .unwrap_or(Value::Null);
    json!({
        "name": name,
        "message": t.get("message").cloned().unwrap_or(Value::Null),
        "target": t.get("target").and_then(Value::as_str).unwrap_or(sha),
        "commit": crate::repository::render_commit(st, base, project, sha),
        "release": release,
        "protected": tag_protected(st, project, name),
        "created_at": t.get("created_at").cloned().unwrap_or(Value::Null),
    })
}

fn project_tags(st: &Store, project: &str) -> Vec<Value> {
    let prefix = format!("{project}:");
    all(st, "tags")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, t)| t.clone())
        .collect()
}

fn list_tags(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut tags: Vec<Value> = project_tags(st, &project)
        .into_iter()
        .filter(|t| {
            r.q("search")
                .is_none_or(|s| match (s.strip_prefix('^'), s.strip_suffix('$')) {
                    (Some(p), _) => str_of(t, "name").starts_with(p),
                    (_, Some(p)) => str_of(t, "name").ends_with(p),
                    _ => str_of(t, "name").contains(s),
                })
        })
        .collect();
    let date = |t: &Value| -> String {
        crate::repository::commit_doc(st, &project, str_of(t, "commit"))
            .map(|c| str_of(c, "committed_date").to_owned())
            .unwrap_or_else(|| str_of(t, "created_at").to_owned())
    };
    match r.q("order_by").unwrap_or("updated") {
        "name" => tags.sort_by(|a, b| str_of(a, "name").cmp(str_of(b, "name"))),
        "version" => tags.sort_by_key(|a| version_key(str_of(a, "name"))),
        _ => tags.sort_by_key(&date),
    }
    if r.q("sort") != Some("asc") {
        tags.reverse();
    }
    let items = tags
        .iter()
        .map(|t| render_tag(st, &r.base, &project, t))
        .collect();
    paged(r, items)
}

fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn get_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match get(st, "tags", &format!("{project}:{}", r.p("tag_name"))) {
        Some(t) => ok(render_tag(st, &r.base, &project, t)),
        None => not_found("Tag"),
    }
}

fn insert_tag(
    st: &mut Store,
    project: &str,
    name: &str,
    sha: &str,
    message: Option<String>,
    now: &str,
) -> Value {
    let doc = json!({
        "name": name,
        "commit": sha,
        "message": message,
        "target": if message.as_deref().is_some_and(|m| !m.is_empty()) { fake_sha(&format!("tag:{project}:{name}")) } else { sha.to_owned() },
        "created_at": now,
    });
    st.collection_mut("tags")
        .insert(format!("{project}:{name}"), doc.clone());
    doc
}

fn create_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("tag_name") else {
        return missing("tag_name");
    };
    let Some(reference) = r.s("ref") else {
        return missing("ref");
    };
    if get(st, "tags", &format!("{project}:{name}")).is_some() {
        return bad_request(&format!("Tag {name} already exists"));
    }
    if name.contains(' ') || name.contains("..") || name.starts_with('-') {
        return bad_request("Tag name invalid");
    }
    let Some(snap) = crate::repository::resolve_ref(st, &project, &reference) else {
        return bad_request("Target ref is invalid");
    };
    let now = now(w);
    let st = svc_mut(w);
    let doc = insert_tag(st, &project, &name, &snap.sha, r.s("message"), &now);
    created(render_tag(st, &r.base, &project, &doc))
}

fn delete_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("tag_name").to_owned();
    let key = format!("{project}:{name}");
    if get(st, "tags", &key).is_none() {
        return not_found("Tag");
    }
    if tag_protected(st, &project, &name) {
        return forbidden();
    }
    remove_doc(svc_mut(w), "tags", &key);
    no_content()
}

fn tag_signature(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    match get(st, "tags", &format!("{project}:{}", r.p("tag_name"))) {
        Some(t) => match t.get("signature") {
            Some(sig) => ok(sig.clone()),
            None => not_found("Signature"),
        },
        None => not_found("Tag"),
    }
}

// ---------------------------------------------------------------------------
// Protected tags

fn render_protected_tag(p: &Value, name: &str) -> Value {
    let level = as_u64(&p["create_access_level"]).unwrap_or(40);
    let desc = match level {
        0 => "No one",
        30 => "Developers + Maintainers",
        _ => "Maintainers",
    };
    json!({
        "name": name,
        "create_access_levels": [{
            "id": as_u64(&p["id"]).unwrap_or(0) * 10 + 1,
            "access_level": level,
            "access_level_description": desc,
            "deploy_key_id": null,
            "user_id": null,
            "group_id": null,
        }],
    })
}

fn list_protected_tags(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let prefix = format!("{project}:");
    let items = all(st, "protected_tags")
        .into_iter()
        .filter_map(|(k, p)| k.strip_prefix(&prefix).map(|n| render_protected_tag(p, n)))
        .collect();
    paged(r, items)
}

fn get_protected_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("tag_name");
    match get(st, "protected_tags", &format!("{project}:{name}")) {
        Some(p) => ok(render_protected_tag(p, name)),
        None => not_found("Protected Tag"),
    }
}

fn protect_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let key = format!("{project}:{name}");
    if get(st, "protected_tags", &key).is_some() {
        return error(
            StatusCode::CONFLICT,
            "Protected tag 'name' has already been taken",
        );
    }
    let st = svc_mut(w);
    let id = next_id(st, "protected_tags", 1);
    let doc = json!({"id": id, "name": name, "create_access_level": r.int("create_access_level").unwrap_or(40)});
    st.collection_mut("protected_tags").insert(key, doc.clone());
    created(render_protected_tag(&doc, &name))
}

fn unprotect_tag(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let key = format!("{project}:{}", r.p("tag_name"));
    if get(st, "protected_tags", &key).is_none() {
        return not_found("Protected Tag");
    }
    remove_doc(svc_mut(w), "protected_tags", &key);
    no_content()
}

// ---------------------------------------------------------------------------
// Releases

const ARCHIVE_FORMATS: [&str; 4] = ["zip", "tar.gz", "tar.bz2", "tar"];

fn render_release(st: &Store, base: &str, project: &str, rel: &Value, html: bool) -> Value {
    let tag = str_of(rel, "tag_name");
    let sha = get(st, "tags", &format!("{project}:{tag}"))
        .map(|t| str_of(t, "commit").to_owned())
        .unwrap_or_default();
    let author = rel
        .get("author")
        .and_then(Value::as_str)
        .map(|u| user_basic(st, base, u));
    let milestones: Vec<Value> = value_list(&rel["milestones"])
        .iter()
        .filter_map(|title| {
            all(st, "milestones")
                .into_iter()
                .find(|(k, m)| {
                    str_of(m, "title") == title
                        && (project_of(k) == project
                            || project.starts_with(&format!("{}/", project_of(k))))
                })
                .map(|(k, _)| crate::issues::render_milestone(st, base, k))
        })
        .collect();
    let links: Vec<Value> = rel
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|l| {
            let direct = l.get("direct_asset_path").and_then(Value::as_str);
            json!({
                "id": l["id"],
                "name": str_of(l, "name"),
                "url": str_of(l, "url"),
                "direct_asset_url": direct.map(|d| format!("{base}/{project}/-/releases/{tag}/downloads{d}")).unwrap_or_else(|| str_of(l, "url").to_owned()),
                "direct_asset_path": direct,
                "link_type": l.get("link_type").cloned().unwrap_or(json!("other")),
            })
        })
        .collect();
    let mut out = json!({
        "tag_name": tag,
        "name": rel.get("name").cloned().unwrap_or_else(|| json!(tag)),
        "description": rel.get("description").cloned().unwrap_or(json!("")),
        "created_at": rel["created_at"],
        "released_at": rel.get("released_at").cloned().unwrap_or_else(|| rel["created_at"].clone()),
        "upcoming_release": rel.get("released_at").and_then(Value::as_str).is_some_and(|d| d > "2026-12-31"),
        "milestones": milestones,
        "commit_path": format!("/{project}/-/commit/{sha}"),
        "tag_path": format!("/{project}/-/tags/{tag}"),
        "assets": {
            "count": 4 + links.len(),
            "sources": ARCHIVE_FORMATS.iter().map(|f| json!({"format": f, "url": format!("{base}/{project}/-/archive/{tag}/{}-{tag}.{f}", last_segment(project))})).collect::<Vec<_>>(),
            "links": links,
        },
        "evidences": rel.get("evidences").cloned().unwrap_or(json!([])),
        "_links": {
            "closed_issues_url": format!("{base}/{project}/-/issues?release_tag={tag}&scope=all&state=closed"),
            "closed_merge_requests_url": format!("{base}/{project}/-/merge_requests?release_tag={tag}&scope=all&state=closed"),
            "edit_url": format!("{base}/{project}/-/releases/{tag}/edit"),
            "merged_merge_requests_url": format!("{base}/{project}/-/merge_requests?release_tag={tag}&scope=all&state=merged"),
            "opened_issues_url": format!("{base}/{project}/-/issues?release_tag={tag}&scope=all&state=opened"),
            "opened_merge_requests_url": format!("{base}/{project}/-/merge_requests?release_tag={tag}&scope=all&state=opened"),
            "self": format!("{base}/{project}/-/releases/{tag}"),
        },
    });
    if !sha.is_empty() {
        out["commit"] = crate::repository::render_commit(st, base, project, &sha);
    }
    if let Some(a) = author {
        out["author"] = a;
    }
    if html {
        out["description_html"] = json!(format!(
            "<p dir=\"auto\">{}</p>",
            str_of(rel, "description")
        ));
    }
    out
}

fn list_releases(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let prefix = format!("{project}@");
    let mut rels: Vec<Value> = all(st, "releases")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| v.clone())
        .collect();
    let field = if r.q("order_by") == Some("created_at") {
        "created_at"
    } else {
        "released_at"
    };
    rels.sort_by(|a, b| {
        let fa = a
            .get(field)
            .or(a.get("created_at"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let fb = b
            .get(field)
            .or(b.get("created_at"))
            .and_then(Value::as_str)
            .unwrap_or("");
        fb.cmp(fa)
    });
    if r.q("sort") == Some("asc") {
        rels.reverse();
    }
    let html = r.q("include_html_description") == Some("true");
    let items = rels
        .iter()
        .map(|x| render_release(st, &r.base, &project, x, html))
        .collect();
    paged(r, items)
}

fn require_release(st: &Store, r: &Req) -> Result<(String, String), Response> {
    let project = require_project(st, r)?;
    let key = format!("{project}@{}", r.p("tag_name"));
    if get(st, "releases", &key).is_some() {
        Ok((project, key))
    } else {
        Err(error(axum::http::StatusCode::NOT_FOUND, "404 Not found"))
    }
}

fn get_release(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_release(st, r) {
        Ok((p, k)) => ok(render_release(
            st,
            &r.base,
            &p,
            get(st, "releases", &k).expect("release"),
            r.q("include_html_description") == Some("true"),
        )),
        Err(e) => e,
    }
}

fn milestone_titles(st: &Store, project: &str, r: &Req) -> Result<Option<Vec<String>>, Response> {
    let Some(titles) = r.list("milestones") else {
        return Ok(None);
    };
    for t in &titles {
        let exists = all(st, "milestones").iter().any(|(k, m)| {
            str_of(m, "title") == t
                && (project_of(k) == project || project.starts_with(&format!("{}/", project_of(k))))
        });
        if !exists {
            return Err(bad_request(&format!("Milestone(s) not found: {t}")));
        }
    }
    Ok(Some(titles))
}

fn create_release(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(tag) = r.s("tag_name") else {
        return missing("tag_name");
    };
    let key = format!("{project}@{tag}");
    if get(st, "releases", &key).is_some() {
        return error(StatusCode::CONFLICT, "Release already exists");
    }
    let milestones = match milestone_titles(st, &project, r) {
        Ok(m) => m.unwrap_or_default(),
        Err(e) => return e,
    };
    let tag_exists = get(st, "tags", &format!("{project}:{tag}")).is_some();
    let snap = if tag_exists {
        None
    } else {
        let Some(reference) = r.s("ref") else {
            return error(StatusCode::UNPROCESSABLE_ENTITY, "Ref is not specified");
        };
        match crate::repository::resolve_ref(st, &project, &reference) {
            Some(s) => Some(s),
            None => return error(StatusCode::UNPROCESSABLE_ENTITY, "Ref is invalid"),
        }
    };
    let now = now(w);
    let st = svc_mut(w);
    if let Some(s) = snap {
        insert_tag(st, &project, &tag, &s.sha, r.s("tag_message"), &now);
    }
    let mut links = Vec::new();
    if let Some(ls) = r
        .body
        .get("assets")
        .and_then(|a| a.get("links"))
        .and_then(Value::as_array)
    {
        for (i, l) in ls.iter().enumerate() {
            let mut l = l.clone();
            l["id"] = json!(next_id(st, "releases", 1) * 100 + i as u64);
            links.push(l);
        }
    }
    let author = current_username(st);
    let doc = json!({
        "tag_name": tag,
        "name": r.s("name").unwrap_or_else(|| tag.clone()),
        "description": r.s("description").unwrap_or_default(),
        "created_at": now,
        "released_at": r.s("released_at").unwrap_or_else(|| now.clone()),
        "author": author,
        "milestones": milestones,
        "links": links,
        "evidences": [],
    });
    st.collection_mut("releases").insert(key, doc.clone());
    created(render_release(st, &r.base, &project, &doc, false))
}

fn update_release(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (project, key) = match require_release(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let milestones = match milestone_titles(st, &project, r) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let doc = st
        .collection_mut("releases")
        .get_mut(&key)
        .expect("release");
    for f in ["name", "description", "released_at"] {
        if let Some(v) = r.s(f) {
            doc[f] = json!(v);
        }
    }
    if let Some(m) = milestones {
        doc["milestones"] = json!(m);
    }
    let doc = doc.clone();
    ok(render_release(st, &r.base, &project, &doc, false))
}

fn delete_release(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (project, key) = match require_release(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let out = render_release(
        st,
        &r.base,
        &project,
        get(st, "releases", &key).expect("release"),
        false,
    );
    remove_doc(svc_mut(w), "releases", &key);
    ok(out)
}

fn create_evidence(w: &mut WorldState, r: &Req) -> Response {
    let (project, key) = match require_release(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let doc = st
        .collection_mut("releases")
        .get_mut(&key)
        .expect("release");
    let mut ev = doc
        .get("evidences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let sha = fake_sha(&format!("evidence:{key}:{}", ev.len()));
    ev.push(json!({
        "sha": sha,
        "filepath": format!("{}/{project}/-/releases/{}/evidences/{}.json", r.base, str_of(doc, "tag_name"), ev.len() + 1),
        "collected_at": now,
    }));
    doc["evidences"] = Value::Array(ev);
    accepted(json!({"message": "202 Accepted"}))
}

fn download_asset(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (_, key) = match require_release(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = format!("/{}", r.p("asset_path").trim_start_matches('/'));
    let rel = get(st, "releases", &key).cloned().unwrap_or(json!({}));
    let link = rel.get("links").and_then(Value::as_array).and_then(|ls| {
        ls.iter()
            .find(|l| str_of(l, "direct_asset_path") == path)
            .cloned()
    });
    match link {
        Some(l) => text(
            StatusCode::OK,
            "application/octet-stream",
            l.get("content")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{} ({})\n", str_of(&l, "name"), str_of(&l, "url"))),
        ),
        None => error(axum::http::StatusCode::NOT_FOUND, "404 Not found"),
    }
}

fn list_links(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (project, key) = match require_release(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let rel = render_release(
        st,
        &r.base,
        &project,
        get(st, "releases", &key).expect("release"),
        false,
    );
    paged(
        r,
        rel["assets"]["links"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    )
}

fn create_link(w: &mut WorldState, r: &Req) -> Response {
    let (project, key) = match require_release(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let Some(url) = r.s("url") else {
        return missing("url");
    };
    let st = svc_mut(w);
    let id = all(st, "releases").len() as u64 * 100 + 50;
    let doc = st
        .collection_mut("releases")
        .get_mut(&key)
        .expect("release");
    let mut links = doc
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let id = id + links.len() as u64;
    links.push(json!({"id": id, "name": name, "url": url, "direct_asset_path": r.s("direct_asset_path"), "link_type": r.s("link_type").unwrap_or_else(|| "other".into())}));
    doc["links"] = Value::Array(links);
    let doc = doc.clone();
    let rel = render_release(st, &r.base, &project, &doc, false);
    created(
        rel["assets"]["links"]
            .as_array()
            .and_then(|l| l.last().cloned())
            .unwrap_or(Value::Null),
    )
}

// ---------------------------------------------------------------------------
// Snippets

fn snippet_scope(st: &Store, r: &Req) -> Result<Option<String>, Response> {
    if r.path.contains_key("id") {
        require_project(st, r).map(Some)
    } else {
        Ok(None)
    }
}

fn snippet_files(s: &Value) -> Vec<(String, String)> {
    match s.get("files").and_then(Value::as_array) {
        Some(files) => files
            .iter()
            .map(|f| {
                (
                    str_of(f, "file_path").to_owned(),
                    str_of(f, "content").to_owned(),
                )
            })
            .collect(),
        None => vec![(
            str_of(s, "file_name").to_owned(),
            str_of(s, "content").to_owned(),
        )],
    }
}

fn render_snippet(st: &Store, base: &str, s: &Value) -> Value {
    let id = &s["id"];
    let web = match s.get("project").and_then(Value::as_str) {
        Some(p) => format!("{base}/{p}/-/snippets/{id}"),
        None => format!("{base}/-/snippets/{id}"),
    };
    let files = snippet_files(s);
    let host = base
        .split("://")
        .nth(1)
        .unwrap_or("gitlab.local")
        .split(':')
        .next()
        .unwrap_or("gitlab.local")
        .to_owned();
    json!({
        "id": id,
        "title": str_of(s, "title"),
        "description": s.get("description").cloned().unwrap_or(Value::Null),
        "visibility": s.get("visibility").cloned().unwrap_or(json!("private")),
        "author": user_basic(st, base, str_of(s, "author")),
        "created_at": s["created_at"],
        "updated_at": s.get("updated_at").unwrap_or(&s["created_at"]),
        "project_id": s.get("project").and_then(Value::as_str).map(|p| json!(project_id(st, p))).unwrap_or(Value::Null),
        "web_url": web,
        "raw_url": format!("{web}/raw"),
        "file_name": files.first().map(|(p, _)| p.clone()),
        "files": files.iter().map(|(p, _)| json!({"path": p, "raw_url": format!("{web}/raw/main/{p}")})).collect::<Vec<_>>(),
        "imported": false,
        "imported_from": "none",
        "ssh_url_to_repo": format!("git@{host}:snippets/{id}.git"),
        "http_url_to_repo": format!("{base}/snippets/{id}.git"),
    })
}

fn snippets_in(st: &Store, scope: &Option<String>) -> Vec<Value> {
    let me = current_username(st);
    all(st, "snippets")
        .into_iter()
        .filter(|(_, s)| match scope {
            Some(p) => str_of(s, "project") == p,
            None => s.get("project").is_none_or(Value::is_null) && str_of(s, "author") == me,
        })
        .map(|(_, s)| s.clone())
        .collect()
}

fn list_snippets(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match snippet_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let items = snippets_in(st, &scope)
        .iter()
        .map(|s| render_snippet(st, &r.base, s))
        .collect();
    paged(r, items)
}

fn public_snippets(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let items = all(st, "snippets")
        .into_iter()
        .filter(|(_, s)| str_of(s, "visibility") == "public")
        .map(|(_, s)| render_snippet(st, &r.base, s))
        .collect();
    paged(r, items)
}

fn require_snippet(st: &Store, r: &Req) -> Result<String, Response> {
    let scope = snippet_scope(st, r)?;
    let id = r.p("snippet_id").to_owned();
    match get(st, "snippets", &id) {
        Some(s) if scope.as_deref().is_none_or(|p| str_of(s, "project") == p) => Ok(id),
        _ => Err(not_found("Snippet")),
    }
}

fn get_snippet(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_snippet(st, r) {
        Ok(id) => ok(render_snippet(
            st,
            &r.base,
            get(st, "snippets", &id).expect("snippet"),
        )),
        Err(e) => e,
    }
}

fn snippet_raw(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_snippet(st, r) {
        Ok(id) => {
            let files = snippet_files(get(st, "snippets", &id).expect("snippet"));
            text(
                StatusCode::OK,
                "text/plain; charset=utf-8",
                files.first().map(|(_, c)| c.clone()).unwrap_or_default(),
            )
        }
        Err(e) => e,
    }
}

fn snippet_file_raw(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_snippet(st, r) {
        Ok(id) => {
            let files = snippet_files(get(st, "snippets", &id).expect("snippet"));
            match files.iter().find(|(p, _)| p == r.p("file_path")) {
                Some((_, c)) => text(StatusCode::OK, "text/plain; charset=utf-8", c.clone()),
                None => not_found("File"),
            }
        }
        Err(e) => e,
    }
}

fn create_snippet(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match snippet_scope(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(title) = r.s("title") else {
        return missing("title");
    };
    let files: Vec<Value> = match r.body.get("files").and_then(Value::as_array) {
        Some(f) if !f.is_empty() => f
            .iter()
            .map(|x| json!({"file_path": str_of(x, "file_path"), "content": str_of(x, "content")}))
            .collect(),
        _ => match (r.s("file_name"), r.s("content")) {
            (Some(n), Some(c)) => vec![json!({"file_path": n, "content": c})],
            _ => return missing("files"),
        },
    };
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "snippets", 1);
    let doc = json!({
        "id": id,
        "project": scope,
        "title": title,
        "description": r.s("description"),
        "visibility": r.s("visibility").unwrap_or_else(|| "private".into()),
        "author": current_username(st),
        "created_at": now,
        "updated_at": now,
        "files": files,
    });
    st.collection_mut("snippets")
        .insert(id.to_string(), doc.clone());
    created(render_snippet(st, &r.base, &doc))
}

fn update_snippet(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_snippet(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let now = now(w);
    let st = svc_mut(w);
    let doc = st.collection_mut("snippets").get_mut(&id).expect("snippet");
    for f in ["title", "description", "visibility"] {
        if let Some(v) = r.s(f) {
            doc[f] = json!(v);
        }
    }
    let mut files: Vec<Value> = snippet_files(doc)
        .into_iter()
        .map(|(p, c)| json!({"file_path": p, "content": c}))
        .collect();
    if let Some(changes) = r.body.get("files").and_then(Value::as_array) {
        for ch in changes {
            let path = str_of(ch, "file_path").to_owned();
            let prev = ch
                .get("previous_path")
                .and_then(Value::as_str)
                .unwrap_or(&path)
                .to_owned();
            match ch.get("action").and_then(Value::as_str).unwrap_or("update") {
                "create" => {
                    files.push(json!({"file_path": path, "content": str_of(ch, "content")}))
                }
                "delete" => files.retain(|f| str_of(f, "file_path") != path),
                "move" => {
                    if let Some(f) = files.iter_mut().find(|f| str_of(f, "file_path") == prev) {
                        f["file_path"] = json!(path);
                        if let Some(c) = ch.get("content") {
                            f["content"] = c.clone();
                        }
                    }
                }
                _ => {
                    if let Some(f) = files.iter_mut().find(|f| str_of(f, "file_path") == path) {
                        f["content"] = ch.get("content").cloned().unwrap_or(json!(""));
                    }
                }
            }
        }
    }
    doc["files"] = Value::Array(files);
    doc["updated_at"] = json!(now);
    let doc = doc.clone();
    ok(render_snippet(st, &r.base, &doc))
}

fn delete_snippet(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_snippet(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "snippets", &id);
    no_content()
}

// ---------------------------------------------------------------------------
// Webhooks

const HOOK_EVENTS: &[(&str, bool)] = &[
    ("push_events", true),
    ("issues_events", false),
    ("confidential_issues_events", false),
    ("merge_requests_events", false),
    ("tag_push_events", false),
    ("note_events", false),
    ("confidential_note_events", false),
    ("job_events", false),
    ("pipeline_events", false),
    ("wiki_page_events", false),
    ("deployment_events", false),
    ("feature_flag_events", false),
    ("releases_events", false),
    ("milestone_events", false),
    ("emoji_events", false),
    ("resource_access_token_events", false),
    ("member_events", false),
    ("project_events", false),
    ("subgroup_events", false),
];

fn hook_scope(st: &Store, r: &Req) -> Result<(bool, String), Response> {
    if r.uri_path.contains("/groups/") {
        require_group(st, r).map(|g| (true, g))
    } else {
        require_project(st, r).map(|p| (false, p))
    }
}

fn render_hook(st: &Store, h: &Value) -> Value {
    let mut out = json!({
        "id": h["id"],
        "url": str_of(h, "url"),
        "name": h.get("name").cloned().unwrap_or(json!("")),
        "description": h.get("description").cloned().unwrap_or(json!("")),
        "created_at": h["created_at"],
        "enable_ssl_verification": h.get("enable_ssl_verification").cloned().unwrap_or(json!(true)),
        "push_events_branch_filter": h.get("push_events_branch_filter").cloned().unwrap_or(Value::Null),
        "branch_filter_strategy": h.get("branch_filter_strategy").cloned().unwrap_or(json!("wildcard")),
        "alert_status": h.get("alert_status").cloned().unwrap_or(json!("executable")),
        "disabled_until": null,
        "url_variables": [],
        "custom_webhook_template": h.get("custom_webhook_template").cloned().unwrap_or(Value::Null),
        "custom_headers": h.get("custom_headers").and_then(Value::as_array).map(|hs| hs.iter().map(|x| json!({"key": x["key"]})).collect::<Vec<_>>()).unwrap_or_default(),
    });
    for (ev, default) in HOOK_EVENTS {
        out[*ev] = h.get(*ev).cloned().unwrap_or(json!(default));
    }
    if let Some(g) = h.get("group").and_then(Value::as_str) {
        out["group_id"] = json!(group_id(st, g));
    } else {
        out["project_id"] = json!(project_id(st, str_of(h, "project")));
    }
    out
}

fn hooks_of(st: &Store, group: bool, scope: &str) -> Vec<Value> {
    all(st, "hooks")
        .into_iter()
        .filter(|(_, h)| {
            if group {
                str_of(h, "group") == scope
            } else {
                str_of(h, "project") == scope
            }
        })
        .map(|(_, h)| h.clone())
        .collect()
}

fn list_hooks(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (group, scope) = match hook_scope(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let items = hooks_of(st, group, &scope)
        .iter()
        .map(|h| render_hook(st, h))
        .collect();
    paged(r, items)
}

fn require_hook(st: &Store, r: &Req) -> Result<String, Response> {
    let (group, scope) = hook_scope(st, r)?;
    let id = r.p("hook_id").to_owned();
    if hooks_of(st, group, &scope)
        .iter()
        .any(|h| id_is(&h["id"], &id))
    {
        Ok(id)
    } else {
        Err(not_found("Hook"))
    }
}

fn get_hook(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    match require_hook(st, r) {
        Ok(id) => ok(render_hook(st, get(st, "hooks", &id).expect("hook"))),
        Err(e) => e,
    }
}

fn apply_hook_fields(doc: &mut Value, r: &Req) {
    for f in [
        "url",
        "name",
        "description",
        "token",
        "push_events_branch_filter",
        "branch_filter_strategy",
        "custom_webhook_template",
    ] {
        if let Some(v) = r.s(f) {
            doc[f] = json!(v);
        }
    }
    for (ev, _) in HOOK_EVENTS {
        if let Some(b) = r.bool(ev) {
            doc[*ev] = json!(b);
        }
    }
    if let Some(b) = r.bool("enable_ssl_verification") {
        doc["enable_ssl_verification"] = json!(b);
    }
    if let Some(h) = r.body.get("custom_headers") {
        doc["custom_headers"] = h.clone();
    }
}

fn create_hook(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let (group, scope) = match hook_scope(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(url) = r.s("url") else {
        return missing("url");
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return invalid("url", "is blocked: Only allowed schemes are http, https");
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "hooks", 1);
    let mut doc = json!({"id": id, "created_at": now, "url": url});
    if group {
        doc["group"] = json!(scope);
    } else {
        doc["project"] = json!(scope);
    }
    apply_hook_fields(&mut doc, r);
    st.collection_mut("hooks")
        .insert(id.to_string(), doc.clone());
    created(render_hook(st, &doc))
}

fn update_hook(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_hook(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let st = svc_mut(w);
    let doc = st.collection_mut("hooks").get_mut(&id).expect("hook");
    apply_hook_fields(doc, r);
    let doc = doc.clone();
    ok(render_hook(st, &doc))
}

fn delete_hook(w: &mut WorldState, r: &Req) -> Response {
    let id = match require_hook(store(w), r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    remove_doc(svc_mut(w), "hooks", &id);
    no_content()
}

fn hook_events(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let id = match require_hook(st, r) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let status = r.q("status");
    let mut items: Vec<Value> = all(st, "hook_events")
        .into_iter()
        .filter(|(_, e)| id_is(&e["hook_id"], &id))
        .filter(|(_, e)| {
            status.is_none_or(|s| {
                let code = as_u64(&e["response_status"]).unwrap_or(0);
                match s {
                    "successful" => (200..300).contains(&code),
                    "client_failure" => (400..500).contains(&code),
                    "server_failure" => code >= 500,
                    other => other.parse::<u64>().ok() == Some(code),
                }
            })
        })
        .map(|(_, e)| {
            json!({
                "id": e["id"],
                "url": e.get("url").cloned().unwrap_or(json!("")),
                "trigger": e.get("trigger").cloned().unwrap_or(json!("push_hooks")),
                "request_headers": e.get("request_headers").cloned().unwrap_or(json!({"Content-Type": "application/json", "User-Agent": "GitLab/17.11.2-ee"})),
                "request_data": e.get("request_data").cloned().unwrap_or(json!({})),
                "response_headers": e.get("response_headers").cloned().unwrap_or(json!({})),
                "response_body": e.get("response_body").cloned().unwrap_or(json!("")),
                "execution_duration": e.get("execution_duration").cloned().unwrap_or(json!(0.12)),
                "response_status": e.get("response_status").cloned().unwrap_or(json!("200")),
            })
        })
        .collect();
    items.sort_by(|a, b| as_u64(&b["id"]).cmp(&as_u64(&a["id"])));
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Uploads

/// Extracts `(filename, bytes)` from a single-file multipart body.
fn multipart_file(r: &Req) -> Option<(String, Vec<u8>)> {
    let ct = r.headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let boundary = ct.split("boundary=").nth(1)?.trim_matches('"').to_owned();
    let body = &r.raw_body[..];
    let marker = format!("--{boundary}");
    let text = String::from_utf8_lossy(body);
    let part = text.split(&marker).find(|p| p.contains("filename="))?;
    let filename = part
        .split("filename=\"")
        .nth(1)?
        .split('"')
        .next()?
        .to_owned();
    let start_rel = part.find("\r\n\r\n")? + 4;
    // Locate the same bytes in the raw body to keep binary content intact.
    let part_start = text.find(part)?;
    let start = part_start + start_rel;
    let end = body[start..]
        .windows(marker.len() + 2)
        .position(|w| w == format!("\r\n{marker}").as_bytes())
        .map(|p| start + p)
        .unwrap_or(body.len());
    Some((filename, body.get(start..end)?.to_vec()))
}

fn image_ext(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".ico",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

fn upload(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some((filename, bytes)) = multipart_file(r) else {
        return missing("file");
    };
    let pid = project_id(st, &project);
    let st = svc_mut(w);
    let id = next_id(st, "uploads", 1);
    let secret = fake_sha(&format!("upload:{project}:{id}"))[..32].to_owned();
    st.collection_mut("uploads").insert(
        id.to_string(),
        json!({"id": id, "project": project, "secret": secret, "filename": filename, "content_base64": b64_encode(&bytes), "size": bytes.len()}),
    );
    let url = format!("/uploads/{secret}/{filename}");
    let alt = filename
        .rsplit_once('.')
        .map_or(filename.as_str(), |(b, _)| b)
        .to_owned();
    let markdown = if image_ext(&filename) {
        format!("![{alt}]({url})")
    } else {
        format!("[{filename}]({url})")
    };
    created(json!({
        "id": id,
        "alt": alt,
        "url": url,
        "full_path": format!("/-/project/{pid}{url}"),
        "markdown": markdown,
    }))
}

fn download_upload(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let found = all(st, "uploads").into_iter().find(|(_, u)| {
        str_of(u, "project") == project
            && str_of(u, "secret") == r.p("secret")
            && str_of(u, "filename") == r.p("filename")
    });
    let Some((_, u)) = found else {
        return error(axum::http::StatusCode::NOT_FOUND, "404 Not found");
    };
    let bytes = b64_decode(str_of(u, "content_base64")).unwrap_or_default();
    let name = str_of(u, "filename").to_lowercase();
    let ct = if name.ends_with(".png") {
        "image/png"
    } else if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        "image/jpeg"
    } else if name.ends_with(".gif") {
        "image/gif"
    } else if name.ends_with(".txt") || name.ends_with(".log") {
        "text/plain"
    } else {
        "application/octet-stream"
    };
    let mut resp = Response::new(axum::body::Body::from(bytes));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    resp
}
