//! Store documents rendered as HCP Terraform JSON:API resource objects.
//!
//! Documents keep API attribute names (kebab-case) at the top level; relation fields hold IDs
//! and are listed in each renderer's `skip` set so they are not leaked as attributes.

use serde_json::{Value, json};

use crate::common::{
    Db, hex_digest, merge_attributes, opt_str, rel, rel_many, str_field, str_list,
};

pub const CONFIRMABLE: &[&str] = &[
    "planned",
    "cost_estimated",
    "policy_checked",
    "policy_override",
    "planned_and_saved",
    "post_plan_completed",
];

pub const CANCELABLE: &[&str] = &[
    "pending",
    "fetching",
    "queuing",
    "plan_queued",
    "planning",
    "cost_estimating",
    "policy_checking",
    "pre_plan_running",
    "post_plan_running",
    "confirmed",
    "apply_queued",
    "applying",
];

pub fn organization(db: &Db, org: &Value) -> Value {
    let name = str_field(org, "name");
    let attributes = merge_attributes(
        json!({
            "name": name,
            "external-id": "",
            "created-at": "2023-01-09T15:00:00Z",
            "email": "",
            "session-timeout": null,
            "session-remember": null,
            "collaborator-auth-policy": "password",
            "plan-expired": false,
            "plan-expires-at": null,
            "plan-is-trial": false,
            "plan-is-enterprise": false,
            "plan-identifier": "standard",
            "cost-estimation-enabled": true,
            "send-passing-statuses-for-untriggered-speculative-plans": false,
            "aggregated-commit-status-enabled": true,
            "speculative-plan-management-enabled": true,
            "allow-force-delete-workspaces": false,
            "fair-run-queuing-enabled": true,
            "saml-enabled": false,
            "owners-team-saml-role-id": null,
            "two-factor-conformant": true,
            "assessments-enforced": false,
            "default-execution-mode": "remote",
            "stacks-enabled": true,
            "is-unified": false,
            "remaining-testable-count": 5,
            "enforce-hyok": false,
            "permissions": {
                "can-update": true,
                "can-destroy": true,
                "can-access-via-teams": true,
                "can-create-module": true,
                "can-create-team": true,
                "can-create-workspace": true,
                "can-manage-users": true,
                "can-manage-subscription": true,
                "can-manage-sso": true,
                "can-update-oauth": true,
                "can-update-sentinel": true,
                "can-update-ssh-keys": true,
                "can-update-api-token": true,
                "can-traverse": true,
                "can-start-trial": false,
                "can-update-agent-pools": true,
                "can-manage-tags": true,
                "can-manage-varsets": true,
                "can-read-varsets": true,
                "can-manage-public-providers": true,
                "can-create-provider": true,
                "can-manage-public-modules": true,
                "can-manage-custom-providers": false,
                "can-manage-run-tasks": true,
                "can-read-run-tasks": true,
                "can-create-project": true,
                "can-deploy-no-code-modules": true,
                "can-manage-no-code-modules": true,
                "can-manage-auditing": false,
                "can-create-workspace-migration": false,
                "can-enable-stacks": true,
                "can-update-hyok-configuration": false,
                "can-view-hyok-feature-info": false
            }
        }),
        org,
        &["default-project"],
    );
    let default_project = opt_str(org, "default-project")
        .map(str::to_owned)
        .or_else(|| {
            db.all("projects")
                .into_iter()
                .find(|p| str_field(p, "organization") == name)
                .map(|p| str_field(p, "id").to_owned())
        });
    json!({
        "id": name,
        "type": "organizations",
        "attributes": attributes,
        "relationships": {
            "default-project": rel("projects", default_project.as_deref()),
            "default-agent-pool": rel("agent-pools", None),
            "oauth-tokens": { "links": { "related": format!("/api/v2/organizations/{name}/oauth-tokens") } },
            "authentication-token": { "links": { "related": format!("/api/v2/organizations/{name}/authentication-token") } },
            "entitlement-set": { "data": { "id": format!("org-{name}"), "type": "entitlement-sets" } },
        },
        "links": { "self": format!("/api/v2/organizations/{name}") }
    })
}

