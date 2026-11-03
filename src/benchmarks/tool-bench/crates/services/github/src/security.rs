//! Code scanning, Dependabot, and secret scanning alerts; global, repository, and
//! organization security advisories; code quality findings.
//!
//! Alert documents are stored close to the API shape (keyed `owner/repo#number`); the
//! handlers add URLs and apply GitHub's filters.

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use mock_core::{http::PageParams, world::ServiceState};
use serde_json::{Map, Value, json};

use crate::{
    Ctx,
    render::{all, get, is_org, repo_key, user_ref, with_prefix},
    util::{
        error, link, not_found, ok, opt_s, paged, percent_encode_query, s, str_list, u, unix,
        validation, with_header,
    },
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    let m = ctx.req.method.clone();
    if m != Method::GET && m != Method::PATCH {
        return None;
    }
    let get_ = m == Method::GET;
    match segs {
        ["advisories"] if get_ => return Some(global_advisories(ctx)),
        ["advisories", ghsa] if get_ => {
            return Some(match get(ctx.svc, "advisories", ghsa) {
                Some(a) => ok(global_advisory_json(ctx, a)),
                None => not_found(),
            });
        }
        ["orgs", org, "security-advisories"] if get_ => {
            if !is_org(ctx.svc, org) {
                return Some(not_found());
            }
            return Some(repo_advisories(ctx, |a| {
                s(a, "repo")
                    .split('/')
                    .next()
                    .is_some_and(|o| o.eq_ignore_ascii_case(org))
            }));
        }
        _ => {}
    }
    let ["repos", o, r, rest @ ..] = segs else {
        return None;
    };
    let first = *rest.first()?;
    if !matches!(
        first,
        "code-scanning" | "dependabot" | "secret-scanning" | "security-advisories" | "code-quality"
    ) {
        return None;
    }
    let Some(repo) = repo_key(ctx.svc, o, r) else {
        return Some(not_found());
    };
    Some(match rest {
        ["code-scanning", "alerts"] if get_ => code_scanning_list(ctx, &repo),
        ["code-scanning", "alerts", n] if get_ => {
            match alert(ctx.svc, "code_scanning_alerts", &repo, n) {
                Some(a) => ok(code_scanning_json(ctx, &repo, &a)),
                None => not_found(),
            }
        }
        ["code-scanning", "alerts", n] if m == Method::PATCH => {
            update_alert(ctx, "code_scanning_alerts", &repo, n, code_scanning_json)
        }
        ["dependabot", "alerts"] if get_ => dependabot_list(ctx, &repo),
        ["dependabot", "alerts", n] if get_ => {
            match alert(ctx.svc, "dependabot_alerts", &repo, n) {
                Some(a) => ok(dependabot_json(ctx, &repo, &a)),
                None => not_found(),
            }
        }
        ["dependabot", "alerts", n] if m == Method::PATCH => {
            update_alert(ctx, "dependabot_alerts", &repo, n, dependabot_json)
        }
        ["secret-scanning", "alerts"] if get_ => secret_list(ctx, &repo),
        ["secret-scanning", "alerts", n] if get_ => {
            match alert(ctx.svc, "secret_scanning_alerts", &repo, n) {
                Some(a) => ok(secret_json(ctx, &repo, &a)),
                None => not_found(),
            }
        }
        ["secret-scanning", "alerts", n] if m == Method::PATCH => {
            update_alert(ctx, "secret_scanning_alerts", &repo, n, secret_json)
        }
        ["security-advisories"] if get_ => {
            let key = repo.clone();
            repo_advisories(ctx, move |a| s(a, "repo") == key)
        }
        ["security-advisories", ghsa] if get_ => match get(ctx.svc, "repo_advisories", ghsa) {
            Some(a) if s(a, "repo") == repo => ok(repo_advisory_json(ctx, a)),
            _ => not_found(),
        },
        ["code-quality", "findings", n] if get_ => {
            match alert(ctx.svc, "code_quality_findings", &repo, n) {
                Some(f) => ok(code_quality_json(ctx, &repo, &f)),
                None => not_found(),
            }
        }
        _ => not_found(),
    })
}

