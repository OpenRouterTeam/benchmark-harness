//! IAM (query protocol). Collections:
//!
//! - `users` keyed by `UserName`: IAM user fields plus `AttachedPolicies` (ARNs),
//!   `InlinePolicies` (`{name: document}`), `Tags`, `MFADevices`, `LoginProfile`.
//! - `groups` keyed by `GroupName`: plus `Members` (user names), `AttachedPolicies`, `InlinePolicies`.
//! - `roles` keyed by `RoleName`: plus `AssumeRolePolicyDocument` (object), `AttachedPolicies`, `InlinePolicies`.
//! - `policies` keyed by ARN: customer managed policies with `Document` (object), optional `Versions`.
//!   AWS managed policies are built in (see [`aws_managed`]) and may be overridden here.
//! - `access_keys` keyed by `AccessKeyId`: `UserName`, `Status`, `CreateDate`, optional `LastUsedDate`/`LastUsedService`.
//!
//! Policy documents are returned URL-encoded, as IAM does (botocore decodes them).

use axum::{http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::{
    Ctx, Req, query,
    util::{Params, el, glob_match, iso, percent_encode, s, tel, to_xml, value_time},
};

const NS: &str = "https://iam.amazonaws.com/doc/2010-05-08/";

type Out = Result<Option<String>, (StatusCode, &'static str, String)>;

fn not_found(msg: String) -> (StatusCode, &'static str, String) {
    (StatusCode::NOT_FOUND, "NoSuchEntity", msg)
}

fn conflict(code: &'static str, msg: impl Into<String>) -> (StatusCode, &'static str, String) {
    (StatusCode::CONFLICT, code, msg.into())
}

fn invalid(msg: impl Into<String>) -> (StatusCode, &'static str, String) {
    (StatusCode::BAD_REQUEST, "ValidationError", msg.into())
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let p = Params::parse(&req.body_text());
    let action = p.str("Action");
    let result = run(ctx, &action, &p);
    let rid = ctx.request_id();
    match result {
        Ok(body) => query::ok(NS, &action, body, &rid),
        Err((status, code, msg)) => query::error(NS, status, code, &msg, &rid),
    }
}

fn run(ctx: &mut Ctx, action: &str, p: &Params) -> Out {
    match action {
        "ListUsers" => list_users(ctx, p),
        "GetUser" => get_user(ctx, p),
        "CreateUser" => create_user(ctx, p),
        "DeleteUser" => delete_user(ctx, p),
        "UpdateUser" => update_user(ctx, p),
        "TagUser" => tag_entity(ctx, "users", &p.str("UserName"), p, "user"),
        "UntagUser" => untag_entity(ctx, "users", &p.str("UserName"), p, "user"),
        "ListUserTags" => list_tags(ctx, "users", &p.str("UserName"), "user"),
        "ListRoles" => list_roles(ctx, p),
        "GetRole" => get_role(ctx, p),
        "CreateRole" => create_role(ctx, p),
        "DeleteRole" => delete_role(ctx, p),
        "UpdateRole" | "UpdateRoleDescription" => update_role(ctx, p),
        "TagRole" => tag_entity(ctx, "roles", &p.str("RoleName"), p, "role"),
        "UntagRole" => untag_entity(ctx, "roles", &p.str("RoleName"), p, "role"),
        "ListRoleTags" => list_tags(ctx, "roles", &p.str("RoleName"), "role"),
        "ListGroups" => list_groups(ctx, p),
        "GetGroup" => get_group(ctx, p),
        "CreateGroup" => create_group(ctx, p),
        "DeleteGroup" => delete_group(ctx, p),
        "AddUserToGroup" => add_user_to_group(ctx, p),
        "RemoveUserFromGroup" => remove_user_from_group(ctx, p),
        "ListGroupsForUser" => list_groups_for_user(ctx, p),
        "ListPolicies" => list_policies(ctx, p),
        "GetPolicy" => get_policy(ctx, p),
        "GetPolicyVersion" => get_policy_version(ctx, p),
        "ListPolicyVersions" => list_policy_versions(ctx, p),
        "CreatePolicy" => create_policy(ctx, p),
        "CreatePolicyVersion" => create_policy_version(ctx, p),
        "DeletePolicy" => delete_policy(ctx, p),
        "ListEntitiesForPolicy" => list_entities_for_policy(ctx, p),
        "AttachUserPolicy" => attach(ctx, "users", &p.str("UserName"), p, true),
        "DetachUserPolicy" => attach(ctx, "users", &p.str("UserName"), p, false),
        "AttachGroupPolicy" => attach(ctx, "groups", &p.str("GroupName"), p, true),
        "DetachGroupPolicy" => attach(ctx, "groups", &p.str("GroupName"), p, false),
        "AttachRolePolicy" => attach(ctx, "roles", &p.str("RoleName"), p, true),
        "DetachRolePolicy" => attach(ctx, "roles", &p.str("RoleName"), p, false),
        "ListAttachedUserPolicies" => list_attached(ctx, "users", &p.str("UserName"), p),
        "ListAttachedGroupPolicies" => list_attached(ctx, "groups", &p.str("GroupName"), p),
        "ListAttachedRolePolicies" => list_attached(ctx, "roles", &p.str("RoleName"), p),
        "PutUserPolicy" => put_inline(ctx, "users", &p.str("UserName"), p),
        "PutGroupPolicy" => put_inline(ctx, "groups", &p.str("GroupName"), p),
        "PutRolePolicy" => put_inline(ctx, "roles", &p.str("RoleName"), p),
        "GetUserPolicy" => get_inline(ctx, "users", "UserName", p),
        "GetGroupPolicy" => get_inline(ctx, "groups", "GroupName", p),
        "GetRolePolicy" => get_inline(ctx, "roles", "RoleName", p),
        "DeleteUserPolicy" => delete_inline(ctx, "users", &p.str("UserName"), p),
        "DeleteGroupPolicy" => delete_inline(ctx, "groups", &p.str("GroupName"), p),
        "DeleteRolePolicy" => delete_inline(ctx, "roles", &p.str("RoleName"), p),
        "ListUserPolicies" => list_inline(ctx, "users", &p.str("UserName"), p),
        "ListGroupPolicies" => list_inline(ctx, "groups", &p.str("GroupName"), p),
        "ListRolePolicies" => list_inline(ctx, "roles", &p.str("RoleName"), p),
        "CreateAccessKey" => create_access_key(ctx, p),
        "DeleteAccessKey" => delete_access_key(ctx, p),
        "UpdateAccessKey" => update_access_key(ctx, p),
        "ListAccessKeys" => list_access_keys(ctx, p),
        "GetAccessKeyLastUsed" => access_key_last_used(ctx, p),
        "SimulatePrincipalPolicy" => simulate(ctx, p),
        "ListMFADevices" => list_mfa(ctx, p),
        "GetLoginProfile" => get_login_profile(ctx, p),
        "DeleteLoginProfile" => delete_login_profile(ctx, p),
        "ListAccountAliases" => {
            let alias = account_alias(ctx);
            Ok(Some(format!(
                "<AccountAliases>{}</AccountAliases><IsTruncated>false</IsTruncated>",
                alias.map(|a| tel("member", &a)).unwrap_or_default()
            )))
        }
        "GetAccountSummary" => account_summary(ctx),
        "GetAccountPasswordPolicy" => Err(not_found(format!(
            "The Password Policy with domain name {} cannot be found.",
            ctx.account
        ))),
        "ListInstanceProfiles" | "ListInstanceProfilesForRole" => Ok(Some(
            "<InstanceProfiles/><IsTruncated>false</IsTruncated>".into(),
        )),
        "ListSAMLProviders" => Ok(Some("<SAMLProviderList/>".into())),
        "ListOpenIDConnectProviders" => Ok(Some("<OpenIDConnectProviderList/>".into())),
        "ListServerCertificates" => Ok(Some(
            "<ServerCertificateMetadataList/><IsTruncated>false</IsTruncated>".into(),
        )),
        "ListVirtualMFADevices" => Ok(Some(
            "<VirtualMFADevices/><IsTruncated>false</IsTruncated>".into(),
        )),
        "ListSigningCertificates" | "ListSSHPublicKeys" | "ListServiceSpecificCredentials" => {
            Ok(Some(empty_list_for(action)))
        }
        "" => Err((
            StatusCode::BAD_REQUEST,
            "MissingAction",
            "Missing Action".into(),
        )),
        other => Err((
            StatusCode::BAD_REQUEST,
            "InvalidAction",
            format!("Could not find operation {other} for version 2010-05-08"),
        )),
    }
}

fn empty_list_for(action: &str) -> String {
    match action {
        "ListSigningCertificates" => "<Certificates/><IsTruncated>false</IsTruncated>".into(),
        "ListSSHPublicKeys" => "<SSHPublicKeys/><IsTruncated>false</IsTruncated>".into(),
        _ => "<ServiceSpecificCredentials/>".into(),
    }
}

fn account_alias(ctx: &Ctx) -> Option<String> {
    ctx.docs("account")
        .first()
        .and_then(|(_, a)| a.get("Alias").and_then(Value::as_str).map(str::to_owned))
}

// ───────────────────────────── helpers ─────────────────────────────

fn arn(ctx: &Ctx, kind: &str, path: &str, name: &str) -> String {
    format!("arn:aws:iam::{}:{kind}{path}{name}", ctx.account)
}

fn upper_id(ctx: &mut Ctx, kind: &str, prefix: &str, len: usize) -> String {
    let raw = ctx.svc.ids.next_prefixed(kind, "", len);
    format!("{prefix}{}", raw.to_ascii_uppercase())
}

fn ts(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(value_time).map(iso)
}

fn page(items: Vec<String>, p: &Params, item_xml: &str) -> String {
    let max = p.int("MaxItems").unwrap_or(100).max(1) as usize;
    let start = p
        .get("Marker")
        .and_then(|m| m.trim_start_matches("mk").parse::<usize>().ok())
        .unwrap_or(0);
    let total = items.len();
    let slice: String = items
        .into_iter()
        .skip(start)
        .take(max)
        .map(|i| el("member", &i))
        .collect();
    let truncated = start + max < total;
    let marker = if truncated {
        tel("Marker", &format!("mk{}", start + max))
    } else {
        String::new()
    };
    format!("<{item_xml}>{slice}</{item_xml}><IsTruncated>{truncated}</IsTruncated>{marker}")
}

fn need<'a>(p: &'a Params, key: &str) -> Result<&'a str, (StatusCode, &'static str, String)> {
    match p.get(key) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(invalid(format!(
            "1 validation error detected: Value null at '{}' failed to satisfy constraint: Member must not be null",
            lower_first(key)
        ))),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn kind_name(collection: &str) -> &'static str {
    match collection {
        "users" => "user",
        "groups" => "group",
        _ => "role",
    }
}

