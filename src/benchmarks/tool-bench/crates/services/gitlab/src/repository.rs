//! Repository: files, tree, branches, commits, compare, blame, protected branches, plus the
//! ref/overlay helpers other modules use.
//!
//! Model: `branches` (`<project>:<branch>` -> `commit`, `base`), `files`
//! (`<project>:<branch>:<path>` -> `content` or `deleted`; a branch inherits the files of its
//! `base` chain), `commits` (`<project>@<sha>`, with `parent_ids` and `branch`), `tags`
//! (`<project>:<tag>` -> `commit`), `protected_branches` (`<project>:<name>`).

use std::collections::{BTreeMap, BTreeSet};

use axum::{
    Router,
    http::StatusCode,
    response::Response,
    routing::{delete, get as rget},
};
use mock_core::{
    store::Store,
    world::{SharedWorld, WorldState},
};
use serde_json::{Value, json};

use crate::common::*;

pub fn routes() -> Router<SharedWorld> {
    let f = "/projects/{id}/repository/files/{file_path}";
    Router::new()
        .route(
            f,
            rget(wrap(get_file))
                .head(wrap(get_file))
                .post(wrap(create_file))
                .put(wrap(update_file))
                .delete(wrap(delete_file)),
        )
        .route(&format!("{f}/raw"), rget(wrap(raw_file)))
        .route(&format!("{f}/blame"), rget(wrap(blame)))
        .route("/projects/{id}/repository/tree", rget(wrap(tree)))
        .route("/projects/{id}/repository/archive", rget(wrap(archive)))
        .route(
            "/projects/{id}/repository/contributors",
            rget(wrap(contributors)),
        )
        .route(
            "/projects/{id}/repository/branches",
            rget(wrap(list_branches)).post(wrap(create_branch)),
        )
        .route(
            "/projects/{id}/repository/branches/{branch}",
            rget(wrap(get_branch))
                .head(wrap(get_branch))
                .delete(wrap(delete_branch)),
        )
        .route(
            "/projects/{id}/repository/merged_branches",
            delete(wrap(delete_merged)),
        )
        .route(
            "/projects/{id}/repository/commits",
            rget(wrap(list_commits)).post(wrap(create_commit)),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}",
            rget(wrap(get_commit)),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/diff",
            rget(wrap(commit_diff)),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/refs",
            rget(wrap(commit_refs)),
        )
        .route(
            "/projects/{id}/repository/commits/{sha}/merge_requests",
            rget(wrap(commit_mrs)),
        )
        .route("/projects/{id}/repository/compare", rget(wrap(compare)))
        .route(
            "/projects/{id}/protected_branches",
            rget(wrap(list_protected)).post(wrap(protect)),
        )
        .route(
            "/projects/{id}/protected_branches/{branch}",
            rget(wrap(get_protected))
                .patch(wrap(update_protected))
                .delete(wrap(unprotect)),
        )
}

// ---------------------------------------------------------------------------
// Ref and file helpers

pub(crate) fn default_branch(st: &Store, project: &str) -> String {
    get(st, "projects", project)
        .and_then(|p| p.get("default_branch")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "main".into())
}

pub(crate) fn branch_doc<'a>(st: &'a Store, project: &str, branch: &str) -> Option<&'a Value> {
    get(st, "branches", &format!("{project}:{branch}"))
}

/// Head commit sha of a branch.
pub(crate) fn branch_head(st: &Store, project: &str, branch: &str) -> Option<String> {
    branch_doc(st, project, branch).map(|b| str_of(b, "commit").to_owned())
}

pub(crate) fn commit_doc<'a>(st: &'a Store, project: &str, sha: &str) -> Option<&'a Value> {
    if sha.len() < 7 {
        return None;
    }
    if let Some(c) = get(st, "commits", &format!("{project}@{sha}")) {
        return Some(c);
    }
    let prefix = format!("{project}@{sha}");
    all(st, "commits")
        .into_iter()
        .find(|(k, _)| k.starts_with(&prefix))
        .map(|(_, c)| c)
}

/// A resolved ref: the branch whose files to read, and the commit sha.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub branch: String,
    pub sha: String,
}

/// Resolves a branch name, tag name or (short) sha.
pub(crate) fn resolve_ref(st: &Store, project: &str, r: &str) -> Option<Snapshot> {
    let r = r
        .strip_prefix("refs/heads/")
        .or_else(|| r.strip_prefix("refs/tags/"))
        .unwrap_or(r);
    if r == "HEAD" {
        let b = default_branch(st, project);
        return branch_head(st, project, &b).map(|sha| Snapshot { branch: b, sha });
    }
    if let Some(sha) = branch_head(st, project, r) {
        return Some(Snapshot {
            branch: r.to_owned(),
            sha,
        });
    }
    let sha = if let Some(t) = get(st, "tags", &format!("{project}:{r}")) {
        str_of(t, "commit").to_owned()
    } else if r.chars().all(|c| c.is_ascii_hexdigit()) && r.len() >= 7 {
        match commit_doc(st, project, r) {
            Some(c) => str_of(c, "id").to_owned(),
            None => all(st, "branches")
                .into_iter()
                .find(|(k, b)| {
                    k.starts_with(&format!("{project}:")) && str_of(b, "commit").starts_with(r)
                })
                .map(|(_, b)| str_of(b, "commit").to_owned())?,
        }
    } else {
        return None;
    };
    // Which branch's files represent this commit: a branch pointing at it, else the commit's
    // recorded branch, else the default branch.
    let branch = all(st, "branches")
        .into_iter()
        .find(|(k, b)| k.starts_with(&format!("{project}:")) && str_of(b, "commit") == sha)
        .map(|(_, b)| str_of(b, "name").to_owned())
        .or_else(|| {
            commit_doc(st, project, &sha)
                .and_then(|c| c.get("branch")?.as_str().map(str::to_owned))
                .filter(|b| branch_doc(st, project, b).is_some())
        })
        .unwrap_or_else(|| default_branch(st, project));
    Some(Snapshot { branch, sha })
}

/// Branch overlay chain: the branch, its base, its base's base...
fn chain(st: &Store, project: &str, branch: &str) -> Vec<String> {
    let mut out = vec![branch.to_owned()];
    let mut cur = branch.to_owned();
    while let Some(base) =
        branch_doc(st, project, &cur).and_then(|b| b.get("base")?.as_str().map(str::to_owned))
    {
        if out.contains(&base) {
            break;
        }
        out.push(base.clone());
        cur = base;
    }
    out
}

