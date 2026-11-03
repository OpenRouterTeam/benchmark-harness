//! Store documents → GitHub REST JSON. Stored documents are compact (labels as names,
//! users as logins); these functions expand them into the full API shapes.

use mock_core::world::ServiceState;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    git,
    util::{arr, b, login_of, names_of, node_id, opt_s, s, u},
};

pub fn get<'a>(svc: &'a ServiceState, coll: &str, key: &str) -> Option<&'a Value> {
    svc.store.collection(coll).and_then(|c| c.get(key))
}

pub fn all<'a>(svc: &'a ServiceState, coll: &str) -> Vec<(&'a str, &'a Value)> {
    svc.store
        .collection(coll)
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

pub fn with_prefix<'a>(
    svc: &'a ServiceState,
    coll: &str,
    prefix: &str,
) -> Vec<(&'a str, &'a Value)> {
    all(svc, coll)
        .into_iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .collect()
}

pub fn viewer_login(svc: &ServiceState) -> String {
    get(svc, "viewer", "viewer")
        .map(|v| s(v, "login").to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "mona".to_owned())
}

/// The profile document for a login: `users`, then `orgs`, then the viewer.
pub fn account<'a>(svc: &'a ServiceState, login: &str) -> Option<&'a Value> {
    let lower = login.to_ascii_lowercase();
    let find = |coll: &str| {
        svc.store.collection(coll).and_then(|c| {
            c.get(login).or_else(|| {
                c.iter()
                    .find(|(k, v)| {
                        k.to_ascii_lowercase() == lower
                            || s(v, "login").to_ascii_lowercase() == lower
                    })
                    .map(|(_, v)| v)
            })
        })
    };
    get(svc, "viewer", "viewer")
        .filter(|v| s(v, "login").eq_ignore_ascii_case(login))
        .or_else(|| find("users"))
        .or_else(|| find("orgs"))
}

pub fn is_org(svc: &ServiceState, login: &str) -> bool {
    let lower = login.to_ascii_lowercase();
    svc.store.collection("orgs").is_some_and(|c| {
        c.iter().any(|(k, v)| {
            k.to_ascii_lowercase() == lower || s(v, "login").to_ascii_lowercase() == lower
        })
    })
}

pub fn exists_account(svc: &ServiceState, login: &str) -> bool {
    account(svc, login).is_some()
}

fn synthetic_id(login: &str) -> u64 {
    let d = Sha256::digest(login.as_bytes());
    100_000 + (u64::from(d[0]) << 16 | u64::from(d[1]) << 8 | u64::from(d[2])) % 9_000_000
}

pub fn account_id(svc: &ServiceState, login: &str) -> u64 {
    account(svc, login)
        .map(|a| u(a, "id"))
        .filter(|id| *id != 0)
        .unwrap_or_else(|| synthetic_id(login))
}

pub fn account_type(svc: &ServiceState, login: &str) -> String {
    if is_org(svc, login) {
        return "Organization".into();
    }
    account(svc, login)
        .and_then(|a| opt_s(a, "type"))
        .unwrap_or_else(|| {
            if login.ends_with("[bot]") {
                "Bot".into()
            } else {
                "User".into()
            }
        })
}

pub fn account_node_id(svc: &ServiceState, login: &str) -> String {
    account(svc, login)
        .and_then(|a| opt_s(a, "node_id"))
        .unwrap_or_else(|| {
            let prefix = match account_type(svc, login).as_str() {
                "Organization" => "O",
                "Bot" => "BOT",
                _ => "U",
            };
            node_id(prefix, login)
        })
}

/// Simple user (as embedded in issues, PRs, comments).
pub fn user(svc: &ServiceState, base: &str, login: &str) -> Value {
    if login.is_empty() {
        return Value::Null;
    }
    let login = account(svc, login)
        .map(|a| s(a, "login").to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| login.to_owned());
    let id = account_id(svc, &login);
    let api = format!("{base}/api/v3/users/{login}");
    json!({
        "login": login,
        "id": id,
        "node_id": account_node_id(svc, &login),
        "avatar_url": format!("{base}/avatars/u/{id}?"),
        "gravatar_id": "",
        "url": api,
        "html_url": format!("{base}/{login}"),
        "followers_url": format!("{api}/followers"),
        "following_url": format!("{api}/following{{/other_user}}"),
        "gists_url": format!("{api}/gists{{/gist_id}}"),
        "starred_url": format!("{api}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/subscriptions"),
        "organizations_url": format!("{api}/orgs"),
        "repos_url": format!("{api}/repos"),
        "events_url": format!("{api}/events{{/privacy}}"),
        "received_events_url": format!("{api}/received_events"),
        "type": account_type(svc, &login),
        "site_admin": account(svc, &login).is_some_and(|a| b(a, "site_admin")),
    })
}

pub fn user_ref(svc: &ServiceState, base: &str, v: &Value) -> Value {
    let login = login_of(v);
    if login.is_empty() {
        Value::Null
    } else {
        user(svc, base, &login)
    }
}