pub fn project(proj: &Value) -> Value {
    let id = str_field(proj, "id");
    let attributes = merge_attributes(
        json!({
            "name": "",
            "description": "",
            "created-at": "2023-01-09T15:00:00Z",
            "workspace-count": 0,
            "team-count": 0,
            "stack-count": 0,
            "is-unified": false,
            "auto-destroy-activity-duration": null,
            "default-execution-mode": "remote",
            "setting-overwrites": { "default-execution-mode": false, "default-agent-pool": false },
            "permissions": {
                "can-read": true,
                "can-update": true,
                "can-destroy": true,
                "can-create-workspace": true,
                "can-move-workspace": true,
                "can-move-stack": true,
                "can-deploy-no-code-modules": true,
                "can-read-teams": true,
                "can-manage-tags": true,
                "can-manage-teams": true,
                "can-manage-in-hcp": false,
                "can-manage-ephemeral-workspace-for-projects": true,
                "can-manage-varsets": true
            }
        }),
        proj,
        &["organization"],
    );
    json!({
        "id": id,
        "type": "projects",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(proj, "organization")),
            "default-agent-pool": rel("agent-pools", None),
        },
        "links": { "self": format!("/api/v2/projects/{id}") }
    })
}

pub fn project_with_counts(db: &Db, proj: &Value) -> Value {
    let id = str_field(proj, "id");
    let mut out = project(proj);
    let workspaces = db
        .filter("workspaces", |w| str_field(w, "project") == id)
        .len();
    let stacks = db.filter("stacks", |s| str_field(s, "project") == id).len();
    let teams = db
        .filter("team_projects", |t| str_field(t, "project") == id)
        .len();
    out["attributes"]["workspace-count"] = json!(workspaces);
    out["attributes"]["stack-count"] = json!(stacks);
    out["attributes"]["team-count"] = json!(teams);
    out
}