/// All files visible on a branch (`path -> content`).
pub(crate) fn files_at(st: &Store, project: &str, branch: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    for b in chain(st, project, branch) {
        let prefix = format!("{project}:{b}:");
        for (k, f) in all(st, "files") {
            if let Some(path) = k.strip_prefix(&prefix) {
                if out.contains_key(path) || deleted.contains(path) {
                    continue;
                }
                if f.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
                    deleted.insert(path.to_owned());
                } else {
                    out.insert(path.to_owned(), str_of(f, "content").to_owned());
                }
            }
        }
    }
    out
}

pub(crate) fn file_at(st: &Store, project: &str, branch: &str, path: &str) -> Option<String> {
    for b in chain(st, project, branch) {
        if let Some(f) = get(st, "files", &format!("{project}:{b}:{path}")) {
            if f.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
                return None;
            }
            return Some(str_of(f, "content").to_owned());
        }
    }
    None
}

/// Last commit touching `path` on a branch (by recorded diffs), else the branch head.
fn last_commit_for(st: &Store, project: &str, snap: &Snapshot, path: &str) -> String {
    let reach = reachable(st, project, &snap.sha);
    let mut best: Option<(String, String)> = None;
    for sha in &reach {
        if let Some(c) = commit_doc(st, project, sha) {
            let touches = c.get("diffs").and_then(Value::as_array).is_some_and(|ds| {
                ds.iter()
                    .any(|d| str_of(d, "new_path") == path || str_of(d, "old_path") == path)
            });
            if touches {
                let date = str_of(c, "committed_date").to_owned();
                if best.as_ref().is_none_or(|(d, _)| date > *d) {
                    best = Some((date, sha.clone()));
                }
            }
        }
    }
    best.map(|(_, s)| s).unwrap_or_else(|| snap.sha.clone())
}

/// Commit shas reachable from `sha` through `parent_ids`.
pub(crate) fn reachable(st: &Store, project: &str, sha: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![sha.to_owned()];
    while let Some(s) = stack.pop() {
        if out.contains(&s) {
            continue;
        }
        let Some(c) = commit_doc(st, project, &s) else {
            continue;
        };
        out.push(str_of(c, "id").to_owned());
        for p in value_list(&c["parent_ids"]) {
            stack.push(p);
        }
    }
    out
}

pub(crate) fn blob_id(project: &str, path: &str, content: &str) -> String {
    let _ = project;
    fake_sha(&format!("blob:{path}:{content}"))
}

/// GitLab commit JSON for a sha (synthesized when the dataset has no doc).
pub(crate) fn render_commit(st: &Store, base: &str, project: &str, sha: &str) -> Value {
    let c = commit_doc(st, project, sha).cloned().unwrap_or_else(|| {
        json!({"id": sha, "title": "Update", "message": "Update\n", "author_name": "GitLab", "author_email": "noreply@example.com",
               "authored_date": "2026-01-01T00:00:00.000Z", "committed_date": "2026-01-01T00:00:00.000Z", "parent_ids": []})
    });
    let id = str_of(&c, "id").to_owned();
    let message = c
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}\n", str_of(&c, "title")));
    let title = c
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| message.lines().next().unwrap_or("").to_owned());
    let authored = c
        .get("authored_date")
        .or(c.get("committed_date"))
        .cloned()
        .unwrap_or(Value::Null);
    let committed = c
        .get("committed_date")
        .or(c.get("authored_date"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "id": id,
        "short_id": short_sha(&id),
        "created_at": committed,
        "parent_ids": c.get("parent_ids").cloned().unwrap_or(json!([])),
        "title": title,
        "message": message,
        "author_name": str_of(&c, "author_name"),
        "author_email": str_of(&c, "author_email"),
        "authored_date": authored,
        "committer_name": c.get("committer_name").and_then(Value::as_str).unwrap_or(str_of(&c, "author_name")),
        "committer_email": c.get("committer_email").and_then(Value::as_str).unwrap_or(str_of(&c, "author_email")),
        "committed_date": committed,
        "trailers": {},
        "extended_trailers": {},
        "web_url": format!("{base}/{project}/-/commit/{id}"),
    })
}

fn commit_stats(st: &Store, project: &str, sha: &str) -> Value {
    let diffs = commit_diffs(st, project, sha);
    let (mut add, mut del) = (0, 0);
    for d in &diffs {
        for l in str_of(d, "diff").lines() {
            if l.starts_with('+') && !l.starts_with("+++") {
                add += 1;
            } else if l.starts_with('-') && !l.starts_with("---") {
                del += 1;
            }
        }
    }
    json!({"additions": add, "deletions": del, "total": add + del})
}

/// Recorded diffs of a commit, else the diff against its first parent's snapshot.
fn commit_diffs(st: &Store, project: &str, sha: &str) -> Vec<Value> {
    let Some(c) = commit_doc(st, project, sha) else {
        return Vec::new();
    };
    if let Some(d) = c.get("diffs").and_then(Value::as_array) {
        return d.clone();
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Diffs

/// Unified diff hunks (3 lines of context) between two texts, GitLab style (no file header).
pub(crate) fn unified_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    // LCS table (files in datasets are small).
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // Edit script: (' ' | '-' | '+', old line no, new line no, text)
    let mut ops: Vec<(char, usize, usize, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', i, j, a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', i, j, a[i]));
            i += 1;
        } else {
            ops.push(('+', i, j, b[j]));
            j += 1;
        }
    }
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != ' ')
        .map(|(k, _)| k)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    // Group changes into hunks with 3 lines of context.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &k in &changed {
        let start = k.saturating_sub(3);
        let end = (k + 3).min(ops.len() - 1);
        match hunks.last_mut() {
            Some(h) if start <= h.1 + 1 => h.1 = end.max(h.1),
            _ => hunks.push((start, end)),
        }
    }
    let mut out = String::new();
    for (s, e) in hunks {
        let slice = &ops[s..=e];
        let old_start = slice
            .iter()
            .find(|o| o.0 != '+')
            .map_or(slice[0].1, |o| o.1)
            + 1;
        let new_start = slice
            .iter()
            .find(|o| o.0 != '-')
            .map_or(slice[0].2, |o| o.2)
            + 1;
        let old_len = slice.iter().filter(|o| o.0 != '+').count();
        let new_len = slice.iter().filter(|o| o.0 != '-').count();
        let old_start = if old_len == 0 {
            old_start - 1
        } else {
            old_start
        };
        let new_start = if new_len == 0 {
            new_start - 1
        } else {
            new_start
        };
        out.push_str(&format!(
            "@@ -{old_start},{old_len} +{new_start},{new_len} @@\n"
        ));
        for o in slice {
            out.push(o.0);
            out.push_str(o.3);
            out.push('\n');
        }
    }
    out
}

