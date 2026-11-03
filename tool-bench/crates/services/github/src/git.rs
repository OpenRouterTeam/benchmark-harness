//! A small Git object model over the store.
//!
//! * `commits`  key `owner/repo@sha`: `{sha, message, author{name,email,date}, committer{..},
//!   author_login, parents: [sha], files: [{filename, status, content, previous_filename, mode}]}`.
//!   `files` is the change set against the first parent (full new content for added/modified
//!   files), so a commit's tree is the parent's tree with the changes applied.
//! * `branches` key `owner/repo:name`: `{name, sha, protected}`.
//! * `tags`     key `owner/repo:name`: `{name, sha, message, tagger}` (annotated tags).
//! * `trees`    key `owner/repo@sha`: snapshots created through `POST git/trees`.
//! * `blobs`    key `owner/repo@sha`: blobs created through `POST git/blobs`.
//!
//! Blob and tree IDs are real Git SHA-1s, so the server's blob verification succeeds.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    sha1::{blob_sha, hex, sha1, sha1_hex},
    util::{arr, b64_decode, s, unix},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub content: Vec<u8>,
    pub mode: String,
}

pub type FileMap = BTreeMap<String, FileEntry>;

#[derive(Debug, Clone)]
pub struct FileChange {
    pub filename: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub patch: String,
    pub previous_filename: Option<String>,
    pub sha: String,
}

pub fn commit_key(repo: &str, sha: &str) -> String {
    format!("{repo}@{sha}")
}

pub fn commit<'a>(svc: &'a ServiceState, repo: &str, sha: &str) -> Option<&'a Value> {
    let c = svc.store.collection("commits")?;
    if let Some(doc) = c.get(&commit_key(repo, sha)) {
        return Some(doc);
    }
    if sha.len() >= 7 && sha.chars().all(|ch| ch.is_ascii_hexdigit()) {
        let prefix = commit_key(repo, sha);
        return c
            .iter()
            .find(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v);
    }
    None
}

pub fn repo_commits<'a>(svc: &'a ServiceState, repo: &str) -> Vec<&'a Value> {
    let prefix = format!("{repo}@");
    svc.store
        .collection("commits")
        .map(|c| {
            c.iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| v)
                .collect()
        })
        .unwrap_or_default()
}

pub fn parents(doc: &Value) -> Vec<String> {
    arr(doc, "parents")
        .iter()
        .filter_map(|p| {
            p.as_str()
                .map(str::to_owned)
                .or_else(|| p.get("sha").and_then(Value::as_str).map(str::to_owned))
        })
        .collect()
}

pub fn file_content(f: &Value) -> Vec<u8> {
    if let Some(c) = f.get("content").and_then(Value::as_str) {
        return c.as_bytes().to_vec();
    }
    if let Some(c) = f.get("content_base64").and_then(Value::as_str) {
        return b64_decode(c).unwrap_or_default();
    }
    Vec::new()
}

/// The full tree of a commit.
pub fn file_map(svc: &ServiceState, repo: &str, sha: &str) -> Option<FileMap> {
    // Walk to the root first (iteratively), then replay.
    let mut chain = Vec::new();
    let mut cur = commit(svc, repo, sha)?;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(s(cur, "sha").to_owned()) {
            break;
        }
        chain.push(cur);
        match parents(cur).first() {
            Some(p) => match commit(svc, repo, p) {
                Some(next) => cur = next,
                None => break,
            },
            None => break,
        }
    }
    let mut map = FileMap::new();
    for doc in chain.iter().rev() {
        apply_changes(&mut map, doc);
    }
    Some(map)
}

fn apply_changes(map: &mut FileMap, doc: &Value) {
    for f in arr(doc, "files") {
        let name = s(f, "filename").to_owned();
        match s(f, "status") {
            "removed" | "deleted" => {
                map.remove(&name);
            }
            status => {
                if status == "renamed"
                    && let Some(prev) = f.get("previous_filename").and_then(Value::as_str)
                {
                    let old = map.remove(prev);
                    if f.get("content").is_none() && f.get("content_base64").is_none() {
                        if let Some(old) = old {
                            map.insert(name, old);
                        }
                        continue;
                    }
                }
                let mode = f
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("100644")
                    .to_owned();
                map.insert(
                    name,
                    FileEntry {
                        content: file_content(f),
                        mode,
                    },
                );
            }
        }
    }
}

