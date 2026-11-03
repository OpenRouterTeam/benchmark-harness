//! Users, organizations, teams, notifications, gists, and stars.

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{
    Ctx,
    render::{
        self, account, all, get, is_org, org_full, org_simple, repo_key, repo_min, team_json, user,
        user_full,
    },
    util::{
        arr, b, error, login_of, names_of, next_id, no_content, not_found, ok, opt_s, paged, s,
        status_json, u, unix, validation,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let get_ = m == Method::GET;
    Some(match segs {
        ["user"] if get_ => {
            let v = ctx.viewer();
            ok(user_full(ctx.svc, ctx.base(), &v, true))
        }
        ["users", login] if get_ => match account(ctx.svc, login) {
            Some(_) => {
                if is_org(ctx.svc, login) {
                    ok(org_full(ctx.svc, ctx.base(), login))
                } else {
                    let private = login.eq_ignore_ascii_case(&ctx.viewer());
                    ok(user_full(ctx.svc, ctx.base(), login, private))
                }
            }
            None => not_found(),
        },
        ["user", "orgs"] if get_ => {
            let v = ctx.viewer();
            orgs_of(ctx, &v)
        }
        ["users", login, "orgs"] if get_ => orgs_of(ctx, login),
        ["orgs", org] if get_ => {
            if is_org(ctx.svc, org) {
                ok(org_full(ctx.svc, ctx.base(), org))
            } else {
                not_found()
            }
        }
        ["orgs", org, "members"] if get_ => org_members(ctx, org),
        ["orgs", org, "teams"] if get_ => {
            if !is_org(ctx.svc, org) {
                return Some(not_found());
            }
            let teams: Vec<Value> = render::with_prefix(ctx.svc, "teams", &format!("{org}/"))
                .iter()
                .map(|(_, t)| team_json(ctx.svc, ctx.base(), org, s(t, "slug")))
                .collect();
            paged(ctx.req, teams)
        }
        ["orgs", org, "teams", slug] if get_ => {
            match get(ctx.svc, "teams", &format!("{org}/{slug}")) {
                Some(_) => ok(team_json(ctx.svc, ctx.base(), org, slug)),
                None => not_found(),
            }
        }
        ["orgs", org, "teams", slug, "members"] if get_ => {
            match get(ctx.svc, "teams", &format!("{org}/{slug}")) {
                Some(t) => {
                    let members: Vec<Value> = names_of(arr(t, "members"))
                        .iter()
                        .map(|l| user(ctx.svc, ctx.base(), l))
                        .collect();
                    paged(ctx.req, members)
                }
                None => not_found(),
            }
        }
        ["user", "teams"] if get_ => {
            let v = ctx.viewer();
            let teams: Vec<Value> = all(ctx.svc, "teams")
                .iter()
                .filter(|(_, t)| names_of(arr(t, "members")).contains(&v))
                .map(|(k, t)| {
                    let org = k.split('/').next().unwrap_or("");
                    let mut tj = team_json(ctx.svc, ctx.base(), org, s(t, "slug"));
                    tj["organization"] = org_simple(ctx.svc, ctx.base(), org);
                    tj
                })
                .collect();
            paged(ctx.req, teams)
        }

        // ------------------------------------------------ notifications
        ["notifications"] if get_ => list_notifications(ctx, None),
        ["notifications"] if m == Method::PUT => mark_all_read(ctx, None),
        ["repos", o, r, "notifications"] if get_ => {
            let Some(key) = repo_key(ctx.svc, o, r) else {
                return Some(not_found());
            };
            list_notifications(ctx, Some(&key))
        }
        ["repos", o, r, "notifications"] if m == Method::PUT => {
            let Some(key) = repo_key(ctx.svc, o, r) else {
                return Some(not_found());
            };
            mark_all_read(ctx, Some(&key))
        }
        ["notifications", "threads", id] => thread(ctx, id, &m),
        ["notifications", "threads", id, "subscription"] => thread_subscription(ctx, id, &m),
        ["repos", o, r, "subscription"] => {
            let Some(key) = repo_key(ctx.svc, o, r) else {
                return Some(not_found());
            };
            repo_subscription(ctx, &key, &m)
        }

        // ------------------------------------------------ gists
        ["gists"] if get_ => {
            let v = ctx.viewer();
            list_gists(ctx, &v, true)
        }
        ["gists", "public"] if get_ => list_gists(ctx, "", false),
        ["users", login, "gists"] if get_ => {
            if account(ctx.svc, login).is_none() {
                return Some(not_found());
            }
            let own = login.eq_ignore_ascii_case(&ctx.viewer());
            list_gists(ctx, login, own)
        }
        ["gists"] if m == Method::POST => create_gist(ctx),
        ["gists", id] if get_ => match get(ctx.svc, "gists", id) {
            Some(_) => ok(gist_json(ctx, id, true)),
            None => not_found(),
        },
        ["gists", id] if m == Method::PATCH => update_gist(ctx, id),

        // ------------------------------------------------ stars
        ["user", "starred"] if get_ => {
            let v = ctx.viewer();
            starred(ctx, &v)
        }
        ["users", login, "starred"] if get_ => {
            if account(ctx.svc, login).is_none() {
                return Some(not_found());
            }
            starred(ctx, login)
        }
        ["user", "starred", o, r] => star(ctx, o, r, &m),
        ["repos", o, r, "stargazers"] if get_ => {
            let Some(key) = repo_key(ctx.svc, o, r) else {
                return Some(not_found());
            };
            let users: Vec<Value> = all(ctx.svc, "stars")
                .iter()
                .filter(|(_, st)| s(st, "repo") == key)
                .map(|(_, st)| user(ctx.svc, ctx.base(), s(st, "user")))
                .collect();
            paged(ctx.req, users)
        }
        ["meta"] if get_ => ok(json!({
            "verifiable_password_authentication": true,
            "installed_version": "3.17.0",
            "hooks": [],
            "git": [],
            "pages": [],
        })),
        _ => return None,
    })
}

fn orgs_of(ctx: &Ctx, login: &str) -> Response {
    if account(ctx.svc, login).is_none() {
        return not_found();
    }
    let orgs: Vec<Value> = all(ctx.svc, "orgs")
        .iter()
        .filter(|(_, o)| {
            names_of(arr(o, "members"))
                .iter()
                .any(|m| m.eq_ignore_ascii_case(login))
        })
        .map(|(k, o)| {
            org_simple(
                ctx.svc,
                ctx.base(),
                opt_s(o, "login").as_deref().unwrap_or(k),
            )
        })
        .collect();
    paged(ctx.req, orgs)
}

fn org_members(ctx: &Ctx, org: &str) -> Response {
    let Some(o) = account(ctx.svc, org).filter(|_| is_org(ctx.svc, org)) else {
        return not_found();
    };
    let members: Vec<Value> = names_of(arr(o, "members"))
        .iter()
        .map(|l| user(ctx.svc, ctx.base(), l))
        .collect();
    paged(ctx.req, members)
}

// ---------------------------------------------------------------- notifications

fn notification_json(ctx: &Ctx, n: &Value) -> Value {
    let id = opt_s(n, "id").unwrap_or_else(|| u(n, "id").to_string());
    let repo = s(n, "repo");
    let api = ctx.req.api();
    let subject = n.get("subject").cloned().unwrap_or(json!({}));
    let kind = opt_s(&subject, "type").unwrap_or_else(|| "Issue".into());
    let number = u(&subject, "number");
    let subject_url = match kind.as_str() {
        "PullRequest" => json!(format!("{api}/repos/{repo}/pulls/{number}")),
        "Issue" => json!(format!("{api}/repos/{repo}/issues/{number}")),
        "Release" => json!(format!("{api}/repos/{repo}/releases/{number}")),
        "Commit" => json!(format!("{api}/repos/{repo}/commits/{}", s(&subject, "sha"))),
        _ => subject.get("url").cloned().unwrap_or(Value::Null),
    };
    json!({
        "id": id,
        "unread": n.get("unread").cloned().unwrap_or(json!(true)),
        "reason": opt_s(n, "reason").unwrap_or_else(|| "subscribed".into()),
        "updated_at": n.get("updated_at").cloned().unwrap_or(Value::Null),
        "last_read_at": n.get("last_read_at").cloned().unwrap_or(Value::Null),
        "subject": {
            "title": s(&subject, "title"),
            "url": subject_url,
            "latest_comment_url": subject_url,
            "type": kind,
        },
        "repository": repo_min(ctx.svc, ctx.base(), repo),
        "url": format!("{api}/notifications/threads/{id}"),
        "subscription_url": format!("{api}/notifications/threads/{id}/subscription"),
    })
}

fn visible_notification(n: &Value) -> bool {
    !b(n, "done")
}

fn list_notifications(ctx: &Ctx, repo: Option<&str>) -> Response {
    let include_read = ctx.req.q("all") == Some("true");
    let participating = ctx.req.q("participating") == Some("true");
    let since = ctx.req.q("since").and_then(unix);
    let before = ctx.req.q("before").and_then(unix);
    let mut items: Vec<&Value> = all(ctx.svc, "notifications")
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| visible_notification(n))
        .filter(|n| repo.is_none_or(|r| s(n, "repo") == r))
        .filter(|n| include_read || n.get("unread").and_then(Value::as_bool).unwrap_or(true))
        .filter(|n| {
            !participating
                || matches!(
                    s(n, "reason"),
                    "author"
                        | "comment"
                        | "mention"
                        | "review_requested"
                        | "assign"
                        | "state_change"
                        | "team_mention"
                )
        })
        .filter(|n| {
            let t = unix(s(n, "updated_at")).unwrap_or(0);
            since.is_none_or(|x| t >= x) && before.is_none_or(|x| t < x)
        })
        .collect();
    items.sort_by_key(|n| std::cmp::Reverse(unix(s(n, "updated_at")).unwrap_or(0)));
    let out: Vec<Value> = items.iter().map(|n| notification_json(ctx, n)).collect();
    paged(ctx.req, out)
}

