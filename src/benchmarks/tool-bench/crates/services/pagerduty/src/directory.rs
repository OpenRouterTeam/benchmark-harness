//! Users, teams (and memberships), services, escalation policies, priorities, business
//! services and service dependencies.

use axum::extract::{Path, State};
use axum::response::Response;
use mock_core::world::SharedWorld;
use serde_json::{Value, json};

use crate::schedules::schedule_user_ids;
use crate::util::{
    ApiErr, Ctx, Kind, Req, classic, created, default, derived_hex, derived_id, matches_query,
    merge_fields, no_content, ok, ref_id, run, stored_ref,
};

const USER_ROLES: [&str; 8] = [
    "admin",
    "limited_user",
    "observer",
    "owner",
    "read_only_user",
    "restricted_access",
    "read_only_limited_user",
    "user",
];
const TEAM_ROLES: [&str; 3] = ["observer", "responder", "manager"];

/// Sorts by `name` (case-insensitive), then `id`, as PagerDuty's directory lists do.
pub fn sort_by_name(items: &mut [Value]) {
    items.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["name"].as_str().unwrap_or_default().to_ascii_lowercase(),
                v["id"].as_str().unwrap_or_default().to_owned(),
            )
        };
        key(a).cmp(&key(b))
    });
}

fn name_taken(ctx: &Ctx, collection: &str, name: &str, except: Option<&str>) -> bool {
    ctx.docs(collection).iter().any(|d| {
        d["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name.trim()))
            && except.is_none_or(|e| d["id"] != e)
    })
}

fn wrapped<'a>(body: &'a Value, key: &str) -> &'a Value {
    body.get(key).filter(|v| v.is_object()).unwrap_or(body)
}

// ---------------------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------------------

pub fn render_user(ctx: &Ctx, mut doc: Value) -> Value {
    ctx.base_fields(&mut doc, Kind::User);
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    default(&mut doc, "email", json!(format!("{id}@example.com")));
    default(&mut doc, "time_zone", json!("Etc/UTC"));
    default(&mut doc, "color", json!("purple"));
    default(&mut doc, "role", json!("user"));
    default(
        &mut doc,
        "avatar_url",
        json!(format!(
            "https://secure.gravatar.com/avatar/{}.png?d=mm&r=PG",
            derived_hex(&id, 32)
        )),
    );
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "invitation_sent", json!(false));
    default(&mut doc, "job_title", Value::Null);
    default(&mut doc, "teams", json!([]));
    default(&mut doc, "contact_methods", json!([]));
    default(&mut doc, "notification_rules", json!([]));
    default(&mut doc, "coordinated_incidents", json!([]));
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    doc
}

fn user_team_ids(user: &Value) -> Vec<String> {
    user["teams"]
        .as_array()
        .map(|ts| {
            ts.iter()
                .filter_map(|t| ref_id(t).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub async fn get_me(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let me = ctx.me();
        ok(json!({ "user": render_user(ctx, me) }))
    })
}

pub async fn list_users(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let teams = q.list("team_ids");
        let mut users: Vec<Value> = ctx
            .docs("users")
            .into_iter()
            .filter(|u| matches_query(q.get("query"), &[&u["name"], &u["email"]]))
            .filter(|u| teams.is_empty() || user_team_ids(u).iter().any(|t| teams.contains(t)))
            .collect();
        sort_by_name(&mut users);
        let users = users.into_iter().map(|u| render_user(ctx, u)).collect();
        classic(q, "users", users)
    })
}

pub async fn get_user(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let user = ctx.lookup(Kind::User, &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "user": render_user(ctx, user) }))
    })
}