fn alert(svc: &ServiceState, coll: &str, repo: &str, n: &str) -> Option<Value> {
    get(svc, coll, &format!("{repo}#{n}")).cloned()
}

fn alerts(svc: &ServiceState, coll: &str, repo: &str) -> Vec<Value> {
    let mut v: Vec<Value> = with_prefix(svc, coll, &format!("{repo}#"))
        .into_iter()
        .map(|(_, a)| a.clone())
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(u(a, "number")));
    v
}

fn merge(base: Value, extra: &Value) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            if k == "repo" || out.contains_key(k) {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn sort_dir(ctx: &Ctx, items: &mut [Value], default_field: &str) {
    let field = match ctx.req.q("sort") {
        Some("updated") => "updated_at",
        Some("created") => "created_at",
        _ => default_field,
    };
    items.sort_by_key(|a| (unix(s(a, field)).unwrap_or(0), u(a, "number")));
    if ctx.req.q("direction") != Some("asc") {
        items.reverse();
    }
}

fn update_alert(
    ctx: &mut Ctx,
    coll: &str,
    repo: &str,
    n: &str,
    render: fn(&Ctx, &str, &Value) -> Value,
) -> Response {
    let body = match ctx.req.json() {
        Ok(b) => b,
        Err(e) => return e,
    };
    let key = format!("{repo}#{n}");
    let Some(mut a) = get(ctx.svc, coll, &key).cloned() else {
        return not_found();
    };
    let state = s(&body, "state").to_owned();
    let viewer = ctx.viewer();
    match (coll, state.as_str()) {
        ("secret_scanning_alerts", "resolved") => {
            a["state"] = json!("resolved");
            a["resolution"] = body.get("resolution").cloned().unwrap_or(Value::Null);
            a["resolution_comment"] = body
                .get("resolution_comment")
                .cloned()
                .unwrap_or(Value::Null);
            a["resolved_at"] = json!(ctx.now);
            a["resolved_by"] = json!(viewer);
        }
        (_, "dismissed") => {
            a["state"] = json!("dismissed");
            a["dismissed_reason"] = body.get("dismissed_reason").cloned().unwrap_or(Value::Null);
            a["dismissed_comment"] = body
                .get("dismissed_comment")
                .cloned()
                .unwrap_or(Value::Null);
            a["dismissed_at"] = json!(ctx.now);
            a["dismissed_by"] = json!(viewer);
        }
        (_, "open") => {
            a["state"] = json!("open");
            for k in [
                "dismissed_reason",
                "dismissed_at",
                "dismissed_by",
                "resolution",
                "resolved_at",
                "resolved_by",
            ] {
                a[k] = Value::Null;
            }
        }
        _ => return validation("Alert", "state", "invalid"),
    }
    a["updated_at"] = json!(ctx.now);
    ctx.svc.store.collection_mut(coll).insert(key, a.clone());
    ok(render(ctx, repo, &a))
}

// ---------------------------------------------------------------- code scanning

fn code_scanning_json(ctx: &Ctx, repo: &str, a: &Value) -> Value {
    let n = u(a, "number");
    let api = format!("{}/repos/{repo}/code-scanning/alerts/{n}", ctx.req.api());
    let rule = a.get("rule").cloned().unwrap_or(json!({}));
    let tool = a
        .get("tool")
        .cloned()
        .unwrap_or(json!({"name": "CodeQL", "guid": null, "version": "2.20.0"}));
    let inst = a.get("most_recent_instance").cloned().unwrap_or(json!({}));
    let base = json!({
        "number": n,
        "created_at": a.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().unwrap_or(Value::Null),
        "url": api,
        "html_url": format!("{}/{repo}/security/code-scanning/{n}", ctx.base()),
        "instances_url": format!("{api}/instances"),
        "state": opt_s(a, "state").unwrap_or_else(|| "open".into()),
        "fixed_at": a.get("fixed_at").cloned().unwrap_or(Value::Null),
        "dismissed_by": a.get("dismissed_by").map(|d| user_ref(ctx.svc, ctx.base(), d)).unwrap_or(Value::Null),
        "dismissed_at": a.get("dismissed_at").cloned().unwrap_or(Value::Null),
        "dismissed_reason": a.get("dismissed_reason").cloned().unwrap_or(Value::Null),
        "dismissed_comment": a.get("dismissed_comment").cloned().unwrap_or(Value::Null),
        "rule": rule,
        "tool": tool,
        "most_recent_instance": inst,
    });
    merge(base, a)
}

fn code_scanning_list(ctx: &Ctx, repo: &str) -> Response {
    if get(ctx.svc, "repos", repo)
        .is_some_and(|r| r.get("code_scanning_enabled") == Some(&json!(false)))
    {
        return error(StatusCode::NOT_FOUND, "no analysis found");
    }
    let state = ctx.req.q("state");
    let severity = ctx.req.q("severity");
    let tool = ctx.req.q("tool_name");
    let reference = ctx.req.q("ref");
    let mut items: Vec<Value> = alerts(ctx.svc, "code_scanning_alerts", repo)
        .into_iter()
        .filter(|a| state.is_none_or(|st| opt_s(a, "state").as_deref().unwrap_or("open") == st))
        .filter(|a| {
            severity.is_none_or(|sv| {
                let rule = a.get("rule").cloned().unwrap_or_default();
                s(&rule, "security_severity_level") == sv || s(&rule, "severity") == sv
            })
        })
        .filter(|a| {
            tool.is_none_or(|t| {
                a.get("tool")
                    .and_then(|x| x.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("CodeQL")
                    == t
            })
        })
        .filter(|a| {
            reference.is_none_or(|r| {
                let inst_ref = a
                    .get("most_recent_instance")
                    .and_then(|i| i.get("ref"))
                    .and_then(Value::as_str)
                    .unwrap_or("refs/heads/main");
                inst_ref == r || inst_ref == format!("refs/heads/{r}")
            })
        })
        .collect();
    sort_dir(ctx, &mut items, "created_at");
    let out: Vec<Value> = items
        .iter()
        .map(|a| code_scanning_json(ctx, repo, a))
        .collect();
    paged(ctx.req, out)
}

// ---------------------------------------------------------------- dependabot

fn dependabot_json(ctx: &Ctx, repo: &str, a: &Value) -> Value {
    let n = u(a, "number");
    let base = json!({
        "number": n,
        "state": opt_s(a, "state").unwrap_or_else(|| "open".into()),
        "dependency": a.get("dependency").cloned().unwrap_or(Value::Null),
        "security_advisory": a.get("security_advisory").cloned().unwrap_or(Value::Null),
        "security_vulnerability": a.get("security_vulnerability").cloned().unwrap_or(Value::Null),
        "url": format!("{}/repos/{repo}/dependabot/alerts/{n}", ctx.req.api()),
        "html_url": format!("{}/{repo}/security/dependabot/{n}", ctx.base()),
        "created_at": a.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().unwrap_or(Value::Null),
        "dismissed_at": a.get("dismissed_at").cloned().unwrap_or(Value::Null),
        "dismissed_by": a.get("dismissed_by").map(|d| user_ref(ctx.svc, ctx.base(), d)).unwrap_or(Value::Null),
        "dismissed_reason": a.get("dismissed_reason").cloned().unwrap_or(Value::Null),
        "dismissed_comment": a.get("dismissed_comment").cloned().unwrap_or(Value::Null),
        "fixed_at": a.get("fixed_at").cloned().unwrap_or(Value::Null),
        "auto_dismissed_at": a.get("auto_dismissed_at").cloned().unwrap_or(Value::Null),
    });
    merge(base, a)
}

fn severity_of_dependabot(a: &Value) -> String {
    a.get("security_vulnerability")
        .and_then(|v| v.get("severity"))
        .or_else(|| a.get("security_advisory").and_then(|v| v.get("severity")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Cursor pagination (`after`/`before` with `Link`), as the Dependabot API does.
fn cursor_page(ctx: &Ctx, items: Vec<Value>, cursor_of: fn(&Value) -> String) -> Response {
    let per_page = ctx
        .req
        .q("per_page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 100);
    let start = match ctx.req.q("after") {
        Some(after) => items
            .iter()
            .position(|a| cursor_of(a) == after)
            .map_or(items.len(), |p| p + 1),
        None => 0,
    };
    let page: Vec<Value> = items.iter().skip(start).take(per_page).cloned().collect();
    let resp = ok(Value::Array(page.clone()));
    if start + per_page < items.len()
        && let Some(last) = page.last()
    {
        let others: Vec<String> = ctx
            .req
            .query
            .iter()
            .filter(|(k, _)| k != "after" && k != "before" && k != "per_page")
            .map(|(k, v)| format!("{k}={}", percent_encode_query(v)))
            .collect();
        let mut url = format!("{}{}?", ctx.base(), ctx.req.path);
        for o in others {
            url.push_str(&o);
            url.push('&');
        }
        let l = format!(
            "<{url}per_page={per_page}&after={}>; rel=\"next\"",
            cursor_of(last)
        );
        return with_header(resp, axum::http::header::LINK, &l);
    }
    resp
}

fn dependabot_cursor(a: &Value) -> String {
    crate::util::b64_encode(format!("cursor:{}", u(a, "number")).as_bytes())
}

fn dependabot_list(ctx: &Ctx, repo: &str) -> Response {
    let states: Vec<String> = ctx
        .req
        .q("state")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let sev: Vec<String> = ctx
        .req
        .q("severity")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let eco: Vec<String> = ctx
        .req
        .q("ecosystem")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let pkgs: Vec<String> = ctx
        .req
        .q("package")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let scope = ctx.req.q("scope");
    let manifest = ctx.req.q("manifest");
    let mut items: Vec<Value> = alerts(ctx.svc, "dependabot_alerts", repo)
        .into_iter()
        .filter(|a| {
            states.is_empty()
                || states
                    .iter()
                    .any(|st| opt_s(a, "state").as_deref().unwrap_or("open") == st)
        })
        .filter(|a| sev.is_empty() || sev.contains(&severity_of_dependabot(a)))
        .filter(|a| {
            let pkg = a
                .get("dependency")
                .and_then(|d| d.get("package"))
                .cloned()
                .unwrap_or_default();
            (eco.is_empty() || eco.iter().any(|e| e == s(&pkg, "ecosystem")))
                && (pkgs.is_empty() || pkgs.iter().any(|p| p == s(&pkg, "name")))
        })
        .filter(|a| {
            scope.is_none_or(|sc| {
                a.get("dependency")
                    .and_then(|d| d.get("scope"))
                    .and_then(Value::as_str)
                    == Some(sc)
            })
        })
        .filter(|a| {
            manifest.is_none_or(|mf| {
                a.get("dependency")
                    .and_then(|d| d.get("manifest_path"))
                    .and_then(Value::as_str)
                    == Some(mf)
            })
        })
        .collect();
    sort_dir(ctx, &mut items, "created_at");
    let out: Vec<Value> = items
        .iter()
        .map(|a| dependabot_json(ctx, repo, a))
        .collect();
    cursor_page(ctx, out, dependabot_cursor)
}

// ---------------------------------------------------------------- secret scanning

fn secret_json(ctx: &Ctx, repo: &str, a: &Value) -> Value {
    let n = u(a, "number");
    let api = format!("{}/repos/{repo}/secret-scanning/alerts/{n}", ctx.req.api());
    let base = json!({
        "number": n,
        "created_at": a.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().unwrap_or(Value::Null),
        "url": api,
        "html_url": format!("{}/{repo}/security/secret-scanning/{n}", ctx.base()),
        "locations_url": format!("{api}/locations"),
        "state": opt_s(a, "state").unwrap_or_else(|| "open".into()),
        "resolution": a.get("resolution").cloned().unwrap_or(Value::Null),
        "resolved_at": a.get("resolved_at").cloned().unwrap_or(Value::Null),
        "resolved_by": a.get("resolved_by").map(|d| user_ref(ctx.svc, ctx.base(), d)).unwrap_or(Value::Null),
        "resolution_comment": a.get("resolution_comment").cloned().unwrap_or(Value::Null),
        "secret_type": s(a, "secret_type"),
        "secret_type_display_name": s(a, "secret_type_display_name"),
        "secret": a.get("secret").cloned().unwrap_or(Value::Null),
        "validity": opt_s(a, "validity").unwrap_or_else(|| "unknown".into()),
        "push_protection_bypassed": a.get("push_protection_bypassed").cloned().unwrap_or(json!(false)),
        "publicly_leaked": a.get("publicly_leaked").cloned().unwrap_or(json!(false)),
        "multi_repo": a.get("multi_repo").cloned().unwrap_or(json!(false)),
    });
    merge(base, a)
}

fn secret_list(ctx: &Ctx, repo: &str) -> Response {
    let state = ctx.req.q("state");
    let types: Vec<String> = ctx
        .req
        .q("secret_type")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let resolutions: Vec<String> = ctx
        .req
        .q("resolution")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut items: Vec<Value> = alerts(ctx.svc, "secret_scanning_alerts", repo)
        .into_iter()
        .filter(|a| state.is_none_or(|st| opt_s(a, "state").as_deref().unwrap_or("open") == st))
        .filter(|a| types.is_empty() || types.iter().any(|t| t == s(a, "secret_type")))
        .filter(|a| resolutions.is_empty() || resolutions.iter().any(|r| r == s(a, "resolution")))
        .collect();
    sort_dir(ctx, &mut items, "created_at");
    let out: Vec<Value> = items.iter().map(|a| secret_json(ctx, repo, a)).collect();
    paged(ctx.req, out)
}

// ---------------------------------------------------------------- advisories

fn global_advisory_json(ctx: &Ctx, a: &Value) -> Value {
    let id = s(a, "ghsa_id");
    let base = json!({
        "ghsa_id": id,
        "cve_id": a.get("cve_id").cloned().unwrap_or(Value::Null),
        "url": format!("{}/advisories/{id}", ctx.req.api()),
        "html_url": format!("{}/advisories/{id}", ctx.base()),
        "repository_advisory_url": a.get("repository_advisory_url").cloned().unwrap_or(Value::Null),
        "summary": s(a, "summary"),
        "description": a.get("description").cloned().unwrap_or(Value::Null),
        "type": opt_s(a, "type").unwrap_or_else(|| "reviewed".into()),
        "severity": opt_s(a, "severity").unwrap_or_else(|| "unknown".into()),
        "source_code_location": a.get("source_code_location").cloned().unwrap_or(Value::Null),
        "identifiers": a.get("identifiers").cloned().unwrap_or_else(|| {
            let mut ids = vec![json!({"type": "GHSA", "value": id})];
            if let Some(cve) = a.get("cve_id").and_then(Value::as_str) {
                ids.push(json!({"type": "CVE", "value": cve}));
            }
            json!(ids)
        }),
        "references": a.get("references").cloned().unwrap_or(json!([])),
        "published_at": a.get("published_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().or_else(|| a.get("published_at").cloned()).unwrap_or(Value::Null),
        "github_reviewed_at": a.get("github_reviewed_at").cloned().or_else(|| a.get("published_at").cloned()).unwrap_or(Value::Null),
        "nvd_published_at": a.get("nvd_published_at").cloned().unwrap_or(Value::Null),
        "withdrawn_at": a.get("withdrawn_at").cloned().unwrap_or(Value::Null),
        "vulnerabilities": a.get("vulnerabilities").cloned().unwrap_or(json!([])),
        "cvss": a.get("cvss").cloned().unwrap_or(Value::Null),
        "cwes": a.get("cwes").cloned().unwrap_or(json!([])),
        "credits": a.get("credits").cloned().unwrap_or(json!([])),
    });
    merge(base, a)
}

fn global_advisories(ctx: &Ctx) -> Response {
    let q = |k: &str| ctx.req.q(k);
    let mut items: Vec<Value> = all(ctx.svc, "advisories")
        .into_iter()
        .map(|(_, a)| a.clone())
        .filter(|a| q("ghsa_id").is_none_or(|g| s(a, "ghsa_id") == g))
        .filter(|a| q("cve_id").is_none_or(|c| s(a, "cve_id") == c))
        .filter(|a| {
            q("type").is_none_or(|t| opt_s(a, "type").as_deref().unwrap_or("reviewed") == t)
        })
        .filter(|a| q("severity").is_none_or(|sv| s(a, "severity") == sv))
        .filter(|a| {
            q("ecosystem").is_none_or(|e| {
                a.get("vulnerabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|vs| {
                        vs.iter().any(|v| {
                            v.get("package")
                                .and_then(|p| p.get("ecosystem"))
                                .and_then(Value::as_str)
                                == Some(e)
                        })
                    })
            })
        })
        .filter(|a| {
            q("affects").is_none_or(|pkg| {
                let names: Vec<&str> = pkg
                    .split(',')
                    .map(|p| p.split('@').next().unwrap_or(p))
                    .collect();
                a.get("vulnerabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|vs| {
                        vs.iter().any(|v| {
                            v.get("package")
                                .and_then(|p| p.get("name"))
                                .and_then(Value::as_str)
                                .is_some_and(|n| names.contains(&n))
                        })
                    })
            })
        })
        .filter(|a| {
            q("cwes").is_none_or(|c| {
                let want: Vec<String> = c
                    .split(',')
                    .map(|x| {
                        if x.starts_with("CWE-") {
                            x.to_owned()
                        } else {
                            format!("CWE-{x}")
                        }
                    })
                    .collect();
                a.get("cwes").and_then(Value::as_array).is_some_and(|cs| {
                    cs.iter()
                        .any(|cw| want.iter().any(|w| s(cw, "cwe_id") == w))
                })
            })
        })
        .filter(|a| {
            q("is_withdrawn").is_none_or(|w| {
                let withdrawn = a.get("withdrawn_at").is_some_and(|x| !x.is_null());
                withdrawn == (w == "true")
            })
        })
        .filter(|a| q("published").is_none_or(|p| date_filter(p, s(a, "published_at"))))
        .filter(|a| q("updated").is_none_or(|p| date_filter(p, s(a, "updated_at"))))
        .filter(|a| {
            q("modified").is_none_or(|p| {
                date_filter(p, s(a, "updated_at")) || date_filter(p, s(a, "published_at"))
            })
        })
        .collect();
    items.sort_by_key(|a| std::cmp::Reverse(unix(s(a, "published_at")).unwrap_or(0)));
    if q("direction") == Some("asc") {
        items.reverse();
    }
    let out: Vec<Value> = items.iter().map(|a| global_advisory_json(ctx, a)).collect();
    let params = PageParams::parse(None, q("per_page"), 30, 100);
    let page: Vec<Value> = out.iter().take(params.per_page).cloned().collect();
    link(
        ctx.req,
        ok(Value::Array(page)),
        params,
        out.len().min(params.per_page),
    )
}

fn date_filter(spec: &str, ts: &str) -> bool {
    let Some(v) = unix(ts) else {
        return false;
    };
    if let Some((lo, hi)) = spec.split_once("..") {
        return (lo == "*" || unix(lo).is_some_and(|l| v >= l))
            && (hi == "*" || unix(hi).is_some_and(|h| v < h + 86400));
    }
    if let Some(rest) = spec.strip_prefix(">=") {
        return unix(rest).is_some_and(|x| v >= x);
    }
    if let Some(rest) = spec.strip_prefix('>') {
        return unix(rest).is_some_and(|x| v > x);
    }
    if let Some(rest) = spec.strip_prefix("<=") {
        return unix(rest).is_some_and(|x| v < x + 86400);
    }
    if let Some(rest) = spec.strip_prefix('<') {
        return unix(rest).is_some_and(|x| v < x);
    }
    unix(spec).is_some_and(|d| v >= d && v < d + 86400)
}

fn repo_advisory_json(ctx: &Ctx, a: &Value) -> Value {
    let id = s(a, "ghsa_id");
    let repo = s(a, "repo");
    let base = json!({
        "ghsa_id": id,
        "cve_id": a.get("cve_id").cloned().unwrap_or(Value::Null),
        "url": format!("{}/repos/{repo}/security-advisories/{id}", ctx.req.api()),
        "html_url": format!("{}/{repo}/security/advisories/{id}", ctx.base()),
        "summary": s(a, "summary"),
        "description": a.get("description").cloned().unwrap_or(Value::Null),
        "severity": opt_s(a, "severity").unwrap_or_else(|| "unknown".into()),
        "author": a.get("author").map(|d| user_ref(ctx.svc, ctx.base(), d)).unwrap_or(Value::Null),
        "publisher": a.get("publisher").map(|d| user_ref(ctx.svc, ctx.base(), d)).unwrap_or(Value::Null),
        "identifiers": a.get("identifiers").cloned().unwrap_or_else(|| json!([{"type": "GHSA", "value": id}])),
        "state": opt_s(a, "state").unwrap_or_else(|| "published".into()),
        "created_at": a.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": a.get("updated_at").cloned().unwrap_or(Value::Null),
        "published_at": a.get("published_at").cloned().unwrap_or(Value::Null),
        "closed_at": a.get("closed_at").cloned().unwrap_or(Value::Null),
        "withdrawn_at": a.get("withdrawn_at").cloned().unwrap_or(Value::Null),
        "submission": null,
        "vulnerabilities": a.get("vulnerabilities").cloned().unwrap_or(json!([])),
        "cvss": a.get("cvss").cloned().unwrap_or(Value::Null),
        "cwes": a.get("cwes").cloned().unwrap_or(json!([])),
        "cwe_ids": a.get("cwe_ids").cloned().unwrap_or(json!([])),
        "credits": a.get("credits").cloned().unwrap_or(json!([])),
        "credits_detailed": a.get("credits_detailed").cloned().unwrap_or(json!([])),
        "collaborating_users": a.get("collaborating_users").cloned().unwrap_or(Value::Null),
        "collaborating_teams": a.get("collaborating_teams").cloned().unwrap_or(Value::Null),
        "private_fork": null,
    });
    merge(base, a)
}

fn repo_advisories(ctx: &Ctx, scope: impl Fn(&Value) -> bool) -> Response {
    let state = ctx.req.q("state");
    let mut items: Vec<Value> = all(ctx.svc, "repo_advisories")
        .into_iter()
        .map(|(_, a)| a.clone())
        .filter(|a| scope(a))
        .filter(|a| {
            state.is_none_or(|st| opt_s(a, "state").as_deref().unwrap_or("published") == st)
        })
        .collect();
    let field = match ctx.req.q("sort") {
        Some("updated") => "updated_at",
        Some("published") => "published_at",
        _ => "created_at",
    };
    items.sort_by_key(|a| unix(s(a, field)).unwrap_or(0));
    if ctx.req.q("direction") != Some("asc") {
        items.reverse();
    }
    let out: Vec<Value> = items.iter().map(|a| repo_advisory_json(ctx, a)).collect();
    let per_page = ctx
        .req
        .q("per_page")
        .and_then(|p| p.parse().ok())
        .unwrap_or(30usize);
    ok(Value::Array(out.into_iter().take(per_page).collect()))
}

// ---------------------------------------------------------------- code quality

fn code_quality_json(ctx: &Ctx, repo: &str, f: &Value) -> Value {
    let n = u(f, "number");
    let base = json!({
        "number": n,
        "url": format!("{}/repos/{repo}/code-quality/findings/{n}", ctx.req.api()),
        "html_url": format!("{}/{repo}/security/quality/{n}", ctx.base()),
        "state": opt_s(f, "state").unwrap_or_else(|| "open".into()),
    });
    merge(base, f)
}

#[allow(dead_code)]
fn unused(v: &Value) -> Vec<String> {
    str_list(v)
}