fn find_notification(ctx: &Ctx, id: &str) -> Option<String> {
    all(ctx.svc, "notifications")
        .iter()
        .find(|(k, n)| {
            *k == id || opt_s(n, "id").as_deref() == Some(id) || u(n, "id").to_string() == id
        })
        .map(|(k, _)| (*k).to_owned())
}

fn thread(ctx: &mut Ctx, id: &str, m: &Method) -> Response {
    let Some(key) = find_notification(ctx, id) else {
        return not_found();
    };
    let now = ctx.now.clone();
    match *m {
        Method::GET => {
            let n = get(ctx.svc, "notifications", &key)
                .cloned()
                .unwrap_or_default();
            if !visible_notification(&n) {
                return not_found();
            }
            ok(notification_json(ctx, &n))
        }
        Method::PATCH => {
            if let Some(n) = ctx.svc.store.collection_mut("notifications").get_mut(&key) {
                n["unread"] = json!(false);
                n["last_read_at"] = json!(now);
            }
            status_json(StatusCode::RESET_CONTENT, Value::Null)
        }
        Method::DELETE => {
            if let Some(n) = ctx.svc.store.collection_mut("notifications").get_mut(&key) {
                n["unread"] = json!(false);
                n["done"] = json!(true);
                n["last_read_at"] = json!(now);
            }
            no_content()
        }
        _ => not_found(),
    }
}

