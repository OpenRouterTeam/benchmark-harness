//! Confluence Server `/wiki/rest/api`: content (pages, blog posts, comments, attachments),
//! children, labels, properties, versions, CQL search, spaces, users, templates,
//! restrictions, permission checks and page views.

use serde_json::{Map, Value, json};

use crate::jql;
use crate::model::{Ctx, Out, Req, id_num, iso_z, no_content, ok, parse_time, s};

fn cerr(status: u16, msg: &str) -> Out {
    Out::Json(
        axum::http::StatusCode::from_u16(status).expect("status"),
        json!({"statusCode": status, "data": {"authorized": status != 403, "valid": true, "errors": [], "successful": false}, "message": msg, "reason": match status {
            404 => "Not Found",
            403 => "Forbidden",
            409 => "Conflict",
            _ => "Bad Request",
        }}),
    )
}

fn missing(id: &str) -> Out {
    cerr(
        404,
        &format!("No content found with id: ContentId{{id={id}}}"),
    )
}

fn wiki(ctx: &Ctx) -> String {
    format!("{}/wiki", ctx.base)
}

fn cuser(ctx: &Ctx, name: &str) -> Value {
    match ctx.user_doc(name) {
        Some(u) => {
            json!({"type": "known", "username": s(&u, "name"), "userKey": format!("8a8b8c{:026x}", s(&u, "name").len() as u64 * 7919), "profilePicture": {"path": "/wiki/images/icons/profilepics/default.svg".to_string(), "width": 48, "height": 48, "isDefault": true}, "displayName": s(&u, "displayName"), "email": s(&u, "emailAddress"), "_links": {"self": format!("{}/rest/api/user?key={}", wiki(ctx), s(&u, "name"))}, "_expandable": {"status": ""}})
        }
        None => json!({"type": "anonymous", "displayName": "Anonymous"}),
    }
}

fn space_obj(ctx: &Ctx, key: &str) -> Value {
    let Some(sp) = ctx.get("spaces", key).or_else(|| {
        ctx.all("spaces")
            .into_iter()
            .find(|x| s(x, "key").eq_ignore_ascii_case(key))
    }) else {
        return Value::Null;
    };
    json!({"id": id_num(&sp["id"]), "key": s(&sp, "key"), "name": s(&sp, "name"), "type": if s(&sp, "type").is_empty() { "global" } else { s(&sp, "type") }, "status": "current", "description": {"plain": {"value": s(&sp, "description"), "representation": "plain"}}, "_links": {"webui": format!("/display/{}", s(&sp, "key")), "self": format!("{}/rest/api/space/{}", wiki(ctx), s(&sp, "key"))}, "_expandable": {"metadata": "", "icon": "", "homepage": format!("/rest/api/content/{}", s(&sp, "homepage"))}})
}

fn page_doc(ctx: &Ctx, id: &str) -> Option<Value> {
    ctx.get("pages", id).filter(|p| s(p, "status") != "deleted")
}

fn ancestors(ctx: &Ctx, p: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cur = p.get("parent").and_then(|x| x.as_str().map(str::to_owned));
    while let Some(id) = cur {
        let Some(a) = ctx.get("pages", &id) else {
            break;
        };
        out.push(a.clone());
        cur = a.get("parent").and_then(|x| x.as_str().map(str::to_owned));
    }
    out.reverse();
    out
}

fn title_path(t: &str) -> String {
    t.replace(' ', "+")
}

fn wants(expand: &str, k: &str) -> bool {
    expand
        .split(',')
        .any(|e| e.trim() == k || e.trim().starts_with(&format!("{k}.")))
}

fn body_view(storage: &str) -> String {
    storage.to_owned()
}

/// Renders a page/blog post.
fn content_obj(ctx: &Ctx, p: &Value, expand: &str) -> Value {
    let id = s(p, "id");
    let space = s(p, "space");
    let mut o = json!({"id": id, "type": if s(p, "type").is_empty() { "page" } else { s(p, "type") }, "status": if s(p, "status").is_empty() { "current" } else { s(p, "status") }, "title": s(p, "title")});
    let ver = p["version"].as_i64().unwrap_or(1);
    let mut expandable = Map::new();
    o["space"] = space_obj(ctx, space);
    o["history"] = json!({"latest": true, "createdBy": cuser(ctx, s(p, "createdBy")), "createdDate": s(p, "created"), "lastUpdated": {"by": cuser(ctx, if s(p, "updatedBy").is_empty() { s(p, "createdBy") } else { s(p, "updatedBy") }), "when": if s(p, "updated").is_empty() { s(p, "created") } else { s(p, "updated") }, "number": ver}, "_links": {"self": format!("{}/rest/api/content/{id}/history", wiki(ctx))}});
    o["version"] = json!({"by": cuser(ctx, if s(p, "updatedBy").is_empty() { s(p, "createdBy") } else { s(p, "updatedBy") }), "when": if s(p, "updated").is_empty() { s(p, "created") } else { s(p, "updated") }, "message": s(p, "versionMessage"), "number": ver, "minorEdit": false, "hidden": false, "_links": {"self": format!("{}/rest/experimental/content/{id}/version/{ver}", wiki(ctx))}});
    if wants(expand, "ancestors") {
        o["ancestors"] = json!(ancestors(ctx, p).iter().map(|a| json!({"id": s(a, "id"), "type": "page", "status": "current", "title": s(a, "title"), "_links": {"webui": format!("/display/{}/{}", s(a, "space"), title_path(s(a, "title"))), "self": format!("{}/rest/api/content/{}", wiki(ctx), s(a, "id"))}})).collect::<Vec<_>>());
    } else {
        expandable.insert("ancestors".into(), json!(""));
    }
    if wants(expand, "body") {
        let storage = s(p, "body");
        let mut body = Map::new();
        if wants(expand, "body.storage") || expand.split(',').any(|e| e.trim() == "body") {
            body.insert("storage".into(), json!({"value": storage, "representation": "storage", "_expandable": {"content": format!("/rest/api/content/{id}")}}));
        }
        if wants(expand, "body.view")
            || wants(expand, "body.export_view")
            || wants(expand, "body.styled_view")
        {
            body.insert(
                "view".into(),
                json!({"value": body_view(storage), "representation": "view"}),
            );
            body.insert(
                "export_view".into(),
                json!({"value": body_view(storage), "representation": "export_view"}),
            );
        }
        o["body"] = Value::Object(body);
    } else {
        expandable.insert("body".into(), json!(""));
    }
    if wants(expand, "metadata") || wants(expand, "metadata.labels") {
        let labels: Vec<Value> = p["labels"].as_array().cloned().unwrap_or_default().iter().enumerate().map(|(i, l)| json!({"prefix": "global", "name": l.as_str().unwrap_or(""), "id": (5_000_000 + i).to_string(), "label": l.as_str().unwrap_or("")})).collect();
        o["metadata"] = json!({"labels": {"results": labels, "start": 0, "limit": 200, "size": labels.len()}, "_expandable": {"properties": "", "currentuser": "", "frontend": ""}});
    }
    if wants(expand, "children") {
        let kids: Vec<Value> = children(ctx, id, "page")
            .iter()
            .map(|c| content_obj(ctx, c, ""))
            .collect();
        o["children"] =
            json!({"page": {"results": kids, "start": 0, "limit": 25, "size": kids.len()}});
    }
    if wants(expand, "container") {
        o["container"] = space_obj(ctx, space);
    }
    if !s(p, "container").is_empty() {
        let parent = ctx.get("pages", s(p, "container"));
        o["container"] = parent.map(|x| json!({"id": s(&x, "id"), "type": "page", "status": "current", "title": s(&x, "title")})).unwrap_or(Value::Null);
    }
    o["extensions"] = json!({"position": p.get("position").cloned().unwrap_or(json!("none"))});
    o["_links"] = json!({"webui": format!("/display/{space}/{}", title_path(s(p, "title"))), "edit": format!("/pages/resumedraft.action?draftId={id}"), "tinyui": format!("/x/{id}"), "collection": "/rest/api/content", "base": wiki(ctx), "context": "/wiki", "self": format!("{}/rest/api/content/{id}", wiki(ctx))});
    expandable.insert(
        "children".into(),
        json!(format!("/rest/api/content/{id}/child")),
    );
    expandable.insert(
        "descendants".into(),
        json!(format!("/rest/api/content/{id}/descendant")),
    );
    o["_expandable"] = Value::Object(expandable);
    o
}

