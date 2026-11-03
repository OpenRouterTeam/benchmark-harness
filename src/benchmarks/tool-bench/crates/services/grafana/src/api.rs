//! Core Grafana HTTP API: settings, health, users, search, dashboards, folders,
//! datasources, annotations, snapshots, short URLs, plugins, teams and rendering.

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{Ctx, PUBLIC_URL, Req, VERSION, error, not_found, ok, rfc3339, text};

pub(crate) fn route(ctx: &mut Ctx, req: &Req, segs: &[&str]) -> Response {
    let m = req.method.clone();
    match (m, &segs[1..]) {
        (Method::GET, ["frontend", "settings"]) => frontend_settings(ctx, req),
        (Method::GET, ["health"]) => {
            ok(json!({"commit": "9e10d5b7a4", "database": "ok", "version": VERSION}))
        }
        (Method::GET, ["user"]) => ok(ctx.user()),
        (Method::GET, ["user", "orgs"]) => ok(orgs(ctx)),
        (Method::GET, ["user", "teams"]) => ok(json!([])),
        (Method::GET, ["org"]) => {
            let o = orgs(ctx);
            ok(
                json!({"id": o[0]["orgId"], "name": o[0]["name"], "address": {"address1": "", "address2": "", "city": "", "zipCode": "", "state": "", "country": ""}}),
            )
        }
        (Method::GET, ["search"]) => search(ctx, req),
        (Method::GET, ["dashboards", "uid", uid]) => get_dashboard(ctx, uid),
        (Method::DELETE, ["dashboards", "uid", uid]) => delete_dashboard(ctx, uid),
        (Method::GET, ["dashboards", "uid", uid, "versions"]) => list_versions(ctx, req, uid),
        (Method::GET, ["dashboards", "uid", uid, "versions", v]) => get_version(ctx, uid, v),
        (Method::POST, ["dashboards", "db"]) => save_dashboard(ctx, req),
        (Method::GET, ["dashboards", "tags"]) => dashboard_tags(ctx),
        (Method::GET, ["folders"]) => list_folders(ctx, req),
        (Method::POST, ["folders"]) => create_folder(ctx, req),
        (Method::GET, ["folders", uid]) => match ctx.get("folders", uid) {
            Some(f) => ok(folder_json(ctx, &f)),
            None => folder_not_found(),
        },
        (Method::GET, ["datasources"]) => ok(Value::Array(
            ctx.coll("datasources")
                .into_iter()
                .map(|(_, d)| ds_list_item(&d))
                .collect(),
        )),
        (Method::POST, ["datasources"]) => create_datasource(ctx, req),
        (Method::GET, ["datasources", "uid", uid]) => match ctx.get("datasources", uid) {
            Some(d) => ok(ds_full(&d)),
            None => not_found("Data source not found"),
        },
        (Method::GET, ["datasources", "uid", uid, "health"]) => ds_health(ctx, uid),
        (Method::PUT, ["datasources", "uid", uid]) => update_datasource(ctx, req, uid),
        (Method::DELETE, ["datasources", "uid", uid]) => match ctx.remove("datasources", uid) {
            Some(d) => ok(json!({"id": d["id"], "message": "Data source deleted"})),
            None => not_found("Data source not found"),
        },
        (Method::GET, ["datasources", "name", name]) => {
            let name = crate::url_decode(name);
            match ctx
                .coll("datasources")
                .into_iter()
                .find(|(_, d)| d["name"] == name.as_str())
            {
                Some((_, d)) => ok(ds_full(&d)),
                None => not_found("Data source not found"),
            }
        }
        (Method::GET, ["datasources", id]) if id.parse::<i64>().is_ok() => {
            let id: i64 = id.parse().unwrap_or(0);
            match ctx
                .coll("datasources")
                .into_iter()
                .find(|(_, d)| d["id"].as_i64() == Some(id))
            {
                Some((_, d)) => ok(ds_full(&d)),
                None => not_found("Data source not found"),
            }
        }
        (Method::GET, ["annotations"]) => list_annotations(ctx, req),
        (Method::POST, ["annotations"]) => create_annotation(ctx, req, false),
        (Method::POST, ["annotations", "graphite"]) => create_annotation(ctx, req, true),
        (Method::GET, ["annotations", "tags"]) => annotation_tags(ctx, req),
        (Method::GET, ["annotations", id]) => match ctx.get("annotations", id) {
            Some(a) => ok(annotation_json(&a)),
            None => not_found("Annotation not found"),
        },
        (Method::PATCH, ["annotations", id]) | (Method::PUT, ["annotations", id]) => {
            update_annotation(ctx, req, id)
        }
        (Method::DELETE, ["annotations", id]) => match ctx.remove("annotations", id) {
            Some(_) => ok(json!({"message": "Annotation deleted"})),
            None => not_found("Annotation not found"),
        },
        (Method::POST, ["snapshots"]) => create_snapshot(ctx, req),
        (Method::GET, ["snapshots", key]) => match ctx.get("snapshots", key) {
            Some(s) => ok(json!({"dashboard": s["dashboard"], "meta": snapshot_meta(&s)})),
            None => not_found("Dashboard snapshot not found"),
        },
        (Method::DELETE, ["snapshots", key]) => match ctx.remove("snapshots", key) {
            Some(s) => ok(
                json!({"message": "Snapshot deleted. It might take an hour before it's cleared from any CDN caches.", "id": s["id"]}),
            ),
            None => not_found("Failed to get dashboard snapshot"),
        },
        (Method::GET, ["dashboard", "snapshots"]) => list_snapshots(ctx, req),
        (Method::POST, ["short-urls"]) => short_url(ctx, req),
        (Method::GET, ["plugins"]) => ok(Value::Array(
            ctx.coll("plugins")
                .into_iter()
                .map(|(_, p)| plugin_list_item(&p))
                .collect(),
        )),
        (Method::GET, ["plugins", "grafana-irm-app", "settings"]) => crate::irm::settings(ctx, req),
        (Method::GET, ["plugins", id, "settings"]) => match ctx.get("plugins", id) {
            Some(p) => ok(plugin_settings(&p)),
            None => not_found("Plugin not found, no installed plugin with that id"),
        },
        (Method::POST, ["plugins", id, "install"]) => install_plugin(ctx, req, id),
        (Method::POST, ["plugins", id, "uninstall"]) => match ctx.remove("plugins", id) {
            Some(_) => ok(json!({})),
            None => not_found("Plugin not installed"),
        },
        (_, ["plugins", id, "resources", ..]) => match ctx.get("plugins", id) {
            Some(_) => not_found("Not found"),
            None => not_found("Plugin not found"),
        },
        (Method::GET, ["teams", "search"]) => teams_search(ctx, req),
        (Method::GET, ["teams", id]) => match ctx.get("teams", id) {
            Some(t) => ok(t),
            None => not_found("Team not found"),
        },
        _ => not_found("Not found"),
    }
}