pub fn workspace(db: &Db, ws: &Value) -> Value {
    let id = str_field(ws, "id");
    let org = str_field(ws, "organization");
    let name = str_field(ws, "name");
    let created = ws
        .get("created-at")
        .cloned()
        .unwrap_or(json!("2024-01-01T00:00:00Z"));
    let runs_count = db.filter("runs", |r| str_field(r, "workspace") == id).len();
    let failures = db
        .filter("runs", |r| {
            str_field(r, "workspace") == id && str_field(r, "status") == "errored"
        })
        .len();
    let attributes = merge_attributes(
        json!({
            "actions": { "is-destroyable": true },
            "allow-destroy-plan": true,
            "assessments-enabled": false,
            "auto-apply": false,
            "auto-apply-run-trigger": false,
            "auto-destroy-at": null,
            "auto-destroy-activity-duration": null,
            "auto-destroy-status": null,
            "can-queue-destroy-plan": true,
            "created-at": created,
            "description": "",
            "environment": "default",
            "execution-mode": "remote",
            "file-triggers-enabled": true,
            "global-remote-state": false,
            "project-remote-state": false,
            "inherits-project-auto-destroy": true,
            "latest-change-at": ws.get("updated-at").cloned().unwrap_or(created.clone()),
            "last-assessment-result-at": null,
            "locked": false,
            "locked-reason": "",
            "migration-environment": "",
            "name": name,
            "no-code-upgrade-available": false,
            "operations": true,
            "permissions": {
                "can-update": true,
                "can-destroy": true,
                "can-queue-run": true,
                "can-read-run": true,
                "can-read-variable": true,
                "can-update-variable": true,
                "can-read-state-versions": true,
                "can-read-state-outputs": true,
                "can-create-state-versions": true,
                "can-queue-apply": true,
                "can-lock": true,
                "can-unlock": true,
                "can-force-unlock": true,
                "can-read-settings": true,
                "can-manage-tags": true,
                "can-manage-run-tasks": true,
                "can-force-delete": true,
                "can-manage-assessments": true,
                "can-manage-ephemeral-workspaces": true,
                "can-read-assessments": true,
                "can-queue-destroy": true,
                "can-manage-hyok": false
            },
            "queue-all-runs": false,
            "speculative-enabled": true,
            "source": "tfe-ui",
            "source-name": null,
            "source-url": null,
            "structured-run-output-enabled": true,
            "terraform-version": "1.9.5",
            "trigger-prefixes": [],
            "trigger-patterns": [],
            "vcs-repo": null,
            "vcs-repo-identifier": null,
            "working-directory": "",
            "updated-at": ws.get("updated-at").cloned().unwrap_or(created.clone()),
            "resource-count": 0,
            "apply-duration-average": 74000,
            "plan-duration-average": 31000,
            "policy-check-failures": 0,
            "run-failures": failures,
            "workspace-kpis-runs-count": runs_count,
            "tag-names": [],
            "setting-overwrites": { "execution-mode": false, "agent-pool": false },
            "hyok-enabled": false
        }),
        ws,
        &[
            "organization",
            "project",
            "current-run",
            "latest-run",
            "latest-run-summary",
            "current-state-version",
            "locked-by",
            "readme",
            "tag-bindings",
            "simulated-plan",
        ],
    );
    let identifier = ws
        .pointer("/vcs-repo/identifier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut attributes = attributes;
    if let Some(identifier) = identifier {
        attributes["vcs-repo-identifier"] = json!(identifier);
    }

    let current_sv = opt_str(ws, "current-state-version");
    let outputs: Vec<String> = current_sv
        .and_then(|sv| db.get("state_versions", sv))
        .map(|sv| {
            sv.get("outputs")
                .and_then(Value::as_array)
                .map(|outs| outs.iter().map(|o| str_field(o, "id").to_owned()).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let mut relationships = json!({
        "organization": rel("organizations", Some(org)),
        "project": rel("projects", opt_str(ws, "project")),
        "current-run": rel("runs", opt_str(ws, "current-run")),
        "latest-run": rel("runs", opt_str(ws, "latest-run").or(opt_str(ws, "current-run"))),
        "current-state-version": rel("state-versions", current_sv),
        "current-configuration-version": { "data": null },
        "outputs": rel_many("workspace-outputs", outputs.iter().map(String::as_str)),
        "agent-pool": rel("agent-pools", None),
        "remote-state-consumers": { "links": { "related": format!("/api/v2/workspaces/{id}/relationships/remote-state-consumers") } },
        "vars": { "links": { "related": format!("/api/v2/workspaces/{id}/vars") } },
        "tag-bindings": { "links": { "related": format!("/api/v2/workspaces/{id}/tag-bindings") } },
        "effective-tag-bindings": { "links": { "related": format!("/api/v2/workspaces/{id}/effective-tag-bindings") } },
    });
    if let Some(locked_by) = ws.get("locked-by").filter(|v| v.is_object()) {
        relationships["locked-by"] = json!({ "data": {
            "id": str_field(locked_by, "id"),
            "type": str_field(locked_by, "type"),
        } });
    }
    if ws.get("readme").and_then(Value::as_str).is_some() {
        relationships["readme"] = rel("workspace-readme", Some(&readme_id(id)));
    }
    json!({
        "id": id,
        "type": "workspaces",
        "attributes": attributes,
        "relationships": relationships,
        "links": {
            "self": format!("/api/v2/organizations/{org}/workspaces/{name}"),
            "self-html": format!("/app/{org}/workspaces/{name}"),
        }
    })
}

pub fn readme_id(workspace_id: &str) -> String {
    format!("readme-{}", workspace_id.trim_start_matches("ws-"))
}

pub fn readme(ws: &Value) -> Option<Value> {
    let markdown = ws.get("readme").and_then(Value::as_str)?;
    Some(json!({
        "id": readme_id(str_field(ws, "id")),
        "type": "workspace-readme",
        "attributes": { "raw-markdown": markdown }
    }))
}

fn status_in(status: &str, set: &[&str]) -> bool {
    set.contains(&status)
}

pub fn run(db: &Db, run: &Value) -> Value {
    let id = str_field(run, "id");
    let status = str_field(run, "status");
    let created = run.get("created-at").cloned().unwrap_or(Value::Null);
    let confirmable = status_in(status, CONFIRMABLE);
    let attributes = merge_attributes(
        json!({
            "actions": {
                "is-cancelable": status_in(status, CANCELABLE),
                "is-confirmable": confirmable,
                "is-discardable": confirmable || status == "policy_soft_failed",
                "is-force-cancelable": false
            },
            "allow-config-generation": false,
            "allow-empty-apply": false,
            "auto-apply": false,
            "canceled-at": null,
            "created-at": created,
            "force-cancel-available-at": null,
            "has-changes": false,
            "is-destroy": false,
            "message": "Queued manually using the Terraform API",
            "permissions": {
                "can-apply": true,
                "can-cancel": true,
                "can-comment": true,
                "can-discard": true,
                "can-force-execute": true,
                "can-force-cancel": true,
                "can-override-policy-check": true
            },
            "plan-only": false,
            "position-in-queue": 0,
            "refresh": true,
            "refresh-only": false,
            "replace-addrs": [],
            "save-plan": false,
            "source": "tfe-api",
            "status": status,
            "status-timestamps": {},
            "target-addrs": [],
            "terraform-version": "1.9.5",
            "trigger-reason": "manual",
            "variables": []
        }),
        run,
        &[
            "workspace",
            "plan",
            "apply",
            "created-by",
            "confirmed-by",
            "confirm-comment",
            "discard-comment",
            "cancel-comment",
            "input-variables",
            "configuration-version",
        ],
    );
    let comments: Vec<String> = db
        .filter("comments", |c| str_field(c, "run") == id)
        .iter()
        .map(|c| str_field(c, "id").to_owned())
        .collect();
    let policy_checks: Vec<String> = db
        .filter("policy_checks", |c| str_field(c, "run") == id)
        .iter()
        .map(|c| str_field(c, "id").to_owned())
        .collect();
    json!({
        "id": id,
        "type": "runs",
        "attributes": attributes,
        "relationships": {
            "workspace": rel("workspaces", opt_str(run, "workspace")),
            "apply": rel("applies", opt_str(run, "apply")),
            "plan": rel("plans", opt_str(run, "plan")),
            "configuration-version": rel("configuration-versions", opt_str(run, "configuration-version")),
            "created-by": rel("users", opt_str(run, "created-by")),
            "confirmed-by": rel("users", opt_str(run, "confirmed-by")),
            "cost-estimate": { "data": null },
            "policy-checks": rel_many("policy-checks", policy_checks.iter().map(String::as_str)),
            "comments": rel_many("comments", comments.iter().map(String::as_str)),
            "run-events": { "links": { "related": format!("/api/v2/runs/{id}/run-events") } },
            "task-stages": { "links": { "related": format!("/api/v2/runs/{id}/task-stages") } },
        },
        "links": { "self": format!("/api/v2/runs/{id}") }
    })
}

pub fn plan(db: &Db, base: &str, plan: &Value) -> Value {
    let id = str_field(plan, "id");
    let attributes = merge_attributes(
        json!({
            "execution-details": { "mode": "remote" },
            "generated-configuration": false,
            "has-changes": false,
            "resource-additions": 0,
            "resource-changes": 0,
            "resource-destructions": 0,
            "resource-imports": 0,
            "status": "finished",
            "status-timestamps": {},
            "log-read-url": format!("{base}/archivist/v1/object/plan-log-{id}")
        }),
        plan,
        &["run", "log", "resource-changes-detail", "json-output"],
    );
    let exports: Vec<String> = db
        .filter("plan_exports", |e| str_field(e, "plan") == id)
        .iter()
        .map(|e| str_field(e, "id").to_owned())
        .collect();
    json!({
        "id": id,
        "type": "plans",
        "attributes": attributes,
        "relationships": {
            "state-versions": { "data": [] },
            "exports": rel_many("plan-exports", exports.iter().map(String::as_str)),
        },
        "links": {
            "self": format!("/api/v2/plans/{id}"),
            "json-output": format!("/api/v2/plans/{id}/json-output"),
            "json-output-redacted": format!("/api/v2/plans/{id}/json-output-redacted"),
            "json-schema": format!("/api/v2/plans/{id}/json-schema"),
        }
    })
}

pub fn apply(base: &str, apply: &Value) -> Value {
    let id = str_field(apply, "id");
    let attributes = merge_attributes(
        json!({
            "execution-details": { "mode": "remote" },
            "resource-additions": 0,
            "resource-changes": 0,
            "resource-destructions": 0,
            "resource-imports": 0,
            "status": "pending",
            "status-timestamps": {},
            "log-read-url": format!("{base}/archivist/v1/object/apply-log-{id}")
        }),
        apply,
        &["run", "log", "state-version"],
    );
    json!({
        "id": id,
        "type": "applies",
        "attributes": attributes,
        "relationships": {
            "state-versions": rel_many("state-versions", opt_str(apply, "state-version")),
        },
        "links": { "self": format!("/api/v2/applies/{id}") }
    })
}

pub fn variable(var: &Value) -> Value {
    let id = str_field(var, "id");
    let workspace = str_field(var, "workspace");
    let mut attributes = merge_attributes(
        json!({
            "key": "",
            "value": "",
            "sensitive": false,
            "category": "terraform",
            "hcl": false,
            "created-at": "2024-01-01T00:00:00Z",
            "description": null,
            "version-id": hex_digest(&format!("{id}:{}", var.get("value").unwrap_or(&Value::Null)))
        }),
        var,
        &["workspace"],
    );
    if attributes["sensitive"] == json!(true) {
        attributes["value"] = Value::Null;
    }
    json!({
        "id": id,
        "type": "vars",
        "attributes": attributes,
        "relationships": { "configurable": {
            "data": { "id": workspace, "type": "workspaces" },
            "links": { "related": format!("/api/v2/workspaces/{workspace}") }
        } },
        "links": { "self": format!("/api/v2/workspaces/{workspace}/vars/{id}") }
    })
}

pub fn varset(db: &Db, vs: &Value) -> Value {
    let id = str_field(vs, "id");
    let workspaces = str_list(vs, "workspaces");
    let projects = str_list(vs, "projects");
    let vars: Vec<String> = db
        .filter("varset_vars", |v| str_field(v, "varset") == id)
        .iter()
        .map(|v| str_field(v, "id").to_owned())
        .collect();
    let mut attributes = merge_attributes(
        json!({
            "name": "",
            "description": "",
            "global": false,
            "priority": false,
            "updated-at": "2024-01-01T00:00:00Z",
        }),
        vs,
        &["organization", "workspaces", "projects", "stacks"],
    );
    attributes["var-count"] = json!(vars.len());
    attributes["workspace-count"] = json!(workspaces.len());
    attributes["project-count"] = json!(projects.len());
    json!({
        "id": id,
        "type": "varsets",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(vs, "organization")),
            "vars": rel_many("vars", vars.iter().map(String::as_str)),
            "workspaces": rel_many("workspaces", workspaces.iter().map(String::as_str)),
            "projects": rel_many("projects", projects.iter().map(String::as_str)),
            "stacks": { "data": [] },
        }
    })
}

pub fn varset_var(var: &Value) -> Value {
    let id = str_field(var, "id");
    let mut attributes = merge_attributes(
        json!({
            "key": "",
            "value": "",
            "sensitive": false,
            "category": "terraform",
            "hcl": false,
            "created-at": "2024-01-01T00:00:00Z",
            "description": null,
            "version-id": hex_digest(&format!("{id}:{}", var.get("value").unwrap_or(&Value::Null)))
        }),
        var,
        &["varset"],
    );
    if attributes["sensitive"] == json!(true) {
        attributes["value"] = Value::Null;
    }
    json!({
        "id": id,
        "type": "vars",
        "attributes": attributes,
        "relationships": { "varset": rel("varsets", opt_str(var, "varset")) }
    })
}

pub fn policy_set(ps: &Value) -> Value {
    let id = str_field(ps, "id");
    let workspaces = str_list(ps, "workspaces");
    let projects = str_list(ps, "projects");
    let mut attributes = merge_attributes(
        json!({
            "name": "",
            "description": "",
            "kind": "sentinel",
            "global": false,
            "overridable": false,
            "agent-enabled": false,
            "policy-tool-version": "latest",
            "policies-path": null,
            "policy-count": 0,
            "vcs-repo": null,
            "policy-update-patterns": [],
            "tag-selectors": [],
            "created-at": "2024-01-01T00:00:00Z",
            "updated-at": "2024-01-01T00:00:00Z",
        }),
        ps,
        &["organization", "workspaces", "projects", "policies"],
    );
    attributes["workspace-count"] = json!(workspaces.len());
    attributes["project-count"] = json!(projects.len());
    json!({
        "id": id,
        "type": "policy-sets",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(ps, "organization")),
            "workspaces": rel_many("workspaces", workspaces.iter().map(String::as_str)),
            "projects": rel_many("projects", projects.iter().map(String::as_str)),
            "workspace-exclusions": { "data": [] },
            "project-exclusions": { "data": [] },
        },
        "links": { "self": format!("/api/v2/policy-sets/{id}") }
    })
}