fn diff_entry(path: &str, old: Option<&str>, new: Option<&str>) -> Value {
    let diff = unified_diff(old.unwrap_or(""), new.unwrap_or(""));
    json!({
        "diff": diff,
        "new_path": path,
        "old_path": path,
        "a_mode": if old.is_some() { "100644" } else { "0" },
        "b_mode": if new.is_some() { "100644" } else { "0" },
        "new_file": old.is_none(),
        "renamed_file": false,
        "deleted_file": new.is_none(),
        "generated_file": false,
    })
}

/// File-level diffs between two snapshots.
fn snapshot_diffs(st: &Store, project: &str, from: &Snapshot, to: &Snapshot) -> Vec<Value> {
    let a = files_at(st, project, &from.branch);
    let b = files_at(st, project, &to.branch);
    let paths: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    paths
        .into_iter()
        .filter(|p| a.get(*p) != b.get(*p))
        .map(|p| {
            diff_entry(
                p,
                a.get(p).map(String::as_str),
                b.get(p).map(String::as_str),
            )
        })
        .collect()
}

/// `(commits newest first, diffs)` from `from` to `to`.
pub(crate) fn compare_refs(
    st: &Store,
    project: &str,
    from: &str,
    to: &str,
) -> Option<(Vec<Value>, Vec<Value>)> {
    let f = resolve_ref(st, project, from)?;
    let t = resolve_ref(st, project, to)?;
    let base: BTreeSet<String> = reachable(st, project, &f.sha).into_iter().collect();
    let mut commits: Vec<Value> = reachable(st, project, &t.sha)
        .into_iter()
        .filter(|s| !base.contains(s))
        .filter_map(|s| commit_doc(st, project, &s).cloned())
        .collect();
    commits.sort_by(|a, b| str_of(b, "committed_date").cmp(str_of(a, "committed_date")));
    let diffs = if f.branch == t.branch && f.sha != t.sha {
        // Same branch, different points: use recorded commit diffs.
        commits
            .iter()
            .rev()
            .flat_map(|c| commit_diffs(st, project, str_of(c, "id")))
            .collect()
    } else {
        snapshot_diffs(st, project, &f, &t)
    };
    Some((commits, diffs))
}

// ---------------------------------------------------------------------------
// Mutation helpers

fn author_of(st: &Store, username: &str) -> (String, String) {
    user(st, username)
        .map(|u| (str_of(u, "name").to_owned(), str_of(u, "email").to_owned()))
        .unwrap_or_else(|| (username.to_owned(), format!("{username}@example.com")))
}

/// Records a commit on `branch` (moving its head) and returns the sha.
#[allow(clippy::too_many_arguments)]
fn add_commit(
    st: &mut Store,
    project: &str,
    branch: &str,
    message: &str,
    username: &str,
    now: &str,
    diffs: Vec<Value>,
    extra_parents: Vec<String>,
) -> String {
    let parent = branch_head(st, project, branch);
    let n = all(st, "commits").len();
    let sha = fake_sha(&format!("{project}:{branch}:{n}:{message}:{now}"));
    let (name, email) = author_of(st, username);
    let mut parents: Vec<String> = parent.into_iter().collect();
    parents.extend(extra_parents);
    let title = message.lines().next().unwrap_or("").to_owned();
    st.collection_mut("commits").insert(
        format!("{project}@{sha}"),
        json!({
            "id": sha,
            "title": title,
            "message": message,
            "author_name": name,
            "author_email": email,
            "authored_date": now,
            "committer_name": name,
            "committer_email": email,
            "committed_date": now,
            "parent_ids": parents,
            "branch": branch,
            "diffs": diffs,
        }),
    );
    let key = format!("{project}:{branch}");
    match st.collection_mut("branches").get_mut(&key) {
        Some(b) => b["commit"] = json!(sha),
        None => {
            st.collection_mut("branches")
                .insert(key, json!({"name": branch, "commit": sha, "base": null}));
        }
    }
    if let Some(p) = st.collection_mut("projects").get_mut(project) {
        p["last_activity_at"] = json!(now);
    }
    sha
}

fn write_file(st: &mut Store, project: &str, branch: &str, path: &str, content: Option<&str>) {
    let doc = match content {
        Some(c) => json!({"content": c}),
        None => json!({"deleted": true}),
    };
    st.collection_mut("files")
        .insert(format!("{project}:{branch}:{path}"), doc);
}

/// Applies the source branch's files onto the target branch and records a merge commit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_branches(
    st: &mut Store,
    project: &str,
    source_project: &str,
    source: &str,
    target: &str,
    message: &str,
    username: &str,
    now: &str,
) -> String {
    let src = files_at(st, source_project, source);
    let dst = files_at(st, project, target);
    let mut diffs = Vec::new();
    for (path, content) in &src {
        if dst.get(path) != Some(content) {
            diffs.push(diff_entry(
                path,
                dst.get(path).map(String::as_str),
                Some(content),
            ));
            write_file(st, project, target, path, Some(content));
        }
    }
    // Files the source branch deleted relative to its base.
    for path in dst.keys() {
        if !src.contains_key(path) && branch_doc(st, source_project, source).is_some() {
            let deleted_on_source = chain(st, source_project, source).iter().any(|b| {
                get(st, "files", &format!("{source_project}:{b}:{path}"))
                    .is_some_and(|f| f.get("deleted").and_then(Value::as_bool).unwrap_or(false))
            });
            if deleted_on_source {
                diffs.push(diff_entry(path, dst.get(path).map(String::as_str), None));
                write_file(st, project, target, path, None);
            }
        }
    }
    let source_head: Vec<String> = branch_head(st, source_project, source)
        .into_iter()
        .collect();
    add_commit(
        st,
        project,
        target,
        message,
        username,
        now,
        diffs,
        source_head,
    )
}

pub(crate) fn remove_branch(st: &mut Store, project: &str, branch: &str) {
    remove_doc(st, "branches", &format!("{project}:{branch}"));
}

// ---------------------------------------------------------------------------
// Files

fn file_ref(st: &Store, r: &Req, project: &str) -> Result<Snapshot, Response> {
    let Some(reference) = r.q("ref") else {
        return Err(missing("ref"));
    };
    resolve_ref(st, project, reference).ok_or_else(|| not_found("Commit"))
}