fn orgs(ctx: &Ctx) -> Value {
    let list = ctx.coll("orgs");
    if list.is_empty() {
        return json!([{"orgId": 1, "name": "Octo Corp", "role": "Editor"}]);
    }
    Value::Array(list.into_iter().map(|(_, o)| o).collect())
}

fn frontend_settings(ctx: &Ctx, _req: &Req) -> Response {
    let mut datasources = Map::new();
    let mut default_ds = Value::Null;
    for (_, d) in ctx.coll("datasources") {
        if d["isDefault"] == true {
            default_ds = d["name"].clone();
        }
        datasources.insert(
            d["name"].as_str().unwrap_or_default().to_owned(),
            json!({
                "type": d["type"], "name": d["name"], "meta": {"id": d["type"], "type": "datasource", "name": d["typeName"], "backend": true, "metrics": true, "logs": d["type"] == "loki", "tracing": d["type"] == "tempo", "alerting": d["type"] == "prometheus" || d["type"] == "loki"},
                "url": format!("/api/datasources/proxy/uid/{}", d["uid"].as_str().unwrap_or_default()),
                "isDefault": d["isDefault"], "access": d["access"], "id": d["id"], "uid": d["uid"],
                "jsonData": d["jsonData"], "readOnly": d["readOnly"], "apiVersion": "", "angularDetected": false,
            }),
        );
    }
    ok(json!({
        "allowOrgCreate": false,
        "appUrl": format!("{PUBLIC_URL}/"),
        "appSubUrl": "",
        "authProxyEnabled": false,
        "buildInfo": {"buildstamp": 1_760_000_000, "commit": "9e10d5b7a4", "commitShort": "9e10d5b", "edition": "Open Source", "env": "production", "hasUpdate": false, "hideVersion": false, "latestVersion": VERSION, "version": VERSION, "versionString": format!("Grafana v{VERSION} (9e10d5b7a4)")},
        "datasources": datasources,
        "defaultDatasource": default_ds,
        "featureToggles": {"kubernetesDashboards": false, "provisioning": false, "alertingSimplifiedRouting": true},
        "namespace": "default",
        "unifiedAlertingEnabled": true,
        "unifiedAlerting": {"minInterval": "10s", "alertStateHistoryBackend": "annotations"},
        "rendererAvailable": true,
        "rendererVersion": "3.12.1",
        "anonymousEnabled": false,
        "disableLoginForm": false,
    }))
}

// ───────────────────────────── search / dashboards / folders ─────────────────────────────

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn folder_title(ctx: &Ctx, uid: &str) -> Option<String> {
    ctx.get("folders", uid)
        .and_then(|f| f["title"].as_str().map(str::to_owned))
}