pub async fn create_user(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = wrapped(&body, "user");
        let name = input["name"].as_str().map(str::trim).unwrap_or_default();
        let email = input["email"].as_str().map(str::trim).unwrap_or_default();
        let mut errors = Vec::new();
        if name.is_empty() {
            errors.push("Name can't be blank".to_owned());
        }
        if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
            errors.push("Email is invalid".to_owned());
        }
        let role = input["role"].as_str().unwrap_or("user");
        if !USER_ROLES.contains(&role) {
            errors.push(format!("Role must be one of {}", USER_ROLES.join(", ")));
        }
        if ctx.docs("users").iter().any(|u| {
            u["email"]
                .as_str()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
        }) {
            errors.push("Email has already been taken".to_owned());
        }
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let id = ctx.new_id("users");
        let mut user = json!({
            "id": id,
            "name": name,
            "email": email,
            "role": role,
            "time_zone": input["time_zone"].as_str().unwrap_or("Etc/UTC"),
            "job_title": input.get("job_title").cloned().unwrap_or(Value::Null),
            "description": input.get("description").cloned().unwrap_or(Value::Null),
            "invitation_sent": false,
            "teams": [],
        });
        merge_fields(&mut user, input, &["color"]);
        ctx.put("users", &id, user.clone());
        created(json!({ "user": render_user(ctx, user) }))
    })
}

// ---------------------------------------------------------------------------------------
// Teams
// ---------------------------------------------------------------------------------------

pub fn render_team(ctx: &Ctx, mut doc: Value) -> Value {
    ctx.base_fields(&mut doc, Kind::Team);
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "default_role", json!("manager"));
    default(&mut doc, "parent", Value::Null);
    ctx.fix_ref(&mut doc["parent"], Kind::Team);
    doc
}

pub async fn list_teams(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let mut teams: Vec<Value> = ctx
            .docs("teams")
            .into_iter()
            .filter(|t| matches_query(q.get("query"), &[&t["name"]]))
            .collect();
        sort_by_name(&mut teams);
        let teams = teams.into_iter().map(|t| render_team(ctx, t)).collect();
        classic(q, "teams", teams)
    })
}

pub async fn get_team(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let team = ctx.lookup(Kind::Team, &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "team": render_team(ctx, team) }))
    })
}

fn team_input(input: &Value) -> Result<(), ApiErr> {
    if let Some(role) = input.get("default_role").and_then(Value::as_str)
        && !["manager", "none"].contains(&role)
    {
        return Err(ApiErr::invalid("Default role must be one of manager, none"));
    }
    Ok(())
}

pub async fn create_team(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = wrapped(&body, "team");
        let name = input["name"].as_str().map(str::trim).unwrap_or_default();
        if name.is_empty() {
            return Err(ApiErr::invalid("Name can't be blank"));
        }
        team_input(input)?;
        if name_taken(ctx, "teams", name, None) {
            return Err(ApiErr::invalid("Name has already been taken"));
        }
        let id = ctx.new_id("teams");
        let team = json!({
            "id": id,
            "name": name,
            "description": input.get("description").cloned().unwrap_or(Value::Null),
            "default_role": input["default_role"].as_str().unwrap_or("manager"),
        });
        ctx.put("teams", &id, team.clone());
        created(json!({ "team": render_team(ctx, team) }))
    })
}

pub async fn update_team(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut team = ctx.get("teams", &id).ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let input = wrapped(&body, "team");
        team_input(input)?;
        if let Some(name) = input["name"].as_str() {
            if name.trim().is_empty() {
                return Err(ApiErr::invalid("Name can't be blank"));
            }
            if name_taken(ctx, "teams", name, Some(&id)) {
                return Err(ApiErr::invalid("Name has already been taken"));
            }
            team["name"] = json!(name.trim());
        }
        merge_fields(&mut team, input, &["description", "default_role"]);
        ctx.put("teams", &id, team.clone());
        ok(json!({ "team": render_team(ctx, team) }))
    })
}

fn references_team(doc: &Value, team: &str) -> bool {
    doc["teams"]
        .as_array()
        .is_some_and(|ts| ts.iter().any(|t| t["id"] == team))
}

pub async fn delete_team(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        if !ctx.exists("teams", &id) {
            return Err(ApiErr::NotFound);
        }
        let in_use = ["escalation_policies", "services", "schedules"]
            .iter()
            .any(|c| ctx.docs(c).iter().any(|d| references_team(d, &id)));
        if in_use {
            return Err(ApiErr::invalid(
                "Team has associated escalation policies, services or schedules",
            ));
        }
        ctx.remove("teams", &id);
        for (key, mut user) in ctx.entries("users") {
            if references_team(&user, &id) {
                if let Some(ts) = user["teams"].as_array_mut() {
                    ts.retain(|t| t["id"] != id.as_str());
                }
                ctx.put("users", &key, user);
            }
        }
        for (key, _) in ctx.children("team_members", &id) {
            ctx.remove("team_members", &key);
        }
        no_content()
    })
}