fn get_file(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let snap = match file_ref(st, r, &project) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = r.p("file_path");
    let Some(content) = file_at(st, &project, &snap.branch, path) else {
        return not_found("File");
    };
    let last = last_commit_for(st, &project, &snap, path);
    let digest = {
        use sha2::{Digest, Sha256};
        Sha256::digest(content.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    ok(json!({
        "file_name": last_segment(path),
        "file_path": path,
        "size": content.len(),
        "encoding": "base64",
        "content_sha256": digest,
        "ref": r.q("ref").unwrap_or(""),
        "blob_id": blob_id(&project, path, &content),
        "commit_id": snap.sha,
        "last_commit_id": last,
        "execute_filemode": false,
        "content": b64_encode(content.as_bytes()),
    }))
}

fn raw_file(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let reference = r
        .q("ref")
        .map(str::to_owned)
        .unwrap_or_else(|| default_branch(st, &project));
    let Some(snap) = resolve_ref(st, &project, &reference) else {
        return not_found("Commit");
    };
    match file_at(st, &project, &snap.branch, r.p("file_path")) {
        Some(c) => text(StatusCode::OK, "text/plain; charset=utf-8", c),
        None => not_found("File"),
    }
}

fn blame(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let snap = match file_ref(st, r, &project) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = r.p("file_path");
    let Some(content) = file_at(st, &project, &snap.branch, path) else {
        return not_found("File");
    };
    let sha = last_commit_for(st, &project, &snap, path);
    let commit = render_commit(st, &r.base, &project, &sha);
    let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
    let start = r.q("range[start]").and_then(|s| s.parse::<usize>().ok());
    let end = r.q("range[end]").and_then(|s| s.parse::<usize>().ok());
    if let (Some(s), Some(e)) = (start, end) {
        if s == 0 || e < s || s > lines.len() {
            return bad_request("range[start] is invalid");
        }
        lines = lines[s - 1..e.min(lines.len())].to_vec();
    }
    ok(json!([{
        "commit": {
            "id": commit["id"],
            "parent_ids": commit["parent_ids"],
            "message": commit["message"],
            "authored_date": commit["authored_date"],
            "author_name": commit["author_name"],
            "author_email": commit["author_email"],
            "committed_date": commit["committed_date"],
            "committer_name": commit["committer_name"],
            "committer_email": commit["committer_email"],
        },
        "lines": lines,
    }]))
}

fn content_from_body(r: &Req) -> Result<String, Response> {
    let Some(content) = r.s("content") else {
        return Err(missing("content"));
    };
    if r.s("encoding").as_deref() == Some("base64") {
        b64_decode(&content)
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| bad_request("content is invalid base64"))
    } else {
        Ok(content)
    }
}

/// Branch to commit to; creates it from `start_branch` when missing.
fn target_branch(w: &mut WorldState, r: &Req, project: &str) -> Result<String, Response> {
    let Some(branch) = r.s("branch") else {
        return Err(missing("branch"));
    };
    let st = store(w);
    if branch_doc(st, project, &branch).is_some() {
        return Ok(branch);
    }
    let start = r.s("start_branch").or_else(|| r.s("start_sha"));
    match start {
        Some(s) => {
            let Some(snap) = resolve_ref(st, project, &s) else {
                return Err(bad_request(
                    "You can only create or edit files when you are on a branch",
                ));
            };
            let st = svc_mut(w);
            st.collection_mut("branches").insert(
                format!("{project}:{branch}"),
                json!({"name": branch, "commit": snap.sha, "base": snap.branch}),
            );
            Ok(branch)
        }
        None => {
            // Empty repository: the first commit creates the default branch.
            let has_any = all(st, "branches")
                .iter()
                .any(|(k, _)| k.starts_with(&format!("{project}:")));
            if has_any {
                Err(error(
                    StatusCode::BAD_REQUEST,
                    "You can only create or edit files when you are on a branch",
                ))
            } else {
                Ok(branch)
            }
        }
    }
}

fn create_file(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let content = match content_from_body(r) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(message) = r.s("commit_message") else {
        return missing("commit_message");
    };
    let branch = match target_branch(w, r, &project) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let path = r.p("file_path").to_owned();
    if file_at(store(w), &project, &branch, &path).is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "A file with this name already exists",
        );
    }
    let now = now(w);
    let st = svc_mut(w);
    let username = current_username(st);
    write_file(st, &project, &branch, &path, Some(&content));
    let diffs = vec![diff_entry(&path, None, Some(&content))];
    add_commit(
        st,
        &project,
        &branch,
        &message,
        &username,
        &now,
        diffs,
        vec![],
    );
    created(json!({"file_path": path, "branch": branch}))
}

fn update_file(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let content = match content_from_body(r) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(message) = r.s("commit_message") else {
        return missing("commit_message");
    };
    let branch = match target_branch(w, r, &project) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let path = r.p("file_path").to_owned();
    let st = store(w);
    let previous = r.s("previous_path");
    let old_path = previous.clone().unwrap_or_else(|| path.clone());
    let Some(old) = file_at(st, &project, &branch, &old_path) else {
        return error(
            StatusCode::BAD_REQUEST,
            "A file with this name doesn't exist",
        );
    };
    if let Some(last) = r.s("last_commit_id") {
        let snap = resolve_ref(st, &project, &branch).expect("branch");
        if last != last_commit_for(st, &project, &snap, &old_path) && last != snap.sha {
            return error(
                StatusCode::BAD_REQUEST,
                "You are attempting to update a file that has changed since you started editing it.",
            );
        }
    }
    let now = now(w);
    let st = svc_mut(w);
    let username = current_username(st);
    let mut diffs = Vec::new();
    if previous.is_some() && old_path != path {
        write_file(st, &project, &branch, &old_path, None);
        let mut d = diff_entry(&path, Some(&old), Some(&content));
        d["old_path"] = json!(old_path);
        d["renamed_file"] = json!(true);
        diffs.push(d);
    } else {
        diffs.push(diff_entry(&path, Some(&old), Some(&content)));
    }
    write_file(st, &project, &branch, &path, Some(&content));
    add_commit(
        st,
        &project,
        &branch,
        &message,
        &username,
        &now,
        diffs,
        vec![],
    );
    ok(json!({"file_path": path, "branch": branch}))
}

fn delete_file(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(message) = r.s("commit_message") else {
        return missing("commit_message");
    };
    let branch = match target_branch(w, r, &project) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let path = r.p("file_path").to_owned();
    let Some(old) = file_at(store(w), &project, &branch, &path) else {
        return error(
            StatusCode::BAD_REQUEST,
            "A file with this name doesn't exist",
        );
    };
    let now = now(w);
    let st = svc_mut(w);
    let username = current_username(st);
    write_file(st, &project, &branch, &path, None);
    add_commit(
        st,
        &project,
        &branch,
        &message,
        &username,
        &now,
        vec![diff_entry(&path, Some(&old), None)],
        vec![],
    );
    no_content()
}

// ---------------------------------------------------------------------------
// Tree

