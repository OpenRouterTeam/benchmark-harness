//! Mock Linear workspace (replica mode) answering the 198 tools of
//! `@tacticlaunch/mcp-linear` 1.4.5 on `POST /_mcp/tools/call`.
//!
//! Results mirror the server: the handler's JSON pretty-printed as one text block, plus
//! `structuredContent` (array results wrapped as `{ "items": [...] }`). Errors are
//! `Error: <message>` with `isError: true`, including the server's argument validation
//! (`Unknown argument(s) for ...`, `Invalid arguments for ...`). See [`store`] for the
//! store layout.

pub mod issues;
pub mod misc;
pub mod projects;
pub mod store;
pub mod teams;

use std::{collections::HashMap, sync::OnceLock};

use axum::{Json, Router, extract::State, http::StatusCode, response::Response, routing::post};
use mock_core::{
    http::json,
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};
use serde_json::{Value, json as json_value};

use crate::store::{Ctx, R};

pub const NAME: &str = "linear";
pub const PORT: u16 = 8127;

pub struct Linear;

impl MockService for Linear {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route(TOOLS_CALL_PATH, post(tools_call))
            .fallback(not_found)
    }
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({ "errors": [{ "message": "Not Found" }] }),
    )
}

/// Argument schemas of every tool, extracted from `servers/snapshots/linear.json`.
fn schemas() -> &'static HashMap<String, Value> {
    static SCHEMAS: OnceLock<HashMap<String, Value>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/tools.json")).expect("tools.json is valid")
    })
}

const INITIATIVE_TOOLS: &[&str] = &[
    "getInitiatives",
    "getInitiativeById",
    "createInitiative",
    "updateInitiative",
    "archiveInitiative",
    "unarchiveInitiative",
    "deleteInitiative",
    "getInitiativeProjects",
    "getInitiativeUpdateById",
    "getInitiativeUpdates",
    "createInitiativeUpdate",
    "updateInitiativeUpdate",
    "archiveInitiativeUpdate",
    "unarchiveInitiativeUpdate",
    "addProjectToInitiative",
    "removeProjectFromInitiative",
];

fn type_ok(spec: &Value, v: &Value, key: &str) -> bool {
    if v.is_null() {
        return spec["nullable"].as_bool().unwrap_or(false);
    }
    let ok = match spec["type"].as_str().unwrap_or("any") {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_i64().is_some() && (key != "limit" || v.as_i64().is_some_and(|n| n > 0)),
        "boolean" => v.is_boolean(),
        "array" => v.as_array().is_some_and(|a| {
            spec.get("items_enum")
                .and_then(Value::as_array)
                .is_none_or(|allowed| a.iter().all(|x| allowed.contains(x)))
        }),
        "object" => v.is_object(),
        _ => true,
    };
    ok && spec
        .get("enum")
        .and_then(Value::as_array)
        .is_none_or(|allowed| allowed.contains(v))
}

/// The server's `validateToolArguments` plus an approximation of its type guards.
fn validate(name: &str, args: &Value) -> Result<(), String> {
    let Some(schema) = schemas().get(name) else {
        return Err(format!("Unknown tool: {name}"));
    };
    let Some(obj) = args.as_object() else {
        return Ok(());
    };
    let props = schema["props"].as_object().cloned().unwrap_or_default();
    let unknown: Vec<&str> = obj
        .keys()
        .filter(|k| !props.contains_key(k.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "Unknown argument(s) for {name}: {}",
            unknown.join(", ")
        ));
    }
    let short = name.trim_start_matches("linear_");
    let invalid = || {
        if INITIATIVE_TOOLS.contains(&short) {
            format!("Invalid input for {short}")
        } else {
            format!("Invalid arguments for {short}")
        }
    };
    for req in schema["required"].as_array().into_iter().flatten() {
        let k = req.as_str().unwrap_or("");
        match obj.get(k) {
            None => return Err(invalid()),
            Some(v) if v.is_null() && !props[k]["nullable"].as_bool().unwrap_or(false) => {
                return Err(invalid());
            }
            _ => {}
        }
        if k.starts_with("confirm") && obj.get(k) != Some(&Value::Bool(true)) {
            return Err(invalid());
        }
    }
    for (k, v) in obj {
        if !type_ok(&props[k.as_str()], v, k) {
            return Err(invalid());
        }
    }
    Ok(())
}