fn search(ctx: &Ctx, req: &Req) -> Response {
    let query = req.q("query").unwrap_or_default().to_lowercase();
    let typ = req.q("type");
    let tags: Vec<&str> = req.qs("tag");
    let uids: Vec<&str> = req.qs("dashboardUIDs");
    let folder_uids: Vec<&str> = req.qs("folderUIDs");
    let limit = req
        .q("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(1000)
        .clamp(1, 5000);
    let page = req
        .q("page")
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let mut hits = Vec::new();
    if typ.is_none_or(|t| t == "dash-folder") && tags.is_empty() && uids.is_empty() {
        for (uid, f) in ctx.coll("folders") {
            let title = f["title"].as_str().unwrap_or_default();
            if !query.is_empty() && !title.to_lowercase().contains(&query) {
                continue;
            }
            if !folder_uids.is_empty()
                && !f["parentUid"]
                    .as_str()
                    .is_some_and(|p| folder_uids.contains(&p))
            {
                continue;
            }
            let slug = slugify(title);
            let mut hit = json!({
                "id": f["id"], "uid": uid, "orgId": 1, "title": title, "uri": format!("db/{slug}"),
                "url": format!("/dashboards/f/{uid}/{slug}"), "slug": "", "type": "dash-folder", "tags": [], "isStarred": false, "sortMeta": 0, "isDeleted": false,
            });
            if let Some(p) = f["parentUid"].as_str() {
                hit["folderUid"] = json!(p);
                hit["folderTitle"] = json!(folder_title(ctx, p));
            }
            hits.push(hit);
        }
    }
    if typ.is_none_or(|t| t == "dash-db") {
        for (uid, d) in ctx.coll("dashboards") {
            let dash = &d["dashboard"];
            let title = dash["title"].as_str().unwrap_or_default();
            let dtags: Vec<String> = dash["tags"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if !query.is_empty() && !title.to_lowercase().contains(&query) {
                continue;
            }
            if !tags.iter().all(|t| dtags.iter().any(|d| d == t)) {
                continue;
            }
            if !uids.is_empty() && !uids.contains(&uid.as_str()) {
                continue;
            }
            let fuid = d["folderUid"].as_str().unwrap_or_default();
            if !folder_uids.is_empty()
                && !folder_uids.contains(&fuid)
                && !(fuid.is_empty() && folder_uids.contains(&"general"))
            {
                continue;
            }
            let slug = slugify(title);
            let mut hit = json!({
                "id": dash["id"], "uid": uid, "orgId": 1, "title": title, "uri": format!("db/{slug}"),
                "url": format!("/d/{uid}/{slug}"), "slug": "", "type": "dash-db", "tags": dtags, "isStarred": false, "sortMeta": 0, "isDeleted": false,
            });
            if !fuid.is_empty() {
                let ft = folder_title(ctx, fuid).unwrap_or_default();
                hit["folderId"] = ctx
                    .get("folders", fuid)
                    .map(|f| f["id"].clone())
                    .unwrap_or(Value::Null);
                hit["folderUid"] = json!(fuid);
                hit["folderTitle"] = json!(ft);
                hit["folderUrl"] = json!(format!("/dashboards/f/{fuid}/{}", slugify(&ft)));
            }
            hits.push(hit);
        }
    }
    hits.sort_by(|a, b| {
        let ka = (
            a["type"] != "dash-folder",
            a["title"].as_str().unwrap_or_default().to_lowercase(),
        );
        let kb = (
            b["type"] != "dash-folder",
            b["title"].as_str().unwrap_or_default().to_lowercase(),
        );
        ka.cmp(&kb)
    });
    let page_hits: Vec<Value> = hits
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .collect();
    ok(Value::Array(page_hits))
}

fn dashboard_meta(ctx: &Ctx, uid: &str, d: &Value) -> Value {
    let dash = &d["dashboard"];
    let title = dash["title"].as_str().unwrap_or_default();
    let slug = slugify(title);
    let fuid = d["folderUid"].as_str().unwrap_or_default();
    let ftitle = if fuid.is_empty() {
        "Dashboards".to_owned()
    } else {
        folder_title(ctx, fuid).unwrap_or_default()
    };
    json!({
        "type": "db", "canSave": true, "canEdit": true, "canAdmin": false, "canStar": true, "canDelete": true,
        "slug": slug, "url": format!("/d/{uid}/{slug}"), "expires": "0001-01-01T00:00:00Z",
        "created": d["created"], "updated": d["updated"], "updatedBy": d["updatedBy"], "createdBy": d["createdBy"],
        "version": dash["version"], "hasAcl": false, "isFolder": false,
        "folderId": if fuid.is_empty() { json!(0) } else { ctx.get("folders", fuid).map(|f| f["id"].clone()).unwrap_or(json!(0)) },
        "folderUid": fuid, "folderTitle": ftitle,
        "folderUrl": if fuid.is_empty() { String::new() } else { format!("/dashboards/f/{fuid}/{}", slugify(&ftitle)) },
        "provisioned": d["provisioned"].as_bool().unwrap_or(false), "provisionedExternalId": "",
        "annotationsPermissions": {"dashboard": {"canAdd": true, "canEdit": true, "canDelete": true}, "organization": {"canAdd": true, "canEdit": true, "canDelete": true}},
        "publicDashboardEnabled": false,
    })
}

fn get_dashboard(ctx: &Ctx, uid: &str) -> Response {
    match ctx.get("dashboards", uid) {
        Some(d) => ok(json!({"dashboard": d["dashboard"], "meta": dashboard_meta(ctx, uid, &d)})),
        None => not_found("Dashboard not found"),
    }
}

fn delete_dashboard(ctx: &mut Ctx, uid: &str) -> Response {
    match ctx.remove("dashboards", uid) {
        Some(d) => {
            let title = d["dashboard"]["title"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            ok(
                json!({"id": d["dashboard"]["id"], "message": format!("Dashboard {title} deleted"), "title": title}),
            )
        }
        None => not_found("Dashboard not found"),
    }
}

fn versions_of(ctx: &Ctx, uid: &str) -> Vec<Value> {
    let mut v: Vec<Value> = ctx
        .coll("dashboard_versions")
        .into_iter()
        .filter(|(k, _)| k.split_once('/').is_some_and(|(u, _)| u == uid))
        .map(|(_, v)| v)
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x["version"].as_i64().unwrap_or(0)));
    v
}

fn version_summary(uid: &str, dash_id: &Value, v: &Value) -> Value {
    json!({
        "id": v["id"], "dashboardId": dash_id, "dashboardUid": uid, "uid": uid,
        "parentVersion": v["version"].as_i64().unwrap_or(1) - 1, "restoredFrom": v["restoredFrom"].as_i64().unwrap_or(0),
        "version": v["version"], "created": v["created"], "createdBy": v["createdBy"], "message": v["message"].as_str().unwrap_or_default(),
    })
}

fn list_versions(ctx: &Ctx, req: &Req, uid: &str) -> Response {
    let Some(d) = ctx.get("dashboards", uid) else {
        return not_found("Dashboard not found");
    };
    let limit = req
        .q("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(1000);
    let start = req
        .q("start")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(0);
    let versions: Vec<Value> = versions_of(ctx, uid)
        .iter()
        .skip(start)
        .take(limit)
        .map(|v| version_summary(uid, &d["dashboard"]["id"], v))
        .collect();
    ok(json!({"continueToken": "", "versions": versions}))
}

fn get_version(ctx: &Ctx, uid: &str, v: &str) -> Response {
    let Some(d) = ctx.get("dashboards", uid) else {
        return not_found("Dashboard not found");
    };
    let n: i64 = v.parse().unwrap_or(-1);
    match versions_of(ctx, uid)
        .into_iter()
        .find(|x| x["version"].as_i64() == Some(n))
    {
        Some(x) => {
            let mut out = version_summary(uid, &d["dashboard"]["id"], &x);
            out["data"] = x["data"].clone();
            ok(out)
        }
        None => not_found("Dashboard version not found"),
    }
}

fn save_dashboard(ctx: &mut Ctx, req: &Req) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut dash = body["dashboard"].clone();
    if !dash.is_object() {
        return error(StatusCode::BAD_REQUEST, "bad request data");
    }
    let title = dash["title"].as_str().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "Dashboard title cannot be empty", "status": "empty-name"}),
        );
    }
    let overwrite = body["overwrite"].as_bool().unwrap_or(false);
    let folder_uid = body["folderUid"].as_str().map(str::to_owned);
    if let Some(f) = &folder_uid
        && !f.is_empty()
        && ctx.get("folders", f).is_none()
    {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "folder not found", "status": "folder-not-found"}),
        );
    }
    let uid = dash["uid"]
        .as_str()
        .filter(|u| !u.is_empty())
        .map(str::to_owned);
    let existing = uid.as_ref().and_then(|u| ctx.get("dashboards", u));
    // Same title in the same folder is a conflict for a new dashboard.
    let target_folder = folder_uid
        .clone()
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|e| e["folderUid"].as_str().map(str::to_owned))
        })
        .unwrap_or_default();
    if !overwrite
        && ctx.coll("dashboards").iter().any(|(k, d)| {
            Some(k) != uid.as_ref()
                && d["dashboard"]["title"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case(&title))
                && d["folderUid"].as_str().unwrap_or_default() == target_folder
        })
    {
        return json_status(
            StatusCode::PRECONDITION_FAILED,
            json!({"message": "A dashboard with the same name in the folder already exists", "status": "name-exists"}),
        );
    }
    let now = ctx.now_str();
    let login = ctx.login();
    let (uid, id, version, created, created_by, folder) = match (&uid, &existing) {
        (Some(u), Some(e)) => {
            let current = e["dashboard"]["version"].as_i64().unwrap_or(1);
            if !overwrite && dash["version"].as_i64().is_some_and(|v| v != current) {
                return json_status(
                    StatusCode::PRECONDITION_FAILED,
                    json!({"message": "The dashboard has been changed by someone else", "status": "version-mismatch"}),
                );
            }
            (
                u.clone(),
                e["dashboard"]["id"].clone(),
                current + 1,
                e["created"].clone(),
                e["createdBy"].clone(),
                folder_uid
                    .clone()
                    .unwrap_or_else(|| e["folderUid"].as_str().unwrap_or_default().to_owned()),
            )
        }
        _ => {
            let id = ctx
                .coll("dashboards")
                .iter()
                .filter_map(|(_, d)| d["dashboard"]["id"].as_i64())
                .max()
                .unwrap_or(0)
                + 1;
            let u = uid.clone().unwrap_or_else(|| ctx.new_uid("dashboard"));
            (
                u,
                json!(id),
                1,
                json!(now),
                json!(login),
                folder_uid.clone().unwrap_or_default(),
            )
        }
    };
    dash["uid"] = json!(uid);
    dash["id"] = id.clone();
    dash["version"] = json!(version);
    let doc = json!({
        "folderUid": folder, "created": created, "updated": now, "createdBy": created_by, "updatedBy": login,
        "dashboard": dash,
    });
    ctx.put("dashboards", &uid, doc);
    let vid = ctx.next_id("dashboard_versions", "id", 0);
    ctx.put(
        "dashboard_versions",
        &format!("{uid}/{version}"),
        json!({"id": vid, "version": version, "created": now, "createdBy": login, "message": body["message"].as_str().unwrap_or_default(), "data": dash}),
    );
    let slug = slugify(&title);
    ok(json!({
        "id": id, "uid": uid, "url": format!("/d/{uid}/{slug}"), "status": "success", "version": version, "slug": slug, "folderUid": folder,
    }))
}