fn tree(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let reference = r
        .q("ref")
        .map(str::to_owned)
        .unwrap_or_else(|| default_branch(st, &project));
    let Some(snap) = resolve_ref(st, &project, &reference) else {
        return not_found("Tree");
    };
    let files = files_at(st, &project, &snap.branch);
    let dir = r.q("path").unwrap_or("").trim_matches('/').to_owned();
    let recursive = r.q("recursive") == Some("true");
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut entries: BTreeMap<(u8, String), Value> = BTreeMap::new();
    let mut found = dir.is_empty();
    for (path, content) in &files {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        found = true;
        let parts: Vec<&str> = rest.split('/').collect();
        if recursive {
            let mut acc = prefix.clone();
            for (i, part) in parts.iter().enumerate() {
                let full = format!("{acc}{part}");
                if i + 1 < parts.len() {
                    entries
                        .entry((0, full.clone()))
                        .or_insert_with(|| tree_item(&full, "tree", ""));
                    acc = format!("{full}/");
                } else {
                    entries.insert((1, full.clone()), tree_item(&full, "blob", content));
                }
            }
        } else if parts.len() == 1 {
            let full = format!("{prefix}{}", parts[0]);
            entries.insert((1, full.clone()), tree_item(&full, "blob", content));
        } else {
            let full = format!("{prefix}{}", parts[0]);
            entries
                .entry((0, full.clone()))
                .or_insert_with(|| tree_item(&full, "tree", ""));
        }
    }
    if !found {
        return not_found("Tree");
    }
    // GitLab lists trees first, then blobs, each sorted by name (recursive: by path).
    let mut items: Vec<Value> = entries.into_values().collect();
    if recursive {
        items.sort_by(|a, b| str_of(a, "path").cmp(str_of(b, "path")));
    }
    if r.q("pagination") == Some("keyset") {
        let per_page = r
            .q("per_page")
            .and_then(|p| p.parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 100);
        let start = r
            .q("page_token")
            .and_then(|t| {
                items
                    .iter()
                    .position(|i| str_of(i, "id") == t)
                    .map(|p| p + 1)
            })
            .unwrap_or(0);
        let page: Vec<Value> = items.iter().skip(start).take(per_page).cloned().collect();
        let mut resp = ok(Value::Array(page.clone()));
        if start + per_page < items.len()
            && let Some(last) = page.last()
        {
            resp.headers_mut().insert(
                "x-next-page-token",
                axum::http::HeaderValue::from_str(str_of(last, "id")).expect("ascii"),
            );
        }
        return resp;
    }
    paged(r, items)
}

fn tree_item(path: &str, ty: &str, content: &str) -> Value {
    json!({
        "id": fake_sha(&format!("{ty}:{path}:{content}")),
        "name": last_segment(path),
        "type": ty,
        "path": path,
        "mode": if ty == "tree" { "040000" } else { "100644" },
    })
}

fn archive(_w: &mut WorldState, _r: &Req) -> Response {
    error(StatusCode::NOT_ACCEPTABLE, "406 Not Acceptable")
}

fn contributors(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut by: BTreeMap<(String, String), u64> = BTreeMap::new();
    for (k, c) in all(st, "commits") {
        if k.starts_with(&format!("{project}@")) {
            *by.entry((
                str_of(c, "author_name").to_owned(),
                str_of(c, "author_email").to_owned(),
            ))
            .or_default() += 1;
        }
    }
    let items = by
        .into_iter()
        .map(|((name, email), n)| json!({"name": name, "email": email, "commits": n, "additions": 0, "deletions": 0}))
        .collect();
    paged(r, items)
}

// ---------------------------------------------------------------------------
// Branches

fn protection(st: &Store, project: &str, branch: &str) -> Option<Value> {
    let prefix = format!("{project}:");
    all(st, "protected_branches")
        .into_iter()
        .filter_map(|(k, p)| k.strip_prefix(&prefix).map(|n| (n.to_owned(), p)))
        .find(|(name, _)| glob_match(name, branch))
        .map(|(_, p)| p.clone())
}

fn glob_match(pattern: &str, s: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == s,
        Some((pre, post)) => {
            s.len() >= pre.len() + post.len() && s.starts_with(pre) && s.ends_with(post)
        }
    }
}

pub(crate) fn render_branch(st: &Store, base: &str, project: &str, name: &str) -> Value {
    let b = branch_doc(st, project, name).cloned().unwrap_or(json!({}));
    let sha = str_of(&b, "commit").to_owned();
    let default = default_branch(st, project) == name;
    let prot = protection(st, project, name);
    let push = prot
        .as_ref()
        .map(|p| as_u64(&p["push_access_level"]).unwrap_or(40));
    let merge = prot
        .as_ref()
        .map(|p| as_u64(&p["merge_access_level"]).unwrap_or(40));
    let merged = !default
        && reachable(
            st,
            project,
            &branch_head(st, project, &default_branch(st, project)).unwrap_or_default(),
        )
        .contains(&sha);
    json!({
        "name": name,
        "commit": render_commit(st, base, project, &sha),
        "merged": b.get("merged").and_then(Value::as_bool).unwrap_or(merged),
        "protected": prot.is_some(),
        "developers_can_push": push.is_none_or(|l| l <= 30),
        "developers_can_merge": merge.is_none_or(|l| l <= 30),
        "can_push": true,
        "default": default,
        "web_url": format!("{base}/{project}/-/tree/{name}"),
    })
}

fn list_branches(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let prefix = format!("{project}:");
    let search = r.q("search");
    let mut names: Vec<String> = all(st, "branches")
        .into_iter()
        .filter_map(|(k, _)| k.strip_prefix(&prefix).map(str::to_owned))
        .filter(|n| {
            search.is_none_or(|s| match (s.strip_prefix('^'), s.strip_suffix('$')) {
                (Some(p), _) => n.starts_with(p),
                (_, Some(p)) => n.ends_with(p),
                _ => n.to_lowercase().contains(&s.to_lowercase()),
            })
        })
        .collect();
    names.sort();
    let items = names
        .iter()
        .map(|n| render_branch(st, &r.base, &project, n))
        .collect();
    paged(r, items)
}

fn get_branch(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("branch");
    if branch_doc(st, &project, name).is_none() {
        return not_found("Branch");
    }
    ok(render_branch(st, &r.base, &project, name))
}

fn valid_branch_name(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with('-')
        && !n.ends_with('/')
        && !n.ends_with(".lock")
        && !n.contains("..")
        && !n.contains("//")
        && !n
            .chars()
            .any(|c| c.is_whitespace() || "~^:?*[\\".contains(c))
}

fn create_branch(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("branch") else {
        return missing("branch");
    };
    let Some(reference) = r.s("ref") else {
        return missing("ref");
    };
    if !valid_branch_name(&name) {
        return error(StatusCode::BAD_REQUEST, "Branch name is invalid");
    }
    if branch_doc(st, &project, &name).is_some() {
        return error(StatusCode::BAD_REQUEST, "Branch already exists");
    }
    let Some(snap) = resolve_ref(st, &project, &reference) else {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Invalid reference name: {reference}"),
        );
    };
    let st = svc_mut(w);
    st.collection_mut("branches").insert(
        format!("{project}:{name}"),
        json!({"name": name, "commit": snap.sha, "base": snap.branch}),
    );
    created(render_branch(st, &r.base, &project, &name))
}