fn dispatch(ctx: &mut Ctx, name: &str, a: &Value) -> R {
    use issues as i;
    use misc as m;
    use projects as p;
    use teams as t;
    match name.trim_start_matches("linear_") {
        // users & org
        "getViewer" => t::get_viewer(ctx, a),
        "getOrganization" => t::get_organization(ctx, a),
        "getUsers" => t::get_users(ctx, a),
        "getLabels" => t::get_labels(ctx, a),
        // teams
        "getTeams" => t::get_teams(ctx, a),
        "getWorkflowStates" => t::get_workflow_states(ctx, a),
        "createWorkflowState" => t::create_workflow_state(ctx, a),
        "updateWorkflowState" => t::update_workflow_state(ctx, a),
        "updateTeam" => t::update_team(ctx, a),
        "getTeamMemberships" => t::get_team_memberships(ctx, a),
        "createTeam" => t::create_team(ctx, a),
        "archiveTeam" => t::archive_team(ctx, a),
        "addUserToTeam" => t::add_user_to_team(ctx, a),
        "removeUserFromTeam" => t::remove_user_from_team(ctx, a),
        "updateTeamMembership" => t::update_team_membership(ctx, a),
        "getTeamLabels" => t::get_team_labels(ctx, a),
        "createTeamLabel" => t::create_team_label(ctx, a),
        // projects
        "getProjects" => p::get_projects(ctx, a),
        "getProjectById" => p::get_project(ctx, a),
        "createProject" => p::create_project(ctx, a),
        "updateProject" => p::update_project(ctx, a),
        "createProjectUpdate" => p::create_update(ctx, a, false),
        "updateProjectUpdate" => p::edit_update(ctx, a, false),
        "getProjectUpdateById" => p::get_update(ctx, a, false),
        "getProjectUpdates" => p::list_updates(ctx, a, false),
        "archiveProjectUpdate" => p::archive_update(ctx, a, false, true),
        "unarchiveProjectUpdate" => p::archive_update(ctx, a, false, false),
        "deleteProjectUpdate" => p::delete_project_update(ctx, a),
        "archiveProject" => p::archive_project(ctx, a),
        "addIssueToProject" => p::add_issue_to_project(ctx, a),
        "removeIssueFromProject" => p::remove_issue_from_project(ctx, a),
        "getProjectIssues" => p::project_issues(ctx, a),
        "getProjectMembers" => p::project_members(ctx, a),
        "addProjectMember" => p::change_member(ctx, a, true),
        "removeProjectMember" => p::change_member(ctx, a, false),
        // roadmaps
        "getRoadmaps" => p::get_roadmaps(ctx, a),
        "getRoadmapById" => p::get_roadmap(ctx, a),
        "createRoadmap" => p::create_roadmap(ctx, a),
        "updateRoadmap" => p::update_roadmap(ctx, a),
        "archiveRoadmap" => p::archive_roadmap(ctx, a),
        // releases
        "getReleasePipelines" => m::get_pipelines(ctx, a),
        "getReleasePipelineById" => m::get_pipeline(ctx, a),
        "createReleasePipeline" => m::create_pipeline(ctx, a),
        "updateReleasePipeline" => m::update_pipeline(ctx, a),
        "archiveReleasePipeline" => m::archive_pipeline(ctx, a, true),
        "unarchiveReleasePipeline" => m::archive_pipeline(ctx, a, false),
        "deleteReleasePipeline" => m::delete_pipeline(ctx, a),
        "getReleases" => m::get_releases(ctx, a),
        "getReleaseById" => m::get_release(ctx, a),
        "searchReleases" => m::search_releases(ctx, a),
        "getReleaseStages" => m::get_stages(ctx, a),
        "createReleaseStage" => m::create_stage(ctx, a),
        "updateReleaseStage" => m::update_stage(ctx, a),
        "archiveReleaseStage" => m::archive_stage(ctx, a, true),
        "unarchiveReleaseStage" => m::archive_stage(ctx, a, false),
        "getReleaseNotes" => m::get_notes(ctx, a),
        "getReleaseNoteById" => m::get_note(ctx, a),
        "createRelease" => m::create_release(ctx, a),
        "updateRelease" => m::update_release(ctx, a),
        "completeRelease" => m::complete_release(ctx, a),
        "archiveRelease" => m::archive_release(ctx, a, true),
        "unarchiveRelease" => m::archive_release(ctx, a, false),
        "addIssueToRelease" => m::issue_release(ctx, a, true),
        "removeIssueFromRelease" => m::issue_release(ctx, a, false),
        "createReleaseNote" => m::create_note(ctx, a),
        "updateReleaseNote" => m::update_note(ctx, a),
        "deleteReleaseNote" => m::delete_note(ctx, a),
        // milestones
        "getMilestones" => p::get_milestones(ctx, a),
        "getMilestoneById" => p::get_milestone(ctx, a),
        "createMilestone" => p::create_milestone(ctx, a),
        "updateMilestone" => p::update_milestone(ctx, a),
        "archiveMilestone" => p::archive_milestone(ctx, a),
        // documents
        "getDocuments" => p::get_documents(ctx, a),
        "getDocumentById" => p::get_document(ctx, a),
        "getProjectDocuments" => p::parent_documents(ctx, a, "project"),
        "getInitiativeDocuments" => p::parent_documents(ctx, a, "initiative"),
        "getTeamDocuments" => p::parent_documents(ctx, a, "team"),
        "getIssueDocuments" => p::parent_documents(ctx, a, "issue"),
        "getReleaseDocuments" => p::parent_documents(ctx, a, "release"),
        "getCycleDocuments" => p::parent_documents(ctx, a, "cycle"),
        "getTeamResources" => p::team_resources(ctx, a),
        "searchDocuments" => p::search_documents(ctx, a),
        "getDocumentContentHistory" => p::content_history(ctx, a),
        "createDocument" => p::create_document(ctx, a),
        "updateDocument" => p::update_document(ctx, a),
        "archiveDocument" => p::archive_document(ctx, a, true),
        "unarchiveDocument" => p::archive_document(ctx, a, false),
        // server & oauth
        "getRateLimitStatus" => m::rate_limit_status(ctx, a),
        "getServerStatus" => m::server_status(ctx, a),
        "generateOAuthApplicationSetup" => m::oauth_setup(ctx, a),
        "generateOAuthAuthorizationUrl" => m::oauth_url(ctx, a),
        "createOAuthClientCredentialsToken" => m::oauth_client_token(ctx, a),
        "getManagedOAuthApplications" => m::oauth_apps(ctx, a),
        "getManagedOAuthApplicationById" => m::oauth_app(ctx, a),
        "createManagedOAuthApplication" => m::create_oauth_app(ctx, a),
        "updateManagedOAuthApplication" => m::update_oauth_app(ctx, a),
        "archiveManagedOAuthApplication" => m::archive_oauth_app(ctx, a),
        "rotateManagedOAuthApplicationSecret" => m::rotate_oauth_secret(ctx, a, false),
        "rotateManagedOAuthApplicationWebhookSecret" => m::rotate_oauth_secret(ctx, a, true),
        // ops
        "getWebhookById" => m::get_webhook(ctx, a),
        "getWebhooks" => m::get_webhooks(ctx, a),
        "createWebhook" => m::create_webhook(ctx, a),
        "updateWebhook" => m::update_webhook(ctx, a),
        "rotateWebhookSecret" => m::rotate_webhook_secret(ctx, a),
        "deleteWebhook" => m::delete_webhook(ctx, a),
        "getAttachments" => i::get_attachments(ctx, a),
        "addAttachment" => i::add_attachment(ctx, a),
        "getNotifications" => m::get_notifications(ctx, a),
        "markNotificationAsRead" => m::mark_read(ctx, a),
        "getSubscriptions" => m::get_subscriptions(ctx, a),
        "markAllNotificationsAsRead" => m::mark_all_read(ctx, a),
        "getUnreadNotificationCount" => m::unread_count(ctx, a),
        "getAuthenticationSessions" => m::get_sessions(ctx, a),
        "logoutSession" => m::logout(ctx, a, "one"),
        "logoutOtherSessions" => m::logout(ctx, a, "others"),
        "logoutAllSessions" => m::logout(ctx, a, "all"),
        "getOrganizationAuditEvents" => m::org_audit(ctx, a),
        "getUserAuditEvents" => m::user_audit(ctx, a),
        "getIntegrations" => m::integrations(ctx, a),
        // views
        "getSavedViews" => m::get_views(ctx, a),
        "createSavedView" => m::create_view(ctx, a),
        "updateSavedView" => m::update_view(ctx, a),
        "deleteSavedView" => m::delete_view(ctx, a),
        "getFavoriteViews" => m::favorite_views(ctx, a),
        "addToFavorites" => m::add_favorite(ctx, a),
        "removeFromFavorites" => m::remove_favorite(ctx, a),
        // cycles
        "getCycles" => t::get_cycles(ctx, a),
        "getCycleById" => t::get_cycle(ctx, a),
        "createCycle" => t::create_cycle(ctx, a),
        "updateCycle" => t::update_cycle(ctx, a),
        "completeCycle" => t::complete_cycle(ctx, a),
        "getCycleStats" => t::cycle_stats(ctx, a),
        "getActiveCycle" => t::get_active_cycle(ctx, a),
        "getCycleIssues" => t::get_cycle_issues(ctx, a),
        "addIssueToCycle" => t::add_issue_to_cycle(ctx, a),
        "removeIssueFromCycle" => t::remove_issue_from_cycle(ctx, a),
        // initiatives
        "getInitiatives" => p::get_initiatives(ctx, a),
        "getInitiativeById" => p::get_initiative(ctx, a),
        "createInitiative" => p::create_initiative(ctx, a),
        "updateInitiative" => p::update_initiative(ctx, a),
        "archiveInitiative" => p::archive_initiative(ctx, a, true),
        "unarchiveInitiative" => p::archive_initiative(ctx, a, false),
        "deleteInitiative" => p::delete_initiative(ctx, a),
        "getInitiativeProjects" => p::get_initiative_projects(ctx, a),
        "getInitiativeUpdateById" => p::get_update(ctx, a, true),
        "getInitiativeUpdates" => p::list_updates(ctx, a, true),
        "createInitiativeUpdate" => p::create_update(ctx, a, true),
        "updateInitiativeUpdate" => p::edit_update(ctx, a, true),
        "archiveInitiativeUpdate" => p::archive_update(ctx, a, true, true),
        "unarchiveInitiativeUpdate" => p::archive_update(ctx, a, true, false),
        "addProjectToInitiative" => p::add_project_to_initiative(ctx, a),
        "removeProjectFromInitiative" => p::remove_project_from_initiative(ctx, a),
        // customers
        "getCustomers" => m::get_customers(ctx, a),
        "getCustomerById" => m::get_customer(ctx, a),
        "createCustomer" => m::create_customer(ctx, a),
        "updateCustomer" => m::update_customer(ctx, a),
        "deleteCustomer" => m::delete_customer(ctx, a),
        "getCustomerNeeds" => m::get_needs(ctx, a),
        "getCustomerNeedById" => m::get_need(ctx, a),
        "createCustomerNeed" => m::create_need(ctx, a),
        "createCustomerNeedFromAttachment" => m::create_need_from_attachment(ctx, a),
        "updateCustomerNeed" => m::update_need(ctx, a),
        "archiveCustomerNeed" => m::archive_need(ctx, a, Some(true)),
        "unarchiveCustomerNeed" => m::archive_need(ctx, a, Some(false)),
        "deleteCustomerNeed" => m::archive_need(ctx, a, None),
        "getCustomerStatuses" => m::get_customer_statuses(ctx, a),
        "getCustomerTiers" => m::get_customer_tiers(ctx, a),
        // issues
        "getIssues" => i::get_issues(ctx, a),
        "getIssueById" => i::get_issue_by_id(ctx, a),
        "getCustomFields" => i::get_custom_fields(ctx, a),
        "getIssueCustomFields" => i::get_issue_custom_fields(ctx, a),
        "searchIssues" => i::search_issues(ctx, a),
        "createIssue" => i::create_issue(ctx, a),
        "updateIssue" => i::update_issue(ctx, a),
        "updateIssueCustomField" => i::update_issue_custom_field(ctx, a),
        "createComment" => i::create_comment(ctx, a),
        "updateComment" => i::update_comment(ctx, a),
        "deleteComment" => i::delete_comment(ctx, a),
        "addIssueLabel" => i::add_label(ctx, a, true),
        "removeIssueLabel" => i::add_label(ctx, a, false),
        "getIssueTemplates" => i::get_templates(ctx, a),
        "getIssueTemplateById" => i::get_template(ctx, a),
        "createIssueTemplate" => i::create_template(ctx, a),
        "updateIssueTemplate" => i::update_template(ctx, a),
        "createIssueFromTemplate" => i::create_from_template(ctx, a),
        "getTeamTemplates" => i::get_team_templates(ctx, a),
        "archiveTemplate" => i::archive_template(ctx, a),
        "assignIssue" => i::assign_issue(ctx, a),
        "subscribeToIssue" => i::subscribe(ctx, a),
        "convertIssueToSubtask" => i::convert_to_subtask(ctx, a),
        "createIssueRelation" => i::create_relation(ctx, a),
        "deleteIssueRelation" => i::delete_relation(ctx, a),
        "archiveIssue" => i::archive_issue(ctx, a),
        "setIssuePriority" => i::set_priority(ctx, a),
        "transferIssue" => i::transfer_issue(ctx, a),
        "duplicateIssue" => i::duplicate_issue(ctx, a),
        "getIssueHistory" => i::issue_history(ctx, a),
        "getComments" => i::get_comments(ctx, a),
        _ => Err(format!("Unknown tool: {name}")),
    }
}

/// Runs one tool call against a service state; the reply is an MCP `CallToolResult`.
pub fn call_tool(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    let args = if args.is_null() {
        json_value!({})
    } else {
        args.clone()
    };
    let outcome = validate(name, &args).and_then(|()| dispatch(ctx, name, &args));
    match outcome {
        Ok(result) => {
            let text = serde_json::to_string_pretty(&result).expect("JSON serializes");
            let array_out = schemas()
                .get(name)
                .and_then(|s| s["array_out"].as_bool())
                .unwrap_or(false);
            let structured = match (&result, array_out) {
                (Value::Array(_), true) => Some(json_value!({ "items": result })),
                (Value::Object(_), false) => Some(result.clone()),
                _ => None,
            };
            let mut out = json_value!({ "content": [{ "type": "text", "text": text }] });
            if let Some(sc) = structured {
                out["structuredContent"] = sc;
            }
            out["isError"] = json_value!(false);
            out
        }
        Err(message) => json_value!({
            "content": [{ "type": "text", "text": format!("Error: {message}") }],
            "isError": true,
        }),
    }
}

async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_unix();
    let st = guard.service_mut(NAME);
    let mut ctx = Ctx { st, now };
    Json(call_tool(&mut ctx, &call.name, &call.arguments))
}