fn json_status(code: StatusCode, v: Value) -> Response {
    mock_core::http::json(code, v)
}

fn dashboard_tags(ctx: &Ctx) -> Response {
    let mut counts: Vec<(String, i64)> = Vec::new();
    for (_, d) in ctx.coll("dashboards") {
        for t in d["dashboard"]["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            match counts.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => counts.push((t.to_owned(), 1)),
            }
        }
    }
    counts.sort();
    ok(Value::Array(
        counts
            .into_iter()
            .map(|(t, c)| json!({"term": t, "count": c}))
            .collect(),
    ))
}

fn folder_json(ctx: &Ctx, f: &Value) -> Value {
    let uid = f["uid"].as_str().unwrap_or_default();
    let title = f["title"].as_str().unwrap_or_default();
    let mut v = json!({
        "id": f["id"], "uid": uid, "orgId": 1, "title": title, "url": format!("/dashboards/f/{uid}/{}", slugify(title)),
        "hasAcl": false, "canSave": true, "canEdit": true, "canAdmin": true, "canDelete": true,
        "createdBy": f["createdBy"].as_str().unwrap_or("admin"), "created": f["created"], "updatedBy": f["updatedBy"].as_str().unwrap_or("admin"), "updated": f["updated"], "version": f["version"].as_i64().unwrap_or(1),
    });
    if let Some(p) = f["parentUid"].as_str() {
        v["parentUid"] = json!(p);
        v["parents"] = json!([ctx
            .get("folders", p)
            .map(|pf| folder_json(ctx, &pf))
            .unwrap_or(Value::Null)]);
    }
    v
}