/// Resolves a branch, tag, `refs/...`, `heads/...`, `HEAD`, or (short) SHA to a commit SHA.
pub fn resolve_ref(svc: &ServiceState, repo: &str, r: &str) -> Option<String> {
    let r = r.trim();
    let default = || {
        svc.store
            .collection("repos")
            .and_then(|c| c.get(repo))
            .map(|d| s(d, "default_branch").to_owned())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "main".to_owned())
    };
    let name = if r.is_empty() || r == "HEAD" {
        default()
    } else {
        r.to_owned()
    };
    let stripped = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("heads/"));
    if let Some(b) = stripped {
        return branch_sha(svc, repo, b);
    }
    let tag = name
        .strip_prefix("refs/tags/")
        .or_else(|| name.strip_prefix("tags/"));
    if let Some(t) = tag {
        return tag_commit(svc, repo, t);
    }
    if let Some(sha) = branch_sha(svc, repo, &name) {
        return Some(sha);
    }
    if let Some(sha) = tag_commit(svc, repo, &name) {
        return Some(sha);
    }
    if let Some(c) = commit(svc, repo, &name) {
        return Some(s(c, "sha").to_owned());
    }
    // An annotated tag object peels to its commit.
    svc.store.collection("tags").and_then(|c| {
        c.iter()
            .filter(|(k, _)| k.starts_with(&format!("{repo}:")))
            .find(|(_, t)| tag_object_sha(repo, t) == name)
            .map(|(_, t)| s(t, "sha").to_owned())
    })
}

pub fn branch_sha(svc: &ServiceState, repo: &str, branch: &str) -> Option<String> {
    svc.store
        .collection("branches")
        .and_then(|c| c.get(&format!("{repo}:{branch}")))
        .map(|d| s(d, "sha").to_owned())
}

pub fn tag_commit(svc: &ServiceState, repo: &str, tag: &str) -> Option<String> {
    svc.store
        .collection("tags")
        .and_then(|c| c.get(&format!("{repo}:{tag}")))
        .map(|d| s(d, "sha").to_owned())
}

/// Deterministic annotated-tag object ID.
pub fn tag_object_sha(repo: &str, tag: &Value) -> String {
    if let Some(sha) = tag.get("tag_sha").and_then(Value::as_str) {
        return sha.to_owned();
    }
    sha1_hex(
        format!(
            "tag {repo} {} {} {}",
            s(tag, "name"),
            s(tag, "sha"),
            s(tag, "message")
        )
        .as_bytes(),
    )
}

// ---------------------------------------------------------------- trees

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: String,
    pub mode: String,
    pub kind: &'static str,
    pub sha: String,
    pub size: Option<usize>,
}

/// Direct children of `dir` ("" for the root), sorted like Git.
pub fn tree_entries(map: &FileMap, dir: &str) -> Vec<TreeEntry> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut files = Vec::new();
    let mut dirs = BTreeSet::new();
    for (path, entry) in map {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((d, _)) => {
                dirs.insert(d.to_owned());
            }
            None => files.push(TreeEntry {
                path: rest.to_owned(),
                mode: entry.mode.clone(),
                kind: "blob",
                sha: blob_sha(&entry.content),
                size: Some(entry.content.len()),
            }),
        }
    }
    for d in dirs {
        let full = format!("{prefix}{d}");
        files.push(TreeEntry {
            path: d,
            mode: "040000".into(),
            kind: "tree",
            sha: tree_sha(map, &full),
            size: None,
        });
    }
    files.sort_by_key(git_sort_key);
    files
}

fn git_sort_key(e: &TreeEntry) -> String {
    if e.kind == "tree" {
        format!("{}/", e.path)
    } else {
        e.path.clone()
    }
}

