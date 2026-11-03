//! GraphQL mutations.

use serde_json::{Map, Value, json};

use super::{
    exec::{FieldError, FieldResult},
    objects::{self as o, find_node, key_of},
    resolve::Gh,
};
use crate::{
    issues::{issue_fields, next_issue_db_id, next_number, rename_label_everywhere},
    projects,
    pulls::{
        self, after_submit, event_state, insert_review, insert_review_comment, review_node_id,
    },
    render::{self, all, get, label_doc, split_key, with_prefix},
    util::{arr, b, login_of, names_of, next_id, node_id, opt_s, s, u},
};

fn input(args: &Map<String, Value>) -> Value {
    args.get("input").cloned().unwrap_or(json!({}))
}

fn id_list(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn with_client_id(input: &Value, mut payload: Value) -> Value {
    payload["clientMutationId"] = input
        .get("clientMutationId")
        .cloned()
        .unwrap_or(Value::Null);
    payload
}

impl Gh<'_> {
    fn node(&self, id: &str) -> Result<Value, FieldError> {
        let v = find_node(self.svc, &self.base, id);
        if v.is_null() {
            Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )))
        } else {
            Ok(v)
        }
    }

    fn node_of(&self, id: &str, typenames: &[&str]) -> Result<Value, FieldError> {
        let v = self.node(id)?;
        let t = v["__typename"].as_str().unwrap_or("");
        if typenames.contains(&t) {
            Ok(v)
        } else {
            Err(FieldError::not_found(format!(
                "Could not resolve to {} node with the global id of '{id}'",
                typenames.join(" or ")
            )))
        }
    }

    fn login_from_node(&self, id: &str) -> Result<String, FieldError> {
        let v = self.node_of(id, &["User", "Bot", "Mannequin"])?;
        Ok(v["login"].as_str().unwrap_or("").to_owned())
    }

    fn label_names(&self, ids: &[String]) -> Result<Vec<String>, FieldError> {
        ids.iter()
            .map(|id| {
                let v = self.node_of(id, &["Label"])?;
                Ok(v["name"].as_str().unwrap_or("").to_owned())
            })
            .collect()
    }

    pub fn mutation(&mut self, field: &str, args: &Map<String, Value>) -> FieldResult {
        let inp = input(args);
        let out = match field {
            "createIssue" => self.create_issue(&inp)?,
            "updateIssue" => self.update_issue(&inp)?,
            "closeIssue" => self.close_issue(&inp)?,
            "reopenIssue" => self.reopen_issue(&inp)?,
            "createLabel" => self.create_label(&inp)?,
            "updateLabel" => self.update_label(&inp)?,
            "deleteLabel" => self.delete_label(&inp)?,
            "convertPullRequestToDraft" => self.set_draft(&inp, true)?,
            "markPullRequestReadyForReview" => self.set_draft(&inp, false)?,
            "addPullRequestReview" => self.add_review(&inp)?,
            "submitPullRequestReview" => self.submit_review(&inp)?,
            "deletePullRequestReview" => self.delete_review(&inp)?,
            "addPullRequestReviewThread" => self.add_thread(&inp)?,
            "resolveReviewThread" => self.resolve_thread(&inp, true)?,
            "unresolveReviewThread" => self.resolve_thread(&inp, false)?,
            "addDiscussionComment" => self.add_discussion_comment(&inp)?,
            "updateDiscussionComment" => self.update_discussion_comment(&inp)?,
            "deleteDiscussionComment" => self.delete_discussion_comment(&inp)?,
            "markDiscussionCommentAsAnswer" => self.mark_answer(&inp, true)?,
            "unmarkDiscussionCommentAsAnswer" => self.mark_answer(&inp, false)?,
            "minimizeComment" => self.minimize(&inp, true)?,
            "unminimizeComment" => self.minimize(&inp, false)?,
            "addProjectV2ItemById" => self.add_project_item(&inp)?,
            "updateProjectV2ItemFieldValue" => self.update_item_field(&inp, false)?,
            "clearProjectV2ItemFieldValue" => self.update_item_field(&inp, true)?,
            "deleteProjectV2Item" => self.delete_project_item(&inp)?,
            "createProjectV2StatusUpdate" => self.create_status_update(&inp)?,
            "createProjectV2" => self.create_project(&inp)?,
            "createProjectV2Field" => self.create_project_field(&inp)?,
            "updateProjectV2Field" => self.update_project_field(&inp)?,
            "createProjectV2View" => self.create_view(&inp)?,
            "updateProjectV2View" => self.update_view(&inp)?,
            "deleteProjectV2View" => self.delete_view(&inp)?,
            "setIssueFieldValue" => self.set_issue_field_value(&inp)?,
            _ => {
                return Err(FieldError::invalid(format!(
                    "Field '{field}' doesn't exist on type 'Mutation'"
                )));
            }
        };
        Ok(with_client_id(&inp, out))
    }

    // ------------------------------------------------------------ issues

    fn create_issue(&mut self, inp: &Value) -> FieldResult {
        let repo_obj = self.node_of(s(inp, "repositoryId"), &["Repository"])?;
        let repo = key_of(&repo_obj).unwrap_or_default();
        let title = s(inp, "title").trim().to_owned();
        if title.is_empty() {
            return Err(FieldError::unprocessable("Title can't be blank"));
        }
        let assignees: Vec<String> = id_list(inp, "assigneeIds")
            .iter()
            .map(|id| self.login_from_node(id))
            .collect::<Result<_, _>>()?;
        let labels = self.label_names(&id_list(inp, "labelIds"))?;
        let milestone = match opt_s(inp, "milestoneId") {
            Some(id) => Some(self.node_of(&id, &["Milestone"])?["number"].clone()),
            None => None,
        };
        let issue_type = match opt_s(inp, "issueTypeId") {
            Some(id) => {
                let owner = repo.split('/').next().unwrap_or("");
                let t = with_prefix(self.svc, "issue_types", &format!("{owner}:"))
                    .into_iter()
                    .find(|(_, t)| render::issue_type_out(owner, t)["node_id"] == id.as_str())
                    .map(|(_, t)| s(t, "name").to_owned());
                match t {
                    Some(t) => json!(t),
                    None => {
                        return Err(FieldError::not_found(format!(
                            "Could not resolve to an IssueType with the global id of '{id}'"
                        )));
                    }
                }
            }
            None => Value::Null,
        };
        let parent = match opt_s(inp, "parentIssueId") {
            Some(id) => Some(key_of(&self.node_of(&id, &["Issue"])?).unwrap_or_default()),
            None => None,
        };
        let number = next_number(self.svc, &repo);
        let id = next_issue_db_id(self.svc);
        let key = format!("{repo}#{number}");
        let node = self.svc.ids.next_prefixed("issue_node", "I_kwDO", 16);
        let doc = json!({
            "id": id,
            "node_id": node,
            "number": number,
            "title": title,
            "body": inp.get("body").cloned().unwrap_or(Value::Null),
            "user": self.viewer,
            "labels": labels,
            "assignees": assignees,
            "milestone": milestone,
            "state": "open",
            "state_reason": null,
            "type": issue_type,
            "parent": parent,
            "created_at": self.now,
            "updated_at": self.now,
            "closed_at": null,
        });
        self.svc
            .store
            .collection_mut("issues")
            .insert(key.clone(), doc);
        if let Some(p) = &parent
            && let Some(pd) = self.svc.store.collection_mut("issues").get_mut(p)
        {
            let mut subs = names_of(arr(pd, "sub_issues"));
            subs.push(key.clone());
            pd["sub_issues"] = json!(subs);
        }
        Ok(json!({"issue": o::issue(self.svc, &self.base, &key)}))
    }

    fn issue_key(&self, id: &str) -> Result<(String, &'static str), FieldError> {
        let v = self.node_of(id, &["Issue", "PullRequest"])?;
        let coll = if v["__typename"] == "Issue" {
            "issues"
        } else {
            "pulls"
        };
        Ok((key_of(&v).unwrap_or_default(), coll))
    }

    fn update_issue(&mut self, inp: &Value) -> FieldResult {
        let (key, coll) = self.issue_key(s(inp, "id"))?;
        let assignees = match inp.get("assigneeIds") {
            Some(Value::Array(ids)) => {
                let mut out = Vec::new();
                for id in ids.iter().filter_map(Value::as_str) {
                    let v = self.node_of(id, &["User", "Bot", "Mannequin"])?;
                    out.push(v["login"].as_str().unwrap_or("").to_owned());
                }
                Some(out)
            }
            _ => None,
        };
        let labels = match inp.get("labelIds") {
            Some(Value::Array(_)) => Some(self.label_names(&id_list(inp, "labelIds"))?),
            _ => None,
        };
        let milestone = match inp.get("milestoneId") {
            Some(Value::String(id)) => Some(self.node_of(id, &["Milestone"])?["number"].clone()),
            Some(Value::Null) => Some(Value::Null),
            _ => None,
        };
        let now = self.now.clone();
        let Some(d) = self.svc.store.collection_mut(coll).get_mut(&key) else {
            return Err(FieldError::not_found("Issue not found"));
        };
        if let Some(t) = inp.get("title").and_then(Value::as_str) {
            d["title"] = json!(t);
        }
        if let Some(bd) = inp.get("body") {
            d["body"] = bd.clone();
        }
        if let Some(a) = assignees {
            d["assignees"] = json!(a);
        }
        if let Some(l) = labels {
            d["labels"] = json!(l);
        }
        if let Some(m) = milestone {
            d["milestone"] = m;
        }
        if let Some(st) = inp.get("state").and_then(Value::as_str) {
            d["state"] = json!(st.to_ascii_lowercase());
        }
        d["updated_at"] = json!(now);
        let obj = o::issue_or_pull(self.svc, &self.base, &key);
        Ok(json!({"issue": obj}))
    }

    fn close_issue(&mut self, inp: &Value) -> FieldResult {
        let (key, _) = self.issue_key(s(inp, "issueId"))?;
        let reason = opt_s(inp, "stateReason")
            .unwrap_or_else(|| "COMPLETED".into())
            .to_ascii_lowercase();
        let dup = match opt_s(inp, "duplicateIssueId") {
            Some(id) => Some(self.issue_key(&id)?.0),
            None => None,
        };
        if reason == "duplicate" && dup.is_none() {
            return Err(FieldError::unprocessable(
                "A duplicate issue is required when closing as a duplicate",
            ));
        }
        let now = self.now.clone();
        let viewer = self.viewer.clone();
        if let Some(d) = self.svc.store.collection_mut("issues").get_mut(&key) {
            if s(d, "state") != "closed" {
                d["closed_at"] = json!(now);
                d["closed_by"] = json!(viewer);
            }
            d["state"] = json!("closed");
            d["state_reason"] = json!(reason);
            if let Some(dk) = &dup {
                d["duplicate_of"] = json!(dk);
            }
            d["updated_at"] = json!(now);
        }
        Ok(json!({"issue": o::issue(self.svc, &self.base, &key)}))
    }

    fn reopen_issue(&mut self, inp: &Value) -> FieldResult {
        let (key, _) = self.issue_key(s(inp, "issueId"))?;
        let now = self.now.clone();
        if let Some(d) = self.svc.store.collection_mut("issues").get_mut(&key) {
            if s(d, "state") == "closed" {
                d["state_reason"] = json!("reopened");
            }
            d["state"] = json!("open");
            d["closed_at"] = Value::Null;
            d["closed_by"] = Value::Null;
            d["updated_at"] = json!(now);
            if let Some(o) = d.as_object_mut() {
                o.remove("duplicate_of");
            }
        }
        Ok(json!({"issue": o::issue(self.svc, &self.base, &key)}))
    }

    // ------------------------------------------------------------ labels

    fn create_label(&mut self, inp: &Value) -> FieldResult {
        let repo_obj = self.node_of(s(inp, "repositoryId"), &["Repository"])?;
        let repo = key_of(&repo_obj).unwrap_or_default();
        let name = s(inp, "name").trim().to_owned();
        if name.is_empty() {
            return Err(FieldError::unprocessable("Name can't be blank"));
        }
        if label_doc(self.svc, &repo, &name).is_some() {
            return Err(FieldError::unprocessable("Name has already been taken"));
        }
        let color = s(inp, "color").trim_start_matches('#').to_ascii_lowercase();
        if color.len() != 6 || !color.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FieldError::unprocessable("Color is invalid"));
        }
        let id = next_id(self.svc, "labels", "id", 1);
        let key = format!("{repo}:{name}");
        self.svc.store.collection_mut("labels").insert(
            key.clone(),
            json!({"id": id, "node_id": node_id("LA", &key), "name": name, "color": color, "description": inp.get("description").cloned().unwrap_or(Value::Null), "default": false, "created_at": self.now}),
        );
        Ok(json!({"label": o::label(self.svc, &self.base, &repo, &name)}))
    }

    fn label_key(&self, id: &str) -> Result<(String, String, String), FieldError> {
        let v = self.node_of(id, &["Label"])?;
        let repo = v["_repo"].as_str().unwrap_or("").to_owned();
        let name = v["_name"].as_str().unwrap_or("").to_owned();
        let key = label_doc(self.svc, &repo, &name)
            .map(|(k, _)| k.to_owned())
            .unwrap_or_default();
        Ok((key, repo, name))
    }

    fn update_label(&mut self, inp: &Value) -> FieldResult {
        let (key, repo, old) = self.label_key(s(inp, "id"))?;
        let mut doc = get(self.svc, "labels", &key).cloned().unwrap_or_default();
        if let Some(c) = opt_s(inp, "color") {
            doc["color"] = json!(c.trim_start_matches('#').to_ascii_lowercase());
        }
        if let Some(d) = inp.get("description") {
            doc["description"] = d.clone();
        }
        let new_name = opt_s(inp, "name").unwrap_or_else(|| old.clone());
        if !new_name.eq_ignore_ascii_case(&old) && label_doc(self.svc, &repo, &new_name).is_some() {
            return Err(FieldError::unprocessable("Name has already been taken"));
        }
        doc["name"] = json!(new_name);
        self.svc.store.collection_mut("labels").remove(&key);
        self.svc
            .store
            .collection_mut("labels")
            .insert(format!("{repo}:{new_name}"), doc);
        if new_name != old {
            rename_label_everywhere(self.svc, &repo, &old, Some(&new_name));
        }
        Ok(json!({"label": o::label(self.svc, &self.base, &repo, &new_name)}))
    }

    fn delete_label(&mut self, inp: &Value) -> FieldResult {
        let (key, repo, name) = self.label_key(s(inp, "id"))?;
        self.svc.store.collection_mut("labels").remove(&key);
        rename_label_everywhere(self.svc, &repo, &name, None);
        Ok(json!({}))
    }

    // ------------------------------------------------------------ pull requests

    fn pull_key(&self, id: &str) -> Result<String, FieldError> {
        Ok(key_of(&self.node_of(id, &["PullRequest"])?).unwrap_or_default())
    }

    fn set_draft(&mut self, inp: &Value, draft: bool) -> FieldResult {
        let key = self.pull_key(s(inp, "pullRequestId"))?;
        let now = self.now.clone();
        if let Some(p) = self.svc.store.collection_mut("pulls").get_mut(&key) {
            if s(p, "state") != "open" {
                return Err(FieldError::unprocessable("Pull request is closed"));
            }
            p["draft"] = json!(draft);
            p["updated_at"] = json!(now);
        }
        Ok(json!({"pullRequest": o::pull(self.svc, &self.base, &key)}))
    }

    fn add_review(&mut self, inp: &Value) -> FieldResult {
        let key = self.pull_key(s(inp, "pullRequestId"))?;
        let event = s(inp, "event");
        let Some(state) = event_state(event) else {
            return Err(FieldError::invalid(format!(
                "Argument 'event' on InputObject 'AddPullRequestReviewInput' has an invalid value ({event})."
            )));
        };
        let (repo, _) = split_key(&key);
        let p = get(self.svc, "pulls", &key).cloned().unwrap_or_default();
        let commit =
            opt_s(inp, "commitOID").unwrap_or_else(|| render::pr_head_sha(self.svc, repo, &p));
        let viewer = self.viewer.clone();
        let now = self.now.clone();
        let doc = insert_review(
            self.svc,
            &key,
            &viewer,
            s(inp, "body"),
            state,
            &commit,
            &now,
        )
        .map_err(FieldError::unprocessable)?;
        let rid = u(&doc, "id");
        for t in arr(inp, "threads") {
            insert_review_comment(
                self.svc,
                &key,
                &viewer,
                s(t, "body"),
                s(t, "path"),
                t.get("line").and_then(Value::as_u64),
                t.get("startLine").and_then(Value::as_u64),
                &opt_s(t, "side").unwrap_or_else(|| "RIGHT".into()),
                None,
                Some(rid),
                &commit,
                &now,
                "line",
            );
        }
        Ok(json!({"pullRequestReview": o::review(self.svc, &self.base, &doc)}))
    }

    fn review_doc(&self, id: &str) -> Result<Value, FieldError> {
        all(self.svc, "reviews")
            .into_iter()
            .map(|(_, r)| r.clone())
            .find(|r| review_node_id(r) == id)
            .ok_or_else(|| {
                FieldError::not_found(format!(
                    "Could not resolve to a node with the global id of '{id}'"
                ))
            })
    }

    fn submit_review(&mut self, inp: &Value) -> FieldResult {
        let mut r = self.review_doc(s(inp, "pullRequestReviewId"))?;
        if s(&r, "state") != "PENDING" {
            return Err(FieldError::unprocessable(
                "Review has already been submitted",
            ));
        }
        let event = s(inp, "event");
        let Some(state) = event_state(event).filter(|st| *st != "PENDING") else {
            return Err(FieldError::invalid(format!(
                "Argument 'event' on InputObject 'SubmitPullRequestReviewInput' has an invalid value ({event})."
            )));
        };
        let pull_key = format!("{}#{}", s(&r, "repo"), u(&r, "pull_number"));
        let p = get(self.svc, "pulls", &pull_key)
            .cloned()
            .unwrap_or_default();
        let author = login_of(p.get("user").unwrap_or(&Value::Null));
        if author.eq_ignore_ascii_case(&self.viewer)
            && (state == "APPROVED" || state == "CHANGES_REQUESTED")
        {
            return Err(FieldError::unprocessable(if state == "APPROVED" {
                "Can not approve your own pull request"
            } else {
                "Can not request changes on your own pull request"
            }));
        }
        if let Some(body) = opt_s(inp, "body") {
            r["body"] = json!(body);
        }
        if state == "CHANGES_REQUESTED" && s(&r, "body").trim().is_empty() {
            return Err(FieldError::unprocessable("Body can't be blank"));
        }
        r["state"] = json!(state);
        r["submitted_at"] = json!(self.now);
        let id = u(&r, "id").to_string();
        self.svc
            .store
            .collection_mut("reviews")
            .insert(id, r.clone());
        let viewer = self.viewer.clone();
        let now = self.now.clone();
        after_submit(self.svc, &pull_key, &viewer, &now);
        Ok(json!({"pullRequestReview": o::review(self.svc, &self.base, &r)}))
    }

    fn delete_review(&mut self, inp: &Value) -> FieldResult {
        let r = self.review_doc(s(inp, "pullRequestReviewId"))?;
        if s(&r, "state") != "PENDING" {
            return Err(FieldError::unprocessable(
                "Can not delete a non-pending pull request review",
            ));
        }
        let rid = u(&r, "id");
        self.svc
            .store
            .collection_mut("reviews")
            .remove(&rid.to_string());
        let comment_keys: Vec<String> = all(self.svc, "review_comments")
            .iter()
            .filter(|(_, c)| u(c, "review_id") == rid)
            .map(|(k, _)| (*k).to_owned())
            .collect();
        for k in comment_keys {
            self.svc.store.collection_mut("review_comments").remove(&k);
        }
        Ok(json!({"pullRequestReview": o::review(self.svc, &self.base, &r)}))
    }

    fn add_thread(&mut self, inp: &Value) -> FieldResult {
        let (key, review_id) = match opt_s(inp, "pullRequestReviewId") {
            Some(id) => {
                let r = self.review_doc(&id)?;
                (
                    format!("{}#{}", s(&r, "repo"), u(&r, "pull_number")),
                    Some(u(&r, "id")),
                )
            }
            None => (self.pull_key(s(inp, "pullRequestId"))?, None),
        };
        let (repo, _) = split_key(&key);
        let p = get(self.svc, "pulls", &key).cloned().unwrap_or_default();
        let (changes, _) = render::pr_changes(self.svc, repo, &p);
        let path = s(inp, "path").to_owned();
        let subject = opt_s(inp, "subjectType")
            .unwrap_or_else(|| "LINE".into())
            .to_ascii_lowercase();
        let line = inp.get("line").and_then(Value::as_u64);
        let Some(change) = changes.iter().find(|c| c.filename == path) else {
            return Ok(json!({"thread": null}));
        };
        if subject == "line" {
            let Some(l) = line else {
                return Ok(json!({"thread": null}));
            };
            // The line must fall inside a diff hunk on the requested side.
            let side_right = opt_s(inp, "side").is_none_or(|sd| sd == "RIGHT");
            if !line_in_patch(&change.patch, l, side_right) {
                return Ok(json!({"thread": null}));
            }
        }
        if s(inp, "body").trim().is_empty() {
            return Err(FieldError::unprocessable("Body can't be blank"));
        }
        let commit = render::pr_head_sha(self.svc, repo, &p);
        let viewer = self.viewer.clone();
        let now = self.now.clone();
        let doc = insert_review_comment(
            self.svc,
            &key,
            &viewer,
            s(inp, "body"),
            &path,
            line,
            inp.get("startLine").and_then(Value::as_u64),
            &opt_s(inp, "side").unwrap_or_else(|| "RIGHT".into()),
            None,
            review_id,
            &commit,
            &now,
            &subject,
        );
        Ok(json!({"thread": o::thread(self.svc, &doc)}))
    }

    fn resolve_thread(&mut self, inp: &Value, resolved: bool) -> FieldResult {
        let id = s(inp, "threadId").to_owned();
        let v = self.node_of(&id, &["PullRequestReviewThread"])?;
        let viewer = self.viewer.clone();
        let pull = v["_pull"].as_str().unwrap_or("").to_owned();
        self.svc.store.collection_mut("review_threads").insert(
            id.clone(),
            json!({"pull": pull, "is_resolved": resolved, "resolved_by": if resolved { json!(viewer) } else { Value::Null }}),
        );
        let root = all(self.svc, "review_comments")
            .into_iter()
            .map(|(_, c)| c.clone())
            .find(|c| {
                c.get("in_reply_to_id").is_none_or(Value::is_null) && o::thread_node_id(c) == id
            })
            .unwrap_or_default();
        Ok(json!({"thread": o::thread(self.svc, &root)}))
    }

    // ------------------------------------------------------------ discussions

    fn add_discussion_comment(&mut self, inp: &Value) -> FieldResult {
        let d = self.node_of(s(inp, "discussionId"), &["Discussion"])?;
        let dkey = key_of(&d).unwrap_or_default();
        let body = s(inp, "body").to_owned();
        if body.trim().is_empty() {
            return Err(FieldError::unprocessable("Body can't be blank"));
        }
        let reply_to = match opt_s(inp, "replyToId") {
            Some(rid) => match get(self.svc, "discussion_comments", &rid) {
                // Replies nest one level: replying to a reply attaches to its parent.
                Some(c) if s(c, "discussion") == dkey => Some(opt_s(c, "reply_to").unwrap_or(rid)),
                _ => None,
            },
            None => None,
        };
        let db_id = next_id(self.svc, "discussion_comments", "id", 1);
        let node = self
            .svc
            .ids
            .next_prefixed("discussion_comment", "DC_kwDO", 16);
        self.svc.store.collection_mut("discussion_comments").insert(
            node.clone(),
            json!({"id": db_id, "discussion": dkey, "reply_to": reply_to, "body": body, "author": self.viewer, "created_at": self.now, "updated_at": self.now}),
        );
        let now = self.now.clone();
        if let Some(dd) = self.svc.store.collection_mut("discussions").get_mut(&dkey) {
            dd["updated_at"] = json!(now);
        }
        Ok(json!({"comment": o::discussion_comment(self.svc, &self.base, &node)}))
    }

    fn update_discussion_comment(&mut self, inp: &Value) -> FieldResult {
        let id = s(inp, "commentId").to_owned();
        if get(self.svc, "discussion_comments", &id).is_none() {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )));
        }
        let body = s(inp, "body").to_owned();
        if body.trim().is_empty() {
            return Err(FieldError::unprocessable("Body can't be blank"));
        }
        let now = self.now.clone();
        if let Some(c) = self
            .svc
            .store
            .collection_mut("discussion_comments")
            .get_mut(&id)
        {
            c["body"] = json!(body);
            c["updated_at"] = json!(now);
        }
        Ok(json!({"comment": o::discussion_comment(self.svc, &self.base, &id)}))
    }

    fn delete_discussion_comment(&mut self, inp: &Value) -> FieldResult {
        let id = s(inp, "id").to_owned();
        let obj = o::discussion_comment(self.svc, &self.base, &id);
        if obj.is_null() {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )));
        }
        let replies: Vec<String> = all(self.svc, "discussion_comments")
            .iter()
            .filter(|(_, c)| s(c, "reply_to") == id)
            .map(|(k, _)| (*k).to_owned())
            .collect();
        let dkey = obj["_discussion"].as_str().unwrap_or("").to_owned();
        let coll = self.svc.store.collection_mut("discussion_comments");
        coll.remove(&id);
        for r in replies {
            coll.remove(&r);
        }
        if let Some(d) = self.svc.store.collection_mut("discussions").get_mut(&dkey)
            && d.get("answer_comment").and_then(Value::as_str) == Some(id.as_str())
        {
            d["answer_comment"] = Value::Null;
            d["answer_chosen_at"] = Value::Null;
        }
        Ok(json!({"comment": obj}))
    }

    fn mark_answer(&mut self, inp: &Value, mark: bool) -> FieldResult {
        let id = s(inp, "id").to_owned();
        let Some(c) = get(self.svc, "discussion_comments", &id).cloned() else {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )));
        };
        let dkey = s(&c, "discussion").to_owned();
        let (repo, _) = split_key(&dkey);
        let d = get(self.svc, "discussions", &dkey)
            .cloned()
            .unwrap_or_default();
        let answerable = o::category_doc(self.svc, repo, s(&d, "category"))
            .is_some_and(|(_, cat)| b(cat, "is_answerable"));
        if mark && !answerable {
            return Err(FieldError::unprocessable(
                "This discussion's category does not accept answers",
            ));
        }
        if mark && c.get("reply_to").is_some_and(|r| !r.is_null()) {
            return Err(FieldError::unprocessable(
                "Only top-level comments can be marked as the answer",
            ));
        }
        let now = self.now.clone();
        let viewer = self.viewer.clone();
        if let Some(dd) = self.svc.store.collection_mut("discussions").get_mut(&dkey) {
            if mark {
                dd["answer_comment"] = json!(id);
                dd["answer_chosen_at"] = json!(now);
                dd["answer_chosen_by"] = json!(viewer);
            } else if dd.get("answer_comment").and_then(Value::as_str) == Some(id.as_str()) {
                dd["answer_comment"] = Value::Null;
                dd["answer_chosen_at"] = Value::Null;
                dd["answer_chosen_by"] = Value::Null;
            }
        }
        Ok(json!({"discussion": o::discussion(self.svc, &self.base, &dkey)}))
    }

    fn minimize(&mut self, inp: &Value, minimize: bool) -> FieldResult {
        let id = s(inp, "subjectId").to_owned();
        let v = self.node(&id)?;
        let reason = opt_s(inp, "classifier").map(|c| c.to_ascii_lowercase());
        let (coll, key) = match v["__typename"].as_str().unwrap_or("") {
            "DiscussionComment" => ("discussion_comments", id.clone()),
            "IssueComment" => ("issue_comments", v["_id"].as_u64().unwrap_or(0).to_string()),
            "PullRequestReviewComment" => (
                "review_comments",
                v["_id"].as_u64().unwrap_or(0).to_string(),
            ),
            "PullRequestReview" => ("reviews", v["_id"].as_u64().unwrap_or(0).to_string()),
            t => {
                return Err(FieldError::unprocessable(format!(
                    "{t} cannot be minimized"
                )));
            }
        };
        if let Some(d) = self.svc.store.collection_mut(coll).get_mut(&key) {
            d["minimized"] = json!(minimize);
            d["minimized_reason"] = if minimize { json!(reason) } else { Value::Null };
        }
        let field = if minimize {
            "minimizedComment"
        } else {
            "unminimizedComment"
        };
        Ok(
            json!({field: {"__typename": v["__typename"], "id": id, "isMinimized": minimize, "minimizedReason": if minimize { json!(reason) } else { Value::Null }, "viewerCanMinimize": true}}),
        )
    }

    // ------------------------------------------------------------ projects

    fn project_key_of(&self, id: &str) -> Result<String, FieldError> {
        Ok(key_of(&self.node_of(id, &["ProjectV2"])?).unwrap_or_default())
    }

    fn add_project_item(&mut self, inp: &Value) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let content = self.node_of(s(inp, "contentId"), &["Issue", "PullRequest"])?;
        let ckey = key_of(&content).unwrap_or_default();
        let viewer = self.viewer.clone();
        let now = self.now.clone();
        let (key, _) = projects::insert_item(self.svc, &pkey, &ckey, &viewer, &now);
        Ok(json!({"item": o::project_item(self.svc, &key)}))
    }

    fn item_key_in(&self, pkey: &str, item_id: &str) -> Result<String, FieldError> {
        projects::items_of(self.svc, pkey)
            .into_iter()
            .find(|(k, i)| projects::item_node_id(k, i) == item_id)
            .map(|(k, _)| k)
            .ok_or_else(|| {
                FieldError::not_found(format!(
                    "Could not resolve to a node with the global id of '{item_id}'"
                ))
            })
    }

    fn update_item_field(&mut self, inp: &Value, clear: bool) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let ikey = self.item_key_in(&pkey, s(inp, "itemId"))?;
        let fid = s(inp, "fieldId").to_owned();
        let Some(f) = projects::fields_of(self.svc, &pkey)
            .into_iter()
            .find(|f| projects::field_node_id(&pkey, f) == fid)
        else {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{fid}'"
            )));
        };
        let stored = if clear {
            Value::Null
        } else {
            let v = inp.get("value").cloned().unwrap_or(json!({}));
            let raw = v
                .get("text")
                .or_else(|| v.get("number"))
                .or_else(|| v.get("date"))
                .or_else(|| v.get("singleSelectOptionId"))
                .or_else(|| v.get("iterationId"))
                .cloned()
                .unwrap_or(Value::Null);
            if raw.is_null() {
                return Err(FieldError::unprocessable("A value is required"));
            }
            projects::normalize_value(&f, &raw).map_err(FieldError::unprocessable)?
        };
        let now = self.now.clone();
        let fkey = u(&f, "id").to_string();
        if let Some(i) = self
            .svc
            .store
            .collection_mut("project_items")
            .get_mut(&ikey)
        {
            if !i.get("field_values").is_some_and(Value::is_object) {
                i["field_values"] = json!({});
            }
            if stored.is_null() {
                i["field_values"].as_object_mut().map(|o| o.remove(&fkey));
            } else {
                i["field_values"][fkey] = stored;
            }
            i["updated_at"] = json!(now);
        }
        Ok(json!({"projectV2Item": o::project_item(self.svc, &ikey)}))
    }

    fn delete_project_item(&mut self, inp: &Value) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let ikey = self.item_key_in(&pkey, s(inp, "itemId"))?;
        let obj = o::project_item(self.svc, &ikey);
        self.svc.store.collection_mut("project_items").remove(&ikey);
        Ok(json!({"deletedItemId": obj["id"]}))
    }

    fn create_status_update(&mut self, inp: &Value) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let id = self
            .svc
            .ids
            .next_prefixed("status_update", "PVTSU_lADO", 16);
        let db = next_id(self.svc, "project_status_updates", "database_id", 1);
        self.svc
            .store
            .collection_mut("project_status_updates")
            .insert(
                id.clone(),
                json!({
                    "database_id": db,
                    "project": pkey,
                    "body": inp.get("body").cloned().unwrap_or(Value::Null),
                    "status": inp.get("status").cloned().unwrap_or(Value::Null),
                    "start_date": inp.get("startDate").cloned().unwrap_or(Value::Null),
                    "target_date": inp.get("targetDate").cloned().unwrap_or(Value::Null),
                    "creator": self.viewer,
                    "created_at": self.now,
                    "updated_at": self.now,
                }),
            );
        Ok(json!({"statusUpdate": o::status_update(self.svc, &id)}))
    }

    fn create_project(&mut self, inp: &Value) -> FieldResult {
        let owner = self.node_of(s(inp, "ownerId"), &["User", "Organization"])?;
        let login = owner["login"].as_str().unwrap_or("").to_owned();
        let title = s(inp, "title").trim().to_owned();
        if title.is_empty() {
            return Err(FieldError::unprocessable("Title can't be blank"));
        }
        let number = with_prefix(self.svc, "projects", &format!("{login}/"))
            .iter()
            .map(|(_, p)| u(p, "number"))
            .max()
            .unwrap_or(0)
            + 1;
        let id = next_id(self.svc, "projects", "id", 1);
        let pkey = format!("{login}/{number}");
        self.svc.store.collection_mut("projects").insert(
            pkey.clone(),
            json!({"id": id, "node_id": node_id("PVT", &pkey), "number": number, "title": title, "public": false, "closed": false, "owner": login, "creator": self.viewer, "created_at": self.now, "updated_at": self.now}),
        );
        let base_field = next_id(self.svc, "project_fields", "id", 1);
        let defaults = [
            ("Title", "TITLE", None),
            ("Assignees", "ASSIGNEES", None),
            (
                "Status",
                "SINGLE_SELECT",
                Some(json!([
                    {"id": format!("{:08x}", base_field * 16 + 1), "name": "Todo", "color": "GREEN"},
                    {"id": format!("{:08x}", base_field * 16 + 2), "name": "In Progress", "color": "YELLOW"},
                    {"id": format!("{:08x}", base_field * 16 + 3), "name": "Done", "color": "PURPLE"}
                ])),
            ),
            ("Labels", "LABELS", None),
            ("Repository", "REPOSITORY", None),
            ("Milestone", "MILESTONE", None),
        ];
        for (i, (name, dt, options)) in defaults.into_iter().enumerate() {
            let fid = base_field + i as u64;
            let mut f = json!({"id": fid, "name": name, "data_type": dt, "created_at": self.now, "updated_at": self.now});
            if let Some(opts) = options {
                f["options"] = opts;
            }
            self.svc
                .store
                .collection_mut("project_fields")
                .insert(format!("{pkey}:{fid}"), f);
        }
        let view_id = self.svc.ids.next_prefixed("project_view", "PVTV_lADO", 16);
        self.svc.store.collection_mut("project_views").insert(
            view_id,
            json!({"database_id": 1, "project": pkey, "number": 1, "name": "View 1", "layout": "TABLE_LAYOUT", "filter": null, "visible_fields": [base_field, base_field + 1, base_field + 2], "created_at": self.now}),
        );
        Ok(json!({"projectV2": o::project(self.svc, &self.base, &pkey)}))
    }

    fn create_project_field(&mut self, inp: &Value) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let name = s(inp, "name").trim().to_owned();
        if name.is_empty() {
            return Err(FieldError::unprocessable("Name can't be blank"));
        }
        if projects::fields_of(self.svc, &pkey)
            .iter()
            .any(|f| s(f, "name").eq_ignore_ascii_case(&name))
        {
            return Err(FieldError::unprocessable("Name has already been taken"));
        }
        let dt = s(inp, "dataType").to_ascii_uppercase();
        if !["TEXT", "NUMBER", "DATE", "SINGLE_SELECT", "ITERATION"].contains(&dt.as_str()) {
            return Err(FieldError::invalid(format!(
                "Argument 'dataType' on InputObject 'CreateProjectV2FieldInput' has an invalid value ({dt})."
            )));
        }
        let fid = next_id(self.svc, "project_fields", "id", 1);
        let mut f = json!({"id": fid, "name": name, "data_type": dt, "created_at": self.now, "updated_at": self.now});
        if dt == "SINGLE_SELECT" {
            let opts: Vec<Value> = arr(inp, "singleSelectOptions")
                .iter()
                .enumerate()
                .map(|(i, op)| json!({"id": format!("{:08x}", fid * 64 + i as u64 + 1), "name": s(op, "name"), "color": opt_s(op, "color").unwrap_or_else(|| "GRAY".into()), "description": s(op, "description")}))
                .collect();
            f["options"] = json!(opts);
        }
        if dt == "ITERATION" {
            f["configuration"] =
                iteration_config(fid, inp.get("iterationConfiguration").unwrap_or(&json!({})));
        }
        self.svc
            .store
            .collection_mut("project_fields")
            .insert(format!("{pkey}:{fid}"), f.clone());
        Ok(json!({"projectV2Field": o::project_field(&pkey, &f)}))
    }

    fn update_project_field(&mut self, inp: &Value) -> FieldResult {
        let fid = s(inp, "fieldId").to_owned();
        let found = all(self.svc, "project_fields").into_iter().find(|(k, f)| {
            let pkey = k.rsplit_once(':').map(|(p, _)| p).unwrap_or("");
            projects::field_node_id(pkey, f) == fid
        });
        let Some((key, f)) = found.map(|(k, f)| (k.to_owned(), f.clone())) else {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{fid}'"
            )));
        };
        let pkey = key
            .rsplit_once(':')
            .map(|(p, _)| p.to_owned())
            .unwrap_or_default();
        let mut f = f;
        if let Some(n) = opt_s(inp, "name") {
            f["name"] = json!(n);
        }
        if let Some(cfg) = inp.get("iterationConfiguration") {
            f["configuration"] = iteration_config(u(&f, "id"), cfg);
        }
        if let Some(opts) = inp.get("singleSelectOptions").and_then(Value::as_array) {
            let fidn = u(&f, "id");
            f["options"] = json!(opts.iter().enumerate().map(|(i, op)| json!({"id": format!("{:08x}", fidn * 64 + i as u64 + 1), "name": s(op, "name"), "color": opt_s(op, "color").unwrap_or_else(|| "GRAY".into()), "description": s(op, "description")})).collect::<Vec<_>>());
        }
        f["updated_at"] = json!(self.now);
        self.svc
            .store
            .collection_mut("project_fields")
            .insert(key, f.clone());
        Ok(json!({"projectV2Field": o::project_field(&pkey, &f)}))
    }

    fn visible_field_ids(&self, pkey: &str, inp: &Value) -> Result<Option<Vec<u64>>, FieldError> {
        let Some(cfg) = inp.get("configuration").filter(|c| !c.is_null()) else {
            return Ok(None);
        };
        let defs = projects::fields_of(self.svc, pkey);
        let mut out = Vec::new();
        for id in id_list(cfg, "visibleFieldIds") {
            match defs.iter().find(|f| projects::field_node_id(pkey, f) == id) {
                Some(f) => out.push(u(f, "id")),
                None => {
                    return Err(FieldError::not_found(format!(
                        "Could not resolve to a node with the global id of '{id}'"
                    )));
                }
            }
        }
        Ok(Some(out))
    }

    fn create_view(&mut self, inp: &Value) -> FieldResult {
        let pkey = self.project_key_of(s(inp, "projectId"))?;
        let visible = self.visible_field_ids(&pkey, inp)?;
        let number = all(self.svc, "project_views")
            .iter()
            .filter(|(_, v)| s(v, "project") == pkey)
            .map(|(_, v)| u(v, "number"))
            .max()
            .unwrap_or(0)
            + 1;
        let id = self.svc.ids.next_prefixed("project_view", "PVTV_lADO", 16);
        let db = next_id(self.svc, "project_views", "database_id", 1);
        let visible = visible.unwrap_or_else(|| {
            projects::fields_of(self.svc, &pkey)
                .iter()
                .take(3)
                .map(|f| u(f, "id"))
                .collect()
        });
        self.svc.store.collection_mut("project_views").insert(
            id.clone(),
            json!({"database_id": db, "project": pkey, "number": number, "name": s(inp, "name"), "layout": opt_s(inp, "layout").unwrap_or_else(|| "TABLE_LAYOUT".into()), "filter": inp.get("filter").cloned().unwrap_or(Value::Null), "visible_fields": visible, "created_at": self.now, "updated_at": self.now}),
        );
        Ok(json!({"projectV2View": o::view(self.svc, &id)}))
    }

    fn update_view(&mut self, inp: &Value) -> FieldResult {
        let id = s(inp, "viewId").to_owned();
        let Some(v) = get(self.svc, "project_views", &id).cloned() else {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )));
        };
        let pkey = s(&v, "project").to_owned();
        let visible = self.visible_field_ids(&pkey, inp)?;
        let mut v = v;
        if let Some(n) = opt_s(inp, "name") {
            v["name"] = json!(n);
        }
        if let Some(l) = opt_s(inp, "layout") {
            v["layout"] = json!(l);
        }
        if let Some(fl) = inp.get("filter") {
            v["filter"] = fl.clone();
        }
        if let Some(vis) = visible {
            v["visible_fields"] = json!(vis);
        }
        v["updated_at"] = json!(self.now);
        self.svc
            .store
            .collection_mut("project_views")
            .insert(id.clone(), v);
        Ok(json!({"projectV2View": o::view(self.svc, &id)}))
    }

    fn delete_view(&mut self, inp: &Value) -> FieldResult {
        let id = s(inp, "viewId").to_owned();
        let obj = o::view(self.svc, &id);
        if obj.is_null() {
            return Err(FieldError::not_found(format!(
                "Could not resolve to a node with the global id of '{id}'"
            )));
        }
        self.svc.store.collection_mut("project_views").remove(&id);
        Ok(json!({"projectV2View": obj, "deletedViewId": id}))
    }

    // ------------------------------------------------------------ issue fields

    fn set_issue_field_value(&mut self, inp: &Value) -> FieldResult {
        let (key, coll) = self.issue_key(s(inp, "issueId"))?;
        if coll != "issues" {
            return Err(FieldError::unprocessable(
                "Issue fields can only be set on issues",
            ));
        }
        let (repo, _) = split_key(&key);
        let owner = repo.split('/').next().unwrap_or("").to_owned();
        let defs = issue_fields(self.svc, &owner);
        let mut values: Vec<Value> = get(self.svc, "issues", &key)
            .map(|i| arr(i, "field_values").to_vec())
            .unwrap_or_default();
        for upd in arr(inp, "issueFields") {
            let fid = s(upd, "fieldId");
            let Some(def) = defs
                .iter()
                .find(|d| o::issue_field_node_id(&owner, d) == fid)
            else {
                return Err(FieldError::not_found(format!(
                    "Could not resolve to a node with the global id of '{fid}'"
                )));
            };
            let dbid = u(def, "id");
            values.retain(|v| u(v, "field_id") != dbid);
            if b(upd, "delete") {
                continue;
            }
            let value =
                if let Some(opt_id) = upd.get("singleSelectOptionId").and_then(Value::as_str) {
                    match arr(def, "options")
                        .iter()
                        .find(|op| o::issue_field_option_id(&owner, def, op) == opt_id)
                    {
                        Some(op) => json!(s(op, "name")),
                        None => {
                            return Err(FieldError::not_found(format!(
                                "Could not resolve to a node with the global id of '{opt_id}'"
                            )));
                        }
                    }
                } else if let Some(n) = upd.get("numberValue").filter(|n| !n.is_null()) {
                    n.clone()
                } else if let Some(d) = upd.get("dateValue").filter(|n| !n.is_null()) {
                    d.clone()
                } else if let Some(t) = upd.get("textValue").filter(|n| !n.is_null()) {
                    t.clone()
                } else {
                    return Err(FieldError::unprocessable("A value is required"));
                };
            values.push(json!({"field_id": dbid, "value": value}));
        }
        let now = self.now.clone();
        if let Some(i) = self.svc.store.collection_mut("issues").get_mut(&key) {
            i["field_values"] = json!(values);
            i["updated_at"] = json!(now);
        }
        Ok(json!({"issue": o::issue(self.svc, &self.base, &key)}))
    }
}

