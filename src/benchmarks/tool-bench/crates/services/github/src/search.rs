//! `GET /search/{issues,code,repositories,users,commits}` with GitHub's qualifier syntax.

use axum::{http::Method, response::Response};
use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    Ctx, git,
    render::{
        self, account_type, all, commit_json, get, is_org, issue_json, label_doc, repo_json,
        repo_min, split_key, user,
    },
    sha1::blob_sha,
    util::{arr, b, login_of, names_of, opt_s, percent_encode_path, s, u, unix, validation},
};

pub fn route(ctx: &mut Ctx, segs: &[&str]) -> Option<Response> {
    if ctx.req.method != Method::GET {
        return None;
    }
    let ["search", kind] = segs else {
        return None;
    };
    let Some(q) = ctx.req.q("q").map(str::to_owned) else {
        return Some(validation("Search", "q", "missing"));
    };
    let query = Query::parse(&q);
    Some(match *kind {
        "issues" => search_issues(ctx, &query),
        "code" => search_code(ctx, &query),
        "repositories" => search_repos(ctx, &query),
        "users" => search_users(ctx, &query),
        "commits" => search_commits(ctx, &query),
        _ => return None,
    })
}

#[derive(Debug, Default)]
pub struct Query {
    /// (key, value, negated)
    pub quals: Vec<(String, String, bool)>,
    pub terms: Vec<String>,
    pub any: bool,
}