fn entity(
    ctx: &Ctx,
    collection: &str,
    name: &str,
) -> Result<Value, (StatusCode, &'static str, String)> {
    ctx.get(collection, name).ok_or_else(|| {
        not_found(format!(
            "The {} with name {name} cannot be found.",
            kind_name(collection)
        ))
    })
}

fn tags_xml(v: &Value) -> String {
    match v.get("Tags") {
        Some(Value::Array(t)) if !t.is_empty() => {
            el("Tags", &to_xml(&Value::Array(t.clone()), "member"))
        }
        _ => String::new(),
    }
}

fn user_xml(u: &Value, with_tags: bool) -> String {
    let mut x = String::new();
    x += &tel("Path", &str_or(u, "Path", "/"));
    x += &tel("UserName", &s(u, "UserName"));
    x += &tel("UserId", &s(u, "UserId"));
    x += &tel("Arn", &s(u, "Arn"));
    x += &tel("CreateDate", &ts(u, "CreateDate").unwrap_or_default());
    if let Some(t) = ts(u, "PasswordLastUsed") {
        x += &tel("PasswordLastUsed", &t);
    }
    if let Some(pb) = u.get("PermissionsBoundary").filter(|v| !v.is_null()) {
        x += &el("PermissionsBoundary", &to_xml(pb, "member"));
    }
    if with_tags {
        x += &tags_xml(u);
    }
    x
}

fn str_or(v: &Value, key: &str, default: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

fn doc_text(doc: &Value) -> String {
    match doc {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).expect("JSON serializes"),
    }
}

fn parse_doc(text: &str) -> Result<Value, (StatusCode, &'static str, String)> {
    let v: Value = serde_json::from_str(text).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "MalformedPolicyDocument",
            "Syntax errors in policy.".to_owned(),
        )
    })?;
    if !v.is_object() || v.get("Statement").is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "MalformedPolicyDocument",
            "Policy document should be an object with a Statement element.".to_owned(),
        ));
    }
    Ok(v)
}

fn array_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ───────────────────────────── users ─────────────────────────────

fn list_users(ctx: &mut Ctx, p: &Params) -> Out {
    let prefix = p.get("PathPrefix").unwrap_or("/");
    let users = ctx
        .docs("users")
        .into_iter()
        .filter(|(_, u)| str_or(u, "Path", "/").starts_with(prefix))
        .map(|(_, u)| user_xml(&u, false))
        .collect();
    Ok(Some(page(users, p, "Users")))
}

fn get_user(ctx: &mut Ctx, p: &Params) -> Out {
    let name = match p.get("UserName") {
        Some(n) => n.to_owned(),
        None => ctx
            .docs("account")
            .first()
            .map(|(_, a)| s(a, "UserName"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "ops-automation".into()),
    };
    let user = match ctx.get("users", &name) {
        Some(u) => u,
        None if p.get("UserName").is_none() => json!({
            "UserName": name, "UserId": "AIDATOOLBENCH0000000", "Path": "/",
            "Arn": arn(ctx, "user", "/", &name), "CreateDate": "2024-01-01T00:00:00Z"
        }),
        None => return Err(entity(ctx, "users", &name).unwrap_err()),
    };
    Ok(Some(el("User", &user_xml(&user, true))))
}

fn create_user(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "UserName")?.to_owned();
    if ctx.get("users", &name).is_some() {
        return Err(conflict(
            "EntityAlreadyExists",
            format!("User with name {name} already exists."),
        ));
    }
    let path = p.get("Path").unwrap_or("/").to_owned();
    let user_id = upper_id(ctx, "user", "AIDA", 17);
    let mut user = json!({
        "UserName": name,
        "UserId": user_id,
        "Arn": arn(ctx, "user", &path, &name),
        "Path": path,
        "CreateDate": iso(ctx.now),
        "Tags": tags_param(p),
        "AttachedPolicies": [],
        "InlinePolicies": {},
    });
    if let Some(pb) = p.get("PermissionsBoundary") {
        user["PermissionsBoundary"] =
            json!({"PermissionsBoundaryType": "Policy", "PermissionsBoundaryArn": pb});
    }
    ctx.put("users", &name, user.clone());
    Ok(Some(el("User", &user_xml(&user, true))))
}

fn tags_param(p: &Params) -> Value {
    Value::Array(
        p.indexes("Tags.member")
            .into_iter()
            .map(|n| {
                json!({
                    "Key": p.str(&format!("Tags.member.{n}.Key")),
                    "Value": p.str(&format!("Tags.member.{n}.Value")),
                })
            })
            .collect(),
    )
}