pub async fn list_team_members(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let team = ctx.lookup(Kind::Team, &id).ok_or(ApiErr::NotFound)?;
        let default_role = match team["default_role"].as_str() {
            Some("none") | None => "manager",
            Some(r) => r,
        }
        .to_owned();
        let mut users: Vec<Value> = ctx
            .docs("users")
            .into_iter()
            .filter(|u| references_team(u, &id))
            .collect();
        sort_by_name(&mut users);
        let members = users
            .iter()
            .map(|u| {
                let uid = u["id"].as_str().unwrap_or_default();
                let role = ctx
                    .get("team_members", &format!("{id}/{uid}"))
                    .and_then(|m| m["role"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| default_role.clone());
                json!({ "user": ctx.reference(Kind::User, uid), "role": role })
            })
            .collect();
        classic(&req.query, "members", members)
    })
}

pub async fn add_team_member(
    State(world): State<SharedWorld>,
    Path((team, user)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        if !ctx.exists("teams", &team) {
            return Err(ApiErr::NotFound);
        }
        let mut doc = ctx.get("users", &user).ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let role = body["role"].as_str().unwrap_or("manager").to_owned();
        if !TEAM_ROLES.contains(&role.as_str()) {
            return Err(ApiErr::invalid(
                "Role must be one of observer, responder, manager",
            ));
        }
        if !references_team(&doc, &team) {
            default(&mut doc, "teams", json!([]));
            if let Some(ts) = doc["teams"].as_array_mut() {
                ts.push(stored_ref(Kind::Team, &team));
            }
            ctx.put("users", &user, doc);
        }
        ctx.put(
            "team_members",
            &format!("{team}/{user}"),
            json!({ "user": stored_ref(Kind::User, &user), "role": role }),
        );
        no_content()
    })
}

pub async fn remove_team_member(
    State(world): State<SharedWorld>,
    Path((team, user)): Path<(String, String)>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        if !ctx.exists("teams", &team) {
            return Err(ApiErr::NotFound);
        }
        let mut doc = ctx.get("users", &user).ok_or(ApiErr::NotFound)?;
        if !references_team(&doc, &team) {
            return Err(ApiErr::NotFound);
        }
        if let Some(ts) = doc["teams"].as_array_mut() {
            ts.retain(|t| t["id"] != team.as_str());
        }
        ctx.put("users", &user, doc);
        ctx.remove("team_members", &format!("{team}/{user}"));
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------------------

/// `critical` with an open high-urgency incident, `warning` with any open incident.
fn service_status(ctx: &Ctx, doc: &Value) -> Value {
    let stored = doc["status"].as_str().unwrap_or("active");
    if matches!(stored, "disabled" | "maintenance") {
        return json!(stored);
    }
    let open: Vec<Value> = ctx
        .docs("incidents")
        .into_iter()
        .filter(|i| i["service"]["id"] == doc["id"] && i["status"] != "resolved")
        .collect();
    let status = if open
        .iter()
        .any(|i| i["status"] == "triggered" && i["urgency"].as_str().unwrap_or("high") == "high")
    {
        "critical"
    } else if !open.is_empty() {
        "warning"
    } else {
        "active"
    };
    json!(status)
}

pub fn render_service(ctx: &Ctx, mut doc: Value) -> Value {
    ctx.base_fields(&mut doc, Kind::Service);
    let status = service_status(ctx, &doc);
    doc["status"] = status;
    let last = ctx
        .docs("incidents")
        .iter()
        .filter(|i| i["service"]["id"] == doc["id"])
        .filter_map(|i| i["created_at"].as_str().map(str::to_owned))
        .max();
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "auto_resolve_timeout", json!(14400));
    default(&mut doc, "acknowledgement_timeout", json!(1800));
    default(&mut doc, "created_at", Value::Null);
    let created = doc["created_at"].clone();
    default(&mut doc, "updated_at", created);
    default(&mut doc, "last_incident_timestamp", json!(last));
    default(&mut doc, "teams", json!([]));
    default(&mut doc, "integrations", json!([]));
    default(
        &mut doc,
        "incident_urgency_rule",
        json!({ "type": "constant", "urgency": "high" }),
    );
    default(&mut doc, "support_hours", Value::Null);
    default(&mut doc, "scheduled_actions", json!([]));
    default(
        &mut doc,
        "alert_creation",
        json!("create_alerts_and_incidents"),
    );
    default(
        &mut doc,
        "alert_grouping_parameters",
        json!({ "type": null, "config": null }),
    );
    default(&mut doc, "addons", json!([]));
    ctx.fix_ref(&mut doc["escalation_policy"], Kind::EscalationPolicy);
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    doc
}

