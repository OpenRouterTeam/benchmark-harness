//! Atlas Stream Processing tools (`atlas-streams-*`) over the dataset's workspaces,
//! connections and processors.

use serde_json::{Map, Value, json};

use crate::{Ctx, error_running, fail, ok, str_arg, text};

const WS: &str = "stream_workspaces";
const CONN: &str = "stream_connections";
const PROC: &str = "stream_processors";

fn s<'a>(args: &'a Value, k: &str) -> &'a str {
    str_arg(args, k).unwrap_or_default()
}

fn not_found(detail: String) -> Value {
    fail(format!(
        "Resource not found: [404 Not Found] error calling Atlas API: Not Found; {detail}\n\nUse `atlas-streams-discover` to list available workspaces, connections, and processors."
    ))
}

fn bad_request(msg: &str, hint: &str) -> Value {
    fail(format!("Bad Request: {msg}\n\n{hint}"))
}

fn project_exists(ctx: &Ctx, pid: &str) -> bool {
    ctx.coll("projects").is_some_and(|c| c.contains(pid))
}

fn in_ws(ctx: &Ctx, coll: &str, pid: &str, ws: &str) -> Vec<Value> {
    let prefix = format!("{pid}/{ws}/");
    ctx.coll(coll)
        .map(|c| {
            c.iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| v.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn region_of(w: &Value) -> String {
    match (
        w["dataProcessRegion"]["cloudProvider"].as_str(),
        w["dataProcessRegion"]["region"].as_str(),
    ) {
        (Some(p), Some(r)) => format!("{p}/{r}"),
        _ => "unknown".to_owned(),
    }
}

fn concise_ws(w: &Value) -> Value {
    json!({
        "name": w["name"],
        "region": region_of(w),
        "tier": w["streamConfig"]["tier"].as_str().unwrap_or("unknown"),
        "maxTier": w["streamConfig"]["maxTierSize"].as_str().unwrap_or("unknown"),
    })
}

pub fn call(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    let pid = s(args, "projectId").to_owned();
    if !project_exists(ctx, &pid) {
        return not_found(format!("No group with ID {pid} exists."));
    }
    match name {
        "atlas-streams-discover" => discover(ctx, &pid, args),
        "atlas-streams-build" => build(ctx, &pid, args),
        "atlas-streams-manage" => manage(ctx, &pid, args),
        "atlas-streams-teardown" => teardown(ctx, &pid, args),
        _ => error_running(name, "not implemented"),
    }
}

fn discover(ctx: &mut Ctx, pid: &str, args: &Value) -> Value {
    let action = s(args, "action");
    let ws = s(args, "workspaceName").to_owned();
    let res = s(args, "resourceName").to_owned();
    let detailed = |default_concise: bool| match str_arg(args, "responseFormat") {
        Some("concise") => false,
        Some("detailed") => true,
        _ => !default_concise,
    };
    if !matches!(action, "list-workspaces" | "get-networking") && ws.is_empty() {
        return fail("workspaceName is required for this action.".to_owned());
    }
    let ws_key = format!("{pid}/{ws}");
    let ws_doc = ctx.coll(WS).and_then(|c| c.get(&ws_key)).cloned();
    if !matches!(action, "list-workspaces" | "get-networking") && ws_doc.is_none() {
        return not_found(format!(
            "Stream Processing workspace {ws} not found in group {pid}."
        ));
    }
    match action {
        "list-workspaces" => {
            let all: Vec<Value> = ctx
                .docs(WS)
                .into_iter()
                .filter(|w| w["groupId"] == pid)
                .collect();
            if all.is_empty() {
                return ok(
                    vec![text(
                        "No Stream Processing workspaces found in this project. Use `atlas-streams-build` with resource='workspace' to create one.",
                    )],
                    Some(json!({ "workspaces": [] })),
                );
            }
            let concise: Vec<Value> = all.iter().map(concise_ws).collect();
            let shown = if detailed(true) {
                all.clone()
            } else {
                concise.clone()
            };
            let content = ctx.untrusted(
                &format!("Found {} workspace(s) (total: {}):", all.len(), all.len()),
                vec![serde_json::to_string_pretty(&shown).expect("serializes")],
            );
            ok(content, Some(json!({ "workspaces": concise })))
        }
        "inspect-workspace" => {
            let w = ws_doc.expect("checked");
            let conns = in_ws(ctx, CONN, pid, &ws);
            let mut concise = concise_ws(&w);
            concise["connectionCount"] = json!(conns.len());
            let mut full = w.clone();
            full["connections"] = Value::Array(conns.iter().map(redact).collect());
            let shown = if detailed(false) {
                full
            } else {
                concise.clone()
            };
            let content = ctx.untrusted(
                "Details for the requested workspace:",
                vec![serde_json::to_string_pretty(&shown).expect("serializes")],
            );
            ok(content, Some(json!({ "workspace": concise })))
        }
        "list-connections" => {
            let conns = in_ws(ctx, CONN, pid, &ws);
            if conns.is_empty() {
                return ok(
                    vec![text(format!(
                        "No connections found in workspace '{ws}'. Use `atlas-streams-build` with resource='connection' to add one."
                    ))],
                    Some(json!({ "connections": [] })),
                );
            }
            let concise: Vec<Value> = conns
                .iter()
                .map(|c| json!({ "name": c["name"], "type": c["type"], "state": c["state"] }))
                .collect();
            let shown: Vec<Value> = if detailed(true) {
                conns.iter().map(redact).collect()
            } else {
                concise.clone()
            };
            let content = ctx.untrusted(
                &format!(
                    "Found {} connection(s) in the requested workspace:",
                    conns.len()
                ),
                vec![serde_json::to_string_pretty(&shown).expect("serializes")],
            );
            ok(content, Some(json!({ "connections": concise })))
        }
        "inspect-connection" => {
            let Some(c) = ctx
                .coll(CONN)
                .and_then(|c| c.get(&format!("{ws_key}/{res}")))
                .cloned()
            else {
                return not_found(format!(
                    "Stream connection {res} not found in workspace {ws}."
                ));
            };
            let mut header = "Details for the requested connection:".to_owned();
            if c["type"] == "Cluster"
                && c["clusterName"].is_string()
                && c["clusterName"] != c["name"]
            {
                header.push_str("\n\nNote: This connection's name differs from its target cluster name. Use the connection name (not the cluster name) when referencing it in pipeline stages.");
            }
            let content = ctx.untrusted(
                &header,
                vec![serde_json::to_string_pretty(&redact(&c)).expect("serializes")],
            );
            ok(
                content,
                Some(json!({ "connection": { "type": c["type"], "state": c["state"] } })),
            )
        }
        "list-processors" => {
            let procs = in_ws(ctx, PROC, pid, &ws);
            if procs.is_empty() {
                return ok(
                    vec![text(format!(
                        "No processors found in workspace '{ws}'. Use `atlas-streams-build` with resource='processor' to deploy one."
                    ))],
                    Some(json!({ "processors": [] })),
                );
            }
            let concise: Vec<Value> = procs.iter().map(|p| json!({ "name": p["name"], "state": p["state"], "tier": p["tier"], "effectiveTier": p["effectiveTier"] })).collect();
            let shown = if detailed(true) {
                procs.clone()
            } else {
                concise.clone()
            };
            let content = ctx.untrusted(
                &format!(
                    "Found {} processor(s) in the requested workspace:",
                    procs.len()
                ),
                vec![serde_json::to_string_pretty(&shown).expect("serializes")],
            );
            ok(content, Some(json!({ "processors": concise })))
        }
        "inspect-processor" | "diagnose-processor" => {
            let Some(p) = ctx
                .coll(PROC)
                .and_then(|c| c.get(&format!("{ws_key}/{res}")))
                .cloned()
            else {
                return not_found(format!(
                    "Stream processor {res} not found in workspace {ws}."
                ));
            };
            let mut structured = json!({ "processorState": p["state"], "tier": p["tier"], "effectiveTier": p["effectiveTier"] });
            if action == "inspect-processor" {
                structured["pipeline"] = p["pipeline"].clone();
            }
            if action == "inspect-processor" {
                let content = ctx.untrusted(
                    "Details for the requested processor:",
                    vec![serde_json::to_string_pretty(&p).expect("serializes")],
                );
                return ok(content, Some(structured));
            }
            let stats = &p["stats"];
            let mut sections = vec![
                format!(
                    "## Processor State\nState: {}\nTier: {}",
                    p["state"].as_str().unwrap_or("UNKNOWN"),
                    p["tier"].as_str().unwrap_or("unknown")
                ),
                format!(
                    "## Stats\n{}",
                    serde_json::to_string_pretty(stats).expect("serializes")
                ),
            ];
            let errors = p["recentErrors"].as_array().cloned().unwrap_or_default();
            sections.push(format!(
                "## Recent Errors\n{}",
                if errors.is_empty() {
                    "No recent errors.".to_owned()
                } else {
                    serde_json::to_string_pretty(&errors).expect("serializes")
                }
            ));
            let content = ctx.untrusted(
                "Diagnostic report for the requested processor:",
                vec![sections.join("\n\n")],
            );
            ok(content, Some(structured))
        }
        "get-networking" => {
            let links: Vec<Value> = ctx
                .docs("stream_privatelinks")
                .into_iter()
                .filter(|l| l["groupId"] == pid)
                .collect();
            let sections = [
                format!(
                    "## PrivateLink Connections\n{}",
                    if links.is_empty() {
                        "No PrivateLink connections found.".to_owned()
                    } else {
                        serde_json::to_string_pretty(&links).expect("serializes")
                    }
                ),
                format!(
                    "## Account Details\n{}",
                    str_arg(args, "cloudProvider").map_or(
                        "Provide cloudProvider and region to look up account details.".to_owned(),
                        |p| format!("{p} account details are not available for this project.")
                    )
                ),
            ];
            let content = ctx.untrusted("Streams networking details:", vec![sections.join("\n\n")]);
            ok(content, None)
        }
        other => fail(format!("Unknown action: {other}")),
    }
}

/// Masks secret-valued keys (passwords, secrets, tokens).
fn redact(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let lower = k.to_lowercase();
                    if lower.contains("password")
                        || lower.contains("secret")
                        || lower.contains("token")
                    {
                        (k.clone(), json!("[REDACTED]"))
                    } else {
                        (k.clone(), redact(val))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

fn build(ctx: &mut Ctx, pid: &str, args: &Value) -> Value {
    let resource = s(args, "resource");
    let ws = s(args, "workspaceName").to_owned();
    let ws_key = format!("{pid}/{ws}");
    match resource {
        "workspace" => {
            if ws.is_empty() || args.get("cloudProvider").is_none() || args.get("region").is_none()
            {
                return fail("workspaceName, cloudProvider and region are required when resource='workspace'.".to_owned());
            }
            if ctx.coll(WS).is_some_and(|c| c.contains(&ws_key)) {
                return bad_request(
                    &format!("A Stream Processing workspace named {ws} already exists."),
                    "Use a different workspace name.",
                );
            }
            let tier = str_arg(args, "tier").unwrap_or("SP10");
            let id = ctx.object_id("stream-workspace");
            ctx.coll_mut(WS).insert(
                ws_key.clone(),
                json!({ "_id": id, "groupId": pid, "name": ws, "dataProcessRegion": { "cloudProvider": s(args, "cloudProvider"), "region": s(args, "region") }, "streamConfig": { "tier": tier, "maxTierSize": "SP50" }, "hostnames": [format!("atlas-stream-{id}.virginia-usa.a.query.mongodb.net")] }),
            );
            let sample = args
                .get("includeSampleData")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if sample {
                ctx.coll_mut(CONN).insert(
                    format!("{ws_key}/sample_stream_solar"),
                    json!({ "name": "sample_stream_solar", "type": "Sample", "state": "READY" }),
                );
            }
            ok(
                vec![text(format!(
                    "Workspace '{ws}' created in {}/{} ({tier}).{}\n\nNext: Add data source/sink connections with `atlas-streams-build` resource='connection', then deploy a processor with resource='processor'.",
                    s(args, "cloudProvider"),
                    s(args, "region"),
                    if sample {
                        " Includes sample_stream_solar connection for testing."
                    } else {
                        ""
                    }
                ))],
                Some(json!({ "resource": "workspace" })),
            )
        }
        "connection" => {
            if !ctx.coll(WS).is_some_and(|c| c.contains(&ws_key)) {
                return not_found(format!(
                    "Stream Processing workspace {ws} not found in group {pid}."
                ));
            }
            let cname = s(args, "connectionName").to_owned();
            let ctype = s(args, "connectionType").to_owned();
            let mut doc = Map::new();
            doc.insert("name".to_owned(), json!(cname));
            doc.insert("type".to_owned(), json!(ctype));
            doc.insert("state".to_owned(), json!("READY"));
            if let Some(Value::Object(cfg)) = args.get("connectionConfig") {
                for (k, v) in cfg {
                    doc.insert(k.clone(), v.clone());
                }
            }
            ctx.coll_mut(CONN)
                .insert(format!("{ws_key}/{cname}"), Value::Object(doc));
            ok(
                vec![text(format!(
                    "Connection '{cname}' ({ctype}) added to workspace '{ws}'.\n\nNext: Add more connections or deploy a processor with `atlas-streams-build` resource='processor'. Reference this connection as '{cname}' in your processor pipeline's $source, $merge, $emit, or $iceberg stages."
                ))],
                Some(json!({ "resource": "connection" })),
            )
        }
        "processor" => {
            if !ctx.coll(WS).is_some_and(|c| c.contains(&ws_key)) {
                return not_found(format!(
                    "Stream Processing workspace {ws} not found in group {pid}."
                ));
            }
            let pname = s(args, "processorName").to_owned();
            let pipeline = args
                .get("pipeline")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first = pipeline
                .first()
                .and_then(Value::as_object)
                .and_then(|m| m.keys().next().cloned())
                .unwrap_or_default();
            if first != "$source" {
                return fail(format!(
                    "Invalid pipeline: first stage must be `$source`, but found `{first}`.\n\nEvery stream processor pipeline must start with a $source stage that reads from a connection."
                ));
            }
            let auto = args
                .get("autoStart")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tier = str_arg(args, "processorTier").unwrap_or("SP10");
            ctx.coll_mut(PROC).insert(
                format!("{ws_key}/{pname}"),
                json!({ "name": pname, "pipeline": pipeline, "state": if auto { "STARTED" } else { "CREATED" }, "tier": tier, "effectiveTier": tier, "options": { "dlq": args.get("dlq").cloned().unwrap_or(Value::Null) } }),
            );
            let dlq = match args.get("dlq") {
                Some(d) if d.is_object() => format!(
                    " DLQ configured: {}.{} via '{}'.",
                    d["db"].as_str().unwrap_or(""),
                    d["coll"].as_str().unwrap_or(""),
                    d["connectionName"].as_str().unwrap_or("")
                ),
                _ => " Consider adding a DLQ for production use.".to_owned(),
            };
            let start = if auto {
                "Processor created and started."
            } else {
                "Processor created in CREATED state."
            };
            let next = if auto {
                "Use `atlas-streams-discover` with action 'diagnose-processor' to monitor health."
            } else {
                "Use `atlas-streams-manage` with action 'start-processor' to begin processing."
            };
            let billing = if auto {
                "\n\nNote: Billing for stream processing usage is now active for this processor. Use `atlas-streams-manage` with action 'stop-processor' to stop billing."
            } else {
                ""
            };
            ok(
                vec![text(format!(
                    "{start} Processor '{pname}' deployed in workspace '{ws}'.{dlq}\n\n{next}{billing}"
                ))],
                Some(json!({ "resource": "processor" })),
            )
        }
        "privatelink" => {
            let id = ctx.object_id("stream-privatelink");
            let provider = args["privateLinkConfig"]["provider"]
                .as_str()
                .unwrap_or("AWS")
                .to_owned();
            ctx.coll_mut("stream_privatelinks").insert(
                id.clone(),
                json!({ "_id": id, "groupId": pid, "provider": provider, "state": "IDLE" }),
            );
            ok(
                vec![text(format!(
                    "PrivateLink connection created for {provider}. Use `atlas-streams-discover` with action 'get-networking' to check its status."
                ))],
                Some(json!({ "resource": "privatelink" })),
            )
        }
        other => fail(format!("Unknown resource type: {other}")),
    }
}

fn manage(ctx: &mut Ctx, pid: &str, args: &Value) -> Value {
    let ws = s(args, "workspaceName").to_owned();
    let ws_key = format!("{pid}/{ws}");
    if !ctx.coll(WS).is_some_and(|c| c.contains(&ws_key)) {
        return not_found(format!(
            "Stream Processing workspace {ws} not found in group {pid}."
        ));
    }
    let action = s(args, "action");
    let name = s(args, "resourceName").to_owned();
    let pkey = format!("{ws_key}/{name}");
    let proc_state = ctx
        .coll(PROC)
        .and_then(|c| c.get(&pkey))
        .map(|p| p["state"].as_str().unwrap_or("").to_owned());
    let set_state = |ctx: &mut Ctx, st: &str| {
        if let Some(p) = ctx.coll_mut(PROC).get_mut(&pkey) {
            p["state"] = json!(st);
        }
    };
    match action {
        "start-processor" | "stop-processor" | "modify-processor" if proc_state.is_none() => {
            not_found(format!(
                "Stream processor {name} not found in workspace {ws}."
            ))
        }
        "start-processor" => {
            if proc_state.as_deref() == Some("STARTED") {
                return ok(
                    vec![text(format!(
                        "Processor '{name}' is already running. Use action 'stop-processor' first if you want to restart it."
                    ))],
                    Some(json!({ "processorState": "STARTED" })),
                );
            }
            set_state(ctx, "STARTED");
            let note = if args.get("resumeFromCheckpoint").and_then(Value::as_bool) == Some(false) {
                " Starting from the beginning (no checkpoint resume)."
            } else {
                " Resuming from last checkpoint."
            };
            ok(
                vec![text(format!(
                    "Processor '{name}' started in workspace '{ws}'.{note}\n\nNote: Billing for stream processing usage is now active for this processor.\n\nUse `atlas-streams-discover` with action 'diagnose-processor' to monitor health. Use `atlas-streams-manage` with action 'stop-processor' to stop billing."
                ))],
                Some(json!({ "processorState": "STARTED" })),
            )
        }
        "stop-processor" => {
            if proc_state.as_deref() != Some("STARTED") {
                return ok(
                    vec![text(format!(
                        "Processor '{name}' is not running (state: {}). No action needed.",
                        proc_state.unwrap_or_default()
                    ))],
                    Some(json!({})),
                );
            }
            set_state(ctx, "STOPPED");
            ok(
                vec![text(format!(
                    "Processor '{name}' stopped. State preserved for 45 days.\n\nUse action 'modify-processor' to change its pipeline, or action 'start-processor' to resume."
                ))],
                Some(json!({ "processorState": "STOPPED" })),
            )
        }
        "modify-processor" => {
            if proc_state.as_deref() == Some("STARTED") {
                return fail(format!(
                    "Processor '{name}' must be stopped before modifying. Use action 'stop-processor' first."
                ));
            }
            let mut changed = Vec::new();
            for (arg, field) in [
                ("pipeline", "pipeline"),
                ("dlq", "dlq"),
                ("newName", "name"),
                ("tier", "tier"),
            ] {
                if let Some(v) = args.get(arg).filter(|v| !v.is_null()) {
                    if let Some(p) = ctx.coll_mut(PROC).get_mut(&pkey) {
                        p[field] = v.clone();
                    }
                    changed.push(field);
                }
            }
            if changed.is_empty() {
                return fail("No modifications specified. Provide at least one of: pipeline, dlq, newName, tier, or autoscaling.".to_owned());
            }
            if let Some(new) = str_arg(args, "newName")
                && let Some(p) = ctx.coll_mut(PROC).remove(&pkey)
            {
                ctx.coll_mut(PROC).insert(format!("{ws_key}/{new}"), p);
            }
            ok(
                vec![text(format!(
                    "Processor '{name}' modified (changed: {}).\n\nUse action 'start-processor' to resume processing with the updated configuration.",
                    changed.join(", ")
                ))],
                Some(json!({ "processorState": proc_state.unwrap_or_default() })),
            )
        }
        "update-workspace" => {
            let (region, tier) = (str_arg(args, "newRegion"), str_arg(args, "newTier"));
            if region.is_none() && tier.is_none() {
                return fail(
                    "No updates specified. Provide at least one of: newRegion or newTier."
                        .to_owned(),
                );
            }
            if let Some(w) = ctx.coll_mut(WS).get_mut(&ws_key) {
                if let Some(r) = region {
                    w["dataProcessRegion"]["region"] = json!(r);
                }
                if let Some(t) = tier {
                    w["streamConfig"]["tier"] = json!(t);
                }
            }
            ok(
                vec![text(format!(
                    "Workspace '{ws}' updated. Use `atlas-streams-discover` with action 'inspect-workspace' to verify changes."
                ))],
                Some(json!({})),
            )
        }
        "update-connection" => {
            let ckey = format!("{ws_key}/{name}");
            let Some(c) = ctx.coll_mut(CONN).get_mut(&ckey) else {
                return not_found(format!(
                    "Stream connection {name} not found in workspace {ws}."
                ));
            };
            if let Some(Value::Object(cfg)) = args.get("connectionConfig") {
                for (k, v) in cfg {
                    c[k] = v.clone();
                }
            }
            ok(
                vec![text(format!(
                    "Connection '{name}' updated in workspace '{ws}'."
                ))],
                Some(json!({ "connectionState": "READY" })),
            )
        }
        "accept-peering" => ok(
            vec![text(format!(
                "VPC peering connection '{}' accepted. It may take a few minutes to become active.",
                s(args, "peeringId")
            ))],
            Some(json!({})),
        ),
        "reject-peering" => ok(
            vec![text(format!(
                "VPC peering connection '{}' rejected.",
                s(args, "peeringId")
            ))],
            Some(json!({})),
        ),
        other => fail(format!("Unknown action: {other}")),
    }
}

fn teardown(ctx: &mut Ctx, pid: &str, args: &Value) -> Value {
    let resource = s(args, "resource");
    let ws = s(args, "workspaceName").to_owned();
    let name = s(args, "resourceName").to_owned();
    let ws_key = format!("{pid}/{ws}");
    match resource {
        "processor" => {
            if ctx
                .coll_mut(PROC)
                .remove(&format!("{ws_key}/{name}"))
                .is_none()
            {
                return not_found(format!(
                    "Stream processor {name} not found in workspace {ws}."
                ));
            }
            ok(
                vec![text(format!(
                    "Processor '{name}' deleted from workspace '{ws}'. All state and checkpoints have been permanently removed."
                ))],
                Some(json!({ "resource": "processor" })),
            )
        }
        "connection" => {
            if ctx
                .coll_mut(CONN)
                .remove(&format!("{ws_key}/{name}"))
                .is_none()
            {
                return not_found(format!(
                    "Stream connection {name} not found in workspace {ws}."
                ));
            }
            ok(
                vec![text(format!(
                    "Connection '{name}' deletion initiated in workspace '{ws}'. Use `atlas-streams-discover` with action 'list-connections' to confirm when deletion is complete."
                ))],
                Some(json!({ "resource": "connection" })),
            )
        }
        "workspace" => {
            if ctx.coll(WS).is_none_or(|c| !c.contains(&ws_key)) {
                return not_found(format!(
                    "Stream Processing workspace {ws} not found in group {pid}."
                ));
            }
            let procs = in_ws(ctx, PROC, pid, &ws).len();
            let conns = in_ws(ctx, CONN, pid, &ws).len();
            if in_ws(ctx, PROC, pid, &ws)
                .iter()
                .any(|p| p["state"] == "STARTED")
            {
                return fail(format!(
                    "Received a Forbidden API Error: [403 Forbidden] error calling Atlas API: Forbidden; Workspace {ws} has running stream processors.\n\nThis may be because the workspace has active processors. Stop all processors first with `atlas-streams-manage` action 'stop-processor', then retry deletion."
                ));
            }
            let prefix = format!("{ws_key}/");
            for coll in [PROC, CONN] {
                let keys: Vec<String> = ctx
                    .coll(coll)
                    .map(|c| {
                        c.iter()
                            .filter(|(k, _)| k.starts_with(&prefix))
                            .map(|(k, _)| k.to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                for k in keys {
                    ctx.coll_mut(coll).remove(&k);
                }
            }
            ctx.coll_mut(WS).remove(&ws_key);
            let impact = if procs > 0 || conns > 0 {
                format!(" This will also remove {procs} processor(s) and {conns} connection(s).")
            } else {
                String::new()
            };
            ok(
                vec![text(format!(
                    "Workspace '{ws}' deletion initiated.{impact} Use `atlas-streams-discover` with action 'list-workspaces' to confirm when deletion is complete."
                ))],
                Some(
                    json!({ "resource": "workspace", "processorsRemoved": procs, "connectionsRemoved": conns }),
                ),
            )
        }
        "privatelink" => {
            if ctx.coll_mut("stream_privatelinks").remove(&name).is_none() {
                return not_found(format!("PrivateLink connection {name} not found."));
            }
            ok(
                vec![text(format!(
                    "PrivateLink connection '{name}' deletion initiated. Use `atlas-streams-discover` with action 'get-networking' to confirm when deletion is complete."
                ))],
                Some(json!({ "resource": "privatelink" })),
            )
        }
        "peering" => ok(
            vec![text(format!(
                "VPC peering connection '{name}' deletion initiated."
            ))],
            Some(json!({ "resource": "peering" })),
        ),
        other => fail(format!("Unknown resource type: {other}")),
    }
}