fn folder_not_found() -> Response {
    json_status(
        StatusCode::NOT_FOUND,
        json!({"message": "folder not found", "messageId": "folder.notFound", "statusCode": 404, "traceID": ""}),
    )
}

fn list_folders(ctx: &Ctx, req: &Req) -> Response {
    let parent = req.q("parentUid");
    let limit = req
        .q("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(1000);
    let page = req
        .q("page")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let list: Vec<Value> = ctx
        .coll("folders")
        .into_iter()
        .filter(|(_, f)| f["parentUid"].as_str() == parent)
        .map(|(_, f)| json!({"id": f["id"], "uid": f["uid"], "title": f["title"]}))
        .skip((page - 1) * limit)
        .take(limit)
        .collect();
    ok(Value::Array(list))
}

fn create_folder(ctx: &mut Ctx, req: &Req) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let title = body["title"].as_str().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "folder title cannot be empty", "messageId": "folder.title-empty", "statusCode": 400, "traceID": ""}),
        );
    }
    let parent = body["parentUid"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    if let Some(p) = &parent
        && ctx.get("folders", p).is_none()
    {
        return folder_not_found();
    }
    if ctx.coll("folders").iter().any(|(_, f)| {
        f["title"]
            .as_str()
            .is_some_and(|t| t.eq_ignore_ascii_case(&title))
            && f["parentUid"].as_str() == parent.as_deref()
    }) {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "a folder with the same name already exists in the current location", "messageId": "folder.same-name", "statusCode": 409, "traceID": ""}),
        );
    }
    let uid = match body["uid"].as_str().filter(|u| !u.is_empty()) {
        Some(u) => {
            if ctx.get("folders", u).is_some() {
                return json_status(
                    StatusCode::CONFLICT,
                    json!({"message": "a folder with the same uid already exists", "messageId": "folder.uid-exists", "statusCode": 409, "traceID": ""}),
                );
            }
            u.to_owned()
        }
        None => ctx.new_uid("folder"),
    };
    let id = ctx.next_id("folders", "id", 0).max(
        ctx.coll("dashboards")
            .iter()
            .filter_map(|(_, d)| d["dashboard"]["id"].as_i64())
            .max()
            .unwrap_or(0)
            + 1,
    );
    let now = ctx.now_str();
    let login = ctx.login();
    let mut f = json!({"id": id, "uid": uid, "title": title, "created": now, "updated": now, "createdBy": login, "updatedBy": login, "version": 1});
    if let Some(p) = parent {
        f["parentUid"] = json!(p);
    }
    ctx.put("folders", &uid, f.clone());
    ok(folder_json(ctx, &f))
}

// ───────────────────────────── datasources ─────────────────────────────

fn ds_list_item(d: &Value) -> Value {
    json!({
        "id": d["id"], "uid": d["uid"], "orgId": 1, "name": d["name"], "type": d["type"], "typeName": d["typeName"],
        "typeLogoUrl": format!("public/app/plugins/datasource/{}/img/logo.svg", d["type"].as_str().unwrap_or_default()),
        "access": d["access"].as_str().unwrap_or("proxy"), "url": d["url"], "user": "", "database": "", "basicAuth": false,
        "isDefault": d["isDefault"].as_bool().unwrap_or(false), "jsonData": d["jsonData"].clone().as_object().cloned().unwrap_or_default(), "readOnly": d["readOnly"].as_bool().unwrap_or(false),
    })
}

fn ds_full(d: &Value) -> Value {
    json!({
        "id": d["id"], "uid": d["uid"], "orgId": 1, "name": d["name"], "type": d["type"],
        "typeLogoUrl": format!("public/app/plugins/datasource/{}/img/logo.svg", d["type"].as_str().unwrap_or_default()),
        "access": d["access"].as_str().unwrap_or("proxy"), "url": d["url"], "user": "", "database": "", "basicAuth": false, "basicAuthUser": "",
        "withCredentials": false, "isDefault": d["isDefault"].as_bool().unwrap_or(false), "jsonData": d["jsonData"].clone().as_object().cloned().unwrap_or_default(),
        "secureJsonFields": d["secureJsonFields"].clone().as_object().cloned().unwrap_or_default(), "version": d["version"].as_i64().unwrap_or(1), "readOnly": d["readOnly"].as_bool().unwrap_or(false),
        "apiVersion": "",
    })
}

fn type_name(t: &str) -> &'static str {
    match t {
        "prometheus" => "Prometheus",
        "loki" => "Loki",
        "tempo" => "Tempo",
        "grafana-pyroscope-datasource" => "Grafana Pyroscope",
        "alertmanager" => "Alertmanager",
        "elasticsearch" => "Elasticsearch",
        "grafana-postgresql-datasource" | "postgres" => "PostgreSQL",
        "mysql" => "MySQL",
        "cloudwatch" => "CloudWatch",
        "graphite" => "Graphite",
        "influxdb" => "InfluxDB",
        "jaeger" => "Jaeger",
        "zipkin" => "Zipkin",
        "testdata" => "TestData",
        _ => "Plugin",
    }
}