fn mark_all_read(ctx: &mut Ctx, repo: Option<&str>) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let last_read = opt_s(&body, "last_read_at").unwrap_or_else(|| ctx.now.clone());
    let cutoff = unix(&last_read).unwrap_or(i64::MAX);
    let read = body.get("read").and_then(Value::as_bool).unwrap_or(true);
    let keys: Vec<String> = all(ctx.svc, "notifications")
        .iter()
        .filter(|(_, n)| repo.is_none_or(|r| s(n, "repo") == r))
        .filter(|(_, n)| unix(s(n, "updated_at")).unwrap_or(0) <= cutoff)
        .map(|(k, _)| (*k).to_owned())
        .collect();
    let coll = ctx.svc.store.collection_mut("notifications");
    for k in keys {
        if let Some(n) = coll.get_mut(&k) {
            n["unread"] = json!(!read);
            n["last_read_at"] = json!(last_read);
        }
    }
    status_json(StatusCode::RESET_CONTENT, Value::Null)
}

fn subscription_json(thread_url: &str, sub: &Value, created: &str) -> Value {
    json!({
        "subscribed": sub.get("subscribed").cloned().unwrap_or(json!(true)),
        "ignored": sub.get("ignored").cloned().unwrap_or(json!(false)),
        "reason": sub.get("reason").cloned().unwrap_or(Value::Null),
        "created_at": sub.get("created_at").cloned().unwrap_or(json!(created)),
        "url": format!("{thread_url}/subscription"),
        "thread_url": thread_url,
        "repository_url": sub.get("repository_url").cloned().unwrap_or(Value::Null),
    })
}