pub async fn list_services(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let teams = q.list("team_ids");
        let mut services: Vec<Value> = ctx
            .docs("services")
            .into_iter()
            .filter(|s| matches_query(q.get("query"), &[&s["name"]]))
            .filter(|s| teams.is_empty() || teams.iter().any(|t| references_team(s, t)))
            .collect();
        sort_by_name(&mut services);
        if q.get("sort_by") == Some("name:desc") {
            services.reverse();
        }
        let services = services
            .into_iter()
            .map(|s| render_service(ctx, s))
            .collect();
        classic(q, "services", services)
    })
}

pub async fn get_service(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let service = ctx.lookup(Kind::Service, &id).ok_or(ApiErr::NotFound)?;
        ok(json!({ "service": render_service(ctx, service) }))
    })
}

fn check_service_refs(ctx: &Ctx, input: &Value, errors: &mut Vec<String>) {
    if let Some(ep) = input.get("escalation_policy").filter(|v| !v.is_null()) {
        match ref_id(ep) {
            Some(id) if ctx.exists("escalation_policies", id) => {}
            _ => errors.push("Escalation policy not found".to_owned()),
        }
    }
    if let Some(teams) = input["teams"].as_array() {
        for t in teams {
            if !ref_id(t).is_some_and(|id| ctx.exists("teams", id)) {
                errors.push(format!("Team {} not found", t["id"]));
            }
        }
    }
    if let Some(status) = input["status"].as_str()
        && !["active", "warning", "critical", "maintenance", "disabled"].contains(&status)
    {
        errors.push("Status is not included in the list".to_owned());
    }
}

fn refs_of(input: &Value, kind: Kind) -> Value {
    Value::Array(
        input
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(ref_id)
                    .map(|id| stored_ref(kind, id))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

const SERVICE_FIELDS: [&str; 6] = [
    "description",
    "auto_resolve_timeout",
    "acknowledgement_timeout",
    "incident_urgency_rule",
    "alert_creation",
    "support_hours",
];

pub async fn create_service(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let body = req.json()?;
        let input = wrapped(&body, "service");
        let name = input["name"].as_str().map(str::trim).unwrap_or_default();
        let mut errors = Vec::new();
        if name.is_empty() {
            errors.push("Name can't be blank".to_owned());
        } else if name_taken(ctx, "services", name, None) {
            errors.push("Name has already been taken".to_owned());
        }
        if input.get("escalation_policy").is_none_or(|v| v.is_null()) {
            errors.push("Escalation policy can't be blank".to_owned());
        }
        check_service_refs(ctx, input, &mut errors);
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        let id = ctx.new_id("services");
        let now = ctx.now_str();
        let mut service = json!({
            "id": id,
            "name": name,
            "description": null,
            "status": input["status"].as_str().filter(|s| *s != "warning" && *s != "critical").unwrap_or("active"),
            "escalation_policy": stored_ref(Kind::EscalationPolicy, ref_id(&input["escalation_policy"]).unwrap_or_default()),
            "teams": refs_of(&input["teams"], Kind::Team),
            "created_at": now,
            "updated_at": now,
        });
        merge_fields(&mut service, input, &SERVICE_FIELDS);
        ctx.put("services", &id, service.clone());
        created(json!({ "service": render_service(ctx, service) }))
    })
}

