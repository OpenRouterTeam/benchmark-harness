//! Pull requests: CRUD, files, commits, diffs, merging, branch updates, reviewers,
//! reviews, and review comments.

use std::collections::BTreeMap;

use axum::{
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    Ctx, git,
    issues::{Target, delete_reaction, next_issue_db_id, next_number, reactions},
    render::{
        self, account, all, file_change_json, get, issue_json, pr_base_ref, pr_changes,
        pr_head_ref, pr_head_repo, pr_head_sha, pr_json, pr_list_json, repo_key, split_key,
        team_json, user, with_prefix,
    },
    repos::viewer_person,
    util::{
        arr, b, created, error, login_of, names_of, next_id, no_content, node_id, not_found, ok,
        opt_s, paged, s, status_json, str_list, u, unix, unprocessable, validation, validation_msg,
        with_header,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    let get_ = m == Method::GET;
    let ["repos", o, r, "pulls", rest @ ..] = segs else {
        return None;
    };
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return Some(not_found());
    };
    Some(match rest {
        [] if get_ => list_pulls(ctx, &repo),
        [] if m == Method::POST => create_pull(ctx, &repo),
        ["comments"] if get_ => {
            let out: Vec<Value> = all(ctx.svc, "review_comments")
                .iter()
                .filter(|(_, c)| s(c, "repo") == repo && !is_pending(ctx.svc, c))
                .map(|(_, c)| review_comment_json(ctx, c))
                .collect();
            paged(ctx.req, out)
        }
        ["comments", id] if get_ => match get(ctx.svc, "review_comments", id) {
            Some(c) if s(c, "repo") == repo => ok(review_comment_json(ctx, c)),
            _ => not_found(),
        },
        ["comments", id] if m == Method::PATCH => update_review_comment(ctx, &repo, id),
        ["comments", id] if m == Method::DELETE => match get(ctx.svc, "review_comments", id) {
            Some(c) if s(c, "repo") == repo => {
                ctx.svc.store.collection_mut("review_comments").remove(id);
                no_content()
            }
            _ => not_found(),
        },
        ["comments", id, "reactions"] => reactions(
            ctx,
            &repo,
            &format!("pr_comment:{id}"),
            Target::PullComment(id),
            &m,
        ),
        ["comments", id, "reactions", rid] if m == Method::DELETE => {
            delete_reaction(ctx, &format!("pr_comment:{id}"), rid)
        }
        [n, rest @ ..] => {
            let key = format!("{repo}#{n}");
            if get(ctx.svc, "pulls", &key).is_none() {
                return Some(not_found());
            }
            match rest {
                [] if get_ => get_pull(ctx, &repo, &key),
                [] if m == Method::PATCH => update_pull(ctx, &repo, &key),
                ["files"] if get_ => files(ctx, &repo, &key),
                ["commits"] if get_ => commits(ctx, &repo, &key),
                ["merge"] if get_ => {
                    if is_merged(get(ctx.svc, "pulls", &key).unwrap_or(&Value::Null)) {
                        no_content()
                    } else {
                        not_found()
                    }
                }
                ["merge"] if m == Method::PUT => merge(ctx, &repo, &key),
                ["update-branch"] if m == Method::PUT => update_branch(ctx, &repo, &key),
                ["requested_reviewers"] if get_ => {
                    let p = get(ctx.svc, "pulls", &key).cloned().unwrap_or_default();
                    let owner = repo.split('/').next().unwrap_or("");
                    ok(json!({
                        "users": names_of(arr(&p, "requested_reviewers")).iter().map(|l| user(ctx.svc, ctx.base(), l)).collect::<Vec<_>>(),
                        "teams": names_of(arr(&p, "requested_teams")).iter().map(|t| team_json(ctx.svc, ctx.base(), owner, t)).collect::<Vec<_>>(),
                    }))
                }
                ["requested_reviewers"] if m == Method::POST => request_reviewers(ctx, &repo, &key),
                ["requested_reviewers"] if m == Method::DELETE => {
                    remove_reviewers(ctx, &repo, &key)
                }
                ["reviews"] if get_ => {
                    let out: Vec<Value> = reviews_of(ctx.svc, &key)
                        .iter()
                        .filter(|r| {
                            s(r, "state") != "PENDING"
                                || login_of(r.get("user").unwrap_or(&Value::Null)) == ctx.viewer()
                        })
                        .map(|r| review_json(ctx, r))
                        .collect();
                    paged(ctx.req, out)
                }
                ["reviews"] if m == Method::POST => create_review(ctx, &repo, &key),
                ["reviews", id] if get_ => match review_doc(ctx.svc, &key, id) {
                    Some(r) => ok(review_json(ctx, &r)),
                    None => not_found(),
                },
                ["reviews", id] if m == Method::PUT => update_review(ctx, &key, id),
                ["reviews", id] if m == Method::DELETE => delete_review(ctx, &key, id),
                ["reviews", id, "events"] if m == Method::POST => submit_review(ctx, &key, id),
                ["reviews", id, "comments"] if get_ => {
                    let rid = id.parse::<u64>().unwrap_or(0);
                    let out: Vec<Value> = all(ctx.svc, "review_comments")
                        .iter()
                        .filter(|(_, c)| u(c, "review_id") == rid)
                        .map(|(_, c)| review_comment_json(ctx, c))
                        .collect();
                    paged(ctx.req, out)
                }
                ["comments"] if get_ => {
                    let (_, number) = split_key(&key);
                    let mut cs: Vec<&Value> = all(ctx.svc, "review_comments")
                        .into_iter()
                        .map(|(_, c)| c)
                        .filter(|c| {
                            s(c, "repo") == repo
                                && u(c, "pull_number") == number
                                && !is_pending(ctx.svc, c)
                        })
                        .collect();
                    cs.sort_by_key(|c| u(c, "id"));
                    let out: Vec<Value> = cs.iter().map(|c| review_comment_json(ctx, c)).collect();
                    paged(ctx.req, out)
                }
                ["comments"] if m == Method::POST => create_review_comment(ctx, &repo, &key),
                ["comments", id, "replies"] if m == Method::POST => reply(ctx, &repo, &key, id),
                _ => not_found(),
            }
        }
        _ => not_found(),
    })
}