fn thread_subscription(ctx: &mut Ctx, id: &str, m: &Method) -> Response {
    let Some(key) = find_notification(ctx, id) else {
        return not_found();
    };
    let api = ctx.req.api();
    let thread_url = format!("{api}/notifications/threads/{id}");
    let now = ctx.now.clone();
    match *m {
        Method::GET => {
            let n = get(ctx.svc, "notifications", &key)
                .cloned()
                .unwrap_or_default();
            match n.get("subscription") {
                Some(sub) if !sub.is_null() => ok(subscription_json(&thread_url, sub, &now)),
                _ => not_found(),
            }
        }
        Method::PUT => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let ignored = b(&body, "ignored");
            let sub = json!({"subscribed": !ignored, "ignored": ignored, "reason": null, "created_at": now});
            if let Some(n) = ctx.svc.store.collection_mut("notifications").get_mut(&key) {
                n["subscription"] = sub.clone();
            }
            ok(subscription_json(&thread_url, &sub, &now))
        }
        Method::DELETE => {
            if let Some(n) = ctx.svc.store.collection_mut("notifications").get_mut(&key) {
                n["subscription"] = Value::Null;
            }
            no_content()
        }
        _ => not_found(),
    }
}

fn repo_subscription(ctx: &mut Ctx, key: &str, m: &Method) -> Response {
    let api = ctx.req.api();
    let repo_url = format!("{api}/repos/{key}");
    let now = ctx.now.clone();
    let render = |sub: &Value| {
        json!({
            "subscribed": sub.get("subscribed").cloned().unwrap_or(json!(true)),
            "ignored": sub.get("ignored").cloned().unwrap_or(json!(false)),
            "reason": null,
            "created_at": sub.get("created_at").cloned().unwrap_or(json!(now)),
            "url": format!("{repo_url}/subscription"),
            "repository_url": repo_url,
        })
    };
    match *m {
        Method::GET => match get(ctx.svc, "repo_subscriptions", key) {
            Some(sub) => ok(render(sub)),
            None => not_found(),
        },
        Method::PUT => {
            let body = match ctx.req.json() {
                Ok(b) => b,
                Err(e) => return e,
            };
            let ignored = b(&body, "ignored");
            let subscribed = body
                .get("subscribed")
                .and_then(Value::as_bool)
                .unwrap_or(!ignored);
            let sub = json!({"repo": key, "subscribed": subscribed, "ignored": ignored, "created_at": now});
            ctx.svc
                .store
                .collection_mut("repo_subscriptions")
                .insert(key, sub.clone());
            ok(render(&sub))
        }
        Method::DELETE => {
            ctx.svc
                .store
                .collection_mut("repo_subscriptions")
                .remove(key);
            no_content()
        }
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- gists

fn gist_json(ctx: &Ctx, id: &str, with_content: bool) -> Value {
    let g = get(ctx.svc, "gists", id).cloned().unwrap_or_default();
    let api = ctx.req.api();
    let owner = login_of(g.get("owner").unwrap_or(&Value::Null));
    let mut files = Map::new();
    if let Some(fs) = g.get("files").and_then(Value::as_object) {
        for (name, f) in fs {
            let content = s(f, "content");
            let mut fj = json!({
                "filename": name,
                "type": "text/plain",
                "language": f.get("language").cloned().unwrap_or_else(|| language_of(name)),
                "raw_url": format!("{}/gist/{owner}/{id}/raw/{name}", ctx.base()),
                "size": content.len(),
            });
            if with_content {
                fj["truncated"] = json!(false);
                fj["content"] = json!(content);
            }
            files.insert(name.clone(), fj);
        }
    }
    json!({
        "url": format!("{api}/gists/{id}"),
        "forks_url": format!("{api}/gists/{id}/forks"),
        "commits_url": format!("{api}/gists/{id}/commits"),
        "id": id,
        "node_id": crate::util::node_id("G", id),
        "git_pull_url": format!("{}/gist/{id}.git", ctx.base()),
        "git_push_url": format!("{}/gist/{id}.git", ctx.base()),
        "html_url": format!("{}/gist/{owner}/{id}", ctx.base()),
        "files": files,
        "public": b(&g, "public"),
        "created_at": g.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": g.get("updated_at").cloned().unwrap_or(Value::Null),
        "description": g.get("description").cloned().unwrap_or(Value::Null),
        "comments": u(&g, "comments"),
        "user": null,
        "comments_url": format!("{api}/gists/{id}/comments"),
        "owner": user(ctx.svc, ctx.base(), &owner),
        "truncated": false,
    })
}

fn language_of(name: &str) -> Value {
    let ext = name.rsplit('.').next().unwrap_or("");
    let lang = match ext {
        "md" => "Markdown",
        "rs" => "Rust",
        "go" => "Go",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "sh" => "Shell",
        "yml" | "yaml" => "YAML",
        "json" => "JSON",
        "sql" => "SQL",
        "txt" => "Text",
        _ => return Value::Null,
    };
    json!(lang)
}

fn list_gists(ctx: &Ctx, owner: &str, include_secret: bool) -> Response {
    let since = ctx.req.q("since").and_then(unix);
    let mut items: Vec<(&str, &Value)> = all(ctx.svc, "gists")
        .into_iter()
        .filter(|(_, g)| {
            owner.is_empty()
                || login_of(g.get("owner").unwrap_or(&Value::Null)).eq_ignore_ascii_case(owner)
        })
        .filter(|(_, g)| include_secret || b(g, "public"))
        .filter(|(_, g)| since.is_none_or(|x| unix(s(g, "updated_at")).unwrap_or(0) >= x))
        .collect();
    items.sort_by_key(|(_, g)| std::cmp::Reverse(unix(s(g, "created_at")).unwrap_or(0)));
    let out: Vec<Value> = items
        .iter()
        .map(|(k, _)| gist_json(ctx, k, false))
        .collect();
    paged(ctx.req, out)
}

fn gist_id(ctx: &Ctx) -> String {
    let n = all(ctx.svc, "gists").len() as u64 + 1;
    crate::sha1::sha1_hex(format!("gist {} {}", ctx.now, n).as_bytes())[..32].to_owned()
}

fn create_gist(ctx: &mut Ctx) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(files) = body
        .get("files")
        .and_then(Value::as_object)
        .filter(|f| !f.is_empty())
    else {
        return validation("Gist", "files", "missing_field");
    };
    let mut stored = Map::new();
    for (name, f) in files {
        let content = s(f, "content");
        if content.is_empty() {
            return validation("Gist", "files", "missing_field");
        }
        stored.insert(name.clone(), json!({"content": content}));
    }
    let id = gist_id(ctx);
    let doc = json!({
        "id": id,
        "description": body.get("description").cloned().unwrap_or(Value::Null),
        "public": b(&body, "public"),
        "owner": ctx.viewer(),
        "files": stored,
        "created_at": ctx.now,
        "updated_at": ctx.now,
        "comments": 0,
    });
    ctx.svc
        .store
        .collection_mut("gists")
        .insert(id.clone(), doc);
    crate::util::created(gist_json(ctx, &id, true))
}

fn update_gist(ctx: &mut Ctx, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(mut g) = get(ctx.svc, "gists", id).cloned() else {
        return not_found();
    };
    if login_of(g.get("owner").unwrap_or(&Value::Null)) != ctx.viewer() {
        return not_found();
    }
    if let Some(d) = body.get("description") {
        g["description"] = d.clone();
    }
    if let Some(files) = body.get("files").and_then(Value::as_object) {
        let mut current = g
            .get("files")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (name, f) in files {
            if f.is_null() {
                current.remove(name);
                continue;
            }
            let target = opt_s(f, "filename").unwrap_or_else(|| name.clone());
            let content = f
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| current.get(name).map(|old| s(old, "content").to_owned()))
                .unwrap_or_default();
            if target != *name {
                current.remove(name);
            }
            current.insert(target, json!({"content": content}));
        }
        if current.is_empty() {
            return validation("Gist", "files", "missing_field");
        }
        g["files"] = Value::Object(current);
    }
    g["updated_at"] = json!(ctx.now);
    ctx.svc.store.collection_mut("gists").insert(id, g);
    ok(gist_json(ctx, id, true))
}