fn delete_user(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "UserName")?.to_owned();
    let user = entity(ctx, "users", &name)?;
    let has_keys = ctx
        .docs("access_keys")
        .iter()
        .any(|(_, k)| s(k, "UserName") == name);
    if has_keys {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must delete access keys first.",
        ));
    }
    if !array_of(&user, "AttachedPolicies").is_empty() {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must detach all policies first.",
        ));
    }
    if user
        .get("InlinePolicies")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty())
    {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must delete policies first.",
        ));
    }
    let in_group = ctx
        .docs("groups")
        .iter()
        .any(|(_, g)| array_of(g, "Members").contains(&name));
    if in_group {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must remove users from group first.",
        ));
    }
    if user.get("LoginProfile").is_some_and(|v| !v.is_null()) {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must delete login profile first.",
        ));
    }
    ctx.remove("users", &name);
    Ok(None)
}

fn update_user(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "UserName")?.to_owned();
    let mut user = entity(ctx, "users", &name)?;
    if let Some(path) = p.get("NewPath") {
        user["Path"] = json!(path);
    }
    let new_name = p.get("NewUserName").unwrap_or(&name).to_owned();
    user["UserName"] = json!(new_name);
    let path = str_or(&user, "Path", "/");
    user["Arn"] = json!(arn(ctx, "user", &path, &new_name));
    ctx.remove("users", &name);
    ctx.put("users", &new_name, user);
    Ok(None)
}

fn tag_entity(ctx: &mut Ctx, collection: &str, name: &str, p: &Params, _kind: &str) -> Out {
    let mut doc = entity(ctx, collection, name)?;
    let mut tags: Vec<Value> = doc
        .get("Tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for t in tags_param(p).as_array().cloned().unwrap_or_default() {
        if let Some(existing) = tags.iter_mut().find(|e| e["Key"] == t["Key"]) {
            *existing = t;
        } else {
            tags.push(t);
        }
    }
    doc["Tags"] = Value::Array(tags);
    ctx.put(collection, name, doc);
    Ok(None)
}

fn untag_entity(ctx: &mut Ctx, collection: &str, name: &str, p: &Params, _kind: &str) -> Out {
    let mut doc = entity(ctx, collection, name)?;
    let keys = p.list("TagKeys.member");
    if let Some(tags) = doc.get_mut("Tags").and_then(Value::as_array_mut) {
        tags.retain(|t| !keys.iter().any(|k| t["Key"] == k.as_str()));
    }
    ctx.put(collection, name, doc);
    Ok(None)
}

fn list_tags(ctx: &mut Ctx, collection: &str, name: &str, _kind: &str) -> Out {
    let doc = entity(ctx, collection, name)?;
    let tags = doc.get("Tags").cloned().unwrap_or(json!([]));
    Ok(Some(format!(
        "<Tags>{}</Tags><IsTruncated>false</IsTruncated>",
        to_xml(&tags, "member")
    )))
}

// ───────────────────────────── roles ─────────────────────────────

fn role_xml(ctx: &Ctx, r: &Value, detail: bool) -> String {
    let mut x = String::new();
    x += &tel("Path", &str_or(r, "Path", "/"));
    x += &tel("RoleName", &s(r, "RoleName"));
    x += &tel("RoleId", &s(r, "RoleId"));
    x += &tel(
        "Arn",
        &r.get("Arn")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| arn(ctx, "role", &str_or(r, "Path", "/"), &s(r, "RoleName"))),
    );
    x += &tel("CreateDate", &ts(r, "CreateDate").unwrap_or_default());
    if let Some(doc) = r.get("AssumeRolePolicyDocument").filter(|d| !d.is_null()) {
        x += &tel("AssumeRolePolicyDocument", &percent_encode(&doc_text(doc)));
    }
    if let Some(d) = r.get("Description").and_then(Value::as_str) {
        x += &tel("Description", d);
    }
    x += &el(
        "MaxSessionDuration",
        &r.get("MaxSessionDuration")
            .and_then(Value::as_i64)
            .unwrap_or(3600)
            .to_string(),
    );
    if detail {
        x += &tags_xml(r);
        if let Some(lu) = r.get("RoleLastUsed").filter(|v| v.is_object()) {
            let mut l = String::new();
            if let Some(t) = ts(lu, "LastUsedDate") {
                l += &tel("LastUsedDate", &t);
            }
            if let Some(reg) = lu.get("Region").and_then(Value::as_str) {
                l += &tel("Region", reg);
            }
            x += &el("RoleLastUsed", &l);
        } else {
            x += "<RoleLastUsed/>";
        }
    }
    x
}

fn list_roles(ctx: &mut Ctx, p: &Params) -> Out {
    let prefix = p.get("PathPrefix").unwrap_or("/");
    let roles = ctx
        .docs("roles")
        .into_iter()
        .filter(|(_, r)| str_or(r, "Path", "/").starts_with(prefix))
        .map(|(_, r)| role_xml(ctx, &r, false))
        .collect();
    Ok(Some(page(roles, p, "Roles")))
}

fn get_role(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "RoleName")?;
    let role = entity(ctx, "roles", name)?;
    Ok(Some(el("Role", &role_xml(ctx, &role, true))))
}

fn create_role(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "RoleName")?.to_owned();
    let trust = parse_doc(need(p, "AssumeRolePolicyDocument")?)?;
    if ctx.get("roles", &name).is_some() {
        return Err(conflict(
            "EntityAlreadyExists",
            format!("Role with name {name} already exists."),
        ));
    }
    let path = p.get("Path").unwrap_or("/").to_owned();
    let role_id = upper_id(ctx, "role", "AROA", 17);
    let mut role = json!({
        "RoleName": name,
        "RoleId": role_id,
        "Arn": arn(ctx, "role", &path, &name),
        "Path": path,
        "CreateDate": iso(ctx.now),
        "AssumeRolePolicyDocument": trust,
        "MaxSessionDuration": p.int("MaxSessionDuration").unwrap_or(3600),
        "Tags": tags_param(p),
        "AttachedPolicies": [],
        "InlinePolicies": {},
    });
    if let Some(d) = p.get("Description") {
        role["Description"] = json!(d);
    }
    ctx.put("roles", &name, role.clone());
    Ok(Some(el("Role", &role_xml(ctx, &role, true))))
}

fn delete_role(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "RoleName")?.to_owned();
    let role = entity(ctx, "roles", &name)?;
    if !array_of(&role, "AttachedPolicies").is_empty() {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must detach all policies first.",
        ));
    }
    ctx.remove("roles", &name);
    Ok(None)
}

fn update_role(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "RoleName")?.to_owned();
    let mut role = entity(ctx, "roles", &name)?;
    if let Some(d) = p.get("Description") {
        role["Description"] = json!(d);
    }
    if let Some(m) = p.int("MaxSessionDuration") {
        role["MaxSessionDuration"] = json!(m);
    }
    ctx.put("roles", &name, role.clone());
    if p.get("Action") == Some("UpdateRoleDescription") {
        return Ok(Some(el("Role", &role_xml(ctx, &role, true))));
    }
    Ok(Some(String::new()))
}

// ───────────────────────────── groups ─────────────────────────────

fn group_xml(ctx: &Ctx, g: &Value) -> String {
    let path = str_or(g, "Path", "/");
    let name = s(g, "GroupName");
    format!(
        "{}{}{}{}{}",
        tel("Path", &path),
        tel("GroupName", &name),
        tel("GroupId", &s(g, "GroupId")),
        tel(
            "Arn",
            &g.get("Arn")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| arn(ctx, "group", &path, &name))
        ),
        tel("CreateDate", &ts(g, "CreateDate").unwrap_or_default())
    )
}