fn comment_obj(ctx: &Ctx, c: &Value, expand: &str) -> Value {
    let id = s(c, "id");
    let page = ctx.get("pages", s(c, "page")).unwrap_or(json!({}));
    let mut o = json!({"id": id, "type": "comment", "status": "current", "title": format!("Re: {}", s(&page, "title")), "space": space_obj(ctx, s(&page, "space")),
        "history": {"latest": true, "createdBy": cuser(ctx, s(c, "author")), "createdDate": s(c, "created")},
        "version": {"by": cuser(ctx, s(c, "author")), "when": if s(c, "updated").is_empty() { s(c, "created") } else { s(c, "updated") }, "number": c["version"].as_i64().unwrap_or(1), "minorEdit": false},
        "container": {"id": s(&page, "id"), "type": "page", "status": "current", "title": s(&page, "title"), "_links": {"self": format!("{}/rest/api/content/{}", wiki(ctx), s(&page, "id"))}},
        "extensions": {"location": if s(c, "location").is_empty() { "footer" } else { s(c, "location") }, "_expandable": {"resolution": ""}},
        "_links": {"webui": format!("/display/{}/{}?focusedCommentId={id}#comment-{id}", s(&page, "space"), title_path(s(&page, "title"))), "self": format!("{}/rest/api/content/{id}", wiki(ctx))}});
    if s(c, "location") == "inline" {
        o["extensions"]["inlineProperties"] =
            json!({"originalSelection": s(c, "selection"), "markerRef": format!("marker-{id}")});
        o["extensions"]["resolution"] = json!({"status": if c["resolved"].as_bool().unwrap_or(false) { "resolved" } else { "open" }});
    }
    if !expand.is_empty() || true {
        o["body"] = json!({"storage": {"value": s(c, "body"), "representation": "storage"}, "view": {"value": s(c, "body"), "representation": "view"}});
    }
    let parents: Vec<Value> = c
        .get("parent")
        .and_then(Value::as_str)
        .map(|p| vec![json!({"id": p, "type": "comment"})])
        .unwrap_or_default();
    o["ancestors"] = json!(parents);
    o
}

fn attachment_obj(ctx: &Ctx, a: &Value) -> Value {
    let id = s(a, "id");
    let page = s(a, "page");
    json!({"id": id, "type": "attachment", "status": "current", "title": s(a, "title"),
        "version": {"by": cuser(ctx, s(a, "author")), "when": s(a, "created"), "number": a["version"].as_i64().unwrap_or(1), "minorEdit": false},
        "container": {"id": page, "type": "page"},
        "metadata": {"mediaType": s(a, "mediaType"), "comment": s(a, "comment"), "labels": {"results": [], "start": 0, "limit": 200, "size": 0}},
        "extensions": {"mediaType": s(a, "mediaType"), "fileSize": a["size"].as_i64().unwrap_or(s(a, "content").len() as i64), "comment": s(a, "comment")},
        "_links": {"webui": format!("/pages/viewpageattachments.action?pageId={page}&preview=%2F{page}%2F{id}%2F{}", s(a, "title")), "download": format!("/download/attachments/{page}/{}?version=1&modificationDate=1793286000000&api=v2", s(a, "title")), "self": format!("{}/rest/api/content/{id}", wiki(ctx))}})
}

fn children(ctx: &Ctx, id: &str, ty: &str) -> Vec<Value> {
    let mut kids: Vec<Value> = ctx
        .all("pages")
        .into_iter()
        .filter(|p| {
            s(p, "parent") == id
                && s(p, "status") != "deleted"
                && (ty == "page" && s(p, "type") != "blogpost" || ty == s(p, "type"))
        })
        .collect();
    kids.sort_by_key(|p| p["position"].as_i64().unwrap_or(i64::MAX));
    kids
}