fn is_merged(p: &Value) -> bool {
    b(p, "merged") || p.get("merged_at").is_some_and(|m| !m.is_null())
}

// ---------------------------------------------------------------- list / get / create / update

fn list_pulls(ctx: &Ctx, repo: &str) -> Response {
    let state = ctx.req.q("state").unwrap_or("open");
    let head = ctx.req.q("head");
    let base = ctx.req.q("base");
    let mut items: Vec<(&str, &Value)> = with_prefix(ctx.svc, "pulls", &format!("{repo}#"))
        .into_iter()
        .filter(|(_, p)| state == "all" || s(p, "state") == state)
        .filter(|(_, p)| {
            head.is_none_or(|h| {
                let (owner, branch) = h.split_once(':').unwrap_or(("", h));
                let head_repo = pr_head_repo(repo, p);
                let head_owner = head_repo.split('/').next().unwrap_or("");
                pr_head_ref(p) == branch
                    && (owner.is_empty() || owner.eq_ignore_ascii_case(head_owner))
            })
        })
        .filter(|(_, p)| base.is_none_or(|bs| pr_base_ref(p) == bs))
        .collect();
    let sort = ctx.req.q("sort").unwrap_or("created");
    let direction = ctx.req.q("direction").unwrap_or(if sort == "long-running" {
        "asc"
    } else {
        "desc"
    });
    match sort {
        "updated" => items.sort_by_key(|(_, p)| unix(s(p, "updated_at")).unwrap_or(0)),
        "popularity" => items.sort_by_key(|(k, _)| {
            let (r, n) = split_key(k);
            render::comment_count(ctx.svc, r, n)
        }),
        _ => items.sort_by_key(|(_, p)| (unix(s(p, "created_at")).unwrap_or(0), u(p, "number"))),
    }
    if direction != "asc" {
        items.reverse();
    }
    let out: Vec<Value> = items
        .iter()
        .map(|(k, _)| pr_list_json(ctx.svc, ctx.base(), k))
        .collect();
    paged(ctx.req, out)
}

fn get_pull(ctx: &Ctx, repo: &str, key: &str) -> Response {
    let accept = ctx.req.accept();
    if accept.contains("diff") || accept.contains("patch") {
        let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
        let (changes, commits) = pr_changes(ctx.svc, repo, &p);
        let text = if accept.contains("patch") {
            let mut out = String::new();
            for c in &commits {
                let sha = s(c, "sha");
                out.push_str(&format!(
                    "From {sha} Mon Sep 17 00:00:00 2001\nFrom: {} <{}>\nDate: {}\nSubject: [PATCH] {}\n\n---\n",
                    c.get("author").and_then(|a| a.get("name")).and_then(Value::as_str).unwrap_or(""),
                    c.get("author").and_then(|a| a.get("email")).and_then(Value::as_str).unwrap_or(""),
                    git::commit_date(c),
                    s(c, "message").lines().next().unwrap_or("")
                ));
                out.push_str(&git::diff_text(&git::commit_changes(
                    ctx.svc,
                    &pr_head_repo(repo, &p),
                    sha,
                )));
                out.push('\n');
            }
            out
        } else {
            git::diff_text(&changes)
        };
        let ct = if accept.contains("patch") {
            "application/vnd.github.v3.patch; charset=utf-8"
        } else {
            "application/vnd.github.v3.diff; charset=utf-8"
        };
        return (StatusCode::OK, [(header::CONTENT_TYPE, ct)], text).into_response();
    }
    ok(pr_json(ctx.svc, ctx.base(), key))
}

fn create_pull(ctx: &mut Ctx, repo: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let head = s(&body, "head").to_owned();
    let base = s(&body, "base").to_owned();
    if head.is_empty() {
        return validation("PullRequest", "head", "missing_field");
    }
    if base.is_empty() {
        return validation("PullRequest", "base", "missing_field");
    }
    // An existing issue can be converted (`issue`), otherwise a title is required.
    let title = opt_s(&body, "title");
    if title.is_none() && body.get("issue").is_none() {
        return validation("PullRequest", "title", "missing_field");
    }
    let (head_owner, head_branch) = head
        .split_once(':')
        .map(|(o, b)| (o.to_owned(), b.to_owned()))
        .unwrap_or_else(|| {
            (
                repo.split('/').next().unwrap_or("").to_owned(),
                head.clone(),
            )
        });
    let repo_name = repo.split('/').nth(1).unwrap_or("");
    let head_repo = if head_owner.eq_ignore_ascii_case(repo.split('/').next().unwrap_or("")) {
        repo.to_owned()
    } else {
        // A fork of this repository owned by `head_owner`.
        all(ctx.svc, "repos")
            .iter()
            .find(|(k, d)| {
                k.split('/')
                    .next()
                    .is_some_and(|o| o.eq_ignore_ascii_case(&head_owner))
                    && (opt_s(d, "parent").as_deref() == Some(repo)
                        || k.ends_with(&format!("/{repo_name}")))
            })
            .map(|(k, _)| (*k).to_owned())
            .unwrap_or_default()
    };
    let Some(head_sha) = git::branch_sha(ctx.svc, &head_repo, &head_branch) else {
        return validation("PullRequest", "head", "invalid");
    };
    let Some(base_sha) = git::branch_sha(ctx.svc, repo, &base) else {
        return validation("PullRequest", "base", "invalid");
    };
    if with_prefix(ctx.svc, "pulls", &format!("{repo}#"))
        .iter()
        .any(|(_, p)| {
            s(p, "state") == "open"
                && pr_head_ref(p) == head_branch
                && pr_head_repo(repo, p) == head_repo
                && pr_base_ref(p) == base
        })
    {
        return validation_msg(
            "PullRequest",
            &format!("A pull request already exists for {head_owner}:{head_branch}."),
        );
    }
    let ahead = git::commits_between(ctx.svc, &head_repo, &base_sha, &head_sha);
    if ahead.is_empty() {
        return validation_msg(
            "PullRequest",
            &format!("No commits between {base} and {head_branch}"),
        );
    }
    let number = next_number(ctx.svc, repo);
    let id = next_issue_db_id(ctx.svc);
    let key = format!("{repo}#{number}");
    let head_val = if head_repo == repo {
        json!(head_branch)
    } else {
        json!({"ref": head_branch, "repo": head_repo})
    };
    let doc = json!({
        "id": id,
        "node_id": ctx.svc.ids.next_prefixed("pr_node", "PR_kwDO", 16),
        "number": number,
        "title": title.unwrap_or_default(),
        "body": body.get("body").cloned().unwrap_or(Value::Null),
        "user": ctx.viewer(),
        "state": "open",
        "draft": b(&body, "draft"),
        "head": head_val,
        "base": base,
        "labels": [],
        "assignees": [],
        "requested_reviewers": [],
        "requested_teams": [],
        "maintainer_can_modify": b(&body, "maintainer_can_modify"),
        "merged": false,
        "created_at": ctx.now,
        "updated_at": ctx.now,
        "closed_at": null,
        "merged_at": null,
    });
    ctx.svc
        .store
        .collection_mut("pulls")
        .insert(key.clone(), doc);
    let v = pr_json(ctx.svc, ctx.base(), &key);
    let url = s(&v, "url").to_owned();
    with_header(created(v), header::LOCATION, &url)
}