fn create_datasource(ctx: &mut Ctx, req: &Req) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = body["name"].as_str().unwrap_or_default().trim().to_owned();
    let typ = body["type"].as_str().unwrap_or_default().to_owned();
    if name.is_empty() || typ.is_empty() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "Required field missing", "traceID": ""}),
        );
    }
    if ctx
        .coll("datasources")
        .iter()
        .any(|(_, d)| d["name"] == name.as_str())
    {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "data source with the same name already exists", "traceID": ""}),
        );
    }
    let uid = body["uid"]
        .as_str()
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| ctx.new_uid("datasource"));
    if ctx.get("datasources", &uid).is_some() {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "data source with the same uid already exists", "traceID": ""}),
        );
    }
    let id = ctx.next_id("datasources", "id", 0);
    let secure: Map<String, Value> = body["secureJsonData"]
        .as_object()
        .map(|m| m.keys().map(|k| (k.clone(), json!(true))).collect())
        .unwrap_or_default();
    let d = json!({
        "id": id, "uid": uid, "name": name, "type": typ, "typeName": type_name(&typ), "access": body["access"].as_str().unwrap_or("proxy"),
        "url": body["url"].as_str().unwrap_or_default(), "isDefault": body["isDefault"].as_bool().unwrap_or(false),
        "jsonData": body["jsonData"].clone().as_object().cloned().unwrap_or_default(), "secureJsonFields": secure, "version": 1, "readOnly": false,
    });
    ctx.put("datasources", &uid, d.clone());
    ok(json!({"datasource": ds_full(&d), "id": id, "message": "Datasource added", "name": name}))
}

fn update_datasource(ctx: &mut Ctx, req: &Req, uid: &str) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(mut d) = ctx.get("datasources", uid) else {
        return not_found("Data source not found");
    };
    if d["readOnly"] == true {
        return json_status(
            StatusCode::FORBIDDEN,
            json!({"message": "Cannot update read-only data source", "traceID": ""}),
        );
    }
    for k in [
        "name",
        "url",
        "access",
        "isDefault",
        "jsonData",
        "type",
        "basicAuth",
        "user",
        "database",
    ] {
        if !body[k].is_null() {
            d[k] = body[k].clone();
        }
    }
    if let Some(m) = body["secureJsonData"].as_object() {
        for k in m.keys() {
            d["secureJsonFields"][k] = json!(true);
        }
    }
    d["version"] = json!(d["version"].as_i64().unwrap_or(1) + 1);
    ctx.put("datasources", uid, d.clone());
    ok(
        json!({"datasource": ds_full(&d), "id": d["id"], "message": "Datasource updated", "name": d["name"]}),
    )
}

fn ds_health(ctx: &Ctx, uid: &str) -> Response {
    let Some(d) = ctx.get("datasources", uid) else {
        return not_found("Data source not found");
    };
    if let Some(h) = d.get("health").filter(|h| h.is_object()) {
        let code = if h["status"] == "OK" {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
        return json_status(
            code,
            json!({"status": h["status"], "message": h["message"]}),
        );
    }
    let message = match d["type"].as_str().unwrap_or_default() {
        "prometheus" => "Successfully queried the Prometheus API.",
        "loki" => "Data source successfully connected.",
        "tempo" => "Data source is working",
        "grafana-pyroscope-datasource" => "Data source is working",
        "alertmanager" => "Health check passed.",
        _ => "Data source is working",
    };
    ok(json!({"status": "OK", "message": message}))
}

// ───────────────────────────── annotations ─────────────────────────────

fn annotation_json(a: &Value) -> Value {
    json!({
        "id": a["id"], "alertId": 0, "alertName": "", "dashboardId": a["dashboardId"].as_i64().unwrap_or(0), "dashboardUID": a["dashboardUID"].as_str().unwrap_or_default(),
        "panelId": a["panelId"].as_i64().unwrap_or(0), "userId": a["userId"].as_i64().unwrap_or(1), "newState": "", "prevState": "",
        "created": a["created"], "updated": a["updated"], "time": a["time"], "timeEnd": a["timeEnd"].as_i64().unwrap_or_else(|| a["time"].as_i64().unwrap_or(0)),
        "text": a["text"], "tags": a["tags"].clone().as_array().cloned().unwrap_or_default(), "login": a["login"].as_str().unwrap_or("admin"),
        "email": a["email"].as_str().unwrap_or("admin@localhost"), "avatarUrl": "/avatar/46d229b033af06a191ff2267bca9ae56", "data": {},
    })
}

fn list_annotations(ctx: &Ctx, req: &Req) -> Response {
    let from = req.q("from").and_then(|v| v.parse::<i64>().ok());
    let to = req.q("to").and_then(|v| v.parse::<i64>().ok());
    let duid = req.q("dashboardUID");
    let did = req.q("dashboardId").and_then(|v| v.parse::<i64>().ok());
    let panel = req.q("panelId").and_then(|v| v.parse::<i64>().ok());
    let tags = req.qs("tags");
    let match_any = req.q("matchAny") == Some("true");
    let typ = req.q("type");
    let limit = req
        .q("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let mut list: Vec<Value> = ctx
        .coll("annotations")
        .into_iter()
        .map(|(_, a)| a)
        .filter(|a| {
            let t = a["time"].as_i64().unwrap_or(0);
            let te = a["timeEnd"].as_i64().unwrap_or(t);
            let atags: Vec<&str> = a["tags"]
                .as_array()
                .map(|t| t.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            from.is_none_or(|f| te >= f)
                && to.is_none_or(|x| t <= x)
                && duid.is_none_or(|u| a["dashboardUID"].as_str() == Some(u))
                && did.is_none_or(|d| a["dashboardId"].as_i64() == Some(d))
                && panel.is_none_or(|p| a["panelId"].as_i64() == Some(p))
                && (tags.is_empty()
                    || if match_any {
                        tags.iter().any(|t| atags.contains(t))
                    } else {
                        tags.iter().all(|t| atags.contains(t))
                    })
                && typ.is_none_or(|ty| ty != "alert")
        })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a["time"].as_i64().unwrap_or(0)));
    ok(Value::Array(
        list.into_iter()
            .take(limit)
            .map(|a| annotation_json(&a))
            .collect(),
    ))
}

fn create_annotation(ctx: &mut Ctx, req: &Req, graphite: bool) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = if graphite {
        body["what"].as_str()
    } else {
        body["text"].as_str()
    }
    .unwrap_or_default()
    .to_owned();
    if text.is_empty() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": if graphite { "what field should not be empty" } else { "Failed to save annotation" }, "traceID": ""}),
        );
    }
    let now_ms = (ctx.now * 1000.0) as i64;
    let time = if graphite {
        body["when"].as_i64().map(|w| w * 1000)
    } else {
        body["time"].as_i64()
    }
    .filter(|t| *t > 0)
    .unwrap_or(now_ms);
    let time_end = body["timeEnd"].as_i64().filter(|t| *t > 0).unwrap_or(time);
    let mut dashboard_id = body["dashboardId"].as_i64().unwrap_or(0);
    let dashboard_uid = body["dashboardUID"].as_str().unwrap_or_default().to_owned();
    if !dashboard_uid.is_empty() {
        match ctx.get("dashboards", &dashboard_uid) {
            Some(d) => dashboard_id = d["dashboard"]["id"].as_i64().unwrap_or(0),
            None => return not_found("Dashboard not found"),
        }
    }
    let tags: Vec<Value> = if graphite {
        match &body["tags"] {
            Value::String(s) => s.split_whitespace().map(|t| json!(t)).collect(),
            Value::Array(a) => a.clone(),
            _ => vec![],
        }
    } else {
        body["tags"].as_array().cloned().unwrap_or_default()
    };
    let id = ctx.next_id("annotations", "id", 0);
    let user = ctx.user();
    let a = json!({
        "id": id, "dashboardId": dashboard_id, "dashboardUID": dashboard_uid, "panelId": body["panelId"].as_i64().unwrap_or(0),
        "userId": user["id"], "login": user["login"], "email": user["email"], "created": now_ms, "updated": now_ms,
        "time": time, "timeEnd": time_end, "text": text, "tags": tags,
    });
    ctx.put("annotations", &id.to_string(), a);
    ok(
        json!({"message": if graphite { "Graphite annotation added" } else { "Annotation added" }, "id": id}),
    )
}