/// Full profile (`GET /user`, `GET /users/{login}`).
pub fn user_full(svc: &ServiceState, base: &str, login: &str, private: bool) -> Value {
    let mut out = user(svc, base, login);
    let doc = account(svc, login).cloned().unwrap_or(json!({}));
    let login = s(&out, "login").to_owned();
    let public_repos = all(svc, "repos")
        .iter()
        .filter(|(_, r)| repo_owner(r).eq_ignore_ascii_case(&login) && !b(r, "private"))
        .count();
    let gists = all(svc, "gists")
        .iter()
        .filter(|(_, g)| login_of(g.get("owner").unwrap_or(&Value::Null)) == login)
        .map(|(_, g)| b(g, "public"))
        .collect::<Vec<_>>();
    let field = |k: &str| doc.get(k).cloned().unwrap_or(Value::Null);
    let o = out.as_object_mut().expect("user is an object");
    for k in [
        "name",
        "company",
        "blog",
        "location",
        "email",
        "hireable",
        "bio",
        "twitter_username",
    ] {
        let v = field(k);
        o.insert(
            k.into(),
            if v.is_null() && k == "blog" {
                json!("")
            } else {
                v
            },
        );
    }
    o.insert("public_repos".into(), json!(public_repos));
    o.insert(
        "public_gists".into(),
        json!(gists.iter().filter(|p| **p).count()),
    );
    o.insert("followers".into(), json!(u(&doc, "followers")));
    o.insert("following".into(), json!(u(&doc, "following")));
    o.insert(
        "created_at".into(),
        doc.get("created_at")
            .cloned()
            .unwrap_or(json!("2019-01-01T00:00:00Z")),
    );
    o.insert(
        "updated_at".into(),
        doc.get("updated_at")
            .cloned()
            .unwrap_or(json!("2026-01-01T00:00:00Z")),
    );
    if private {
        let private_repos = all(svc, "repos")
            .iter()
            .filter(|(_, r)| repo_owner(r).eq_ignore_ascii_case(&login) && b(r, "private"))
            .count();
        o.insert(
            "private_gists".into(),
            json!(gists.iter().filter(|p| !**p).count()),
        );
        o.insert("total_private_repos".into(), json!(private_repos));
        o.insert("owned_private_repos".into(), json!(private_repos));
        o.insert("disk_usage".into(), json!(10240));
        o.insert("collaborators".into(), json!(0));
        o.insert("two_factor_authentication".into(), json!(true));
    }
    if s(&out, "type") == "Organization" {
        return org_full(svc, base, &login);
    }
    out
}

pub fn org_simple(svc: &ServiceState, base: &str, login: &str) -> Value {
    let doc = account(svc, login).cloned().unwrap_or(json!({}));
    let login = opt_s(&doc, "login").unwrap_or_else(|| login.to_owned());
    let id = account_id(svc, &login);
    let api = format!("{base}/api/v3/orgs/{login}");
    json!({
        "login": login,
        "id": id,
        "node_id": account_node_id(svc, &login),
        "url": api,
        "repos_url": format!("{api}/repos"),
        "events_url": format!("{api}/events"),
        "hooks_url": format!("{api}/hooks"),
        "issues_url": format!("{api}/issues"),
        "members_url": format!("{api}/members{{/member}}"),
        "public_members_url": format!("{api}/public_members{{/member}}"),
        "avatar_url": format!("{base}/avatars/u/{id}?"),
        "description": doc.get("description").cloned().unwrap_or(Value::Null),
    })
}

pub fn org_full(svc: &ServiceState, base: &str, login: &str) -> Value {
    let doc = account(svc, login).cloned().unwrap_or(json!({}));
    let mut out = org_simple(svc, base, login);
    let login = s(&out, "login").to_owned();
    let o = out.as_object_mut().expect("object");
    for k in ["name", "company", "blog", "location", "email"] {
        o.insert(k.into(), doc.get(k).cloned().unwrap_or(Value::Null));
    }
    o.insert(
        "public_repos".into(),
        json!(
            all(svc, "repos")
                .iter()
                .filter(|(_, r)| repo_owner(r) == login && !b(r, "private"))
                .count()
        ),
    );
    o.insert("followers".into(), json!(0));
    o.insert("following".into(), json!(0));
    o.insert("html_url".into(), json!(format!("{base}/{login}")));
    o.insert(
        "created_at".into(),
        doc.get("created_at")
            .cloned()
            .unwrap_or(json!("2018-01-01T00:00:00Z")),
    );
    o.insert(
        "updated_at".into(),
        doc.get("updated_at")
            .cloned()
            .unwrap_or(json!("2026-01-01T00:00:00Z")),
    );
    o.insert("type".into(), json!("Organization"));
    out
}

// ---------------------------------------------------------------- repositories

pub fn repo_owner(r: &Value) -> String {
    let o = login_of(r.get("owner").unwrap_or(&Value::Null));
    if o.is_empty() {
        s(r, "full_name").split('/').next().unwrap_or("").to_owned()
    } else {
        o
    }
}

/// Case-insensitive repo lookup; returns the canonical key.
pub fn repo_key(svc: &ServiceState, owner: &str, name: &str) -> Option<String> {
    let want = format!("{owner}/{name}").to_ascii_lowercase();
    svc.store.collection("repos").and_then(|c| {
        c.iter()
            .find(|(k, _)| k.to_ascii_lowercase() == want)
            .map(|(k, _)| k.to_owned())
    })
}