pub fn policy_check(pc: &Value) -> Value {
    let id = str_field(pc, "id");
    let attributes = merge_attributes(
        json!({
            "result": {
                "result": true, "passed": 0, "total-failed": 0, "hard-failed": 0,
                "soft-failed": 0, "advisory-failed": 0, "duration-ms": 0, "sentinel": null
            },
            "scope": "organization",
            "status": "passed",
            "status-timestamps": {},
            "actions": { "is-overridable": false },
            "permissions": { "can-override": false }
        }),
        pc,
        &["run", "log"],
    );
    json!({
        "id": id,
        "type": "policy-checks",
        "attributes": attributes,
        "relationships": { "run": rel("runs", opt_str(pc, "run")) },
        "links": { "output": format!("/api/v2/policy-checks/{id}/output") }
    })
}

pub fn team(team: &Value) -> Value {
    let id = str_field(team, "id");
    let users = str_list(team, "users");
    let memberships = str_list(team, "organization-memberships");
    let mut attributes = merge_attributes(
        json!({
            "name": "",
            "sso-team-id": null,
            "visibility": "secret",
            "allow-member-token-management": true,
            "is-unified": false,
            "organization-access": {
                "manage-policies": false,
                "manage-policy-overrides": false,
                "delegate-policy-overrides": false,
                "manage-workspaces": false,
                "manage-vcs-settings": false,
                "manage-providers": false,
                "manage-modules": false,
                "manage-run-tasks": false,
                "manage-projects": false,
                "read-workspaces": false,
                "read-projects": false,
                "manage-membership": false,
                "manage-teams": false,
                "manage-organization-access": false,
                "access-secret-teams": false,
                "manage-agent-pools": false
            },
            "permissions": { "can-update-membership": true, "can-destroy": true, "can-update-organization-access": true, "can-update-api-token": true, "can-update-visibility": true }
        }),
        team,
        &["organization", "users", "organization-memberships"],
    );
    attributes["users-count"] = json!(users.len());
    json!({
        "id": id,
        "type": "teams",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(team, "organization")),
            "users": rel_many("users", users.iter().map(String::as_str)),
            "organization-memberships": rel_many("organization-memberships", memberships.iter().map(String::as_str)),
            "authentication-token": { "meta": {} },
        },
        "links": { "self": format!("/api/v2/teams/{id}") }
    })
}