fn update_pull(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    if let Some(t) = body.get("title") {
        match t.as_str() {
            Some(t) if !t.trim().is_empty() => p["title"] = json!(t),
            _ => return validation("PullRequest", "title", "missing_field"),
        }
    }
    if let Some(bd) = body.get("body") {
        p["body"] = bd.clone();
    }
    if let Some(mcm) = body.get("maintainer_can_modify") {
        p["maintainer_can_modify"] = mcm.clone();
    }
    if let Some(base) = opt_s(&body, "base") {
        if is_merged(&p) || s(&p, "state") == "closed" {
            return validation_msg(
                "PullRequest",
                "Cannot change the base branch of a closed pull request.",
            );
        }
        if git::branch_sha(ctx.svc, repo, &base).is_none() {
            return validation("PullRequest", "base", "invalid");
        }
        p["base"] = json!(base);
    }
    if let Some(state) = opt_s(&body, "state") {
        match state.as_str() {
            "closed" => {
                if s(&p, "state") != "closed" {
                    p["state"] = json!("closed");
                    p["closed_at"] = json!(ctx.now);
                }
            }
            "open" => {
                if is_merged(&p) {
                    return validation_msg("PullRequest", "Cannot reopen a merged pull request.");
                }
                if s(&p, "state") == "closed" {
                    let head_repo = pr_head_repo(repo, &p);
                    if git::branch_sha(ctx.svc, &head_repo, &pr_head_ref(&p)).is_none() {
                        return validation_msg(
                            "PullRequest",
                            "Cannot reopen this pull request because the head branch was deleted.",
                        );
                    }
                }
                p["state"] = json!("open");
                p["closed_at"] = Value::Null;
            }
            _ => return validation("PullRequest", "state", "invalid"),
        }
    }
    p["updated_at"] = json!(ctx.now);
    ctx.svc.store.collection_mut("pulls").insert(key, p);
    ok(pr_json(ctx.svc, ctx.base(), key))
}

fn files(ctx: &Ctx, repo: &str, key: &str) -> Response {
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let head = pr_head_sha(ctx.svc, repo, &p);
    let (changes, _) = pr_changes(ctx.svc, repo, &p);
    let head_repo = pr_head_repo(repo, &p);
    let out: Vec<Value> = changes
        .iter()
        .map(|c| file_change_json(ctx.base(), &head_repo, &head, c))
        .collect();
    paged(ctx.req, out)
}

fn commits(ctx: &Ctx, repo: &str, key: &str) -> Response {
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let (_, commits) = pr_changes(ctx.svc, repo, &p);
    let head_repo = pr_head_repo(repo, &p);
    let out: Vec<Value> = commits
        .iter()
        .map(|c| render::commit_json(ctx.svc, ctx.base(), &head_repo, c, false))
        .collect();
    paged(ctx.req, out)
}

// ---------------------------------------------------------------- merge / update-branch

fn approvals(svc: &ServiceState, key: &str) -> usize {
    // Latest review state per reviewer.
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    for r in reviews_of(svc, key) {
        let st = s(&r, "state");
        if st == "PENDING" || st == "COMMENTED" {
            continue;
        }
        latest.insert(
            login_of(r.get("user").unwrap_or(&Value::Null)),
            st.to_owned(),
        );
    }
    latest.values().filter(|s| *s == "APPROVED").count()
}

fn changes_requested(svc: &ServiceState, key: &str) -> bool {
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    for r in reviews_of(svc, key) {
        let st = s(&r, "state");
        if st == "PENDING" || st == "COMMENTED" {
            continue;
        }
        latest.insert(
            login_of(r.get("user").unwrap_or(&Value::Null)),
            st.to_owned(),
        );
    }
    latest.values().any(|s| s == "CHANGES_REQUESTED")
}

/// Required approvals from the repository's branch protection (dataset `branches.*.required_approvals`).
fn required_approvals(svc: &ServiceState, repo: &str, base: &str) -> usize {
    get(svc, "branches", &format!("{repo}:{base}"))
        .map(|b| u(b, "required_approvals") as usize)
        .unwrap_or(0)
}