pub async fn update_service(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let mut service = ctx.get("services", &id).ok_or(ApiErr::NotFound)?;
        let body = req.json()?;
        let input = wrapped(&body, "service");
        let mut errors = Vec::new();
        if let Some(name) = input["name"].as_str() {
            if name.trim().is_empty() {
                errors.push("Name can't be blank".to_owned());
            } else if name_taken(ctx, "services", name, Some(&id)) {
                errors.push("Name has already been taken".to_owned());
            }
        }
        check_service_refs(ctx, input, &mut errors);
        if !errors.is_empty() {
            return Err(ApiErr::Invalid(errors));
        }
        if let Some(name) = input["name"].as_str() {
            service["name"] = json!(name.trim());
        }
        if let Some(ep) = input.get("escalation_policy").and_then(ref_id) {
            service["escalation_policy"] = stored_ref(Kind::EscalationPolicy, ep);
        }
        if input["teams"].is_array() {
            service["teams"] = refs_of(&input["teams"], Kind::Team);
        }
        if let Some(status) = input["status"].as_str()
            && ["active", "disabled", "maintenance"].contains(&status)
        {
            service["status"] = json!(status);
        }
        merge_fields(&mut service, input, &SERVICE_FIELDS);
        service["updated_at"] = json!(ctx.now_str());
        ctx.put("services", &id, service.clone());
        ok(json!({ "service": render_service(ctx, service) }))
    })
}

pub async fn delete_service(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        ctx.remove("services", &id).ok_or(ApiErr::NotFound)?;
        no_content()
    })
}

// ---------------------------------------------------------------------------------------
// Escalation policies
// ---------------------------------------------------------------------------------------

pub fn target_kind(target: &Value) -> Kind {
    match target["type"].as_str() {
        Some(t) if t.starts_with("user") => Kind::User,
        Some("schedule_v3_reference") => Kind::ScheduleV3,
        _ => Kind::Schedule,
    }
}

pub fn render_ep(ctx: &Ctx, mut doc: Value) -> Value {
    ctx.base_fields(&mut doc, Kind::EscalationPolicy);
    let id = doc["id"].as_str().unwrap_or_default().to_owned();
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "num_loops", json!(0));
    default(
        &mut doc,
        "on_call_handoff_notifications",
        json!("if_has_services"),
    );
    default(&mut doc, "escalation_rules", json!([]));
    if let Some(rules) = doc["escalation_rules"].as_array_mut() {
        for (i, rule) in rules.iter_mut().enumerate() {
            default(rule, "id", json!(derived_id(&format!("{id}/rule/{i}"))));
            default(rule, "escalation_delay_in_minutes", json!(30));
            default(rule, "targets", json!([]));
            if let Some(targets) = rule["targets"].as_array_mut() {
                for t in targets {
                    let kind = target_kind(t);
                    ctx.fix_ref(t, kind);
                }
            }
        }
    }
    let services: Vec<Value> = ctx
        .docs("services")
        .iter()
        .filter(|s| s["escalation_policy"]["id"] == id.as_str())
        .filter_map(|s| s["id"].as_str())
        .map(|sid| ctx.reference(Kind::Service, sid))
        .collect();
    doc["services"] = json!(services);
    default(&mut doc, "teams", json!([]));
    ctx.fix_refs(&mut doc["teams"], Kind::Team);
    doc
}

/// Users an escalation policy can page: direct user targets plus schedule members.
pub fn ep_user_ids(ctx: &Ctx, ep: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for rule in ep["escalation_rules"].as_array().into_iter().flatten() {
        for t in rule["targets"].as_array().into_iter().flatten() {
            let Some(tid) = ref_id(t) else { continue };
            match target_kind(t) {
                Kind::User => out.push(tid.to_owned()),
                _ => out.extend(schedule_user_ids(ctx, tid)),
            }
        }
    }
    out
}