pub fn user(user: &Value) -> Value {
    let id = str_field(user, "id");
    let attributes = merge_attributes(
        json!({
            "username": "",
            "is-service-account": false,
            "auth-method": "tfc",
            "avatar-url": "https://www.gravatar.com/avatar/9babb00091b97b9ce9538c45807fd35f?s=100&d=mm",
            "v2-only": false,
            "two-factor": { "enabled": true, "verified": true },
            "email": "",
            "unconfirmed-email": null,
            "has-git-hub-app-token": false,
            "is-admin": false,
            "is-site-admin": false,
            "is-sso-login": false,
            "permissions": {
                "can-create-organizations": true,
                "can-change-email": true,
                "can-change-username": true,
                "can-manage-user-tokens": true,
                "can-view2fa-settings": true,
                "can-manage-hcp-account": false
            }
        }),
        user,
        &["current"],
    );
    json!({
        "id": id,
        "type": "users",
        "attributes": attributes,
        "relationships": {
            "authentication-tokens": { "links": { "related": "/api/v2/users/{id}/authentication-tokens".replace("{id}", id) } }
        },
        "links": { "self": format!("/api/v2/users/{id}") }
    })
}

pub fn team_access(ta: &Value) -> Value {
    let id = str_field(ta, "id");
    let attributes = merge_attributes(
        access_permissions(str_field(ta, "access")),
        ta,
        &["team", "workspace"],
    );
    json!({
        "id": id,
        "type": "team-workspaces",
        "attributes": attributes,
        "relationships": {
            "team": rel("teams", opt_str(ta, "team")),
            "workspace": rel("workspaces", opt_str(ta, "workspace")),
        },
        "links": { "self": format!("/api/v2/team-workspaces/{id}") }
    })
}