fn merge(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let (_, number) = split_key(key);
    if is_merged(&p) || s(&p, "state") != "open" {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Pull Request is not mergeable",
        );
    }
    if b(&p, "draft") {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Pull Request is still a draft",
        );
    }
    let head_sha = pr_head_sha(ctx.svc, repo, &p);
    if let Some(expected) = opt_s(&body, "sha")
        && expected != head_sha
    {
        return error(
            StatusCode::CONFLICT,
            "Head branch was modified. Review and try the merge again.",
        );
    }
    let state = s(&p, "mergeable_state");
    if state == "dirty" || p.get("mergeable") == Some(&json!(false)) {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Pull Request is not mergeable",
        );
    }
    let base_ref = pr_base_ref(&p);
    let required = required_approvals(ctx.svc, repo, &base_ref);
    if changes_requested(ctx.svc, key) && required > 0 {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Changes must be made before merging",
        );
    }
    if approvals(ctx.svc, key) < required {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            &format!(
                "At least {required} approving review{} is required by reviewers with write access.",
                if required == 1 { "" } else { "s" }
            ),
        );
    }
    if state == "blocked" && approvals(ctx.svc, key) == 0 {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "At least 1 approving review is required by reviewers with write access.",
        );
    }
    let method = opt_s(&body, "merge_method").unwrap_or_else(|| "merge".into());
    if !["merge", "squash", "rebase"].contains(&method.as_str()) {
        return validation("PullRequest", "merge_method", "invalid");
    }
    if let Some(r) = get(ctx.svc, "repos", repo) {
        let allowed = match method.as_str() {
            "squash" => r.get("allow_squash_merge"),
            "rebase" => r.get("allow_rebase_merge"),
            _ => r.get("allow_merge_commit"),
        };
        if allowed == Some(&json!(false)) {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                &format!(
                    "{} merges are not allowed on this repository.",
                    match method.as_str() {
                        "squash" => "Squash",
                        "rebase" => "Rebase",
                        _ => "Merge commits",
                    }
                ),
            );
        }
    }
    let Some(base_head) = git::branch_sha(ctx.svc, repo, &base_ref) else {
        return error(StatusCode::METHOD_NOT_ALLOWED, "Base branch was deleted");
    };
    let head_repo = pr_head_repo(repo, &p);
    let (changes, commits) = pr_changes(ctx.svc, repo, &p);
    let head_map = git::file_map(ctx.svc, &head_repo, &head_sha).unwrap_or_default();
    let mut map = git::file_map(ctx.svc, repo, &base_head).unwrap_or_default();
    for c in &changes {
        match head_map.get(&c.filename) {
            Some(e) => {
                map.insert(c.filename.clone(), e.clone());
            }
            None => {
                map.remove(&c.filename);
            }
        }
        if let Some(prev) = &c.previous_filename {
            map.remove(prev);
        }
    }
    // Make sure head commits are visible in the base repo (forks).
    if head_repo != repo {
        let docs: Vec<(String, Value)> = with_prefix(ctx.svc, "commits", &format!("{head_repo}@"))
            .into_iter()
            .map(|(k, v)| (k.replacen(&head_repo, repo, 1), v.clone()))
            .collect();
        for (k, v) in docs {
            if ctx
                .svc
                .store
                .collection("commits")
                .is_none_or(|c| !c.contains(&k))
            {
                ctx.svc.store.collection_mut("commits").insert(k, v);
            }
        }
    }
    let title = s(&p, "title").to_owned();
    let viewer = ctx.viewer();
    let author = viewer_person(ctx);
    let now = ctx.now.clone();
    let head_label = format!(
        "{}/{}",
        head_repo.split('/').next().unwrap_or(""),
        pr_head_ref(&p)
    );
    let sha = match method.as_str() {
        "squash" => {
            let t = opt_s(&body, "commit_title").unwrap_or_else(|| format!("{title} (#{number})"));
            let msg = match opt_s(&body, "commit_message") {
                Some(m) => format!("{t}\n\n{m}"),
                None => {
                    let lines: Vec<String> = commits
                        .iter()
                        .map(|c| format!("* {}", s(c, "message").lines().next().unwrap_or("")))
                        .collect();
                    format!("{t}\n\n{}", lines.join("\n"))
                }
            };
            git::write_commit(
                ctx.svc,
                repo,
                std::slice::from_ref(&base_head),
                &map,
                &msg,
                &author,
                Some(&viewer),
                &now,
            )
        }
        "rebase" => {
            let mut parent = base_head.clone();
            let mut cur = git::file_map(ctx.svc, repo, &base_head).unwrap_or_default();
            for c in &commits {
                let csha = s(c, "sha").to_owned();
                let cmap = git::file_map(ctx.svc, repo, &csha).unwrap_or_default();
                for ch in git::commit_changes(ctx.svc, repo, &csha) {
                    match cmap.get(&ch.filename) {
                        Some(e) => {
                            cur.insert(ch.filename.clone(), e.clone());
                        }
                        None => {
                            cur.remove(&ch.filename);
                        }
                    }
                }
                let a = c.get("author").cloned().unwrap_or_else(|| author.clone());
                let login = render::commit_login(ctx.svc, c, "author");
                parent = git::write_commit(
                    ctx.svc,
                    repo,
                    &[parent],
                    &cur,
                    s(c, "message"),
                    &a,
                    login.as_deref(),
                    &now,
                );
            }
            parent
        }
        _ => {
            let t = opt_s(&body, "commit_title")
                .unwrap_or_else(|| format!("Merge pull request #{number} from {head_label}"));
            let msg = match opt_s(&body, "commit_message") {
                Some(m) => format!("{t}\n\n{m}"),
                None => format!("{t}\n\n{title}"),
            };
            git::write_commit(
                ctx.svc,
                repo,
                &[base_head.clone(), head_sha.clone()],
                &map,
                &msg,
                &author,
                Some(&viewer),
                &now,
            )
        }
    };
    git::set_branch(ctx.svc, repo, &base_ref, &sha);
    if let Some(d) = ctx.svc.store.collection_mut("pulls").get_mut(key) {
        d["state"] = json!("closed");
        d["merged"] = json!(true);
        d["merged_at"] = json!(now);
        d["closed_at"] = json!(now);
        d["merged_by"] = json!(viewer);
        d["merge_commit_sha"] = json!(sha);
        d["merge_method"] = json!(method);
        d["head_sha"] = json!(head_sha);
        d["base_sha"] = json!(base_head);
        d["updated_at"] = json!(now);
    }
    // Close issues referenced with closing keywords in the PR body.
    close_linked_issues(ctx, repo, &p);
    ok(json!({"sha": sha, "merged": true, "message": "Pull Request successfully merged"}))
}