fn iteration_config(fid: u64, cfg: &Value) -> Value {
    let mut iterations: Vec<Value> = arr(cfg, "iterations")
        .iter()
        .enumerate()
        .map(|(i, it)| {
            json!({
                "id": format!("{:08x}", fid * 128 + i as u64 + 1),
                "title": s(it, "title"),
                "start_date": s(it, "startDate").get(..10).unwrap_or(s(it, "startDate")),
                "duration": u(it, "duration"),
            })
        })
        .collect();
    if iterations.is_empty() {
        // GitHub creates three iterations of `duration` days from `startDate`.
        let duration = u(cfg, "duration").max(1);
        let start = s(cfg, "startDate")
            .get(..10)
            .unwrap_or(s(cfg, "startDate"))
            .to_owned();
        iterations = (0..3u64)
            .map(|i| {
                let date = crate::util::add_secs(&start, (i * duration * 86400) as i64);
                json!({
                    "id": format!("{:08x}", fid * 128 + i + 1),
                    "title": format!("Iteration {}", i + 1),
                    "start_date": date.get(..10).unwrap_or(&date),
                    "duration": duration,
                })
            })
            .collect();
    }
    json!({
        "duration": cfg.get("duration").cloned().unwrap_or(json!(14)),
        "start_day": 1,
        "start_date": s(cfg, "startDate").get(..10),
        "iterations": iterations,
    })
}

/// Whether new-file line `line` (RIGHT) or old-file line (LEFT) appears in the patch hunks.
fn line_in_patch(patch: &str, line: u64, right: bool) -> bool {
    let (mut old, mut new) = (0u64, 0u64);
    for l in patch.lines() {
        if let Some(h) = l.strip_prefix("@@ ") {
            let mut parts = h.split_whitespace();
            let o_start = parts
                .next()
                .and_then(|p| p.trim_start_matches('-').split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let n_start = parts
                .next()
                .and_then(|p| p.trim_start_matches('+').split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            old = o_start;
            new = n_start;
            continue;
        }
        match l.chars().next() {
            Some('+') => {
                if right && new == line {
                    return true;
                }
                new += 1;
            }
            Some('-') => {
                if !right && old == line {
                    return true;
                }
                old += 1;
            }
            _ => {
                if (right && new == line) || (!right && old == line) {
                    return true;
                }
                old += 1;
                new += 1;
            }
        }
    }
    false
}

#[allow(dead_code)]
fn unused() -> Option<()> {
    let _ = pulls::reviews_of;
    None
}