pub fn access_permissions(access: &str) -> Value {
    let (runs, variables, state, mocks, locking, tasks) = match access {
        "read" => ("read", "read", "read-outputs", "none", false, false),
        "plan" => ("plan", "read", "read", "none", false, false),
        "write" => ("apply", "write", "write", "read", true, false),
        _ => ("apply", "write", "write", "read", true, true),
    };
    json!({
        "access": access,
        "runs": runs,
        "variables": variables,
        "state-versions": state,
        "sentinel-mocks": mocks,
        "workspace-locking": locking,
        "run-tasks": tasks,
        "policy-overrides": false
    })
}

pub fn team_project(tp: &Value) -> Value {
    let id = str_field(tp, "id");
    let attributes = merge_attributes(json!({ "access": "read" }), tp, &["team", "project"]);
    json!({
        "id": id,
        "type": "team-projects",
        "attributes": attributes,
        "relationships": {
            "team": rel("teams", opt_str(tp, "team")),
            "project": rel("projects", opt_str(tp, "project")),
        },
        "links": { "self": format!("/api/v2/team-projects/{id}") }
    })
}

pub fn stack(stack: &Value) -> Value {
    let id = str_field(stack, "id");
    let attributes = merge_attributes(
        json!({
            "name": "",
            "description": "",
            "vcs-repo": null,
            "speculative-enabled": false,
            "created-at": "2024-01-01T00:00:00Z",
            "updated-at": "2024-01-01T00:00:00Z",
            "upstream-count": 0,
            "downstream-count": 0,
            "inputs-count": 0,
            "outputs-count": 0,
            "creation-source": "ui",
            "working-directory": "",
            "trigger-patterns": []
        }),
        stack,
        &["organization", "project", "latest-stack-configuration"],
    );
    json!({
        "id": id,
        "type": "stacks",
        "attributes": attributes,
        "relationships": {
            "project": rel("projects", opt_str(stack, "project")),
            "agent-pool": rel("agent-pools", None),
            "latest-stack-configuration": rel("stack-configurations", opt_str(stack, "latest-stack-configuration")),
        },
        "links": { "self": format!("/api/v2/stacks/{id}") }
    })
}