/// Issue numbers referenced with GitHub closing keywords (`Fixes #12`).
pub fn closing_refs(text: &str) -> Vec<u64> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    for kw in [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ] {
        let pat = format!("{kw} #");
        let mut idx = 0;
        while let Some(pos) = lower[idx..].find(&pat) {
            let start = idx + pos;
            let boundary = start == 0 || !lower.as_bytes()[start - 1].is_ascii_alphanumeric();
            let num_start = start + pat.len();
            let digits: String = lower[num_start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if boundary
                && let Ok(n) = digits.parse()
                && !out.contains(&n)
            {
                out.push(n);
            }
            idx = num_start;
        }
    }
    out
}

fn close_linked_issues(ctx: &mut Ctx, repo: &str, p: &Value) {
    let now = ctx.now.clone();
    let viewer = ctx.viewer();
    for n in closing_refs(s(p, "body")) {
        let key = format!("{repo}#{n}");
        if let Some(i) = ctx.svc.store.collection_mut("issues").get_mut(&key)
            && s(i, "state") == "open"
        {
            i["state"] = json!("closed");
            i["state_reason"] = json!("completed");
            i["closed_at"] = json!(now);
            i["closed_by"] = json!(viewer);
            i["updated_at"] = json!(now);
        }
    }
}

fn update_branch(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    if s(&p, "state") != "open" {
        return unprocessable("Cannot update a closed pull request branch.");
    }
    let head_repo = pr_head_repo(repo, &p);
    let head_ref = pr_head_ref(&p);
    let Some(head_sha) = git::branch_sha(ctx.svc, &head_repo, &head_ref) else {
        return unprocessable("Head branch was deleted.");
    };
    if let Some(expected) = opt_s(&body, "expected_head_sha")
        && expected != head_sha
    {
        return unprocessable("expected head sha didn't match current head ref.");
    }
    let base_ref = pr_base_ref(&p);
    let base_sha = git::branch_sha(ctx.svc, repo, &base_ref).unwrap_or_default();
    if git::ancestors(ctx.svc, &head_repo, &head_sha).contains(&base_sha) {
        return unprocessable("There are no new commits on the base branch.");
    }
    if s(&p, "mergeable_state") == "dirty" {
        return unprocessable("merge conflict between base and head");
    }
    // Merge base into head: head's changes applied on top of the base tree.
    let (changes, _) = pr_changes(ctx.svc, repo, &p);
    let head_map = git::file_map(ctx.svc, &head_repo, &head_sha).unwrap_or_default();
    let mut map = git::file_map(ctx.svc, repo, &base_sha).unwrap_or_default();
    for c in &changes {
        match head_map.get(&c.filename) {
            Some(e) => {
                map.insert(c.filename.clone(), e.clone());
            }
            None => {
                map.remove(&c.filename);
            }
        }
    }
    let viewer = ctx.viewer();
    let author = viewer_person(ctx);
    let now = ctx.now.clone();
    let msg = format!("Merge branch '{base_ref}' into {head_ref}");
    let sha = git::write_commit(
        ctx.svc,
        &head_repo,
        &[head_sha, base_sha],
        &map,
        &msg,
        &author,
        Some(&viewer),
        &now,
    );
    git::set_branch(ctx.svc, &head_repo, &head_ref, &sha);
    if let Some(d) = ctx.svc.store.collection_mut("pulls").get_mut(key) {
        d["updated_at"] = json!(now);
        if s(d, "mergeable_state") == "behind" {
            d["mergeable_state"] = json!("clean");
        }
    }
    let (_, number) = split_key(key);
    status_json(
        StatusCode::ACCEPTED,
        json!({"message": "Updating pull request branch.", "url": format!("{}/{repo}/pull/{number}", ctx.base())}),
    )
}

// ---------------------------------------------------------------- reviewers

fn request_reviewers(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let users = str_list(body.get("reviewers").unwrap_or(&Value::Null));
    let teams = str_list(body.get("team_reviewers").unwrap_or(&Value::Null));
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let author = login_of(p.get("user").unwrap_or(&Value::Null));
    let owner = repo.split('/').next().unwrap_or("").to_owned();
    let collaborators: Vec<String> = crate::issues::assignable(ctx.svc, repo);
    let mut cur_users = names_of(arr(&p, "requested_reviewers"));
    let mut cur_teams = names_of(arr(&p, "requested_teams"));
    for login in &users {
        if login.eq_ignore_ascii_case(&author) {
            return validation_msg(
                "PullRequest",
                "Review cannot be requested from pull request author.",
            );
        }
        if !collaborators.iter().any(|c| c.eq_ignore_ascii_case(login)) {
            return status_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "message": format!("Reviews may only be requested from collaborators. One or more of the users or teams you specified is not a collaborator of the {repo} repository."),
                    "documentation_url": crate::util::DOCS,
                    "status": "422",
                }),
            );
        }
        let canon = account(ctx.svc, login)
            .map(|a| s(a, "login").to_owned())
            .unwrap_or_else(|| login.clone());
        if !cur_users.contains(&canon) {
            cur_users.push(canon);
        }
    }
    for t in &teams {
        if get(ctx.svc, "teams", &format!("{owner}/{t}")).is_none() {
            return status_json(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "message": format!("Reviews may only be requested from collaborators. One or more of the users or teams you specified is not a collaborator of the {repo} repository."),
                    "documentation_url": crate::util::DOCS,
                    "status": "422",
                }),
            );
        }
        if !cur_teams.contains(t) {
            cur_teams.push(t.clone());
        }
    }
    let now = ctx.now.clone();
    if let Some(d) = ctx.svc.store.collection_mut("pulls").get_mut(key) {
        d["requested_reviewers"] = json!(cur_users);
        d["requested_teams"] = json!(cur_teams);
        d["updated_at"] = json!(now);
    }
    created(pr_json(ctx.svc, ctx.base(), key))
}