pub async fn list_escalation_policies(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let q = &req.query;
        let users = q.list("user_ids");
        let teams = q.list("team_ids");
        let mut eps: Vec<Value> = ctx
            .docs("escalation_policies")
            .into_iter()
            .filter(|e| matches_query(q.get("query"), &[&e["name"]]))
            .filter(|e| teams.is_empty() || teams.iter().any(|t| references_team(e, t)))
            .filter(|e| users.is_empty() || ep_user_ids(ctx, e).iter().any(|u| users.contains(u)))
            .collect();
        sort_by_name(&mut eps);
        let eps = eps.into_iter().map(|e| render_ep(ctx, e)).collect();
        classic(q, "escalation_policies", eps)
    })
}

pub async fn get_escalation_policy(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let ep = ctx
            .lookup(Kind::EscalationPolicy, &id)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "escalation_policy": render_ep(ctx, ep) }))
    })
}

// ---------------------------------------------------------------------------------------
// Priorities, business services, service dependencies
// ---------------------------------------------------------------------------------------

pub async fn list_priorities(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let mut priorities = ctx.docs("priorities");
        priorities.sort_by_key(|p| p["order"].as_i64().unwrap_or(i64::MAX));
        let priorities = priorities
            .into_iter()
            .map(|mut p| {
                ctx.base_fields(&mut p, Kind::Priority);
                default(&mut p, "description", json!(""));
                p["html_url"] = Value::Null;
                p
            })
            .collect();
        classic(&req.query, "priorities", priorities)
    })
}

fn render_business_service(ctx: &Ctx, mut doc: Value) -> Value {
    ctx.base_fields(&mut doc, Kind::BusinessService);
    default(&mut doc, "description", Value::Null);
    default(&mut doc, "point_of_contact", Value::Null);
    default(&mut doc, "team", Value::Null);
    ctx.fix_ref(&mut doc["team"], Kind::Team);
    doc
}

pub async fn list_business_services(State(world): State<SharedWorld>, req: Req) -> Response {
    run(&world, &req, |ctx| {
        let mut items = ctx.docs("business_services");
        sort_by_name(&mut items);
        let items = items
            .into_iter()
            .map(|b| render_business_service(ctx, b))
            .collect();
        classic(&req.query, "business_services", items)
    })
}

pub async fn get_business_service(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        let b = ctx
            .lookup(Kind::BusinessService, &id)
            .ok_or(ApiErr::NotFound)?;
        ok(json!({ "business_service": render_business_service(ctx, b) }))
    })
}

fn dependency_side(side: &Value) -> Value {
    let business = side["type"]
        .as_str()
        .is_some_and(|t| t.starts_with("business"));
    json!({
        "id": side["id"],
        "type": if business { "business_service_reference" } else { "technical_service_reference" },
    })
}

fn relationships(ctx: &Ctx, id: &str) -> Vec<Value> {
    ctx.docs("service_dependencies")
        .into_iter()
        .filter(|d| d["supporting_service"]["id"] == id || d["dependent_service"]["id"] == id)
        .map(|d| {
            json!({
                "id": d["id"],
                "type": "service_dependency",
                "supporting_service": dependency_side(&d["supporting_service"]),
                "dependent_service": dependency_side(&d["dependent_service"]),
            })
        })
        .collect()
}

pub async fn technical_service_dependencies(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        if ctx.lookup(Kind::Service, &id).is_none() {
            return Err(ApiErr::NotFound);
        }
        ok(json!({ "relationships": relationships(ctx, &id) }))
    })
}

pub async fn business_service_dependencies(
    State(world): State<SharedWorld>,
    Path(id): Path<String>,
    req: Req,
) -> Response {
    run(&world, &req, |ctx| {
        if ctx.lookup(Kind::BusinessService, &id).is_none() {
            return Err(ApiErr::NotFound);
        }
        ok(json!({ "relationships": relationships(ctx, &id) }))
    })
}

/// Services this one depends on or that depend on it (either direction).
pub fn related_service_ids(ctx: &Ctx, id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in ctx.docs("service_dependencies") {
        let (s, t) = (
            &d["supporting_service"]["id"],
            &d["dependent_service"]["id"],
        );
        if s == id
            && let Some(x) = t.as_str()
        {
            out.push(x.to_owned());
        }
        if t == id
            && let Some(x) = s.as_str()
        {
            out.push(x.to_owned());
        }
    }
    out
}