fn list_groups(ctx: &mut Ctx, p: &Params) -> Out {
    let prefix = p.get("PathPrefix").unwrap_or("/");
    let groups = ctx
        .docs("groups")
        .into_iter()
        .filter(|(_, g)| str_or(g, "Path", "/").starts_with(prefix))
        .map(|(_, g)| group_xml(ctx, &g))
        .collect();
    Ok(Some(page(groups, p, "Groups")))
}

fn get_group(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "GroupName")?;
    let group = entity(ctx, "groups", name)?;
    let users = array_of(&group, "Members")
        .iter()
        .filter_map(|u| ctx.get("users", u))
        .map(|u| user_xml(&u, false))
        .collect();
    Ok(Some(format!(
        "<Group>{}</Group>{}",
        group_xml(ctx, &group),
        page(users, p, "Users")
    )))
}

fn create_group(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "GroupName")?.to_owned();
    if ctx.get("groups", &name).is_some() {
        return Err(conflict(
            "EntityAlreadyExists",
            format!("Group with name {name} already exists."),
        ));
    }
    let path = p.get("Path").unwrap_or("/").to_owned();
    let gid = upper_id(ctx, "group", "AGPA", 17);
    let group = json!({
        "GroupName": name, "GroupId": gid, "Arn": arn(ctx, "group", &path, &name),
        "Path": path, "CreateDate": iso(ctx.now), "Members": [], "AttachedPolicies": [], "InlinePolicies": {},
    });
    ctx.put("groups", &name, group.clone());
    Ok(Some(el("Group", &group_xml(ctx, &group))))
}

fn delete_group(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "GroupName")?.to_owned();
    let group = entity(ctx, "groups", &name)?;
    if !array_of(&group, "Members").is_empty() {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must remove users from group first.",
        ));
    }
    if !array_of(&group, "AttachedPolicies").is_empty() {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete entity, must detach all policies first.",
        ));
    }
    ctx.remove("groups", &name);
    Ok(None)
}

fn add_user_to_group(ctx: &mut Ctx, p: &Params) -> Out {
    let gname = need(p, "GroupName")?.to_owned();
    let uname = need(p, "UserName")?.to_owned();
    let mut group = entity(ctx, "groups", &gname)?;
    entity(ctx, "users", &uname)?;
    let mut members = array_of(&group, "Members");
    if !members.contains(&uname) {
        members.push(uname);
    }
    group["Members"] = json!(members);
    ctx.put("groups", &gname, group);
    Ok(None)
}

fn remove_user_from_group(ctx: &mut Ctx, p: &Params) -> Out {
    let gname = need(p, "GroupName")?.to_owned();
    let uname = need(p, "UserName")?.to_owned();
    let mut group = entity(ctx, "groups", &gname)?;
    entity(ctx, "users", &uname)?;
    let mut members = array_of(&group, "Members");
    if !members.contains(&uname) {
        return Err(not_found(format!(
            "The user with name {uname} cannot be found."
        )));
    }
    members.retain(|m| *m != uname);
    group["Members"] = json!(members);
    ctx.put("groups", &gname, group);
    Ok(None)
}

fn list_groups_for_user(ctx: &mut Ctx, p: &Params) -> Out {
    let uname = need(p, "UserName")?.to_owned();
    entity(ctx, "users", &uname)?;
    let groups = ctx
        .docs("groups")
        .into_iter()
        .filter(|(_, g)| array_of(g, "Members").contains(&uname))
        .map(|(_, g)| group_xml(ctx, &g))
        .collect();
    Ok(Some(page(groups, p, "Groups")))
}

// ───────────────────────────── managed policies ─────────────────────────────