pub fn repo_json(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(r) = get(svc, "repos", key) else {
        return Value::Null;
    };
    let (owner, name) = key.split_once('/').unwrap_or((key, key));
    let api = format!("{base}/api/v3/repos/{key}");
    let private = b(r, "private");
    let open_issues = all(svc, "issues")
        .iter()
        .filter(|(k, i)| k.starts_with(&format!("{key}#")) && s(i, "state") == "open")
        .count()
        + all(svc, "pulls")
            .iter()
            .filter(|(k, p)| k.starts_with(&format!("{key}#")) && s(p, "state") == "open")
            .count();
    let owner_json = if is_org(svc, owner) {
        let mut o = user(svc, base, owner);
        o["type"] = json!("Organization");
        o
    } else {
        user(svc, base, owner)
    };
    let mut out = json!({
        "id": u(r, "id"),
        "node_id": opt_s(r, "node_id").unwrap_or_else(|| node_id("R", key)),
        "name": opt_s(r, "name").unwrap_or_else(|| name.to_owned()),
        "full_name": key,
        "private": private,
        "owner": owner_json,
        "html_url": format!("{base}/{key}"),
        "description": r.get("description").cloned().unwrap_or(Value::Null),
        "fork": b(r, "fork"),
        "url": api,
        "forks_url": format!("{api}/forks"),
        "keys_url": format!("{api}/keys{{/key_id}}"),
        "collaborators_url": format!("{api}/collaborators{{/collaborator}}"),
        "teams_url": format!("{api}/teams"),
        "hooks_url": format!("{api}/hooks"),
        "issue_events_url": format!("{api}/issues/events{{/number}}"),
        "events_url": format!("{api}/events"),
        "assignees_url": format!("{api}/assignees{{/user}}"),
        "branches_url": format!("{api}/branches{{/branch}}"),
        "tags_url": format!("{api}/tags"),
        "blobs_url": format!("{api}/git/blobs{{/sha}}"),
        "git_tags_url": format!("{api}/git/tags{{/sha}}"),
        "git_refs_url": format!("{api}/git/refs{{/sha}}"),
        "trees_url": format!("{api}/git/trees{{/sha}}"),
        "statuses_url": format!("{api}/statuses/{{sha}}"),
        "languages_url": format!("{api}/languages"),
        "stargazers_url": format!("{api}/stargazers"),
        "contributors_url": format!("{api}/contributors"),
        "subscribers_url": format!("{api}/subscribers"),
        "subscription_url": format!("{api}/subscription"),
        "commits_url": format!("{api}/commits{{/sha}}"),
        "git_commits_url": format!("{api}/git/commits{{/sha}}"),
        "comments_url": format!("{api}/comments{{/number}}"),
        "issue_comment_url": format!("{api}/issues/comments{{/number}}"),
        "contents_url": format!("{api}/contents/{{+path}}"),
        "compare_url": format!("{api}/compare/{{base}}...{{head}}"),
        "merges_url": format!("{api}/merges"),
        "archive_url": format!("{api}/{{archive_format}}{{/ref}}"),
        "downloads_url": format!("{api}/downloads"),
        "issues_url": format!("{api}/issues{{/number}}"),
        "pulls_url": format!("{api}/pulls{{/number}}"),
        "milestones_url": format!("{api}/milestones{{/number}}"),
        "notifications_url": format!("{api}/notifications{{?since,all,participating}}"),
        "labels_url": format!("{api}/labels{{/name}}"),
        "releases_url": format!("{api}/releases{{/id}}"),
        "deployments_url": format!("{api}/deployments"),
        "created_at": r.get("created_at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
        "updated_at": r.get("updated_at").cloned().unwrap_or(json!("2026-01-01T00:00:00Z")),
        "pushed_at": r.get("pushed_at").cloned().unwrap_or_else(|| r.get("updated_at").cloned().unwrap_or(json!("2026-01-01T00:00:00Z"))),
        "git_url": format!("git://{}/{key}.git", base.split("://").nth(1).unwrap_or(base)),
        "ssh_url": format!("git@{}:{key}.git", base.split("://").nth(1).unwrap_or(base)),
        "clone_url": format!("{base}/{key}.git"),
        "svn_url": format!("{base}/{key}"),
        "homepage": r.get("homepage").cloned().unwrap_or(Value::Null),
        "size": u(r, "size"),
        "stargazers_count": u(r, "stargazers_count"),
        "watchers_count": u(r, "stargazers_count"),
        "language": r.get("language").cloned().unwrap_or(Value::Null),
        "has_issues": r.get("has_issues").cloned().unwrap_or(json!(true)),
        "has_projects": r.get("has_projects").cloned().unwrap_or(json!(true)),
        "has_downloads": true,
        "has_wiki": r.get("has_wiki").cloned().unwrap_or(json!(false)),
        "has_pages": false,
        "has_discussions": r.get("has_discussions").cloned().unwrap_or(json!(false)),
        "forks_count": u(r, "forks_count"),
        "mirror_url": null,
        "archived": b(r, "archived"),
        "disabled": false,
        "open_issues_count": open_issues,
        "license": r.get("license").cloned().unwrap_or(Value::Null),
        "allow_forking": true,
        "is_template": false,
        "web_commit_signoff_required": false,
        "topics": r.get("topics").cloned().unwrap_or(json!([])),
        "visibility": opt_s(r, "visibility").unwrap_or_else(|| if private { "private".into() } else { "public".into() }),
        "forks": u(r, "forks_count"),
        "open_issues": open_issues,
        "watchers": u(r, "stargazers_count"),
        "default_branch": opt_s(r, "default_branch").unwrap_or_else(|| "main".into()),
        "permissions": {"admin": true, "maintain": true, "push": true, "triage": true, "pull": true},
    });
    if let Some(parent) = opt_s(r, "parent")
        && get(svc, "repos", &parent).is_some()
    {
        out["parent"] = repo_json_shallow(svc, base, &parent);
        out["source"] = out["parent"].clone();
    }
    out
}

fn repo_json_shallow(svc: &ServiceState, base: &str, key: &str) -> Value {
    let mut v = repo_json(svc, base, key);
    if let Some(o) = v.as_object_mut() {
        o.remove("parent");
        o.remove("source");
    }
    v
}

/// Minimal repository (as embedded in search results and notifications).
pub fn repo_min(svc: &ServiceState, base: &str, key: &str) -> Value {
    let full = repo_json(svc, base, key);
    if full.is_null() {
        return full;
    }
    let keep = [
        "id",
        "node_id",
        "name",
        "full_name",
        "private",
        "owner",
        "html_url",
        "description",
        "fork",
        "url",
        "created_at",
        "updated_at",
        "pushed_at",
        "stargazers_count",
        "watchers_count",
        "language",
        "forks_count",
        "open_issues_count",
        "default_branch",
        "topics",
        "visibility",
        "archived",
    ];
    let o: Map<String, Value> = full
        .as_object()
        .expect("object")
        .iter()
        .filter(|(k, _)| keep.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::Object(o)
}

// ---------------------------------------------------------------- labels / milestones

pub fn label_doc<'a>(
    svc: &'a ServiceState,
    repo: &str,
    name: &str,
) -> Option<(&'a str, &'a Value)> {
    let lower = name.to_ascii_lowercase();
    with_prefix(svc, "labels", &format!("{repo}:"))
        .into_iter()
        .find(|(_, l)| s(l, "name").to_ascii_lowercase() == lower)
}

pub fn label_json(svc: &ServiceState, base: &str, repo: &str, name: &str) -> Value {
    let doc = label_doc(svc, repo, name).map(|(_, d)| d.clone());
    let key = format!("{repo}:{name}");
    let d = doc.unwrap_or(json!({"name": name}));
    let real_name = opt_s(&d, "name").unwrap_or_else(|| name.to_owned());
    let id = match u(&d, "id") {
        0 => synthetic_id(&key),
        id => id,
    };
    json!({
        "id": id,
        "node_id": opt_s(&d, "node_id").unwrap_or_else(|| node_id("LA", &format!("{repo}:{real_name}"))),
        "url": format!("{base}/api/v3/repos/{repo}/labels/{}", crate::util::percent_encode_path(&real_name)),
        "name": real_name,
        "color": opt_s(&d, "color").unwrap_or_else(|| "ededed".into()),
        "default": b(&d, "default"),
        "description": d.get("description").cloned().unwrap_or(Value::Null),
    })
}

pub fn milestone_json(svc: &ServiceState, base: &str, repo: &str, number: u64) -> Value {
    let Some(m) = get(svc, "milestones", &format!("{repo}:{number}")) else {
        return Value::Null;
    };
    let api = format!("{base}/api/v3/repos/{repo}/milestones/{number}");
    let (open, closed) = issues_in_milestone(svc, repo, number);
    json!({
        "url": api,
        "html_url": format!("{base}/{repo}/milestone/{number}"),
        "labels_url": format!("{api}/labels"),
        "id": u(m, "id"),
        "node_id": opt_s(m, "node_id").unwrap_or_else(|| node_id("MI", &format!("{repo}:{number}"))),
        "number": number,
        "title": s(m, "title"),
        "description": m.get("description").cloned().unwrap_or(Value::Null),
        "creator": user_ref(svc, base, m.get("creator").unwrap_or(&Value::Null)),
        "open_issues": open,
        "closed_issues": closed,
        "state": opt_s(m, "state").unwrap_or_else(|| "open".into()),
        "created_at": m.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": m.get("updated_at").cloned().unwrap_or(Value::Null),
        "due_on": m.get("due_on").cloned().unwrap_or(Value::Null),
        "closed_at": m.get("closed_at").cloned().unwrap_or(Value::Null),
    })
}

fn issues_in_milestone(svc: &ServiceState, repo: &str, number: u64) -> (usize, usize) {
    let prefix = format!("{repo}#");
    let mut open = 0;
    let mut closed = 0;
    for coll in ["issues", "pulls"] {
        for (_, i) in with_prefix(svc, coll, &prefix) {
            if milestone_number(i) == Some(number) {
                if s(i, "state") == "open" {
                    open += 1;
                } else {
                    closed += 1;
                }
            }
        }
    }
    (open, closed)
}

pub fn milestone_number(i: &Value) -> Option<u64> {
    match i.get("milestone") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::Object(o)) => o.get("number").and_then(Value::as_u64),
        _ => None,
    }
}