// ---------------------------------------------------------------- stars

fn starred(ctx: &Ctx, login: &str) -> Response {
    let mut stars: Vec<&Value> = all(ctx.svc, "stars")
        .into_iter()
        .map(|(_, st)| st)
        .filter(|st| s(st, "user").eq_ignore_ascii_case(login))
        .filter(|st| get(ctx.svc, "repos", s(st, "repo")).is_some())
        .collect();
    let sort = ctx.req.q("sort").unwrap_or("created");
    let asc = ctx.req.q("direction") == Some("asc");
    match sort {
        "updated" => stars.sort_by_key(|st| {
            get(ctx.svc, "repos", s(st, "repo"))
                .map(|r| unix(s(r, "updated_at")).unwrap_or(0))
                .unwrap_or(0)
        }),
        _ => stars.sort_by_key(|st| unix(s(st, "starred_at")).unwrap_or(0)),
    }
    if !asc {
        stars.reverse();
    }
    // `application/vnd.github.star+json` wraps each repository with its starred_at.
    let with_dates = ctx.req.accept().contains("star+json");
    let out: Vec<Value> = stars
        .iter()
        .map(|st| {
            let repo = render::repo_json(ctx.svc, ctx.base(), s(st, "repo"));
            if with_dates {
                json!({"starred_at": st.get("starred_at").cloned().unwrap_or(Value::Null), "repo": repo})
            } else {
                repo
            }
        })
        .collect();
    paged(ctx.req, out)
}