fn remove_reviewers(ctx: &mut Ctx, _repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let users = str_list(body.get("reviewers").unwrap_or(&Value::Null));
    let teams = str_list(body.get("team_reviewers").unwrap_or(&Value::Null));
    if let Some(d) = ctx.svc.store.collection_mut("pulls").get_mut(key) {
        let mut u_ = names_of(arr(d, "requested_reviewers"));
        u_.retain(|x| !users.iter().any(|r| r.eq_ignore_ascii_case(x)));
        let mut t_ = names_of(arr(d, "requested_teams"));
        t_.retain(|x| !teams.contains(x));
        d["requested_reviewers"] = json!(u_);
        d["requested_teams"] = json!(t_);
    }
    ok(pr_json(ctx.svc, ctx.base(), key))
}

// ---------------------------------------------------------------- reviews

pub fn reviews_of(svc: &ServiceState, key: &str) -> Vec<Value> {
    let (repo, number) = split_key(key);
    let mut v: Vec<Value> = all(svc, "reviews")
        .into_iter()
        .map(|(_, r)| r.clone())
        .filter(|r| s(r, "repo") == repo && u(r, "pull_number") == number)
        .collect();
    v.sort_by_key(|r| u(r, "id"));
    v
}

fn review_doc(svc: &ServiceState, key: &str, id: &str) -> Option<Value> {
    reviews_of(svc, key)
        .into_iter()
        .find(|r| u(r, "id").to_string() == id)
}

pub fn review_json(ctx: &Ctx, r: &Value) -> Value {
    let repo = s(r, "repo");
    let number = u(r, "pull_number");
    let id = u(r, "id");
    let pr_url = format!("{}/repos/{repo}/pulls/{number}", ctx.req.api());
    let html = format!("{}/{repo}/pull/{number}#pullrequestreview-{id}", ctx.base());
    let author = login_of(r.get("user").unwrap_or(&Value::Null));
    let mut v = json!({
        "id": id,
        "node_id": review_node_id(r),
        "user": user(ctx.svc, ctx.base(), &author),
        "body": s(r, "body"),
        "state": opt_s(r, "state").unwrap_or_else(|| "COMMENTED".into()),
        "html_url": html,
        "pull_request_url": pr_url,
        "author_association": render::association(ctx.svc, repo, &author),
        "_links": {"html": {"href": html}, "pull_request": {"href": pr_url}},
        "commit_id": r.get("commit_id").cloned().unwrap_or(Value::Null),
    });
    if s(r, "state") != "PENDING" {
        v["submitted_at"] = r.get("submitted_at").cloned().unwrap_or(Value::Null);
    }
    v
}

pub fn review_node_id(r: &Value) -> String {
    opt_s(r, "node_id").unwrap_or_else(|| node_id("PRR", &u(r, "id").to_string()))
}

pub fn event_state(event: &str) -> Option<&'static str> {
    match event.to_ascii_uppercase().as_str() {
        "APPROVE" => Some("APPROVED"),
        "REQUEST_CHANGES" => Some("CHANGES_REQUESTED"),
        "COMMENT" => Some("COMMENTED"),
        "" | "PENDING" => Some("PENDING"),
        _ => None,
    }
}

/// Creates a review; returns the review document. Shared with GraphQL.
pub fn insert_review(
    svc: &mut ServiceState,
    key: &str,
    viewer: &str,
    body: &str,
    state: &str,
    commit_id: &str,
    now: &str,
) -> Result<Value, String> {
    let (repo, number) = split_key(key);
    let p = get(svc, "pulls", key).cloned().unwrap_or_default();
    let author = login_of(p.get("user").unwrap_or(&Value::Null));
    if author.eq_ignore_ascii_case(viewer) && (state == "APPROVED" || state == "CHANGES_REQUESTED")
    {
        return Err(if state == "APPROVED" {
            "Can not approve your own pull request".into()
        } else {
            "Can not request changes on your own pull request".into()
        });
    }
    if state == "PENDING"
        && reviews_of(svc, key).iter().any(|r| {
            s(r, "state") == "PENDING" && login_of(r.get("user").unwrap_or(&Value::Null)) == viewer
        })
    {
        return Err("User can only have one pending review per pull request".into());
    }
    if (state == "CHANGES_REQUESTED" || state == "COMMENTED") && body.trim().is_empty() {
        return Err("Body can't be blank".into());
    }
    let id = next_id(svc, "reviews", "id", 1);
    let doc = json!({
        "id": id,
        "node_id": node_id("PRR", &id.to_string()),
        "repo": repo,
        "pull_number": number,
        "user": viewer,
        "body": body,
        "state": state,
        "commit_id": commit_id,
        "submitted_at": if state == "PENDING" { Value::Null } else { json!(now) },
    });
    svc.store
        .collection_mut("reviews")
        .insert(id.to_string(), doc.clone());
    if state != "PENDING" {
        after_submit(svc, key, viewer, now);
    }
    Ok(doc)
}

/// Clears the reviewer's pending request once they submit.
pub fn after_submit(svc: &mut ServiceState, key: &str, viewer: &str, now: &str) {
    if let Some(d) = svc.store.collection_mut("pulls").get_mut(key) {
        let mut users = names_of(arr(d, "requested_reviewers"));
        users.retain(|x| !x.eq_ignore_ascii_case(viewer));
        d["requested_reviewers"] = json!(users);
        d["updated_at"] = json!(now);
    }
}