fn delete_branch(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("branch").to_owned();
    if branch_doc(st, &project, &name).is_none() {
        return not_found("Branch");
    }
    if default_branch(st, &project) == name {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "The default branch of a project cannot be deleted.",
        );
    }
    if protection(st, &project, &name).is_some() {
        return forbidden();
    }
    remove_branch(svc_mut(w), &project, &name);
    no_content()
}

fn delete_merged(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let default = default_branch(st, &project);
    let head = branch_head(st, &project, &default).unwrap_or_default();
    let reach = reachable(st, &project, &head);
    let prefix = format!("{project}:");
    let doomed: Vec<String> = all(st, "branches")
        .into_iter()
        .filter_map(|(k, b)| {
            k.strip_prefix(&prefix)
                .map(|n| (n.to_owned(), str_of(b, "commit").to_owned()))
        })
        .filter(|(n, sha)| {
            *n != default && reach.contains(sha) && protection(st, &project, n).is_none()
        })
        .map(|(n, _)| n)
        .collect();
    let st = svc_mut(w);
    for n in doomed {
        remove_branch(st, &project, &n);
    }
    accepted(json!({"message": "202 Accepted"}))
}

// ---------------------------------------------------------------------------
// Commits

fn list_commits(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let shas: Vec<String> = if r.q("all") == Some("true") {
        all(st, "commits")
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{project}@")))
            .map(|(_, c)| str_of(c, "id").to_owned())
            .collect()
    } else {
        let reference = r
            .q("ref_name")
            .map(str::to_owned)
            .unwrap_or_else(|| default_branch(st, &project));
        if let Some((from, to)) = reference
            .split_once("...")
            .or_else(|| reference.split_once(".."))
        {
            match compare_refs(st, &project, from, to) {
                Some((c, _)) => c.iter().map(|c| str_of(c, "id").to_owned()).collect(),
                None => return not_found("Commit"),
            }
        } else {
            match resolve_ref(st, &project, &reference) {
                Some(s) => reachable(st, &project, &s.sha),
                None => return ok(json!([])),
            }
        }
    };
    let mut commits: Vec<Value> = shas
        .iter()
        .filter_map(|s| commit_doc(st, &project, s).cloned())
        .filter(|c| {
            let date = str_of(c, "committed_date");
            r.q("since").is_none_or(|s| date >= s)
                && r.q("until").is_none_or(|u| date <= u)
                && r.q("author").is_none_or(|a| {
                    let a = a.to_lowercase();
                    str_of(c, "author_name").to_lowercase().contains(&a)
                        || str_of(c, "author_email").to_lowercase().contains(&a)
                })
                && r.q("path").is_none_or(|p| {
                    c.get("diffs").and_then(Value::as_array).is_some_and(|ds| {
                        ds.iter().any(|d| {
                            str_of(d, "new_path").starts_with(p)
                                || str_of(d, "old_path").starts_with(p)
                        })
                    })
                })
        })
        .collect();
    commits.sort_by(|a, b| str_of(b, "committed_date").cmp(str_of(a, "committed_date")));
    let stats = r.q("with_stats") == Some("true");
    let items = commits
        .iter()
        .map(|c| {
            let mut out = render_commit(st, &r.base, &project, str_of(c, "id"));
            if stats {
                out["stats"] = commit_stats(st, &project, str_of(c, "id"));
            }
            out
        })
        .collect();
    paged(r, items)
}

fn get_commit(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(snap) = resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let mut out = render_commit(st, &r.base, &project, &snap.sha);
    if r.q("stats") != Some("false") {
        out["stats"] = commit_stats(st, &project, &snap.sha);
    }
    out["status"] = Value::Null;
    out["project_id"] = json!(project_id(st, &project));
    out["last_pipeline"] = crate::ci::latest_pipeline_for_sha(st, &project, &snap.sha)
        .map(|p| crate::ci::render_pipeline_basic(st, &r.base, &p))
        .unwrap_or(Value::Null);
    ok(out)
}

fn commit_diff(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(snap) = resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let items = commit_diffs(st, &project, &snap.sha);
    paged(r, items)
}

fn commit_refs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(snap) = resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let ty = r.q("type").unwrap_or("all");
    let mut items = Vec::new();
    if ty != "tag" {
        for (k, b) in all(st, "branches") {
            if let Some(name) = k.strip_prefix(&format!("{project}:"))
                && reachable(st, &project, str_of(b, "commit")).contains(&snap.sha)
            {
                items.push(json!({"type": "branch", "name": name}));
            }
        }
    }
    if ty != "branch" {
        for (k, t) in all(st, "tags") {
            if let Some(name) = k.strip_prefix(&format!("{project}:"))
                && reachable(st, &project, str_of(t, "commit")).contains(&snap.sha)
            {
                items.push(json!({"type": "tag", "name": name}));
            }
        }
    }
    paged(r, items)
}

fn commit_mrs(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(snap) = resolve_ref(st, &project, r.p("sha")) else {
        return not_found("Commit");
    };
    let items = all(st, "merge_requests")
        .into_iter()
        .filter(|(k, m)| {
            project_of(k) == project
                && (str_of(m, "sha") == snap.sha
                    || str_of(m, "merge_commit_sha") == snap.sha
                    || crate::merge_requests::mr_commit_shas(st, k).contains(&snap.sha))
        })
        .map(|(k, _)| crate::merge_requests::render(st, &r.base, k))
        .collect();
    paged(r, items)
}