/// A handful of AWS managed policies, always present (documents abbreviated).
pub fn aws_managed() -> Vec<Value> {
    let p = |name: &str, path: &str, id: &str, desc: &str, actions: Value| {
        json!({
            "PolicyName": name,
            "PolicyId": id,
            "Arn": format!("arn:aws:iam::aws:policy{path}{name}"),
            "Path": path,
            "DefaultVersionId": "v1",
            "IsAttachable": true,
            "Description": desc,
            "CreateDate": "2015-02-06T18:39:46Z",
            "UpdateDate": "2024-06-12T10:00:00Z",
            "Document": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}]},
        })
    };
    vec![
        p(
            "AdministratorAccess",
            "/",
            "ANPAIWMBCKSKIEE64ZLYK",
            "Provides full access to AWS services and resources.",
            json!("*"),
        ),
        p(
            "PowerUserAccess",
            "/",
            "ANPAJYRXTHIB4FOVS3ZXS",
            "Provides full access to AWS services and resources, but does not allow management of Users and groups.",
            json!(["*"]),
        ),
        p(
            "ReadOnlyAccess",
            "/",
            "ANPAILL3HVNFSB6DCOWYQ",
            "Provides read-only access to AWS services and resources.",
            json!([
                "ec2:Describe*",
                "s3:Get*",
                "s3:List*",
                "iam:Get*",
                "iam:List*",
                "logs:Describe*",
                "logs:Get*",
                "logs:FilterLogEvents",
                "cloudwatch:Describe*",
                "cloudwatch:Get*",
                "cloudwatch:List*",
                "lambda:Get*",
                "lambda:List*",
                "sts:GetCallerIdentity"
            ]),
        ),
        p(
            "ViewOnlyAccess",
            "/job-function/",
            "ANPAID22R6XPJATWOFDK6",
            "This policy grants permissions to view resources and basic metadata across all AWS services.",
            json!([
                "ec2:Describe*",
                "s3:ListAllMyBuckets",
                "s3:ListBucket",
                "iam:List*",
                "logs:Describe*",
                "cloudwatch:Describe*",
                "cloudwatch:List*",
                "lambda:List*"
            ]),
        ),
        p(
            "SecurityAudit",
            "/",
            "ANPAIX2T3QCXHR2OGGCTO",
            "The security audit template grants access to read security configuration metadata.",
            json!([
                "iam:Get*",
                "iam:List*",
                "iam:GenerateCredentialReport",
                "s3:GetBucket*",
                "s3:ListAllMyBuckets",
                "ec2:Describe*",
                "logs:Describe*",
                "cloudwatch:Describe*"
            ]),
        ),
        p(
            "IAMFullAccess",
            "/",
            "ANPAI7XKCFMBPM3QQRRVQ",
            "Provides full access to IAM via the AWS Management Console.",
            json!(["iam:*", "organizations:DescribeAccount"]),
        ),
        p(
            "IAMReadOnlyAccess",
            "/",
            "ANPAJKSO7NDY4T57MWDSQ",
            "Provides read only access to IAM via the AWS Management Console.",
            json!(["iam:GenerateCredentialReport", "iam:Get*", "iam:List*"]),
        ),
        p(
            "IAMUserChangePassword",
            "/",
            "ANPAJ4L4MM2A7QIEB56MS",
            "Provides the ability for an IAM user to change their own password.",
            json!(["iam:ChangePassword", "iam:GetAccountPasswordPolicy"]),
        ),
        p(
            "AmazonS3FullAccess",
            "/",
            "ANPAIFIR6V6BVTRAHWINE",
            "Provides full access to all buckets via the AWS Management Console.",
            json!(["s3:*", "s3-object-lambda:*"]),
        ),
        p(
            "AmazonS3ReadOnlyAccess",
            "/",
            "ANPAIZTJ4DXE7G6AGAE6M",
            "Provides read only access to all buckets via the AWS Management Console.",
            json!([
                "s3:Get*",
                "s3:List*",
                "s3:Describe*",
                "s3-object-lambda:Get*",
                "s3-object-lambda:List*"
            ]),
        ),
        p(
            "AmazonEC2FullAccess",
            "/",
            "ANPAI3VAJF5ZCRZ7MCQE6",
            "Provides full access to Amazon EC2 via the AWS Management Console.",
            json!([
                "ec2:*",
                "elasticloadbalancing:*",
                "cloudwatch:*",
                "autoscaling:*"
            ]),
        ),
        p(
            "AmazonEC2ReadOnlyAccess",
            "/",
            "ANPAIGDT4SV4GSETWTBZK",
            "Provides read only access to Amazon EC2 via the AWS Management Console.",
            json!([
                "ec2:Describe*",
                "elasticloadbalancing:Describe*",
                "cloudwatch:ListMetrics",
                "cloudwatch:GetMetricStatistics",
                "cloudwatch:Describe*",
                "autoscaling:Describe*"
            ]),
        ),
        p(
            "CloudWatchFullAccess",
            "/",
            "ANPAIKEABORKUXN6DEAZU",
            "Provides full access to CloudWatch.",
            json!(["autoscaling:Describe*", "cloudwatch:*", "logs:*", "sns:*"]),
        ),
        p(
            "CloudWatchReadOnlyAccess",
            "/",
            "ANPAJN23PDQP7SZQAE3QE",
            "Provides read only access to CloudWatch.",
            json!([
                "autoscaling:Describe*",
                "cloudwatch:Describe*",
                "cloudwatch:Get*",
                "cloudwatch:List*",
                "logs:Get*",
                "logs:List*",
                "logs:StartQuery",
                "logs:StopQuery",
                "logs:Describe*",
                "logs:TestMetricFilter",
                "logs:FilterLogEvents",
                "sns:Get*",
                "sns:List*"
            ]),
        ),
        p(
            "CloudWatchLogsReadOnlyAccess",
            "/",
            "ANPAJ2YIYDYSNNEHK3VKW",
            "Provides read only access to CloudWatch Logs.",
            json!([
                "logs:Describe*",
                "logs:Get*",
                "logs:List*",
                "logs:StartQuery",
                "logs:StopQuery",
                "logs:TestMetricFilter",
                "logs:FilterLogEvents",
                "logs:StartLiveTail",
                "logs:StopLiveTail"
            ]),
        ),
        p(
            "AWSLambda_ReadOnlyAccess",
            "/",
            "ANPAZKAPJZG4P6AG6KFQB",
            "Grants read-only access to AWS Lambda service.",
            json!([
                "lambda:Get*",
                "lambda:List*",
                "logs:DescribeLogGroups",
                "logs:GetLogEvents"
            ]),
        ),
        p(
            "AWSLambda_FullAccess",
            "/",
            "ANPAZKAPJZG4MW35THVLF",
            "Grants full access to AWS Lambda service.",
            json!([
                "lambda:*",
                "logs:DescribeLogGroups",
                "logs:GetLogEvents",
                "iam:PassRole"
            ]),
        ),
        p(
            "AWSLambdaBasicExecutionRole",
            "/service-role/",
            "ANPAJNCQGXC42545SKXIK",
            "Provides write permissions to CloudWatch Logs.",
            json!([
                "logs:CreateLogGroup",
                "logs:CreateLogStream",
                "logs:PutLogEvents"
            ]),
        ),
        p(
            "AmazonDynamoDBReadOnlyAccess",
            "/",
            "ANPAIY2XFNA232XJ6J7X2",
            "Provides read only access to Amazon DynamoDB via the AWS Management Console.",
            json!([
                "dynamodb:BatchGetItem",
                "dynamodb:Describe*",
                "dynamodb:List*",
                "dynamodb:GetItem",
                "dynamodb:Query",
                "dynamodb:Scan"
            ]),
        ),
        p(
            "AWSBillingReadOnlyAccess",
            "/",
            "ANPAJ5WEO2EZLL3C2Q4HM",
            "Allows users to view bills on the Billing Console.",
            json!([
                "aws-portal:View*",
                "billing:Get*",
                "ce:Get*",
                "cur:Describe*"
            ]),
        ),
        p(
            "SupportUser",
            "/job-function/",
            "ANPAI3V4GSSN5SJY3P2RO",
            "This policy grants permissions to troubleshoot and resolve issues in an AWS account.",
            json!(["support:*", "ec2:Describe*", "iam:Get*", "iam:List*"]),
        ),
    ]
}

/// Every managed policy: built-in AWS managed ones (dataset entries override) and the dataset's own.
fn all_policies(ctx: &Ctx) -> Vec<Value> {
    let dataset = ctx.docs("policies");
    let mut out: Vec<Value> = aws_managed()
        .into_iter()
        .filter(|b| !dataset.iter().any(|(k, _)| *k == s(b, "Arn")))
        .collect();
    out.extend(dataset.into_iter().map(|(k, mut v)| {
        if v.get("Arn").is_none() {
            v["Arn"] = json!(k);
        }
        v
    }));
    out
}

fn find_policy(ctx: &Ctx, policy_arn: &str) -> Result<Value, (StatusCode, &'static str, String)> {
    all_policies(ctx)
        .into_iter()
        .find(|p| s(p, "Arn") == policy_arn)
        .ok_or_else(|| {
            not_found(format!(
                "Policy {policy_arn} does not exist or is not attachable."
            ))
        })
}

fn attachment_count(ctx: &Ctx, policy_arn: &str) -> usize {
    ["users", "groups", "roles"]
        .iter()
        .flat_map(|c| ctx.docs(c))
        .filter(|(_, e)| {
            array_of(e, "AttachedPolicies")
                .iter()
                .any(|a| a == policy_arn)
        })
        .count()
}

fn policy_xml(ctx: &Ctx, pol: &Value) -> String {
    let arn_s = s(pol, "Arn");
    let mut x = String::new();
    x += &tel("PolicyName", &s(pol, "PolicyName"));
    x += &tel("PolicyId", &s(pol, "PolicyId"));
    x += &tel("Arn", &arn_s);
    x += &tel("Path", &str_or(pol, "Path", "/"));
    x += &tel("DefaultVersionId", &str_or(pol, "DefaultVersionId", "v1"));
    x += &el(
        "AttachmentCount",
        &attachment_count(ctx, &arn_s).to_string(),
    );
    x += "<PermissionsBoundaryUsageCount>0</PermissionsBoundaryUsageCount>";
    x += &el(
        "IsAttachable",
        &pol.get("IsAttachable")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            .to_string(),
    );
    if let Some(d) = pol.get("Description").and_then(Value::as_str) {
        x += &tel("Description", d);
    }
    x += &tel("CreateDate", &ts(pol, "CreateDate").unwrap_or_default());
    x += &tel(
        "UpdateDate",
        &ts(pol, "UpdateDate")
            .or_else(|| ts(pol, "CreateDate"))
            .unwrap_or_default(),
    );
    x
}

fn list_policies(ctx: &mut Ctx, p: &Params) -> Out {
    let scope = p.get("Scope").unwrap_or("All");
    let only_attached = p.bool("OnlyAttached").unwrap_or(false);
    let prefix = p.get("PathPrefix").unwrap_or("/");
    let items = all_policies(ctx)
        .into_iter()
        .filter(|pol| {
            let aws = s(pol, "Arn").starts_with("arn:aws:iam::aws:");
            match scope {
                "AWS" => aws,
                "Local" => !aws,
                _ => true,
            }
        })
        .filter(|pol| str_or(pol, "Path", "/").starts_with(prefix))
        .filter(|pol| !only_attached || attachment_count(ctx, &s(pol, "Arn")) > 0)
        .map(|pol| policy_xml(ctx, &pol))
        .collect();
    Ok(Some(page(items, p, "Policies")))
}