fn create_review(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let Some(state) = event_state(s(&body, "event")) else {
        return validation("PullRequestReview", "event", "invalid");
    };
    let commit_id = opt_s(&body, "commit_id").unwrap_or_else(|| pr_head_sha(ctx.svc, repo, &p));
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let doc = match insert_review(
        ctx.svc,
        key,
        &viewer,
        s(&body, "body"),
        state,
        &commit_id,
        &now,
    ) {
        Ok(d) => d,
        Err(msg) => return unprocessable(&msg),
    };
    let rid = u(&doc, "id");
    for c in arr(&body, "comments") {
        let path = s(c, "path").to_owned();
        let line = c
            .get("line")
            .and_then(Value::as_u64)
            .or_else(|| c.get("position").and_then(Value::as_u64));
        insert_review_comment(
            ctx.svc,
            key,
            &viewer,
            s(c, "body"),
            &path,
            line,
            c.get("start_line").and_then(Value::as_u64),
            &opt_s(c, "side").unwrap_or_else(|| "RIGHT".into()),
            None,
            Some(rid),
            &commit_id,
            &now,
            "line",
        );
    }
    ok(review_json(ctx, &doc))
}

fn update_review(ctx: &mut Ctx, key: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(mut r) = review_doc(ctx.svc, key, id) else {
        return not_found();
    };
    let Some(text) = opt_s(&body, "body") else {
        return validation("PullRequestReview", "body", "missing_field");
    };
    r["body"] = json!(text);
    ctx.svc
        .store
        .collection_mut("reviews")
        .insert(id, r.clone());
    ok(review_json(ctx, &r))
}

fn delete_review(ctx: &mut Ctx, key: &str, id: &str) -> Response {
    let Some(r) = review_doc(ctx.svc, key, id) else {
        return not_found();
    };
    if s(&r, "state") != "PENDING" {
        return unprocessable("Can not delete a non-pending pull request review");
    }
    ctx.svc.store.collection_mut("reviews").remove(id);
    let rid = u(&r, "id");
    let comment_keys: Vec<String> = all(ctx.svc, "review_comments")
        .iter()
        .filter(|(_, c)| u(c, "review_id") == rid)
        .map(|(k, _)| (*k).to_owned())
        .collect();
    for k in comment_keys {
        ctx.svc.store.collection_mut("review_comments").remove(&k);
    }
    ok(review_json(ctx, &r))
}

fn submit_review(ctx: &mut Ctx, key: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(mut r) = review_doc(ctx.svc, key, id) else {
        return not_found();
    };
    if s(&r, "state") != "PENDING" {
        return unprocessable("Can not submit a non-pending pull request review");
    }
    let Some(state) = event_state(s(&body, "event")).filter(|s| *s != "PENDING") else {
        return validation("PullRequestReview", "event", "invalid");
    };
    if let Some(t) = opt_s(&body, "body") {
        r["body"] = json!(t);
    }
    r["state"] = json!(state);
    r["submitted_at"] = json!(ctx.now);
    ctx.svc
        .store
        .collection_mut("reviews")
        .insert(id, r.clone());
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    after_submit(ctx.svc, key, &viewer, &now);
    ok(review_json(ctx, &r))
}

// ---------------------------------------------------------------- review comments

pub fn is_pending(svc: &ServiceState, c: &Value) -> bool {
    let rid = u(c, "review_id");
    rid != 0 && get(svc, "reviews", &rid.to_string()).is_some_and(|r| s(r, "state") == "PENDING")
}

fn diff_hunk_for(
    svc: &ServiceState,
    repo: &str,
    key: &str,
    path: &str,
    line: Option<u64>,
) -> String {
    let p = get(svc, "pulls", key).cloned().unwrap_or_default();
    let (changes, _) = pr_changes(svc, repo, &p);
    let Some(c) = changes.iter().find(|c| c.filename == path) else {
        return String::new();
    };
    let lines: Vec<&str> = c.patch.lines().collect();
    let end = line
        .map(|l| (l as usize + 1).min(lines.len()))
        .unwrap_or(lines.len())
        .max(1)
        .min(lines.len());
    lines[..end].join("\n")
}

#[allow(clippy::too_many_arguments)]
pub fn insert_review_comment(
    svc: &mut ServiceState,
    key: &str,
    viewer: &str,
    body: &str,
    path: &str,
    line: Option<u64>,
    start_line: Option<u64>,
    side: &str,
    in_reply_to: Option<u64>,
    review_id: Option<u64>,
    commit_id: &str,
    now: &str,
    subject_type: &str,
) -> Value {
    let (repo, number) = split_key(key);
    let id = next_id(svc, "review_comments", "id", 1);
    let hunk = diff_hunk_for(svc, repo, key, path, line);
    let doc = json!({
        "id": id,
        "node_id": node_id("PRRC", &id.to_string()),
        "repo": repo,
        "pull_number": number,
        "review_id": review_id,
        "in_reply_to_id": in_reply_to,
        "user": viewer,
        "body": body,
        "path": path,
        "line": line,
        "original_line": line,
        "start_line": start_line,
        "original_start_line": start_line,
        "side": side,
        "start_side": start_line.map(|_| side),
        "commit_id": commit_id,
        "original_commit_id": commit_id,
        "diff_hunk": hunk,
        "subject_type": subject_type,
        "created_at": now,
        "updated_at": now,
    });
    svc.store
        .collection_mut("review_comments")
        .insert(id.to_string(), doc.clone());
    doc
}