fn create_commit(w: &mut WorldState, r: &Req) -> Response {
    let project = match require_project(store(w), r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(message) = r.s("commit_message") else {
        return missing("commit_message");
    };
    let Some(actions) = r.body.get("actions").and_then(Value::as_array).cloned() else {
        return missing("actions");
    };
    if actions.is_empty() {
        return bad_request("actions is empty");
    }
    let branch = match target_branch(w, r, &project) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let st = store(w);
    let mut planned: Vec<(String, Option<String>, Option<String>)> = Vec::new(); // (path, new content, old path)
    let mut current = files_at(st, &project, &branch);
    let mut diffs = Vec::new();
    for a in &actions {
        let action = a.get("action").and_then(Value::as_str).unwrap_or("create");
        let Some(path) = a
            .get("file_path")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return missing("actions[file_path]");
        };
        let raw = a.get("content").and_then(Value::as_str).map(str::to_owned);
        let content = if a.get("encoding").and_then(Value::as_str) == Some("base64") {
            raw.and_then(|c| b64_decode(&c))
                .and_then(|b| String::from_utf8(b).ok())
        } else {
            raw
        };
        let old = current.get(&path).cloned();
        match action {
            "create" => {
                if old.is_some() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "A file with this name already exists",
                    );
                }
                let c = content.unwrap_or_default();
                diffs.push(diff_entry(&path, None, Some(&c)));
                current.insert(path.clone(), c.clone());
                planned.push((path, Some(c), None));
            }
            "update" => {
                let Some(o) = old else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "A file with this name doesn't exist",
                    );
                };
                let c = content.unwrap_or_default();
                diffs.push(diff_entry(&path, Some(&o), Some(&c)));
                current.insert(path.clone(), c.clone());
                planned.push((path, Some(c), None));
            }
            "delete" => {
                let Some(o) = old else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "A file with this name doesn't exist",
                    );
                };
                diffs.push(diff_entry(&path, Some(&o), None));
                current.remove(&path);
                planned.push((path, None, None));
            }
            "move" => {
                let Some(prev) = a
                    .get("previous_path")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    return missing("actions[previous_path]");
                };
                let Some(o) = current.get(&prev).cloned() else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "A file with this name doesn't exist",
                    );
                };
                let c = content.unwrap_or_else(|| o.clone());
                let mut d = diff_entry(&path, Some(&o), Some(&c));
                d["old_path"] = json!(prev);
                d["renamed_file"] = json!(true);
                diffs.push(d);
                current.remove(&prev);
                current.insert(path.clone(), c.clone());
                planned.push((path, Some(c), Some(prev)));
            }
            "chmod" => {}
            other => {
                return bad_request(&format!(
                    "actions[action] does not have a valid value: {other}"
                ));
            }
        }
    }
    let now = now(w);
    let st = svc_mut(w);
    let username = current_username(st);
    for (path, content, prev) in planned {
        if let Some(p) = prev {
            write_file(st, &project, &branch, &p, None);
        }
        write_file(st, &project, &branch, &path, content.as_deref());
    }
    let sha = add_commit(
        st,
        &project,
        &branch,
        &message,
        &username,
        &now,
        diffs,
        vec![],
    );
    let mut out = render_commit(st, &r.base, &project, &sha);
    out["stats"] = commit_stats(st, &project, &sha);
    out["status"] = Value::Null;
    out["project_id"] = json!(project_id(st, &project));
    out["last_pipeline"] = Value::Null;
    created(out)
}

fn compare(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(from) = r.q("from") else {
        return missing("from");
    };
    let Some(to) = r.q("to") else {
        return missing("to");
    };
    let Some((commits, diffs)) = compare_refs(st, &project, from, to) else {
        return not_found("Ref");
    };
    let same = resolve_ref(st, &project, from).map(|s| s.sha)
        == resolve_ref(st, &project, to).map(|s| s.sha);
    let rendered: Vec<Value> = commits
        .iter()
        .rev()
        .map(|c| render_commit(st, &r.base, &project, str_of(c, "id")))
        .collect();
    let head = rendered.last().cloned().unwrap_or(Value::Null);
    ok(json!({
        "commit": head,
        "commits": rendered,
        "diffs": diffs,
        "compare_timeout": false,
        "compare_same_ref": same,
        "web_url": format!("{}/{project}/-/compare/{from}...{to}", r.base),
    }))
}

// ---------------------------------------------------------------------------
// Protected branches

fn access_levels(st: &Store, p: &Value, field: &str, id_base: u64) -> Value {
    let custom = p
        .get(format!("{field}s"))
        .and_then(Value::as_array)
        .cloned();
    if let Some(list) = custom {
        return Value::Array(
            list.iter()
                .enumerate()
                .map(|(i, e)| {
                    let level = as_u64(&e["access_level"]);
                    let user_id = e.get("user").and_then(Value::as_str).and_then(|u| user(st, u)).and_then(|u| as_u64(&u["id"]));
                    json!({
                        "id": id_base + i as u64,
                        "access_level": level.unwrap_or(40),
                        "access_level_description": match (user_id, e.get("user").and_then(Value::as_str)) {
                            (Some(_), Some(u)) => user(st, u).map(|d| str_of(d, "name").to_owned()).unwrap_or_default(),
                            _ => access_level_description(level.unwrap_or(40)).to_owned() + "s",
                        },
                        "deploy_key_id": null,
                        "user_id": user_id,
                        "group_id": null,
                    })
                })
                .collect(),
        );
    }
    let level = as_u64(&p[field]).unwrap_or(40);
    let desc = match level {
        0 => "No one".to_owned(),
        30 => "Developers + Maintainers".to_owned(),
        40 => "Maintainers".to_owned(),
        60 => "Admins".to_owned(),
        l => access_level_description(l).to_owned(),
    };
    json!([{
        "id": id_base,
        "access_level": level,
        "access_level_description": desc,
        "deploy_key_id": null,
        "user_id": null,
        "group_id": null,
    }])
}

fn render_protected(st: &Store, p: &Value, name: &str) -> Value {
    let id = as_u64(&p["id"]).unwrap_or(0);
    json!({
        "id": id,
        "name": name,
        "push_access_levels": access_levels(st, p, "push_access_level", id * 10 + 1),
        "merge_access_levels": access_levels(st, p, "merge_access_level", id * 10 + 2),
        "unprotect_access_levels": access_levels(st, p, "unprotect_access_level", id * 10 + 3),
        "allow_force_push": p.get("allow_force_push").cloned().unwrap_or(json!(false)),
        "code_owner_approval_required": p.get("code_owner_approval_required").cloned().unwrap_or(json!(false)),
        "inherited": false,
    })
}

fn list_protected(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let prefix = format!("{project}:");
    let items = all(st, "protected_branches")
        .into_iter()
        .filter_map(|(k, p)| k.strip_prefix(&prefix).map(|n| (n.to_owned(), p)))
        .filter(|(n, _)| r.q("search").is_none_or(|s| n.contains(s)))
        .map(|(n, p)| render_protected(st, p, &n))
        .collect();
    paged(r, items)
}

fn get_protected(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("branch");
    match get(st, "protected_branches", &format!("{project}:{name}")) {
        Some(p) => ok(render_protected(st, p, name)),
        None => not_found("Protected Branch"),
    }
}

fn protect(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let Some(name) = r.s("name") else {
        return missing("name");
    };
    let key = format!("{project}:{name}");
    if get(st, "protected_branches", &key).is_some() {
        return error(
            StatusCode::CONFLICT,
            "Protected branch 'name' has already been taken",
        );
    }
    let st = svc_mut(w);
    let id = next_id(st, "protected_branches", 1);
    let doc = json!({
        "id": id,
        "name": name,
        "push_access_level": r.int("push_access_level").unwrap_or(40),
        "merge_access_level": r.int("merge_access_level").unwrap_or(40),
        "unprotect_access_level": r.int("unprotect_access_level").unwrap_or(40),
        "allow_force_push": r.bool("allow_force_push").unwrap_or(false),
        "code_owner_approval_required": r.bool("code_owner_approval_required").unwrap_or(false),
    });
    st.collection_mut("protected_branches")
        .insert(key, doc.clone());
    created(render_protected(st, &doc, &name))
}