impl Query {
    pub fn parse(q: &str) -> Self {
        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        for c in q.chars() {
            match c {
                '"' => {
                    quoted = !quoted;
                }
                c if c.is_whitespace() && !quoted => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
        let mut out = Self::default();
        for t in tokens {
            if t == "OR" {
                out.any = true;
                continue;
            }
            if t == "AND" || t == "NOT" {
                continue;
            }
            let (neg, body) = match t.strip_prefix('-') {
                Some(rest) if rest.contains(':') => (true, rest.to_owned()),
                _ => (false, t.clone()),
            };
            match body.split_once(':') {
                Some((k, v))
                    if !k.is_empty()
                        && k.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                        && !v.is_empty() =>
                {
                    out.quals.push((k.to_ascii_lowercase(), v.to_owned(), neg));
                }
                _ => {
                    let term = body.trim_matches(|c: char| c == '(' || c == ')').to_owned();
                    if !term.is_empty() {
                        out.terms.push(term.to_ascii_lowercase());
                    }
                }
            }
        }
        out
    }

    pub fn values(&self, key: &str) -> Vec<&str> {
        self.quals
            .iter()
            .filter(|(k, _, neg)| k == key && !neg)
            .map(|(_, v, _)| v.as_str())
            .collect()
    }

    pub fn negated(&self, key: &str) -> Vec<&str> {
        self.quals
            .iter()
            .filter(|(k, _, neg)| k == key && *neg)
            .map(|(_, v, _)| v.as_str())
            .collect()
    }

    pub fn first(&self, key: &str) -> Option<&str> {
        self.values(key).into_iter().next()
    }

    /// Free-text match over `haystack` (already lowercased).
    pub fn text_matches(&self, haystack: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let hit = |t: &String| haystack.contains(t.as_str());
        if self.any {
            self.terms.iter().any(hit)
        } else {
            self.terms.iter().all(hit)
        }
    }

    pub fn score(&self, haystack: &str) -> usize {
        self.terms
            .iter()
            .map(|t| haystack.matches(t.as_str()).count())
            .sum()
    }
}

/// `>=2026-01-01`, `<5`, `2026-01-01..2026-02-01`, `10..*`, or exact.
fn cmp_matches(spec: &str, value: f64, parse: &dyn Fn(&str) -> Option<f64>) -> bool {
    if let Some((lo, hi)) = spec.split_once("..") {
        let lo_ok = lo == "*" || parse(lo).is_some_and(|l| value >= l);
        let hi_ok = hi == "*" || parse(hi).is_some_and(|h| value <= h);
        return lo_ok && hi_ok;
    }
    for (op, f) in [
        (">=", (|a: f64, b: f64| a >= b) as fn(f64, f64) -> bool),
        ("<=", |a, b| a <= b),
        (">", |a, b| a > b),
        ("<", |a, b| a < b),
    ] {
        if let Some(rest) = spec.strip_prefix(op) {
            return parse(rest).is_some_and(|x| f(value, x));
        }
    }
    parse(spec).is_some_and(|x| (value - x).abs() < f64::EPSILON)
}

fn date_matches(spec: &str, ts: &str) -> bool {
    let Some(v) = unix(ts) else {
        return false;
    };
    let parse = |s: &str| unix(s).map(|t| t as f64);
    if !spec.contains("..") && !spec.starts_with(['<', '>']) {
        // A bare date matches the whole day.
        return unix(spec).is_some_and(|d| v >= d && v < d + 86400);
    }
    // `<=date` should include the whole day.
    if let Some(rest) = spec.strip_prefix("<=")
        && rest.len() == 10
    {
        return unix(rest).is_some_and(|d| v < d + 86400);
    }
    cmp_matches(spec, v as f64, &parse)
}

fn num_matches(spec: &str, v: f64) -> bool {
    cmp_matches(spec, v, &|s: &str| s.parse().ok())
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn repo_in_scope(q: &Query, repo: &str) -> bool {
    let owner = repo.split('/').next().unwrap_or("");
    let repos = q.values("repo");
    let owners: Vec<&str> = q
        .values("org")
        .into_iter()
        .chain(q.values("user"))
        .chain(q.values("owner"))
        .collect();
    (repos.is_empty() || repos.iter().any(|r| eq(r, repo)))
        && (owners.is_empty() || owners.iter().any(|o| eq(o, owner)))
        && !q.negated("repo").iter().any(|r| eq(r, repo))
        && !q.negated("org").iter().any(|o| eq(o, owner))
}

fn wants_order(ctx: &Ctx) -> (Option<String>, bool) {
    let sort = ctx.req.q("sort").map(str::to_owned);
    let asc = ctx.req.q("order") == Some("asc");
    (sort, asc)
}

// ---------------------------------------------------------------- issues

fn issue_matches(svc: &ServiceState, q: &Query, key: &str, doc: &Value, is_pr: bool) -> bool {
    let (repo, number) = split_key(key);
    if !repo_in_scope(q, repo) {
        return false;
    }
    let state = s(doc, "state");
    let merged = b(doc, "merged") || doc.get("merged_at").is_some_and(|m| !m.is_null());
    for v in q.values("is").into_iter().chain(q.values("type")) {
        let ok = match v.to_ascii_lowercase().as_str() {
            "issue" => !is_pr,
            "pr" | "pull-request" => is_pr,
            "open" => state == "open",
            "closed" => state == "closed",
            "merged" => merged,
            "unmerged" => is_pr && !merged && state == "closed",
            "draft" => b(doc, "draft"),
            "locked" => b(doc, "locked"),
            "unlocked" => !b(doc, "locked"),
            "public" => !get(svc, "repos", repo).is_some_and(|r| b(r, "private")),
            "private" => get(svc, "repos", repo).is_some_and(|r| b(r, "private")),
            // `type:Bug` filters by issue type.
            other => !is_pr && s(doc, "type").eq_ignore_ascii_case(other),
        };
        if !ok {
            return false;
        }
    }
    for v in q.negated("is") {
        let hit = match v {
            "draft" => b(doc, "draft"),
            "open" => state == "open",
            "closed" => state == "closed",
            "merged" => merged,
            _ => false,
        };
        if hit {
            return false;
        }
    }
    if let Some(st) = q.first("state")
        && !eq(st, state)
    {
        return false;
    }
    let author = login_of(doc.get("user").unwrap_or(&Value::Null));
    if q.values("author").iter().any(|a| !eq(a, &author))
        || q.negated("author").iter().any(|a| eq(a, &author))
    {
        return false;
    }
    let assignees = names_of(arr(doc, "assignees"));
    for a in q.values("assignee") {
        if !assignees.iter().any(|x| eq(x, a)) {
            return false;
        }
    }
    if q.negated("assignee")
        .iter()
        .any(|a| assignees.iter().any(|x| eq(x, a)))
    {
        return false;
    }
    let labels = names_of(arr(doc, "labels"));
    for l in q.values("label") {
        let options: Vec<&str> = l.split(',').collect();
        if !options.iter().any(|o| labels.iter().any(|x| eq(x, o))) {
            return false;
        }
    }
    if q.negated("label")
        .iter()
        .any(|l| labels.iter().any(|x| eq(x, l)))
    {
        return false;
    }
    for v in q.values("no") {
        let ok = match v {
            "label" => labels.is_empty(),
            "assignee" => assignees.is_empty(),
            "milestone" => render::milestone_number(doc).is_none(),
            "type" => s(doc, "type").is_empty(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if let Some(ms) = q.first("milestone") {
        let title = render::milestone_number(doc)
            .and_then(|n| get(svc, "milestones", &format!("{repo}:{n}")))
            .map(|m| s(m, "title").to_owned())
            .unwrap_or_default();
        if !eq(ms, &title) {
            return false;
        }
    }
    let comments: Vec<(String, String)> = all(svc, "issue_comments")
        .iter()
        .filter(|(_, c)| s(c, "repo") == repo && u(c, "issue_number") == number)
        .map(|(_, c)| {
            (
                login_of(c.get("user").unwrap_or(&Value::Null)),
                s(c, "body").to_owned(),
            )
        })
        .collect();
    for m in q.values("mentions") {
        let pat = format!("@{}", m.to_ascii_lowercase());
        if !s(doc, "body").to_ascii_lowercase().contains(&pat)
            && !comments
                .iter()
                .any(|(_, b)| b.to_ascii_lowercase().contains(&pat))
        {
            return false;
        }
    }
    for c in q.values("commenter") {
        if !comments.iter().any(|(l, _)| eq(l, c)) {
            return false;
        }
    }
    for who in q.values("involves") {
        let involved = eq(&author, who)
            || assignees.iter().any(|a| eq(a, who))
            || comments.iter().any(|(l, _)| eq(l, who))
            || s(doc, "body")
                .to_ascii_lowercase()
                .contains(&format!("@{}", who.to_ascii_lowercase()));
        if !involved {
            return false;
        }
    }
    if is_pr {
        let requested = names_of(arr(doc, "requested_reviewers"));
        for r in q.values("review-requested") {
            if !requested.iter().any(|x| eq(x, r)) {
                return false;
            }
        }
        let reviewers: Vec<String> = crate::pulls::reviews_of(svc, key)
            .iter()
            .filter(|r| s(r, "state") != "PENDING")
            .map(|r| login_of(r.get("user").unwrap_or(&Value::Null)))
            .collect();
        for r in q.values("reviewed-by") {
            if !reviewers.iter().any(|x| eq(x, r)) {
                return false;
            }
        }
        if let Some(rv) = q.first("review") {
            let states: Vec<String> = crate::pulls::reviews_of(svc, key)
                .iter()
                .map(|r| s(r, "state").to_owned())
                .collect();
            let ok = match rv {
                "approved" => states.iter().any(|s| s == "APPROVED"),
                "changes_requested" => states.iter().any(|s| s == "CHANGES_REQUESTED"),
                "none" => states.is_empty(),
                "required" => !states.iter().any(|s| s == "APPROVED"),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        if let Some(h) = q.first("head")
            && !eq(h, &render::pr_head_ref(doc))
        {
            return false;
        }
        if let Some(bs) = q.first("base")
            && !eq(bs, &render::pr_base_ref(doc))
        {
            return false;
        }
        if let Some(d) = q.first("merged")
            && !date_matches(d, s(doc, "merged_at"))
        {
            return false;
        }
    } else if let Some(reason) = q.first("reason") {
        let want = reason.to_ascii_lowercase().replace(' ', "_");
        if s(doc, "state_reason") != want {
            return false;
        }
    }
    for (field, qual) in [
        ("created_at", "created"),
        ("updated_at", "updated"),
        ("closed_at", "closed"),
    ] {
        if let Some(d) = q.first(qual)
            && !date_matches(d, s(doc, field))
        {
            return false;
        }
    }
    if let Some(n) = q.first("comments")
        && !num_matches(n, comments.len() as f64)
    {
        return false;
    }
    let ins: Vec<String> = q
        .values("in")
        .iter()
        .flat_map(|v| v.split(','))
        .map(str::to_ascii_lowercase)
        .collect();
    let mut hay = String::new();
    if ins.is_empty() || ins.iter().any(|i| i == "title") {
        hay.push_str(&s(doc, "title").to_ascii_lowercase());
        hay.push('\n');
    }
    if ins.is_empty() || ins.iter().any(|i| i == "body") {
        hay.push_str(&s(doc, "body").to_ascii_lowercase());
        hay.push('\n');
    }
    if ins.iter().any(|i| i == "comments") || ins.is_empty() {
        for (_, c) in &comments {
            hay.push_str(&c.to_ascii_lowercase());
            hay.push('\n');
        }
    }
    // `#123` / `123` matches the number.
    if q.terms.len() == 1
        && let Some(n) = q.terms[0].trim_start_matches('#').parse::<u64>().ok()
        && n == number
    {
        return true;
    }
    q.text_matches(&hay)
}

fn search_issues(ctx: &Ctx, q: &Query) -> Response {
    let mut hits: Vec<(String, bool, usize)> = Vec::new();
    for (coll, is_pr) in [("issues", false), ("pulls", true)] {
        for (k, d) in all(ctx.svc, coll) {
            if issue_matches(ctx.svc, q, k, d, is_pr) {
                let hay = format!("{} {}", s(d, "title"), s(d, "body")).to_ascii_lowercase();
                hits.push((k.to_owned(), is_pr, q.score(&hay)));
            }
        }
    }
    let (sort, asc) = wants_order(ctx);
    let doc_of = |k: &str, pr: bool| {
        get(ctx.svc, if pr { "pulls" } else { "issues" }, k)
            .cloned()
            .unwrap_or_default()
    };
    match sort.as_deref() {
        Some("created") | Some("updated") => {
            let field = format!("{}_at", sort.as_deref().unwrap_or("created"));
            hits.sort_by_key(|(k, pr, _)| unix(s(&doc_of(k, *pr), &field)).unwrap_or(0));
            if !asc {
                hits.reverse();
            }
        }
        Some("comments") => {
            hits.sort_by_key(|(k, _, _)| {
                let (r, n) = split_key(k);
                render::comment_count(ctx.svc, r, n)
            });
            if !asc {
                hits.reverse();
            }
        }
        _ => {
            hits.sort_by_key(|(k, pr, score)| {
                (
                    std::cmp::Reverse(*score),
                    std::cmp::Reverse(unix(s(&doc_of(k, *pr), "updated_at")).unwrap_or(0)),
                )
            });
        }
    }
    let items: Vec<Value> = hits
        .iter()
        .map(|(k, _, _)| {
            let mut v = issue_json(ctx.svc, ctx.base(), k);
            v["score"] = json!(1.0);
            v
        })
        .collect();
    paged_search(ctx, items)
}

fn paged_search(ctx: &Ctx, items: Vec<Value>) -> Response {
    let params = ctx.req.page();
    let total = items.len();
    let page = mock_core::http::paginate(&items, params);
    let resp =
        crate::util::ok(json!({"total_count": total, "incomplete_results": false, "items": page}));
    crate::util::link(ctx.req, resp, params, total)
}

// ---------------------------------------------------------------- code

fn search_code(ctx: &Ctx, q: &Query) -> Response {
    let paths = q.values("path");
    let filenames = q.values("filename");
    let exts = q.values("extension");
    let langs = q.values("language");
    let ins: Vec<String> = q
        .values("in")
        .iter()
        .flat_map(|v| v.split(','))
        .map(str::to_ascii_lowercase)
        .collect();
    let mut items = Vec::new();
    for (repo, r) in all(ctx.svc, "repos") {
        if !repo_in_scope(q, repo) || b(r, "archived") && q.first("archived") != Some("true") {
            continue;
        }
        // Forks are only searched when asked for (or named explicitly).
        if b(r, "fork") && q.first("fork").is_none() && q.values("repo").is_empty() {
            continue;
        }
        let Some(sha) = git::resolve_ref(ctx.svc, repo, "") else {
            continue;
        };
        let Some(map) = git::file_map(ctx.svc, repo, &sha) else {
            continue;
        };
        for (path, e) in &map {
            let name = path.rsplit('/').next().unwrap_or(path);
            if !paths.iter().all(|p| {
                let p = p.trim_start_matches('/');
                path.starts_with(p) || glob_path(p, path)
            }) {
                continue;
            }
            if !filenames.iter().all(|f| eq(f, name)) {
                continue;
            }
            if !exts.iter().all(|x| {
                name.to_ascii_lowercase()
                    .ends_with(&format!(".{}", x.to_ascii_lowercase()))
            }) {
                continue;
            }
            if !langs
                .iter()
                .all(|l| language_of(name).is_some_and(|ln| eq(ln, l)))
            {
                continue;
            }
            let content = String::from_utf8_lossy(&e.content).to_ascii_lowercase();
            let hay = if ins.iter().any(|i| i == "path") {
                path.to_ascii_lowercase()
            } else if ins.iter().any(|i| i == "file") {
                content.clone()
            } else {
                format!("{content}\n{}", path.to_ascii_lowercase())
            };
            if q.terms.is_empty()
                && paths.is_empty()
                && filenames.is_empty()
                && exts.is_empty()
                && langs.is_empty()
            {
                continue;
            }
            if !q.text_matches(&hay) {
                continue;
            }
            let blob = blob_sha(&e.content);
            let enc = percent_encode_path(path);
            items.push(json!({
                "name": name,
                "path": path,
                "sha": blob,
                "url": format!("{}/repos/{repo}/contents/{enc}?ref={sha}", ctx.req.api()),
                "git_url": format!("{}/repos/{repo}/git/blobs/{blob}", ctx.req.api()),
                "html_url": format!("{}/{repo}/blob/{sha}/{enc}", ctx.base()),
                "repository": repo_min(ctx.svc, ctx.base(), repo),
                "score": 1.0,
            }));
        }
    }
    paged_search(ctx, items)
}

fn glob_path(pattern: &str, path: &str) -> bool {
    if !pattern.contains('*') {
        return path.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut idx = 0;
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        match path[idx..].find(p) {
            Some(pos) if i > 0 || pos == 0 => idx += pos + p.len(),
            _ => return false,
        }
    }
    true
}

fn language_of(name: &str) -> Option<&'static str> {
    Some(match name.rsplit('.').next()? {
        "rs" => "Rust",
        "go" => "Go",
        "py" => "Python",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" => "JavaScript",
        "rb" => "Ruby",
        "java" => "Java",
        "sh" => "Shell",
        "yml" | "yaml" => "YAML",
        "json" => "JSON",
        "md" => "Markdown",
        "tf" => "HCL",
        "toml" => "TOML",
        "sql" => "SQL",
        _ => return None,
    })
}

// ---------------------------------------------------------------- repositories

fn search_repos(ctx: &Ctx, q: &Query) -> Response {
    let ins: Vec<String> = q
        .values("in")
        .iter()
        .flat_map(|v| v.split(','))
        .map(str::to_ascii_lowercase)
        .collect();
    let mut hits: Vec<(&str, &Value, usize)> = Vec::new();
    for (key, r) in all(ctx.svc, "repos") {
        if !repo_in_scope(q, key) {
            continue;
        }
        let topics: Vec<String> = arr(r, "topics")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_ascii_lowercase)
            .collect();
        if !q
            .values("topic")
            .iter()
            .all(|t| topics.contains(&t.to_ascii_lowercase()))
        {
            continue;
        }
        if !q.values("language").iter().all(|l| eq(l, s(r, "language"))) {
            continue;
        }
        let mut ok = true;
        for v in q.values("is") {
            ok &= match v {
                "public" => !b(r, "private"),
                "private" => b(r, "private"),
                "fork" => b(r, "fork"),
                "archived" => b(r, "archived"),
                _ => true,
            };
        }
        if let Some(a) = q.first("archived") {
            ok &= b(r, "archived") == (a == "true");
        }
        if let Some(f) = q.first("fork") {
            ok &= f == "true" || f == "only" || !b(r, "fork");
            if f == "only" {
                ok &= b(r, "fork");
            }
        } else if b(r, "fork") && q.values("repo").is_empty() {
            ok = false;
        }
        if let Some(st) = q.first("stars") {
            ok &= num_matches(st, u(r, "stargazers_count") as f64);
        }
        if let Some(fk) = q.first("forks") {
            ok &= num_matches(fk, u(r, "forks_count") as f64);
        }
        for (field, qual) in [("created_at", "created"), ("pushed_at", "pushed")] {
            if let Some(d) = q.first(qual) {
                ok &= date_matches(d, s(r, field));
            }
        }
        if !ok {
            continue;
        }
        let name = key.split('/').nth(1).unwrap_or("").to_ascii_lowercase();
        let mut hay = String::new();
        if ins.is_empty() || ins.iter().any(|i| i == "name") {
            hay.push_str(&name);
            hay.push(' ');
            hay.push_str(&key.to_ascii_lowercase());
            hay.push(' ');
        }
        if ins.is_empty() || ins.iter().any(|i| i == "description") {
            hay.push_str(&s(r, "description").to_ascii_lowercase());
            hay.push(' ');
        }
        if ins.is_empty() || ins.iter().any(|i| i == "topics") {
            hay.push_str(&topics.join(" "));
        }
        if ins.iter().any(|i| i == "readme")
            && let Some(sha) = git::resolve_ref(ctx.svc, key, "")
            && let Some(map) = git::file_map(ctx.svc, key, &sha)
        {
            for (p, e) in &map {
                if p.to_ascii_lowercase().starts_with("readme") {
                    hay.push_str(&String::from_utf8_lossy(&e.content).to_ascii_lowercase());
                }
            }
        }
        if !q.text_matches(&hay) {
            continue;
        }
        let score = q.score(&hay) + usize::from(q.terms.contains(&name)) * 10;
        hits.push((key, r, score));
    }
    let (sort, asc) = wants_order(ctx);
    match sort.as_deref() {
        Some("stars") => hits.sort_by_key(|(_, r, _)| u(r, "stargazers_count")),
        Some("forks") => hits.sort_by_key(|(_, r, _)| u(r, "forks_count")),
        Some("updated") => {
            hits.sort_by_key(|(_, r, _)| unix(s(r, "updated_at")).unwrap_or(0) as u64)
        }
        _ => {
            hits.sort_by_key(|(k, r, sc)| {
                (
                    std::cmp::Reverse(*sc),
                    std::cmp::Reverse(u(r, "stargazers_count")),
                    k.to_owned(),
                )
            });
        }
    }
    if sort.is_some() && !asc {
        hits.reverse();
    }
    let items: Vec<Value> = hits
        .iter()
        .map(|(k, _, _)| {
            let mut v = repo_json(ctx.svc, ctx.base(), k);
            v["score"] = json!(1.0);
            v
        })
        .collect();
    paged_search(ctx, items)
}

// ---------------------------------------------------------------- users / orgs

fn search_users(ctx: &Ctx, q: &Query) -> Response {
    let ins: Vec<String> = q
        .values("in")
        .iter()
        .flat_map(|v| v.split(','))
        .map(str::to_ascii_lowercase)
        .collect();
    let mut logins: Vec<String> = Vec::new();
    for coll in ["users", "orgs"] {
        for (k, d) in all(ctx.svc, coll) {
            let l = opt_s(d, "login").unwrap_or_else(|| k.to_owned());
            if !logins.iter().any(|x| eq(x, &l)) {
                logins.push(l);
            }
        }
    }
    if let Some(v) = get(ctx.svc, "viewer", "viewer") {
        let l = s(v, "login").to_owned();
        if !l.is_empty() && !logins.iter().any(|x| eq(x, &l)) {
            logins.push(l);
        }
    }
    let mut hits: Vec<(String, usize)> = Vec::new();
    for login in logins {
        let doc = render::account(ctx.svc, &login)
            .cloned()
            .unwrap_or_default();
        let kind = account_type(ctx.svc, &login);
        if let Some(t) = q.first("type") {
            let want = if eq(t, "org") { "Organization" } else { "User" };
            if kind != want {
                continue;
            }
        }
        if let Some(loc) = q.first("location")
            && !s(&doc, "location")
                .to_ascii_lowercase()
                .contains(&loc.to_ascii_lowercase())
        {
            continue;
        }
        if let Some(f) = q.first("followers")
            && !num_matches(f, u(&doc, "followers") as f64)
        {
            continue;
        }
        if let Some(r) = q.first("repos") {
            let n = all(ctx.svc, "repos")
                .iter()
                .filter(|(_, rd)| eq(&render::repo_owner(rd), &login))
                .count();
            if !num_matches(r, n as f64) {
                continue;
            }
        }
        if let Some(d) = q.first("created")
            && !date_matches(d, s(&doc, "created_at"))
        {
            continue;
        }
        let mut hay = String::new();
        if ins.is_empty() || ins.iter().any(|i| i == "login") {
            hay.push_str(&login.to_ascii_lowercase());
            hay.push(' ');
        }
        if ins.is_empty() || ins.iter().any(|i| i == "name" || i == "fullname") {
            hay.push_str(&s(&doc, "name").to_ascii_lowercase());
            hay.push(' ');
        }
        if ins.is_empty() || ins.iter().any(|i| i == "email") {
            hay.push_str(&s(&doc, "email").to_ascii_lowercase());
        }
        if !q.text_matches(&hay) {
            continue;
        }
        let score = q.score(&hay) + usize::from(q.terms.iter().any(|t| eq(t, &login))) * 10;
        hits.push((login, score));
    }
    let (sort, asc) = wants_order(ctx);
    match sort.as_deref() {
        Some("followers") => hits.sort_by_key(|(l, _)| {
            render::account(ctx.svc, l)
                .map(|d| u(d, "followers"))
                .unwrap_or(0)
        }),
        Some("repositories") => hits.sort_by_key(|(l, _)| {
            all(ctx.svc, "repos")
                .iter()
                .filter(|(_, r)| eq(&render::repo_owner(r), l))
                .count() as u64
        }),
        Some("joined") => hits.sort_by_key(|(l, _)| {
            render::account(ctx.svc, l)
                .map(|d| unix(s(d, "created_at")).unwrap_or(0))
                .unwrap_or(0) as u64
        }),
        _ => hits.sort_by_key(|(l, sc)| (std::cmp::Reverse(*sc), l.to_ascii_lowercase())),
    }
    if sort.is_some() && !asc {
        hits.reverse();
    }
    let items: Vec<Value> = hits
        .iter()
        .map(|(l, _)| {
            let mut v = user(ctx.svc, ctx.base(), l);
            if is_org(ctx.svc, l) {
                v["type"] = json!("Organization");
            }
            v["score"] = json!(1.0);
            v
        })
        .collect();
    paged_search(ctx, items)
}

// ---------------------------------------------------------------- commits

fn search_commits(ctx: &Ctx, q: &Query) -> Response {
    let mut hits: Vec<(String, Value)> = Vec::new();
    for (repo, _) in all(ctx.svc, "repos") {
        if !repo_in_scope(q, repo) {
            continue;
        }
        let Some(head) = git::resolve_ref(ctx.svc, repo, "") else {
            continue;
        };
        for c in git::log(ctx.svc, repo, &head) {
            let author_login = render::commit_login(ctx.svc, &c, "author").unwrap_or_default();
            let committer_login =
                render::commit_login(ctx.svc, &c, "committer").unwrap_or_default();
            let a_email = c
                .get("author")
                .and_then(|a| a.get("email"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let a_name = c
                .get("author")
                .and_then(|a| a.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let a_date = c
                .get("author")
                .and_then(|a| a.get("date"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let sha = s(&c, "sha");
            let mut ok = q.values("author").iter().all(|a| eq(a, &author_login))
                && q.values("committer")
                    .iter()
                    .all(|a| eq(a, &committer_login))
                && q.values("author-email").iter().all(|a| eq(a, a_email))
                && q.values("author-name").iter().all(|a| {
                    a_name
                        .to_ascii_lowercase()
                        .contains(&a.to_ascii_lowercase())
                })
                && q.values("hash")
                    .iter()
                    .all(|h| sha.starts_with(&h.to_ascii_lowercase()))
                && q.values("parent")
                    .iter()
                    .all(|p| git::parents(&c).iter().any(|x| x.starts_with(p)));
            if let Some(d) = q.first("author-date") {
                ok &= date_matches(d, a_date);
            }
            if let Some(d) = q.first("committer-date") {
                ok &= date_matches(d, git::commit_date(&c));
            }
            if let Some(mg) = q.first("merge") {
                ok &= (git::parents(&c).len() > 1) == (mg == "true");
            }
            if !ok || !q.text_matches(&s(&c, "message").to_ascii_lowercase()) {
                continue;
            }
            if q.terms.is_empty()
                && q.quals
                    .iter()
                    .all(|(k, _, _)| matches!(k.as_str(), "repo" | "org" | "user" | "owner"))
            {
                // GitHub requires some search text or a non-scope qualifier.
            }
            hits.push((repo.to_owned(), c));
        }
    }
    let (sort, asc) = wants_order(ctx);
    let date_of = |c: &Value, key: &str| {
        unix(
            c.get(key)
                .and_then(|a| a.get("date"))
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
        .unwrap_or(0)
    };
    match sort.as_deref() {
        Some("committer-date") => hits.sort_by_key(|(_, c)| date_of(c, "committer")),
        _ => hits.sort_by_key(|(_, c)| date_of(c, "author")),
    }
    if !asc {
        hits.reverse();
    }
    let items: Vec<Value> = hits
        .iter()
        .map(|(repo, c)| {
            let mut v = commit_json(ctx.svc, ctx.base(), repo, c, false);
            if let Some(o) = v.as_object_mut() {
                o.remove("node_id");
            }
            v["repository"] = repo_min(ctx.svc, ctx.base(), repo);
            v["score"] = json!(1.0);
            if let Some(cm) = v.get_mut("commit").and_then(Value::as_object_mut) {
                cm.remove("verification");
            }
            v
        })
        .collect();
    paged_search(ctx, items)
}

#[allow(dead_code)]
fn label_exists(svc: &ServiceState, repo: &str, name: &str) -> bool {
    label_doc(svc, repo, name).is_some()
}