fn paged(results: Vec<Value>, req: &Req, base_path: &str) -> Value {
    let start = req.q_i64("start").unwrap_or(0).max(0) as usize;
    let limit = req.q_i64("limit").unwrap_or(25).clamp(1, 500) as usize;
    let total = results.len();
    let vals: Vec<Value> = results.into_iter().skip(start).take(limit).collect();
    let mut links = json!({"base": "", "context": "/wiki", "self": base_path});
    if start + vals.len() < total {
        links["next"] = json!(format!("{base_path}?limit={limit}&start={}", start + limit));
    }
    json!({"results": vals, "start": start, "limit": limit, "size": vals.len(), "_links": links})
}

fn cql_values(ctx: &Ctx, p: &Value, field: &str) -> Option<Vec<String>> {
    let one = |v: &str| {
        if v.is_empty() {
            vec![]
        } else {
            vec![v.to_owned()]
        }
    };
    Some(match field {
        "type" => one(if s(p, "type").is_empty() {
            "page"
        } else {
            s(p, "type")
        }),
        "space" | "space.key" => {
            let sp = ctx.get("spaces", s(p, "space"));
            let mut v = one(s(p, "space"));
            if let Some(sp) = sp {
                v.push(s(&sp, "name").to_owned());
                v.push(s(&sp, "id").to_owned());
            }
            v
        }
        "space.title" | "space.name" => ctx
            .get("spaces", s(p, "space"))
            .map(|sp| one(s(&sp, "name")))
            .unwrap_or_default(),
        "title" => one(s(p, "title")),
        "text" | "sitesearch" => vec![
            s(p, "title").to_owned(),
            strip_tags(s(p, "body")),
            p["labels"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default(),
        ],
        "label" | "labeltext" => p["labels"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        "id" | "content" => one(s(p, "id")),
        "parent" => one(s(p, "parent")),
        "ancestor" => ancestors(ctx, p)
            .iter()
            .map(|a| s(a, "id").to_owned())
            .collect(),
        "creator" | "contributor" => {
            let mut v = Vec::new();
            for n in [s(p, "createdBy"), s(p, "updatedBy")] {
                if let Some(u) = ctx.user_doc(n) {
                    v.extend([
                        s(&u, "name").to_owned(),
                        s(&u, "displayName").to_owned(),
                        s(&u, "emailAddress").to_owned(),
                    ]);
                }
            }
            v
        }
        "created" => one(s(p, "created")),
        "lastmodified" => one(if s(p, "updated").is_empty() {
            s(p, "created")
        } else {
            s(p, "updated")
        }),
        _ => return None,
    })
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
}

fn run_cql(ctx: &Ctx, cql: &str) -> Result<Vec<Value>, String> {
    let q =
        jql::parse(cql).map_err(|e| e.replace("Error in the JQL Query", "Could not parse cql"))?;
    let me = ctx.me_name();
    let none = |_: &str| Vec::new();
    let env = jql::Env {
        now: ctx.now,
        me: &me,
        sprints: &none,
    };
    let mut out = Vec::new();
    for p in ctx
        .all("pages")
        .into_iter()
        .filter(|p| s(p, "status") != "deleted")
    {
        let get = |f: &str| cql_values(ctx, &p, f);
        if jql::eval(&q.expr, &env, &get).map_err(|e| {
            e.replace("Field", "Could not find field").replace(
                " does not exist or you do not have permission to view it.",
                "",
            )
        })? {
            out.push(p);
        }
    }
    for (field, desc) in q.order.iter().rev() {
        out.sort_by(|a, b| {
            let key = |p: &Value| match field.as_str() {
                "title" => s(p, "title").to_lowercase(),
                "created" => s(p, "created").to_owned(),
                _ => (if s(p, "updated").is_empty() {
                    s(p, "created")
                } else {
                    s(p, "updated")
                })
                .to_owned(),
            };
            let o = key(a).cmp(&key(b));
            if *desc { o.reverse() } else { o }
        });
    }
    Ok(out)
}

fn excerpt(p: &Value) -> String {
    let t = strip_tags(s(p, "body"));
    let short: String = t.chars().take(200).collect();
    if t.chars().count() > 200 {
        format!("{short}...")
    } else {
        short
    }
}

fn friendly(t: &str) -> String {
    parse_time(t)
        .map(|x| {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let (y, m, d) = crate::model::civil(x.div_euclid(86_400));
            format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
        })
        .unwrap_or_default()
}

fn search(ctx: &Ctx, req: &Req) -> Out {
    let cql = req.q("cql").unwrap_or("").to_owned();
    if cql.trim().is_empty() {
        return cerr(400, "CQL query parameter is required");
    }
    let found = match run_cql(ctx, &cql) {
        Ok(f) => f,
        Err(e) => return cerr(400, &e),
    };
    let start = req.q_i64("start").unwrap_or(0).max(0) as usize;
    let limit = req.q_i64("limit").unwrap_or(25).clamp(1, 1000) as usize;
    let expand = req.q("expand").unwrap_or("");
    let total = found.len();
    let results: Vec<Value> = found
        .iter()
        .skip(start)
        .take(limit)
        .map(|p| {
            let updated = if s(p, "updated").is_empty() { s(p, "created") } else { s(p, "updated") };
            let space = ctx.get("spaces", s(p, "space")).unwrap_or(json!({}));
            json!({"content": content_obj(ctx, p, expand), "title": s(p, "title"), "excerpt": excerpt(p), "url": format!("/display/{}/{}", s(p, "space"), title_path(s(p, "title"))), "resultGlobalContainer": {"title": s(&space, "name"), "displayUrl": format!("/display/{}", s(&space, "key"))}, "breadcrumbs": [], "entityType": "content", "iconCssClass": "aui-icon content-type-page", "lastModified": updated, "friendlyLastModified": friendly(updated), "score": 0.0})
        })
        .collect();
    ok(
        json!({"results": results, "start": start, "limit": limit, "size": results.len(), "totalSize": total, "cqlQuery": cql, "searchDuration": 12, "_links": {"base": wiki(ctx), "context": "/wiki"}}),
    )
}

fn user_search(ctx: &Ctx, req: &Req) -> Out {
    let cql = req.q("cql").unwrap_or("");
    let needle = cql.split(['"', '\'']).nth(1).unwrap_or(cql).to_lowercase();
    let users: Vec<Value> = crate::issues::find_users(ctx, &needle)
        .iter()
        .map(|u| json!({"user": cuser(ctx, s(u, "name")), "title": s(u, "displayName"), "excerpt": "", "url": format!("/display/~{}", s(u, "name")), "entityType": "user", "lastModified": "", "score": 0.0}))
        .collect();
    let total = users.len();
    ok(
        json!({"results": users, "start": 0, "limit": 25, "size": total, "totalSize": total, "cqlQuery": cql, "searchDuration": 5, "_links": {"base": wiki(ctx), "context": "/wiki"}}),
    )
}

fn storage_of(body: &Value) -> Option<String> {
    for rep in ["storage", "wiki", "editor"] {
        if let Some(v) = body[rep]["value"].as_str() {
            return Some(v.to_owned());
        }
    }
    None
}

fn create_content(ctx: &mut Ctx, b: &Value) -> Out {
    let ty = if s(b, "type").is_empty() {
        "page"
    } else {
        s(b, "type")
    };
    let body = storage_of(&b["body"]).unwrap_or_default();
    if ty == "comment" {
        let page = s(&b["container"], "id").to_owned();
        let Some(pg) = page_doc(ctx, &page) else {
            return cerr(404, &format!("Cannot find content with id {page}"));
        };
        let id = ctx.next_id("page_comments", 100_000_000);
        let parent = b["ancestors"]
            .as_array()
            .and_then(|a| a.last())
            .map(|a| s(a, "id").to_owned())
            .filter(|x| !x.is_empty());
        let location = b["extensions"]["location"].as_str().unwrap_or("footer");
        let mut c = json!({"id": id, "page": s(&pg, "id"), "body": body, "author": ctx.me_name(), "created": iso_z(ctx.now), "location": location});
        if let Some(p) = parent {
            c["parent"] = json!(p);
        }
        if let Some(sel) = b["extensions"]["inlineProperties"]["originalSelection"].as_str() {
            if !strip_tags(&s(&pg, "body").replace("&nbsp;", " ")).contains(sel)
                && !s(&pg, "body").contains(sel)
            {
                return cerr(
                    400,
                    "Cannot find the specified text selection in the page content.",
                );
            }
            c["selection"] = json!(sel);
        }
        ctx.put("page_comments", &id, c.clone());
        return ok(comment_obj(ctx, &c, "body.storage"));
    }
    let title = s(b, "title").trim().to_owned();
    if title.is_empty() {
        return cerr(400, "Content title is required");
    }
    let space = s(&b["space"], "key").to_owned();
    if ctx.get("spaces", &space).is_none() {
        return cerr(404, &format!("No space with key : {space}"));
    }
    if ctx.all("pages").iter().any(|p| {
        s(p, "space") == space
            && s(p, "title").eq_ignore_ascii_case(&title)
            && s(p, "status") != "deleted"
    }) {
        return cerr(
            400,
            &format!(
                "A page with this title already exists: A page already exists with the title {title} in the space with key {space}"
            ),
        );
    }
    let parent = b["ancestors"]
        .as_array()
        .and_then(|a| a.last())
        .map(|a| s(a, "id").to_owned())
        .filter(|x| !x.is_empty());
    if let Some(p) = &parent
        && page_doc(ctx, p).is_none()
    {
        return cerr(
            404,
            &format!("No content found with id: ContentId{{id={p}}}"),
        );
    }
    let id = ctx.next_id("pages", 100_000);
    let now = iso_z(ctx.now);
    let me = ctx.me_name();
    let parent = parent.or_else(|| {
        ctx.get("spaces", &space)
            .map(|sp| s(&sp, "homepage").to_owned())
            .filter(|h| !h.is_empty())
    });
    let p = json!({"id": id, "type": ty, "title": title, "space": space, "parent": parent, "body": body, "version": 1, "created": now, "createdBy": me, "updated": now, "updatedBy": me, "labels": [], "status": "current", "history": [{"number": 1, "title": title, "body": body, "when": now, "by": me, "message": ""}]});
    ctx.put("pages", &id, p.clone());
    ok(content_obj(ctx, &p, "body.storage,version,space,ancestors"))
}

fn update_content(ctx: &mut Ctx, id: &str, b: &Value) -> Out {
    if let Some(mut c) = ctx.get("page_comments", id) {
        if let Some(v) = storage_of(&b["body"]) {
            c["body"] = json!(v);
        }
        c["version"] = json!(c["version"].as_i64().unwrap_or(1) + 1);
        c["updated"] = json!(iso_z(ctx.now));
        ctx.put("page_comments", id, c.clone());
        return ok(comment_obj(ctx, &c, ""));
    }
    let Some(mut p) = page_doc(ctx, id) else {
        return missing(id);
    };
    let cur = p["version"].as_i64().unwrap_or(1);
    let next = b["version"]["number"].as_i64().unwrap_or(cur + 1);
    if next != cur + 1 {
        return cerr(
            409,
            &format!("Version must be incremented on update. Current version is: {cur}"),
        );
    }
    if let Some(t) = b["title"].as_str() {
        let t = t.trim();
        if t.is_empty() {
            return cerr(400, "Content title is required");
        }
        if ctx.all("pages").iter().any(|x| {
            s(x, "id") != id
                && s(x, "space") == s(&p, "space")
                && s(x, "title").eq_ignore_ascii_case(t)
                && s(x, "status") != "deleted"
        }) {
            return cerr(
                400,
                &format!(
                    "A page with this title already exists: A page already exists with the title {t} in the space with key {}",
                    s(&p, "space")
                ),
            );
        }
        p["title"] = json!(t);
    }
    if let Some(v) = storage_of(&b["body"]) {
        p["body"] = json!(v);
    }
    if let Some(parent) = b["ancestors"]
        .as_array()
        .and_then(|a| a.last())
        .map(|a| s(a, "id").to_owned())
        .filter(|x| !x.is_empty())
    {
        if page_doc(ctx, &parent).is_none() {
            return missing(&parent);
        }
        p["parent"] = json!(parent);
    }
    let now = iso_z(ctx.now);
    let me = ctx.me_name();
    p["version"] = json!(next);
    p["updated"] = json!(now);
    p["updatedBy"] = json!(me);
    p["versionMessage"] = json!(s(&b["version"], "message"));
    let mut hist = p["history"].as_array().cloned().unwrap_or_default();
    hist.push(json!({"number": next, "title": s(&p, "title"), "body": s(&p, "body"), "when": now, "by": me, "message": s(&b["version"], "message")}));
    p["history"] = json!(hist);
    ctx.put("pages", id, p.clone());
    ok(content_obj(ctx, &p, "body.storage,version,space,ancestors"))
}

fn historical(ctx: &Ctx, p: &Value, n: i64) -> Option<Value> {
    let h = p["history"]
        .as_array()?
        .iter()
        .find(|h| h["number"].as_i64() == Some(n))?
        .clone();
    let mut old = p.clone();
    old["title"] = h["title"].clone();
    old["body"] = h["body"].clone();
    old["version"] = json!(n);
    old["updated"] = h["when"].clone();
    old["updatedBy"] = h["by"].clone();
    if n != p["version"].as_i64().unwrap_or(1) {
        old["status"] = json!("historical");
    }
    let _ = ctx;
    Some(old)
}

pub fn route(ctx: &mut Ctx, req: &Req) -> Out {
    let path = req
        .path
        .trim_start_matches("/wiki")
        .trim_start_matches('/')
        .to_owned();
    let segs: Vec<&str> = path.split('/').collect();
    let m = req.method.as_str();
    let expand = req.q("expand").unwrap_or("").to_owned();
    match (m, segs.as_slice()) {
        ("GET", ["rest", "api", "user", "current"]) => ok(cuser(ctx, &ctx.me_name())),
        ("GET", ["rest", "api", "user"]) => {
            let who = req
                .q("username")
                .or(req.q("key"))
                .or(req.q("accountId"))
                .unwrap_or("");
            match ctx.user_doc(who) {
                Some(u) => ok(cuser(ctx, s(&u, "name"))),
                None => cerr(404, &format!("No user found with username: {who}")),
            }
        }
        ("GET", ["rest", "api", "search"] | ["rest", "api", "content", "search"]) => {
            search(ctx, req)
        }
        ("GET", ["rest", "api", "search", "user"]) => user_search(ctx, req),
        ("GET", ["rest", "api", "group", _, "member"]) => ok(paged(
            ctx.all("users")
                .iter()
                .map(|u| cuser(ctx, s(u, "name")))
                .collect(),
            req,
            "/rest/api/group/member",
        )),
        ("GET", ["rest", "api", "space"]) => {
            let keys = req.q_list("spaceKey");
            let spaces: Vec<Value> = ctx
                .all("spaces")
                .into_iter()
                .filter(|sp| keys.is_empty() || keys.iter().any(|k| k == s(sp, "key")))
                .map(|sp| space_obj(ctx, s(&sp, "key")))
                .collect();
            ok(paged(spaces, req, "/rest/api/space"))
        }
        ("GET", ["rest", "api", "space", key]) => match space_obj(ctx, key) {
            Value::Null => cerr(404, &format!("No space with key : {key}")),
            v => ok(v),
        },
        ("GET", ["rest", "api", "space", key, "content", ..]) => {
            if space_obj(ctx, key).is_null() {
                return cerr(404, &format!("No space with key : {key}"));
            }
            let ty = segs.get(5).copied().unwrap_or("page");
            let depth_root = req.q("depth") == Some("root");
            let pages: Vec<Value> = ctx
                .all("pages")
                .into_iter()
                .filter(|p| {
                    s(p, "space") == *key
                        && s(p, "status") != "deleted"
                        && (if ty == "blogpost" {
                            s(p, "type") == "blogpost"
                        } else {
                            s(p, "type") != "blogpost"
                        })
                        && (!depth_root || p["parent"].is_null())
                })
                .map(|p| content_obj(ctx, &p, &expand))
                .collect();
            if segs.len() > 5 {
                return ok(paged(
                    pages,
                    req,
                    &format!("/rest/api/space/{key}/content/{ty}"),
                ));
            }
            ok(
                json!({"page": paged(pages, req, &format!("/rest/api/space/{key}/content/page")), "blogpost": {"results": [], "start": 0, "limit": 25, "size": 0}}),
            )
        }
        ("GET", ["rest", "api", "space", key, "permission" | "permissions"]) => {
            if space_obj(ctx, key).is_null() {
                return cerr(404, &format!("No space with key : {key}"));
            }
            ok(
                json!([{"operation": {"operation": "read", "targetType": "space"}, "subjects": {"group": {"results": [{"type": "group", "name": "confluence-users"}], "size": 1}}}]),
            )
        }
        ("GET", ["rest", "api", "content"]) => {
            let space = req.q("spaceKey");
            let title = req.q("title");
            let ty = req.q("type").unwrap_or("page");
            let pages: Vec<Value> = ctx
                .all("pages")
                .into_iter()
                .filter(|p| s(p, "status") != "deleted")
                .filter(|p| space.is_none_or(|sp| s(p, "space") == sp))
                .filter(|p| title.is_none_or(|t| s(p, "title") == t))
                .filter(|p| {
                    if ty == "blogpost" {
                        s(p, "type") == "blogpost"
                    } else {
                        s(p, "type") != "blogpost"
                    }
                })
                .map(|p| content_obj(ctx, &p, &expand))
                .collect();
            ok(paged(pages, req, "/rest/api/content"))
        }
        ("POST", ["rest", "api", "content"]) => create_content(ctx, &req.body),
        ("GET", ["rest", "api", "content", id]) => {
            if let Some(c) = ctx.get("page_comments", id) {
                return ok(comment_obj(ctx, &c, &expand));
            }
            if let Some(a) = ctx.get("page_attachments", id) {
                return ok(attachment_obj(ctx, &a));
            }
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            if let Some(v) = req.q_i64("version") {
                return match historical(ctx, &p, v) {
                    Some(old) => ok(content_obj(ctx, &old, &expand)),
                    None => cerr(
                        404,
                        &format!("No content found with id: ContentId{{id={id}}} and version: {v}"),
                    ),
                };
            }
            ok(content_obj(ctx, &p, &expand))
        }
        ("PUT", ["rest", "api", "content", id]) => update_content(ctx, id, &req.body),
        ("DELETE", ["rest", "api", "content", id]) => {
            if ctx.remove("page_comments", id).is_some()
                || ctx.remove("page_attachments", id).is_some()
            {
                return no_content();
            }
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            p["status"] = json!(if req.q("status") == Some("trashed")
                || s(&p, "status") == "trashed"
            {
                "deleted"
            } else {
                "trashed"
            });
            ctx.put("pages", id, p);
            no_content()
        }
        ("POST", ["rest", "api", "content", id, "copy"]) => {
            let Some(src) = page_doc(ctx, id) else {
                return missing(id);
            };
            let dest_space = s(&req.body["destination"], "value").to_owned();
            let title = req.body["pageTitle"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| s(&src, "title").to_owned());
            let body = json!({"type": "page", "title": title, "space": {"key": if s(&req.body["destination"], "type") == "space" { dest_space.clone() } else { s(&src, "space").to_owned() }}, "ancestors": if s(&req.body["destination"], "type") == "parent_page" { json!([{"id": dest_space}]) } else { json!([]) }, "body": {"storage": {"value": s(&src, "body"), "representation": "storage"}}});
            create_content(ctx, &body)
        }
        ("GET", ["rest", "api", "content", id, "child"]) => {
            if page_doc(ctx, id).is_none() {
                return missing(id);
            }
            let kids: Vec<Value> = children(ctx, id, "page")
                .iter()
                .map(|c| content_obj(ctx, c, &expand))
                .collect();
            let comments: Vec<Value> = ctx
                .all("page_comments")
                .into_iter()
                .filter(|c| s(c, "page") == *id)
                .map(|c| comment_obj(ctx, &c, &expand))
                .collect();
            ok(
                json!({"page": paged(kids, req, &format!("/rest/api/content/{id}/child/page")), "comment": paged(comments, req, &format!("/rest/api/content/{id}/child/comment")), "_links": {"base": wiki(ctx), "context": "/wiki"}}),
            )
        }
        ("GET", ["rest", "api", "content", id, "child" | "descendant", ty]) => {
            if page_doc(ctx, id).is_none() {
                return missing(id);
            }
            let list: Vec<Value> = match *ty {
                "page" | "blogpost" => {
                    if segs[4] == "descendant" {
                        let mut out = Vec::new();
                        let mut stack = vec![id.to_string()];
                        while let Some(cur) = stack.pop() {
                            for c in children(ctx, &cur, "page") {
                                stack.push(s(&c, "id").to_owned());
                                out.push(content_obj(ctx, &c, &expand));
                            }
                        }
                        out
                    } else {
                        children(ctx, id, ty)
                            .iter()
                            .map(|c| content_obj(ctx, c, &expand))
                            .collect()
                    }
                }
                "comment" => {
                    let loc = req.q("location");
                    ctx.all("page_comments")
                        .into_iter()
                        .filter(|c| s(c, "page") == *id)
                        .filter(|c| {
                            loc.is_none_or(|l| {
                                l == if s(c, "location").is_empty() {
                                    "footer"
                                } else {
                                    s(c, "location")
                                }
                            })
                        })
                        .map(|c| comment_obj(ctx, &c, &expand))
                        .collect()
                }
                "attachment" => {
                    let fname = req.q("filename");
                    ctx.all("page_attachments")
                        .into_iter()
                        .filter(|a| s(a, "page") == *id)
                        .filter(|a| fname.is_none_or(|f| s(a, "title") == f))
                        .map(|a| attachment_obj(ctx, &a))
                        .collect()
                }
                _ => vec![],
            };
            ok(paged(
                list,
                req,
                &format!("/rest/api/content/{id}/child/{ty}"),
            ))
        }
        ("POST" | "PUT", ["rest", "api", "content", id, "child", "attachment", ..]) => {
            if page_doc(ctx, id).is_none() {
                return missing(id);
            }
            let files = multipart(req);
            if files.is_empty() {
                return cerr(400, "No attachment file found in request");
            }
            let mut out = Vec::new();
            for (name, data, ct, comment) in files {
                let existing = ctx
                    .all("page_attachments")
                    .into_iter()
                    .find(|a| s(a, "page") == *id && s(a, "title") == name);
                if existing.is_some() && m == "POST" && req.q("allowDuplicated") != Some("true") {
                    return cerr(
                        400,
                        &format!(
                            "Cannot add a new attachment with same file name as an existing attachment: {name}"
                        ),
                    );
                }
                let (aid, version) = match &existing {
                    Some(e) => (
                        s(e, "id").to_owned(),
                        e["version"].as_i64().unwrap_or(1) + 1,
                    ),
                    None => {
                        let max = ctx
                            .all("page_attachments")
                            .iter()
                            .filter_map(|a| {
                                s(a, "id").trim_start_matches("att").parse::<i64>().ok()
                            })
                            .max()
                            .unwrap_or(199_999_999);
                        (format!("att{}", max.max(199_999_999) + 1), 1)
                    }
                };
                let a = json!({"id": aid, "page": id, "title": name, "mediaType": ct, "size": data.len(), "content": String::from_utf8_lossy(&data), "comment": comment, "author": ctx.me_name(), "created": iso_z(ctx.now), "version": version});
                ctx.put("page_attachments", &aid, a.clone());
                out.push(attachment_obj(ctx, &a));
            }
            ok(json!({"results": out, "size": out.len(), "start": 0, "limit": 50}))
        }
        ("GET", ["rest", "api", "content", id, "label"]) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let labels: Vec<Value> = p["labels"].as_array().cloned().unwrap_or_default().iter().enumerate().map(|(i, l)| json!({"prefix": "global", "name": l.as_str().unwrap_or(""), "id": (5_000_000 + i).to_string(), "label": l.as_str().unwrap_or("")})).collect();
            ok(paged(labels, req, &format!("/rest/api/content/{id}/label")))
        }
        ("POST", ["rest", "api", "content", id, "label"]) => {
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let items: Vec<Value> = match &req.body {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let mut labels: Vec<Value> = p["labels"].as_array().cloned().unwrap_or_default();
            for it in items {
                let name = s(&it, "name").trim().to_lowercase();
                if name.is_empty() || name.contains(' ') {
                    return cerr(
                        400,
                        &format!("Label name '{name}' is invalid: labels can't contain spaces"),
                    );
                }
                if !labels.iter().any(|l| l == name.as_str()) {
                    labels.push(json!(name));
                }
            }
            p["labels"] = json!(labels);
            ctx.put("pages", id, p.clone());
            let out: Vec<Value> = labels.iter().enumerate().map(|(i, l)| json!({"prefix": "global", "name": l.as_str().unwrap_or(""), "id": (5_000_000 + i).to_string(), "label": l.as_str().unwrap_or("")})).collect();
            ok(json!({"results": out, "start": 0, "limit": 200, "size": out.len()}))
        }
        ("DELETE", ["rest", "api", "content", id, "label", ..]) => {
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let name = req
                .q("name")
                .map(str::to_owned)
                .or_else(|| segs.get(6).map(|x| x.to_string()))
                .unwrap_or_default();
            let labels: Vec<Value> = p["labels"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|l| l != name.as_str())
                .collect();
            p["labels"] = json!(labels);
            ctx.put("pages", id, p);
            no_content()
        }
        ("GET", ["rest", "api", "content", id, "property"]) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let props: Vec<Value> = p["properties"].as_object().cloned().unwrap_or_default().into_iter().map(|(k, v)| json!({"id": k, "key": k, "value": v["value"], "version": {"number": v["version"].as_i64().unwrap_or(1)}})).collect();
            ok(paged(
                props,
                req,
                &format!("/rest/api/content/{id}/property"),
            ))
        }
        ("GET", ["rest", "api", "content", id, "property", key]) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            match p["properties"].get(*key) {
                Some(v) => ok(
                    json!({"id": key, "key": key, "value": v["value"], "version": {"number": v["version"].as_i64().unwrap_or(1)}}),
                ),
                None => cerr(404, &format!("Cannot find property with key: {key}")),
            }
        }
        ("POST" | "PUT", ["rest", "api", "content", id, "property", ..]) => {
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let key = segs
                .get(6)
                .map(|x| x.to_string())
                .unwrap_or_else(|| s(&req.body, "key").to_owned());
            let ver = p["properties"][&key]["version"].as_i64().unwrap_or(0) + 1;
            if !p["properties"].is_object() {
                p["properties"] = json!({});
            }
            p["properties"][&key] = json!({"value": req.body["value"], "version": ver});
            ctx.put("pages", id, p);
            ok(
                json!({"id": key, "key": key, "value": req.body["value"], "version": {"number": ver}}),
            )
        }
        ("DELETE", ["rest", "api", "content", id, "property", key]) => {
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            if let Some(o) = p["properties"].as_object_mut() {
                o.remove(*key);
            }
            ctx.put("pages", id, p);
            no_content()
        }
        ("GET", ["rest", "api", "content", id, "history"]) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let c = content_obj(ctx, &p, "");
            let mut h = c["history"].clone();
            h["lastUpdated"] = c["version"].clone();
            ok(h)
        }
        (
            "GET",
            ["rest", "api", "content", id, "version"]
            | ["rest", "experimental", "content", id, "version"],
        ) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let mut list: Vec<Value> = p["history"].as_array().cloned().unwrap_or_default().iter().map(|h| json!({"by": cuser(ctx, s(h, "by")), "when": s(h, "when"), "message": s(h, "message"), "number": h["number"], "minorEdit": false})).collect();
            list.reverse();
            ok(paged(list, req, &format!("/rest/api/content/{id}/version")))
        }
        ("GET", ["rest", "api" | "experimental", "content", id, "version", n]) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            match p["history"].as_array().and_then(|h| {
                h.iter()
                    .find(|x| x["number"].as_i64().is_some_and(|v| n.parse() == Ok(v)))
            }) {
                Some(h) => ok(
                    json!({"by": cuser(ctx, s(h, "by")), "when": s(h, "when"), "message": s(h, "message"), "number": h["number"], "minorEdit": false, "content": {"id": id, "type": "page", "title": s(h, "title"), "body": {"storage": {"value": s(h, "body"), "representation": "storage"}}}}),
                ),
                None => cerr(404, &format!("Version {n} of content {id} does not exist")),
            }
        }
        (
            "GET",
            [
                "rest",
                "api",
                "content",
                id,
                "restriction" | "restrictions",
                ..,
            ],
        ) => {
            let Some(p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let restr = p.get("restrictions").cloned().unwrap_or(json!({}));
            let op = |name: &str| {
                let users: Vec<Value> = restr[name]["users"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|u| cuser(ctx, u))
                    .collect();
                let groups: Vec<Value> = restr[name]["groups"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|g| json!({"type": "group", "name": g}))
                    .collect();
                json!({"operation": name, "restrictions": {"user": {"results": users, "start": 0, "limit": 100, "size": users.len()}, "group": {"results": groups, "start": 0, "limit": 100, "size": groups.len()}}, "_links": {"self": format!("{}/rest/api/content/{id}/restriction/byOperation/{name}", wiki(ctx))}})
            };
            ok(
                json!({"read": op("read"), "update": op("update"), "_links": {"base": wiki(ctx), "context": "/wiki"}}),
            )
        }
        (
            "PUT" | "POST",
            [
                "rest",
                "api",
                "content",
                id,
                "restriction" | "restrictions",
                ..,
            ],
        ) => {
            let Some(mut p) = page_doc(ctx, id) else {
                return missing(id);
            };
            let items = match &req.body {
                Value::Array(a) => a.clone(),
                other => other["results"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![other.clone()]),
            };
            let mut restr = if m == "PUT" {
                json!({})
            } else {
                p.get("restrictions").cloned().unwrap_or(json!({}))
            };
            for it in items {
                let opname = s(&it, "operation").to_owned();
                let users: Vec<Value> = it["restrictions"]["user"]
                    .as_array()
                    .or(it["restrictions"]["user"]["results"].as_array())
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|u| {
                        json!(
                            [s(u, "username"), s(u, "userKey"), s(u, "accountId")]
                                .into_iter()
                                .find(|x| !x.is_empty())
                                .unwrap_or("")
                        )
                    })
                    .collect();
                let groups: Vec<Value> = it["restrictions"]["group"]
                    .as_array()
                    .or(it["restrictions"]["group"]["results"].as_array())
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|g| json!(s(g, "name")))
                    .collect();
                restr[&opname] = json!({"users": users, "groups": groups});
            }
            p["restrictions"] = restr;
            ctx.put("pages", id, p);
            route(
                ctx,
                &Req {
                    method: axum::http::Method::GET,
                    ..req_clone(req)
                },
            )
        }
        ("POST", ["rest", "api", "content", id, "permission", "check"]) => {
            if page_doc(ctx, id).is_none() {
                return missing(id);
            }
            let op = s(&req.body, "operation").to_owned();
            let subject = s(&req.body["subject"], "identifier").to_owned();
            let p = page_doc(ctx, id).expect("page");
            let restricted = p["restrictions"][&op]["users"]
                .as_array()
                .is_some_and(|u| !u.is_empty() && !u.iter().any(|x| x == subject.as_str()));
            let exists = ctx.user_doc(&subject).is_some();
            let errors: Vec<Value> = if !exists {
                vec![json!({"message": {"key": "user.not.found", "args": [subject]}})]
            } else {
                vec![]
            };
            ok(json!({"hasPermission": exists && !restricted, "errors": errors}))
        }
        ("GET", ["rest", "api", "analytics", "content", id, "views"]) => match page_doc(ctx, id) {
            Some(p) => {
                ok(json!({"id": id_num(&p["id"]), "count": p["views"].as_i64().unwrap_or(0)}))
            }
            None => missing(id),
        },
        ("GET", ["rest", "api", "template", "page"]) => {
            let space = req.q("spaceKey");
            let list: Vec<Value> = ctx
                .all("templates")
                .into_iter()
                .filter(|t| space.is_none_or(|sp| s(t, "space").is_empty() || s(t, "space") == sp))
                .map(|t| template_obj(ctx, &t, false))
                .collect();
            ok(paged(list, req, "/rest/api/template/page"))
        }
        ("GET", ["rest", "api", "template", id]) => match ctx.get("templates", id) {
            Some(t) => ok(template_obj(ctx, &t, true)),
            None => cerr(404, &format!("Could not find template with id {id}")),
        },
        ("GET", ["download", "attachments", page, name]) => {
            let name = crate::model::decode(name);
            match ctx
                .all("page_attachments")
                .into_iter()
                .find(|a| s(a, "page") == *page && s(a, "title") == name)
            {
                Some(a) => Out::Bytes(
                    s(&a, "mediaType").to_owned(),
                    s(&a, "content").as_bytes().to_vec(),
                ),
                None => cerr(404, "Attachment not found"),
            }
        }
        ("POST", ["pages", "movepage.action"]) => {
            let id = req.q("pageId").unwrap_or("").to_owned();
            let target = req.q("targetId").unwrap_or("").to_owned();
            let pos = req.q("position").unwrap_or("append").to_owned();
            let (Some(mut p), Some(t)) = (page_doc(ctx, &id), page_doc(ctx, &target)) else {
                return cerr(404, "Page not found");
            };
            match pos.as_str() {
                "above" | "below" => {
                    p["parent"] = t["parent"].clone();
                    p["space"] = t["space"].clone();
                    p["position"] = json!(
                        t["position"].as_i64().unwrap_or(0) + if pos == "above" { -1 } else { 1 }
                    );
                }
                _ => {
                    p["parent"] = json!(target);
                    p["space"] = t["space"].clone();
                }
            }
            ctx.put("pages", &id, p);
            ok(json!({"page": {"id": id}, "status": "success"}))
        }
        _ => cerr(404, "null for uri"),
    }
}