pub fn tree_sha(map: &FileMap, dir: &str) -> String {
    let mut body = Vec::new();
    for e in tree_entries(map, dir) {
        let mode = if e.kind == "tree" { "40000" } else { &e.mode };
        body.extend_from_slice(format!("{mode} {}\0", e.path).as_bytes());
        let raw: Vec<u8> = (0..e.sha.len() / 2)
            .map(|i| u8::from_str_radix(&e.sha[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect();
        body.extend_from_slice(&raw);
    }
    let mut data = format!("tree {}\0", body.len()).into_bytes();
    data.extend_from_slice(&body);
    hex(&sha1(&data))
}

/// All directories present in `map` (including the root "").
pub fn dirs(map: &FileMap) -> BTreeSet<String> {
    let mut out = BTreeSet::from([String::new()]);
    for path in map.keys() {
        let mut acc = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            out.insert(acc.clone());
        }
    }
    out
}

/// Finds a tree by SHA: a stored snapshot, or a (sub)tree of any commit in the repo.
pub fn find_tree(svc: &ServiceState, repo: &str, sha: &str) -> Option<(FileMap, String)> {
    if let Some(doc) = svc
        .store
        .collection("trees")
        .and_then(|c| c.get(&commit_key(repo, sha)))
    {
        return Some((snapshot_map(doc), String::new()));
    }
    for c in repo_commits(svc, repo) {
        let Some(map) = file_map(svc, repo, s(c, "sha")) else {
            continue;
        };
        for d in dirs(&map) {
            if tree_sha(&map, &d) == sha {
                return Some((map, d));
            }
        }
    }
    None
}

pub fn snapshot_map(doc: &Value) -> FileMap {
    doc.get("files")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(path, f)| {
                    (
                        path.clone(),
                        FileEntry {
                            content: file_content(f),
                            mode: f
                                .get("mode")
                                .and_then(Value::as_str)
                                .unwrap_or("100644")
                                .to_owned(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn snapshot_doc(map: &FileMap) -> Value {
    let files: serde_json::Map<String, Value> =
        map.iter().map(|(p, e)| (p.clone(), file_json(e))).collect();
    json!({ "files": files })
}

fn file_json(e: &FileEntry) -> Value {
    match std::str::from_utf8(&e.content) {
        Ok(text) => json!({"content": text, "mode": e.mode}),
        Err(_) => json!({"content_base64": crate::util::b64_encode(&e.content), "mode": e.mode}),
    }
}

pub fn find_blob(svc: &ServiceState, repo: &str, sha: &str) -> Option<Vec<u8>> {
    if let Some(doc) = svc
        .store
        .collection("blobs")
        .and_then(|c| c.get(&commit_key(repo, sha)))
    {
        return Some(file_content(doc));
    }
    if let Some(trees) = svc.store.collection("trees") {
        let prefix = format!("{repo}@");
        for (k, doc) in trees.iter() {
            if !k.starts_with(&prefix) {
                continue;
            }
            for e in snapshot_map(doc).values() {
                if blob_sha(&e.content) == sha {
                    return Some(e.content.clone());
                }
            }
        }
    }
    for c in repo_commits(svc, repo) {
        for f in arr(c, "files") {
            let content = file_content(f);
            if blob_sha(&content) == sha {
                return Some(content);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- history

/// Commits reachable from `sha`, newest first by committer date (like `git log`).
pub fn log(svc: &ServiceState, repo: &str, sha: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![sha.to_owned()];
    while let Some(cur) = stack.pop() {
        let Some(doc) = commit(svc, repo, &cur) else {
            continue;
        };
        let full = s(doc, "sha").to_owned();
        if !seen.insert(full) {
            continue;
        }
        stack.extend(parents(doc));
        out.push(doc.clone());
    }
    out.sort_by(|a, b| {
        let da = unix(commit_date(a)).unwrap_or(0);
        let db = unix(commit_date(b)).unwrap_or(0);
        db.cmp(&da)
    });
    out
}

pub fn commit_date(doc: &Value) -> &str {
    let c = doc
        .get("committer")
        .and_then(|c| c.get("date"))
        .and_then(Value::as_str);
    c.or_else(|| {
        doc.get("author")
            .and_then(|a| a.get("date"))
            .and_then(Value::as_str)
    })
    .unwrap_or("")
}

pub fn ancestors(svc: &ServiceState, repo: &str, sha: &str) -> HashSet<String> {
    log(svc, repo, sha)
        .iter()
        .map(|c| s(c, "sha").to_owned())
        .collect()
}

/// Commits in `head` but not in `base`, oldest first.
pub fn commits_between(svc: &ServiceState, repo: &str, base: &str, head: &str) -> Vec<Value> {
    let base_set = ancestors(svc, repo, base);
    let mut v: Vec<Value> = log(svc, repo, head)
        .into_iter()
        .filter(|c| !base_set.contains(s(c, "sha")))
        .collect();
    v.reverse();
    v
}

/// Newest common ancestor of `a` and `b`.
pub fn merge_base(svc: &ServiceState, repo: &str, a: &str, b: &str) -> Option<String> {
    let a_set = ancestors(svc, repo, a);
    log(svc, repo, b)
        .iter()
        .map(|c| s(c, "sha").to_owned())
        .find(|sha| a_set.contains(sha))
}

/// File changes a commit introduced relative to its first parent.
pub fn commit_changes(svc: &ServiceState, repo: &str, sha: &str) -> Vec<FileChange> {
    let Some(doc) = commit(svc, repo, sha) else {
        return Vec::new();
    };
    let new = file_map(svc, repo, sha).unwrap_or_default();
    let old = parents(doc)
        .first()
        .and_then(|p| file_map(svc, repo, p))
        .unwrap_or_default();
    let renames: BTreeMap<String, String> = arr(doc, "files")
        .iter()
        .filter(|f| s(f, "status") == "renamed")
        .filter_map(|f| {
            f.get("previous_filename")
                .and_then(Value::as_str)
                .map(|p| (s(f, "filename").to_owned(), p.to_owned()))
        })
        .collect();
    diff_maps(&old, &new, &renames)
}

pub fn diff_maps(
    old: &FileMap,
    new: &FileMap,
    renames: &BTreeMap<String, String>,
) -> Vec<FileChange> {
    let renamed_from: HashSet<&String> = renames.values().collect();
    let mut out = Vec::new();
    let paths: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for path in paths {
        if renamed_from.contains(path) && !new.contains_key(path) {
            continue;
        }
        let before = renames
            .get(path)
            .and_then(|p| old.get(p))
            .or_else(|| old.get(path));
        let after = new.get(path);
        let (status, prev) = match (before, after) {
            (None, Some(_)) => ("added", None),
            (Some(_), None) => ("removed", None),
            (Some(a), Some(b)) => {
                if let Some(p) = renames.get(path) {
                    ("renamed", Some(p.clone()))
                } else if a == b {
                    continue;
                } else {
                    ("modified", None)
                }
            }
            (None, None) => continue,
        };
        let old_text = before.map(|e| e.content.as_slice()).unwrap_or(&[]);
        let new_text = after.map(|e| e.content.as_slice()).unwrap_or(&[]);
        let (patch, additions, deletions) = unified_diff(old_text, new_text);
        out.push(FileChange {
            filename: path.clone(),
            status: status.to_owned(),
            additions,
            deletions,
            patch,
            previous_filename: prev,
            sha: blob_sha(after.map(|e| e.content.as_slice()).unwrap_or(old_text)),
        });
    }
    out
}

/// Unified diff body (GitHub `patch` field, hunks only) plus addition/deletion counts.
pub fn unified_diff(old: &[u8], new: &[u8]) -> (String, usize, usize) {
    let a: Vec<&str> = split_lines(old);
    let b: Vec<&str> = split_lines(new);
    let ops = lcs_ops(&a, &b);
    let adds = ops.iter().filter(|o| matches!(o, Op::Add(_))).count();
    let dels = ops.iter().filter(|o| matches!(o, Op::Del(_))).count();
    if adds == 0 && dels == 0 {
        return (String::new(), 0, 0);
    }

    // Group into hunks with 3 lines of context.
    const CTX: usize = 3;
    let mut hunks: Vec<(usize, usize)> = Vec::new(); // op index ranges
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| !matches!(o, Op::Eq(..)))
        .map(|(i, _)| i)
        .collect();
    for &i in &changed {
        let start = i.saturating_sub(CTX);
        let end = (i + CTX + 1).min(ops.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => hunks.push((start, end)),
        }
    }

    let mut out = String::new();
    for (start, end) in hunks {
        // Line numbers at hunk start.
        let (mut ol, mut nl) = (1usize, 1usize);
        for o in &ops[..start] {
            match o {
                Op::Eq(..) => {
                    ol += 1;
                    nl += 1;
                }
                Op::Del(_) => ol += 1,
                Op::Add(_) => nl += 1,
            }
        }
        let mut body = String::new();
        let (mut oc, mut nc) = (0usize, 0usize);
        for o in &ops[start..end] {
            match o {
                Op::Eq(l, _) => {
                    body.push_str(&format!(" {l}\n"));
                    oc += 1;
                    nc += 1;
                }
                Op::Del(l) => {
                    body.push_str(&format!("-{l}\n"));
                    oc += 1;
                }
                Op::Add(l) => {
                    body.push_str(&format!("+{l}\n"));
                    nc += 1;
                }
            }
        }
        let os = if oc == 0 { ol.saturating_sub(1) } else { ol };
        let ns = if nc == 0 { nl.saturating_sub(1) } else { nl };
        out.push_str(&format!("@@ -{os},{oc} +{ns},{nc} @@\n"));
        out.push_str(&body);
    }
    if out.ends_with('\n') {
        out.pop();
    }
    (out, adds, dels)
}

fn split_lines(data: &[u8]) -> Vec<&str> {
    let text = std::str::from_utf8(data).unwrap_or("");
    if text.is_empty() {
        return Vec::new();
    }
    let t = text.strip_suffix('\n').unwrap_or(text);
    t.split('\n').collect()
}

enum Op<'a> {
    Eq(&'a str, ()),
    Del(&'a str),
    Add(&'a str),
}

fn lcs_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Eq(a[i], ()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op::Del(a[i]));
            i += 1;
        } else {
            ops.push(Op::Add(b[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Del(a[i]));
        i += 1;
    }
    while j < m {
        ops.push(Op::Add(b[j]));
        j += 1;
    }
    ops
}

/// Full `git diff` text for a set of changes.
pub fn diff_text(changes: &[FileChange]) -> String {
    let mut out = String::new();
    for c in changes {
        let old = c.previous_filename.as_deref().unwrap_or(&c.filename);
        out.push_str(&format!("diff --git a/{old} b/{}\n", c.filename));
        match c.status.as_str() {
            "added" => out.push_str("new file mode 100644\n"),
            "removed" => out.push_str("deleted file mode 100644\n"),
            "renamed" => out.push_str(&format!(
                "similarity index 90%\nrename from {old}\nrename to {}\n",
                c.filename
            )),
            _ => {}
        }
        let a = if c.status == "added" {
            "/dev/null".to_owned()
        } else {
            format!("a/{old}")
        };
        let b = if c.status == "removed" {
            "/dev/null".to_owned()
        } else {
            format!("b/{}", c.filename)
        };
        if !c.patch.is_empty() {
            out.push_str(&format!("--- {a}\n+++ {b}\n{}\n", c.patch));
        }
    }
    out
}

/// Stores a new commit whose tree is `map`, returning its SHA.
#[allow(clippy::too_many_arguments)]
pub fn write_commit(
    svc: &mut ServiceState,
    repo: &str,
    parent_shas: &[String],
    map: &FileMap,
    message: &str,
    author: &Value,
    author_login: Option<&str>,
    now: &str,
) -> String {
    let old = parent_shas
        .first()
        .and_then(|p| file_map(svc, repo, p))
        .unwrap_or_default();
    let changes = diff_maps(&old, map, &BTreeMap::new());
    let files: Vec<Value> = changes
        .iter()
        .map(|c| {
            if c.status == "removed" {
                json!({"filename": c.filename, "status": "removed"})
            } else {
                let e = &map[&c.filename];
                let mut f = file_json(e);
                f["filename"] = json!(c.filename);
                f["status"] = json!(c.status);
                f
            }
        })
        .collect();
    let tree = tree_sha(map, "");
    let sha = sha1_hex(
        format!(
            "commit {repo}\ntree {tree}\nparents {}\nmessage {message}\ndate {now}\n",
            parent_shas.join(",")
        )
        .as_bytes(),
    );
    let mut person = author.clone();
    if !person.is_object() {
        person = json!({});
    }
    if person.get("date").is_none() {
        person["date"] = json!(now);
    }
    let doc = json!({
        "sha": sha,
        "message": message,
        "author": person,
        "committer": {"name": "GitHub Enterprise", "email": "noreply@github.octo-corp.com", "date": now},
        "author_login": author_login,
        "committer_login": "web-flow",
        "parents": parent_shas,
        "files": files,
    });
    svc.store
        .collection_mut("commits")
        .insert(commit_key(repo, &sha), doc);
    sha
}

pub fn set_branch(svc: &mut ServiceState, repo: &str, branch: &str, sha: &str) {
    let key = format!("{repo}:{branch}");
    let coll = svc.store.collection_mut("branches");
    if let Some(doc) = coll.get_mut(&key) {
        doc["sha"] = json!(sha);
    } else {
        coll.insert(key, json!({"name": branch, "sha": sha, "protected": false}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_counts_and_hunks() {
        let (patch, adds, dels) = unified_diff(b"a\nb\nc\n", b"a\nB\nc\nd\n");
        assert_eq!((adds, dels), (2, 1));
        assert_eq!(patch, "@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d");
        let (patch, adds, dels) = unified_diff(b"", b"x\n");
        assert_eq!((adds, dels), (1, 0));
        assert_eq!(patch, "@@ -0,0 +1,1 @@\n+x");
    }

    #[test]
    fn tree_sha_matches_git() {
        // `git init; printf 'hello\n' > a.txt; git add .; git write-tree`
        let mut map = FileMap::new();
        map.insert(
            "a.txt".into(),
            FileEntry {
                content: b"hello\n".to_vec(),
                mode: "100644".into(),
            },
        );
        assert_eq!(
            tree_sha(&map, ""),
            "2e81171448eb9f2ee3821e3d447aa6b2fe3ddba1"
        );
    }
}