pub fn state_version(base: &str, sv: &Value) -> Value {
    let id = str_field(sv, "id");
    let outputs: Vec<String> = sv
        .get("outputs")
        .and_then(Value::as_array)
        .map(|o| o.iter().map(|o| str_field(o, "id").to_owned()).collect())
        .unwrap_or_default();
    let attributes = merge_attributes(
        json!({
            "created-at": "2024-01-01T00:00:00Z",
            "size": 18_000,
            "hosted-state-download-url": format!("{base}/archivist/v1/object/state-{id}"),
            "hosted-json-state-download-url": format!("{base}/archivist/v1/object/state-json-{id}"),
            "hosted-state-upload-url": null,
            "hosted-json-state-upload-url": null,
            "status": "finalized",
            "intermediate": false,
            "modules": null,
            "providers": null,
            "resources": [],
            "resources-processed": true,
            "serial": 1,
            "state-version": 4,
            "terraform-version": "1.9.5",
            "vcs-commit-url": null,
            "vcs-commit-sha": null,
            "billable-rum-count": 0
        }),
        sv,
        &["workspace", "run", "outputs", "created-by"],
    );
    json!({
        "id": id,
        "type": "state-versions",
        "attributes": attributes,
        "relationships": {
            "run": rel("runs", opt_str(sv, "run")),
            "created-by": rel("users", opt_str(sv, "created-by")),
            "workspace": rel("workspaces", opt_str(sv, "workspace")),
            "outputs": rel_many("state-version-outputs", outputs.iter().map(String::as_str)),
        },
        "links": { "self": format!("/api/v2/state-versions/{id}") }
    })
}

pub fn state_version_output(out: &Value) -> Value {
    let id = str_field(out, "id");
    let sensitive = out
        .get("sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "id": id,
        "type": "state-version-outputs",
        "attributes": {
            "name": str_field(out, "name"),
            "sensitive": sensitive,
            "type": out.get("type").cloned().unwrap_or(json!("string")),
            "value": if sensitive { Value::Null } else { out.get("value").cloned().unwrap_or(Value::Null) },
            "detailed-type": out.get("type").cloned().unwrap_or(json!("string")),
        },
        "links": { "self": format!("/api/v2/state-version-outputs/{id}") }
    })
}

pub fn workspace_output(out: &Value) -> Value {
    let id = str_field(out, "id");
    let sensitive = out
        .get("sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "id": id,
        "type": "workspace-outputs",
        "attributes": {
            "name": str_field(out, "name"),
            "sensitive": sensitive,
            "output-type": out.get("type").cloned().unwrap_or(json!("string")),
            "value": if sensitive { Value::Null } else { out.get("value").cloned().unwrap_or(Value::Null) },
        },
        "links": { "self": format!("/api/v2/workspace-outputs/{id}") }
    })
}

pub fn comment(c: &Value) -> Value {
    let id = str_field(c, "id");
    json!({
        "id": id,
        "type": "comments",
        "attributes": { "body": str_field(c, "body") },
        "relationships": {
            "run-event": rel("run-events", opt_str(c, "run-event")),
        },
        "links": { "self": format!("/api/v2/comments/{id}") }
    })
}

pub fn plan_export(pe: &Value) -> Value {
    let id = str_field(pe, "id");
    let attributes = merge_attributes(
        json!({
            "data-type": "sentinel-mock-bundle-v0",
            "status": "queued",
            "status-timestamps": {}
        }),
        pe,
        &["plan", "created-at"],
    );
    json!({
        "id": id,
        "type": "plan-exports",
        "attributes": attributes,
        "relationships": { "plan": rel("plans", opt_str(pe, "plan")) },
        "links": {
            "self": format!("/api/v2/plan-exports/{id}"),
            "download": format!("/api/v2/plan-exports/{id}/download"),
        }
    })
}

pub fn registry_module(db: &Db, module: &Value) -> Value {
    let id = str_field(module, "id");
    let no_code: Vec<String> = db
        .filter("no_code_modules", |n| str_field(n, "registry-module") == id)
        .iter()
        .map(|n| str_field(n, "id").to_owned())
        .collect();
    let mut attributes = merge_attributes(
        json!({
            "name": "",
            "namespace": "",
            "provider": "",
            "registry-name": "private",
            "status": "setup_complete",
            "version-statuses": [],
            "created-at": "2024-01-01T00:00:00Z",
            "updated-at": "2024-01-01T00:00:00Z",
            "vcs-repo": null,
            "publishing-mechanism": "git_tag",
            "test-config": null,
            "no-code": false,
            "permissions": { "can-delete": true, "can-resync": true, "can-retry": true }
        }),
        module,
        &["organization", "details", "metadata"],
    );
    attributes["no-code"] = json!(!no_code.is_empty() || attributes["no-code"] == json!(true));
    json!({
        "id": id,
        "type": "registry-modules",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(module, "organization")),
            "no-code-modules": rel_many("no-code-modules", no_code.iter().map(String::as_str)),
        },
        "links": { "self": format!(
            "/api/v2/organizations/{}/registry-modules/{}/{}/{}/{}",
            str_field(module, "organization"),
            opt_str(module, "registry-name").unwrap_or("private"),
            str_field(module, "namespace"),
            str_field(module, "name"),
            str_field(module, "provider"),
        ) }
    })
}