fn req_clone(r: &Req) -> Req {
    Req {
        method: r.method.clone(),
        path: r.path.clone(),
        query: r.query.clone(),
        body: r.body.clone(),
        raw: Vec::new(),
        content_type: r.content_type.clone(),
    }
}

fn template_obj(ctx: &Ctx, t: &Value, body: bool) -> Value {
    let mut o = json!({"templateId": s(t, "id"), "name": s(t, "name"), "description": s(t, "description"), "templateType": "page", "labels": [], "_links": {"self": format!("{}/rest/api/template/{}", wiki(ctx), s(t, "id"))}});
    if !s(t, "space").is_empty() {
        o["space"] = json!({"key": s(t, "space")});
    }
    if body {
        o["body"] = json!({"storage": {"value": s(t, "body"), "representation": "storage"}});
    }
    o
}

fn multipart(req: &Req) -> Vec<(String, Vec<u8>, String, String)> {
    let Some(boundary) = req
        .content_type
        .split("boundary=")
        .nth(1)
        .map(|b| b.trim_matches('"').to_owned())
    else {
        return vec![];
    };
    let delim = format!("--{boundary}");
    let raw = &req.raw;
    let positions: Vec<usize> = raw
        .windows(delim.len())
        .enumerate()
        .filter(|(_, w)| *w == delim.as_bytes())
        .map(|(i, _)| i)
        .collect();
    let mut files = Vec::new();
    let mut comment = String::new();
    for w in positions.windows(2) {
        let part = &raw[w[0] + delim.len()..w[1]];
        let Some(split) = part.windows(4).position(|x| x == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&part[..split]).to_string();
        let mut data = part[split + 4..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        }
        if head.contains("name=\"comment\"") {
            comment = String::from_utf8_lossy(&data).to_string();
            continue;
        }
        let Some(name) = head
            .split("filename=\"")
            .nth(1)
            .and_then(|x| x.split('"').next())
        else {
            continue;
        };
        let ct = head
            .lines()
            .find_map(|l| {
                l.strip_prefix("Content-Type: ")
                    .or(l.strip_prefix("content-type: "))
            })
            .unwrap_or("application/octet-stream")
            .trim()
            .to_owned();
        files.push((name.to_owned(), data, ct, String::new()));
    }
    for f in &mut files {
        f.3 = comment.clone();
    }
    files
}