fn update_annotation(ctx: &mut Ctx, req: &Req, id: &str) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(mut a) = ctx.get("annotations", id) else {
        return not_found("Annotation not found");
    };
    let patch = req.method == Method::PATCH;
    for k in ["text", "tags", "time", "timeEnd"] {
        if !body[k].is_null() {
            a[k] = body[k].clone();
        } else if !patch && k == "tags" {
            a[k] = json!([]);
        }
    }
    a["updated"] = json!((ctx.now * 1000.0) as i64);
    ctx.put("annotations", id, a);
    ok(json!({"message": if patch { "Annotation patched" } else { "Annotation updated" }}))
}

fn annotation_tags(ctx: &Ctx, req: &Req) -> Response {
    let filter = req.q("tag").unwrap_or_default().to_lowercase();
    let limit = req
        .q("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let mut counts: Vec<(String, i64)> = Vec::new();
    for (_, a) in ctx.coll("annotations") {
        for t in a["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !t.to_lowercase().contains(&filter) {
                continue;
            }
            match counts.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => counts.push((t.to_owned(), 1)),
            }
        }
    }
    counts.sort();
    let tags: Vec<Value> = counts
        .into_iter()
        .take(limit)
        .map(|(t, c)| json!({"tag": t, "count": c}))
        .collect();
    ok(json!({"result": {"tags": tags}}))
}

// ───────────────────────────── snapshots, short URLs ─────────────────────────────

fn snapshot_meta(s: &Value) -> Value {
    json!({
        "isSnapshot": true, "type": "snapshot", "canSave": false, "canEdit": false, "canAdmin": false, "canStar": false, "canDelete": false,
        "slug": "", "url": "", "expires": s["expires"], "created": s["created"], "updated": "0001-01-01T00:00:00Z", "updatedBy": "", "createdBy": "",
        "version": 0, "hasAcl": false, "isFolder": false, "folderId": 0, "folderUid": "", "folderTitle": "", "folderUrl": "", "provisioned": false, "provisionedExternalId": "",
    })
}

fn create_snapshot(ctx: &mut Ctx, req: &Req) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if !body["dashboard"].is_object() {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "bad request data", "traceID": ""}),
        );
    }
    let key = body["key"]
        .as_str()
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| ctx.state.ids.next_prefixed("snapshot-key", "", 32));
    let delete_key = body["deleteKey"]
        .as_str()
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| ctx.state.ids.next_prefixed("snapshot-delete", "", 32));
    let id = ctx.next_id("snapshots", "id", 0);
    let expires_secs = body["expires"].as_i64().unwrap_or(0);
    let expires = if expires_secs > 0 {
        rfc3339(ctx.now + expires_secs as f64)
    } else {
        "2126-10-30T00:00:00Z".to_owned()
    };
    let name = body["name"].as_str().map(str::to_owned).unwrap_or_else(|| {
        body["dashboard"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    });
    let mut dashboard = body["dashboard"].clone();
    dashboard["snapshot"] = json!({"timestamp": ctx.now_str()});
    let s = json!({
        "id": id, "key": key, "deleteKey": delete_key, "name": name, "orgId": 1, "userId": ctx.user()["id"], "external": false, "externalUrl": "",
        "expires": expires, "created": ctx.now_str(), "dashboard": dashboard,
    });
    ctx.put("snapshots", &key, s);
    ok(json!({
        "deleteKey": delete_key, "deleteUrl": format!("{PUBLIC_URL}/api/snapshots-delete/{delete_key}"), "id": id, "key": key, "url": format!("{PUBLIC_URL}/dashboard/snapshot/{key}"),
    }))
}

