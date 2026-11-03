//! Project and group wikis. Collection `wikis`, key `<project-or-group path>/-/<slug>`:
//! `title, slug, format, content, created_at, updated_at`.

use axum::{Router, response::Response, routing::get as rget};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    let mut router = Router::new();
    for scope in ["projects", "groups"] {
        router = router
            .route(
                &format!("/{scope}/{{id}}/wikis"),
                rget(wrap(list)).post(wrap(create)),
            )
            .route(
                &format!("/{scope}/{{id}}/wikis/{{slug}}"),
                rget(wrap(show)).put(wrap(update)).delete(wrap(remove)),
            );
    }
    router
}

fn scope_of(st: &Store, r: &Req) -> Result<String, Response> {
    if r.uri_path.contains("/groups/") {
        require_group(st, r)
    } else {
        require_project(st, r)
    }
}

pub(crate) fn slugify(title: &str) -> String {
    title
        .trim()
        .split('/')
        .map(|seg| seg.trim().replace(' ', "-"))
        .collect::<Vec<_>>()
        .join("/")
}

fn pages(st: &Store, scope: &str) -> Vec<(String, Value)> {
    let prefix = format!("{scope}/-/");
    all(st, "wikis")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, v)| (k.to_owned(), v.clone()))
        .collect()
}

fn render(p: &Value, with_content: bool, html: bool) -> Value {
    let mut out = json!({
        "format": p.get("format").cloned().unwrap_or(json!("markdown")),
        "slug": str_of(p, "slug"),
        "title": str_of(p, "title"),
        "encoding": "UTF-8",
    });
    if with_content {
        let content = str_of(p, "content");
        out["content"] = if html {
            json!(
                content
                    .split("\n\n")
                    .map(|para| format!("<p data-sourcepos=\"1:1-1:1\" dir=\"auto\">{para}</p>"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            json!(content)
        };
        out["front_matter"] = json!({});
    }
    out
}

fn list(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match scope_of(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let with_content = r.q("with_content") == Some("true") || r.q("with_content") == Some("1");
    let html = r.q("render_html") == Some("true");
    let items = pages(st, &scope)
        .iter()
        .map(|(_, p)| render(p, with_content, html))
        .collect();
    paged(r, items)
}

fn find(st: &Store, scope: &str, slug: &str) -> Option<(String, Value)> {
    pages(st, scope)
        .into_iter()
        .find(|(_, p)| str_of(p, "slug") == slug)
}

fn show(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match scope_of(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    match find(st, &scope, r.p("slug")) {
        Some((_, p)) => ok(render(&p, true, r.q("render_html") == Some("true"))),
        None => not_found("Wiki Page"),
    }
}

fn valid_format(f: &str) -> bool {
    ["markdown", "rdoc", "asciidoc", "org"].contains(&f)
}

fn create(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match scope_of(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(title) = r.s("title").filter(|t| !t.trim().is_empty()) else {
        return missing("title");
    };
    let Some(content) = r.s("content") else {
        return missing("content");
    };
    let format = r.s("format").unwrap_or_else(|| "markdown".into());
    if !valid_format(&format) {
        return bad_request("format does not have a valid value");
    }
    let slug = slugify(&title);
    if find(st, &scope, &slug).is_some() {
        return invalid(
            "base",
            "Duplicate page: A page with that title already exists",
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let doc = json!({
        "title": last_segment(&title).trim(),
        "slug": slug,
        "format": format,
        "content": content,
        "created_at": now,
        "updated_at": now,
    });
    st.collection_mut("wikis")
        .insert(format!("{scope}/-/{slug}"), doc.clone());
    created(render(&doc, true, false))
}

fn update(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match scope_of(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some((key, mut doc)) = find(st, &scope, r.p("slug")) else {
        return not_found("Wiki Page");
    };
    if !r.has("title") && !r.has("content") && !r.has("format") {
        return bad_request(
            "content, title, format are missing, at least one parameter must be provided",
        );
    }
    if let Some(f) = r.s("format") {
        if !valid_format(&f) {
            return bad_request("format does not have a valid value");
        }
        doc["format"] = json!(f);
    }
    if let Some(c) = r.s("content") {
        doc["content"] = json!(c);
    }
    let now = now(w);
    doc["updated_at"] = json!(now);
    let st = svc_mut(w);
    let mut new_key = key.clone();
    if let Some(t) = r.s("title").filter(|t| !t.trim().is_empty()) {
        let slug = slugify(&t);
        doc["title"] = json!(last_segment(&t).trim());
        doc["slug"] = json!(slug);
        new_key = format!("{scope}/-/{slug}");
    }
    remove_doc(st, "wikis", &key);
    st.collection_mut("wikis").insert(new_key, doc.clone());
    ok(render(&doc, true, false))
}

fn remove(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let scope = match scope_of(st, r) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some((key, _)) = find(st, &scope, r.p("slug")) else {
        return not_found("Wiki Page");
    };
    remove_doc(svc_mut(w), "wikis", &key);
    no_content()
}

/// `/search?scope=wiki_blobs` results.
pub(crate) fn search_wikis(
    st: &Store,
    projects: &[String],
    group: Option<&str>,
    term: &str,
) -> Vec<Value> {
    let needle = term.to_lowercase();
    let mut scopes: Vec<String> = projects.to_vec();
    if let Some(g) = group {
        scopes.push(g.to_owned());
    }
    let mut out = Vec::new();
    for scope in scopes {
        for (_, p) in pages(st, &scope) {
            let content = str_of(&p, "content");
            if content.to_lowercase().contains(&needle)
                || str_of(&p, "title").to_lowercase().contains(&needle)
            {
                let slug = str_of(&p, "slug");
                out.push(json!({
                    "basename": slug,
                    "data": content.lines().take(3).map(|l| format!("{l}\n")).collect::<String>(),
                    "path": format!("{slug}.md"),
                    "filename": format!("{slug}.md"),
                    "id": null,
                    "ref": "main",
                    "startline": 1,
                    "project_id": project_id(st, &scope),
                    "group_id": if get(st, "groups", &scope).is_some() { json!(group_id(st, &scope)) } else { Value::Null },
                }));
            }
        }
    }
    out
}