fn get_policy(ctx: &mut Ctx, p: &Params) -> Out {
    let pol = find_policy(ctx, need(p, "PolicyArn")?)?;
    Ok(Some(el("Policy", &policy_xml(ctx, &pol))))
}

fn versions(pol: &Value) -> Vec<Value> {
    match pol.get("Versions").and_then(Value::as_array) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => vec![json!({
            "VersionId": str_or(pol, "DefaultVersionId", "v1"),
            "Document": pol.get("Document").cloned().unwrap_or(json!({})),
            "IsDefaultVersion": true,
            "CreateDate": pol.get("UpdateDate").or_else(|| pol.get("CreateDate")).cloned().unwrap_or(Value::Null),
        })],
    }
}

fn version_xml(v: &Value, with_doc: bool) -> String {
    let mut x = String::new();
    if with_doc {
        x += &tel(
            "Document",
            &percent_encode(&doc_text(v.get("Document").unwrap_or(&json!({})))),
        );
    }
    x += &tel("VersionId", &s(v, "VersionId"));
    x += &el(
        "IsDefaultVersion",
        &v.get("IsDefaultVersion")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .to_string(),
    );
    x += &tel("CreateDate", &ts(v, "CreateDate").unwrap_or_default());
    x
}

fn get_policy_version(ctx: &mut Ctx, p: &Params) -> Out {
    let policy_arn = need(p, "PolicyArn")?.to_owned();
    let pol = find_policy(ctx, &policy_arn)?;
    let vid = need(p, "VersionId")?;
    let v = versions(&pol)
        .into_iter()
        .find(|v| s(v, "VersionId") == vid)
        .ok_or_else(|| {
            not_found(format!(
                "Policy {policy_arn} version {vid} does not exist or is not attachable."
            ))
        })?;
    Ok(Some(el("PolicyVersion", &version_xml(&v, true))))
}

fn list_policy_versions(ctx: &mut Ctx, p: &Params) -> Out {
    let pol = find_policy(ctx, need(p, "PolicyArn")?)?;
    let items = versions(&pol)
        .iter()
        .map(|v| version_xml(v, false))
        .collect();
    Ok(Some(page(items, p, "Versions")))
}

fn create_policy(ctx: &mut Ctx, p: &Params) -> Out {
    let name = need(p, "PolicyName")?.to_owned();
    let doc = parse_doc(need(p, "PolicyDocument")?)?;
    let path = p.get("Path").unwrap_or("/").to_owned();
    let policy_arn = arn(ctx, "policy", &path, &name);
    if all_policies(ctx).iter().any(|x| s(x, "Arn") == policy_arn) {
        return Err(conflict(
            "EntityAlreadyExists",
            format!("A policy called {name} already exists. Duplicate names are not allowed."),
        ));
    }
    let pid = upper_id(ctx, "policy", "ANPA", 17);
    let mut pol = json!({
        "PolicyName": name, "PolicyId": pid, "Arn": policy_arn, "Path": path,
        "DefaultVersionId": "v1", "IsAttachable": true, "CreateDate": iso(ctx.now),
        "UpdateDate": iso(ctx.now), "Document": doc, "Tags": tags_param(p),
    });
    if let Some(d) = p.get("Description") {
        pol["Description"] = json!(d);
    }
    ctx.put("policies", &policy_arn, pol.clone());
    Ok(Some(el("Policy", &policy_xml(ctx, &pol))))
}