fn list_snapshots(ctx: &Ctx, req: &Req) -> Response {
    let q = req.q("query").unwrap_or_default().to_lowercase();
    let limit = req
        .q("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let list: Vec<Value> = ctx
        .coll("snapshots")
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| s["name"].as_str().unwrap_or_default().to_lowercase().contains(&q))
        .take(limit)
        .map(|s| json!({"id": s["id"], "name": s["name"], "key": s["key"], "orgId": 1, "userId": s["userId"], "external": false, "externalUrl": "", "expires": s["expires"], "created": s["created"], "updated": s["created"]}))
        .collect();
    ok(Value::Array(list))
}

fn short_url(ctx: &mut Ctx, req: &Req) -> Response {
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(path) = body["path"].as_str().filter(|p| !p.is_empty()) else {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"message": "Invalid short URL path", "traceID": ""}),
        );
    };
    let uid = ctx.new_uid("short-url");
    ctx.put(
        "short_urls",
        &uid,
        json!({"uid": uid, "path": path, "created": ctx.now_str()}),
    );
    ok(json!({"uid": uid, "url": format!("{PUBLIC_URL}/goto/{uid}?orgId=1")}))
}

// ───────────────────────────── plugins, teams ─────────────────────────────

fn plugin_list_item(p: &Value) -> Value {
    json!({
        "name": p["name"], "type": p["type"].as_str().unwrap_or("app"), "id": p["id"], "enabled": p["enabled"].as_bool().unwrap_or(true), "pinned": p["pinned"].as_bool().unwrap_or(false),
        "info": {"author": {"name": p["author"].as_str().unwrap_or("Grafana Labs"), "url": "https://grafana.com"}, "description": p["description"], "links": [], "logos": {"small": "", "large": ""}, "build": {}, "screenshots": null, "version": p["version"], "updated": p["updated"].as_str().unwrap_or("2026-09-01"), "keywords": []},
        "dependencies": {"grafanaDependency": ">=10.4.0", "grafanaVersion": "*", "plugins": []}, "latestVersion": p["version"], "hasUpdate": false,
        "defaultNavUrl": "", "category": "", "state": "", "signature": "valid", "signatureType": "grafana", "signatureOrg": "Grafana Labs", "accessControl": {}, "angularDetected": false,
    })
}

fn plugin_settings(p: &Value) -> Value {
    let mut v = plugin_list_item(p);
    v["module"] = json!(format!(
        "public/plugins/{}/module.js",
        p["id"].as_str().unwrap_or_default()
    ));
    v["baseUrl"] = json!(format!(
        "public/plugins/{}",
        p["id"].as_str().unwrap_or_default()
    ));
    v["includes"] = json!([]);
    v["jsonData"] = p["jsonData"]
        .clone()
        .as_object()
        .cloned()
        .map(Value::Object)
        .unwrap_or(json!({}));
    v["secureJsonFields"] = json!({});
    v["autoEnabled"] = json!(false);
    v
}

fn install_plugin(ctx: &mut Ctx, req: &Req, id: &str) -> Response {
    let body = req.json().unwrap_or(json!({}));
    if ctx.get("plugins", id).is_some() {
        return json_status(
            StatusCode::CONFLICT,
            json!({"message": "Plugin already installed", "traceID": ""}),
        );
    }
    let Some(catalog) = ctx.get("plugin_catalog", id) else {
        return json_status(
            StatusCode::NOT_FOUND,
            json!({"message": format!("Plugin not found (plugin \"{id}\" not found in the plugin catalog)"), "traceID": ""}),
        );
    };
    let mut p = catalog.clone();
    if let Some(v) = body["version"].as_str().filter(|v| !v.is_empty()) {
        p["version"] = json!(v);
    }
    p["id"] = json!(id);
    p["updated"] = json!(ctx.now_str()[..10].to_owned());
    ctx.put("plugins", id, p);
    ok(json!({}))
}

fn teams_search(ctx: &Ctx, req: &Req) -> Response {
    let q = req
        .q("query")
        .or(req.q("name"))
        .unwrap_or_default()
        .to_lowercase();
    let per_page = req
        .q("perpage")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let page = req
        .q("page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let all: Vec<Value> = ctx
        .coll("teams")
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| {
            t["name"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&q)
        })
        .collect();
    let total = all.len();
    let teams: Vec<Value> = all
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    ok(json!({"totalCount": total, "teams": teams, "page": page, "perPage": per_page}))
}

// ───────────────────────────── rendering ─────────────────────────────

/// A 4x2 RGBA PNG (fixed bytes): small, valid, and deterministic.
const PANEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x68, 0xd8, 0xf8,
    0x0f, 0x00, 0x04, 0x84, 0x02, 0x1e, 0x2d, 0x3a, 0x25, 0x7a, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

pub(crate) fn render(req: &Req) -> Response {
    if req.method != Method::GET {
        return not_found("Not found");
    }
    text(StatusCode::OK, "image/png", PANEL_PNG.to_vec())
}