fn update_protected(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let name = r.p("branch").to_owned();
    let key = format!("{project}:{name}");
    if get(st, "protected_branches", &key).is_none() {
        return not_found("Protected Branch");
    }
    let st = svc_mut(w);
    let doc = st
        .collection_mut("protected_branches")
        .get_mut(&key)
        .expect("protected");
    for f in ["allow_force_push", "code_owner_approval_required"] {
        if let Some(b) = r.bool(f) {
            doc[f] = json!(b);
        }
    }
    for (arg, field) in [
        ("allowed_to_push", "push_access_level"),
        ("allowed_to_merge", "merge_access_level"),
        ("allowed_to_unprotect", "unprotect_access_level"),
    ] {
        if let Some(list) = r.body.get(arg).and_then(Value::as_array) {
            let kept: Vec<Value> = list
                .iter()
                .filter(|e| !e.get("_destroy").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|e| {
                    if let Some(l) = e.get("access_level") {
                        Some(json!({"access_level": l}))
                    } else {
                        e.get("user_id")
                            .map(|u| json!({"access_level": 40, "user_id": u}))
                    }
                })
                .collect();
            if let Some(first) = kept.first().and_then(|e| as_u64(&e["access_level"])) {
                doc[field] = json!(first);
            }
        }
    }
    let doc = doc.clone();
    ok(render_protected(st, &doc, &name))
}

fn unprotect(w: &mut WorldState, r: &Req) -> Response {
    let st = store(w);
    let project = match require_project(st, r) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let key = format!("{project}:{}", r.p("branch"));
    if get(st, "protected_branches", &key).is_none() {
        return not_found("Protected Branch");
    }
    remove_doc(svc_mut(w), "protected_branches", &key);
    no_content()
}

// ---------------------------------------------------------------------------
// Search helpers (used by `/search?scope=blobs|commits`)

fn wildcard(pattern: &str, s: &str) -> bool {
    if !pattern.contains('*') {
        return s == pattern || s.ends_with(&format!("/{pattern}"));
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match s[pos..].find(part) {
            Some(idx) => {
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }
    parts.last().is_none_or(|l| l.is_empty() || s.ends_with(l))
}

pub(crate) fn search_blobs(st: &Store, r: &Req, projects: &[String], term: &str) -> Vec<Value> {
    // Inline filters (`foo filename:*.rs`, `path:src/`, `extension:py`) as GitLab basic search.
    let mut needle_parts = Vec::new();
    let mut filename = r.q("filename").map(str::to_owned);
    let mut path_filter = r.q("path").map(str::to_owned);
    let mut extension = r.q("extension").map(str::to_owned);
    for tok in term.split_whitespace() {
        if let Some(v) = tok.strip_prefix("filename:") {
            filename = Some(v.to_owned());
        } else if let Some(v) = tok.strip_prefix("path:") {
            path_filter = Some(v.to_owned());
        } else if let Some(v) = tok.strip_prefix("extension:") {
            extension = Some(v.to_owned());
        } else {
            needle_parts.push(tok);
        }
    }
    let needle = needle_parts.join(" ").to_lowercase();
    let mut out = Vec::new();
    for project in projects {
        let reference = r
            .q("ref")
            .map(str::to_owned)
            .unwrap_or_else(|| default_branch(st, project));
        let Some(snap) = resolve_ref(st, project, &reference) else {
            continue;
        };
        for (path, content) in files_at(st, project, &snap.branch) {
            let name = last_segment(&path).to_owned();
            if filename
                .as_ref()
                .is_some_and(|f| !wildcard(f, &name) && !wildcard(f, &path))
            {
                continue;
            }
            if path_filter
                .as_ref()
                .is_some_and(|p| !(wildcard(p, &path) || path.starts_with(p.trim_end_matches('*'))))
            {
                continue;
            }
            if extension
                .as_ref()
                .is_some_and(|e| !name.ends_with(&format!(".{e}")))
            {
                continue;
            }
            let lines: Vec<&str> = content.lines().collect();
            let hit = if needle.is_empty() {
                Some(0)
            } else {
                lines
                    .iter()
                    .position(|l| l.to_lowercase().contains(&needle))
            };
            let Some(idx) = hit else {
                if !needle.is_empty() && path.to_lowercase().contains(&needle) {
                    out.push(blob_result(
                        st, project, &path, &snap, &lines, 0, &reference,
                    ));
                }
                continue;
            };
            out.push(blob_result(
                st, project, &path, &snap, &lines, idx, &reference,
            ));
        }
    }
    out
}

fn blob_result(
    st: &Store,
    project: &str,
    path: &str,
    snap: &Snapshot,
    lines: &[&str],
    idx: usize,
    reference: &str,
) -> Value {
    let start = idx.saturating_sub(1);
    let end = (idx + 2).min(lines.len());
    let data: String = lines[start..end].iter().map(|l| format!("{l}\n")).collect();
    let _ = snap;
    json!({
        "basename": path.rsplit_once('.').map_or(path, |(b, _)| b),
        "data": data,
        "path": path,
        "filename": path,
        "id": null,
        "ref": reference,
        "startline": start + 1,
        "project_id": project_id(st, project),
    })
}

pub(crate) fn search_commits(st: &Store, r: &Req, projects: &[String], term: &str) -> Vec<Value> {
    let needle = term.to_lowercase();
    let mut out = Vec::new();
    for project in projects {
        for (k, c) in all(st, "commits") {
            if k.starts_with(&format!("{project}@"))
                && (str_of(c, "message").to_lowercase().contains(&needle)
                    || str_of(c, "title").to_lowercase().contains(&needle)
                    || str_of(c, "id").starts_with(&needle))
            {
                let mut v = render_commit(st, &r.base, project, str_of(c, "id"));
                v["project_id"] = json!(project_id(st, project));
                out.push(v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_single_change() {
        let d = unified_diff("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(d, "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
    }

    #[test]
    fn unified_diff_new_file() {
        assert_eq!(unified_diff("", "x\ny\n"), "@@ -0,0 +1,2 @@\n+x\n+y\n");
    }

    #[test]
    fn globbing() {
        assert!(glob_match("release/*", "release/1.0"));
        assert!(!glob_match("release/*", "main"));
        assert!(wildcard("*.rs", "src/main.rs"));
        assert!(wildcard("main.rs", "src/main.rs"));
    }
}
