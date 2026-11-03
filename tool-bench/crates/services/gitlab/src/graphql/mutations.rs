//! GraphQL mutations. Each one updates the shared collections and returns its payload
//! object, which the executor then projects through the mutation's selection set.

use mock_core::{store::Store, world::WorldState};
use serde_json::{Map, Value, json};

use super::{
    Ctx, R,
    parser::Document,
    types::{self, gid_num, key_by_gid},
};
use crate::common::*;

fn ctx<'a>(w: &'a WorldState, base: &str, doc: &'a Document) -> Ctx<'a> {
    Ctx {
        st: store(w),
        base: base.to_owned(),
        vars: Map::new(),
        doc,
        today: w.clock.now_rfc3339()[..10].to_owned(),
    }
}

fn payload(name: &str, errors: Vec<String>, fields: Value) -> Value {
    let mut p = json!({"__typename": name, "errors": errors, "clientMutationId": null});
    merge_into(&mut p, fields);
    p
}

fn input(args: &Map<String, Value>) -> Map<String, Value> {
    args.get("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| args.clone())
}

fn s(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Label names for label GIDs within a project's visible labels.
fn label_names(st: &Store, project: &str, gids: &[String]) -> Result<Vec<String>, String> {
    let labels = crate::issues::project_labels(st, project, true);
    gids.iter()
        .map(|g| {
            let n = gid_num(g);
            labels
                .iter()
                .find(|(_, l)| as_u64(&l["id"]) == n)
                .map(|(_, l)| str_of(l, "name").to_owned())
                .ok_or_else(|| format!("The resource that you are attempting to access does not exist or you don't have permission to perform this action ({g})"))
        })
        .collect()
}

fn usernames(st: &Store, gids: &[String]) -> Vec<String> {
    gids.iter()
        .filter_map(|g| {
            gid_num(g).and_then(|n| user_by_id(st, &n.to_string()).map(|(k, _)| k.to_owned()))
        })
        .collect()
}

fn type_from_gid(gid: &str) -> Option<&'static str> {
    let n = gid_num(gid)?;
    types::WORK_ITEM_TYPES
        .iter()
        .find(|t| t.1 == n)
        .map(|t| t.3)
}

fn not_found_mutation() -> String {
    "The resource that you are attempting to access does not exist or you don't have permission to perform this action".into()
}

pub(crate) fn run(w: &mut WorldState, base: &str, name: &str, args: &Map<String, Value>) -> R {
    let doc = Document::default();
    match name {
        "workItemCreate" => work_item_create(w, base, &doc, &input(args)),
        "workItemUpdate" => work_item_update(w, base, &doc, &input(args)),
        "workItemConvert" => {
            let i = input(args);
            let key = key_by_gid(store(w), &s(&i, "id").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            let Some(t) = s(&i, "workItemTypeId").as_deref().and_then(type_from_gid) else {
                return Ok(payload(
                    "WorkItemConvertPayload",
                    vec!["Work item type not found".into()],
                    json!({"workItem": null}),
                ));
            };
            if t == "epic" {
                return Ok(payload(
                    "WorkItemConvertPayload",
                    vec!["Work item type cannot be changed to epic".into()],
                    json!({"workItem": null}),
                ));
            }
            let now = now(w);
            let st = svc_mut(w);
            let d = st.collection_mut("issues").get_mut(&key).expect("issue");
            d["issue_type"] = json!(t);
            d["updated_at"] = json!(now);
            let c = ctx(w, base, &doc);
            Ok(payload(
                "WorkItemConvertPayload",
                vec![],
                json!({"workItem": types::work_item_obj(&c, &key)}),
            ))
        }
        "workItemAddLinkedItems" => {
            let i = input(args);
            let key = key_by_gid(store(w), &s(&i, "id").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            let targets: Vec<String> = i.get("workItemsIds").map(value_list).unwrap_or_default();
            let link_type = match s(&i, "linkType").as_deref() {
                Some("BLOCKS") => "blocks",
                Some("BLOCKED_BY") => "is_blocked_by",
                _ => "relates_to",
            };
            let now = now(w);
            let mut linked = Vec::new();
            for t in &targets {
                let Some(tk) = key_by_gid(store(w), t) else {
                    return Ok(payload(
                        "WorkItemAddLinkedItemsPayload",
                        vec![format!("Work item {t} not found")],
                        json!({"workItem": null, "message": null}),
                    ));
                };
                let st = svc_mut(w);
                let exists = all(st, "issue_links").iter().any(|(_, l)| {
                    (str_of(l, "source") == key && str_of(l, "target") == tk)
                        || (str_of(l, "source") == tk && str_of(l, "target") == key)
                });
                if !exists && tk != key {
                    let id = next_id(st, "issue_links", 1);
                    st.collection_mut("issue_links").insert(
                        id.to_string(),
                        json!({"id": id, "source": key, "target": tk, "link_type": link_type, "created_at": now, "updated_at": now}),
                    );
                    linked.push(t.clone());
                }
            }
            let c = ctx(w, base, &doc);
            let msg = if linked.is_empty() {
                Value::Null
            } else {
                json!(format!(
                    "Successfully linked ID(s): {}.",
                    linked
                        .iter()
                        .filter_map(|g| gid_num(g))
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(" and ")
                ))
            };
            Ok(payload(
                "WorkItemAddLinkedItemsPayload",
                vec![],
                json!({"workItem": types::work_item_obj(&c, &key), "message": msg}),
            ))
        }
        "workItemRemoveLinkedItems" => {
            let i = input(args);
            let key = key_by_gid(store(w), &s(&i, "id").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            let targets: Vec<String> = i.get("workItemsIds").map(value_list).unwrap_or_default();
            let target_keys: Vec<String> = targets
                .iter()
                .filter_map(|t| key_by_gid(store(w), t))
                .collect();
            let st = svc_mut(w);
            let doomed: Vec<String> = all(st, "issue_links")
                .iter()
                .filter(|(_, l)| {
                    (str_of(l, "source") == key
                        && target_keys.iter().any(|t| t == str_of(l, "target")))
                        || (str_of(l, "target") == key
                            && target_keys.iter().any(|t| t == str_of(l, "source")))
                })
                .map(|(k, _)| (*k).to_owned())
                .collect();
            for d in &doomed {
                remove_doc(st, "issue_links", d);
            }
            let c = ctx(w, base, &doc);
            Ok(payload(
                "WorkItemRemoveLinkedItemsPayload",
                vec![],
                json!({"workItem": types::work_item_obj(&c, &key), "message": format!("Successfully unlinked IDs: {}.", targets.join(", "))}),
            ))
        }
        "issueMove" | "workItemMove" => issue_move(w, base, &doc, &input(args), name),
        "createNote" => create_note(w, base, &doc, &input(args)),
        "awardEmojiAdd" | "awardEmojiRemove" | "awardEmojiToggle" => {
            award(w, base, &doc, &input(args), name)
        }
        "timelineEventCreate" => {
            let i = input(args);
            let key = key_by_gid(store(w), &s(&i, "incidentId").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            if get(store(w), "issues", &key)
                .and_then(|d| d.get("issue_type")?.as_str().map(str::to_owned))
                .as_deref()
                != Some("incident")
            {
                return Ok(payload(
                    "TimelineEventCreatePayload",
                    vec!["Timeline events can only be added to incidents".into()],
                    json!({"timelineEvent": null}),
                ));
            }
            let Some(note) = s(&i, "note").filter(|n| !n.trim().is_empty()) else {
                return Ok(payload(
                    "TimelineEventCreatePayload",
                    vec!["Timeline text can't be blank".into()],
                    json!({"timelineEvent": null}),
                ));
            };
            let now = now(w);
            let st = svc_mut(w);
            let id = next_id(st, "timeline_events", 1);
            let doc_ev = json!({
                "id": id,
                "incident": key,
                "note": note,
                "occurred_at": s(&i, "occurredAt").unwrap_or_else(|| now.clone()),
                "created_at": now,
                "updated_at": now,
                "action": "comment",
                "author": current_username(st),
                "tags": i.get("timelineEventTagNames").map(value_list).unwrap_or_default(),
            });
            st.collection_mut("timeline_events")
                .insert(id.to_string(), doc_ev.clone());
            Ok(payload(
                "TimelineEventCreatePayload",
                vec![],
                json!({"timelineEvent": types::timeline_obj(&doc_ev)}),
            ))
        }
        "issueSetSeverity" | "issueSetEscalationStatus" => {
            let i = input(args);
            let project = project_key(store(w), &s(&i, "projectPath").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            let key = format!("{project}#{}", s(&i, "iid").unwrap_or_default());
            if get(store(w), "issues", &key).is_none() {
                return Err(not_found_mutation());
            }
            let now = now(w);
            let st = svc_mut(w);
            let d = st.collection_mut("issues").get_mut(&key).expect("issue");
            if name == "issueSetSeverity" {
                d["severity"] = json!(s(&i, "severity").unwrap_or_default().to_uppercase());
            } else {
                d["escalation_status"] = json!(s(&i, "status").unwrap_or_default().to_uppercase());
            }
            d["updated_at"] = json!(now);
            let c = ctx(w, base, &doc);
            Ok(payload(
                &format!("{}{}Payload", name[..1].to_uppercase(), &name[1..]),
                vec![],
                json!({"issue": types::issue_obj(&c, &key)}),
            ))
        }
        "iterationCreate" | "createIteration" => iteration_create(w, base, &doc, &input(args)),
        "updateIteration" => iteration_update(w, base, &doc, &input(args)),
        "iterationDelete" => {
            let i = input(args);
            let id = gid_num(&s(&i, "id").unwrap_or_default())
                .map(|n| n.to_string())
                .unwrap_or_default();
            let Some(it) = get(store(w), "iterations", &id).cloned() else {
                return Err(not_found_mutation());
            };
            remove_doc(svc_mut(w), "iterations", &id);
            let c = ctx(w, base, &doc);
            Ok(payload(
                "IterationDeletePayload",
                vec![],
                json!({"group": types::group_obj(&c, str_of(&it, "group"))}),
            ))
        }
        "iterationCadenceCreate" | "iterationCadenceUpdate" => {
            cadence_upsert(w, &input(args), name)
        }
        "iterationCadenceDestroy" => {
            let i = input(args);
            let id = gid_num(&s(&i, "id").unwrap_or_default())
                .map(|n| n.to_string())
                .unwrap_or_default();
            let Some(cad) = get(store(w), "iteration_cadences", &id).cloned() else {
                return Err(not_found_mutation());
            };
            let st = svc_mut(w);
            remove_doc(st, "iteration_cadences", &id);
            let doomed: Vec<String> = all(st, "iterations")
                .iter()
                .filter(|(_, it)| id_is(&it["cadence_id"], &id))
                .map(|(k, _)| (*k).to_owned())
                .collect();
            for d in doomed {
                remove_doc(st, "iterations", &d);
            }
            let c = ctx(w, base, &doc);
            Ok(payload(
                "IterationCadenceDestroyPayload",
                vec![],
                json!({"group": types::group_obj(&c, str_of(&cad, "group"))}),
            ))
        }
        "updateDependencyProxySettings" | "updateDependencyProxyImageTtlGroupPolicy" => {
            let i = input(args);
            let g = group_key(store(w), &s(&i, "groupPath").unwrap_or_default())
                .ok_or_else(not_found_mutation)?;
            let st = svc_mut(w);
            let mut cur = get(st, "dependency_proxy", &g)
                .cloned()
                .unwrap_or(json!({"enabled": true}));
            if name == "updateDependencyProxySettings" {
                if let Some(e) = i.get("enabled").and_then(Value::as_bool) {
                    cur["enabled"] = json!(e);
                }
                if let Some(id) = s(&i, "identity") {
                    cur["identity"] = json!(id);
                }
                if i.contains_key("secret") {
                    cur["secret_set"] = json!(true);
                }
            } else {
                if let Some(e) = i.get("enabled").and_then(Value::as_bool) {
                    cur["ttl_enabled"] = json!(e);
                }
                if let Some(t) = i.get("ttl").and_then(Value::as_u64) {
                    cur["ttl"] = json!(t);
                }
            }
            st.collection_mut("dependency_proxy").insert(g, cur.clone());
            Ok(payload(
                &format!("{}{}Payload", name[..1].to_uppercase(), &name[1..]),
                vec![],
                json!({"dependencyProxySetting": {"__typename": "DependencyProxySetting", "enabled": cur["enabled"]}, "dependencyProxyImageTtlPolicy": {"__typename": "DependencyProxyImageTtlGroupPolicy", "enabled": cur.get("ttl_enabled").cloned().unwrap_or(json!(false)), "ttl": cur.get("ttl").cloned().unwrap_or(json!(90))}}),
            ))
        }
        "vulnerabilityDismiss"
        | "vulnerabilityConfirm"
        | "vulnerabilityResolve"
        | "vulnerabilityRevertToDetected" => {
            let i = input(args);
            let id = gid_num(&s(&i, "id").unwrap_or_default())
                .map(|n| n.to_string())
                .unwrap_or_default();
            if get(store(w), "vulnerabilities", &id).is_none() {
                return Err(not_found_mutation());
            }
            let now = now(w);
            let st = svc_mut(w);
            let me = current_username(st);
            let v = st
                .collection_mut("vulnerabilities")
                .get_mut(&id)
                .expect("vulnerability");
            match name {
                "vulnerabilityDismiss" => {
                    v["state"] = json!("dismissed");
                    v["dismissed_at"] = json!(now);
                    v["dismissed_by"] = json!(me);
                    v["dismissal_reason"] = json!(
                        s(&i, "dismissalReason")
                            .unwrap_or_else(|| "NOT_APPLICABLE".into())
                            .to_lowercase()
                    );
                }
                "vulnerabilityConfirm" => {
                    v["state"] = json!("confirmed");
                    v["confirmed_at"] = json!(now);
                    v["confirmed_by"] = json!(me);
                }
                "vulnerabilityResolve" => {
                    v["state"] = json!("resolved");
                    v["resolved_at"] = json!(now);
                }
                _ => {
                    v["state"] = json!("detected");
                    v["dismissed_at"] = Value::Null;
                    v["confirmed_at"] = Value::Null;
                    v["resolved_at"] = Value::Null;
                }
            }
            if let Some(c) = s(&i, "comment") {
                v["state_comment"] = json!(c);
            }
            let v = v.clone();
            let c = ctx(w, base, &doc);
            Ok(payload(
                &format!("{}{}Payload", name[..1].to_uppercase(), &name[1..]),
                vec![],
                json!({"vulnerability": types::vulnerability_obj(&c, &v)}),
            ))
        }
        _ => Err(format!("Field '{name}' doesn't exist on type 'Mutation'")),
    }
}

fn work_item_create(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>) -> R {
    let st = store(w);
    let path = s(i, "namespacePath")
        .or_else(|| s(i, "projectPath"))
        .unwrap_or_default();
    let (scope, is_group) = match project_key(st, &path) {
        Some(p) => (p, false),
        None => match group_key(st, &path) {
            Some(g) => (g, true),
            None => return Err(not_found_mutation()),
        },
    };
    let issue_type = match s(i, "workItemTypeId").as_deref().and_then(type_from_gid) {
        Some(t) => t,
        None => {
            return Ok(payload(
                "WorkItemCreatePayload",
                vec!["Work item type can't be blank".into()],
                json!({"workItem": null}),
            ));
        }
    };
    if is_group && issue_type != "epic" {
        return Ok(payload(
            "WorkItemCreatePayload",
            vec![format!(
                "{} is not allowed in groups",
                types::type_by_issue_type(issue_type).0
            )],
            json!({"workItem": null}),
        ));
    }
    if !is_group && issue_type == "epic" {
        return Ok(payload(
            "WorkItemCreatePayload",
            vec!["Epics can only be created in groups".into()],
            json!({"workItem": null}),
        ));
    }
    let Some(title) = s(i, "title").filter(|t| !t.trim().is_empty()) else {
        return Ok(payload(
            "WorkItemCreatePayload",
            vec!["Title can't be blank".into()],
            json!({"workItem": null}),
        ));
    };
    let labels = match i
        .get("labelsWidget")
        .and_then(|l| l.get("labelIds"))
        .map(value_list)
    {
        Some(g) => match label_names(st, &scope, &g) {
            Ok(n) => n,
            Err(e) => {
                return Ok(payload(
                    "WorkItemCreatePayload",
                    vec![e],
                    json!({"workItem": null}),
                ));
            }
        },
        None => vec![],
    };
    let assignees = i
        .get("assigneesWidget")
        .and_then(|a| a.get("assigneeIds"))
        .map(|v| usernames(st, &value_list(v)))
        .unwrap_or_default();
    let parent = i
        .get("hierarchyWidget")
        .and_then(|h| h.get("parentId"))
        .and_then(Value::as_str)
        .and_then(|g| key_by_gid(st, g));
    let milestone = i
        .get("milestoneWidget")
        .and_then(|m| m.get("milestoneId"))
        .and_then(Value::as_str)
        .and_then(gid_num);
    if let Some(m) = milestone
        && crate::issues::milestone_for_project(st, &scope, &m.to_string()).is_none()
    {
        return Ok(payload(
            "WorkItemCreatePayload",
            vec!["Milestone not found".into()],
            json!({"workItem": null}),
        ));
    }
    let iteration = i
        .get("iterationWidget")
        .and_then(|m| m.get("iterationId"))
        .and_then(Value::as_str)
        .and_then(gid_num);
    let now = now(w);
    let st = svc_mut(w);
    let iid = next_iid(st, "issues", &scope, '#');
    let id = next_id(st, "issues", 1);
    let author = current_username(st);
    let key = format!("{scope}#{iid}");
    let mut d = json!({
        "id": id,
        "iid": iid,
        "title": title,
        "description": i.get("descriptionWidget").and_then(|d| d.get("description")).and_then(Value::as_str).unwrap_or(""),
        "state": "opened",
        "author": author,
        "assignees": assignees,
        "labels": labels,
        "milestone_id": milestone,
        "created_at": now,
        "updated_at": now,
        "closed_at": null,
        "confidential": i.get("confidential").and_then(Value::as_bool).unwrap_or(false),
        "issue_type": issue_type,
        "weight": i.get("weightWidget").and_then(|x| x.get("weight")).cloned().unwrap_or(Value::Null),
        "health_status": i.get("healthStatusWidget").and_then(|x| x.get("healthStatus")).cloned().unwrap_or(Value::Null),
        "start_date": i.get("startAndDueDateWidget").and_then(|x| x.get("startDate")).cloned().unwrap_or(Value::Null),
        "due_date": i.get("startAndDueDateWidget").and_then(|x| x.get("dueDate")).cloned().unwrap_or(Value::Null),
    });
    if let Some(p) = parent {
        d["parent"] = json!(p);
    }
    if let Some(it) = iteration {
        d["iteration_id"] = json!(it);
    }
    st.collection_mut("issues").insert(key.clone(), d);
    if !is_group {
        crate::notes::record_event(
            st, &scope, &author, "opened", "WorkItem", id, iid, &title, &now,
        );
    }
    let c = ctx(w, base, doc);
    Ok(payload(
        "WorkItemCreatePayload",
        vec![],
        json!({"workItem": types::work_item_obj(&c, &key)}),
    ))
}

fn work_item_update(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>) -> R {
    let st = store(w);
    let Some(key) = s(i, "id").and_then(|g| key_by_gid(st, &g)) else {
        return Err(not_found_mutation());
    };
    let scope = project_of(&key).to_owned();
    let lw = i.get("labelsWidget").cloned().unwrap_or(Value::Null);
    let resolve = |k: &str| -> Result<Option<Vec<String>>, String> {
        match lw.get(k) {
            Some(v) if !v.is_null() => label_names(st, &scope, &value_list(v)).map(Some),
            _ => Ok(None),
        }
    };
    let (add, remove, set) = match (
        resolve("addLabelIds"),
        resolve("removeLabelIds"),
        resolve("labelIds"),
    ) {
        (Ok(a), Ok(r), Ok(s)) => (a, r, s),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            return Ok(payload(
                "WorkItemUpdatePayload",
                vec![e],
                json!({"workItem": null}),
            ));
        }
    };
    let assignees = i
        .get("assigneesWidget")
        .and_then(|a| a.get("assigneeIds"))
        .map(|v| usernames(st, &value_list(v)));
    let hierarchy = i.get("hierarchyWidget").cloned();
    let parent: Option<Option<String>> =
        hierarchy
            .as_ref()
            .and_then(|h| h.as_object())
            .and_then(|h| {
                if h.contains_key("parentId") {
                    Some(
                        h.get("parentId")
                            .and_then(Value::as_str)
                            .and_then(|g| key_by_gid(st, g)),
                    )
                } else {
                    None
                }
            });
    let children: Option<Vec<String>> =
        hierarchy
            .as_ref()
            .and_then(|h| h.get("childrenIds"))
            .map(|c| {
                value_list(c)
                    .iter()
                    .filter_map(|g| key_by_gid(st, g))
                    .collect()
            });
    let milestone: Option<Value> = i
        .get("milestoneWidget")
        .and_then(|m| m.as_object())
        .map(|m| {
            m.get("milestoneId")
                .and_then(Value::as_str)
                .and_then(gid_num)
                .map(|n| json!(n))
                .unwrap_or(Value::Null)
        });
    if let Some(Value::Number(n)) = &milestone
        && crate::issues::milestone_for_project(st, &scope, &n.to_string()).is_none()
    {
        return Ok(payload(
            "WorkItemUpdatePayload",
            vec!["Milestone not found".into()],
            json!({"workItem": null}),
        ));
    }
    let iteration: Option<Value> = i
        .get("iterationWidget")
        .and_then(|m| m.as_object())
        .map(|m| {
            m.get("iterationId")
                .and_then(Value::as_str)
                .and_then(gid_num)
                .map(|n| json!(n))
                .unwrap_or(Value::Null)
        });
    let status_name: Option<String> = i
        .get("statusWidget")
        .and_then(|sw| sw.get("status"))
        .and_then(Value::as_str)
        .and_then(gid_num)
        .and_then(|n| types::STATUSES.iter().find(|s| s.0 == n))
        .map(|s| s.1.to_owned());
    if i.get("statusWidget").is_some() && status_name.is_none() {
        return Ok(payload(
            "WorkItemUpdatePayload",
            vec!["Status not found".into()],
            json!({"workItem": null}),
        ));
    }
    let custom: Option<Vec<Value>> = i
        .get("customFieldsWidget")
        .and_then(Value::as_array)
        .cloned();
    let now = now(w);
    let st = svc_mut(w);
    let me = current_username(st);
    let mut system: Vec<String> = Vec::new();
    {
        let d = st.collection_mut("issues").get_mut(&key).expect("issue");
        if let Some(t) = s(i, "title").filter(|t| !t.trim().is_empty()) {
            if t != str_of(d, "title") {
                system.push(format!(
                    "changed title from **{}** to **{t}**",
                    str_of(d, "title")
                ));
            }
            d["title"] = json!(t);
        }
        if let Some(desc) = i
            .get("descriptionWidget")
            .and_then(|x| x.get("description"))
            .and_then(Value::as_str)
        {
            d["description"] = json!(desc);
            system.push("changed the description".into());
        }
        let mut labels = value_list(&d["labels"]);
        if let Some(set) = set {
            labels = set;
        }
        for l in add.unwrap_or_default() {
            if !labels.contains(&l) {
                system.push(format!("added ~\"{l}\" label"));
                labels.push(l);
            }
        }
        for l in remove.unwrap_or_default() {
            if labels.contains(&l) {
                system.push(format!("removed ~\"{l}\" label"));
            }
            labels.retain(|x| *x != l);
        }
        d["labels"] = json!(labels);
        if let Some(a) = assignees {
            system.push(if a.is_empty() {
                "unassigned".into()
            } else {
                format!(
                    "assigned to {}",
                    a.iter()
                        .map(|u| format!("@{u}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
            d["assignees"] = json!(a);
        }
        if let Some(wt) = i.get("weightWidget").and_then(|x| x.get("weight")) {
            d["weight"] = wt.clone();
        }
        if let Some(h) = i
            .get("healthStatusWidget")
            .and_then(|x| x.get("healthStatus"))
        {
            d["health_status"] = h.clone();
        }
        if let Some(dates) = i.get("startAndDueDateWidget").and_then(Value::as_object) {
            if let Some(sd) = dates.get("startDate") {
                d["start_date"] = sd.clone();
            }
            if let Some(dd) = dates.get("dueDate") {
                d["due_date"] = dd.clone();
            }
        }
        if let Some(m) = milestone {
            system.push(if m.is_null() {
                "removed milestone".into()
            } else {
                "changed milestone".into()
            });
            d["milestone_id"] = m;
        }
        if let Some(it) = iteration {
            d["iteration_id"] = it;
        }
        if let Some(c) = i.get("confidential").and_then(Value::as_bool) {
            d["confidential"] = json!(c);
        }
        if let Some(p) = parent {
            d["parent"] = p.map(Value::String).unwrap_or(Value::Null);
        }
        if let Some(cf) = custom {
            let mut values = d.get("custom_fields").cloned().unwrap_or(json!({}));
            for v in cf {
                let Some(id) = v
                    .get("customFieldId")
                    .and_then(Value::as_str)
                    .and_then(gid_num)
                else {
                    continue;
                };
                let val = v
                    .get("textValue")
                    .or(v.get("numberValue"))
                    .or(v.get("dateValue"))
                    .cloned()
                    .or_else(|| {
                        v.get("selectedOptionIds").map(|ids| {
                            json!(
                                value_list(ids)
                                    .iter()
                                    .filter_map(|g| gid_num(g))
                                    .map(|n| n.to_string())
                                    .collect::<Vec<_>>()
                            )
                        })
                    })
                    .unwrap_or(Value::Null);
                values[id.to_string()] = val;
            }
            d["custom_fields"] = values;
        }
        let event = s(i, "stateEvent");
        let mut close = event.as_deref() == Some("CLOSE");
        let reopen = event.as_deref() == Some("REOPEN");
        if let Some(name) = &status_name {
            d["status"] = json!(name);
            if ["Done", "Won't do", "Duplicate"].contains(&name.as_str()) {
                close = true;
            } else if str_of(d, "state") == "closed" {
                d["state"] = json!("opened");
                d["closed_at"] = Value::Null;
            }
        }
        if close && str_of(d, "state") != "closed" {
            d["state"] = json!("closed");
            d["closed_at"] = json!(now);
            d["closed_by"] = json!(me);
            system.push("closed".into());
        }
        if reopen && str_of(d, "state") == "closed" {
            d["state"] = json!("opened");
            d["closed_at"] = Value::Null;
            d["closed_by"] = Value::Null;
            if d.get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| s == "Done")
            {
                d["status"] = json!("To do");
            }
            system.push("reopened".into());
        }
        d["updated_at"] = json!(now);
    }
    if let Some(children) = children {
        for c in children {
            if let Some(cd) = st.collection_mut("issues").get_mut(&c) {
                cd["parent"] = json!(key);
            }
        }
    }
    for body in system {
        crate::notes::add_system_note(st, &key, &me, &body, &now);
    }
    let c = ctx(w, base, doc);
    Ok(payload(
        "WorkItemUpdatePayload",
        vec![],
        json!({"workItem": types::work_item_obj(&c, &key)}),
    ))
}

fn issue_move(
    w: &mut WorldState,
    base: &str,
    doc: &Document,
    i: &Map<String, Value>,
    name: &str,
) -> R {
    let st = store(w);
    let (key, target) = if name == "issueMove" {
        let project = project_key(st, &s(i, "projectPath").unwrap_or_default())
            .ok_or_else(not_found_mutation)?;
        (
            format!("{project}#{}", s(i, "iid").unwrap_or_default()),
            s(i, "targetProjectPath").unwrap_or_default(),
        )
    } else {
        (
            key_by_gid(st, &s(i, "id").unwrap_or_default()).ok_or_else(not_found_mutation)?,
            s(i, "targetNamespacePath")
                .or_else(|| s(i, "targetFullPath"))
                .unwrap_or_default(),
        )
    };
    let Some(src) = get(st, "issues", &key).cloned() else {
        return Err(not_found_mutation());
    };
    let Some(target) = project_key(st, &target) else {
        return Ok(payload(
            "IssueMovePayload",
            vec!["Cannot move issue due to insufficient permissions!".into()],
            json!({"issue": null, "workItem": null}),
        ));
    };
    if target == project_of(&key) {
        return Ok(payload(
            "IssueMovePayload",
            vec!["Cannot move issue to project it originates from!".into()],
            json!({"issue": null, "workItem": null}),
        ));
    }
    let now = now(w);
    let st = svc_mut(w);
    let iid = next_iid(st, "issues", &target, '#');
    let id = next_id(st, "issues", 1);
    let mut moved = src.clone();
    moved["id"] = json!(id);
    moved["iid"] = json!(iid);
    moved["labels"] = json!(
        crate::issues::project_labels(st, &target, true)
            .iter()
            .filter(|(_, l)| value_list(&src["labels"]).contains(&str_of(l, "name").to_owned()))
            .map(|(_, l)| str_of(l, "name").to_owned())
            .collect::<Vec<_>>()
    );
    moved["milestone_id"] = Value::Null;
    moved["created_at"] = json!(now);
    moved["updated_at"] = json!(now);
    let new_key = format!("{target}#{iid}");
    st.collection_mut("issues").insert(new_key.clone(), moved);
    let me = current_username(st);
    let old = st.collection_mut("issues").get_mut(&key).expect("issue");
    old["state"] = json!("closed");
    old["closed_at"] = json!(now);
    old["closed_by"] = json!(me);
    old["moved_to_id"] = json!(id);
    old["updated_at"] = json!(now);
    crate::notes::add_system_note(st, &key, &me, &format!("moved to {new_key}"), &now);
    crate::notes::add_system_note(st, &new_key, &me, &format!("moved from {key}"), &now);
    let c = ctx(w, base, doc);
    Ok(payload(
        if name == "issueMove" {
            "IssueMovePayload"
        } else {
            "WorkItemMovePayload"
        },
        vec![],
        json!({"issue": types::issue_obj(&c, &new_key), "workItem": types::work_item_obj(&c, &new_key)}),
    ))
}

fn noteable_key(st: &Store, gid: &str) -> Option<String> {
    if gid.contains("/MergeRequest/") {
        let n = gid_num(gid);
        return all(st, "merge_requests")
            .into_iter()
            .find(|(_, m)| as_u64(&m["id"]) == n)
            .map(|(k, _)| k.to_owned());
    }
    key_by_gid(st, gid)
}

fn create_note(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>) -> R {
    let st = store(w);
    let Some(key) = s(i, "noteableId").and_then(|g| noteable_key(st, &g)) else {
        return Err(not_found_mutation());
    };
    let Some(body) = s(i, "body").filter(|b| !b.trim().is_empty()) else {
        return Ok(payload(
            "CreateNotePayload",
            vec!["Note can't be blank".into()],
            json!({"note": null}),
        ));
    };
    let discussion = s(i, "discussionId").map(|d| d.rsplit('/').next().unwrap_or(&d).to_owned());
    if let Some(d) = &discussion
        && !all(st, "notes")
            .iter()
            .any(|(_, n)| str_of(n, "noteable") == key && str_of(n, "discussion_id") == d)
    {
        return Ok(payload(
            "CreateNotePayload",
            vec!["Discussion not found".into()],
            json!({"note": null}),
        ));
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "notes", 1);
    let me = current_username(st);
    let is_mr = key.contains('!');
    let (did, ty) = match &discussion {
        Some(d) => (d.clone(), json!("DiscussionNote")),
        None => (fake_sha(&format!("discussion:{id}")), Value::Null),
    };
    let n = json!({
        "id": id,
        "noteable": key,
        "body": body,
        "author": me,
        "created_at": now,
        "updated_at": now,
        "system": false,
        "internal": i.get("internal").and_then(Value::as_bool).unwrap_or(false),
        "discussion_id": did,
        "type": ty,
        "resolvable": is_mr && !ty.is_null(),
        "resolved": false,
    });
    st.collection_mut("notes").insert(id.to_string(), n.clone());
    let coll = if is_mr { "merge_requests" } else { "issues" };
    if let Some(d) = st.collection_mut(coll).get_mut(&key) {
        d["updated_at"] = json!(now);
    }
    let project = project_of(&key).to_owned();
    crate::notes::record_event(
        st,
        &project,
        &me,
        "commented on",
        "Note",
        id,
        iid_of(&key),
        "",
        &now,
    );
    let c = ctx(w, base, doc);
    Ok(payload(
        "CreateNotePayload",
        vec![],
        json!({"note": types::note_obj_pub(&c, &n)}),
    ))
}

fn award(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>, name: &str) -> R {
    let st = store(w);
    let gid = s(i, "awardableId").unwrap_or_default();
    let target = if gid.contains("Note/") {
        gid_num(&gid)
            .filter(|n| get(st, "notes", &n.to_string()).is_some())
            .map(|n| format!("note:{n}"))
    } else {
        noteable_key(st, &gid)
    };
    let Some(target) = target else {
        return Err(not_found_mutation());
    };
    let Some(emoji) = s(i, "name").map(|n| n.trim_matches(':').to_owned()) else {
        return Ok(payload(
            "AwardEmojiAddPayload",
            vec!["Name can't be blank".into()],
            json!({"awardEmoji": null}),
        ));
    };
    let me = current_username(st);
    let existing = all(st, "award_emoji")
        .into_iter()
        .find(|(_, a)| {
            str_of(a, "awardable") == target
                && str_of(a, "name") == emoji
                && str_of(a, "user") == me
        })
        .map(|(k, a)| (k.to_owned(), a.clone()));
    let now = now(w);
    let c_name = match name {
        "awardEmojiAdd" => "AwardEmojiAddPayload",
        "awardEmojiRemove" => "AwardEmojiRemovePayload",
        _ => "AwardEmojiTogglePayload",
    };
    let remove = name == "awardEmojiRemove" || (name == "awardEmojiToggle" && existing.is_some());
    if remove {
        let Some((k, a)) = existing else {
            return Ok(payload(
                c_name,
                vec![format!(
                    "User has not awarded emoji of type {emoji} on the awardable"
                )],
                json!({"awardEmoji": null, "toggledOn": false}),
            ));
        };
        remove_doc(svc_mut(w), "award_emoji", &k);
        let c = ctx(w, base, doc);
        let mut o = json!({"__typename": "AwardEmoji", "name": str_of(&a, "name"), "description": str_of(&a, "name"), "emoji": "", "unicode": "", "unicodeVersion": "6.0"});
        o["user"] = types::user_obj(&c, &me);
        return Ok(payload(
            c_name,
            vec![],
            json!({"awardEmoji": o, "toggledOn": false}),
        ));
    }
    if existing.is_some() {
        return Ok(payload(
            c_name,
            vec!["Name has already been taken".into()],
            json!({"awardEmoji": null}),
        ));
    }
    let st = svc_mut(w);
    let id = next_id(st, "award_emoji", 1);
    st.collection_mut("award_emoji").insert(
        id.to_string(),
        json!({"id": id, "name": emoji, "user": me, "awardable": target, "created_at": now, "updated_at": now}),
    );
    let c = ctx(w, base, doc);
    let o = json!({"__typename": "AwardEmoji", "name": emoji, "description": emoji.replace('_', " "), "emoji": "", "unicode": "", "unicodeVersion": "6.0", "user": types::user_obj(&c, &me)});
    Ok(payload(
        c_name,
        vec![],
        json!({"awardEmoji": o, "toggledOn": true}),
    ))
}

fn iteration_create(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>) -> R {
    let st = store(w);
    let g = group_key(st, &s(i, "groupPath").unwrap_or_default()).ok_or_else(not_found_mutation)?;
    let cadence = s(i, "iterationsCadenceId").and_then(|c| gid_num(&c));
    if let Some(c) = cadence
        && get(st, "iteration_cadences", &c.to_string()).is_none()
    {
        return Ok(payload(
            "IterationCreatePayload",
            vec!["Iterations cadence not found".into()],
            json!({"iteration": null}),
        ));
    }
    let start = s(i, "startDate");
    let due = s(i, "dueDate");
    let mut errors = Vec::new();
    if start.is_none() {
        errors.push("Start date can't be blank".to_owned());
    }
    if due.is_none() {
        errors.push("Due date can't be blank".to_owned());
    }
    if let (Some(a), Some(b)) = (&start, &due)
        && b < a
    {
        errors.push("Due date must be greater than start date".into());
    }
    if !errors.is_empty() {
        return Ok(payload(
            "IterationCreatePayload",
            errors,
            json!({"iteration": null}),
        ));
    }
    let now = now(w);
    let st = svc_mut(w);
    let id = next_id(st, "iterations", 1);
    let iid = all(st, "iterations")
        .iter()
        .filter(|(_, it)| str_of(it, "group") == g)
        .filter_map(|(_, it)| as_u64(&it["iid"]))
        .max()
        .unwrap_or(0)
        + 1;
    let it = json!({
        "id": id,
        "iid": iid,
        "sequence": iid,
        "group": g,
        "title": s(i, "title"),
        "description": s(i, "description"),
        "start_date": start,
        "due_date": due,
        "cadence_id": cadence,
        "created_at": now,
        "updated_at": now,
    });
    st.collection_mut("iterations")
        .insert(id.to_string(), it.clone());
    let c = ctx(w, base, doc);
    Ok(payload(
        "IterationCreatePayload",
        vec![],
        json!({"iteration": types::iteration_obj(&c, &it)}),
    ))
}

fn iteration_update(w: &mut WorldState, base: &str, doc: &Document, i: &Map<String, Value>) -> R {
    let id = gid_num(&s(i, "id").unwrap_or_default())
        .map(|n| n.to_string())
        .unwrap_or_default();
    if get(store(w), "iterations", &id).is_none() {
        return Err(not_found_mutation());
    }
    let now = now(w);
    let st = svc_mut(w);
    let it = st
        .collection_mut("iterations")
        .get_mut(&id)
        .expect("iteration");
    for (from, to) in [
        ("title", "title"),
        ("description", "description"),
        ("startDate", "start_date"),
        ("dueDate", "due_date"),
    ] {
        if let Some(v) = i.get(from) {
            it[to] = v.clone();
        }
    }
    it["updated_at"] = json!(now);
    let it = it.clone();
    let c = ctx(w, base, doc);
    Ok(payload(
        "UpdateIterationPayload",
        vec![],
        json!({"iteration": types::iteration_obj(&c, &it)}),
    ))
}

fn cadence_upsert(w: &mut WorldState, i: &Map<String, Value>, name: &str) -> R {
    let st = store(w);
    let create = name == "iterationCadenceCreate";
    let id = if create {
        let g =
            group_key(st, &s(i, "groupPath").unwrap_or_default()).ok_or_else(not_found_mutation)?;
        let mut errors = Vec::new();
        if s(i, "title").is_none() {
            errors.push("Title can't be blank".to_owned());
        }
        if i.get("automatic").is_none() {
            errors.push("Automatic can't be blank".to_owned());
        }
        if i.get("active").is_none() {
            errors.push("Active can't be blank".to_owned());
        }
        if i.get("automatic").and_then(Value::as_bool).unwrap_or(false)
            && (s(i, "startDate").is_none() || i.get("durationInWeeks").is_none())
        {
            errors.push("Start date can't be blank".to_owned());
        }
        if !errors.is_empty() {
            return Ok(payload(
                "IterationCadenceCreatePayload",
                errors,
                json!({"iterationCadence": null}),
            ));
        }
        let st = svc_mut(w);
        let id = next_id(st, "iteration_cadences", 1);
        st.collection_mut("iteration_cadences")
            .insert(id.to_string(), json!({"id": id, "group": g}));
        id.to_string()
    } else {
        let id = gid_num(&s(i, "id").unwrap_or_default())
            .map(|n| n.to_string())
            .unwrap_or_default();
        if get(st, "iteration_cadences", &id).is_none() {
            return Err(not_found_mutation());
        }
        id
    };
    let st = svc_mut(w);
    let c = st
        .collection_mut("iteration_cadences")
        .get_mut(&id)
        .expect("cadence");
    for (from, to) in [
        ("title", "title"),
        ("description", "description"),
        ("automatic", "automatic"),
        ("active", "active"),
        ("durationInWeeks", "duration_in_weeks"),
        ("iterationsInAdvance", "iterations_in_advance"),
        ("startDate", "start_date"),
        ("rollOver", "roll_over"),
    ] {
        if let Some(v) = i.get(from) {
            c[to] = v.clone();
        }
    }
    let c = c.clone();
    Ok(payload(
        if create {
            "IterationCadenceCreatePayload"
        } else {
            "IterationCadenceUpdatePayload"
        },
        vec![],
        json!({"iterationCadence": types::cadence_obj(&c)}),
    ))
}