// ---------------------------------------------------------------- issues

/// Repository and number of an issue/PR key `owner/repo#N`.
pub fn split_key(key: &str) -> (&str, u64) {
    let (repo, n) = key.rsplit_once('#').unwrap_or((key, "0"));
    (repo, n.parse().unwrap_or(0))
}

pub fn comment_count(svc: &ServiceState, repo: &str, number: u64) -> usize {
    all(svc, "issue_comments")
        .iter()
        .filter(|(_, c)| s(c, "repo") == repo && u(c, "issue_number") == number)
        .count()
}

pub fn reactions_summary(svc: &ServiceState, base: &str, subject: &str, url: &str) -> Value {
    let mut counts: Map<String, Value> = Map::new();
    let kinds = [
        "+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes",
    ];
    let reacts: Vec<String> = all(svc, "reactions")
        .iter()
        .filter(|(_, r)| s(r, "subject") == subject)
        .map(|(_, r)| s(r, "content").to_owned())
        .collect();
    counts.insert("url".into(), json!(format!("{url}/reactions")));
    counts.insert("total_count".into(), json!(reacts.len()));
    for k in kinds {
        counts.insert(k.into(), json!(reacts.iter().filter(|r| *r == k).count()));
    }
    let _ = base;
    Value::Object(counts)
}

pub fn issue_type_json(svc: &ServiceState, owner: &str, name: &str) -> Value {
    let lower = name.to_ascii_lowercase();
    let doc = with_prefix(svc, "issue_types", &format!("{owner}:"))
        .into_iter()
        .find(|(_, t)| s(t, "name").to_ascii_lowercase() == lower)
        .map(|(_, t)| t.clone());
    match doc {
        Some(t) => issue_type_out(owner, &t),
        None => {
            json!({"id": synthetic_id(name), "node_id": node_id("IT", &format!("{owner}:{name}")), "name": name, "description": null, "color": "gray"})
        }
    }
}