fn star(ctx: &mut Ctx, o: &str, r: &str, m: &Method) -> Response {
    let Some(key) = repo_key(ctx.svc, o, r) else {
        return not_found();
    };
    let v = ctx.viewer();
    let skey = format!("{v}|{key}");
    match *m {
        Method::GET => {
            if get(ctx.svc, "stars", &skey).is_some() {
                no_content()
            } else {
                not_found()
            }
        }
        Method::PUT => {
            if get(ctx.svc, "stars", &skey).is_none() {
                let doc = json!({"user": v, "repo": key, "starred_at": ctx.now});
                ctx.svc.store.collection_mut("stars").insert(skey, doc);
                if let Some(rd) = ctx.svc.store.collection_mut("repos").get_mut(&key) {
                    rd["stargazers_count"] = json!(u(rd, "stargazers_count") + 1);
                }
            }
            no_content()
        }
        Method::DELETE => {
            if ctx
                .svc
                .store
                .collection_mut("stars")
                .remove(&skey)
                .is_some()
                && let Some(rd) = ctx.svc.store.collection_mut("repos").get_mut(&key)
            {
                rd["stargazers_count"] = json!(u(rd, "stargazers_count").saturating_sub(1));
            }
            no_content()
        }
        _ => error(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed"),
    }
}

#[allow(dead_code)]
pub fn next_notification_id(ctx: &Ctx) -> u64 {
    next_id(ctx.svc, "notifications", "id", 1)
}