fn create_policy_version(ctx: &mut Ctx, p: &Params) -> Out {
    let policy_arn = need(p, "PolicyArn")?.to_owned();
    let doc = parse_doc(need(p, "PolicyDocument")?)?;
    let mut pol = ctx.get("policies", &policy_arn).ok_or_else(|| {
        not_found(format!(
            "Policy {policy_arn} does not exist or is not attachable."
        ))
    })?;
    let mut vs = versions(&pol);
    if vs.len() >= 5 {
        return Err(conflict(
            "LimitExceeded",
            "A managed policy can have up to 5 versions. Before you create a new version, you must delete an existing version.",
        ));
    }
    let n = vs
        .iter()
        .filter_map(|v| {
            s(v, "VersionId")
                .trim_start_matches('v')
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1;
    let set_default = p.bool("SetAsDefault").unwrap_or(false);
    if set_default {
        for v in &mut vs {
            v["IsDefaultVersion"] = json!(false);
        }
    }
    let nv = json!({"VersionId": format!("v{n}"), "Document": doc, "IsDefaultVersion": set_default, "CreateDate": iso(ctx.now)});
    vs.push(nv.clone());
    if set_default {
        pol["DefaultVersionId"] = json!(format!("v{n}"));
        pol["Document"] = nv["Document"].clone();
    }
    pol["Versions"] = Value::Array(vs);
    pol["UpdateDate"] = json!(iso(ctx.now));
    ctx.put("policies", &policy_arn, pol);
    Ok(Some(el("PolicyVersion", &version_xml(&nv, false))))
}

fn delete_policy(ctx: &mut Ctx, p: &Params) -> Out {
    let policy_arn = need(p, "PolicyArn")?.to_owned();
    if ctx.get("policies", &policy_arn).is_none() {
        return Err(not_found(format!(
            "Policy {policy_arn} does not exist or is not attachable."
        )));
    }
    if attachment_count(ctx, &policy_arn) > 0 {
        return Err(conflict(
            "DeleteConflict",
            "Cannot delete a policy attached to entities.",
        ));
    }
    ctx.remove("policies", &policy_arn);
    Ok(None)
}

fn list_entities_for_policy(ctx: &mut Ctx, p: &Params) -> Out {
    let policy_arn = need(p, "PolicyArn")?.to_owned();
    find_policy(ctx, &policy_arn)?;
    let mut x = String::new();
    for (collection, tag, name_key, id_key) in [
        ("groups", "PolicyGroups", "GroupName", "GroupId"),
        ("users", "PolicyUsers", "UserName", "UserId"),
        ("roles", "PolicyRoles", "RoleName", "RoleId"),
    ] {
        let members: String = ctx
            .docs(collection)
            .iter()
            .filter(|(_, e)| array_of(e, "AttachedPolicies").contains(&policy_arn))
            .map(|(_, e)| {
                el(
                    "member",
                    &format!(
                        "{}{}",
                        tel(name_key, &s(e, name_key)),
                        tel(id_key, &s(e, id_key))
                    ),
                )
            })
            .collect();
        x += &el(tag, &members);
    }
    x += "<IsTruncated>false</IsTruncated>";
    Ok(Some(x))
}

fn attach(ctx: &mut Ctx, collection: &str, name: &str, p: &Params, attach: bool) -> Out {
    let policy_arn = need(p, "PolicyArn")?.to_owned();
    let mut doc = entity(ctx, collection, name)?;
    let mut attached = array_of(&doc, "AttachedPolicies");
    if attach {
        let pol = find_policy(ctx, &policy_arn)?;
        if pol.get("IsAttachable") == Some(&json!(false)) {
            return Err(not_found(format!(
                "Policy {policy_arn} does not exist or is not attachable."
            )));
        }
        if !attached.contains(&policy_arn) {
            if attached.len() >= 10 {
                return Err(conflict(
                    "LimitExceeded",
                    format!(
                        "Cannot exceed quota for PoliciesPer{}: 10",
                        capitalize(kind_name(collection))
                    ),
                ));
            }
            attached.push(policy_arn);
        }
    } else {
        if !attached.contains(&policy_arn) {
            return Err(not_found(format!("Policy {policy_arn} was not found.")));
        }
        attached.retain(|a| *a != policy_arn);
    }
    doc["AttachedPolicies"] = json!(attached);
    ctx.put(collection, name, doc);
    Ok(None)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn list_attached(ctx: &mut Ctx, collection: &str, name: &str, p: &Params) -> Out {
    let doc = entity(ctx, collection, name)?;
    let all = all_policies(ctx);
    let prefix = p.get("PathPrefix").unwrap_or("/");
    let items = array_of(&doc, "AttachedPolicies")
        .iter()
        .filter_map(|a| all.iter().find(|pol| s(pol, "Arn") == *a))
        .filter(|pol| str_or(pol, "Path", "/").starts_with(prefix))
        .map(|pol| {
            format!(
                "{}{}",
                tel("PolicyName", &s(pol, "PolicyName")),
                tel("PolicyArn", &s(pol, "Arn"))
            )
        })
        .collect();
    Ok(Some(page(items, p, "AttachedPolicies")))
}

// ───────────────────────────── inline policies ─────────────────────────────

fn put_inline(ctx: &mut Ctx, collection: &str, name: &str, p: &Params) -> Out {
    let pname = need(p, "PolicyName")?.to_owned();
    let doc = parse_doc(need(p, "PolicyDocument")?)?;
    let mut e = entity(ctx, collection, name)?;
    let mut inline = e
        .get("InlinePolicies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    inline.insert(pname, doc);
    e["InlinePolicies"] = Value::Object(inline);
    ctx.put(collection, name, e);
    Ok(None)
}

fn inline_of(e: &Value) -> Map<String, Value> {
    e.get("InlinePolicies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn get_inline(ctx: &mut Ctx, collection: &str, name_key: &str, p: &Params) -> Out {
    let name = need(p, name_key)?.to_owned();
    let pname = need(p, "PolicyName")?.to_owned();
    let e = entity(ctx, collection, &name)?;
    let doc = inline_of(&e).get(&pname).cloned().ok_or_else(|| {
        not_found(format!(
            "The {} policy with name {pname} cannot be found.",
            kind_name(collection)
        ))
    })?;
    Ok(Some(format!(
        "{}{}{}",
        tel(name_key, &name),
        tel("PolicyName", &pname),
        tel("PolicyDocument", &percent_encode(&doc_text(&doc)))
    )))
}

fn delete_inline(ctx: &mut Ctx, collection: &str, name: &str, p: &Params) -> Out {
    let pname = need(p, "PolicyName")?.to_owned();
    let mut e = entity(ctx, collection, name)?;
    let mut inline = inline_of(&e);
    if inline.shift_remove(&pname).is_none() {
        return Err(not_found(format!(
            "The {} policy with name {pname} cannot be found.",
            kind_name(collection)
        )));
    }
    e["InlinePolicies"] = Value::Object(inline);
    ctx.put(collection, name, e);
    Ok(None)
}

fn list_inline(ctx: &mut Ctx, collection: &str, name: &str, p: &Params) -> Out {
    let e = entity(ctx, collection, name)?;
    let items = inline_of(&e).keys().map(|k| crate::util::esc(k)).collect();
    Ok(Some(page(items, p, "PolicyNames")))
}

// ───────────────────────────── access keys ─────────────────────────────

fn key_user(ctx: &Ctx, p: &Params) -> String {
    p.get("UserName").map(str::to_owned).unwrap_or_else(|| {
        ctx.docs("account")
            .first()
            .map(|(_, a)| s(a, "UserName"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "ops-automation".into())
    })
}

fn key_xml(k: &Value) -> String {
    format!(
        "{}{}{}{}",
        tel("UserName", &s(k, "UserName")),
        tel("AccessKeyId", &s(k, "AccessKeyId")),
        tel("Status", &str_or(k, "Status", "Active")),
        tel("CreateDate", &ts(k, "CreateDate").unwrap_or_default())
    )
}

fn create_access_key(ctx: &mut Ctx, p: &Params) -> Out {
    let user = key_user(ctx, p);
    entity(ctx, "users", &user)?;
    let count = ctx
        .docs("access_keys")
        .iter()
        .filter(|(_, k)| s(k, "UserName") == user)
        .count();
    if count >= 2 {
        return Err(conflict(
            "LimitExceeded",
            "Cannot exceed quota for AccessKeysPerUser: 2",
        ));
    }
    let id = upper_id(ctx, "access-key", "AKIA", 16);
    let secret = ctx.svc.ids.next_prefixed("access-key-secret", "", 40);
    let key = json!({"AccessKeyId": id, "UserName": user, "Status": "Active", "CreateDate": iso(ctx.now)});
    ctx.put("access_keys", &id, key.clone());
    Ok(Some(el(
        "AccessKey",
        &format!("{}{}", key_xml(&key), tel("SecretAccessKey", &secret)),
    )))
}

fn owned_key(ctx: &Ctx, p: &Params) -> Result<(String, Value), (StatusCode, &'static str, String)> {
    let id = need(p, "AccessKeyId")?.to_owned();
    let user = key_user(ctx, p);
    match ctx.get("access_keys", &id) {
        Some(k) if s(&k, "UserName") == user => Ok((id, k)),
        _ => Err(not_found(format!(
            "The Access Key with id {id} cannot be found."
        ))),
    }
}

fn delete_access_key(ctx: &mut Ctx, p: &Params) -> Out {
    if let Some(u) = p.get("UserName") {
        entity(ctx, "users", u)?;
    }
    let (id, _) = owned_key(ctx, p)?;
    ctx.remove("access_keys", &id);
    Ok(None)
}

fn update_access_key(ctx: &mut Ctx, p: &Params) -> Out {
    if let Some(u) = p.get("UserName") {
        entity(ctx, "users", u)?;
    }
    let status = need(p, "Status")?.to_owned();
    if status != "Active" && status != "Inactive" {
        return Err(invalid(format!(
            "1 validation error detected: Value '{status}' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive, Expired]"
        )));
    }
    let (id, mut k) = owned_key(ctx, p)?;
    k["Status"] = json!(status);
    ctx.put("access_keys", &id, k);
    Ok(None)
}

fn list_access_keys(ctx: &mut Ctx, p: &Params) -> Out {
    let user = key_user(ctx, p);
    entity(ctx, "users", &user)?;
    let items = ctx
        .docs("access_keys")
        .into_iter()
        .filter(|(_, k)| s(k, "UserName") == user)
        .map(|(_, k)| key_xml(&k))
        .collect();
    Ok(Some(page(items, p, "AccessKeyMetadata")))
}

fn access_key_last_used(ctx: &mut Ctx, p: &Params) -> Out {
    let id = need(p, "AccessKeyId")?.to_owned();
    let k = ctx.get("access_keys", &id).ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "AccessDenied",
            format!("The Access Key with id {id} cannot be found."),
        )
    })?;
    let inner = match ts(&k, "LastUsedDate") {
        Some(t) => format!(
            "{}{}{}",
            tel("LastUsedDate", &t),
            tel("ServiceName", &str_or(&k, "LastUsedService", "s3")),
            tel("Region", &str_or(&k, "LastUsedRegion", "us-east-1"))
        ),
        None => "<ServiceName>N/A</ServiceName><Region>N/A</Region>".into(),
    };
    Ok(Some(format!(
        "{}{}",
        tel("UserName", &s(&k, "UserName")),
        el("AccessKeyLastUsed", &inner)
    )))
}

fn list_mfa(ctx: &mut Ctx, p: &Params) -> Out {
    let user = key_user(ctx, p);
    let u = entity(ctx, "users", &user)?;
    let items = u
        .get("MFADevices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| {
            format!(
                "{}{}{}",
                tel("UserName", &user),
                tel("SerialNumber", &s(d, "SerialNumber")),
                tel("EnableDate", &ts(d, "EnableDate").unwrap_or_default())
            )
        })
        .collect();
    Ok(Some(page(items, p, "MFADevices")))
}

fn get_login_profile(ctx: &mut Ctx, p: &Params) -> Out {
    let user = need(p, "UserName")?.to_owned();
    let u = entity(ctx, "users", &user)?;
    match u.get("LoginProfile").filter(|v| v.is_object()) {
        Some(lp) => Ok(Some(el(
            "LoginProfile",
            &format!(
                "{}{}{}",
                tel("UserName", &user),
                tel("CreateDate", &ts(lp, "CreateDate").unwrap_or_default()),
                el(
                    "PasswordResetRequired",
                    &lp.get("PasswordResetRequired")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        .to_string()
                )
            ),
        ))),
        None => Err(not_found(format!(
            "Login Profile for User {user} cannot be found."
        ))),
    }
}

fn delete_login_profile(ctx: &mut Ctx, p: &Params) -> Out {
    let user = need(p, "UserName")?.to_owned();
    let mut u = entity(ctx, "users", &user)?;
    if u.get("LoginProfile").is_none_or(Value::is_null) {
        return Err(not_found(format!(
            "Login Profile for User {user} cannot be found."
        )));
    }
    u.as_object_mut()
        .expect("user is an object")
        .shift_remove("LoginProfile");
    ctx.put("users", &user, u);
    Ok(None)
}

fn account_summary(ctx: &mut Ctx) -> Out {
    let count = |c: &str| ctx.docs(c).len();
    let entries = [
        ("Users", count("users")),
        ("UsersQuota", 5000),
        ("Groups", count("groups")),
        ("GroupsQuota", 300),
        ("Roles", count("roles")),
        ("RolesQuota", 1000),
        ("Policies", count("policies")),
        ("PoliciesQuota", 1500),
        ("AccessKeysPerUserQuota", 2),
        ("AttachedPoliciesPerUserQuota", 10),
        ("AccountMFAEnabled", 1),
    ];
    let x: String = entries
        .iter()
        .map(|(k, v)| format!("<entry><key>{k}</key><value>{v}</value></entry>"))
        .collect();
    Ok(Some(el("SummaryMap", &x)))
}

// ───────────────────────────── simulation ─────────────────────────────

struct Source {
    id: String,
    kind: &'static str,
    doc: Value,
}

fn principal_policies(
    ctx: &Ctx,
    principal_arn: &str,
) -> Result<Vec<Source>, (StatusCode, &'static str, String)> {
    let all = all_policies(ctx);
    let managed = |e: &Value| -> Vec<Source> {
        array_of(e, "AttachedPolicies")
            .iter()
            .filter_map(|a| all.iter().find(|p| s(p, "Arn") == *a))
            .map(|p| Source {
                id: s(p, "PolicyName"),
                kind: if s(p, "Arn").starts_with("arn:aws:iam::aws:") {
                    "aws-managed"
                } else {
                    "user-managed"
                },
                doc: p.get("Document").cloned().unwrap_or(json!({})),
            })
            .collect()
    };
    let inline = |e: &Value, prefix: &str| -> Vec<Source> {
        inline_of(e)
            .into_iter()
            .map(|(k, v)| Source {
                id: format!("{prefix}_{k}"),
                kind: "IAM Policy",
                doc: v,
            })
            .collect()
    };
    for (collection, key) in [
        ("users", "UserName"),
        ("roles", "RoleName"),
        ("groups", "GroupName"),
    ] {
        for (_, e) in ctx.docs(collection) {
            let e_arn = e
                .get("Arn")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    arn(
                        ctx,
                        kind_name(collection),
                        &str_or(&e, "Path", "/"),
                        &s(&e, key),
                    )
                });
            if e_arn != principal_arn {
                continue;
            }
            let mut out = managed(&e);
            out.extend(inline(&e, &s(&e, key)));
            if collection == "users" {
                let name = s(&e, "UserName");
                for (_, g) in ctx.docs("groups") {
                    if array_of(&g, "Members").contains(&name) {
                        out.extend(managed(&g));
                        out.extend(inline(&g, &s(&g, "GroupName")));
                    }
                }
            }
            return Ok(out);
        }
    }
    Err((
        StatusCode::BAD_REQUEST,
        "InvalidInput",
        format!(
            "Invalid Entity Arn: {principal_arn} does not clearly define entity type and name."
        ),
    ))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn statement_matches(st: &Value, action: &str, resource: &str) -> bool {
    let action_ok = if st.get("NotAction").is_some() {
        !strings(st.get("NotAction"))
            .iter()
            .any(|a| glob_match(a, action))
    } else {
        strings(st.get("Action"))
            .iter()
            .any(|a| glob_match(a, action))
    };
    let resource_ok = if st.get("NotResource").is_some() {
        !strings(st.get("NotResource"))
            .iter()
            .any(|r| glob_match(r, resource))
    } else {
        let rs = strings(st.get("Resource"));
        rs.is_empty() || rs.iter().any(|r| glob_match(r, resource))
    };
    action_ok && resource_ok
}

fn simulate(ctx: &mut Ctx, p: &Params) -> Out {
    let principal = need(p, "PolicySourceArn")?.to_owned();
    let actions = p.list("ActionNames.member");
    if actions.is_empty() {
        return Err(invalid(
            "1 validation error detected: Value null at 'actionNames' failed to satisfy constraint: Member must not be null",
        ));
    }
    let mut resources = p.list("ResourceArns.member");
    if resources.is_empty() {
        resources.push("*".into());
    }
    let mut sources = principal_policies(ctx, &principal)?;
    for (i, extra) in p.list("PolicyInputList.member").iter().enumerate() {
        if let Ok(doc) = serde_json::from_str::<Value>(extra) {
            sources.push(Source {
                id: format!("PolicyInputList.{}", i + 1),
                kind: "IAM Policy",
                doc,
            });
        }
    }
    let mut results = String::new();
    for action in &actions {
        for resource in &resources {
            let mut decision = "implicitDeny";
            let mut matched = String::new();
            for src in &sources {
                for st in strings_statements(&src.doc) {
                    if !statement_matches(&st, action, resource) {
                        continue;
                    }
                    let effect = s(&st, "Effect");
                    matched += &el(
                        "member",
                        &format!(
                            "{}{}<StartPosition><Line>1</Line><Column>1</Column></StartPosition><EndPosition><Line>1</Line><Column>1</Column></EndPosition>",
                            tel("SourcePolicyId", &src.id),
                            tel("SourcePolicyType", src.kind)
                        ),
                    );
                    if effect == "Deny" {
                        decision = "explicitDeny";
                    } else if effect == "Allow" && decision != "explicitDeny" {
                        decision = "allowed";
                    }
                }
            }
            results += &el(
                "member",
                &format!(
                    "{}{}{}<MatchedStatements>{matched}</MatchedStatements><MissingContextValues/>",
                    tel("EvalActionName", action),
                    tel("EvalResourceName", resource),
                    tel("EvalDecision", decision)
                ),
            );
        }
    }
    Ok(Some(format!(
        "<EvaluationResults>{results}</EvaluationResults><IsTruncated>false</IsTruncated>"
    )))
}

fn strings_statements(doc: &Value) -> Vec<Value> {
    match doc.get("Statement") {
        Some(Value::Array(a)) => a.clone(),
        Some(o @ Value::Object(_)) => vec![o.clone()],
        _ => Vec::new(),
    }
}