pub fn issue_type_out(owner: &str, t: &Value) -> Value {
    json!({
        "id": u(t, "id"),
        "node_id": opt_s(t, "node_id").unwrap_or_else(|| node_id("IT", &format!("{owner}:{}", s(t, "name")))),
        "name": s(t, "name"),
        "description": t.get("description").cloned().unwrap_or(Value::Null),
        "color": opt_s(t, "color").unwrap_or_else(|| "gray".into()),
        "is_enabled": t.get("is_enabled").cloned().unwrap_or(json!(true)),
        "created_at": t.get("created_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
        "updated_at": t.get("updated_at").cloned().unwrap_or(json!("2025-01-01T00:00:00Z")),
    })
}

pub fn issue_json(svc: &ServiceState, base: &str, key: &str) -> Value {
    if let Some(i) = get(svc, "issues", key) {
        return issue_doc_json(svc, base, key, i, None);
    }
    if let Some(p) = get(svc, "pulls", key) {
        return issue_doc_json(svc, base, key, p, Some(p));
    }
    Value::Null
}

/// Issue representation of an issue or a pull request document.
pub fn issue_doc_json(
    svc: &ServiceState,
    base: &str,
    key: &str,
    i: &Value,
    pr: Option<&Value>,
) -> Value {
    let (repo, number) = split_key(key);
    let api_repo = format!("{base}/api/v3/repos/{repo}");
    let url = format!("{api_repo}/issues/{number}");
    let html = if pr.is_some() {
        format!("{base}/{repo}/pull/{number}")
    } else {
        format!("{base}/{repo}/issues/{number}")
    };
    let labels: Vec<Value> = names_of(arr(i, "labels"))
        .iter()
        .map(|n| label_json(svc, base, repo, n))
        .collect();
    let assignees: Vec<Value> = names_of(arr(i, "assignees"))
        .iter()
        .map(|l| user(svc, base, l))
        .collect();
    let milestone = milestone_number(i)
        .map(|m| milestone_json(svc, base, repo, m))
        .unwrap_or(Value::Null);
    let owner = repo.split('/').next().unwrap_or("");
    let issue_type = opt_s(i, "type")
        .map(|t| issue_type_json(svc, owner, &t))
        .unwrap_or(Value::Null);
    let author = login_of(i.get("user").unwrap_or(&Value::Null));
    let subject = format!("issue:{key}");
    let mut out = json!({
        "url": url,
        "repository_url": api_repo,
        "labels_url": format!("{url}/labels{{/name}}"),
        "comments_url": format!("{url}/comments"),
        "events_url": format!("{url}/events"),
        "html_url": html,
        "id": u(i, "id"),
        "node_id": issue_node_id(key, i, pr.is_some()),
        "number": number,
        "title": s(i, "title"),
        "user": user(svc, base, &author),
        "labels": labels,
        "state": opt_s(i, "state").unwrap_or_else(|| "open".into()),
        "locked": b(i, "locked"),
        "assignee": assignees.first().cloned().unwrap_or(Value::Null),
        "assignees": assignees,
        "milestone": milestone,
        "comments": comment_count(svc, repo, number),
        "created_at": i.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": i.get("updated_at").cloned().unwrap_or(Value::Null),
        "closed_at": i.get("closed_at").cloned().unwrap_or(Value::Null),
        "author_association": opt_s(i, "author_association").unwrap_or_else(|| association(svc, repo, &author)),
        "active_lock_reason": null,
        "body": i.get("body").cloned().unwrap_or(Value::Null),
        "closed_by": i.get("closed_by").map(|c| user_ref(svc, base, c)).unwrap_or(Value::Null),
        "reactions": reactions_summary(svc, base, &subject, &url),
        "timeline_url": format!("{url}/timeline"),
        "performed_via_github_app": null,
        "state_reason": i.get("state_reason").cloned().unwrap_or(Value::Null),
        "type": issue_type,
    });
    if pr.is_none() {
        let subs = sub_issue_keys(i);
        if !subs.is_empty() || i.get("parent").is_some() {
            let done = subs
                .iter()
                .filter(|k| get(svc, "issues", k).is_some_and(|d| s(d, "state") == "closed"))
                .count();
            out["sub_issues_summary"] = json!({
                "total": subs.len(),
                "completed": done,
                "percent_completed": (done * 100).checked_div(subs.len()).unwrap_or(0),
            });
        }
    }
    if let Some(p) = pr {
        out["draft"] = json!(b(p, "draft"));
        out["pull_request"] = json!({
            "url": format!("{api_repo}/pulls/{number}"),
            "html_url": format!("{base}/{repo}/pull/{number}"),
            "diff_url": format!("{base}/{repo}/pull/{number}.diff"),
            "patch_url": format!("{base}/{repo}/pull/{number}.patch"),
            "merged_at": p.get("merged_at").cloned().unwrap_or(Value::Null),
        });
    }
    out
}

pub fn issue_node_id(key: &str, i: &Value, is_pr: bool) -> String {
    opt_s(i, "node_id").unwrap_or_else(|| node_id(if is_pr { "PR" } else { "I" }, key))
}

pub fn sub_issue_keys(i: &Value) -> Vec<String> {
    names_of(arr(i, "sub_issues"))
}

pub fn association(svc: &ServiceState, repo: &str, login: &str) -> String {
    let owner = repo.split('/').next().unwrap_or("");
    if login.eq_ignore_ascii_case(owner) {
        return "OWNER".into();
    }
    if is_org(svc, owner)
        && let Some(o) = account(svc, owner)
        && names_of(arr(o, "members"))
            .iter()
            .any(|m| m.eq_ignore_ascii_case(login))
    {
        return "MEMBER".into();
    }
    if let Some(r) = get(svc, "repos", repo)
        && arr(r, "collaborators")
            .iter()
            .any(|c| login_of(c).eq_ignore_ascii_case(login))
    {
        return "COLLABORATOR".into();
    }
    "CONTRIBUTOR".into()
}

pub fn issue_comment_json(svc: &ServiceState, base: &str, c: &Value) -> Value {
    let repo = s(c, "repo");
    let id = u(c, "id");
    let number = u(c, "issue_number");
    let url = format!("{base}/api/v3/repos/{repo}/issues/comments/{id}");
    let is_pr = get(svc, "pulls", &format!("{repo}#{number}")).is_some();
    let html_kind = if is_pr { "pull" } else { "issues" };
    let author = login_of(c.get("user").unwrap_or(&Value::Null));
    json!({
        "id": id,
        "node_id": opt_s(c, "node_id").unwrap_or_else(|| node_id("IC", &id.to_string())),
        "url": url,
        "html_url": format!("{base}/{repo}/{html_kind}/{number}#issuecomment-{id}"),
        "body": s(c, "body"),
        "user": user(svc, base, &author),
        "created_at": c.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": c.get("updated_at").cloned().or_else(|| c.get("created_at").cloned()).unwrap_or(Value::Null),
        "issue_url": format!("{base}/api/v3/repos/{repo}/issues/{number}"),
        "author_association": association(svc, repo, &author),
        "reactions": reactions_summary(svc, base, &format!("issue_comment:{id}"), &url),
        "performed_via_github_app": null,
    })
}

// ---------------------------------------------------------------- pull requests

pub fn pr_head_ref(p: &Value) -> String {
    match p.get("head") {
        Some(Value::String(r)) => r.clone(),
        Some(Value::Object(o)) => o
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => String::new(),
    }
}

pub fn pr_base_ref(p: &Value) -> String {
    match p.get("base") {
        Some(Value::String(r)) => r.clone(),
        Some(Value::Object(o)) => o
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_owned(),
        _ => "main".into(),
    }
}

/// Repo holding the head branch (forks set `head.repo`).
pub fn pr_head_repo(repo: &str, p: &Value) -> String {
    p.get("head")
        .and_then(|h| h.get("repo"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| repo.to_owned())
}

pub fn pr_head_sha(svc: &ServiceState, repo: &str, p: &Value) -> String {
    if let Some(sha) = p
        .get("head")
        .and_then(|h| h.get("sha"))
        .and_then(Value::as_str)
    {
        return sha.to_owned();
    }
    if let Some(sha) = opt_s(p, "head_sha") {
        return sha;
    }
    let head_repo = pr_head_repo(repo, p);
    git::branch_sha(svc, &head_repo, &pr_head_ref(p)).unwrap_or_default()
}

pub fn pr_base_sha(svc: &ServiceState, repo: &str, p: &Value) -> String {
    if let Some(sha) = p
        .get("base")
        .and_then(|h| h.get("sha"))
        .and_then(Value::as_str)
    {
        return sha.to_owned();
    }
    if let Some(sha) = opt_s(p, "base_sha") {
        return sha;
    }
    git::branch_sha(svc, repo, &pr_base_ref(p)).unwrap_or_default()
}

/// Changes and commits of a PR (merge base → head).
pub fn pr_changes(svc: &ServiceState, repo: &str, p: &Value) -> (Vec<git::FileChange>, Vec<Value>) {
    let head = pr_head_sha(svc, repo, p);
    let base_sha = pr_base_sha(svc, repo, p);
    let head_repo = pr_head_repo(repo, p);
    let commits_repo = if git::commit(svc, &head_repo, &head).is_some() {
        head_repo
    } else {
        repo.to_owned()
    };
    // PR commits may be recorded explicitly (e.g. after the head branch was deleted).
    let mb = git::merge_base(svc, &commits_repo, &base_sha, &head)
        .or_else(|| git::merge_base(svc, repo, &base_sha, &head))
        .unwrap_or_else(|| base_sha.clone());
    let old = git::file_map(svc, &commits_repo, &mb)
        .or_else(|| git::file_map(svc, repo, &mb))
        .unwrap_or_default();
    let new = git::file_map(svc, &commits_repo, &head).unwrap_or_default();
    let changes = git::diff_maps(&old, &new, &std::collections::BTreeMap::new());
    let commits = git::commits_between(svc, &commits_repo, &mb, &head);
    (changes, commits)
}

pub fn branch_json(svc: &ServiceState, base: &str, repo: &str, branch: &str, sha: &str) -> Value {
    let r = get(svc, "repos", repo);
    let owner = repo.split('/').next().unwrap_or("");
    let _ = r;
    json!({
        "label": format!("{owner}:{branch}"),
        "ref": branch,
        "sha": sha,
        "user": user(svc, base, owner),
        "repo": repo_json(svc, base, repo),
    })
}

pub fn pr_json(svc: &ServiceState, base: &str, key: &str) -> Value {
    let Some(p) = get(svc, "pulls", key) else {
        return Value::Null;
    };
    let (repo, number) = split_key(key);
    let api = format!("{base}/api/v3/repos/{repo}/pulls/{number}");
    let issue_api = format!("{base}/api/v3/repos/{repo}/issues/{number}");
    let head_repo = pr_head_repo(repo, p);
    let head_sha = pr_head_sha(svc, repo, p);
    let base_sha = pr_base_sha(svc, repo, p);
    let (changes, commits) = pr_changes(svc, repo, p);
    let labels: Vec<Value> = names_of(arr(p, "labels"))
        .iter()
        .map(|n| label_json(svc, base, repo, n))
        .collect();
    let assignees: Vec<Value> = names_of(arr(p, "assignees"))
        .iter()
        .map(|l| user(svc, base, l))
        .collect();
    let reviewers: Vec<Value> = names_of(arr(p, "requested_reviewers"))
        .iter()
        .map(|l| user(svc, base, l))
        .collect();
    let owner = repo.split('/').next().unwrap_or("");
    let teams: Vec<Value> = names_of(arr(p, "requested_teams"))
        .iter()
        .map(|t| team_json(svc, base, owner, t))
        .collect();
    let author = login_of(p.get("user").unwrap_or(&Value::Null));
    let merged = b(p, "merged") || p.get("merged_at").is_some_and(|m| !m.is_null());
    let state = opt_s(p, "state").unwrap_or_else(|| "open".into());
    let mergeable_state = opt_s(p, "mergeable_state").unwrap_or_else(|| {
        if merged || state == "closed" {
            "unknown".into()
        } else if b(p, "draft") {
            "draft".into()
        } else {
            "clean".into()
        }
    });
    let mergeable = p.get("mergeable").cloned().unwrap_or_else(|| {
        if merged {
            Value::Null
        } else {
            json!(mergeable_state != "dirty")
        }
    });
    json!({
        "url": api,
        "id": u(p, "id"),
        "node_id": issue_node_id(key, p, true),
        "html_url": format!("{base}/{repo}/pull/{number}"),
        "diff_url": format!("{base}/{repo}/pull/{number}.diff"),
        "patch_url": format!("{base}/{repo}/pull/{number}.patch"),
        "issue_url": issue_api,
        "commits_url": format!("{api}/commits"),
        "review_comments_url": format!("{api}/comments"),
        "review_comment_url": format!("{base}/api/v3/repos/{repo}/pulls/comments{{/number}}"),
        "comments_url": format!("{issue_api}/comments"),
        "statuses_url": format!("{base}/api/v3/repos/{repo}/statuses/{head_sha}"),
        "number": number,
        "state": state,
        "locked": b(p, "locked"),
        "title": s(p, "title"),
        "user": user(svc, base, &author),
        "body": p.get("body").cloned().unwrap_or(Value::Null),
        "labels": labels,
        "milestone": milestone_number(p).map(|m| milestone_json(svc, base, repo, m)).unwrap_or(Value::Null),
        "active_lock_reason": null,
        "created_at": p.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": p.get("updated_at").cloned().unwrap_or(Value::Null),
        "closed_at": p.get("closed_at").cloned().unwrap_or(Value::Null),
        "merged_at": p.get("merged_at").cloned().unwrap_or(Value::Null),
        "merge_commit_sha": p.get("merge_commit_sha").cloned().unwrap_or(Value::Null),
        "assignee": assignees.first().cloned().unwrap_or(Value::Null),
        "assignees": assignees,
        "requested_reviewers": reviewers,
        "requested_teams": teams,
        "head": branch_json(svc, base, &head_repo, &pr_head_ref(p), &head_sha),
        "base": branch_json(svc, base, repo, &pr_base_ref(p), &base_sha),
        "_links": {
            "self": {"href": api},
            "html": {"href": format!("{base}/{repo}/pull/{number}")},
            "issue": {"href": issue_api},
            "comments": {"href": format!("{issue_api}/comments")},
            "review_comments": {"href": format!("{api}/comments")},
            "commits": {"href": format!("{api}/commits")},
            "statuses": {"href": format!("{base}/api/v3/repos/{repo}/statuses/{head_sha}")},
        },
        "author_association": association(svc, repo, &author),
        "auto_merge": null,
        "draft": b(p, "draft"),
        "merged": merged,
        "mergeable": mergeable,
        "rebaseable": mergeable,
        "mergeable_state": mergeable_state,
        "merged_by": p.get("merged_by").map(|m| user_ref(svc, base, m)).unwrap_or(Value::Null),
        "comments": comment_count(svc, repo, number),
        "review_comments": all(svc, "review_comments").iter().filter(|(_, c)| s(c, "repo") == repo && u(c, "pull_number") == number && s(c, "state") != "pending").count(),
        "maintainer_can_modify": b(p, "maintainer_can_modify"),
        "commits": commits.len(),
        "additions": changes.iter().map(|c| c.additions).sum::<usize>(),
        "deletions": changes.iter().map(|c| c.deletions).sum::<usize>(),
        "changed_files": changes.len(),
    })
}

/// A pull request in list form (no counts / mergeability).
pub fn pr_list_json(svc: &ServiceState, base: &str, key: &str) -> Value {
    let mut v = pr_json(svc, base, key);
    if let Some(o) = v.as_object_mut() {
        for k in [
            "merged",
            "mergeable",
            "rebaseable",
            "mergeable_state",
            "merged_by",
            "comments",
            "review_comments",
            "maintainer_can_modify",
            "commits",
            "additions",
            "deletions",
            "changed_files",
        ] {
            o.remove(k);
        }
    }
    v
}

pub fn team_json(svc: &ServiceState, base: &str, org: &str, slug: &str) -> Value {
    let doc = get(svc, "teams", &format!("{org}/{slug}"))
        .cloned()
        .unwrap_or(json!({"slug": slug, "name": slug}));
    let id = match u(&doc, "id") {
        0 => synthetic_id(&format!("{org}/{slug}")),
        id => id,
    };
    let api = format!(
        "{base}/api/v3/organizations/{}/team/{id}",
        account_id(svc, org)
    );
    json!({
        "id": id,
        "node_id": opt_s(&doc, "node_id").unwrap_or_else(|| node_id("T", &format!("{org}/{slug}"))),
        "url": api,
        "html_url": format!("{base}/orgs/{org}/teams/{slug}"),
        "name": opt_s(&doc, "name").unwrap_or_else(|| slug.to_owned()),
        "slug": slug,
        "description": doc.get("description").cloned().unwrap_or(Value::Null),
        "privacy": opt_s(&doc, "privacy").unwrap_or_else(|| "closed".into()),
        "notification_setting": "notifications_enabled",
        "permission": opt_s(&doc, "permission").unwrap_or_else(|| "push".into()),
        "members_url": format!("{api}/members{{/member}}"),
        "repositories_url": format!("{api}/repos"),
        "parent": null,
    })
}

// ---------------------------------------------------------------- commits

pub fn git_person(p: Option<&Value>) -> Value {
    let p = p.cloned().unwrap_or(json!({}));
    json!({
        "name": opt_s(&p, "name").unwrap_or_else(|| "GitHub Enterprise".into()),
        "email": opt_s(&p, "email").unwrap_or_else(|| "noreply@github.octo-corp.com".into()),
        "date": p.get("date").cloned().unwrap_or(Value::Null),
    })
}

pub fn commit_login(svc: &ServiceState, doc: &Value, role: &str) -> Option<String> {
    if let Some(l) = opt_s(doc, &format!("{role}_login")) {
        return Some(l);
    }
    let email = doc
        .get(role)
        .and_then(|p| p.get("email"))
        .and_then(Value::as_str)?
        .to_ascii_lowercase();
    all(svc, "users")
        .iter()
        .find(|(_, u)| s(u, "email").to_ascii_lowercase() == email)
        .map(|(_, u)| s(u, "login").to_owned())
}

/// `git/commits/{sha}` shape.
pub fn git_commit_json(svc: &ServiceState, base: &str, repo: &str, doc: &Value) -> Value {
    let sha = s(doc, "sha");
    let tree = git::file_map(svc, repo, sha)
        .map(|m| git::tree_sha(&m, ""))
        .unwrap_or_default();
    let api = format!("{base}/api/v3/repos/{repo}");
    json!({
        "sha": sha,
        "node_id": node_id("C", &format!("{repo}@{sha}")),
        "url": format!("{api}/git/commits/{sha}"),
        "html_url": format!("{base}/{repo}/commit/{sha}"),
        "author": git_person(doc.get("author")),
        "committer": git_person(doc.get("committer").or_else(|| doc.get("author"))),
        "tree": {"sha": tree, "url": format!("{api}/git/trees/{tree}")},
        "message": s(doc, "message"),
        "parents": git::parents(doc).iter().map(|p| json!({"sha": p, "url": format!("{api}/git/commits/{p}"), "html_url": format!("{base}/{repo}/commit/{p}")})).collect::<Vec<_>>(),
        "verification": {"verified": false, "reason": "unsigned", "signature": null, "payload": null, "verified_at": null},
    })
}

pub fn file_change_json(base: &str, repo: &str, sha: &str, c: &git::FileChange) -> Value {
    let path = crate::util::percent_encode_path(&c.filename);
    let mut v = json!({
        "sha": c.sha,
        "filename": c.filename,
        "status": c.status,
        "additions": c.additions,
        "deletions": c.deletions,
        "changes": c.additions + c.deletions,
        "blob_url": format!("{base}/{repo}/blob/{sha}/{path}"),
        "raw_url": format!("{base}/{repo}/raw/{sha}/{path}"),
        "contents_url": format!("{base}/api/v3/repos/{repo}/contents/{path}?ref={sha}"),
    });
    if !c.patch.is_empty() {
        v["patch"] = json!(c.patch);
    }
    if let Some(p) = &c.previous_filename {
        v["previous_filename"] = json!(p);
    }
    v
}

/// `repos/{o}/{r}/commits/{sha}` shape (optionally with stats and files).
pub fn commit_json(
    svc: &ServiceState,
    base: &str,
    repo: &str,
    doc: &Value,
    with_files: bool,
) -> Value {
    let sha = s(doc, "sha");
    let api = format!("{base}/api/v3/repos/{repo}");
    let git_commit = git_commit_json(svc, base, repo, doc);
    let author = commit_login(svc, doc, "author")
        .map(|l| user(svc, base, &l))
        .unwrap_or(Value::Null);
    let committer = commit_login(svc, doc, "committer")
        .map(|l| user(svc, base, &l))
        .unwrap_or(Value::Null);
    let mut out = json!({
        "sha": sha,
        "node_id": node_id("C", &format!("{repo}@{sha}")),
        "commit": {
            "author": git_commit["author"],
            "committer": git_commit["committer"],
            "message": git_commit["message"],
            "tree": git_commit["tree"],
            "url": git_commit["url"],
            "comment_count": 0,
            "verification": git_commit["verification"],
        },
        "url": format!("{api}/commits/{sha}"),
        "html_url": format!("{base}/{repo}/commit/{sha}"),
        "comments_url": format!("{api}/commits/{sha}/comments"),
        "author": author,
        "committer": committer,
        "parents": git::parents(doc).iter().map(|p| json!({"sha": p, "url": format!("{api}/commits/{p}"), "html_url": format!("{base}/{repo}/commit/{p}")})).collect::<Vec<_>>(),
    });
    if with_files {
        let changes = git::commit_changes(svc, repo, sha);
        let adds: usize = changes.iter().map(|c| c.additions).sum();
        let dels: usize = changes.iter().map(|c| c.deletions).sum();
        out["stats"] = json!({"total": adds + dels, "additions": adds, "deletions": dels});
        out["files"] = Value::Array(
            changes
                .iter()
                .map(|c| file_change_json(base, repo, sha, c))
                .collect(),
        );
    }
    out
}