pub fn review_comment_json(ctx: &Ctx, c: &Value) -> Value {
    let repo = s(c, "repo");
    let number = u(c, "pull_number");
    let id = u(c, "id");
    let api = ctx.req.api();
    let url = format!("{api}/repos/{repo}/pulls/comments/{id}");
    let html = format!("{}/{repo}/pull/{number}#discussion_r{id}", ctx.base());
    let pr_url = format!("{api}/repos/{repo}/pulls/{number}");
    let author = login_of(c.get("user").unwrap_or(&Value::Null));
    let mut v = json!({
        "url": url,
        "pull_request_review_id": c.get("review_id").cloned().unwrap_or(Value::Null),
        "id": id,
        "node_id": opt_s(c, "node_id").unwrap_or_else(|| node_id("PRRC", &id.to_string())),
        "diff_hunk": s(c, "diff_hunk"),
        "path": s(c, "path"),
        "commit_id": s(c, "commit_id"),
        "original_commit_id": opt_s(c, "original_commit_id").unwrap_or_else(|| s(c, "commit_id").to_owned()),
        "user": user(ctx.svc, ctx.base(), &author),
        "body": s(c, "body"),
        "created_at": c.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": c.get("updated_at").cloned().or_else(|| c.get("created_at").cloned()).unwrap_or(Value::Null),
        "html_url": html,
        "pull_request_url": pr_url,
        "author_association": render::association(ctx.svc, repo, &author),
        "_links": {"self": {"href": url}, "html": {"href": html}, "pull_request": {"href": pr_url}},
        "reactions": render::reactions_summary(ctx.svc, ctx.base(), &format!("pr_comment:{id}"), &url),
        "start_line": c.get("start_line").cloned().unwrap_or(Value::Null),
        "original_start_line": c.get("original_start_line").cloned().unwrap_or(Value::Null),
        "start_side": c.get("start_side").cloned().unwrap_or(Value::Null),
        "line": c.get("line").cloned().unwrap_or(Value::Null),
        "original_line": c.get("original_line").cloned().or_else(|| c.get("line").cloned()).unwrap_or(Value::Null),
        "side": opt_s(c, "side").unwrap_or_else(|| "RIGHT".into()),
        "subject_type": opt_s(c, "subject_type").unwrap_or_else(|| "line".into()),
    });
    if let Some(r) = c.get("in_reply_to_id").filter(|r| !r.is_null()) {
        v["in_reply_to_id"] = r.clone();
    }
    v
}

fn create_review_comment(ctx: &mut Ctx, repo: &str, key: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(text) = opt_s(&body, "body") else {
        return validation("PullRequestReviewComment", "body", "missing_field");
    };
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    if let Some(parent_id) = body.get("in_reply_to").and_then(Value::as_u64) {
        return reply_to(ctx, repo, key, parent_id, &text);
    }
    let p = get(ctx.svc, "pulls", key).cloned().unwrap_or_default();
    let path = s(&body, "path").to_owned();
    if path.is_empty() {
        return validation("PullRequestReviewComment", "path", "missing_field");
    }
    let (changes, _) = pr_changes(ctx.svc, repo, &p);
    if !changes.iter().any(|c| c.filename == path) {
        return validation_msg(
            "PullRequestReviewComment",
            "pull_request_review_thread.path diff too large or not in diff",
        );
    }
    let commit_id = opt_s(&body, "commit_id").unwrap_or_else(|| pr_head_sha(ctx.svc, repo, &p));
    let subject = opt_s(&body, "subject_type").unwrap_or_else(|| "line".into());
    let line = body.get("line").and_then(Value::as_u64);
    if subject == "line" && line.is_none() {
        return validation("PullRequestReviewComment", "line", "missing_field");
    }
    let doc = insert_review_comment(
        ctx.svc,
        key,
        &viewer,
        &text,
        &path,
        line,
        body.get("start_line").and_then(Value::as_u64),
        &opt_s(&body, "side").unwrap_or_else(|| "RIGHT".into()),
        None,
        None,
        &commit_id,
        &now,
        &subject,
    );
    created(review_comment_json(ctx, &doc))
}

fn reply_to(ctx: &mut Ctx, repo: &str, key: &str, parent_id: u64, text: &str) -> Response {
    let Some(parent) = get(ctx.svc, "review_comments", &parent_id.to_string()).cloned() else {
        return not_found();
    };
    let (_, number) = split_key(key);
    if s(&parent, "repo") != repo || u(&parent, "pull_number") != number {
        return not_found();
    }
    // Replies attach to the thread's root comment.
    let root = parent
        .get("in_reply_to_id")
        .and_then(Value::as_u64)
        .unwrap_or(parent_id);
    let viewer = ctx.viewer();
    let now = ctx.now.clone();
    let line = parent.get("line").and_then(Value::as_u64);
    let doc = insert_review_comment(
        ctx.svc,
        key,
        &viewer,
        text,
        s(&parent, "path"),
        line,
        parent.get("start_line").and_then(Value::as_u64),
        &opt_s(&parent, "side").unwrap_or_else(|| "RIGHT".into()),
        Some(root),
        None,
        s(&parent, "commit_id"),
        &now,
        &opt_s(&parent, "subject_type").unwrap_or_else(|| "line".into()),
    );
    created(review_comment_json(ctx, &doc))
}

fn reply(ctx: &mut Ctx, repo: &str, key: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(text) = opt_s(&body, "body") else {
        return validation("PullRequestReviewComment", "body", "missing_field");
    };
    reply_to(ctx, repo, key, id.parse().unwrap_or(0), &text)
}

fn update_review_comment(ctx: &mut Ctx, repo: &str, id: &str) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(mut c) = get(ctx.svc, "review_comments", id)
        .filter(|c| s(c, "repo") == repo)
        .cloned()
    else {
        return not_found();
    };
    let Some(text) = opt_s(&body, "body") else {
        return validation("PullRequestReviewComment", "body", "missing_field");
    };
    c["body"] = json!(text);
    c["updated_at"] = json!(ctx.now);
    ctx.svc
        .store
        .collection_mut("review_comments")
        .insert(id, c.clone());
    ok(review_comment_json(ctx, &c))
}

/// Issue JSON helper used by search (pull requests as issues).
#[allow(dead_code)]
pub fn as_issue(svc: &ServiceState, base: &str, key: &str) -> Value {
    issue_json(svc, base, key)
}