pub fn no_code_module(ncm: &Value) -> Value {
    let id = str_field(ncm, "id");
    let options: Vec<String> = ncm
        .get("variable-options")
        .and_then(Value::as_array)
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, _)| format!("ncvaropt-{}-{i}", id.trim_start_matches("nocode-")))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": id,
        "type": "no-code-modules",
        "attributes": {
            "enabled": ncm.get("enabled").cloned().unwrap_or(json!(true)),
            "version-pin": str_field(ncm, "version-pin"),
        },
        "relationships": {
            "organization": rel("organizations", opt_str(ncm, "organization")),
            "registry-module": rel("registry-modules", opt_str(ncm, "registry-module")),
            "variable-options": rel_many("variable-options", options.iter().map(String::as_str)),
        },
        "links": { "self": format!("/api/v2/no-code-modules/{id}") }
    })
}

pub fn no_code_variable_options(ncm: &Value) -> Vec<Value> {
    let id = str_field(ncm, "id");
    ncm.get("variable-options")
        .and_then(Value::as_array)
        .map(|o| {
            o.iter()
                .enumerate()
                .map(|(i, opt)| {
                    json!({
                        "id": format!("ncvaropt-{}-{i}", id.trim_start_matches("nocode-")),
                        "type": "variable-options",
                        "attributes": {
                            "variable-name": str_field(opt, "variable-name"),
                            "variable-type": str_field(opt, "variable-type"),
                            "options": opt.get("options").cloned().unwrap_or(json!([])),
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn registry_provider(provider: &Value) -> Value {
    let id = str_field(provider, "id");
    let versions: Vec<String> = provider
        .get("versions")
        .and_then(Value::as_array)
        .map(|v| v.iter().map(|v| str_field(v, "id").to_owned()).collect())
        .unwrap_or_default();
    let registry = opt_str(provider, "registry-name").unwrap_or("private");
    let attributes = merge_attributes(
        json!({
            "name": "",
            "namespace": "",
            "registry-name": registry,
            "created-at": "2024-01-01T00:00:00Z",
            "updated-at": "2024-01-01T00:00:00Z",
            "permissions": { "can-delete": true }
        }),
        provider,
        &["organization", "versions"],
    );
    json!({
        "id": id,
        "type": "registry-providers",
        "attributes": attributes,
        "relationships": {
            "organization": rel("organizations", opt_str(provider, "organization")),
            "registry-provider-versions": rel_many("registry-provider-versions", versions.iter().map(String::as_str)),
        },
        "links": { "self": format!(
            "/api/v2/organizations/{}/registry-providers/{registry}/{}/{}",
            str_field(provider, "organization"),
            str_field(provider, "namespace"),
            str_field(provider, "name"),
        ) }
    })
}

/// Version resources (and their platforms) of a private registry provider, for `included`.
/// Platforms carry no back-reference to their version: go-tfe's JSON:API decoder resolves
/// included relations recursively, so a version <-> platform cycle would never terminate.
pub fn registry_provider_versions(provider: &Value) -> Vec<Value> {
    let provider_id = str_field(provider, "id");
    let mut out = Vec::new();
    for v in provider
        .get("versions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let vid = str_field(v, "id");
        let platforms: Vec<&Value> = v
            .get("platforms")
            .and_then(Value::as_array)
            .map(|p| p.iter().collect())
            .unwrap_or_default();
        out.push(json!({
            "id": vid,
            "type": "registry-provider-versions",
            "attributes": {
                "version": str_field(v, "version"),
                "created-at": v.get("created-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
                "updated-at": v.get("updated-at").cloned().unwrap_or(json!("2024-01-01T00:00:00Z")),
                "key-id": str_field(v, "key-id"),
                "protocols": v.get("protocols").cloned().unwrap_or(json!(["5.0"])),
                "permissions": { "can-delete": true, "can-upload-asset": true },
                "shasums-uploaded": true,
                "shasums-sig-uploaded": true
            },
            "relationships": {
                "registry-provider": rel("registry-providers", Some(provider_id)),
                "platforms": rel_many("registry-provider-platforms", platforms.iter().map(|p| str_field(p, "id"))),
            },
            "links": {
                "shasums-download": format!("/api/v2/registry-provider-versions/{vid}/shasums"),
                "shasums-sig-download": format!("/api/v2/registry-provider-versions/{vid}/shasums-sig"),
            }
        }));
        for p in platforms {
            out.push(json!({
                "id": str_field(p, "id"),
                "type": "registry-provider-platforms",
                "attributes": {
                    "os": str_field(p, "os"),
                    "arch": str_field(p, "arch"),
                    "filename": str_field(p, "filename"),
                    "shasum": str_field(p, "shasum"),
                    "permissions": { "can-delete": true, "can-upload-asset": true },
                    "provider-binary-uploaded": true
                }
            }));
        }
    }
    out
}
